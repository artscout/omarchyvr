//! The OpenXR (Vulkan) side: a session that shows the [`Screen`] as one quad layer (no projection layer, so
//! everything else is black), head-locked or placed in front of the wearer at start.
//!
//! The quad swapchain is filled from a CPU rendered texture only when the screen changes. The session is a normal
//! main session, or with [`Opts::overlay`] an `XR_EXTX_overlay` session that sits on top of another main app
//! (WayVR, desk mode). Use the runtime Monado built with eye tracking OFF if the caller opens the eye chip itself.

use crate::screen::{self, Line, Screen, TEX_H, TEX_W};
use ash::vk::{self, Handle};
use openxr::sys::Handle as _;
use openxr as xr;
use pimax_lenstest::color;
use pimax_lenstest::image::default_threads;
use pimax_lenstest::xr::find_memory_type;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

/// Where the panel hangs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Follows the head (VIEW space): always straight ahead.
    HeadLocked,
    /// Fixed in the room (LOCAL space) where the head pointed (yaw only) when the first frame was drawn.
    PlacedAtStart,
}

/// Options for [`Hud::new`].
#[derive(Clone, Debug)]
pub struct Opts {
    pub placement: Placement,
    /// Distance of the panel in metres (default 1.5; width follows: 1 mm per texture pixel).
    pub distance: f32,
    /// Create an `XR_EXTX_overlay` session (needs the extension; a main app must run).
    pub overlay: bool,
    pub app_name: String,
    /// Read the controllers (and camera-hand pinch, when the runtime offers XR_EXT_hand_interaction): the trigger /
    /// pinch value and the aim pose of both hands, and the head, in STAGE space ([`HudHandle::input`]).
    pub input: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { placement: Placement::HeadLocked, distance: 1.5, overlay: false, app_name: "pimax-hud".into(), input: false }
    }
}

/// What [`Hud::frame`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameResult {
    /// A frame was submitted.
    Rendered,
    /// The session is not running yet (or is not focused/visible): nothing submitted, try again.
    Idle,
    /// The session ended (exit requested, runtime exit or loss): stop calling `frame`.
    Exit,
}

/// Anything that can show a screen: a [`HudHandle`], or a PNG writer in tests and `--simulate`.
pub trait Sink {
    fn show(&self, screen: Screen);
}

struct Shared {
    screen: Mutex<(Screen, u64)>,
    exit: AtomicBool,
    input: Mutex<Input>,
}

/// One hand's select input ([`Opts::input`]): trigger (Touch), select (simple controller) or pinch (hand interaction).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandInput {
    /// The action is bound to a device that is there.
    pub active: bool,
    /// 0..1 (trigger / pinch strength).
    pub value: f32,
    /// Aim pose position in STAGE space (m), when located.
    pub pos: Option<[f32; 3]>,
    /// Aim direction (-Z of the aim pose) in STAGE space, when located.
    pub dir: Option<[f32; 3]>,
}

/// The newest input sample ([`HudHandle::input`]); STAGE space (floor y = 0 under Monado's legacy origin).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Frames sampled so far (0 = nothing yet: the session is not running or not focused).
    pub samples: u64,
    /// Head position in STAGE space.
    pub head_pos: Option<[f32; 3]>,
    /// Head forward (-Z of the view) in STAGE space.
    pub head_dir: Option<[f32; 3]>,
    /// Left, right.
    pub hands: [HandInput; 2],
}

/// The OpenXR objects of [`Opts::input`].
struct InputXr {
    set: xr::ActionSet,
    select: xr::Action<f32>,
    hands: [xr::Path; 2],
    aim_spaces: Vec<xr::Space>,
    stage: xr::Space,
}

fn forward_of(q: xr::Quaternionf) -> [f32; 3] {
    // -Z rotated by q
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    [-(2.0 * (x * z + w * y)), -(2.0 * (y * z - w * x)), -(1.0 - 2.0 * (x * x + y * y))]
}

/// Cloneable, thread-safe handle: change what the panel shows from any thread.
#[derive(Clone)]
pub struct HudHandle {
    shared: Arc<Shared>,
}

