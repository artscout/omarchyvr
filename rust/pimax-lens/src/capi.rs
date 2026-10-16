//! C ABI of the lens model (`include/pimax_lens_rs.h`), called by the thin C driver glue in Monado.
//!
//! All functions are safe to call from several threads once [`pimax_lens_rs_create`] returned (the handle is
//! read-only afterwards). `view` is 0 (left) or 1 (right). Return values: 0 = ok, negative = error.

#![allow(clippy::missing_safety_doc)]

use crate::mask::{visibility_mesh, MaskKind};
use crate::{config, Lens, Level, Message};
use std::ffi::{c_char, c_int, c_void, CString};
use std::path::PathBuf;

/// Incremented when the C ABI changes incompatibly.
pub const ABI_VERSION: u32 = 1;

/// Number of boundary samples of the visibility mask meshes.
const MASK_SEGMENTS: usize = 64;

/// Opaque handle (`pimax_lens_rs` in the header).
pub struct PimaxLensRs {
    lens: Lens,
}

/// Log callback: `level` 0 = info, 1 = warning; `msg` is a NUL terminated line without newline.
pub type LogFn = Option<unsafe extern "C" fn(user: *mut c_void, level: c_int, msg: *const c_char)>;

unsafe fn emit(log: LogFn, user: *mut c_void, messages: &[Message]) {
    let Some(f) = log else { return };
    for m in messages {
        let text: String = m.text.chars().filter(|&c| c != '\0').collect();
        if let Ok(c) = CString::new(text) {
            f(user, if m.level == Level::Warn { 1 } else { 0 }, c.as_ptr());
        }
    }
}

#[no_mangle]
pub extern "C" fn pimax_lens_rs_abi_version() -> u32 {
    ABI_VERSION
}

/// Load the configuration (lens.conf, `PIMAX_LENS_CONF`, `PIMAX_LENS_OFF`), log what is active through `log`, honour
/// `PIMAX_LENS_DEBUG_GRID`, and return a handle. `aspect` = display width / height as the client sees it.
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_create(
    aspect: f32,
    log: LogFn,
    user: *mut c_void,
) -> *mut PimaxLensRs {
    let loaded = crate::load(aspect, &config::process_env);
    emit(log, user, &loaded.messages);
    if let Some(dir) = config::process_env("PIMAX_LENS_DEBUG_GRID").filter(|d| !d.is_empty()) {
        let div = config::process_env("PIMAX_LENS_DEBUG_GRID_DIV")
            .and_then(|d| d.trim().parse::<u32>().ok())
            .filter(|&d| (1..=64).contains(&d))
            .unwrap_or(4);
        let msgs = crate::write_debug_grid(&loaded.lens, &PathBuf::from(dir), div);
        emit(log, user, &msgs);
    }
    Box::into_raw(Box::new(PimaxLensRs { lens: loaded.lens }))
}

#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_destroy(l: *mut PimaxLensRs) {
    if !l.is_null() {
        drop(Box::from_raw(l));
    }
}

unsafe fn handle<'a>(l: *const PimaxLensRs, view: u32) -> Option<&'a Lens> {
    if l.is_null() || view > 1 {
        None
    } else {
        Some(&(*l).lens)
    }
}

/// FOV half-angles in degrees (all positive): left, right, up, down. Writes 4 floats.
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_get_fov(
    l: *const PimaxLensRs,
    view: u32,
    out_deg: *mut f32,
) -> c_int {
    let Some(lens) = handle(l, view) else {
        return -1;
    };
    if out_deg.is_null() {
        return -1;
    }
    std::ptr::copy_nonoverlapping(lens.fov_deg(view as usize).as_ptr(), out_deg, 4);
    0
}

/// Outward cant of each display in degrees (lens.conf `cant_deg`, `PIMAX_CANT_DEG`); 0 on a bad handle.
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_get_cant_deg(l: *const PimaxLensRs) -> f32 {
    if l.is_null() {
        0.0
    } else {
        (*l).lens.config().cant_deg
    }
}

