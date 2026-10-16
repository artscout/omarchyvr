//! The real C glue of the Monado driver (`pimax_lens.c`, compiled against stand-in headers and linked to the Rust
//! library) against the original C model: through the glue the driver gets exactly the numbers the old code gave.
//! Compiled only when build.rs finds the new glue (PIMAX_LENS_GLUE_DIR, ~/src/monado-lens or ~/src/monado).
#![cfg(have_glue)]
use pimax_lens::mask::{visibility_mesh, MaskKind};
use pimax_lens::Lens;
use pimax_lens_difftest::*;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

#[repr(C)]
struct GlueLens {
    rs: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct XrtFov {
    angle_left: f32,
    angle_right: f32,
    angle_up: f32,
    angle_down: f32,
}

#[repr(C)]
struct XrtVisibilityMask {
    ty: i32,
    index_count: u32,
    vertex_count: u32,
}

extern "C" {
    fn glue_pimax_lens_load(l: *mut GlueLens, aspect: f32);
    fn glue_pimax_lens_destroy(l: *mut GlueLens);
    fn glue_pimax_lens_get_fov(l: *const GlueLens, view: u32) -> XrtFov;
    fn glue_pimax_lens_compute(l: *const GlueLens, view: u32, u: f32, v: f32, out: *mut f32)
        -> i32;
    fn glue_pimax_lens_visibility_mask(
        l: *const GlueLens,
        ty: i32,
        view: u32,
    ) -> *mut XrtVisibilityMask;
    fn free(p: *mut c_void);
}

struct Glue(GlueLens);

impl Glue {
    /// Call the glue's `pimax_lens_load` with the environment of the caller (hold it through `with_env`).
    fn load() -> Glue {
        let mut g = Glue(GlueLens {
            rs: std::ptr::null_mut(),
        });
        // SAFETY: GlueLens is the 8-byte struct of the glue header; load fills it.
        unsafe { glue_pimax_lens_load(&mut g.0, ASPECT) };
        assert!(!g.0.rs.is_null());
        g
    }

    fn fov(&self, view: u32) -> [f32; 4] {
        // SAFETY: valid handle.
        let f = unsafe { glue_pimax_lens_get_fov(&self.0, view) };
        [f.angle_left, f.angle_right, f.angle_up, f.angle_down]
    }

