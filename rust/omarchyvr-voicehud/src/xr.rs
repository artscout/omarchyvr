//! The OpenXR (Vulkan) overlay session: one `XR_EXTX_overlay` session with a single small swapchain, the caption
//! texture uploaded through a staging buffer when (and only when) it changed, and a head-locked quad layer in VIEW
//! space. This is the only part that needs a runtime; what is shown and how it looks come from the pure modules.
//!
//! The Vulkan / OpenXR setup follows `pimax-lenstest` (`XR_KHR_vulkan_enable2`, the runtime creates the instance and
//! device); the session is created like WayVR does it (a raw `xrCreateSession` with `XrSessionCreateInfoOverlayEXTX`
//! chained after the Vulkan graphics binding).
//!
//! Frame loop rules this relies on (checked against Monado's `oxr_session_frame_end.c` / multi compositor):
//! * a swapchain image that was released stays valid for `xrEndFrame` until it is acquired again, so an unchanged
//!   caption just resubmits the same image: acquire / wait / copy / release happen only when the texture changed;
//! * `xrEndFrame` with no layers is a discarded frame, and Monado then drops the layers this session delivered
//!   before: submitting nothing is how the caption disappears (and the main app keeps its single-pass fast path).

use crate::color;
use crate::hud::Hud;
use crate::log;
use crate::placement::{Placement, QuadPose, TEX_H, TEX_W};
use ash::vk::{self, Handle};
use openxr as xr;
use openxr::sys::Handle as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

/// How long to wait for the runtime to finish the session after we asked it to exit.
const EXIT_GRACE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct XrConfig {
    pub placement: Placement,
    /// `XrSessionCreateInfoOverlayEXTX::sessionLayersPlacement` (higher is composited on top).
    pub z_order: u32,
    pub prefer_unorm: bool,
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// We were asked to stop (signal) and the session ended cleanly.
    Quit,
    /// The runtime went away or ended the session: tear down and wait for it again.
    Lost(String),
}

/// Load the OpenXR loader library. The caller keeps the entry for the whole process, so reconnecting to a restarted
/// runtime does not unload and reload the loader.
pub fn load_entry() -> Result<xr::Entry, String> {
    // SAFETY: loads the system's OpenXR loader (`libopenxr_loader.so.1`), which implements the OpenXR entry points.
    unsafe { xr::Entry::load(&()) }
        .map_err(|e| format!("cannot load the OpenXR loader (libopenxr_loader.so.1): {e}"))
}

/// Index of a memory type allowed by `bits` that has all of `flags`.
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

/// The Vulkan side: the instance and device the runtime created for us, and the upload machinery. Everything is
/// released in `Drop`, after the OpenXR session (the field order of [`Runtime`] guarantees that).
struct Gfx {
    entry: ash::Entry,
    instance: Option<ash::Instance>,
    device: Option<ash::Device>,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    staging_len: usize,
    queue_family: u32,
    physical_device: vk::PhysicalDevice,
}

impl Gfx {
    fn dev(&self) -> &ash::Device {
        self.device
            .as_ref()
            .expect("Vulkan device exists after create")
    }

    fn inst(&self) -> &ash::Instance {
        self.instance
            .as_ref()
            .expect("Vulkan instance exists after create")
    }

