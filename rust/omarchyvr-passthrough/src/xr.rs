//! The live OpenXR (Vulkan) passthrough client.
//!
//! * A camera thread reads tracking frames (`pimax_cam`: live `/dev/videoN` or a `.pxcam` replay), remaps both
//!   eyes through the current [`Lut`]s and publishes grey eye images with their capture time.
//! * The frame loop expands the newest image to RGBA in a staging buffer, copies it into the eye swapchains and
//!   submits one projection layer in **LOCAL** space whose view poses are the eye poses **at camera time**
//!   (`xrLocateViews` at the converted capture time). The compositor's timewarp then rotates the image from the
//!   camera-time pose to the display-time pose: that is the IMU rotation compensation, done where it is cheapest
//!   and latest. Nothing in Monado changes.
//! * The maps are built on a helper thread from the runtime's own eye poses and FOVs (VIEW space), so the image
//!   matches whatever the compositor expects.
//!
//! The Vulkan / swapchain code is the proven upload path of `pimax-lenstest` (CPU image -> staging buffer ->
//! `vkCmdCopyBufferToImage`), with no shaders and no render passes.

use crate::frames::{self, TileSet};
use crate::lut::{Lut, LutParams};
use crate::remap;
use crate::rig::Rig;
use crate::timing::{self, CamTiming, LatencyStats};
use crate::view::{EyeView, Fov};
use ash::vk::{self, Handle};
use openxr as xr;
use pimax_cam::calib::DeviceCalib;
use pimax_cam::clock;
use pimax_cam::geom::{Pose, Vec3};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Floor grid texture size (px).
const FLOOR_PX: usize = 1024;
const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;
const VK_FORMAT_B8G8R8A8_SRGB: u32 = 50;
const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_FORMAT_B8G8R8A8_UNORM: u32 = 44;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Depth steps for the `+` / `-` keys.
pub const DEPTH_STEPS: [f64; 9] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0, f64::INFINITY];
/// Gamma steps for the `g` key (the cameras' fixed exposure often overexposes a lit room).
pub const GAMMA_STEPS: [f64; 4] = [1.0, 1.5, 2.2, 0.7];

/// Options of `omarchyvr-passthrough run`.
#[derive(Clone, Debug)]
pub struct RunOpts {
    /// Camera source: "" / auto = live camera, `/dev/videoN`, or a `.pxcam` (looped, real time).
    pub source: String,
    /// Calibration XML (default: `pimax_cam` search path).
    pub calib: Option<PathBuf>,
    /// Learned calibration overlay to apply (`pimax-selfcal`; a path or `default`). `None`: factory only.
    pub overlay: Option<String>,
    /// Eye centre in the body frame (m).
    pub head_offset_m: Vec3,
    /// Initial reprojection depth (m, inf allowed).
    pub depth_m: f64,
    /// Swapchain size = recommended / `res_div`.
    pub res_div: u32,
    /// Camera clock correction.
    pub timing: CamTiming,
    /// Exit after this many seconds.
    pub seconds: Option<f64>,
    /// Tone curve.
    pub gamma: f64,
    /// Tone gain.
    pub gain: f64,
    /// Status line period (s).
    pub stats_every_s: f64,
    /// Start with passthrough hidden (`p` shows it).
    pub start_off: bool,
    /// Show the floor grid ([`crate::floorgrid`]; `o` toggles, so does `$XDG_RUNTIME_DIR/omarchyvr-floor-grid`).
    pub floor: bool,
}

/// Swapchain format preference: sRGB (the camera bytes are display-referred), then UNORM.
pub fn choose_format(offered: &[u32]) -> Option<u32> {
    [
        VK_FORMAT_R8G8B8A8_SRGB,
        VK_FORMAT_B8G8R8A8_SRGB,
        VK_FORMAT_R8G8B8A8_UNORM,
        VK_FORMAT_B8G8R8A8_UNORM,
    ]
    .into_iter()
    .find(|f| offered.contains(f))
}

/// Name of a swapchain format.
pub fn format_name(f: u32) -> String {
    match f {
        VK_FORMAT_R8G8B8A8_SRGB => "R8G8B8A8_SRGB".into(),
        VK_FORMAT_B8G8R8A8_SRGB => "B8G8R8A8_SRGB".into(),
        VK_FORMAT_R8G8B8A8_UNORM => "R8G8B8A8_UNORM".into(),
        VK_FORMAT_B8G8R8A8_UNORM => "B8G8R8A8_UNORM".into(),
        o => format!("VkFormat {o}"),
    }
}

