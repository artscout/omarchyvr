//! The OpenXR (Vulkan) application: session, one swapchain per eye drawn by the GPU pass ([`crate::gpu`]), the
//! head-locked text as a VIEW-space quad layer (`pimax_lenstest::hudquad`, so it is single and sharp with the
//! canted displays), controller and terminal input, the raw EyeChip session (`pimax_eye::live::Tracker`, no
//! calibration: the procedure needs the raw per-eye rays), and the end-to-end check through the runtime's
//! `XR_EXT_eye_gaze_interaction` pose.
//!
//! The chip is used directly while the runtime does not use it: Monado opens it only while an app locates an eye
//! gaze space. For the end-to-end check this app releases its own session and creates the gaze space; the runtime
//! then starts its session with the calibration file just saved.

use crate::collect::RawSample;
use crate::gpu::{block, EyeView, MarksGpu};
use crate::procedure::{Cmd, Effect, Procedure};
use crate::report;
use ash::vk::{self, Handle};
use openxr as xr;
use pimax_eye::live::{Config as EyeConfig, State as EyeState, Tracker};
use pimax_eye::track::FrameConv;
use pimax_lenstest::color;
use pimax_lenstest::geometry::{quat_to_mat, Mat3};
use pimax_lenstest::hudquad;
use pimax_lenstest::image::default_threads;
use pimax_lenstest::xr::find_memory_type;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Run options.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Render resolution divisor.
    pub res_div: u32,
    /// Where the calibration is written.
    pub calibration_path: Option<PathBuf>,
    /// Write every raw sample to a CSV next to the results.
    pub raw_log: bool,
}

struct RawTerminal {
    saved: Option<libc::termios>,
}