    /// Create the Vulkan instance and device through the OpenXR runtime, plus the staging buffer, command buffer
    /// and fence of the texture upload.
    ///
    /// # Safety
    /// `xr_instance` and `system` must belong together and `XR_KHR_vulkan_enable2` must be enabled.
    unsafe fn create(xr_instance: &xr::Instance, system: xr::SystemId) -> Result<Gfx, String> {
        let reqs = xr_instance
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
        let entry = ash::Entry::load().map_err(|e| format!("cannot load libvulkan: {e}"))?;
        let mut g = Gfx {
            entry,
            instance: None,
            device: None,
            queue: vk::Queue::null(),
            pool: vk::CommandPool::null(),
            cmd: vk::CommandBuffer::null(),
            fence: vk::Fence::null(),
            staging: vk::Buffer::null(),
            staging_mem: vk::DeviceMemory::null(),
            staging_ptr: std::ptr::null_mut(),
            staging_len: TEX_W as usize * TEX_H as usize * 4,
            queue_family: 0,
            physical_device: vk::PhysicalDevice::null(),
        };

        let app_info = vk::ApplicationInfo::default()
            .application_version(1)
            .engine_version(1)
            .api_version(vk::make_api_version(0, 1, 1, 0));
        #[allow(clippy::missing_transmute_annotations)]
        let raw_instance = xr_instance
            .create_vulkan_instance(
                system,
                std::mem::transmute(g.entry.static_fn().get_instance_proc_addr),
                &vk::InstanceCreateInfo::default().application_info(&app_info) as *const _
                    as *const _,
            )
            .map_err(|e| format!("xrCreateVulkanInstanceKHR: {e:?}"))?
            .map_err(|r| format!("vkCreateInstance: {:?}", vk::Result::from_raw(r)))?;
        g.instance = Some(ash::Instance::load(
            g.entry.static_fn(),
            vk::Instance::from_raw(raw_instance as _),
        ));

        let pd = vk::PhysicalDevice::from_raw(
            xr_instance
                .vulkan_graphics_device(system, g.inst().handle().as_raw() as _)
                .map_err(|e| format!("xrGetVulkanGraphicsDevice2KHR: {e:?}"))? as _,
        );
        g.physical_device = pd;
        let qf = g
            .inst()
            .get_physical_device_queue_family_properties(pd)
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or("the Vulkan device has no graphics queue")? as u32;
        g.queue_family = qf;
        let prio = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(qf)
            .queue_priorities(&prio)];
        #[allow(clippy::missing_transmute_annotations)]
        let raw_device = xr_instance
            .create_vulkan_device(
                system,
                std::mem::transmute(g.entry.static_fn().get_instance_proc_addr),
                pd.as_raw() as _,
                &vk::DeviceCreateInfo::default().queue_create_infos(&qinfo) as *const _ as *const _,
            )
            .map_err(|e| format!("xrCreateVulkanDeviceKHR: {e:?}"))?
            .map_err(|r| format!("vkCreateDevice: {:?}", vk::Result::from_raw(r)))?;
        g.device = Some(ash::Device::load(
            g.inst().fp_v1_0(),
            vk::Device::from_raw(raw_device as _),
        ));
        g.queue = g.dev().get_device_queue(qf, 0);
        let mem_props = g.inst().get_physical_device_memory_properties(pd);

