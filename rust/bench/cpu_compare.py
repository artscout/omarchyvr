#!/usr/bin/env python3
"""CPU / memory / wakeups of `omarchyvr keepalived` (Rust) versus the python daemon over the same wall-clock window.

    cargo build --release && python3 rust/bench/cpu_compare.py [seconds=60]

* "python service": the already running omarchyvr-keepalived.service (MainPID), measured passively - nothing is changed.
* "rust": a second daemon `omarchyvr keepalived --no-wake` started for the window (extra keepalives with the same rate byte
  as the service sends; no wake packet, no video command).
* "python copy" (--python-copy): an extra instance of pimax_keepalived.py with the wake replaced by a plain keepalive,
  for an apples-to-apples pair when the service is not wanted as reference.
CPU time comes from /proc/<pid>/task/*/schedstat (ns resolution, all threads).
"""
import os, signal, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(ROOT, "rust/target/release/omarchyvr")
SECS = float(next((a for a in sys.argv[1:] if not a.startswith("--")), 60))


def cpu_ns(pid):
    tot = 0
    for t in os.listdir(f"/proc/{pid}/task"):
        try:
            tot += int(open(f"/proc/{pid}/task/{t}/schedstat").read().split()[0])
        except OSError:
            pass
    return tot


def status(pid):
    d = {}
    for line in open(f"/proc/{pid}/status"):
        k, _, v = line.partition(":")
        d[k] = v.strip()
    return d


def snap(pid):
    s = status(pid)
    return dict(cpu=cpu_ns(pid), vol=int(s["voluntary_ctxt_switches"]), inv=int(s["nonvoluntary_ctxt_switches"]),
                rss=int(s["VmRSS"].split()[0]), threads=int(s["Threads"]))


def task_ctxt(pid):
    v = i = 0
    for t in os.listdir(f"/proc/{pid}/task"):
        s = status(f"{pid}/task/{t}")
        v += int(s["voluntary_ctxt_switches"]); i += int(s["nonvoluntary_ctxt_switches"])
    return v, i


def measure(procs, secs):
    a = {n: snap(p) for n, p in procs.items()}
    c0 = {n: task_ctxt(p) for n, p in procs.items()}
    t0 = time.time()
    time.sleep(secs)
    el = time.time() - t0
    out = {}
    for n, p in procs.items():
        b = snap(p); c1 = task_ctxt(p)
        out[n] = dict(cpu_ms=(b["cpu"] - a[n]["cpu"]) / 1e6, cpu_pct=(b["cpu"] - a[n]["cpu"]) / 1e9 / el * 100,
                      wake_per_s=(c1[0] + c1[1] - c0[n][0] - c0[n][1]) / el, rss_kb=b["rss"], threads=b["threads"])
    return el, out


def main():
    procs, children = {}, []
    svc = subprocess.run(["systemctl", "--user", "show", "-p", "MainPID", "--value", "omarchyvr-keepalived"], capture_output=True, text=True).stdout.strip()
    if svc and svc != "0":
        procs["python service (pimax_keepalived.py)"] = int(svc)
    if "--python-copy" in sys.argv:
        code = ("import sys; sys.path.insert(0, %r); import pimax_keepalived as k; k.WAKE = k.cmd(0x58,0x04,0x39,0x00,0x00,0x00); k.main()"
                % os.path.join(ROOT, "linux"))
        p = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.DEVNULL); children.append(p)
        procs["python copy (no wake)"] = p.pid
    r = subprocess.Popen([BIN, "keepalived", "--no-wake"], stdout=subprocess.DEVNULL); children.append(r)
    procs["rust  omarchyvr keepalived"] = r.pid
    time.sleep(2)  # settle
    try:
        el, res = measure(procs, SECS)
    finally:
        for c in children:
            c.send_signal(signal.SIGTERM)
        for c in children:
            c.wait(timeout=5)
    print(f"window {el:.1f} s, headset connected, same time interval for every process")
    print(f"{'process':42} {'cpu ms':>9} {'cpu %':>8} {'wakeups/s':>10} {'RSS kB':>8} {'threads':>8}")
    for n, v in res.items():
        print(f"{n:42} {v['cpu_ms']:9.1f} {v['cpu_pct']:8.3f} {v['wake_per_s']:10.1f} {v['rss_kb']:8d} {v['threads']:8d}")
    ks = [v for n, v in res.items() if n.startswith("python")]
    rr = res["rust  omarchyvr keepalived"]
    if ks:
        print(f"rust uses {rr['cpu_ms'] / ks[0]['cpu_ms'] * 100:.0f} % of the python CPU time, {rr['rss_kb'] / ks[0]['rss_kb'] * 100:.0f} % of its RSS")


if __name__ == "__main__":
    main()