impl RawTerminal {
    fn enable() -> RawTerminal {
        // SAFETY: termios calls on fd 0 with a struct tcgetattr fills in.
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

/// What a key means.
pub fn key_command(c: char) -> Option<Option<Cmd>> {
    Some(match c {
        ' ' | 'a' | '\n' => Some(Cmd::Next),
        'r' | 'b' => Some(Cmd::Redo),
        'y' | 'm' => Some(Cmd::Accept),
        'p' => Some(Cmd::Pursuit),
        'c' => Some(Cmd::Cursor),
        's' => Some(Cmd::Service),
        'q' | '\x1b' => None,
        _ => return None,
    })
}

/// Controller buttons (A/X next, B/Y redo, menu save, trigger next).
struct Controls {
    set: xr::ActionSet,
    hands: [xr::Path; 2],
    next: xr::Action<bool>,
    redo: xr::Action<bool>,
    menu: xr::Action<bool>,
    trigger: xr::Action<f32>,
    prev: [bool; 4],
}

impl Controls {
    fn new(instance: &xr::Instance) -> Result<Controls, String> {
        let e = |what: &str, r: xr::sys::Result| format!("{what}: {r:?}");
        let path = |s: &str| instance.string_to_path(s).map_err(|r| e(s, r));
        let hands = [path("/user/hand/left")?, path("/user/hand/right")?];
        let set = instance
            .create_action_set("eyecal", "eyecal", 0)
            .map_err(|r| e("action set", r))?;
        let next = set.create_action::<bool>("next", "next", &hands).map_err(|r| e("next", r))?;
        let redo = set.create_action::<bool>("redo", "redo", &hands).map_err(|r| e("redo", r))?;
        let menu = set.create_action::<bool>("save", "save", &hands).map_err(|r| e("save", r))?;
        let trigger = set
            .create_action::<f32>("trigger", "trigger", &hands)
            .map_err(|r| e("trigger", r))?;
        let b = [
            xr::Binding::new(&next, path("/user/hand/left/input/x/click")?),
            xr::Binding::new(&next, path("/user/hand/right/input/a/click")?),
            xr::Binding::new(&redo, path("/user/hand/left/input/y/click")?),
            xr::Binding::new(&redo, path("/user/hand/right/input/b/click")?),
            xr::Binding::new(&menu, path("/user/hand/left/input/menu/click")?),
            xr::Binding::new(&trigger, path("/user/hand/left/input/trigger/value")?),
            xr::Binding::new(&trigger, path("/user/hand/right/input/trigger/value")?),
        ];
        instance
            .suggest_interaction_profile_bindings(path("/interaction_profiles/oculus/touch_controller")?, &b)
            .map_err(|r| e("suggest bindings", r))?;
        // Valve Index controllers (SteamVR with the Lighthouse setup): A next, B redo / save-and-exit, the trigger
        let idx = [
            xr::Binding::new(&next, path("/user/hand/left/input/a/click")?),
            xr::Binding::new(&next, path("/user/hand/right/input/a/click")?),
            xr::Binding::new(&redo, path("/user/hand/left/input/b/click")?),
            xr::Binding::new(&redo, path("/user/hand/right/input/b/click")?),
            xr::Binding::new(&trigger, path("/user/hand/left/input/trigger/value")?),
            xr::Binding::new(&trigger, path("/user/hand/right/input/trigger/value")?),
        ];
        if let Err(r) = instance
            .suggest_interaction_profile_bindings(path("/interaction_profiles/valve/index_controller")?, &idx)
        {
            println!("Index controller bindings not taken: {r:?}");
        }
        Ok(Controls {
            set,
            hands,
            next,
            redo,
            menu,
            trigger,
            prev: [false; 4],
        })
    }

    /// Commands from button presses since the last call (edges only).
    fn poll<G>(&mut self, session: &xr::Session<G>, extra: Option<&xr::ActionSet>) -> Vec<Cmd> {
        let mut active = vec![xr::ActiveActionSet::new(&self.set)];
        if let Some(e) = extra {
            active.push(xr::ActiveActionSet::new(e));
        }
        if session.sync_actions(&active).is_err() {
            return vec![];
        }
        let mut now = [false; 4];
        for &h in &self.hands {
            now[0] |= self.next.state(session, h).is_ok_and(|s| s.current_state);
            now[1] |= self.redo.state(session, h).is_ok_and(|s| s.current_state);
            now[2] |= self.menu.state(session, h).is_ok_and(|s| s.current_state);
            now[3] |= self.trigger.state(session, h).is_ok_and(|s| s.current_state > 0.7);
        }
        let mut out = Vec::new();
        let cmds = [Cmd::Next, Cmd::Redo, Cmd::Accept, Cmd::Start];
        for i in 0..4 {
            if now[i] && !self.prev[i] {
                out.push(cmds[i]);
            }
        }
        self.prev = now;
        out
    }
}

/// The runtime's eye gaze action (attached at start; its space is only created for the end-to-end check, which is
/// what makes the runtime open the chip).
struct Gaze {
    set: xr::ActionSet,
    action: xr::Action<xr::Posef>,
    space: Option<xr::Space>,
}

impl Gaze {
    fn new(instance: &xr::Instance) -> Result<Gaze, String> {
        let e = |what: &str, r: xr::sys::Result| format!("{what}: {r:?}");
        let set = instance.create_action_set("gaze", "gaze", 0).map_err(|r| e("gaze set", r))?;
        let action = set
            .create_action::<xr::Posef>("gaze_pose", "gaze pose", &[])
            .map_err(|r| e("gaze action", r))?;
        let profile = instance
            .string_to_path("/interaction_profiles/ext/eye_gaze_interaction")
            .map_err(|r| e("profile", r))?;
        let pose = instance
            .string_to_path("/user/eyes_ext/input/gaze_ext/pose")
            .map_err(|r| e("pose path", r))?;
        instance
            .suggest_interaction_profile_bindings(profile, &[xr::Binding::new(&action, pose)])
            .map_err(|r| e("suggest gaze", r))?;
        Ok(Gaze { set, action, space: None })
    }
}

fn forward(q: xr::Quaternionf) -> [f64; 3] {
    let (x, y, z, w) = (f64::from(q.x), f64::from(q.y), f64::from(q.z), f64::from(q.w));
    // rotate (0, 0, -1)
    [-(2.0 * (x * z + w * y)), -(2.0 * (y * z - w * x)), -(1.0 - 2.0 * (x * x + y * y))]
}

struct HudQuad {
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
    uploaded: Vec<u64>,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut u8,
    staging_len: usize,
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
        d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
            .map_err(|r| err("reset", r))?;
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
            image_extent: vk::Extent3D { width: w, height: h, depth: 1 },
        };
        d.cmd_copy_buffer_to_image(self.cmd, staging, image, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
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
        d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)
            .map_err(|r| err("submit", r))?;
        d.wait_for_fences(&[self.fence], true, u64::MAX).map_err(|r| err("wait", r))?;
        d.reset_fences(&[self.fence]).map_err(|r| err("reset fence", r))
    }
}