/// The distortion: UV to sample for R, G and B at display point (u, v). Writes 6 floats: r.x r.y g.x g.y b.x b.y.
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_compute(
    l: *const PimaxLensRs,
    view: u32,
    u: f32,
    v: f32,
    out_uv: *mut f32,
) -> c_int {
    let Some(lens) = handle(l, view) else {
        return -1;
    };
    if out_uv.is_null() {
        return -1;
    }
    let uv = lens.compute(view as usize, u, v);
    let out = std::slice::from_raw_parts_mut(out_uv, 6);
    for c in 0..3 {
        out[c * 2] = uv[c][0];
        out[c * 2 + 1] = uv[c][1];
    }
    0
}

/// 1 if `view` has a configured edge black-out / visibility mask (then [`pimax_lens_rs_visibility_mask`] builds
/// meshes), else 0.
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_has_mask(l: *const PimaxLensRs, view: u32) -> c_int {
    handle(l, view).map_or(0, |lens| {
        c_int::from(lens.config().enabled && lens.has_mask(view as usize))
    })
}

/// Visibility mask mesh (`kind`: 0 hidden triangle mesh, 1 visible triangle mesh, 2 line loop). `counts[0]` receives
/// the vertex count, `counts[1]` the index count. Vertices are written as x,y float pairs. Call once with null
/// buffers and zero capacities to learn the sizes (returns -2), then again with buffers. Returns -3 if there is no
/// mask (use the generic one).
#[no_mangle]
pub unsafe extern "C" fn pimax_lens_rs_visibility_mask(
    l: *const PimaxLensRs,
    view: u32,
    kind: c_int,
    vertices_xy: *mut f32,
    vertex_capacity: u32,
    indices: *mut u32,
    index_capacity: u32,
    counts: *mut u32,
) -> c_int {
    let Some(lens) = handle(l, view) else {
        return -1;
    };
    let Some(kind) = MaskKind::from_i32(kind) else {
        return -1;
    };
    if counts.is_null() {
        return -1;
    }
    let Some(mesh) = visibility_mesh(lens, view as usize, kind, MASK_SEGMENTS) else {
        return -3;
    };
    *counts = mesh.vertices.len() as u32;
    *counts.add(1) = mesh.indices.len() as u32;
    if vertices_xy.is_null()
        || indices.is_null()
        || (mesh.vertices.len() as u32) > vertex_capacity
        || (mesh.indices.len() as u32) > index_capacity
    {
        return -2;
    }
    for (i, v) in mesh.vertices.iter().enumerate() {
        *vertices_xy.add(i * 2) = v[0];
        *vertices_xy.add(i * 2 + 1) = v[1];
    }
    std::ptr::copy_nonoverlapping(mesh.indices.as_ptr(), indices, mesh.indices.len());
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The tests set process environment variables; run them one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    unsafe extern "C" fn collect(user: *mut c_void, level: c_int, msg: *const c_char) {
        let v = &mut *(user as *mut Vec<(i32, String)>);
        v.push((
            level,
            std::ffi::CStr::from_ptr(msg).to_string_lossy().into_owned(),
        ));
    }

    #[test]
    fn create_compute_destroy_with_defaults() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("PIMAX_LENS_CONF", "/nonexistent/lens.conf");
        std::env::remove_var("PIMAX_LENS_OFF");
        std::env::remove_var("PIMAX_LENS_DEBUG_GRID");
        let mut log: Vec<(i32, String)> = Vec::new();
        unsafe {
            let l = pimax_lens_rs_create(
                3840.0 / 3552.0,
                Some(collect),
                &mut log as *mut _ as *mut c_void,
            );
            assert!(!l.is_null());
            assert_eq!(
                log[0],
                (
                    0,
                    "pimax lens: no config at /nonexistent/lens.conf, using built-in defaults"
                        .to_string()
                )
            );
            assert!(log.iter().all(|(lv, _)| *lv == 0));
            let mut fov = [0.0f32; 4];
            assert_eq!(pimax_lens_rs_get_fov(l, 0, fov.as_mut_ptr()), 0);
            assert_eq!(fov, [50.0, 50.0, 44.5, 44.5]);
            let mut uv = [0.0f32; 6];
            assert_eq!(pimax_lens_rs_compute(l, 1, 0.5, 0.5, uv.as_mut_ptr()), 0);
            let want = (*l).lens.compute(1, 0.5, 0.5);
            for c in 0..3 {
                assert_eq!((uv[c * 2], uv[c * 2 + 1]), (want[c][0], want[c][1]));
            }
            assert_eq!(pimax_lens_rs_has_mask(l, 0), 0);
            // errors
            assert_eq!(pimax_lens_rs_get_fov(l, 2, fov.as_mut_ptr()), -1);
            assert_eq!(
                pimax_lens_rs_compute(std::ptr::null(), 0, 0.0, 0.0, uv.as_mut_ptr()),
                -1
            );
            assert_eq!(
                pimax_lens_rs_compute(l, 0, 0.0, 0.0, std::ptr::null_mut()),
                -1
            );
            let mut counts = [0u32; 2];
            assert_eq!(
                pimax_lens_rs_visibility_mask(
                    l,
                    0,
                    0,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    counts.as_mut_ptr()
                ),
                -3
            );
            pimax_lens_rs_destroy(l);
            pimax_lens_rs_destroy(std::ptr::null_mut());
        }
        std::env::remove_var("PIMAX_LENS_CONF");
    }

    #[test]
    fn mask_mesh_two_call_protocol_and_debug_grid_env() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("pimax-lens-capi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("lens.conf");
        std::fs::write(&conf, "mask = ellipse 1.0 0.9\n").unwrap();
        std::env::set_var("PIMAX_LENS_CONF", &conf);
        std::env::set_var("PIMAX_LENS_DEBUG_GRID", dir.join("grid"));
        std::env::set_var("PIMAX_LENS_DEBUG_GRID_DIV", "32");
        let mut log: Vec<(i32, String)> = Vec::new();
        unsafe {
            let l = pimax_lens_rs_create(
                3840.0 / 3552.0,
                Some(collect),
                &mut log as *mut _ as *mut c_void,
            );
            assert_eq!(pimax_lens_rs_has_mask(l, 1), 1);
            let mut counts = [0u32; 2];
            for kind in 0..3 {
                let r = pimax_lens_rs_visibility_mask(
                    l,
                    1,
                    kind,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    counts.as_mut_ptr(),
                );
                assert_eq!(r, -2, "kind {kind}");
                assert!(counts[0] > 0 && counts[1] > 0);
                let mut v = vec![0f32; counts[0] as usize * 2];
                let mut idx = vec![0u32; counts[1] as usize];
                let r = pimax_lens_rs_visibility_mask(
                    l,
                    1,
                    kind,
                    v.as_mut_ptr(),
                    counts[0],
                    idx.as_mut_ptr(),
                    counts[1],
                    counts.as_mut_ptr(),
                );
                assert_eq!(r, 0);
                assert!(idx.iter().all(|&i| i < counts[0]));
            }
            assert_eq!(
                pimax_lens_rs_visibility_mask(
                    l,
                    1,
                    9,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    counts.as_mut_ptr()
                ),
                -1
            );
            pimax_lens_rs_destroy(l);
        }
        assert!(
            log.iter().any(|(_, m)| m.contains("debug grid written")),
            "{log:?}"
        );
        assert!(dir.join("grid/pimax_lens_grid_left.png").exists());
        assert!(dir.join("grid/pimax_lens_summary.txt").exists());
        for k in [
            "PIMAX_LENS_CONF",
            "PIMAX_LENS_DEBUG_GRID",
            "PIMAX_LENS_DEBUG_GRID_DIV",
        ] {
            std::env::remove_var(k);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abi_version_is_exported() {
        assert_eq!(pimax_lens_rs_abi_version(), ABI_VERSION);
    }
}
