//! The OpenXR (Vulkan) session: swapchains, CPU-rendered patterns uploaded through a staging buffer, controller
//! actions (the same actions and bindings as `analysis/tools/ctrlcli.c`), the frame loop, capture mode.
//!
//! This is the only part that needs a runtime; everything it draws and decides comes from the pure modules.

use crate::capture::{self, Capture};
use crate::cli::RunOpts;
use crate::color::{self, Encoding};
use crate::gaze::{self, GazeTest, Reading as GazeReading};
use crate::gazedraw::{EyePose, Painter};
use crate::gazexr::{self, GazeInput};
use crate::hudquad;
use crate::geometry::{
    eyes_to_head, mat_mul, mat_vec, quat_mean, quat_to_mat, rot_y, transpose, yaw_deg, Fov, Mat3,
};
use crate::image::{default_threads, Image};
use crate::input::{Hand, InputMapper, RawInput};
use crate::patterns::swim::SwimView;
use crate::patterns::{self, Params, Pattern};
use crate::report::{self, RuntimeInfo};
use crate::state::{EyeSelect, RenderKey, State};
use crate::swimgpu::{self, SwimGpu};
use crate::timing::{self, Timing};
use crate::tune::{self, Tune};
use ash::vk::{self, Handle};
use openxr as xr;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Image size to render: the runtime's recommendation divided by `res_div`.
pub fn image_size(recommended: (u32, u32), res_div: u32) -> (u32, u32) {
    (
        (recommended.0 / res_div.max(1)).max(16),
        (recommended.1 / res_div.max(1)).max(16),
    )
}

/// Index of a host-visible, coherent memory type allowed by `bits`.
pub fn find_memory_type(
    types: &[vk::MemoryType],
    bits: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    types
        .iter()
        .enumerate()
        .position(|(i, t)| bits & (1 << i) != 0 && t.property_flags.contains(flags))
        .map(|i| i as u32)
}

/// What XR_KHR_visibility_mask returns for each view: vertex / index counts and the extent of the vertices.
fn visibility_lines(session: &xr::Session<xr::Vulkan>) -> Vec<String> {
    let kinds = [
        ("hidden", xr::VisibilityMaskTypeKHR::HIDDEN_TRIANGLE_MESH),
        ("visible", xr::VisibilityMaskTypeKHR::VISIBLE_TRIANGLE_MESH),
        ("line loop", xr::VisibilityMaskTypeKHR::LINE_LOOP),
    ];
    (0..2u32)
        .map(|view| {
            let parts: Vec<String> = kinds
                .iter()
                .map(|(name, kind)| match session.get_visibility_mask_khr(VIEW_TYPE, view, *kind) {
                    Ok(m) if !m.vertices.is_empty() => {
                        let (x0, x1) = m.vertices.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v.x), a.1.max(v.x)));
                        let (y0, y1) = m.vertices.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v.y), a.1.max(v.y)));
                        format!("{name} {} vertices / {} indices (x {x0:.2}..{x1:.2}, y {y0:.2}..{y1:.2})", m.vertices.len(), m.indices.len())
                    }
                    Ok(_) => format!("{name} empty"),
                    Err(e) => format!("{name} error {e:?}"),
                })
                .collect();
            format!("view {view}: {}", parts.join(", "))
        })
        .collect()
}

fn to_fov(f: xr::Fovf) -> Fov {
    Fov {
        left: f.angle_left,
        right: f.angle_right,
        up: f.angle_up,
        down: f.angle_down,
    }
}

/// Puts the terminal into single-key mode while alive (restored on drop). Does nothing if stdin is not a terminal.
struct RawTerminal {
    saved: Option<libc::termios>,
}

impl RawTerminal {
    fn enable() -> RawTerminal {
        // SAFETY: plain libc termios calls on fd 0 with a zeroed struct that tcgetattr fills in.
        unsafe {
            if libc::isatty(0) == 0 {
                return RawTerminal { saved: None };
            }
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return RawTerminal { saved: None };
            }
            let saved = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return RawTerminal { saved: None };
            }
            RawTerminal { saved: Some(saved) }
        }
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if let Some(t) = &self.saved {
            // SAFETY: restores the attributes read in `enable`.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, t);
            }
        }
    }
}

/// Characters typed in the terminal, read on a helper thread.
fn spawn_keyboard() -> mpsc::Receiver<char> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        let mut stdin = std::io::stdin().lock();
        while let Ok(1) = stdin.read(&mut b) {
            if tx.send(b[0] as char).is_err() {
                break;
            }
        }
    });
    rx
}

/// Controller actions, the same set as ctrlcli.c.
struct Controls {
    set: xr::ActionSet,
    hands: [xr::Path; 2],
    trigger: xr::Action<f32>,
    grip: xr::Action<f32>,
    a_or_x: xr::Action<bool>,
    b_or_y: xr::Action<bool>,
    stick: xr::Action<xr::Vector2f>,
    stick_click: xr::Action<bool>,
    menu: xr::Action<bool>,
}

impl Controls {
    fn new(instance: &xr::Instance) -> Result<Controls, String> {
        let e = |what: &str, r: xr::sys::Result| format!("{what}: {r:?}");
        let path = |s: &str| instance.string_to_path(s).map_err(|r| e(s, r));
        let hands = [path("/user/hand/left")?, path("/user/hand/right")?];
        let set = instance
            .create_action_set("ctl", "ctl", 0)
            .map_err(|r| e("create_action_set", r))?;
        let trigger = set
            .create_action::<f32>("trigger", "trigger", &hands)
            .map_err(|r| e("trigger", r))?;
        let grip = set
            .create_action::<f32>("grip", "grip", &hands)
            .map_err(|r| e("grip", r))?;
        let a_or_x = set
            .create_action::<bool>("a_or_x", "a_or_x", &hands)
            .map_err(|r| e("a_or_x", r))?;
        let b_or_y = set
            .create_action::<bool>("b_or_y", "b_or_y", &hands)
            .map_err(|r| e("b_or_y", r))?;
        let stick = set
            .create_action::<xr::Vector2f>("stick", "stick", &hands)
            .map_err(|r| e("stick", r))?;
        let stick_click = set
            .create_action::<bool>("stick_click", "stick_click", &hands)
            .map_err(|r| e("stick_click", r))?;
        let menu = set
            .create_action::<bool>("menu", "menu", &hands)
            .map_err(|r| e("menu", r))?;
        let p = |s: &str| path(s);
        let bindings = [
            xr::Binding::new(&trigger, p("/user/hand/left/input/trigger/value")?),
            xr::Binding::new(&trigger, p("/user/hand/right/input/trigger/value")?),
            xr::Binding::new(&grip, p("/user/hand/left/input/squeeze/value")?),
            xr::Binding::new(&grip, p("/user/hand/right/input/squeeze/value")?),
            xr::Binding::new(&a_or_x, p("/user/hand/left/input/x/click")?),
            xr::Binding::new(&a_or_x, p("/user/hand/right/input/a/click")?),
            xr::Binding::new(&b_or_y, p("/user/hand/left/input/y/click")?),
            xr::Binding::new(&b_or_y, p("/user/hand/right/input/b/click")?),
            xr::Binding::new(&stick, p("/user/hand/left/input/thumbstick")?),
            xr::Binding::new(&stick, p("/user/hand/right/input/thumbstick")?),
            xr::Binding::new(&stick_click, p("/user/hand/left/input/thumbstick/click")?),
            xr::Binding::new(&stick_click, p("/user/hand/right/input/thumbstick/click")?),
            xr::Binding::new(&menu, p("/user/hand/left/input/menu/click")?),
        ];
        instance
            .suggest_interaction_profile_bindings(
                path("/interaction_profiles/oculus/touch_controller")?,
                &bindings,
            )
            .map_err(|r| e("suggest_interaction_profile_bindings", r))?;
        Ok(Controls {
            set,
            hands,
            trigger,
            grip,
            a_or_x,
            b_or_y,
            stick,
            stick_click,
            menu,
        })
    }