/// The raw EyeChip session of this app (no calibration, no filter, no correction).
fn start_tracker(conv: FrameConv) -> Tracker {
    let mut cfg = EyeConfig {
        conv,
        ..EyeConfig::default()
    };
    cfg.usb_device = std::env::var_os("PIMAX_EYE_USB").map(PathBuf::from);
    // the start list of a headset without a built-in one (the Crystal Super, `--frames`): with its own profile
    // upload, exactly as `omarchyvr eyed` starts the chip, so the fit sees the raw gaze the service will see
    if let Some(p) = std::env::var_os("PIMAX_EYE_FRAMES") {
        match std::fs::read_to_string(&p) {
            Ok(t) => {
                cfg.frames = Some(t.into());
                cfg.with_cal = true;
            }
            Err(e) => eprintln!("PIMAX_EYE_FRAMES {}: {e}", PathBuf::from(&p).display()),
        }
    }
    // the user's current eye profile in place of the list's own, as the service uploads it
    if let Some(p) = std::env::var_os("PIMAX_EYE_PROFILE") {
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| {
            pimax_eye::frames::check_profile(&b)?;
            Ok(b)
        }) {
            Ok(b) => cfg.profile = Some(b.into()),
            Err(e) => eprintln!("PIMAX_EYE_PROFILE {}: {e}", PathBuf::from(&p).display()),
        }
    }
    cfg.filter = Some(pimax_eye::filter::FilterParams {
        enabled: false,
        ..Default::default()
    });
    cfg.history = 1024;
    Tracker::start(cfg)
}

fn raw_csv_header() -> &'static str {
    "t_s,tag,l_valid,l_x,l_y,l_z,l_ox,l_oy,l_oz,l_pupil,r_valid,r_x,r_y,r_z,r_ox,r_oy,r_oz,r_pupil,head_qx,head_qy,head_qz,head_qw"
}

fn raw_csv_row(s: &RawSample, tag: &str, head: Option<[f64; 4]>) -> String {
    let mut row = format!("{:.6},{tag}", s.t);
    for e in &s.eyes {
        match e {
            Some(e) => row.push_str(&format!(
                ",1,{:.6},{:.6},{:.6},{:.5},{:.5},{:.5},{:.3}",
                e.dir[0], e.dir[1], e.dir[2], e.origin[0], e.origin[1], e.origin[2], e.pupil_mm
            )),
            None => row.push_str(",0,,,,,,,"),
        }
    }
    match head {
        Some(q) => row.push_str(&format!(",{:.6},{:.6},{:.6},{:.6}", q[0], q[1], q[2], q[3])),
        None => row.push_str(",,,,"),
    }
    row
}

