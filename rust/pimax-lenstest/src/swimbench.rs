//! Offline benchmark of the CPU renderer of the swim pattern (what the old client did every frame per eye):
//! shading, conversion to the swapchain bytes. The upload itself needs a headset session.

use crate::color::Encoding;
use crate::geometry::Fov;
use crate::image::{default_threads, Image};
use crate::patterns::{self, Params, Pattern};
use std::time::Instant;

pub fn run(w: u32, h: u32, frames: u32) -> String {
    let threads = default_threads();
    let p = Params::new(w, h, Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5), 0);
    let mut out = vec![0u8; w as usize * h as usize * 4];
    let (mut render, mut convert) = (0.0f64, 0.0f64);
    let mut img = Image::new(1, 1, [0.0; 3]);
    for _ in 0..frames {
        let t = Instant::now();
        img = patterns::render(Pattern::Swim, &p, None, threads);
        render += t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        img.to_upload(Encoding::Srgb, false, threads, &mut out);
        convert += t.elapsed().as_secs_f64() * 1e3;
    }
    let n = f64::from(frames);
    let _ = img;
    format!(
        "swim CPU renderer, {w}x{h} per eye, {threads} threads, {frames} frames\n  shade {:.1} ms per eye\n  convert to bytes {:.1} ms per eye\n  per frame (2 eyes): {:.1} ms (a 72 Hz frame is 13.9 ms, 90 Hz 11.1 ms)\n",
        render / n,
        convert / n,
        2.0 * (render + convert) / n
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn bench_reports() {
        let s = super::run(64, 64, 1);
        assert!(s.contains("per frame (2 eyes)"), "{s}");
    }
}