    fn sample<G>(&self, session: &xr::Session<G>, extra: Option<&xr::ActionSet>) -> RawInput {
        let mut raw = RawInput::default();
        let mut active = vec![xr::ActiveActionSet::new(&self.set)];
        if let Some(e) = extra {
            active.push(xr::ActiveActionSet::new(e));
        }
        if session.sync_actions(&active).is_err() {
            return raw;
        }
        for (i, &h) in self.hands.iter().enumerate() {
            let mut hand = Hand::default();
            if let Ok(s) = self.trigger.state(session, h) {
                hand.active = s.is_active;
                hand.trigger = s.current_state;
            }
            if let Ok(s) = self.grip.state(session, h) {
                hand.grip = s.current_state;
            }
            if let Ok(s) = self.a_or_x.state(session, h) {
                hand.a = s.current_state;
            }
            if let Ok(s) = self.b_or_y.state(session, h) {
                hand.b = s.current_state;
            }
            if let Ok(s) = self.stick.state(session, h) {
                hand.stick = [s.current_state.x, s.current_state.y];
            }
            if let Ok(s) = self.stick_click.state(session, h) {
                hand.stick_click = s.current_state;
            }
            if let Ok(s) = self.menu.state(session, h) {
                hand.menu = s.current_state;
            }
            raw.hand[i] = hand;
        }
        raw
    }
}

/// One eye: swapchain, its Vulkan images, and the staging buffer the pattern is uploaded from.
struct Eye {
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
    /// Content generation last copied into each swapchain image.
    uploaded: Vec<u64>,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    staging_len: usize,
}

/// The head-locked text: a small swapchain of its own, submitted as a quad layer in VIEW space.
struct HudQuad {
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
    uploaded: Vec<u64>,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    staging_len: usize,
}

impl HudQuad {
    /// # Safety
    /// `device` and `mem_props` belong to the Vulkan device `session` was created with.
    unsafe fn new(
        session: &xr::Session<xr::Vulkan>,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        format: u32,
    ) -> Result<HudQuad, String> {
        let swapchain = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | xr::SwapchainUsageFlags::TRANSFER_DST
                    | xr::SwapchainUsageFlags::SAMPLED,
                format,
                sample_count: 1,
                width: hudquad::TEX_W,
                height: hudquad::TEX_H,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })
            .map_err(|e| format!("xrCreateSwapchain (text quad): {e:?}"))?;
        let images: Vec<vk::Image> = swapchain
            .enumerate_images()
            .map_err(|e| format!("text quad images: {e:?}"))?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect();
        let staging_len = hudquad::TEX_W as usize * hudquad::TEX_H as usize * 4;
        let buffer = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_len as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|r| format!("text quad staging buffer: {r:?}"))?;
        let req = device.get_buffer_memory_requirements(buffer);
        let mt = find_memory_type(
            &mem_props.memory_types[..mem_props.memory_type_count as usize],
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or("no host visible and coherent memory type")?;
        let mem = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mt),
                None,
            )
            .map_err(|r| format!("text quad staging memory: {r:?}"))?;
        device
            .bind_buffer_memory(buffer, mem, 0)
            .map_err(|r| format!("text quad bind: {r:?}"))?;
        let ptr = device
            .map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(|r| format!("text quad map: {r:?}"))? as *mut u8;
        Ok(HudQuad {
            uploaded: vec![0; images.len()],
            swapchain,
            images,
            staging: buffer,
            staging_mem: mem,
            staging_ptr: ptr,
            staging_len,
        })
    }

    /// Render `lines` into the staging buffer (text on a transparent background).
    fn set_text(&self, format: u32, lines: &[String]) {
        let img = hudquad::render(lines);
        // SAFETY: staging_ptr maps staging_len bytes of host-coherent memory owned by self; no upload from it is
        // in flight (every upload waits on its fence).
        let out = unsafe { std::slice::from_raw_parts_mut(self.staging_ptr, self.staging_len) };
        img.to_upload(
            color::encoding_of(format),
            color::is_bgra(format),
            default_threads(),
            out,
        );
        hudquad::alpha_from_black(out);
    }
}

struct Gpu {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
}

impl Gpu {
    /// Copy the staging buffer into the swapchain image and leave it in COLOR_ATTACHMENT_OPTIMAL (as OpenXR wants).
    unsafe fn upload(
        &self,
        staging: vk::Buffer,
        image: vk::Image,
        w: u32,
        h: u32,
    ) -> Result<(), String> {
        let d = &self.device;
        let err = |what: &str, r: vk::Result| format!("{what}: {r:?}");
        d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
            .map_err(|r| err("reset", r))?;
        d.begin_command_buffer(
            self.cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .map_err(|r| err("begin", r))?;
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_dst = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        d.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_dst],
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
            image_extent: vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
        };
        d.cmd_copy_buffer_to_image(
            self.cmd,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[region],
        );
        let to_color = vk::ImageMemoryBarrier::default()
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        d.cmd_pipeline_barrier(
            self.cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_color],
        );
        d.end_command_buffer(self.cmd).map_err(|r| err("end", r))?;
        let cmds = [self.cmd];
        d.queue_submit(
            self.queue,
            &[vk::SubmitInfo::default().command_buffers(&cmds)],
            self.fence,
        )
        .map_err(|r| err("submit", r))?;
        d.wait_for_fences(&[self.fence], true, u64::MAX)
            .map_err(|r| err("wait", r))?;
        d.reset_fences(&[self.fence])
            .map_err(|r| err("reset fence", r))?;
        Ok(())
    }
}

/// Everything the main loop needs from the XR/Vulkan setup.
struct Session {
    instance: xr::Instance,
    session: xr::Session<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    stream: xr::FrameStream<xr::Vulkan>,
    space: xr::Space,
    /// World-locked space for the swim pattern (None: the runtime refused it).
    local: Option<xr::Space>,
    blend: xr::EnvironmentBlendMode,
    gpu: Gpu,
    /// The swim pattern's shader pass (None: `--swim-cpu`, or the pipeline could not be built).
    swim: Option<SwimGpu>,
    /// The gaze action of the `gaze` pattern (None: not asked for, or not available: see `gaze_note`).
    gaze: Option<GazeInput>,
    gaze_note: Option<String>,
    eyes: [Eye; 2],
    /// The text as a quad layer (None: `--no-hud-quad`, capture mode, or the runtime refused it: the text is then
    /// baked into the eye images).
    hud: Option<HudQuad>,
    size: (u32, u32),
    format: u32,
    info: RuntimeInfo,
    controls: Controls,
    // keep the Vulkan loader and instance alive as long as the session
    _vk_entry: ash::Entry,
    _vk_instance: ash::Instance,
}