/// Image size to render: the runtime's recommendation divided by `res_div`.
pub fn image_size(recommended: (u32, u32), res_div: u32) -> (u32, u32) {
    (
        (recommended.0 / res_div.max(1)).max(16),
        (recommended.1 / res_div.max(1)).max(16),
    )
}

/// OpenXR pose -> [`Pose`] (`a_from_b` with b = the posed frame).
pub fn pose_from_xr(p: xr::Posef) -> Pose {
    let q = p.orientation;
    let (x, y, z, w) = (q.x as f64, q.y as f64, q.z as f64, q.w as f64);
    let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-12);
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    Pose {
        r: [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ],
        t: [
            p.position.x as f64,
            p.position.y as f64,
            p.position.z as f64,
        ],
    }
}

/// [`Fov`] from OpenXR.
pub fn fov_from_xr(f: xr::Fovf) -> Fov {
    Fov {
        left: f.angle_left as f64,
        right: f.angle_right as f64,
        up: f.angle_up as f64,
        down: f.angle_down as f64,
    }
}

fn fov_to_xr(f: Fov) -> xr::Fovf {
    xr::Fovf {
        angle_left: f.left as f32,
        angle_right: f.right as f32,
        angle_up: f.up as f32,
        angle_down: f.down as f32,
    }
}

/// What the maps depend on, quantised so that tracking noise in the view poses does not trigger rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapKey {
    depth_mm: i64,
    size: (usize, usize),
    /// per eye: fov (1e-4 rad), eye position (0.1 mm), rotation matrix entries (1e-4)
    eyes: [[i64; 16]; 2],
}

impl MapKey {
    /// Key of a view pair and depth.
    pub fn new(views: &[EyeView; 2], depth_m: f64) -> MapKey {
        let q = |v: f64, s: f64| (v * s).round() as i64;
        let eye = |e: &EyeView| {
            let mut k = [0i64; 16];
            let f = e.fov;
            k[0] = q(f.left, 1e4);
            k[1] = q(f.right, 1e4);
            k[2] = q(f.up, 1e4);
            k[3] = q(f.down, 1e4);
            for i in 0..3 {
                k[4 + i] = q(e.head_from_eye.t[i], 1e4);
                for j in 0..3 {
                    k[7 + i * 3 + j] = q(e.head_from_eye.r[i][j], 1e4);
                }
            }
            k
        };
        MapKey {
            depth_mm: if depth_m.is_finite() {
                q(depth_m, 1e3)
            } else {
                i64::MAX
            },
            size: (views[0].width, views[0].height),
            eyes: [eye(&views[0]), eye(&views[1])],
        }
    }
}

/// Both eyes' maps and the views they were built for.
pub struct MapPair {
    /// Key.
    pub key: MapKey,
    /// Views (pose relative to the head, FOV, size).
    pub views: [EyeView; 2],
    /// Maps.
    pub luts: [Lut; 2],
    /// Build time (ms).
    pub build_ms: f64,
}

/// Build both maps.
pub fn build_maps(rig: &Rig, views: [EyeView; 2], depth_m: f64) -> MapPair {
    let t0 = Instant::now();
    let p = LutParams {
        depth_m,
        ..LutParams::default()
    };
    let luts = [
        Lut::build(rig, &views[0], 0, &p),
        Lut::build(rig, &views[1], 1, &p),
    ];
    MapPair {
        key: MapKey::new(&views, depth_m),
        views,
        luts,
        build_ms: t0.elapsed().as_secs_f64() * 1e3,
    }
}

/// Newest remapped camera image.
struct Latest {
    gen: u64,
    mono_ns: u64,
    counter: u32,
    eyes: [Vec<u8>; 2],
    maps: Option<Arc<MapPair>>,
    remap_ms: f64,
    error: Option<String>,
}

