//! Hardware tests: need the headset plugged in and the shared lock, e.g.
//! `flock -w 3600 /tmp/omarchyvr-headset.lock cargo test -p pimax-cam --test live -- --ignored --test-threads=1 --nocapture`
//! Capture only (UVC stream); nothing is sent to the headset.
use pimax_cam::camera::Camera;
use pimax_cam::frame::FrameKind;
use pimax_cam::source::ReplayOptions;
use pimax_cam::stats::CaptureStats;

fn live() -> Camera {
    Camera::open("auto", 0, ReplayOptions::default())
        .expect("camera 34a4:0014 (plugged in? lock held?)")
}

#[test]
#[ignore]
fn live_capture_is_steady() {
    let mut cam = live();
    eprintln!("{}", cam.describe());
    let mut st = CaptureStats::default();
    let t0 = pimax_cam::clock::monotonic_ns();
    while pimax_cam::clock::monotonic_ns() - t0 < 10_000_000_000 {
        if let Some(i) = cam.next_frame(500).unwrap() {
            st.add(&i);
        }
    }
    st.finish();
    eprintln!("{}", st.report());
    assert!((49.5..50.6).contains(&st.fps()), "fps {}", st.fps());
    assert_eq!(st.dropped, 0, "lost frames");
    assert_eq!(cam.corrupt_frames(), 0);
    assert_eq!(cam.stats.kind_repeats, 0, "tracking/LED alternation broken");
    assert_eq!(cam.stats.unknown, 0);
    assert_eq!(
        cam.stats.scanned_rows + cam.stats.row_mismatch,
        0,
        "metadata row offsets did not match the picture"
    );
    // device clock: exactly one frame period between frames
    assert!(
        (st.dev_dt_ms.mean() - 19.988).abs() < 0.01 && st.dev_dt_ms.std() < 0.01,
        "{}",
        st.dev_dt_ms.fmt("ms")
    );
    // tracking frames at 25 Hz
    assert!((st.tracking_dt_ms.mean() - 39.976).abs() < 0.05);
}

#[test]
#[ignore]
fn live_record_then_replay() {
    let out = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pimax-cam-live.pxcam");
    let mut cam = live();
    cam.start_recording(&out, None).unwrap();
    let mut infos = Vec::new();
    while infos.len() < 100 {
        if let Some(i) = cam.next_frame(500).unwrap() {
            infos.push((i, cam.tile(0).unwrap()));
        }
    }
    let (frames, bytes) = cam.stop_recording().unwrap().unwrap();
    eprintln!(
        "recorded {frames} frames {bytes} B, recorder drops {}",
        cam.stats.record_dropped
    );
    assert_eq!(frames, 100);
    drop(cam);
    let mut rp = Camera::open(out.to_str().unwrap(), 0, ReplayOptions::default()).unwrap();
    for (i, tile0) in &infos {
        let r = rp.next_frame(0).unwrap().unwrap();
        assert_eq!(
            (r.counter, r.dev_ts_us, r.kind, r.y_offset),
            (i.counter, i.dev_ts_us, i.kind, i.y_offset)
        );
        assert_eq!(&rp.tile(0).unwrap(), tile0);
    }
    assert!(rp.next_frame(0).unwrap().is_none());
    assert!(infos
        .iter()
        .any(|(i, _)| i.kind == FrameKind::Tracking as u32));
    std::fs::remove_file(&out).unwrap();
}