fn setup(opts: &RunOpts) -> Result<Session, String> {
    // SAFETY: the OpenXR loader and the Vulkan loader are system libraries; the calls below follow the
    // XR_KHR_vulkan_enable2 sequence of the openxr crate's own example.
    unsafe {
        let entry = xr::Entry::load(&())
            .map_err(|e| format!("cannot load the OpenXR loader (libopenxr_loader.so.1): {e}"))?;
        let exts = entry
            .enumerate_extensions()
            .map_err(|e| format!("enumerate extensions: {e:?}"))?;
        if !exts.khr_vulkan_enable2 {
            return Err("the OpenXR runtime does not offer XR_KHR_vulkan_enable2".into());
        }
        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        enabled.khr_visibility_mask = exts.khr_visibility_mask;
        let want_gaze = opts.patterns.contains(&Pattern::Gaze);
        enabled.ext_eye_gaze_interaction = want_gaze && exts.ext_eye_gaze_interaction;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo { application_name: "pimax-lenstest", application_version: 1, engine_name: "pimax-lenstest", engine_version: 1, api_version: xr::Version::new(1, 0, 0) },
                &enabled,
                &[],
                &(),
            )
            .map_err(|e| format!("xrCreateInstance failed: {e:?} (is monado-service running, XR_RUNTIME_JSON set?)"))?;
        let props = instance
            .properties()
            .map_err(|e| format!("instance properties: {e:?}"))?;
        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .map_err(|e| format!("no head mounted display: {e:?}"))?;
        let sys_props = instance
            .system_properties(system)
            .map_err(|e| format!("system properties: {e:?}"))?;
        let blend = instance
            .enumerate_environment_blend_modes(system, VIEW_TYPE)
            .map_err(|e| format!("blend modes: {e:?}"))?
            .first()
            .copied()
            .unwrap_or(xr::EnvironmentBlendMode::OPAQUE);

        let reqs = instance
            .graphics_requirements::<xr::Vulkan>(system)
            .map_err(|e| format!("graphics requirements: {e:?}"))?;
        let want = xr::Version::new(1, 1, 0);
        if want < reqs.min_api_version_supported
            || want.major() > reqs.max_api_version_supported.major()
        {
            return Err(format!(
                "the runtime wants Vulkan {} .. {}",
                reqs.min_api_version_supported, reqs.max_api_version_supported
            ));
        }
        let vk_entry = ash::Entry::load().map_err(|e| format!("cannot load libvulkan: {e}"))?;
        let app_info = vk::ApplicationInfo::default()
            .application_version(1)
            .engine_version(1)
            .api_version(vk::make_api_version(0, 1, 1, 0));
        #[allow(clippy::missing_transmute_annotations)]
        let raw_instance = instance
            .create_vulkan_instance(
                system,
                std::mem::transmute(vk_entry.static_fn().get_instance_proc_addr),
                &vk::InstanceCreateInfo::default().application_info(&app_info) as *const _
                    as *const _,
            )
            .map_err(|e| format!("xrCreateVulkanInstanceKHR: {e:?}"))?
            .map_err(|r| format!("vkCreateInstance: {:?}", vk::Result::from_raw(r)))?;
        let vk_instance = ash::Instance::load(
            vk_entry.static_fn(),
            vk::Instance::from_raw(raw_instance as _),
        );
        let pd = vk::PhysicalDevice::from_raw(
            instance
                .vulkan_graphics_device(system, vk_instance.handle().as_raw() as _)
                .map_err(|e| format!("xrGetVulkanGraphicsDevice2KHR: {e:?}"))? as _,
        );
        let qf = vk_instance
            .get_physical_device_queue_family_properties(pd)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("the Vulkan device has no graphics queue")? as u32;
        let prio = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(qf)
            .queue_priorities(&prio)];
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
        let device =
            ash::Device::load(vk_instance.fp_v1_0(), vk::Device::from_raw(raw_device as _));
        let queue = device.get_device_queue(qf, 0);
        let mem_props = vk_instance.get_physical_device_memory_properties(pd);

        let (session, waiter, stream) = instance
            .create_session::<xr::Vulkan>(
                system,
                &xr::vulkan::SessionCreateInfo {
                    instance: vk_instance.handle().as_raw() as _,
                    physical_device: pd.as_raw() as _,
                    device: device.handle().as_raw() as _,
                    queue_family_index: qf,
                    queue_index: 0,
                },
            )
            .map_err(|e| format!("xrCreateSession: {e:?}"))?;
        let space = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .map_err(|e| format!("view space: {e:?}"))?;
        let local = match session
            .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
        {
            Ok(l) => Some(l),
            Err(e) => {
                println!("warning: no LOCAL reference space ({e:?}): the swim pattern cannot be world-locked");
                None
            }
        };
        let controls = Controls::new(&instance)?;
        let mut gaze_note = None;
        let mut gaze = None;
        if want_gaze {
            match gazexr::probe(&instance, system, enabled.ext_eye_gaze_interaction)
                .and_then(|()| GazeInput::new(&instance))
            {
                Ok(g) => gaze = Some(g),
                Err(e) => gaze_note = Some(e),
            }
        }
        let mut sets = vec![&controls.set];
        if let Some(g) = &gaze {
            sets.push(g.set());
        }
        session
            .attach_action_sets(&sets)
            .map_err(|e| format!("attach action sets: {e:?}"))?;
        if let Some(g) = gaze.as_mut() {
            if let Err(e) = g.create_space(&session) {
                gaze_note = Some(e);
                gaze = None;
            }
        }

        let views = instance
            .enumerate_view_configuration_views(system, VIEW_TYPE)
            .map_err(|e| format!("view configuration: {e:?}"))?;
        if views.len() != 2 {
            return Err(format!(
                "expected a stereo view configuration, the runtime has {} views",
                views.len()
            ));
        }
        let recommended = (
            views[0].recommended_image_rect_width,
            views[0].recommended_image_rect_height,
        );
        let size = image_size(recommended, opts.res_div);

        let offered: Vec<u32> = session
            .enumerate_swapchain_formats()
            .map_err(|e| format!("swapchain formats: {e:?}"))?;
        let format = color::choose_format(&offered, opts.unorm).ok_or_else(|| {
            format!(
                "none of the 8-bit RGBA/BGRA swapchain formats is offered (offered: {offered:?})"
            )
        })?;

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qf)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|r| format!("command pool: {r:?}"))?;
        let cmd = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(1),
            )
            .map_err(|r| format!("command buffer: {r:?}"))?[0];
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|r| format!("fence: {r:?}"))?;
        let gpu = Gpu {
            device: device.clone(),
            queue,
            pool,
            cmd,
            fence,
        };

        let mut eyes = Vec::new();
        for _ in 0..2 {
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                        | xr::SwapchainUsageFlags::TRANSFER_DST
                        | xr::SwapchainUsageFlags::SAMPLED,
                    format,
                    sample_count: 1,
                    width: size.0,
                    height: size.1,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
                .map_err(|e| {
                    format!(
                        "xrCreateSwapchain {}x{} format {}: {e:?}",
                        size.0,
                        size.1,
                        color::format_name(format)
                    )
                })?;
            let images: Vec<vk::Image> = swapchain
                .enumerate_images()
                .map_err(|e| format!("swapchain images: {e:?}"))?
                .into_iter()
                .map(vk::Image::from_raw)
                .collect();
            let staging_len = size.0 as usize * size.1 as usize * 4;
            let buffer = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(staging_len as u64)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(|r| format!("staging buffer: {r:?}"))?;
            let req = device.get_buffer_memory_requirements(buffer);
            let mt = find_memory_type(
                &mem_props.memory_types[..mem_props.memory_type_count as usize],
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or("no host visible and coherent memory type")?;
            let mem = device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(mt),
                    None,
                )
                .map_err(|r| format!("staging memory: {r:?}"))?;
            device
                .bind_buffer_memory(buffer, mem, 0)
                .map_err(|r| format!("bind: {r:?}"))?;
            let ptr = device
                .map_memory(mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(|r| format!("map: {r:?}"))? as *mut u8;
            eyes.push(Eye {
                uploaded: vec![0; images.len()],
                swapchain,
                images,
                staging: buffer,
                staging_mem: mem,
                staging_ptr: ptr,
                staging_len,
            });
        }
        let eyes: [Eye; 2] = eyes.try_into().map_err(|_| "two eyes")?;

        let hud = if opts.hud_quad && opts.capture.is_none() {
            match HudQuad::new(&session, &device, &mem_props, format) {
                Ok(q) => Some(q),
                Err(e) => {
                    println!("warning: head-locked text quad unavailable ({e}); the text is baked into each eye's image");
                    None
                }
            }
        } else {
            None
        };

        let swim = if opts.swim_cpu {
            None
        } else {
            let imgs: [&[vk::Image]; 2] = [&eyes[0].images, &eyes[1].images];
            match SwimGpu::new(
                &device,
                queue,
                qf,
                &mem_props,
                vk::Format::from_raw(format as i32),
                size,
                imgs,
            ) {
                Ok(g) => Some(g),
                Err(e) => {
                    println!("warning: swim GPU pass unavailable ({e}); using the CPU renderer");
                    None
                }
            }
        };

        let info = RuntimeInfo {
            runtime: format!("{} {}", props.runtime_name, props.runtime_version),
            system: sys_props.system_name.clone(),
            views: views.len(),
            recommended,
            image: size,
            fov: [None, None],
            format: color::format_name(format),
            encoding: match color::encoding_of(format) {
                Encoding::Srgb => "sRGB: the bytes reach the panel unchanged".to_string(),
                Encoding::Linear => {
                    "UNORM: stored as linear light so that the panel gets the intended codes"
                        .to_string()
                }
            },
            controllers: "waiting for the controllers (wake them with a button press)".to_string(),
            visibility: if exts.khr_visibility_mask {
                visibility_lines(&session)
            } else {
                Vec::new()
            },
        };
        Ok(Session {
            instance,
            session,
            waiter,
            stream,
            space,
            local,
            blend,
            gpu,
            swim,
            gaze,
            gaze_note,
            eyes,
            hud,
            size,
            format,
            info,
            controls,
            _vk_entry: vk_entry,
            _vk_instance: vk_instance,
        })
    }
}

