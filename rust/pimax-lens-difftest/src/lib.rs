//! FFI to the original C lens model (`csrc/old/pimax_lens.c`, symbols renamed `c_*` by build.rs) and helpers to
//! compare it with the Rust crate. The C code reads its configuration from the process environment
//! (`PIMAX_LENS_CONF`, `PIMAX_LENS_OFF`, `XDG_CONFIG_HOME`, `HOME`), so every use goes through [`with_env`], which
//! serialises the tests that touch the environment.

use pimax_lens::config::LensConfig;
use pimax_lens::model::deg2rad;
use pimax_lens::{Lens, Level};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::sync::Mutex;

extern "C" {
    fn shim_log_clear();
    fn shim_log_get() -> *const c_char;
    fn shim_lens_new() -> *mut c_void;
    fn shim_lens_free(l: *mut c_void);
    fn shim_lens_load(l: *mut c_void, aspect: f32);
    fn shim_lens_enabled(l: *mut c_void) -> c_int;
    fn shim_lens_get_fov(l: *mut c_void, view: c_uint, out: *mut f32);
    fn shim_lens_compute(l: *mut c_void, view: c_uint, u: f32, v: f32, out: *mut f32);
}

/// The display aspect the driver passes: `PIMAX_EYE_H / PIMAX_EYE_W` = 3840 / 3552.
pub const ASPECT: f32 = 3840.0 / 3552.0;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` with the given environment variables set (`None` = removed), restoring them afterwards. Holds a global
/// lock, so concurrent tests never see each other's variables. Do not nest.
pub fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
        .collect();
    for (k, v) in vars {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    let r = f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    r
}

/// A log line of the C model: level and text.
pub type LogLine = (Level, String);

/// The original C model, loaded from whatever the environment says at the time of [`COldLens::load`].
pub struct COldLens {
    ptr: *mut c_void,
    pub log: Vec<LogLine>,
}

impl COldLens {
    /// Call the original `pimax_lens_load`. The caller must hold the environment via [`with_env`].
    pub fn load(aspect: f32) -> COldLens {
        // SAFETY: shim functions allocate / fill a plain struct; the log buffer is a static NUL terminated string.
        unsafe {
            let ptr = shim_lens_new();
            assert!(!ptr.is_null());
            shim_log_clear();
            shim_lens_load(ptr, aspect);
            let text = CStr::from_ptr(shim_log_get())
                .to_string_lossy()
                .into_owned();
            let log = text
                .lines()
                .map(|l| {
                    let (body, marker) = l.split_at(l.len() - 1);
                    (
                        if marker == "W" {
                            Level::Warn
                        } else {
                            Level::Info
                        },
                        body.to_string(),
                    )
                })
                .collect();
            COldLens { ptr, log }
        }
    }

    pub fn enabled(&self) -> bool {
        // SAFETY: ptr is a valid handle until drop.
        unsafe { shim_lens_enabled(self.ptr) != 0 }
    }

    /// The `xrt_fov` the C code returns, in radians: left, right, up, down.
    pub fn fov(&self, view: u32) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        // SAFETY: out has room for 4 floats.
        unsafe { shim_lens_get_fov(self.ptr, view, out.as_mut_ptr()) };
        out
    }

    /// r.x r.y g.x g.y b.x b.y
    pub fn compute(&self, view: u32, u: f32, v: f32) -> [f32; 6] {
        let mut out = [0.0f32; 6];
        // SAFETY: out has room for 6 floats.
        unsafe { shim_lens_compute(self.ptr, view, u, v, out.as_mut_ptr()) };
        out
    }
}

impl Drop for COldLens {
    fn drop(&mut self) {
        // SAFETY: ptr came from shim_lens_new and is freed once.
        unsafe { shim_lens_free(self.ptr) };
    }
}

/// The Rust model's result in the flat layout of the C one.
pub fn rust_compute(lens: &Lens, view: u32, u: f32, v: f32) -> [f32; 6] {
    let uv = lens.compute(view as usize, u, v);
    [uv[0][0], uv[0][1], uv[1][0], uv[1][1], uv[2][0], uv[2][1]]
}

/// The FOV in radians as the C driver glue converts it (`-DEG2RAD(left)`, `DEG2RAD(right)`, ...).
pub fn rust_fov_rad(lens: &Lens, view: u32) -> [f32; 4] {
    let d = lens.fov_deg(view as usize);
    [-deg2rad(d[0]), deg2rad(d[1]), deg2rad(d[2]), -deg2rad(d[3])]
}

/// Bit-exact comparison of two float slices; `Err` names the first difference.
pub fn same_bits(what: &str, a: &[f32], b: &[f32]) -> Result<(), String> {
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x.to_bits() != y.to_bits() {
            return Err(format!(
                "{what}[{i}]: C {x:?} ({:#010x}) != Rust {y:?} ({:#010x})",
                x.to_bits(),
                y.to_bits()
            ));
        }
    }
    if a.len() != b.len() {
        return Err(format!("{what}: length {} != {}", a.len(), b.len()));
    }
    Ok(())
}

/// Default config as built by the Rust crate (for tests that do not need a file).
pub fn rust_defaults() -> Lens {
    Lens::new(LensConfig::defaults(ASPECT))
}

/// The (u, v) points the Monado compositor asks the distortion callback for: a 128 x 128 grid through the four
/// rotation matrices (identity, 90 cw, 90 ccw, 180) it applies per view, in the same float arithmetic
/// (`render_distortion.c`: `u = (float)(col / 127.0)`, `uv = rot * (u - .5f, v - .5f) + .5f`).
pub fn compositor_points() -> Vec<(f32, f32)> {
    const N: usize = 128;
    let rots: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, -1.0, 0.0],
        [0.0, -1.0, 1.0, 0.0],
        [-1.0, 0.0, 0.0, -1.0],
    ];
    let dim_minus_one = (N - 1) as f64;
    let mut pts = Vec::new();
    for rot in rots {
        for row in 0..N {
            let v = (row as f64 / dim_minus_one) as f32;
            for col in 0..N {
                let u = (col as f64 / dim_minus_one) as f32;
                let (x, y) = (u - 0.5f32, v - 0.5f32);
                let rx = rot[0] * x + rot[1] * y;
                let ry = rot[2] * x + rot[3] * y;
                pts.push((rx + 0.5f32, ry + 0.5f32));
            }
        }
    }
    pts
}