        g.pool = g
            .dev()
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qf)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|r| format!("command pool: {r:?}"))?;
        g.cmd = g
            .dev()
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(g.pool)
                    .command_buffer_count(1),
            )
            .map_err(|r| format!("command buffer: {r:?}"))?[0];
        g.fence = g
            .dev()
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|r| format!("fence: {r:?}"))?;

        g.staging = g
            .dev()
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(g.staging_len as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|r| format!("staging buffer: {r:?}"))?;
        let req = g.dev().get_buffer_memory_requirements(g.staging);
        let mt = find_memory_type(
            &mem_props.memory_types[..mem_props.memory_type_count as usize],
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or("no host visible and coherent memory type")?;
        g.staging_mem = g
            .dev()
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(mt),
                None,
            )
            .map_err(|r| format!("staging memory: {r:?}"))?;
        g.dev()
            .bind_buffer_memory(g.staging, g.staging_mem, 0)
            .map_err(|r| format!("bind staging buffer: {r:?}"))?;
        g.staging_ptr = g
            .dev()
            .map_memory(
                g.staging_mem,
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )
            .map_err(|r| format!("map staging memory: {r:?}"))? as *mut u8;
        Ok(g)
    }

    /// Convert the texture into the swapchain format's bytes, into the staging buffer.
    fn write_staging(&mut self, rgba: &[u8], format: u32) -> Result<(), String> {
        if rgba.len() != self.staging_len {
            return Err(format!(
                "the texture has {} bytes, the staging buffer {}",
                rgba.len(),
                self.staging_len
            ));
        }
        // SAFETY: staging_ptr maps staging_len bytes of host-coherent memory owned by self; no upload from it is in
        // flight (every upload waits on its fence).
        let out = unsafe { std::slice::from_raw_parts_mut(self.staging_ptr, self.staging_len) };
        color::encode_for_format(rgba, out, format);
        Ok(())
    }

    /// Copy the staging buffer into `image` and leave the image in COLOR_ATTACHMENT_OPTIMAL (as OpenXR wants).
    ///
    /// # Safety
    /// `image` is an acquired and waited image of a swapchain created on this device, `w` x `h` large.
    unsafe fn upload(&self, image: vk::Image, w: u32, h: u32) -> Result<(), String> {
        let d = self.dev();
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
            self.staging,
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

impl Drop for Gfx {
    fn drop(&mut self) {
        // SAFETY: the handles were created in `create` (null ones are skipped), nothing is in flight after the wait
        // for idle, and the OpenXR session that used the device is already destroyed (field order of `Runtime`).
        unsafe {
            if let Some(d) = &self.device {
                let _ = d.device_wait_idle();
                if !self.staging_ptr.is_null() {
                    d.unmap_memory(self.staging_mem);
                }
                if self.staging != vk::Buffer::null() {
                    d.destroy_buffer(self.staging, None);
                }
                if self.staging_mem != vk::DeviceMemory::null() {
                    d.free_memory(self.staging_mem, None);
                }
                if self.fence != vk::Fence::null() {
                    d.destroy_fence(self.fence, None);
                }
                if self.pool != vk::CommandPool::null() {
                    d.destroy_command_pool(self.pool, None);
                }
                d.destroy_device(None);
            }
            if let Some(i) = &self.instance {
                i.destroy_instance(None);
            }
        }
    }
}

/// Create the overlay session: the Vulkan graphics binding with `XrSessionCreateInfoOverlayEXTX` chained after it.
///
/// # Safety
/// `info` must hold valid Vulkan handles that belong to `instance`'s runtime.
unsafe fn create_overlay_session(
    instance: &xr::Instance,
    system: xr::SystemId,
    info: &xr::vulkan::SessionCreateInfo,
    z_order: u32,
) -> Result<xr::sys::Session, xr::sys::Result> {
    let overlay = xr::sys::SessionCreateInfoOverlayEXTX {
        ty: xr::sys::SessionCreateInfoOverlayEXTX::TYPE,
        next: std::ptr::null(),
        create_flags: xr::OverlaySessionCreateFlagsEXTX::EMPTY,
        session_layers_placement: z_order,
    };
    let binding = xr::sys::GraphicsBindingVulkanKHR {
        ty: xr::sys::GraphicsBindingVulkanKHR::TYPE,
        next: (&raw const overlay).cast(),
        instance: info.instance,
        physical_device: info.physical_device,
        device: info.device,
        queue_family_index: info.queue_family_index,
        queue_index: info.queue_index,
    };
    let create = xr::sys::SessionCreateInfo {
        ty: xr::sys::SessionCreateInfo::TYPE,
        next: (&raw const binding).cast(),
        create_flags: xr::SessionCreateFlags::default(),
        system_id: system,
    };
    let mut out = xr::sys::Session::NULL;
    let r = (instance.fp().create_session)(instance.as_raw(), &raw const create, &raw mut out);
    if r.into_raw() >= 0 {
        Ok(out)
    } else {
        Err(r)
    }
}

/// The caption quad layer: head-locked in VIEW space, straight alpha from the texture, showing the whole swapchain
/// image.
fn quad_layer<'a>(
    pose: &QuadPose,
    space: &'a xr::Space,
    swapchain: &'a xr::Swapchain<xr::Vulkan>,
) -> xr::CompositionLayerQuad<'a, xr::Vulkan> {
    xr::CompositionLayerQuad::new()
        // straight (unpremultiplied) alpha from the texture
        .layer_flags(
            xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
                | xr::CompositionLayerFlags::UNPREMULTIPLIED_ALPHA,
        )
        .space(space)
        .eye_visibility(xr::EyeVisibility::BOTH)
        .pose(xr::Posef {
            orientation: xr::Quaternionf {
                x: pose.orientation[0],
                y: pose.orientation[1],
                z: pose.orientation[2],
                w: pose.orientation[3],
            },
            position: xr::Vector3f {
                x: pose.position[0],
                y: pose.position[1],
                z: pose.position[2],
            },
        })
        .size(xr::Extent2Df {
            width: pose.size[0],
            height: pose.size[1],
        })
        .sub_image(
            xr::SwapchainSubImage::new()
                .swapchain(swapchain)
                .image_array_index(0)
                .image_rect(xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di {
                        width: TEX_W as i32,
                        height: TEX_H as i32,
                    },
                }),
        )
}