impl Session {
    /// Free the Vulkan objects this client created (staging buffers, command pool, fence). The device and instance
    /// belong to the runtime's session lifetime and are released with the process.
    fn destroy_gpu_objects(&mut self) {
        let d = &self.gpu.device;
        // SAFETY: every upload waits on its fence, so nothing is in flight; the handles were created in `setup` and
        // are destroyed exactly once, here.
        unsafe {
            let _ = d.device_wait_idle();
            for e in &self.eyes {
                d.unmap_memory(e.staging_mem);
                d.destroy_buffer(e.staging, None);
                d.free_memory(e.staging_mem, None);
            }
            if let Some(q) = &self.hud {
                d.unmap_memory(q.staging_mem);
                d.destroy_buffer(q.staging, None);
                d.free_memory(q.staging_mem, None);
            }
            if let Some(g) = self.swim.as_mut() {
                g.destroy();
            }
            d.destroy_fence(self.gpu.fence, None);
            d.destroy_command_pool(self.gpu.pool, None);
        }
    }
}

/// What the swim HUD strip texture of one eye was made from: lines, FOV, eye-to-head rotation.
type SwimHudSig = (Vec<String>, Fov, Option<Mat3>);

/// Per-eye inputs of the pixels that do not come from the key: head-relative rotation (cant), the world pose
/// of the eye for the swim pattern, the centre gain that normalises the tuning warp.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EyeExtra {
    head: Option<Mat3>,
    swim: Option<SwimView>,
    gain: f32,
}

fn quat_of(q: xr::Quaternionf) -> [f32; 4] {
    [q.x, q.y, q.z, q.w]
}

/// Room anchor: heading (yaw-only rotation) and position of the head when the room was placed.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    rot: Mat3,
    pos: [f32; 3],
}

impl Anchor {
    fn from_views(v: &[xr::View]) -> Anchor {
        let head = quat_to_mat(quat_mean(
            quat_of(v[0].pose.orientation),
            quat_of(v[1].pose.orientation),
        ));
        let (p0, p1) = (v[0].pose.position, v[1].pose.position);
        Anchor {
            rot: rot_y(yaw_deg(&head)),
            pos: [
                0.5 * (p0.x + p1.x),
                0.5 * (p0.y + p1.y),
                0.5 * (p0.z + p1.z),
            ],
        }
    }

    /// Pose of a view in the room frame.
    fn view(&self, v: &xr::View) -> SwimView {
        let inv = transpose(&self.rot);
        let p = v.pose.position;
        SwimView {
            rot: mat_mul(&inv, &quat_to_mat(quat_of(v.pose.orientation))),
            pos: mat_vec(
                &inv,
                [p.x - self.pos[0], p.y - self.pos[1], p.z - self.pos[2]],
            ),
        }
    }
}

/// The pattern parameters of one eye for `key` (shared by the CPU renderer and the swim shader).
fn eye_params(
    s: &Session,
    eye: usize,
    key: &RenderKey,
    fov: Fov,
    opts: &RunOpts,
    extra: &EyeExtra,
) -> Params {
    let (w, h) = s.size;
    let mut p = Params::new(w, h, fov, eye);
    p.step = key.step;
    p.square_deg = opts.square_deg;
    p.square_dir = key.dir;
    p.align_shift_deg = key.align.eye_shift_deg(eye);
    p.head = extra.head;
    if matches!(key.pattern, Pattern::Grid | Pattern::Swim) {
        p.warp = key.tune.warp(extra.gain);
    }
    if let Some(v) = extra.swim {
        p.view = v;
    }
    p
}

/// Render the image of `eye` for `key` and write it into the eye's staging buffer.
fn render_into_staging(
    s: &Session,
    eye: usize,
    key: &RenderKey,
    fov: Fov,
    opts: &RunOpts,
    hud: Option<&[String]>,
    extra: &EyeExtra,
) {
    let (w, h) = s.size;
    let threads = default_threads();
    let img: Image = if key.eye.shows(eye) {
        let p = eye_params(s, eye, key, fov, opts, extra);
        patterns::render(key.pattern, &p, hud, threads)
    } else {
        patterns::blank(w, h)
    };
    let e = &s.eyes[eye];
    // SAFETY: staging_ptr maps staging_len bytes of host-coherent memory owned by this Eye; it is only written here,
    // while no upload from it is in flight (every upload waits on its fence).
    let out = unsafe { std::slice::from_raw_parts_mut(e.staging_ptr, e.staging_len) };
    img.to_upload(
        color::encoding_of(s.format),
        color::is_bgra(s.format),
        threads,
        out,
    );
}