impl Sink for HudHandle {
    fn show(&self, screen: Screen) {
        let mut g = self.shared.screen.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != screen {
            g.0 = screen;
            g.1 += 1;
        }
    }
}

impl HudHandle {
    fn update(&self, f: impl FnOnce(&mut Screen)) {
        let mut s = self.shared.screen.lock().unwrap_or_else(|e| e.into_inner()).0.clone();
        f(&mut s);
        self.show(s);
    }

    /// Plain white lines at the default size (keeps the progress bar).
    pub fn set_lines<S: AsRef<str>>(&self, lines: &[S]) {
        self.update(|s| s.lines = lines.iter().map(|l| Line::new(l.as_ref())).collect());
    }

    /// Styled lines (size, colour); keeps the progress bar.
    pub fn set_styled(&self, lines: Vec<Line>) {
        self.update(|s| s.lines = lines);
    }

    /// 0..=1 shows the bar, `None` hides it.
    pub fn set_progress(&self, p: Option<f32>) {
        self.update(|s| s.progress = p);
    }

    /// The newest controller / head sample (only with [`Opts::input`]; all empty otherwise).
    pub fn input(&self) -> Input {
        *self.shared.input.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ask the HUD to end its session: `frame` then returns [`FrameResult::Exit`].
    pub fn request_exit(&self) {
        self.shared.exit.store(true, Ordering::SeqCst);
    }
}

struct Upload {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
}

impl Upload {
    /// Copy a staging buffer into an image and leave it in COLOR_ATTACHMENT_OPTIMAL.
    unsafe fn copy(&self, staging: vk::Buffer, image: vk::Image, w: u32, h: u32) -> Result<(), String> {
        let d = &self.device;
        let err = |what: &str, r: vk::Result| format!("{what}: {r:?}");
        d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty()).map_err(|r| err("reset", r))?;
        d.begin_command_buffer(
            self.cmd,
            &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .map_err(|r| err("begin", r))?;
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let barrier = |old, new, src, dst| {
            vk::ImageMemoryBarrier::default()
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range)
                .old_layout(old)
                .new_layout(new)
                .src_access_mask(src)
                .dst_access_mask(dst)
        };
        d.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier(
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        let region = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D { width: w, height: h, depth: 1 },
        };
        d.cmd_copy_buffer_to_image(self.cmd, staging, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
        d.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier(
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )],
        );
        d.end_command_buffer(self.cmd).map_err(|r| err("end", r))?;
        let cmds = [self.cmd];
        d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)
            .map_err(|r| err("submit", r))?;
        d.wait_for_fences(&[self.fence], true, u64::MAX).map_err(|r| err("wait", r))?;
        d.reset_fences(&[self.fence]).map_err(|r| err("reset fence", r))
    }
}

/// The in-headset panel. Not `Send` (OpenXR frame calls stay on one thread); use [`Hud::handle`] elsewhere.
pub struct Hud {
    // Drop order: swapchain before session before instance.
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
    uploaded: Vec<u64>,
    stream: xr::FrameStream<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    view_space: xr::Space,
    local_space: Option<xr::Space>,
    session: xr::Session<xr::Vulkan>,
    instance: xr::Instance,
    device: ash::Device,
    upload: Upload,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    staging_len: usize,
    format: u32,
    opts: Opts,
    shared: Arc<Shared>,
    running: bool,
    exiting: bool,
    last_gen: u64,
    placed: Option<xr::Posef>,
    event_buf: xr::EventDataBuffer,
    frames: u64,
    input: Option<InputXr>,
}

fn vkerr(what: &str) -> impl Fn(vk::Result) -> String + '_ {
    move |r| format!("{what}: {r:?}")
}

impl Hud {
    /// Create the OpenXR instance, Vulkan device, session and panel swapchain. `XR_RUNTIME_JSON` selects the runtime; while a
    /// monado-service runs, any other runtime is refused ([`crate::runtime::check`]).
    pub fn new(opts: Opts) -> Result<Hud, String> {
        // SAFETY: the XR_KHR_vulkan_enable2 sequence of the openxr crate's example, as in pimax-lenstest/eyecal.
        unsafe { Hud::create(opts) }
    }

