//! Real frames (Linux capture 2026-10-13 19:59, two frames: one tracking, one LED exposure) through the
//! pipeline: metadata, frame kinds, tile rows, tile content against an independent reading of the raw bytes,
//! and a record -> replay round trip.
use pimax_cam::camera::Camera;
use pimax_cam::frame::{FrameKind, FRAME_W, TILE_H, TILE_W};
use pimax_cam::record::Reader;
use pimax_cam::source::ReplayOptions;
use std::path::PathBuf;

fn clip() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/evening2.pxcam")
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn real_frames_parse_and_split() {
    let mut raw = Reader::open(&clip()).unwrap();
    let mut cam = Camera::open(clip().to_str().unwrap(), 0, ReplayOptions::default()).unwrap();
    let expect = [
        (FrameKind::Tracking, 326_124u32, 278u16, 10410u16),
        (FrameKind::Led, 326_125, 10, 40),
    ];
    for (kind, counter, row, exposure) in expect {
        let rec = raw.next_record().unwrap().unwrap();
        let i = cam.next_frame(0).unwrap().unwrap();
        assert_eq!(i.kind, kind as u32);
        assert_eq!(i.counter, counter);
        assert_eq!(i.y_offset, [row; 4]);
        assert_eq!(i.exposure, [exposure; 4]);
        assert_eq!(
            i.dev_ts_us as u32,
            if counter == 326_124 {
                2_850_371_191
            } else {
                2_850_391_179
            }
        );
        assert_eq!(i.dev_ts_us >> 32, 1);
        // rows above the picture are zero, the picture is not
        let d = &rec.data;
        for t in 0..4 {
            let tile = cam.tile(t).unwrap();
            for y in [0usize, 1, 240, 479] {
                let src = &d[(row as usize + y) * FRAME_W + t * TILE_W..][..TILE_W];
                assert_eq!(&tile[y * TILE_W..(y + 1) * TILE_W], src, "tile {t} row {y}");
            }
            assert!(d[(row as usize - 1) * FRAME_W + t * TILE_W..][..TILE_W]
                .iter()
                .all(|&b| b == 0));
            let mean = tile.iter().map(|&b| b as f64).sum::<f64>() / (TILE_W * TILE_H) as f64;
            match kind {
                FrameKind::Tracking => {
                    assert!((8.0..45.0).contains(&mean), "tracking tile {t} mean {mean}")
                }
                _ => assert!((3.0..8.0).contains(&mean), "led tile {t} mean {mean}"),
            }
        }
    }
    assert!(cam.next_frame(0).unwrap().is_none());
    assert_eq!(cam.stats.row_mismatch, 0);
    assert_eq!(cam.stats.dropped, 0);
    assert_eq!(cam.stats.kind_repeats, 0);
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn record_replay_roundtrip_of_real_frames() {
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pimax-cam-roundtrip.pxcam");
    let mut cam = Camera::open(clip().to_str().unwrap(), 0, ReplayOptions::default()).unwrap();
    cam.start_recording(&out, None).unwrap();
    let mut a = Vec::new();
    while let Some(i) = cam.next_frame(0).unwrap() {
        a.push((i, (0..4).map(|t| cam.tile(t).unwrap()).collect::<Vec<_>>()));
    }
    let (frames, _) = cam.stop_recording().unwrap().unwrap();
    assert_eq!(frames, 2);
    let mut again = Camera::open(
        out.to_str().unwrap(),
        0,
        ReplayOptions {
            looping: true,
            realtime: false,
        },
    )
    .unwrap();
    for k in 0..6 {
        let i = again.next_frame(0).unwrap().unwrap();
        let (ref ai, ref at) = a[k % 2];
        assert_eq!(
            (i.counter, i.kind, i.y_offset, i.exposure, i.gain),
            (ai.counter, ai.kind, ai.y_offset, ai.exposure, ai.gain)
        );
        for (t, tile) in at.iter().enumerate() {
            assert_eq!(&again.tile(t).unwrap(), tile);
        }
    }
    // looping restarts the counter: no false drops are reported
    assert_eq!(again.stats.dropped, 0);
    std::fs::remove_file(&out).unwrap();
}
