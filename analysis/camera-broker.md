# Camera frame broker (`pimax-cam broker`)

One process streams the Dream Air cameras (V4L2 allows one streaming reader). Every other process reads the same
frames from shared memory. This lets passthrough, 6DoF head tracking (Monado), controller tracking (LED frames) and
hand tracking run at the same time. The code and the protocol are in `rust/pimax-cam/src/broker.rs` (commit d2bdb13).

## Use

```
pimax-cam broker                         # live camera (headset lock rules as for any camera use)
pimax-cam broker --source X.pxcam --loop # a recording, real-time paced
# clients: the source spec `broker` (default socket) or `broker:/path/to.sock`
pimax-cam stats --source broker --seconds 10
pimax-passthrough run --source broker
PIMAX_CAM_SOURCE=broker  (Monado driver camera code, via pimax_cam::source::open)
```

Status every 5 s: frames/s, clients, notices dropped. Stop it with Ctrl-C or SIGTERM. The socket file is removed at
exit. A second broker on the same socket is refused, and a stale socket file is replaced.

## How it works

* Ring: a memfd with a 4 KiB header page plus N slots (default 8 = 160 ms at 50 fps). Each slot holds a 64 B header
  (seqlock, host ns, driver sequence, length, stride, frame index) and the full 2560x832 raw frame. Writer: lock = 2k+1,
  data, lock = 2k+2. Reader: check 2k+2, copy, re-check.
* Socket `$XDG_RUNTIME_DIR/pimax-cam-broker.sock` (`SOCK_SEQPACKET`). On connect the server sends a hello datagram
  ("PXCB", version, layout, replay flag, source description) with the memfd attached (`SCM_RIGHTS`). After that it
  sends one 8-byte datagram (frame index) per frame.
* Clients get every frame in order while they keep up. A client that falls more than the ring behind jumps to the
  newest frame and counts the skip (`BrokerSource::skipped`). Sends are non-blocking, so no client can slow the camera
  or the other clients.
* Frames are passed through unchanged: tracking and LED frames both, with metadata, `host_ts_ns` (the server's
  CLOCK_MONOTONIC, same machine) and the driver sequence. Consumers filter by kind as before.
* Cost: one copy into the ring plus one copy per client, about 2 MB each at 50 fps.

## For the 6DoF tracking work (branch `slam`, not touched)

Start the broker before Monado and run Monado with `PIMAX_CAM_SOURCE=broker` instead of letting the driver open
`/dev/video0`. Passthrough then runs alongside with `--source broker`. Controller tracking would read the LED frames
from the same broker. Things not done yet: a desk-launcher switch that starts the broker first, and a live-camera run
(only replays tested).