fn capture_env(opts: &RunOpts) -> Result<Option<(PathBuf, PathBuf)>, String> {
    if opts.capture.is_none() {
        return Ok(None);
    }
    let dump_dir = std::env::var("XRT_COMPOSITOR_DUMP_DIR").map_err(|_| "capture mode: monado-service must run with XRT_COMPOSITOR_DUMP_DIR=<dir> (and XRT_COMPOSITOR_DUMP_TRIGGER=<file>); this process needs the same variables".to_string())?;
    let trigger = std::env::var("XRT_COMPOSITOR_DUMP_TRIGGER").map_err(|_| "capture mode: XRT_COMPOSITOR_DUMP_TRIGGER=<file> is not set (use the same value for monado-service and for this client)".to_string())?;
    Ok(Some((PathBuf::from(dump_dir), PathBuf::from(trigger))))
}

/// Run the client until quit, `--seconds`, the end of the capture, or a signal.
pub fn run(opts: RunOpts) -> Result<(), String> {
    let cap_env = capture_env(&opts)?;
    let lens = report::load_lens(opts.lens_conf.as_deref());
    // SAFETY: installing a signal handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let mut s = setup(&opts)?;
    let (w, h) = s.size;

    let mut state = State::new(opts.patterns.clone());
    state.eye = opts.eye;
    state.dir = opts.dir;
    state.hud = opts.hud && opts.capture.is_none();
    for (i, cal) in state.align_cal.iter_mut().enumerate() {
        let e = lens.lens.eye(i);
        *cal = patterns::align::EyeCal {
            gain: e.base[1] * e.scale / e.ca[1],
            cx: e.cx,
            cy: e.cy,
        };
    }
    for (i, g) in state.tune_g.iter_mut().enumerate() {
        let e = lens.lens.eye(i);
        *g = e.base[1] * e.scale;
    }
    // the eye accuracy test (only when asked for: creating its space starts the eye chip in the service)
    let mut gaze_test: Option<GazeTest> = None;
    if opts.patterns.contains(&Pattern::Gaze) {
        let conv = pimax_eye::track::FrameConv::from_env();
        let mut g = GazeTest::new(opts.gaze_points, [conv.flip_x, conv.flip_y, conv.flip_z]);
        g.service_correction = gazexr::service_correction();
        g.unavailable = s.gaze_note.clone();
        match &g.unavailable {
            Some(why) => println!("gaze: NOT AVAILABLE: {why}\n  start the service with PIMAX_EYE=1 (analysis/tools/eye_accuracy.sh does)"),
            None => println!(
                "gaze: eye accuracy test ready ({} dot grid). Keys in this terminal: SPACE/a start-next, r restart, y accept the correction, q quit{}",
                opts.gaze_points,
                g.service_correction
                    .as_ref()
                    .map(|c| format!("\n  warning: the service applies a correction ({c}); run with PIMAX_EYE_CORRECTION=none to measure the raw tracker"))
                    .unwrap_or_default()
            ),
        }
        gaze_test = Some(g);
    }
    let mut gaze_painters: Option<[Painter; 2]> = None;
    let mut gaze_last: Option<(Vec<gaze::Mark>, [EyePose; 2], Vec<String>)> = None;
    let cant_deg = lens.lens.config().cant_deg;
    let head_mode = !opts.eye_axis && cant_deg != 0.0;
    println!(
        "patterns: {} (cant_deg {cant_deg:+.2}; --eye-axis for per-eye axes)",
        if head_mode {
            "grid, rings, channels and align are centred on the HEAD-FORWARD direction, the eye optical axis is the faint marker"
        } else {
            "centred on each eye's optical axis"
        }
    );
    let mut anchor: Option<Anchor> = None;
    let mut tune_printed = Tune::default();
    if let Some(sec) = opts.auto_secs {
        state.auto_secs = sec;
        state.auto_on = true;
    }
    if let Some(start) = opts.start {
        state.idx = state.patterns.iter().position(|&p| p == start).unwrap_or(0);
    }
    let mut mapper = InputMapper::new();
    let keys = spawn_keyboard();
    let _raw = RawTerminal::enable();

    // Capture plan: needs the FOV for the square steps, so it is set up on the first located frame.
    let mut capture: Option<Capture> = None;
    let mut capture_out: Option<PathBuf> = None;

    let mut event_buf = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit_requested = false;
    let mut exit_at: Option<Instant> = None;
    let start = Instant::now();
    let mut last = Instant::now();
    let mut fov: [Option<Fov>; 2] = [None, None];
    let mut rendered: Option<(RenderKey, [Fov; 2], [EyeExtra; 2])> = None;
    let mut head_rots: Option<[Mat3; 2]> = None;
    let mut head_reported = false;
    let mut content_gen = 0u64;
    let mut frames_since_complete = 0u32;
    let mut printed_key: Option<(Pattern, usize, EyeSelect, patterns::Dir)> = None;
    let mut reported = false;
    let mut profile_reported = false;
    let mut frame_count = 0u64;
    let mut timing =
        Timing::new(opts.timing || std::env::var_os("PIMAX_LENSTEST_TIMING").is_some());
    // swim on the GPU: the push constants of the images in `rendered`, and the HUD strip last uploaded
    let mut swim_pc = [[0.0f32; swimgpu::PC_FLOATS]; 2];
    let mut swim_active = false;
    let mut swim_hud_sig: [Option<SwimHudSig>; 2] = [None, None];
    // the text quad: what its staging buffer holds, and its content generation
    let mut quad_sig: Option<Vec<String>> = None;
    let mut quad_gen = 0u64;
    let quad_on = s.hud.is_some();
    if opts.hud && opts.capture.is_none() {
        println!(
            "text: {}",
            if quad_on {
                "head-locked quad layer 2 m ahead (one copy for both eyes)"
            } else {
                "baked into each eye's image, shifted to the head-forward direction so the two copies fuse"
            }
        );
    }
    let mut swim_hud_rows = [(0i32, 0i32); 2];

    'main: loop {
        if STOP.load(Ordering::Relaxed) && !exit_requested {
            println!("stopping (signal)");
            exit_requested = true;
            let _ = s.session.request_exit();
            exit_at = Some(Instant::now());
        }
        while let Some(ev) = s
            .instance
            .poll_event(&mut event_buf)
            .map_err(|e| format!("poll_event: {e:?}"))?
        {
            match ev {
                xr::Event::SessionStateChanged(e) => {
                    println!("session state: {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            s.session
                                .begin(VIEW_TYPE)
                                .map_err(|e| format!("xrBeginSession: {e:?}"))?;
                            running = true;
                        }
                        xr::SessionState::STOPPING => {
                            s.session
                                .end()
                                .map_err(|e| format!("xrEndSession: {e:?}"))?;
                            running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => break 'main,
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => break 'main,
                _ => {}
            }
        }
        if !running {
            if exit_at.is_some_and(|t| t.elapsed() > Duration::from_secs(5)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }

        let t_wait = Instant::now();
        let fs = s.waiter.wait().map_err(|e| format!("xrWaitFrame: {e:?}"))?;
        let t_waited = Instant::now();
        timing.add(timing::WAIT, t_waited - t_wait);
        s.stream
            .begin()
            .map_err(|e| format!("xrBeginFrame: {e:?}"))?;
        timing.add(timing::BEGIN, t_waited.elapsed());
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f32().min(0.25);
        last = now;
        if !fs.should_render {
            s.stream
                .end(fs.predicted_display_time, s.blend, &[])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
            continue;
        }
        let t_loc = Instant::now();
        let (_, views) = s
            .session
            .locate_views(VIEW_TYPE, fs.predicted_display_time, &s.space)
            .map_err(|e| format!("xrLocateViews: {e:?}"))?;
        timing.add(timing::LOCATE, t_loc.elapsed());
        for (i, v) in views.iter().enumerate().take(2) {
            fov[i] = Some(to_fov(v.fov));
        }
        s.info.fov = fov;
        if views.len() >= 2 {
            let m = eyes_to_head(
                quat_of(views[0].pose.orientation),
                quat_of(views[1].pose.orientation),
            );
            head_rots = Some(m);
            if !head_reported {
                let yaw = |m: &Mat3| yaw_deg(&transpose(m));
                println!(
                    "view poses: left eye yaw {:+.2} deg, right eye yaw {:+.2} deg relative to the head (lens.conf cant_deg {cant_deg:+.2})",
                    yaw(&m[0]),
                    yaw(&m[1])
                );
                if head_mode && (yaw(&m[0]) - cant_deg).abs() > 0.1 {
                    println!("warning: the runtime's view poses do not show the configured cant; the patterns follow the poses");
                }
                head_reported = true;
            }
        }
        let (fovs, both) = match (fov[0], fov[1]) {
            (Some(a), Some(b)) => ([a, b], true),
            _ => (
                [Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5); 2],
                false,
            ),
        };
        let steps_for = |p: Pattern, dir: patterns::Dir| -> usize {
            let mut q = Params::new(w, h, fovs[0], 0);
            q.square_deg = opts.square_deg;
            q.square_dir = dir;
            p.steps(&q)
        };

        // controllers and keyboard
        if !profile_reported {
            let names: Vec<String> = s
                .controls
                .hands
                .iter()
                .map(|&h| match s.session.current_interaction_profile(h) {
                    Ok(p) if p != xr::Path::NULL => {
                        s.instance.path_to_string(p).unwrap_or_else(|_| "?".into())
                    }
                    _ => "none".to_string(),
                })
                .collect();
            if names.iter().any(|n| n != "none") {
                s.info.controllers = format!("left {} | right {}", names[0], names[1]);
                println!("controllers: {}", s.info.controllers);
                profile_reported = true;
            }
        }
        let raw = s
            .controls
            .sample(&s.session, s.gaze.as_ref().map(GazeInput::set));
        let mut events = mapper.update(&raw);
        while let Ok(c) = keys.try_recv() {
            if let Some(e) = crate::input::key_event(c) {
                events.push(e);
            }
        }

        if both && capture.is_none() && capture_out.is_none() {
            if let (Some(out), Some((dump_dir, trigger))) = (&opts.capture, &cap_env) {
                let tasks = capture::plan(&state.patterns, &|p| steps_for(p, opts.dir));
                let mut p0 = Params::new(w, h, fovs[0], 0);
                p0.square_deg = opts.square_deg;
                p0.square_dir = opts.dir;
                let names: Vec<String> = tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| capture::file_stem(i, t, &t.pattern.step_name(&p0, t.step)))
                    .collect();
                println!("capture: {} captures into {}", tasks.len(), out.display());
                capture = Some(
                    Capture::new(
                        tasks,
                        names,
                        trigger.clone(),
                        dump_dir.clone(),
                        out.clone(),
                        opts.settle_frames,
                        opts.down,
                        opts.keep_raw,
                    )
                    .map_err(|e| format!("capture setup: {e}"))?,
                );
                capture_out = Some(out.clone());
                // the config next to the pictures, so each picture can be matched with it later
                let _ = std::fs::write(
                    out.join("config.txt"),
                    report::runtime_report(&s.info, &lens, &state, &opts),
                );
            }
        }
        if let Some(cap) = capture.as_mut() {
            if let Some(task) = cap.current() {
                // the capture drives the state
                if let Some(i) = state.patterns.iter().position(|&p| p == task.pattern) {
                    state.idx = i;
                }
                state.step = task.step;
                state.eye = task.eye;
                state.hud = false;
                state.auto_on = false;
            }
        } else {
            let align_before = (state.align, state.pattern());
            for e in events {
                if let (Pattern::Gaze, Some(gt)) = (state.pattern(), gaze_test.as_mut()) {
                    if let Some(cmd) = gazexr::command_of(e) {
                        for fx in gt.command(cmd) {
                            println!("{}", gaze::execute(&fx, opts.gaze_out.as_deref(), None));
                        }
                        continue;
                    }
                }
                state.handle(e, &|p, d| steps_for(p, d));
            }
            if state.pattern() == Pattern::Align && align_before != (state.align, state.pattern()) {
                println!(
                    "{}",
                    patterns::align::report(&state.align, &state.align_cal)
                );
            }
            if state.tune != tune_printed {
                tune_printed = state.tune;
                if !matches!(state.pattern(), Pattern::Grid | Pattern::Swim) {
                    println!("(the tuning warp is only applied to the grid and swim patterns)");
                }
                print!("{}", tune::report(&lens.lens, &state.tune).0);
            }
            state.tick(dt, &|p, d| steps_for(p, d));
        }
        if state.quit {
            exit_requested = true;
        }
        if opts
            .seconds
            .is_some_and(|sec| start.elapsed().as_secs_f32() > sec)
        {
            exit_requested = true;
        }

        if both && !reported {
            print!("{}", report::runtime_report(&s.info, &lens, &state, &opts));
            reported = true;
        }

        // swim: world-locked views from the LOCAL space, the room is anchored at the first pose
        if state.recenter {
            state.recenter = false;
            anchor = None;
            println!("swim: room re-centred on the head");
        }
        let t_world = Instant::now();
        let mut world: Option<Vec<xr::View>> = None;
        if both && state.pattern() == Pattern::Swim {
            if let Some(l) = &s.local {
                if let Ok((_, wv)) = s
                    .session
                    .locate_views(VIEW_TYPE, fs.predicted_display_time, l)
                {
                    if wv.len() >= 2 {
                        if anchor.is_none() {
                            anchor = Some(Anchor::from_views(&wv));
                        }
                        world = Some(wv);
                    }
                }
            }
        }
        timing.add(timing::LOCATE, t_world.elapsed());
        let extras: [EyeExtra; 2] = [0, 1].map(|eye| EyeExtra {
            head: if head_mode {
                head_rots.map(|m| m[eye])
            } else {
                None
            },
            swim: match (&world, &anchor) {
                (Some(wv), Some(a)) => Some(a.view(&wv[eye])),
                _ => None,
            },
            gain: state.tune_g[eye],
        });

        // (re)render when what is shown changed
        let key = state.key();
        let mut p0 = Params::new(w, h, fovs[0], 0);
        p0.square_deg = opts.square_deg;
        p0.square_dir = state.dir;
        let hud_lines = if key.hud {
            Some(report::hud_lines(&state, &p0))
        } else {
            None
        };
        // the text for the quad layer (gaze fills its own below); empty when the text is off or baked per eye
        let mut quad_lines: Vec<String> = if quad_on && state.pattern() != Pattern::Gaze {
            hud_lines.clone().unwrap_or_default()
        } else {
            Vec::new()
        };
        // what the per-eye renderers draw: nothing but the (per-eye) flat label when the quad carries the text
        let eye_hud: Option<&[String]> = if quad_on {
            hud_lines.as_ref().map(|_| &[][..])
        } else {
            hud_lines.as_deref()
        };
        let gaze_frame = both && state.pattern() == Pattern::Gaze && gaze_test.is_some();
        if !gaze_frame {
            gaze_last = None;
        }
        if gaze_frame {
            let t_render = Instant::now();
            swim_active = false;
            let mut reading: Option<GazeReading> = None;
            if let Some(gi) = &s.gaze {
                reading = gi.locate(&s.space, fs.predicted_display_time);
            }
            let gt = gaze_test.as_mut().ok_or("gaze test missing")?;
            for fx in gt.frame(f64::from(dt), reading) {
                println!("{}", gaze::execute(&fx, opts.gaze_out.as_deref(), None));
            }
            let marks = gt.marks(reading.as_ref());
            let mut hud = if key.hud {
                gt.hud(reading.is_some())
            } else {
                Vec::new()
            };
            if quad_on {
                quad_lines = std::mem::take(&mut hud);
            }
            let poses: [EyePose; 2] = std::array::from_fn(|i| EyePose {
                rot: quat_to_mat(quat_of(views[i].pose.orientation)),
                pos: [
                    views[i].pose.position.x,
                    views[i].pose.position.y,
                    views[i].pose.position.z,
                ],
            });
            let sig = (marks, poses, hud);
            if gaze_last.as_ref() != Some(&sig) {
                let painters =
                    gaze_painters.get_or_insert_with(|| [Painter::new(w, h), Painter::new(w, h)]);
                for eye in 0..2 {
                    painters[eye].paint(&sig.0, &poses[eye], &fovs[eye], &sig.2);
                    let e = &s.eyes[eye];
                    // SAFETY: staging_ptr maps staging_len bytes of host-coherent memory owned by this Eye; it is
                    // only written here, while no upload from it is in flight (every upload waits on its fence).
                    let out =
                        unsafe { std::slice::from_raw_parts_mut(e.staging_ptr, e.staging_len) };
                    painters[eye].img.to_upload(
                        color::encoding_of(s.format),
                        color::is_bgra(s.format),
                        default_threads(),
                        out,
                    );
                }
                gaze_last = Some(sig);
                content_gen += 1;
                frames_since_complete = 0;
                timing.add(timing::RENDER, t_render.elapsed());
            }
            rendered = Some((key, fovs, extras));
            let k4 = (key.pattern, key.step, key.eye, key.dir);
            if printed_key != Some(k4) {
                print!("{}", report::banner(&state, &p0));
                printed_key = Some(k4);
            }
        } else if both
            && rendered.as_ref().map(|(k, f, x)| (*k, *f, *x)) != Some((key, fovs, extras))
        {
            let t_render = Instant::now();
            swim_active = key.pattern == Pattern::Swim && s.swim.is_some();
            if swim_active {
                // the shader draws it: only the push constants (and, when the text changed, the HUD strip)
                for (eye, &eye_fov) in fovs.iter().enumerate() {
                    let p = eye_params(&s, eye, &key, eye_fov, &opts, &extras[eye]);
                    let shown = key.eye.shows(eye);
                    let lines = if shown {
                        hud_lines.clone().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    let lines = if quad_on { Vec::new() } else { lines };
                    let sig = (lines, eye_fov, extras[eye].head);
                    if swim_hud_sig[eye].as_ref() != Some(&sig) {
                        let strip =
                            patterns::hud_strip_at(w, h, &eye_fov, sig.2.as_ref(), &sig.0);
                        if let Some(g) = s.swim.as_ref() {
                            swim_hud_rows[eye] =
                                g.set_hud(eye, strip.as_ref().map(|(top, img)| (img, *top)))?;
                        }
                        swim_hud_sig[eye] = Some(sig);
                    }
                    swim_pc[eye] = swimgpu::push_constants(&p, !shown, swim_hud_rows[eye]);
                }
            } else {
                for (eye, &eye_fov) in fovs.iter().enumerate() {
                    render_into_staging(
                        &s,
                        eye,
                        &key,
                        eye_fov,
                        &opts,
                        eye_hud,
                        &extras[eye],
                    );
                }
            }
            timing.add(timing::RENDER, t_render.elapsed());
            content_gen += 1;
            frames_since_complete = 0;
            rendered = Some((key, fovs, extras));
            let k4 = (key.pattern, key.step, key.eye, key.dir);
            if printed_key != Some(k4) {
                print!("{}", report::banner(&state, &p0));
                printed_key = Some(k4);
            }
        }

        // swapchain images: acquire, copy the pattern if this image is stale, release
        let mut complete = true;
        #[allow(clippy::needless_range_loop)]
        for eye in 0..2 {
            let e = &mut s.eyes[eye];
            let idx = e
                .swapchain
                .acquire_image()
                .map_err(|e| format!("xrAcquireSwapchainImage: {e:?}"))?
                as usize;
            e.swapchain
                .wait_image(xr::Duration::INFINITE)
                .map_err(|e| format!("xrWaitSwapchainImage: {e:?}"))?;
            if rendered.is_some() && e.uploaded[idx] != content_gen {
                let t_up = Instant::now();
                match s.swim.as_ref().filter(|_| swim_active) {
                    Some(g) => g.draw(eye, idx, &swim_pc[eye])?,
                    // SAFETY: the image belongs to this swapchain, was just acquired and waited, and the staging
                    // buffer holds a complete image of the size the swapchain was created with.
                    None => unsafe { s.gpu.upload(s.eyes[eye].staging, s.eyes[eye].images[idx], w, h)? },
                }
                timing.add(timing::UPLOAD, t_up.elapsed());
                s.eyes[eye].uploaded[idx] = content_gen;
            }
            let e = &mut s.eyes[eye];
            e.swapchain
                .release_image()
                .map_err(|e| format!("xrReleaseSwapchainImage: {e:?}"))?;
            if e.uploaded.iter().any(|&g| g != content_gen) {
                complete = false;
            }
        }
        if complete && rendered.is_some() {
            frames_since_complete += 1;
        }

        // the head-locked text quad: re-render on a change of the text, copy into the acquired image if stale
        let mut quad_visible = false;
        if let Some(q) = s.hud.as_mut() {
            if !quad_lines.is_empty() {
                if quad_sig.as_ref() != Some(&quad_lines) {
                    q.set_text(s.format, &quad_lines);
                    quad_sig = Some(quad_lines.clone());
                    quad_gen += 1;
                }
                let idx = q
                    .swapchain
                    .acquire_image()
                    .map_err(|e| format!("xrAcquireSwapchainImage (text quad): {e:?}"))?
                    as usize;
                q.swapchain
                    .wait_image(xr::Duration::INFINITE)
                    .map_err(|e| format!("xrWaitSwapchainImage (text quad): {e:?}"))?;
                if q.uploaded[idx] != quad_gen {
                    // SAFETY: the image belongs to the quad's swapchain, was just acquired and waited, and the
                    // staging buffer holds a complete image of the size the swapchain was created with.
                    unsafe {
                        s.gpu.upload(
                            q.staging,
                            q.images[idx],
                            hudquad::TEX_W,
                            hudquad::TEX_H,
                        )?
                    };
                    q.uploaded[idx] = quad_gen;
                }
                q.swapchain
                    .release_image()
                    .map_err(|e| format!("xrReleaseSwapchainImage (text quad): {e:?}"))?;
                quad_visible = true;
            }
        }

        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: w as i32,
                height: h as i32,
            },
        };
        let layer_views: &[xr::View] = match &world {
            Some(wv) => wv,
            None => &views,
        };
        let layer_space = if world.is_some() {
            s.local.as_ref().unwrap_or(&s.space)
        } else {
            &s.space
        };
        let pv = |i: usize| {
            xr::CompositionLayerProjectionView::new()
                .pose(layer_views[i].pose)
                .fov(layer_views[i].fov)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&s.eyes[i].swapchain)
                        .image_array_index(0)
                        .image_rect(rect),
                )
        };
        let proj_views = [pv(0), pv(1)];
        let layer = xr::CompositionLayerProjection::new()
            .space(layer_space)
            .views(&proj_views);
        let t_end = Instant::now();
        // the text quad after the projection layer (on top), in VIEW space: the compositor puts it in the same
        // direction for both eyes whatever the display cant
        let quad_layer = s.hud.as_ref().filter(|_| quad_visible).map(|q| {
            let [px, py, pz] = hudquad::SPEC.position();
            xr::CompositionLayerQuad::new()
                .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                .space(&s.space)
                .eye_visibility(xr::EyeVisibility::BOTH)
                .pose(xr::Posef {
                    orientation: xr::Quaternionf {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        w: 1.0,
                    },
                    position: xr::Vector3f {
                        x: px,
                        y: py,
                        z: pz,
                    },
                })
                .size(xr::Extent2Df {
                    width: hudquad::SPEC.width,
                    height: hudquad::SPEC.height,
                })
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&q.swapchain)
                        .image_array_index(0)
                        .image_rect(xr::Rect2Di {
                            offset: xr::Offset2Di { x: 0, y: 0 },
                            extent: xr::Extent2Di {
                                width: hudquad::TEX_W as i32,
                                height: hudquad::TEX_H as i32,
                            },
                        }),
                )
        });
        if rendered.is_some() {
            match &quad_layer {
                Some(quad) => s
                    .stream
                    .end(fs.predicted_display_time, s.blend, &[&layer, quad])
                    .map_err(|e| format!("xrEndFrame: {e:?}"))?,
                None => s
                    .stream
                    .end(fs.predicted_display_time, s.blend, &[&layer])
                    .map_err(|e| format!("xrEndFrame: {e:?}"))?,
            }
        } else {
            s.stream
                .end(fs.predicted_display_time, s.blend, &[])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
        }
        let t_done = Instant::now();
        timing.add(timing::END, t_done - t_end);
        timing.add(timing::FRAME, t_done - t_waited);
        if let Some(line) = timing.frame_done(t_done) {
            println!("{line}");
        }
        frame_count += 1;

        if let Some(cap) = capture.as_mut() {
            let before = cap.index();
            match cap.frame(frames_since_complete, Instant::now()) {
                capture::Step::Captured(name) => {
                    println!("captured {name} ({}/{})", cap.index(), cap.tasks.len())
                }
                capture::Step::TimedOut(why) => println!("CAPTURE FAILED: {why}"),
                capture::Step::Triggered | capture::Step::Idle => {}
            }
            if cap.index() != before {
                frames_since_complete = 0;
            }
            if cap.done() {
                exit_requested = true;
            }
        }
        if exit_requested && exit_at.is_none() {
            let _ = s.session.request_exit();
            exit_at = Some(Instant::now());
        }
    }
    if state.patterns.contains(&Pattern::Align) {
        print!(
            "{}",
            patterns::align::final_block(
                &state.align,
                &state.align_cal,
                &lens.path.display().to_string()
            )
        );
    }
    if !state.tune.is_zero() {
        let (txt, _) = tune::report(&lens.lens, &state.tune);
        println!(
            "final distortion tuning (equivalent change of {}):",
            lens.path.display()
        );
        print!("{txt}");
    }
    println!("done: {frame_count} frames");
    s.destroy_gpu_objects();
    if let Some(cap) = capture {
        let results = cap.finish();
        let bad: Vec<String> = results
            .iter()
            .filter_map(|(n, r)| r.as_ref().err().map(|e| format!("{n}: {e}")))
            .collect();
        println!(
            "capture: {} pictures written, {} failed",
            results.len() - bad.len(),
            bad.len()
        );
        for b in &bad {
            println!("  FAILED {b}");
        }
        if let Some(dir) = capture_out {
            let list: Vec<String> = results
                .iter()
                .map(|(n, r)| match r {
                    Ok(sum) => format!("{n}.png ok raw-fnv1a={sum:016x}"),
                    Err(e) => format!("{n}.png FAILED {e}"),
                })
                .collect();
            let _ = std::fs::write(dir.join("index.txt"), list.join("\n") + "\n");
        }
        if !bad.is_empty() {
            return Err(format!("{} captures failed", bad.len()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_sizes() {
        assert_eq!(image_size((3552, 3840), 1), (3552, 3840));
        assert_eq!(image_size((3552, 3840), 4), (888, 960));
        assert_eq!(image_size((3552, 3840), 0), (3552, 3840));
        assert_eq!(image_size((20, 20), 16), (16, 16));
    }

    #[test]
    fn memory_type_choice() {
        let mk = |f: vk::MemoryPropertyFlags| vk::MemoryType {
            property_flags: f,
            heap_index: 0,
        };
        let types = [
            mk(vk::MemoryPropertyFlags::DEVICE_LOCAL),
            mk(vk::MemoryPropertyFlags::HOST_VISIBLE),
            mk(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT),
        ];
        let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        assert_eq!(find_memory_type(&types, 0b111, want), Some(2));
        assert_eq!(find_memory_type(&types, 0b011, want), None);
        assert_eq!(
            find_memory_type(&types, 0b100, vk::MemoryPropertyFlags::DEVICE_LOCAL),
            None
        );
        assert_eq!(
            find_memory_type(&types, 0b001, vk::MemoryPropertyFlags::DEVICE_LOCAL),
            Some(0)
        );
    }

    #[test]
    fn fov_conversion_keeps_the_openxr_signs() {
        let f = to_fov(xr::Fovf {
            angle_left: -0.8,
            angle_right: 0.9,
            angle_up: 0.7,
            angle_down: -0.6,
        });
        assert_eq!((f.left, f.right, f.up, f.down), (-0.8, 0.9, 0.7, -0.6));
    }

    #[test]
    fn capture_needs_the_dump_environment() {
        let opts = RunOpts {
            capture: Some("/tmp/x".into()),
            ..RunOpts::default()
        };
        // the environment of the test process has neither variable
        if std::env::var("XRT_COMPOSITOR_DUMP_DIR").is_err() {
            assert!(capture_env(&opts)
                .unwrap_err()
                .contains("XRT_COMPOSITOR_DUMP_DIR"));
        }
        assert_eq!(capture_env(&RunOpts::default()).unwrap(), None);
    }
}