    unsafe fn create(opts: Opts) -> Result<Hud, String> {
        // never start SteamVR (or any other runtime) beside a running Monado (crate::runtime, 2026-10-16)
        crate::runtime::check()?;
        let entry = xr::Entry::load(&()).map_err(|e| format!("cannot load the OpenXR loader: {e}"))?;
        let exts = entry.enumerate_extensions().map_err(|e| format!("extensions: {e:?}"))?;
        if !exts.khr_vulkan_enable2 {
            return Err("the OpenXR runtime does not offer XR_KHR_vulkan_enable2".into());
        }
        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        if opts.overlay {
            if !exts.extx_overlay {
                return Err("the OpenXR runtime does not offer XR_EXTX_overlay".into());
            }
            enabled.extx_overlay = true;
        }
        let hand_interaction = opts.input && exts.ext_hand_interaction;
        enabled.ext_hand_interaction = hand_interaction;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: &opts.app_name,
                    application_version: 1,
                    engine_name: "pimax-hud",
                    engine_version: 1,
                    api_version: xr::Version::new(1, 0, 0),
                },
                &enabled,
                &[],
                &(),
            )
            .map_err(|e| format!("xrCreateInstance: {e:?} (is monado-service running, XR_RUNTIME_JSON set?)"))?;
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .map_err(|e| format!("no head mounted display: {e:?}"))?;
        let reqs = instance
            .graphics_requirements::<xr::Vulkan>(system)
            .map_err(|e| format!("graphics requirements: {e:?}"))?;
        if xr::Version::new(1, 1, 0) < reqs.min_api_version_supported {
            return Err(format!("the runtime wants Vulkan {}", reqs.min_api_version_supported));
        }
        let vk_entry = ash::Entry::load().map_err(|e| format!("cannot load libvulkan: {e}"))?;
        let app_info = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 1, 0));
        #[allow(clippy::missing_transmute_annotations)]
        let raw_instance = instance
            .create_vulkan_instance(
                system,
                std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
                &vk::InstanceCreateInfo::default().application_info(&app_info) as *const _ as *const _,
            )
            .map_err(|e| format!("xrCreateVulkanInstanceKHR: {e:?}"))?
            .map_err(|r| format!("vkCreateInstance: {:?}", vk::Result::from_raw(r)))?;
        let vk_instance = ash::Instance::load(vk_entry.static_fn(), vk::Instance::from_raw(raw_instance as _));
        let pd = vk::PhysicalDevice::from_raw(
            instance
                .vulkan_graphics_device(system, vk_instance.handle().as_raw() as _)
                .map_err(|e| format!("xrGetVulkanGraphicsDevice2KHR: {e:?}"))? as _,
        );
        let qf = vk_instance
            .get_physical_device_queue_family_properties(pd)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("no graphics queue")? as u32;
        let prio = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::default().queue_family_index(qf).queue_priorities(&prio)];
        #[allow(clippy::missing_transmute_annotations)]
        let raw_device = instance
            .create_vulkan_device(
                system,
                std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
                pd.as_raw() as _,
                &vk::DeviceCreateInfo::default().queue_create_infos(&qinfo) as *const _ as *const _,
            )
            .map_err(|e| format!("xrCreateVulkanDeviceKHR: {e:?}"))?
            .map_err(|r| format!("vkCreateDevice: {:?}", vk::Result::from_raw(r)))?;
        let device = ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _));
        let queue = device.get_device_queue(qf, 0);
        let mem_props = vk_instance.get_physical_device_memory_properties(pd);

        let binding = xr::sys::GraphicsBindingVulkanKHR {
            ty: xr::sys::GraphicsBindingVulkanKHR::TYPE,
            next: std::ptr::null(),
            instance: vk_instance.handle().as_raw() as _,
            physical_device: pd.as_raw() as _,
            device: device.handle().as_raw() as _,
            queue_family_index: qf,
            queue_index: 0,
        };
        // XR_EXTX_overlay: the create info is chained in front of the graphics binding.
        let overlay = xr::sys::SessionCreateInfoOverlayEXTX {
            ty: xr::sys::SessionCreateInfoOverlayEXTX::TYPE,
            next: &binding as *const _ as *const _,
            create_flags: xr::sys::OverlaySessionCreateFlagsEXTX::EMPTY,
            session_layers_placement: 1,
        };
        let next: *const std::ffi::c_void =
            if opts.overlay { &overlay as *const _ as *const _ } else { &binding as *const _ as *const _ };
        let info = xr::sys::SessionCreateInfo {
            ty: xr::sys::SessionCreateInfo::TYPE,
            next,
            create_flags: Default::default(),
            system_id: system,
        };
        let mut handle = xr::sys::Session::NULL;
        let r = (instance.fp().create_session)(instance.as_raw(), &info, &mut handle);
        if r.into_raw() < 0 {
            return Err(format!("xrCreateSession: {r:?}{}", if opts.overlay { " (overlay: is a main app running?)" } else { "" }));
        }
        let (session, waiter, stream) = xr::Session::<xr::Vulkan>::from_raw(instance.clone(), handle, Box::new(()));
        std::mem::forget(vk_entry); // the loader stays loaded for the process lifetime

        let view_space = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .map_err(|e| format!("view space: {e:?}"))?;
        let local_space = session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY).ok();
        let input = if opts.input { Some(create_input(&instance, &session, hand_interaction)?) } else { None };
        let offered: Vec<u32> =
            session.enumerate_swapchain_formats().map_err(|e| format!("swapchain formats: {e:?}"))?;
        let format = color::choose_format(&offered, false).ok_or("no 8-bit RGBA/BGRA swapchain format")?;
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | xr::SwapchainUsageFlags::TRANSFER_DST
                    | xr::SwapchainUsageFlags::SAMPLED,
                format,
                sample_count: 1,
                width: TEX_W,
                height: TEX_H,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })
            .map_err(|e| format!("panel swapchain: {e:?}"))?;
        let images: Vec<vk::Image> = swapchain
            .enumerate_images()
            .map_err(|e| format!("panel images: {e:?}"))?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect();
        let staging_len = TEX_W as usize * TEX_H as usize * 4;
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_len as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(vkerr("staging"))?;
        let req = device.get_buffer_memory_requirements(staging);
        let mt = find_memory_type(
            &mem_props.memory_types[..mem_props.memory_type_count as usize],
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or("no host visible memory for the panel")?;
        let staging_mem = device
            .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mt), None)
            .map_err(vkerr("staging memory"))?;
        device.bind_buffer_memory(staging, staging_mem, 0).map_err(vkerr("bind"))?;
        let staging_ptr = device
            .map_memory(staging_mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(vkerr("map"))? as *mut u8;
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qf)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(vkerr("command pool"))?;
        let cmd = device
            .allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1))
            .map_err(vkerr("command buffer"))?[0];
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(vkerr("fence"))?;
        let upload = Upload { device: device.clone(), queue, pool, cmd, fence };
        Ok(Hud {
            uploaded: vec![0; images.len()],
            swapchain,
            images,
            stream,
            waiter,
            view_space,
            local_space,
            session,
            instance,
            device,
            upload,
            staging,
            staging_mem,
            staging_ptr,
            staging_len,
            format,
            opts,
            shared: Arc::new(Shared {
                screen: Mutex::new((Screen::default(), 1)),
                exit: AtomicBool::new(false),
                input: Mutex::new(Input::default()),
            }),
            running: false,
            exiting: false,
            last_gen: 0,
            placed: None,
            event_buf: xr::EventDataBuffer::new(),
            frames: 0,
            input,
        })
    }

    /// A handle to change the screen from any thread.
    pub fn handle(&self) -> HudHandle {
        HudHandle { shared: self.shared.clone() }
    }

    pub fn set_lines<S: AsRef<str>>(&self, lines: &[S]) {
        self.handle().set_lines(lines);
    }

    pub fn set_progress(&self, p: Option<f32>) {
        self.handle().set_progress(p);
    }

    pub fn runtime_name(&self) -> String {
        self.instance.properties().map(|p| p.runtime_name).unwrap_or_default()
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Run one iteration of the frame loop: events, then (when running) wait, draw, submit. Blocks in
    /// `xrWaitFrame` while running, sleeps 20 ms while not. Call it in a loop until [`FrameResult::Exit`].
    pub fn frame(&mut self) -> Result<FrameResult, String> {
        if self.shared.exit.load(Ordering::SeqCst) && !self.exiting {
            self.exiting = true;
            if self.running {
                let _ = self.session.request_exit();
            } else {
                return Ok(FrameResult::Exit);
            }
        }
        while let Some(ev) =
            self.instance.poll_event(&mut self.event_buf).map_err(|e| format!("poll_event: {e:?}"))?
        {
            match ev {
                xr::Event::SessionStateChanged(e) => match e.state() {
                    xr::SessionState::READY => {
                        self.session.begin(VIEW_TYPE).map_err(|e| format!("xrBeginSession: {e:?}"))?;
                        self.running = true;
                    }
                    xr::SessionState::STOPPING => {
                        self.session.end().map_err(|e| format!("xrEndSession: {e:?}"))?;
                        self.running = false;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => return Ok(FrameResult::Exit),
                    _ => {}
                },
                xr::Event::InstanceLossPending(_) => return Ok(FrameResult::Exit),
                _ => {}
            }
        }
        if !self.running {
            std::thread::sleep(Duration::from_millis(20));
            return Ok(FrameResult::Idle);
        }
        let fs = self.waiter.wait().map_err(|e| format!("xrWaitFrame: {e:?}"))?;
        self.sample_input(fs.predicted_display_time);
        self.stream.begin().map_err(|e| format!("xrBeginFrame: {e:?}"))?;
        let screen = {
            let g = self.shared.screen.lock().unwrap_or_else(|e| e.into_inner());
            (g.0.clone(), g.1)
        };
        let show = fs.should_render && !screen.0.is_empty();
        if !show {
            self.stream
                .end(fs.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
            self.frames += 1;
            return Ok(FrameResult::Rendered);
        }
        if self.opts.placement == Placement::PlacedAtStart && self.placed.is_none() {
            self.placed = self.place(fs.predicted_display_time);
        }
        if screen.1 != self.last_gen {
            let img = screen::render(&screen.0);
            // SAFETY: staging maps staging_len bytes; no upload from it is in flight (copies wait on a fence).
            let out = unsafe { std::slice::from_raw_parts_mut(self.staging_ptr, self.staging_len) };
            img.to_upload(color::encoding_of(self.format), color::is_bgra(self.format), default_threads(), out);
            for p in out.chunks_exact_mut(4) {
                p[3] = if p[0] == 0 && p[1] == 0 && p[2] == 0 { 0 } else { 255 };
            }
            self.last_gen = screen.1;
        }
        let idx = self.swapchain.acquire_image().map_err(|e| format!("acquire: {e:?}"))? as usize;
        self.swapchain.wait_image(xr::Duration::INFINITE).map_err(|e| format!("wait image: {e:?}"))?;
        if self.uploaded[idx] != self.last_gen {
            // SAFETY: image of this swapchain, acquired and waited; staging holds a full texture.
            unsafe { self.upload.copy(self.staging, self.images[idx], TEX_W, TEX_H)? };
            self.uploaded[idx] = self.last_gen;
        }
        self.swapchain.release_image().map_err(|e| format!("release: {e:?}"))?;
        let (space, pose) = match (self.opts.placement, self.placed, self.local_space.as_ref()) {
            (Placement::PlacedAtStart, Some(p), Some(l)) => (l, p),
            _ => (
                &self.view_space,
                xr::Posef {
                    orientation: xr::Quaternionf::IDENTITY,
                    position: xr::Vector3f { x: 0.0, y: 0.0, z: -self.opts.distance },
                },
            ),
        };
        let quad = xr::CompositionLayerQuad::new()
            .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
            .space(space)
            .eye_visibility(xr::EyeVisibility::BOTH)
            .pose(pose)
            .size(xr::Extent2Df { width: TEX_W as f32 / 1000.0, height: TEX_H as f32 / 1000.0 })
            .sub_image(
                xr::SwapchainSubImage::new().swapchain(&self.swapchain).image_array_index(0).image_rect(
                    xr::Rect2Di {
                        offset: xr::Offset2Di { x: 0, y: 0 },
                        extent: xr::Extent2Di { width: TEX_W as i32, height: TEX_H as i32 },
                    },
                ),
            );
        self.stream
            .end(fs.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[&quad])
            .map_err(|e| format!("xrEndFrame: {e:?}"))?;
        self.frames += 1;
        Ok(FrameResult::Rendered)
    }

    /// Sync the actions and store the newest input sample (no-op without [`Opts::input`]). Errors (not focused yet)
    /// keep the previous sample.
    fn sample_input(&mut self, time: xr::Time) {
        let Some(inp) = &self.input else { return };
        if self.session.sync_actions(&[xr::ActiveActionSet::new(&inp.set)]).is_err() {
            return;
        }
        let mut s = *self.shared.input.lock().unwrap_or_else(|e| e.into_inner());
        let need = xr::SpaceLocationFlags::ORIENTATION_VALID | xr::SpaceLocationFlags::POSITION_VALID;
        for (i, h) in inp.hands.iter().enumerate() {
            let st = inp.select.state(&self.session, *h).ok();
            let loc = inp.aim_spaces[i].locate(&inp.stage, time).ok().filter(|l| l.location_flags.contains(need));
            s.hands[i] = HandInput {
                active: st.is_some_and(|x| x.is_active),
                value: st.filter(|x| x.is_active).map(|x| x.current_state).unwrap_or(0.0),
                pos: loc.map(|l| [l.pose.position.x, l.pose.position.y, l.pose.position.z]),
                dir: loc.map(|l| forward_of(l.pose.orientation)),
            };
        }
        let head = self.view_space.locate(&inp.stage, time).ok().filter(|l| l.location_flags.contains(need));
        s.head_pos = head.map(|l| [l.pose.position.x, l.pose.position.y, l.pose.position.z]);
        s.head_dir = head.map(|l| forward_of(l.pose.orientation));
        s.samples += 1;
        *self.shared.input.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Pose in LOCAL space: `distance` ahead of the head along its yaw, at head height.
    fn place(&self, time: xr::Time) -> Option<xr::Posef> {
        let local = self.local_space.as_ref()?;
        let loc = self.view_space.locate(local, time).ok()?;
        let need = xr::SpaceLocationFlags::ORIENTATION_VALID | xr::SpaceLocationFlags::POSITION_VALID;
        if !loc.location_flags.contains(need) {
            return None;
        }
        Some(placed_pose(loc.pose, self.opts.distance))
    }
}

/// The action set of [`Opts::input`]: one float "select" (Touch trigger, simple select, hand-interaction pinch) and
/// the aim pose of both hands, attached to the session; STAGE space for locating.
fn create_input(instance: &xr::Instance, session: &xr::Session<xr::Vulkan>, hand_interaction: bool) -> Result<InputXr, String> {
    let p = |s: &str| instance.string_to_path(s).map_err(|e| format!("path {s}: {e:?}"));
    let hands = [p("/user/hand/left")?, p("/user/hand/right")?];
    let set = instance.create_action_set("pimax_hud_input", "Pimax HUD input", 0).map_err(|e| format!("action set: {e:?}"))?;
    let select = set.create_action::<f32>("select", "Select (trigger or pinch)", &hands).map_err(|e| format!("select action: {e:?}"))?;
    let aim = set.create_action::<xr::Posef>("aim", "Aim pose", &hands).map_err(|e| format!("aim action: {e:?}"))?;
    let mut profiles: Vec<(&str, &str)> = vec![
        ("/interaction_profiles/oculus/touch_controller", "trigger/value"),
        ("/interaction_profiles/khr/simple_controller", "select/click"),
        ("/interaction_profiles/valve/index_controller", "trigger/value"),
    ];
    if hand_interaction {
        profiles.push(("/interaction_profiles/ext/hand_interaction_ext", "pinch_ext/value"));
    }
    for (prof, sel) in profiles {
        let mut b = Vec::new();
        for side in ["left", "right"] {
            b.push(xr::Binding::new(&select, p(&format!("/user/hand/{side}/input/{sel}"))?));
            b.push(xr::Binding::new(&aim, p(&format!("/user/hand/{side}/input/aim/pose"))?));
        }
        // a profile the runtime does not know is skipped, the others still count
        let _ = instance.suggest_interaction_profile_bindings(p(prof)?, &b);
    }
    session.attach_action_sets(&[&set]).map_err(|e| format!("attach actions: {e:?}"))?;
    let aim_spaces = hands
        .iter()
        .map(|h| aim.create_space(session, *h, xr::Posef::IDENTITY).map_err(|e| format!("aim space: {e:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    let stage = session
        .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
        .map_err(|e| format!("stage space: {e:?}"))?;
    Ok(InputXr { set, select, hands, aim_spaces, stage })
}

/// Yaw-only pose `distance` metres in front of `head` (head height), facing back at the head.
pub fn placed_pose(head: xr::Posef, distance: f32) -> xr::Posef {
    let q = head.orientation;
    // forward = rotate (0,0,-1) by q
    let fx = -2.0 * (q.x * q.z + q.w * q.y);
    let fz = -(1.0 - 2.0 * (q.x * q.x + q.y * q.y));
    let n = (fx * fx + fz * fz).sqrt();
    let (fx, fz) = if n < 1e-3 { (0.0, -1.0) } else { (fx / n, fz / n) };
    let yaw = (-fx).atan2(-fz); // rotation about +y that turns -z into (fx, fz)
    xr::Posef {
        orientation: xr::Quaternionf { x: 0.0, y: (yaw / 2.0).sin(), z: 0.0, w: (yaw / 2.0).cos() },
        position: xr::Vector3f { x: head.position.x + fx * distance, y: head.position.y, z: head.position.z + fz * distance },
    }
}

impl Drop for Hud {
    fn drop(&mut self) {
        // SAFETY: the device is idle after device_wait_idle; each object was created in `create`, destroyed once.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.unmap_memory(self.staging_mem);
            self.device.destroy_buffer(self.staging, None);
            self.device.free_memory(self.staging_mem, None);
            self.device.destroy_fence(self.upload.fence, None);
            self.device.destroy_command_pool(self.upload.pool, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(yaw: f32) -> xr::Posef {
        xr::Posef {
            orientation: xr::Quaternionf { x: 0.0, y: (yaw / 2.0).sin(), z: 0.0, w: (yaw / 2.0).cos() },
            position: xr::Vector3f { x: 1.0, y: 1.6, z: 2.0 },
        }
    }

    #[test]
    fn placed_in_front_of_the_head() {
        let p = placed_pose(pose(0.0), 1.5);
        assert!((p.position.x - 1.0).abs() < 1e-5 && (p.position.z - 0.5).abs() < 1e-5 && p.position.y == 1.6);
        // head turned 90 degrees left (yaw +90): forward is -x
        let p = placed_pose(pose(std::f32::consts::FRAC_PI_2), 1.5);
        assert!((p.position.x - -0.5).abs() < 1e-4 && (p.position.z - 2.0).abs() < 1e-4, "{:?}", p.position);
        // the panel faces the head: its orientation equals the head yaw
        assert!((p.orientation.y - (std::f32::consts::FRAC_PI_4).sin()).abs() < 1e-4);
    }

    #[test]
    fn looking_straight_down_falls_back_to_forward() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let down = xr::Posef {
            orientation: xr::Quaternionf { x: -h, y: 0.0, z: 0.0, w: h },
            position: xr::Vector3f { x: 0.0, y: 1.0, z: 0.0 },
        };
        let p = placed_pose(down, 1.0);
        assert!(p.position.x.abs() < 1e-3 && (p.position.z + 1.0).abs() < 1e-3);
    }

    #[test]
    fn handle_bumps_the_generation_only_on_change() {
        let shared = Arc::new(Shared { screen: Mutex::new((Screen::default(), 1)), exit: AtomicBool::new(false), input: Mutex::new(Input::default()) });
        let h = HudHandle { shared };
        h.set_lines(&["a"]);
        let g1 = h.shared.screen.lock().unwrap().1;
        h.set_lines(&["a"]);
        assert_eq!(h.shared.screen.lock().unwrap().1, g1);
        h.set_progress(Some(0.5));
        let s = h.shared.screen.lock().unwrap();
        assert_eq!(s.1, g1 + 1);
        assert_eq!(s.0.lines.len(), 1);
    }
}