/// Run the application.
pub fn run(mut proc: Procedure, opts: Opts) -> Result<(), String> {
    // SAFETY: installs async-signal-safe handlers that only store an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    let stamp = report::stamp(report::unix_now());
    let dir = report::results_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut raw_log = if opts.raw_log {
        let p = dir.join(format!("eyecal-{stamp}-raw.csv"));
        match std::fs::File::create(&p) {
            Ok(f) => {
                println!("raw samples: {}", p.display());
                let mut w = std::io::BufWriter::new(f);
                let _ = writeln!(w, "{}", raw_csv_header());
                Some(w)
            }
            Err(e) => {
                println!("warning: no raw log ({}: {e})", p.display());
                None
            }
        }
    } else {
        None
    };
    // SAFETY: the XR_KHR_vulkan_enable2 sequence of the openxr crate's example, as in pimax-lenstest.
    let (instance, system, session, mut waiter, mut stream, vk_instance, device, queue, qf, mem_props, gaze_ext) = unsafe {
        let entry = xr::Entry::load(&()).map_err(|e| format!("cannot load the OpenXR loader: {e}"))?;
        let exts = entry.enumerate_extensions().map_err(|e| format!("extensions: {e:?}"))?;
        if !exts.khr_vulkan_enable2 {
            return Err("the OpenXR runtime does not offer XR_KHR_vulkan_enable2".into());
        }
        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        enabled.ext_eye_gaze_interaction = exts.ext_eye_gaze_interaction;
        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "pimax-eyecal",
                    application_version: 1,
                    engine_name: "pimax-eyecal",
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
        std::mem::forget(vk_entry); // the loader stays loaded for the process lifetime
        (instance, system, session, waiter, stream, vk_instance, device, queue, qf, mem_props, enabled.ext_eye_gaze_interaction)
    };
    let _ = system;
    let view_space = session
        .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
        .map_err(|e| format!("view space: {e:?}"))?;
    let local_space = session
        .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
        .ok();
    let mut controls = Controls::new(&instance)?;
    let mut gaze = if gaze_ext { Gaze::new(&instance).ok() } else { None };
    {
        let mut sets = vec![&controls.set];
        if let Some(g) = &gaze {
            sets.push(&g.set);
        }
        session
            .attach_action_sets(&sets)
            .map_err(|e| format!("attach action sets: {e:?}"))?;
    }
    let views = instance
        .enumerate_view_configuration_views(system, VIEW_TYPE)
        .map_err(|e| format!("views: {e:?}"))?;
    if views.len() != 2 {
        return Err(format!("expected 2 views, got {}", views.len()));
    }
    let div = opts.res_div.max(1);
    let size = (
        (views[0].recommended_image_rect_width / div).max(16),
        (views[0].recommended_image_rect_height / div).max(16),
    );
    let offered: Vec<u32> = session
        .enumerate_swapchain_formats()
        .map_err(|e| format!("swapchain formats: {e:?}"))?;
    let format = color::choose_format(&offered, false).ok_or("no 8-bit RGBA/BGRA swapchain format")?;
    println!(
        "render {}x{} per eye ({}), runtime {}",
        size.0,
        size.1,
        color::format_name(format),
        instance.properties().map(|p| p.runtime_name).unwrap_or_default()
    );
    let mut eye_sc = Vec::new();
    for _ in 0..2 {
        let sc = session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::SAMPLED,
                format,
                sample_count: 1,
                width: size.0,
                height: size.1,
                face_count: 1,
                array_size: 1,
                mip_count: 1,
            })
            .map_err(|e| format!("eye swapchain: {e:?}"))?;
        let imgs: Vec<vk::Image> = sc
            .enumerate_images()
            .map_err(|e| format!("swapchain images: {e:?}"))?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect();
        eye_sc.push((sc, imgs));
    }
    // SAFETY: device, queue and images are valid for the session's lifetime; destroyed before exit.
    let mut gpu = unsafe {
        MarksGpu::new(
            &device,
            queue,
            qf,
            &mem_props,
            vk::Format::from_raw(format as i32),
            size,
            [&eye_sc[0].1, &eye_sc[1].1],
        )?
    };
    // SAFETY: plain Vulkan object creation on the session's device.
    let upload = unsafe {
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qf)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|r| format!("command pool: {r:?}"))?;
        let cmd = device
            .allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1))
            .map_err(|r| format!("command buffer: {r:?}"))?[0];
        let fence = device.create_fence(&vk::FenceCreateInfo::default(), None).map_err(|r| format!("fence: {r:?}"))?;
        Upload { device: device.clone(), queue, pool, cmd, fence }
    };
    // SAFETY: as above; the staging memory stays mapped until the end.
    let mut hud = unsafe {
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
            .map_err(|e| format!("text quad swapchain: {e:?}"))?;
        let images: Vec<vk::Image> = swapchain
            .enumerate_images()
            .map_err(|e| format!("text quad images: {e:?}"))?
            .into_iter()
            .map(vk::Image::from_raw)
            .collect();
        let staging_len = hudquad::TEX_W as usize * hudquad::TEX_H as usize * 4;
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_len as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|r| format!("text staging: {r:?}"))?;
        let req = device.get_buffer_memory_requirements(staging);
        let mt = find_memory_type(
            &mem_props.memory_types[..mem_props.memory_type_count as usize],
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .ok_or("no host visible memory for the text")?;
        let staging_mem = device
            .allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(mt), None)
            .map_err(|r| format!("text memory: {r:?}"))?;
        device
            .bind_buffer_memory(staging, staging_mem, 0)
            .map_err(|r| format!("text bind: {r:?}"))?;
        let staging_ptr = device
            .map_memory(staging_mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .map_err(|r| format!("text map: {r:?}"))? as *mut u8;
        HudQuad {
            uploaded: vec![0; images.len()],
            swapchain,
            images,
            staging,
            staging_mem,
            staging_ptr,
            staging_len,
        }
    };

    let _term = RawTerminal::enable();
    let keys = spawn_keyboard();
    println!(
        "keys: space/a next  r redo  y save  p pursuit  c cursor  s runtime check  q quit   (controllers: A/X next, B/Y redo, menu save, trigger next)"
    );
    let mut tracker = Some(start_tracker(proc.s.conv));
    let mut last_seq: Option<u64> = None;
    let mut service = false;
    let mut service_started: Option<Instant> = None;
    let mut running = false;
    let mut event_buf = xr::EventDataBuffer::new();
    let mut hud_sig: Option<Vec<String>> = None;
    let mut hud_gen = 0u64;
    let mut last_state = None;
    let mut exit_at: Option<Instant> = None;
    let mut frames = 0u64;
    let mut slow = 0u64;
    let t_start = Instant::now();
    let calibration_path = opts
        .calibration_path
        .clone()
        .or_else(pimax_eye::calib::Calibration::default_path);
    'main: loop {
        if STOP.load(Ordering::Relaxed) && exit_at.is_none() {
            let _ = session.request_exit();
            exit_at = Some(Instant::now());
        }
        while let Some(ev) = instance.poll_event(&mut event_buf).map_err(|e| format!("poll_event: {e:?}"))? {
            match ev {
                xr::Event::SessionStateChanged(e) => match e.state() {
                    xr::SessionState::READY => {
                        session.begin(VIEW_TYPE).map_err(|e| format!("xrBeginSession: {e:?}"))?;
                        running = true;
                    }
                    xr::SessionState::STOPPING => {
                        session.end().map_err(|e| format!("xrEndSession: {e:?}"))?;
                        running = false;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => break 'main,
                    _ => {}
                },
                xr::Event::InstanceLossPending(_) => break 'main,
                _ => {}
            }
        }
        if !running {
            if exit_at.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let fs = waiter.wait().map_err(|e| format!("xrWaitFrame: {e:?}"))?;
        stream.begin().map_err(|e| format!("xrBeginFrame: {e:?}"))?;
        let t_frame = Instant::now();
        let (_, xviews) = session
            .locate_views(VIEW_TYPE, fs.predicted_display_time, &view_space)
            .map_err(|e| format!("xrLocateViews: {e:?}"))?;
        let head = local_space.as_ref().and_then(|l| {
            let loc = view_space.locate(l, fs.predicted_display_time).ok()?;
            loc.location_flags
                .contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
                .then(|| {
                    let q = loc.pose.orientation;
                    [f64::from(q.x), f64::from(q.y), f64::from(q.z), f64::from(q.w)]
                })
        });
        let eye_pos = [0, 1].map(|i| {
            let p = xviews[i].pose.position;
            [f64::from(p.x), f64::from(p.y), f64::from(p.z)]
        });
        // input
        let mut cmds = controls.poll(&session, gaze.as_ref().map(|g| &g.set));
        while let Ok(c) = keys.try_recv() {
            match key_command(c.to_ascii_lowercase()) {
                Some(Some(cmd)) => cmds.push(cmd),
                Some(None) if exit_at.is_none() => {
                    let _ = session.request_exit();
                    exit_at = Some(Instant::now());
                }
                Some(None) => {}
                None => {}
            }
        }
        // samples
        let mut batch = Vec::new();
        if let Some(tr) = &tracker {
            let st = tr.state();
            if Some(st) != last_state {
                println!("eye tracker: {st:?}");
                last_state = Some(st);
            }
            for s in tr.samples_since(last_seq) {
                last_seq = Some(s.seq);
                batch.push(RawSample::from_sample(&s));
            }
        }
        let service_dir = if service {
            gaze.as_ref().and_then(|g| {
                let loc = g.space.as_ref()?.locate(&view_space, fs.predicted_display_time).ok()?;
                loc.location_flags
                    .contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
                    .then(|| forward(loc.pose.orientation))
            })
        } else {
            None
        };
        let now = pimax_eye::clock::monotonic_ns() as f64 * 1e-9;
        let tag = proc.tag();
        if let Some(w) = raw_log.as_mut() {
            for s in &batch {
                let _ = writeln!(w, "{}", raw_csv_row(s, &tag, head));
            }
        }
        let mut fx = proc.frame(now, head, eye_pos, &batch, service_dir);
        for c in cmds {
            fx.extend(proc.command(c));
        }
        for e in fx {
            match e {
                Effect::Print(s) => println!("{s}"),
                Effect::SaveResults(json) => {
                    let p = dir.join(format!("eyecal-{stamp}.json"));
                    match report::write_with_backup(&p, &json) {
                        Ok(_) => println!("results: {}", p.display()),
                        Err(e) => println!("could not save the results: {e}"),
                    }
                }
                Effect::SaveCalibration(text) => match &calibration_path {
                    Some(p) => match report::write_with_backup(p, &text) {
                        Ok(b) => println!(
                            "calibration saved: {}{} (gaze apps started from now on use it; a running Monado reads it when the next app starts the eye tracker)",
                            p.display(),
                            b.map_or(String::new(), |b| format!(", previous kept as {}", b.display()))
                        ),
                        Err(e) => println!("could not save the calibration: {e}"),
                    },
                    None => println!("no $HOME: calibration not saved"),
                },
                Effect::Quit => {
                    if exit_at.is_none() {
                        let _ = session.request_exit();
                        exit_at = Some(Instant::now());
                    }
                }
                Effect::RestartTracker(conv) => {
                    if let Some(t) = tracker.take() {
                        t.stop();
                    }
                    last_seq = None;
                    tracker = Some(start_tracker(conv));
                }
                Effect::StartService => {
                    if let Some(t) = tracker.take() {
                        t.stop(); // unsubscribes and releases the chip
                    }
                    match gaze.as_mut() {
                        Some(g) => match g.action.create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY) {
                            Ok(s) => {
                                g.space = Some(s);
                                service = true;
                                service_started = Some(Instant::now());
                            }
                            Err(e) => println!("gaze space: {e:?}"),
                        },
                        None => println!("the runtime offers no XR_EXT_eye_gaze_interaction (PIMAX_EYE=1 service?)"),
                    }
                }
                Effect::StopService => {
                    if let Some(g) = gaze.as_mut() {
                        g.space = None; // the runtime releases the chip
                    }
                    service = false;
                    std::thread::sleep(Duration::from_millis(300));
                    last_seq = None;
                    tracker = Some(start_tracker(proc.s.conv));
                }
            }
        }
        if let Some(t0) = service_started {
            if service && t0.elapsed() > Duration::from_secs(1) && service_dir.is_none() && frames.is_multiple_of(360) {
                println!("waiting for gaze from the runtime...");
            }
        }
        // draw
        let scene = proc.scene();
        let mut hud_lines = scene.hud.clone();
        if let Some(tr) = &tracker {
            match tr.state() {
                EyeState::Streaming => {}
                s => {
                    hud_lines.insert(0, format!("EYE TRACKER: {}", format!("{s:?}").to_ascii_uppercase()));
                }
            }
        }
        for (eye, (sc, _)) in eye_sc.iter_mut().enumerate() {
            let q = xviews[eye].pose.orientation;
            let f = xviews[eye].fov;
            let v = EyeView {
                rot: quat_to_mat([q.x, q.y, q.z, q.w]) as Mat3,
                pos: [xviews[eye].pose.position.x, xviews[eye].pose.position.y, xviews[eye].pose.position.z],
                tan: [(-f.angle_left).tan(), f.angle_right.tan(), f.angle_up.tan(), (-f.angle_down).tan()],
                size,
            };
            let b = block(&scene, &v);
            let idx = sc.acquire_image().map_err(|e| format!("acquire: {e:?}"))? as usize;
            sc.wait_image(xr::Duration::INFINITE).map_err(|e| format!("wait image: {e:?}"))?;
            gpu.draw(eye, idx, &b)?;
            sc.release_image().map_err(|e| format!("release: {e:?}"))?;
        }
        let mut quad_visible = false;
        if !hud_lines.is_empty() {
            if hud_sig.as_ref() != Some(&hud_lines) {
                let img = hudquad::render(&hud_lines);
                // SAFETY: staging maps staging_len bytes; no upload from it is in flight (copies wait).
                let out = unsafe { std::slice::from_raw_parts_mut(hud.staging_ptr, hud.staging_len) };
                img.to_upload(color::encoding_of(format), color::is_bgra(format), default_threads(), out);
                hudquad::alpha_from_black(out);
                hud_sig = Some(hud_lines.clone());
                hud_gen += 1;
            }
            let idx = hud.swapchain.acquire_image().map_err(|e| format!("acquire text: {e:?}"))? as usize;
            hud.swapchain
                .wait_image(xr::Duration::INFINITE)
                .map_err(|e| format!("wait text: {e:?}"))?;
            if hud.uploaded[idx] != hud_gen {
                // SAFETY: image of this swapchain, acquired and waited; staging holds a full texture.
                unsafe { upload.copy(hud.staging, hud.images[idx], hudquad::TEX_W, hudquad::TEX_H)? };
                hud.uploaded[idx] = hud_gen;
            }
            hud.swapchain.release_image().map_err(|e| format!("release text: {e:?}"))?;
            quad_visible = true;
        }
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: size.0 as i32,
                height: size.1 as i32,
            },
        };
        let pv = |i: usize| {
            xr::CompositionLayerProjectionView::new()
                .pose(xviews[i].pose)
                .fov(xviews[i].fov)
                .sub_image(
                    xr::SwapchainSubImage::new()
                        .swapchain(&eye_sc[i].0)
                        .image_array_index(0)
                        .image_rect(rect),
                )
        };
        let proj_views = [pv(0), pv(1)];
        let layer = xr::CompositionLayerProjection::new().space(&view_space).views(&proj_views);
        let [px, py, pz] = hudquad::SPEC.position();
        let quad = xr::CompositionLayerQuad::new()
            .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
            .space(&view_space)
            .eye_visibility(xr::EyeVisibility::BOTH)
            .pose(xr::Posef {
                orientation: xr::Quaternionf { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
                position: xr::Vector3f { x: px, y: py, z: pz },
            })
            .size(xr::Extent2Df {
                width: hudquad::SPEC.width,
                height: hudquad::SPEC.height,
            })
            .sub_image(
                xr::SwapchainSubImage::new()
                    .swapchain(&hud.swapchain)
                    .image_array_index(0)
                    .image_rect(xr::Rect2Di {
                        offset: xr::Offset2Di { x: 0, y: 0 },
                        extent: xr::Extent2Di {
                            width: hudquad::TEX_W as i32,
                            height: hudquad::TEX_H as i32,
                        },
                    }),
            );
        if quad_visible {
            stream
                .end(fs.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[&layer, &quad])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
        } else {
            stream
                .end(fs.predicted_display_time, xr::EnvironmentBlendMode::OPAQUE, &[&layer])
                .map_err(|e| format!("xrEndFrame: {e:?}"))?;
        }
        frames += 1;
        if t_frame.elapsed() > Duration::from_millis(8) {
            slow += 1;
        }
        if frames.is_multiple_of(3600) {
            println!(
                "{} frames in {:.0} s, {} took more than 8 ms of CPU+GPU",
                frames,
                t_start.elapsed().as_secs_f64(),
                slow
            );
        }
    }
    if let Some(t) = tracker.take() {
        t.stop();
    }
    if let Some(w) = raw_log.as_mut() {
        let _ = w.flush();
    }
    // SAFETY: the device is idle after device_wait_idle; every object below was created above, destroyed once.
    unsafe {
        let _ = device.device_wait_idle();
        gpu.destroy();
        device.unmap_memory(hud.staging_mem);
        device.destroy_buffer(hud.staging, None);
        device.free_memory(hud.staging_mem, None);
        device.destroy_fence(upload.fence, None);
        device.destroy_command_pool(upload.pool, None);
    }
    let _ = vk_instance; // instance and device belong to the runtime session; released with the process
    println!("{frames} frames, {slow} slow");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_of_identity_and_yaw() {
        let f = forward(xr::Quaternionf { x: 0.0, y: 0.0, z: 0.0, w: 1.0 });
        assert!((f[2] + 1.0).abs() < 1e-9);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let f = forward(xr::Quaternionf { x: 0.0, y: -h, z: 0.0, w: h });
        assert!((f[0] - 1.0).abs() < 1e-6, "{f:?}");
        let f = forward(xr::Quaternionf { x: h, y: 0.0, z: 0.0, w: h });
        assert!((f[1] - 1.0).abs() < 1e-6, "{f:?}");
    }

    #[test]
    fn keys_and_csv() {
        assert_eq!(key_command(' '), Some(Some(Cmd::Next)));
        assert_eq!(key_command('y'), Some(Some(Cmd::Accept)));
        assert_eq!(key_command('q'), Some(None));
        assert_eq!(key_command('z'), None);
        let s = RawSample { t: 1.5, eyes: [None, None] };
        let row = raw_csv_row(&s, "Intro", None);
        assert_eq!(row.split(',').count(), raw_csv_header().split(',').count());
    }
}