/// A live connection to the runtime: instance, overlay session, swapchain, Vulkan objects.
///
/// Field order matters: Rust drops fields top to bottom, so the OpenXR objects (swapchain, space, frame objects,
/// session) go before the Vulkan device they use, and the OpenXR instance goes last.
pub struct Runtime {
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
    space: xr::Space,
    stream: xr::FrameStream<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    gfx: Gfx,
    instance: xr::Instance,
    format: u32,
    blend: xr::EnvironmentBlendMode,
    pose: QuadPose,
    description: String,
    /// The texture generation currently in the swapchain image that is submitted.
    uploaded: Option<u64>,
}

impl Runtime {
    /// Connect to the OpenXR runtime and create the overlay session. Errors are one-line messages meant for the
    /// log ("is monado-service running?").
    pub fn connect(cfg: &XrConfig, entry: &xr::Entry) -> Result<Runtime, String> {
        // SAFETY: the OpenXR and Vulkan loaders are system libraries; the calls follow the XR_KHR_vulkan_enable2
        // sequence of the openxr crate's example and of pimax-lenstest, and the overlay session creation of WayVR.
        unsafe {
            let exts = entry.enumerate_extensions().map_err(|e| {
                format!("no OpenXR runtime ({e:?}); is monado-service running and XR_RUNTIME_JSON / active_runtime.json set?")
            })?;
            if !exts.khr_vulkan_enable2 {
                return Err("the OpenXR runtime does not offer XR_KHR_vulkan_enable2".into());
            }
            if !exts.extx_overlay {
                return Err("the OpenXR runtime does not offer XR_EXTX_overlay (overlay sessions need Monado)".into());
            }
            let mut enabled = xr::ExtensionSet::default();
            enabled.khr_vulkan_enable2 = true;
            enabled.extx_overlay = true;
            let instance = entry
                .create_instance(
                    &xr::ApplicationInfo {
                        application_name: "omarchyvr-voice-hud",
                        application_version: 1,
                        engine_name: "omarchyvr-voice-hud",
                        engine_version: 1,
                        api_version: xr::Version::new(1, 0, 0),
                    },
                    &enabled,
                    &[],
                    &(),
                )
                .map_err(|e| {
                    format!("xrCreateInstance failed: {e:?} (is monado-service running?)")
                })?;
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

            let gfx = Gfx::create(&instance, system)?;

            let raw_session = create_overlay_session(
                &instance,
                system,
                &xr::vulkan::SessionCreateInfo {
                    instance: gfx.inst().handle().as_raw() as _,
                    physical_device: gfx.physical_device.as_raw() as _,
                    device: gfx.dev().handle().as_raw() as _,
                    queue_family_index: gfx.queue_family,
                    queue_index: 0,
                },
                cfg.z_order,
            )
            .map_err(|r| format!("xrCreateSession (overlay, z-order {}): {r:?}", cfg.z_order))?;
            let (session, waiter, stream) =
                xr::Session::<xr::Vulkan>::from_raw(instance.clone(), raw_session, Box::new(()));
            let space = session
                .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
                .map_err(|e| format!("view space: {e:?}"))?;

            let offered: Vec<u32> = session
                .enumerate_swapchain_formats()
                .map_err(|e| format!("swapchain formats: {e:?}"))?;
            let format = color::choose_format(&offered, cfg.prefer_unorm).ok_or_else(|| {
                format!(
                    "none of the 8-bit RGBA/BGRA swapchain formats is offered (offered: {offered:?})"
                )
            })?;
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
                .map_err(|e| {
                    format!(
                        "xrCreateSwapchain {TEX_W}x{TEX_H} {}: {e:?}",
                        color::format_name(format)
                    )
                })?;
            let images: Vec<vk::Image> = swapchain
                .enumerate_images()
                .map_err(|e| format!("swapchain images: {e:?}"))?
                .into_iter()
                .map(vk::Image::from_raw)
                .collect();

            let description = format!(
                "{} {}, system \"{}\", overlay z-order {}, swapchain {} {TEX_W}x{TEX_H} ({} images, {})",
                props.runtime_name,
                props.runtime_version,
                sys_props.system_name,
                cfg.z_order,
                color::format_name(format),
                images.len(),
                match color::encoding_of(format) {
                    color::Encoding::Srgb => "sRGB: bytes uploaded as authored",
                    color::Encoding::Linear => "UNORM: colour stored as linear light",
                }
            );
            Ok(Runtime {
                swapchain,
                images,
                space,
                stream,
                waiter,
                session,
                gfx,
                instance,
                format,
                blend,
                pose: cfg.placement.pose(),
                description,
                uploaded: None,
            })
        }
    }

