//! C ABI of [`crate::ae`] (declared in `include/pimax_cam.h`, section "Auto-exposure").
//!
//! The driver calls [`pimax_cam_ae_update`] after every tracking frame; when it returns 1 the driver may send
//! `payload` as HID command `0x51` option 0 to the headset. Whether it really sends is the driver's decision
//! (`PIMAX_CAM_AE=1`; `dry` = compute and log only). Nothing here touches the device.

use crate::ae::{payload, AeParams, AeSetting, AutoExposure, Mask, TileView, AE_PAYLOAD_LEN};
use crate::camera::{FLAG_META_VALID, FLAG_REPLAY};
use crate::capi::{guard, PimaxCam};
use crate::frame::FrameKind;
use std::ffi::c_int;

/// Opaque controller handle.
pub struct CAe {
    ae: AutoExposure,
    mask: Mask,
}

/// `pimax_cam_ae_out`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CAeOut {
    /// 1 = send `payload` now (cmd 0x51, opt 0).
    pub send: u32,
    /// 1 = the target changed in this update.
    pub changed: u32,
    /// Counters: frames analysed, sends handed out, target changes.
    pub frames: u64,
    /// Sends.
    pub sends: u64,
    /// Changes.
    pub changes: u64,
    /// Command payload (after `58 19 51 00`).
    pub payload: [u8; AE_PAYLOAD_LEN],
    /// Padding, zero.
    pub pad: [u8; 2],
    /// Target exposure of cameras 0..3.
    pub exposure: [u16; 4],
    /// Target gain of cameras 0..3.
    pub gain: [u16; 4],
    /// LED pair (unchanged).
    pub led_exposure: u16,
    /// LED gain.
    pub led_gain: u16,
    /// Exposure/gain the analysed frame was taken with (metadata), cameras 0..3.
    pub frame_exposure: [u16; 4],
    /// Frame gain.
    pub frame_gain: [u16; 4],
    /// Trimmed mean of the lower and the upper pair (0..255).
    pub mean: [f32; 2],
    /// Saturated fraction per pair.
    pub sat: [f32; 2],
    /// Log error per pair (0 inside the deadband).
    pub err: [f32; 2],
    /// Reserved, zero.
    pub reserved: u32,
}

/// Create a controller with the defaults and the `PIMAX_CAM_AE_*` environment overrides.
#[no_mangle]
pub extern "C" fn pimax_cam_ae_create() -> *mut CAe {
    guard(std::ptr::null_mut(), || {
        Ok(Box::into_raw(Box::new(CAe {
            ae: AutoExposure::new(AeParams::from_env()),
            mask: Mask::default(),
        })))
    })
}

/// Destroy (NULL ok).
///
/// # Safety
/// `ae` from `pimax_cam_ae_create`, not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_ae_destroy(ae: *mut CAe) {
    if !ae.is_null() {
        // SAFETY: created by Box::into_raw.
        drop(unsafe { Box::from_raw(ae) });
    }
}

/// Analyse the current frame of `cam` (call right after `pimax_cam_next` returned a frame). Returns 1 = send
/// `out->payload` now, 0 = nothing to send (LED frame, replay, no metadata, or not due), -1 = error.
///
/// # Safety
/// `ae` and `cam` valid; `out` writable.
#[no_mangle]
pub unsafe extern "C" fn pimax_cam_ae_update(
    ae: *mut CAe,
    cam: *const PimaxCam,
    now_ns: u64,
    out: *mut CAeOut,
) -> c_int {
    guard(-1, || {
        // SAFETY: per contract.
        let a = unsafe { ae.as_mut() }.ok_or("null controller")?;
        // SAFETY: per contract.
        let c = unsafe { cam.as_ref() }.ok_or("null camera")?;
        if out.is_null() {
            return Err("null output".into());
        }
        let fi = *c.cam.info();
        // Only live tracking frames with metadata drive the loop (a replay cannot be controlled).
        if fi.kind != FrameKind::Tracking as u32
            || fi.flags & FLAG_META_VALID == 0
            || fi.flags & FLAG_REPLAY != 0
        {
            return Ok(0);
        }
        let mut views: Vec<TileView> = Vec::with_capacity(4);
        for t in 0..4 {
            views.push(c.cam.tile_view(t).map_err(|e| e.to_string())?);
        }
        let tiles: [TileView; 4] = [views[0], views[1], views[2], views[3]];
        let setting = AeSetting::from_info(&fi, a.ae.p.led_exposure, a.ae.p.led_gain);
        let stats = a.ae.frame_stats(&tiles, &a.mask);
        let d = a.ae.update(&setting, stats, now_ns);
        let tgt = a.ae.target().unwrap_or(setting);
        let o = CAeOut {
            send: d.send as u32,
            changed: d.changed as u32,
            frames: a.ae.frames,
            sends: a.ae.sends,
            changes: a.ae.changes,
            payload: payload(&tgt),
            pad: [0; 2],
            exposure: tgt.exposure,
            gain: tgt.gain,
            led_exposure: tgt.led_exposure,
            led_gain: tgt.led_gain,
            frame_exposure: fi.exposure,
            frame_gain: fi.gain,
            mean: [
                d.stats[0].trimmed_mean as f32,
                d.stats[1].trimmed_mean as f32,
            ],
            sat: [d.stats[0].sat_frac as f32, d.stats[1].sat_frac as f32],
            err: [d.err[0] as f32, d.err[1] as f32],
            reserved: 0,
        };
        // SAFETY: writable per contract.
        unsafe { *out = o };
        Ok(d.send as c_int)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_update_destroy_with_null_camera() {
        let a = pimax_cam_ae_create();
        assert!(!a.is_null());
        let mut o = CAeOut::default();
        // SAFETY: valid controller; a null camera is reported as an error.
        assert_eq!(
            unsafe { pimax_cam_ae_update(a, std::ptr::null(), 0, &mut o) },
            -1
        );
        // SAFETY: created above.
        unsafe { pimax_cam_ae_destroy(a) };
    }

    #[test]
    fn out_layout_is_stable() {
        // The C header mirrors this layout.
        assert_eq!(
            std::mem::size_of::<CAeOut>(),
            8 + 24 + 24 + 8 + 8 + 4 + 8 + 8 + 8 + 8 + 8 + 4
        );
        assert_eq!(std::mem::offset_of!(CAeOut, payload), 32);
        assert_eq!(std::mem::offset_of!(CAeOut, mean), 92);
    }
}