    fn compute(&self, view: u32, u: f32, v: f32) -> [f32; 6] {
        // xrt_uv_triplet = six consecutive floats r.x r.y g.x g.y b.x b.y
        let mut out = [0.0f32; 6];
        // SAFETY: valid handle, out has room for the triplet.
        assert_eq!(
            unsafe { glue_pimax_lens_compute(&self.0, view, u, v, out.as_mut_ptr()) },
            0
        );
        out
    }
}

impl Drop for Glue {
    fn drop(&mut self) {
        // SAFETY: the handle came from load.
        unsafe { glue_pimax_lens_destroy(&mut self.0) };
    }
}

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../analysis/tools/lens.conf.example")
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lens-glue");
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn which_glue_file_is_under_test() {
    let f = env!("PIMAX_LENS_GLUE_FILE");
    println!("glue under test: {f}");
    assert!(std::fs::read_to_string(f)
        .unwrap()
        .contains("pimax_lens_rs_create"));
}

#[test]
fn glue_matches_the_old_c_model_bit_for_bit_for_the_current_lens_conf() {
    let conf = example();
    let (old, glue) = with_env(
        &[
            ("PIMAX_LENS_CONF", conf.to_str()),
            ("PIMAX_LENS_OFF", None),
            ("PIMAX_LENS_DEBUG_GRID", None),
        ],
        || (COldLens::load(ASPECT), Glue::load()),
    );
    for view in 0..2u32 {
        assert_eq!(
            old.fov(view).map(f32::to_bits),
            glue.fov(view).map(f32::to_bits),
            "fov view {view}"
        );
        for (u, v) in compositor_points() {
            let (a, b) = (old.compute(view, u, v), glue.compute(view, u, v));
            same_bits(&format!("view {view} uv({u:?},{v:?})"), &a, &b).unwrap();
        }
    }
}

#[test]
fn glue_with_distortion_off_and_random_old_style_configs() {
    let pts: Vec<(f32, f32)> = compositor_points().into_iter().step_by(97).collect();
    for (i, text) in [
        "distortion = off\nfov = 45 46 47 48\n",
        "k_r = 0.1 -0.2 0.3\nca_b = 0.99\ncenter = 0.05 -0.02\nfov = 60 55 50 52\n",
        "[left]\nscale = 1.1\n[right]\nbase = 0.7\n",
    ]
    .iter()
    .enumerate()
    {
        let f = tmp(&format!("c{i}.conf"));
        std::fs::write(&f, text).unwrap();
        let (old, glue) = with_env(
            &[("PIMAX_LENS_CONF", f.to_str()), ("PIMAX_LENS_OFF", None)],
            || (COldLens::load(ASPECT), Glue::load()),
        );
        for view in 0..2u32 {
            assert_eq!(
                old.fov(view).map(f32::to_bits),
                glue.fov(view).map(f32::to_bits),
                "config {i} fov"
            );
            for &(u, v) in &pts {
                same_bits(
                    &format!("config {i} view {view} ({u:?},{v:?})"),
                    &old.compute(view, u, v),
                    &glue.compute(view, u, v),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn glue_returns_no_custom_mask_by_default_and_a_valid_one_when_configured() {
    let none = with_env(
        &[
            ("PIMAX_LENS_CONF", Some("/nonexistent/x.conf")),
            ("PIMAX_LENS_OFF", None),
        ],
        Glue::load,
    );
    for ty in [1, 2, 3] {
        // SAFETY: valid handle; NULL is the documented answer without a mask.
        assert!(
            unsafe { glue_pimax_lens_visibility_mask(&none.0, ty, 0) }.is_null(),
            "type {ty}"
        );
    }
    let f = tmp("mask.conf");
    let text = "mask = ellipse 1.0 0.9\nblackout = 0.02\n";
    std::fs::write(&f, text).unwrap();
    let glue = with_env(
        &[("PIMAX_LENS_CONF", f.to_str()), ("PIMAX_LENS_OFF", None)],
        Glue::load,
    );
    let (cfg, w) = pimax_lens::config::parse(text, ASPECT);
    assert!(w.is_empty());
    let lens = Lens::new(cfg);
    for (ty, kind) in [
        (1, MaskKind::HiddenTriangleMesh),
        (2, MaskKind::VisibleTriangleMesh),
        (3, MaskKind::LineLoop),
    ] {
        for view in 0..2u32 {
            // SAFETY: valid handle; the result is a malloc'd mask that this test frees.
            let m = unsafe { glue_pimax_lens_visibility_mask(&glue.0, ty, view) };
            assert!(!m.is_null(), "type {ty} view {view}");
            let want = visibility_mesh(&lens, view as usize, kind, 64).unwrap();
            // SAFETY: m points to header + indices + vertices as the Monado helpers lay it out.
            unsafe {
                assert_eq!((*m).ty, ty);
                assert_eq!((*m).vertex_count as usize, want.vertices.len());
                assert_eq!((*m).index_count as usize, want.indices.len());
                let idx =
                    std::slice::from_raw_parts(m.add(1) as *const u32, (*m).index_count as usize);
                assert_eq!(idx, &want.indices[..]);
                let verts = std::slice::from_raw_parts(
                    idx.as_ptr().add(idx.len()) as *const f32,
                    (*m).vertex_count as usize * 2,
                );
                for (i, v) in want.vertices.iter().enumerate() {
                    assert_eq!(
                        (verts[i * 2].to_bits(), verts[i * 2 + 1].to_bits()),
                        (v[0].to_bits(), v[1].to_bits())
                    );
                }
                free(m as *mut c_void);
            }
        }
    }
}