    /// One line about the connection, for the log.
    pub fn describe(&self) -> &str {
        &self.description
    }

    /// Copy the texture into the next swapchain image: acquire, wait, GPU copy, release.
    fn upload_texture(&mut self, rgba: &[u8]) -> Result<(), String> {
        self.gfx.write_staging(rgba, self.format)?;
        let idx = self
            .swapchain
            .acquire_image()
            .map_err(|e| format!("xrAcquireSwapchainImage: {e:?}"))? as usize;
        self.swapchain
            .wait_image(xr::Duration::INFINITE)
            .map_err(|e| format!("xrWaitSwapchainImage: {e:?}"))?;
        // SAFETY: the image belongs to our swapchain, was just acquired and waited, and the staging buffer holds a
        // complete TEX_W x TEX_H image of the swapchain's format.
        let copied = unsafe { self.gfx.upload(self.images[idx], TEX_W, TEX_H) };
        // release in any case so that the swapchain state stays consistent
        self.swapchain
            .release_image()
            .map_err(|e| format!("xrReleaseSwapchainImage: {e:?}"))?;
        copied
    }

    fn lost_or_quit(stop: &AtomicBool, msg: String) -> Outcome {
        if stop.load(Ordering::Relaxed) {
            Outcome::Quit
        } else {
            Outcome::Lost(msg)
        }
    }

