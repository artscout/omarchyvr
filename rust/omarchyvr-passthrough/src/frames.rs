//! Camera frames for the passthrough: the four tiles of one *tracking* frame (the LED frames in between are
//! nearly black and are skipped), with the capture time.

use pimax_cam::camera::Camera;
use pimax_cam::frame::{FrameKind, TILE_COUNT, TILE_H, TILE_W};
use std::io;
use std::path::Path;

/// The four tiles of one frame, 640x480 each, as delivered (tiles 0 and 2 upside down: the rig handles that).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileSet {
    /// Raw tiles.
    pub tiles: [Vec<u8>; TILE_COUNT],
    /// Capture time on `CLOCK_MONOTONIC` (ns): the IMU-clock mapping of the frame's MCU timestamp
    /// (`pimax_cam::clock`); 0 for still images.
    pub mono_ns: u64,
    /// Frame counter from the metadata.
    pub counter: u32,
}

impl TileSet {
    /// An all-black set.
    pub fn black() -> TileSet {
        TileSet {
            tiles: std::array::from_fn(|_| vec![0; TILE_W * TILE_H]),
            mono_ns: 0,
            counter: 0,
        }
    }

    /// Borrowed view for [`crate::remap`].
    pub fn view(&self) -> crate::remap::Tiles<'_> {
        [
            &self.tiles[0],
            &self.tiles[1],
            &self.tiles[2],
            &self.tiles[3],
        ]
    }

    /// Read `PREFIX0.png` .. `PREFIX3.png` (e.g. `.../cam01_ctr49470_tile`, as written by `pimax-cam rectify`).
    pub fn from_pngs(prefix: &str) -> Result<TileSet, String> {
        let mut t = TileSet::black();
        for i in 0..TILE_COUNT {
            let p = format!("{prefix}{i}.png");
            let g = crate::pngdec::read(Path::new(&p))?;
            if (g.width, g.height) != (TILE_W, TILE_H) {
                return Err(format!(
                    "{p}: {}x{}, expected {TILE_W}x{TILE_H}",
                    g.width, g.height
                ));
            }
            t.tiles[i] = g.pixels;
        }
        Ok(t)
    }

    /// Copy the current frame of `cam` into this set (no allocation once sized).
    pub fn fill_from(&mut self, cam: &Camera) -> io::Result<()> {
        for (i, t) in self.tiles.iter_mut().enumerate() {
            t.resize(TILE_W * TILE_H, 0);
            cam.copy_tile(i, t, TILE_W)?;
        }
        let info = cam.info();
        self.mono_ns = info.mono_ts_ns;
        self.counter = info.counter;
        Ok(())
    }
}

/// Wait for the next tracking frame (skipping LED and unknown frames). `Ok(false)`: timeout or end of a replay.
pub fn next_tracking(cam: &mut Camera, timeout_ms: i32, out: &mut TileSet) -> io::Result<bool> {
    // a few LED frames in a row is normal (1 between two tracking frames); give up after many
    for _ in 0..16 {
        match cam.next_frame(timeout_ms)? {
            None => return Ok(false),
            Some(info) if info.kind == FrameKind::Tracking as u32 => {
                out.fill_from(cam)?;
                return Ok(true);
            }
            Some(_) => continue,
        }
    }
    Ok(false)
}

/// Open a camera source: empty / `auto` = live camera (`/dev/videoN` of the Dream Air), a device path, or a
/// `.pxcam` recording (replayed in real time and looped when `looping`).
pub fn open_source(spec: &str, looping: bool, realtime: bool) -> io::Result<Camera> {
    Camera::open(
        spec,
        60,
        pimax_cam::source::ReplayOptions { looping, realtime },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn replayed_fixture_yields_tracking_frames() {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../pimax-cam/tests/fixtures/evening2.pxcam");
        let mut cam = open_source(p.to_str().unwrap(), false, false).unwrap();
        let mut t = TileSet::black();
        let mut n = 0;
        while next_tracking(&mut cam, 100, &mut t).unwrap() {
            n += 1;
            assert!(t.tiles.iter().all(|x| x.len() == TILE_W * TILE_H));
            let mean = t.tiles[1].iter().map(|&v| v as u64).sum::<u64>() / t.tiles[1].len() as u64;
            assert!(mean > 5, "tracking frames are lit: {mean}");
        }
        assert!(n >= 1);
        assert!(TileSet::from_pngs("/nonexistent/x").is_err());
    }
}
