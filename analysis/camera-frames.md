# Pimax Dream Air - camera (34A4:0014) UVC frames, full-packet capture
Capture: `captures2/20261013-130650-full-packets-hub2.pcap` (3.0 s). Camera = dev 29, EP 0x81 iso IN. Tools: `tools/uvc_iso.py`, `tools/uvc_frames.py`.

## IMPORTANT capture limitation
snaplen 65535 only helps ~3%: each iso URB is ~1.6-2.1 MB (128 packets) but only the first 63 960 B of its data are stored (1575 B descriptor table + ~4 packets). So **complete frames cannot be reassembled**. Recoverable: (a) all 128 packet descriptors (offset/len/status) of every URB -> exact frame sizes/rates; (b) the UVC headers of the first ~4 packets of each URB; (c) a ~62 KB pixel window per URB at a known frame offset (windows at frame offsets ~0, ~0.57 MB, ~1.1 MB, ~1.6 MB, drifting a few rows per frame; ~59 % of byte offsets covered when merged across frames). A real recapture needs snaplen >= 2.2 MB (106 MB/s, so ~1 s only).

## UVC payload header (12 bytes, one per iso packet; every packet = header + payload)
`0c | info | PTS u32 LE | STC u32 LE | SOF u16 LE`
* bLength 0x0c. bmHeaderInfo = **0x8d / 0x8c** = EOH|SCR|PTS|FID; FID toggles each frame. EOF bit not seen in data packets; the end of a frame is a header-only **12-byte** packet (151 seen, not inside the captured windows, so its info byte is unverified).
* PTS: constant over a frame; advances **1 998 812.95 ticks per frame** (1998812..1998815) -> **100 MHz** clock, period 19 988.13 us, **50.03 fps** (150 complete frames in 3.0 s). u32 wraps every 42.9 s.
* STC: same 100 MHz clock, +12 500..12 593 ticks per iso packet (one per 125 us microframe); SOF u16 = USB SOF counter. STC-PTS is ~25 k ticks (250 us) for the first packet of a frame.
* 130-131 packets per frame (up to ~17.2 KB each), one 12-byte EOF packet per frame, 0-length idle packets fill the URBs.

## Frame size / format
* **Payload per frame = 2 129 920 B exactly** (150/150 complete frames) = 1280x832x2 = the advertised "YUY2 1280x832". Content is NOT YUY2 (no chroma: even/odd byte means 8.25/8.30, dark pixels ~4-7). It is **8-bit grey, 2560 px wide x 832 rows = 4 cameras side by side**.
* Verified seams at x = 640, 1280, 1920 -> **4 tiles of 640 px**. Image rows occupy y = 24..489 (466 rows); rows 8..23 and 490..831 are all-zero. Rows 0-7 hold metadata (below). (Only 466 non-zero rows rather than 480 - OV7251 crop/readout detail unexplained.)
* Tile order vs calibration XML (trackingA, trackingB, Pi-controllerA, Pi-controllerB = id 0..3) assumed left to right, unverified. Scenes: tile0 blanket close-up, tile1 wall/objects, tile2 shelf, tile3 hand: four different directions.
* Images in `analysis/frames/`: `mosaic_2560x832.pgm/.png` (all windows merged, gaps black), `band_*_x2.5.png` (brightened bands), `cam0..3_rows222-339.pgm` / `_x2.5.png` (640x118 per-camera crops), `band_222_339_tiles_stacked.png`. `mosaic_w1280.png` and `t_raw2560_0.png` are trial renders (mosaic_w1280 is the wrong interpretation); `mosaic_w2560.png` is the same as mosaic_2560x832 (older name).

## Embedded metadata (rows 0-7, same bytes in all 8 rows, cols 0-95)
| byte offset | meaning | conf |
|---|---|---|
| 0..11 | 0/1 byte pattern, changes every frame (counter bits?) | low |
| 32 | const 0x86 | - |
| 33..36 | **u32 LE timestamp in us** (MCU uptime clock; +19 988 us per frame) | high |
| 41..42 | **u16 LE frame counter**, +1 per frame | high |
| 45..60 | 8x u16 LE: 0x0c47 0x0c47 0x0857 0x0857 | 0x000d x4 | med: per-camera exposure[4] and gain[4]; equal to the host command on headset EP 0x01 `58 19 51 00 01 32 47 0c 47 0c 57 08 57 08 0d 00 0d 00 0d 00 0d 00 28 00 3e 00` (12.5 Hz auto-exposure update) |
| 61..68 | 0x5a 0x5a 0x40 0x40 (u16) - the command has 0x28, 0x3e in that slot | low |
| 88..91 | 3c 3d 3e 3f const | - |
Everything else is zero; only timestamp, counter and the first 12 bytes change in this capture.

## Clock relations
* Metadata us timestamp vs IMU u32 us (dev 26 EP 0x83, 500 Hz): **same MCU clock**. frame_ts - IMU(interpolated at host arrival) = -11.8 ms mean, std 0.7 ms = USB readout/transfer latency. Frame ts is therefore directly comparable to IMU timestamps (calibration `delta` 1.638 ms may be the residual camera-IMU offset).
* UVC PTS = 100 x metadata_us + ~245 003 3xx (mod 2^32), clock fit 100.0006 MHz, residual < 2 ticks vs metadata; offset creeps ~50 ticks per 4 frames. Prefer the metadata timestamp (no wrap issue); PTS usable as 32-bit-wrapped alternative.
* Frame counter consecutive, no dropped frames in 3 s. URBs (62.5/s) and frames (50/s) are not aligned.