struct Shared {
    stop: AtomicBool,
    freeze: AtomicBool,
    maps: Mutex<Option<Arc<MapPair>>>,
    latest: Mutex<Latest>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn camera_thread(shared: Arc<Shared>, mut cam: pimax_cam::camera::Camera) {
    let mut ts = TileSet::black();
    let mut bufs: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut last_frame = Instant::now();
    let mut warned = false;
    while !shared.stop.load(Ordering::Relaxed) {
        match frames::next_tracking(&mut cam, 200, &mut ts) {
            Ok(true) => {
                if warned {
                    eprintln!(
                        "camera: frames again after {:.1} s ({})",
                        last_frame.elapsed().as_secs_f64(),
                        cam.describe()
                    );
                }
                last_frame = Instant::now();
                warned = false;
            }
            Ok(false) => {
                // A live camera or broker reopens / reconnects by itself (pimax_cam::resilient): keep waiting, the
                // view holds the last image meanwhile.
                if !warned && last_frame.elapsed() > Duration::from_secs(2) {
                    eprintln!(
                        "camera: no tracking frame for 2 s ({}); waiting for it to come back",
                        cam.describe()
                    );
                    warned = true;
                }
                continue;
            }
            Err(e) => {
                lock(&shared.latest).error = Some(format!("camera: {e}"));
                return;
            }
        }
        if shared.freeze.load(Ordering::Relaxed) {
            continue;
        }
        let Some(maps) = lock(&shared.maps).clone() else {
            continue;
        };
        let t0 = Instant::now();
        for (e, buf) in bufs.iter_mut().enumerate() {
            buf.resize(maps.luts[e].taps.len(), 0);
            remap::remap_grey(&maps.luts[e], &ts.view(), buf, 0);
        }
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let mut l = lock(&shared.latest);
        std::mem::swap(&mut l.eyes, &mut bufs);
        l.gen += 1;
        l.mono_ns = ts.mono_ns;
        l.counter = ts.counter;
        l.maps = Some(maps);
        l.remap_ms = ms;
    }
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

struct Gpu {
    device: ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
}

impl Gpu {
    /// Copy the staging buffer into the swapchain image and leave it in COLOR_ATTACHMENT_OPTIMAL (as OpenXR wants).
    unsafe fn upload(&self, eye: &Eye, image: vk::Image, w: u32, h: u32) -> Result<(), String> {
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
            eye.staging,
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

/// A swapchain of `size` with its staging buffer (host visible, mapped).
///
/// # Safety
/// `device` and `session` belong together (the session was created on that device).
unsafe fn make_eye(
    session: &xr::Session<xr::Vulkan>,
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    format: u32,
    size: (u32, u32),
) -> Result<Eye, String> {
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
                format_name(format)
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
    Ok(Eye {
        uploaded: vec![0; images.len()],
        swapchain,
        images,
        staging: buffer,
        staging_mem: mem,
        staging_ptr: ptr,
        staging_len,
    })
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
        enabled.khr_convert_timespec_time = exts.khr_convert_timespec_time;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo { application_name: "omarchyvr-passthrough", application_version: 1, engine_name: "omarchyvr-passthrough", engine_version: 1, api_version: xr::Version::new(1, 0, 0) },
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
        let view_space = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .map_err(|e| format!("view space: {e:?}"))?;
        let local = session
            .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
            .map_err(|e| format!("local space: {e:?}"))?;

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
        let format = choose_format(&offered).ok_or_else(|| {
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
            eyes.push(make_eye(&session, &device, &mem_props, format, size)?);
        }
        let floor = make_eye(&session, &device, &mem_props, format, (FLOOR_PX as u32, FLOOR_PX as u32))?;
        let stage = session
            .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
            .map_err(|e| format!("stage space: {e:?}"))?;
        let eyes: [Eye; 2] = eyes.try_into().map_err(|_| "two eyes")?;

        let info = format!(
            "{} {} | {} | recommended {}x{} | image {}x{} {} | XR_KHR_convert_timespec_time {}",
            props.runtime_name,
            props.runtime_version,
            sys_props.system_name,
            recommended.0,
            recommended.1,
            size.0,
            size.1,
            format_name(format),
            if exts.khr_convert_timespec_time {
                "yes"
            } else {
                "NO (no pose-time compensation)"
            }
        );
        Ok(Session {
            instance,
            session,
            waiter,
            stream,
            view_space,
            local,
            stage,
            floor,
            floor_format: format,
            have_timespec: exts.khr_convert_timespec_time,
            blend,
            gpu,
            eyes,
            size,
            info,
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
            for e in self.eyes.iter().chain(std::iter::once(&self.floor)) {
                d.unmap_memory(e.staging_mem);
                d.destroy_buffer(e.staging, None);
                d.free_memory(e.staging_mem, None);
            }
            d.destroy_fence(self.gpu.fence, None);
            d.destroy_command_pool(self.gpu.pool, None);
        }
    }
}

/// Everything the main loop needs from the XR/Vulkan setup.
struct Session {
    instance: xr::Instance,
    session: xr::Session<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    stream: xr::FrameStream<xr::Vulkan>,
    view_space: xr::Space,
    local: xr::Space,
    /// STAGE (floor at y = 0) for the floor grid.
    stage: xr::Space,
    /// The floor grid's swapchain (uploaded once).
    floor: Eye,
    floor_format: u32,
    have_timespec: bool,
    blend: xr::EnvironmentBlendMode,
    gpu: Gpu,
    eyes: [Eye; 2],
    size: (u32, u32),
    info: String,
    // keep the Vulkan loader and instance alive as long as the session
    _vk_entry: ash::Entry,
    _vk_instance: ash::Instance,
}

const HELP: &str =
    "keys: p = passthrough on/off, + / - = depth, [ / ] = camera delay -/+ 2 ms, g = gamma, \
f = freeze image, o = floor grid, s = status, q = quit";

fn depth_name(d: f64) -> String {
    if d.is_finite() {
        format!("{d} m")
    } else {
        "infinite".into()
    }
}

fn step_depth(cur: f64, up: bool) -> f64 {
    let i = DEPTH_STEPS
        .iter()
        .position(|&d| d >= cur || !d.is_finite())
        .unwrap_or(DEPTH_STEPS.len() - 1);
    if up {
        DEPTH_STEPS[(i + 1).min(DEPTH_STEPS.len() - 1)]
    } else {
        DEPTH_STEPS[i.saturating_sub(1)]
    }
}

/// Run the client until `q`, `--seconds`, a signal, or the runtime ends the session.
pub fn run(opts: RunOpts) -> Result<(), String> {
    let (mut calib, calib_path) =
        DeviceCalib::load_default(opts.calib.as_deref()).map_err(|e| e.to_string())?;
    if let Some(spec) = &opts.overlay {
        let (c2, log) = crate::rig::apply_overlay(&calib, spec)?;
        println!("{log}");
        calib = c2;
    }
    let rig = Arc::new(Rig::from_calib(&calib, opts.head_offset_m)?);
    println!(
        "calibration {} (device {})",
        calib_path.display(),
        calib.uid
    );
    print!("{}", rig.report());

    // camera first: if it cannot be opened there is no point in starting a session
    let cam = frames::open_source(&opts.source, true, true)
        .map_err(|e| format!("camera source {:?}: {e}", opts.source))?;
    println!("camera: {}", cam.describe());

    // SAFETY: installing a signal handler that only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    let mut s = setup(&opts)?;
    println!("runtime: {}", s.info);
    println!("{HELP}");
    let (w, h) = s.size;

    let shared = Arc::new(Shared {
        stop: AtomicBool::new(false),
        freeze: AtomicBool::new(false),
        maps: Mutex::new(None),
        latest: Mutex::new(Latest {
            gen: 0,
            mono_ns: 0,
            counter: 0,
            eyes: [Vec::new(), Vec::new()],
            maps: None,
            remap_ms: 0.0,
            error: None,
        }),
    });
    let cam_thread = {
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("pt-camera".into())
            .spawn(move || camera_thread(sh, cam))
            .map_err(|e| format!("camera thread: {e}"))?
    };

    let keys = spawn_keyboard();
    let _raw = RawTerminal::enable();
    let mut timing = opts.timing;
    let mut depth = opts.depth_m;
    let mut gamma = opts.gamma;
    let mut gain = opts.gain;
    let mut tone = remap::tone_table(gamma, gain);
    let mut show = !opts.start_off;
    let grid = crate::floorgrid::Grid { px: FLOOR_PX, ..Default::default() };
    let mut floor_on = opts.floor;
    let mut floor_ready = false;
    let mut floor_watch = crate::floorgrid::ToggleWatch::default();
    // live control socket (voice, presets): on/off/gain; see `control`
    let control = match crate::control::Server::start(
        &crate::control::socket_path(),
        crate::control::View { on: show, gain },
    ) {
        Ok(c) => Some(c),
        Err(e) => {
            println!("control socket off: {e}");
            None
        }
    };
    let mut job: Option<JoinHandle<MapPair>> = None;

    let mut event_buf = xr::EventDataBuffer::new();
    let mut running = false;
    let mut exit_requested = false;
    let mut exit_at: Option<Instant> = None;
    let start = Instant::now();
    let mut shown_gen = 0u64;
    let mut content_gen = 0u64;
    // eye poses (LOCAL, at camera time) and FOVs of the image in the staging buffers
    let mut content_views: Option<[xr::View; 2]> = None;
    let mut lat_display = LatencyStats::default();
    let mut lat_ready = LatencyStats::default();
    let mut remap_ms = LatencyStats::default();
    let mut images = 0u64;
    let mut last_stats = Instant::now();
    let mut print_status = false;
    let mut frame_count = 0u64;
    let mut result = Ok(());

    'main: loop {
        if STOP.load(Ordering::Relaxed) && !exit_requested {
            println!("stopping (signal)");
            exit_requested = true;
        }
        if exit_requested && exit_at.is_none() {
            let _ = s.session.request_exit();
            exit_at = Some(Instant::now());
        }
        if let Some(e) = lock(&shared.latest).error.take() {
            result = Err(e);
            exit_requested = true;
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

        let fs = s.waiter.wait().map_err(|e| format!("xrWaitFrame: {e:?}"))?;
        s.stream
            .begin()
            .map_err(|e| format!("xrBeginFrame: {e:?}"))?;
        if !fs.should_render {
            s.stream
                .end(fs.predicted_display_time, s.blend, &[])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
            continue;
        }

        // keyboard
        while let Ok(c) = keys.try_recv() {
            match c {
                'p' | ' ' => {
                    show = !show;
                    println!("passthrough {}", if show { "ON" } else { "OFF" });
                }
                '+' | '=' => {
                    depth = step_depth(depth, true);
                    println!("depth {}", depth_name(depth));
                }
                '-' | '_' => {
                    depth = step_depth(depth, false);
                    println!("depth {}", depth_name(depth));
                }
                ']' => {
                    timing.delay_ns += 2_000_000;
                    println!("camera delay {:.0} ms", timing.delay_ns as f64 / 1e6);
                }
                '[' => {
                    timing.delay_ns -= 2_000_000;
                    println!("camera delay {:.0} ms", timing.delay_ns as f64 / 1e6);
                }
                'g' => {
                    let i = GAMMA_STEPS.iter().position(|&g| g == gamma).unwrap_or(0);
                    gamma = GAMMA_STEPS[(i + 1) % GAMMA_STEPS.len()];
                    tone = remap::tone_table(gamma, gain);
                    shown_gen = 0; // re-expand the current image
                    println!("gamma {gamma}");
                }
                'f' => {
                    let f = !shared.freeze.load(Ordering::Relaxed);
                    shared.freeze.store(f, Ordering::Relaxed);
                    println!("image {}", if f { "FROZEN" } else { "live" });
                }
                'o' => {
                    floor_on = !floor_on;
                    println!("floor grid {}", if floor_on { "ON" } else { "OFF" });
                }
                's' => print_status = true,
                'q' | '\x1b' => exit_requested = true,
                'h' | '?' => println!("{HELP}"),
                _ => {}
            }
        }
        if let Some(c) = &control {
            let mut v = crate::control::View { on: show, gain };
            if c.poll(&mut v) {
                if v.gain != gain {
                    gain = v.gain;
                    tone = remap::tone_table(gamma, gain);
                    shown_gen = 0; // re-expand the current image
                }
                show = v.on;
                println!("control: passthrough {} gain {gain:.2}", if show { "ON" } else { "OFF" });
            } else {
                c.set(v); // keyboard changes
            }
        }
        if opts
            .seconds
            .is_some_and(|sec| start.elapsed().as_secs_f64() > sec)
        {
            exit_requested = true;
        }

        // the runtime's eye poses relative to the head and its FOVs -> maps (rebuilt on a helper thread)
        let (_, head_views) = s
            .session
            .locate_views(VIEW_TYPE, fs.predicted_display_time, &s.view_space)
            .map_err(|e| format!("xrLocateViews (VIEW): {e:?}"))?;
        if head_views.len() >= 2 {
            let ev = |v: &xr::View| EyeView {
                head_from_eye: pose_from_xr(v.pose),
                fov: fov_from_xr(v.fov),
                width: w as usize,
                height: h as usize,
            };
            let views = [ev(&head_views[0]), ev(&head_views[1])];
            let key = MapKey::new(&views, depth);
            let current = lock(&shared.maps).as_ref().map(|m| m.key);
            if job.as_ref().is_some_and(|j| j.is_finished()) {
                if let Some(Ok(m)) = job.take().map(|j| j.join()) {
                    println!(
                        "maps {} ms, depth {}: left {} | right {}",
                        m.build_ms as u64,
                        depth_name(depth),
                        m.luts[0].summary(),
                        m.luts[1].summary()
                    );
                    *lock(&shared.maps) = Some(Arc::new(m));
                }
            } else if job.is_none() && current != Some(key) {
                let rig = rig.clone();
                job = Some(
                    std::thread::Builder::new()
                        .name("pt-maps".into())
                        .spawn(move || build_maps(&rig, views, depth))
                        .map_err(|e| format!("map thread: {e}"))?,
                );
            }
        }

        // newest camera image -> staging, plus the eye poses at its capture time
        {
            let l = lock(&shared.latest);
            if l.gen != shown_gen && l.gen > 0 {
                if let Some(maps) = l.maps.clone() {
                    for e in 0..2 {
                        let eye = &s.eyes[e];
                        let n = maps.luts[e].taps.len();
                        if l.eyes[e].len() == n && n * 4 <= eye.staging_len {
                            // SAFETY: staging_ptr maps staging_len host-coherent bytes; no upload is in flight
                            // (uploads wait on their fence).
                            let dst = unsafe {
                                std::slice::from_raw_parts_mut(eye.staging_ptr, eye.staging_len)
                            };
                            remap::expand_rgba(&l.eyes[e], &tone, dst, 0);
                        }
                    }
                    let now_mono = clock::monotonic_ns();
                    let pose_mono = timing.pose_time(l.mono_ns, now_mono);
                    let pose_time = if s.have_timespec {
                        match s.instance.now() {
                            Ok(xr_now) => xr::Time::from_nanos(timing::mono_to_xr(
                                pose_mono,
                                xr_now.as_nanos(),
                                now_mono,
                            )),
                            Err(_) => fs.predicted_display_time,
                        }
                    } else {
                        fs.predicted_display_time
                    };
                    let (flags, cam_views) = s
                        .session
                        .locate_views(VIEW_TYPE, pose_time, &s.local)
                        .map_err(|e| format!("xrLocateViews (LOCAL, camera time): {e:?}"))?;
                    if flags.contains(xr::ViewStateFlags::ORIENTATION_VALID) && cam_views.len() >= 2
                    {
                        let mut v = [cam_views[0], cam_views[1]];
                        for (e, view) in v.iter_mut().enumerate() {
                            view.fov = fov_to_xr(maps.views[e].fov);
                        }
                        content_views = Some(v);
                        content_gen += 1;
                        images += 1;
                        lat_ready.add((now_mono as f64 - pose_mono as f64) / 1e6);
                        lat_display.add(
                            (fs.predicted_display_time.as_nanos() - pose_time.as_nanos()) as f64
                                / 1e6,
                        );
                        remap_ms.add(l.remap_ms);
                    }
                    shown_gen = l.gen;
                }
            }
        }

        if let Some(on) = floor_watch.poll() {
            if on != floor_on {
                floor_on = on;
                println!("floor grid {} ({})", if on { "ON" } else { "OFF" }, crate::floorgrid::toggle_path().display());
            }
        }
        // the floor grid texture: made and uploaded once, into one swapchain image the quad keeps using
        if floor_on && !floor_ready {
            let bgra = matches!(s.floor_format, VK_FORMAT_B8G8R8A8_SRGB | VK_FORMAT_B8G8R8A8_UNORM);
            let tex = crate::floorgrid::texture(&grid, bgra);
            let f = &mut s.floor;
            // SAFETY: staging_ptr maps staging_len host-coherent bytes; nothing is in flight (uploads wait).
            let dst = unsafe { std::slice::from_raw_parts_mut(f.staging_ptr, f.staging_len) };
            dst[..tex.len()].copy_from_slice(&tex);
            let idx = f.swapchain.acquire_image().map_err(|e| format!("floor xrAcquireSwapchainImage: {e:?}"))? as usize;
            f.swapchain.wait_image(xr::Duration::INFINITE).map_err(|e| format!("floor xrWaitSwapchainImage: {e:?}"))?;
            // SAFETY: the image belongs to the floor swapchain, acquired and waited; staging holds a full image.
            unsafe { s.gpu.upload(f, f.images[idx], FLOOR_PX as u32, FLOOR_PX as u32)? };
            f.swapchain.release_image().map_err(|e| format!("floor xrReleaseSwapchainImage: {e:?}"))?;
            floor_ready = true;
        }
        let floor_pose = if floor_on && floor_ready {
            match s.view_space.locate(&s.stage, fs.predicted_display_time) {
                Ok(l) if l.location_flags.contains(xr::SpaceLocationFlags::POSITION_VALID) => {
                    let (cx, cz) = crate::floorgrid::centre(&grid, l.pose.position.x as f64, l.pose.position.z as f64);
                    let q = crate::floorgrid::FLAT;
                    Some(xr::Posef {
                        orientation: xr::Quaternionf { x: q[0], y: q[1], z: q[2], w: q[3] },
                        position: xr::Vector3f { x: cx as f32, y: 0.0, z: cz as f32 },
                    })
                }
                _ => None,
            }
        } else {
            None
        };
        let floor_rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di { width: FLOOR_PX as i32, height: FLOOR_PX as i32 },
        };
        let quad = floor_pose.map(|pose| {
            xr::CompositionLayerQuad::new()
                .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                .space(&s.stage)
                .eye_visibility(xr::EyeVisibility::BOTH)
                .sub_image(xr::SwapchainSubImage::new().swapchain(&s.floor.swapchain).image_array_index(0).image_rect(floor_rect))
                .pose(pose)
                .size(xr::Extent2Df { width: grid.side_m as f32, height: grid.side_m as f32 })
        });

        let mut layers_submitted = false;
        if show {
            if let Some(views) = content_views {
                for eye in 0..2 {
                    let idx = s.eyes[eye]
                        .swapchain
                        .acquire_image()
                        .map_err(|e| format!("xrAcquireSwapchainImage: {e:?}"))?
                        as usize;
                    s.eyes[eye]
                        .swapchain
                        .wait_image(xr::Duration::INFINITE)
                        .map_err(|e| format!("xrWaitSwapchainImage: {e:?}"))?;
                    if s.eyes[eye].uploaded[idx] != content_gen {
                        // SAFETY: the image belongs to this swapchain, was just acquired and waited, and the staging
                        // buffer holds a complete image of the swapchain's size.
                        unsafe { s.gpu.upload(&s.eyes[eye], s.eyes[eye].images[idx], w, h)? };
                        s.eyes[eye].uploaded[idx] = content_gen;
                    }
                    s.eyes[eye]
                        .swapchain
                        .release_image()
                        .map_err(|e| format!("xrReleaseSwapchainImage: {e:?}"))?;
                }
                let rect = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di {
                        width: w as i32,
                        height: h as i32,
                    },
                };
                let pv = |i: usize| {
                    xr::CompositionLayerProjectionView::new()
                        .pose(views[i].pose)
                        .fov(views[i].fov)
                        .sub_image(
                            xr::SwapchainSubImage::new()
                                .swapchain(&s.eyes[i].swapchain)
                                .image_array_index(0)
                                .image_rect(rect),
                        )
                };
                let proj_views = [pv(0), pv(1)];
                let layer = xr::CompositionLayerProjection::new()
                    .space(&s.local)
                    .views(&proj_views);
                // the grid over the camera image (it marks where the tracker thinks the floor is)
                match &quad {
                    Some(q) => s.stream.end(fs.predicted_display_time, s.blend, &[&*layer, &**q]),
                    None => s.stream.end(fs.predicted_display_time, s.blend, &[&*layer]),
                }
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
                layers_submitted = true;
            }
        }
        if !layers_submitted {
            match &quad {
                Some(q) => s.stream.end(fs.predicted_display_time, s.blend, &[&**q]),
                None => s.stream.end(fs.predicted_display_time, s.blend, &[]),
            }
            .map_err(|e| format!("xrEndFrame: {e:?}"))?;
        }
        frame_count += 1;

        let period = opts.stats_every_s.max(0.5);
        if print_status || last_stats.elapsed().as_secs_f64() >= period {
            let secs = last_stats.elapsed().as_secs_f64();
            let l = lock(&shared.latest);
            println!(
                "status: camera images {:.1}/s (frame {}), remap {} | capture->ready {} | capture->display {} | \
                 depth {}, delay {:.0} ms, gamma {gamma}, {}",
                images as f64 / secs,
                l.counter,
                remap_ms.summary(),
                lat_ready.summary(),
                lat_display.summary(),
                depth_name(depth),
                timing.delay_ns as f64 / 1e6,
                if show { "ON" } else { "OFF" }
            );
            drop(l);
            images = 0;
            lat_ready = LatencyStats::default();
            lat_display = LatencyStats::default();
            remap_ms = LatencyStats::default();
            last_stats = Instant::now();
            print_status = false;
        }
    }
    println!("done: {frame_count} frames");
    shared.stop.store(true, Ordering::Relaxed);
    let _ = cam_thread.join();
    if let Some(j) = job {
        let _ = j.join();
    }
    s.destroy_gpu_objects();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::geom;

    #[test]
    fn depth_steps_walk_the_list() {
        assert_eq!(step_depth(2.0, true), 3.0);
        assert_eq!(step_depth(2.0, false), 1.5);
        assert_eq!(step_depth(0.5, false), 0.5);
        assert!(step_depth(10.0, true).is_infinite());
        assert!(step_depth(f64::INFINITY, true).is_infinite());
        assert_eq!(step_depth(f64::INFINITY, false), 10.0);
        // a value between steps snaps to the next one
        assert_eq!(step_depth(1.2, true), 2.0);
        assert_eq!(depth_name(f64::INFINITY), "infinite");
    }

    #[test]
    fn xr_pose_conversion() {
        // 90 deg about +y: forward (-z) turns to -x (left)
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let p = pose_from_xr(xr::Posef {
            orientation: xr::Quaternionf {
                x: 0.0,
                y: s,
                z: 0.0,
                w: s,
            },
            position: xr::Vector3f {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
        });
        let f = geom::mat_vec(&p.r, [0.0, 0.0, -1.0]);
        assert!(
            (f[0] + 1.0).abs() < 1e-6 && f[1].abs() < 1e-6 && f[2].abs() < 1e-6,
            "{f:?}"
        );
        assert_eq!(p.t, [1.0, 2.0, 3.0]);
        let fov = Fov::dream_air_default();
        let back = fov_from_xr(fov_to_xr(fov));
        assert!((back.left - fov.left).abs() < 1e-6);
    }

    #[test]
    fn map_key_ignores_noise_but_not_changes() {
        let v = EyeView::default_pair(100, 108, 0.063, Fov::dream_air_default());
        let mut v2 = v;
        v2[0].head_from_eye.t[0] += 1e-7;
        assert_eq!(MapKey::new(&v, 2.0), MapKey::new(&v2, 2.0));
        assert_ne!(MapKey::new(&v, 2.0), MapKey::new(&v, 3.0));
        assert_ne!(MapKey::new(&v, 2.0), MapKey::new(&v, f64::INFINITY));
        v2[1].fov.left -= 0.01;
        assert_ne!(MapKey::new(&v, 2.0), MapKey::new(&v2, 2.0));
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn camera_thread_publishes_remapped_images_from_a_replay() {
        let c = DeviceCalib::load(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../pimax-cam/tests/fixtures/headset-calibration.xml"),
        )
        .unwrap();
        let rig =
            Rig::from_calib(&c, geom::scale(crate::rig::DEFAULT_HEAD_OFFSET_MM, 1e-3)).unwrap();
        let views = EyeView::default_pair(64, 70, 0.063, Fov::dream_air_default());
        let maps = build_maps(&rig, views, 2.0);
        assert_eq!(maps.key, MapKey::new(&views, 2.0));
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            freeze: AtomicBool::new(false),
            maps: Mutex::new(Some(Arc::new(maps))),
            latest: Mutex::new(Latest {
                gen: 0,
                mono_ns: 0,
                counter: 0,
                eyes: [Vec::new(), Vec::new()],
                maps: None,
                remap_ms: 0.0,
                error: None,
            }),
        });
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../pimax-cam/tests/fixtures/evening2.pxcam");
        // looping, not real time: frames come as fast as the thread takes them
        let cam = frames::open_source(src.to_str().unwrap(), true, false).unwrap();
        let sh = shared.clone();
        let t = std::thread::spawn(move || camera_thread(sh, cam));
        let t0 = Instant::now();
        while lock(&shared.latest).gen < 3 && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        shared.stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let l = lock(&shared.latest);
        assert!(l.gen >= 3, "gen {}", l.gen);
        assert!(l.error.is_none());
        assert!(l.mono_ns > 0);
        assert_eq!(l.eyes[0].len(), 64 * 70);
        assert!(l.eyes[1].iter().any(|&v| v > 0));
    }

    #[test]
    fn formats_and_sizes() {
        assert_eq!(choose_format(&[44, 50]), Some(50));
        assert_eq!(choose_format(&[37]), Some(37));
        assert_eq!(choose_format(&[1]), None);
        assert_eq!(image_size((3552, 3840), 3), (1184, 1280));
        assert_eq!(format_name(43), "R8G8B8A8_SRGB");
    }
}