    /// Run the session until we are asked to stop (`stop`), or the runtime ends it. `hud` is pumped every frame
    /// (and while no frames run), so the socket is read and the display kept current.
    pub fn run(&mut self, hud: &mut Hud, stop: &AtomicBool) -> Outcome {
        let mut event_buf = xr::EventDataBuffer::new();
        let mut running = false;
        let mut exit_requested_at: Option<Instant> = None;
        loop {
            if stop.load(Ordering::Relaxed) && exit_requested_at.is_none() {
                log::info("stopping (signal): ending the session");
                exit_requested_at = Some(Instant::now());
                match self.session.request_exit() {
                    Ok(()) => {}
                    // not running (yet or any more): nothing to wait for
                    Err(xr::sys::Result::ERROR_SESSION_NOT_RUNNING) => return Outcome::Quit,
                    Err(e) => return Outcome::Lost(format!("xrRequestExitSession: {e:?}")),
                }
            }
            loop {
                let ev = match self.instance.poll_event(&mut event_buf) {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,
                    Err(e) => return Self::lost_or_quit(stop, format!("xrPollEvent: {e:?}")),
                };
                match ev {
                    xr::Event::SessionStateChanged(e) => {
                        log::info(format!("session state: {:?}", e.state()));
                        match e.state() {
                            xr::SessionState::READY => {
                                if let Err(err) = self.session.begin(VIEW_TYPE) {
                                    return Self::lost_or_quit(
                                        stop,
                                        format!("xrBeginSession: {err:?}"),
                                    );
                                }
                                running = true;
                            }
                            xr::SessionState::STOPPING => {
                                if let Err(err) = self.session.end() {
                                    return Self::lost_or_quit(
                                        stop,
                                        format!("xrEndSession: {err:?}"),
                                    );
                                }
                                running = false;
                            }
                            xr::SessionState::EXITING => {
                                return if exit_requested_at.is_some() {
                                    Outcome::Quit
                                } else {
                                    Outcome::Lost("the runtime ended the session (EXITING)".into())
                                };
                            }
                            xr::SessionState::LOSS_PENDING => {
                                return Self::lost_or_quit(
                                    stop,
                                    "session lost: the runtime is shutting down (LOSS_PENDING)"
                                        .into(),
                                );
                            }
                            _ => {}
                        }
                    }
                    xr::Event::InstanceLossPending(_) => {
                        return Self::lost_or_quit(
                            stop,
                            "instance loss pending: the runtime is shutting down".into(),
                        );
                    }
                    xr::Event::MainSessionVisibilityChangedEXTX(e) => {
                        log::info(format!("main session visible: {}", e.visible()));
                    }
                    _ => {}
                }
            }

            if !running {
                if exit_requested_at.is_some_and(|t| t.elapsed() > EXIT_GRACE) {
                    return Outcome::Quit;
                }
                // no frames yet (waiting for READY) or paused (STOPPING): keep the display model current
                hud.pump(Instant::now());
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }

            let fs = match self.waiter.wait() {
                Ok(fs) => fs,
                Err(xr::sys::Result::ERROR_SESSION_NOT_RUNNING) => {
                    running = false;
                    continue;
                }
                Err(e) => return Self::lost_or_quit(stop, format!("xrWaitFrame: {e:?}")),
            };
            match self.stream.begin() {
                Ok(_) => {}
                Err(xr::sys::Result::ERROR_SESSION_NOT_RUNNING) => {
                    running = false;
                    continue;
                }
                Err(e) => return Self::lost_or_quit(stop, format!("xrBeginFrame: {e:?}")),
            }
            // read the socket as late as possible: this frame shows what arrived before now
            let frame = hud.pump(Instant::now());

            let show = fs.should_render && frame.visible;
            // upload only when the texture changed; otherwise the image released last time is submitted again
            if show && self.uploaded != Some(frame.generation) {
                match self.upload_texture(hud.texture()) {
                    Ok(()) => self.uploaded = Some(frame.generation),
                    Err(e) => return Self::lost_or_quit(stop, e),
                }
            }
            let ended = if show {
                let quad = quad_layer(&self.pose, &self.space, &self.swapchain);
                self.stream
                    .end(fs.predicted_display_time, self.blend, &[&quad])
            } else {
                // nothing to show: a frame without layers; Monado discards it and drops our previous layers
                self.stream.end(fs.predicted_display_time, self.blend, &[])
            };
            match ended {
                Ok(()) => {}
                Err(xr::sys::Result::ERROR_SESSION_NOT_RUNNING) => running = false,
                Err(e) => return Self::lost_or_quit(stop, format!("xrEndFrame: {e:?}")),
            }
            if exit_requested_at.is_some_and(|t| t.elapsed() > EXIT_GRACE) {
                return Outcome::Quit;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn outcome_after_a_stop_request_is_a_quit() {
        let stop = AtomicBool::new(false);
        assert_eq!(
            Runtime::lost_or_quit(&stop, "boom".into()),
            Outcome::Lost("boom".into())
        );
        stop.store(true, Ordering::Relaxed);
        assert_eq!(Runtime::lost_or_quit(&stop, "boom".into()), Outcome::Quit);
    }
}
