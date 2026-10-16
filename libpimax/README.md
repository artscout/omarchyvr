# libpimax

Small C11 library (libc + pthread only) for the Pimax Dream Air (34a4:0044) over Linux hidraw. Meant to be linked by a Monado driver.
Protocol references: `../analysis/init-sequence.md`, `../analysis/controllers-settings.md`.

    make            # build/libpimax.a, build/libpimax.so, build/pimax-cli

## What it does
- Discovers the hidraw node of each USB interface (0..3) through sysfs; re-discovers after the headset re-enumerates.
- Session thread: wake `58 06 1a 05 01 00 00`, keepalive `58 04 39 00 RR 00` every 213 ms, keeps IF0..IF3 open and drained (required, else the headset detaches after ~5.4 s), reconnects automatically.
- Command API with reply matching (`4c LL cmd opt ...`), callbacks for IMU, controllers, controller status (0x37) and firmware log lines.
- Needs read/write access to the hidraw nodes (see `../linux/70-pimax.rules`).

## Usage
    pimax_callbacks cb = { .user = ctx, .on_imu = my_imu };
    pimax_t *h = pimax_open(&cb);
    pimax_wait_connected(h, 5000);
    pimax_close(h);
Callbacks run on the session thread. Do not call `pimax_request` from a callback.

## pimax-cli
    pimax-cli imu [s]          IMU rate and last sample once per second
    pimax-cli log [s]          firmware log lines
    pimax-cli controllers [s]  attach burst for slots 2 and 3, decoded reports
    pimax-cli info             serial (0x16) and version (0x03)
The CLI has no video commands. `pimax_video_prepare()` exists in the library but nothing calls it.
It can run next to `pimax-keepalived` (both send keepalives).

## Known gaps
See the report that came with this commit: no tests of video path, controller battery/touch semantics unverified, no cameras, IPD set, brightness, eye tracking.
