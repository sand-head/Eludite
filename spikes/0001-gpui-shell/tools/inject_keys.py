#!/usr/bin/env python3
"""Real keystroke-to-present latency on Linux through the kernel input path.

Creates a uinput keyboard, launches `spike-gpui-shell --key-probe`, and types
home-row letters into it one at a time. For each key it records the wall time
just before the key-press event is written to /dev/uinput and reads back the
app's wall time after the frame showing the edit was presented. The path
covered is kernel -> libinput -> compositor -> Wayland/X11 client -> GPUI ->
present; it still excludes the compositor's own scan-out.

Safety: refuses to run when the session is locked (keys would go to the lock
screen); only types after the app reports its window is focused; the app
exits on focus loss and the injector stops at the first missing sample, so
at most one letter can reach another window. Never sends Enter or modifiers.

Usage: inject_keys.py [--bin PATH] [--count N] [--activate-kwin]
Needs read/write access to /dev/uinput.
"""
import argparse, fcntl, json, os, queue, random, statistics, struct, subprocess, sys, threading, time

UI_SET_EVBIT, UI_SET_KEYBIT = 0x40045564, 0x40045565
UI_DEV_SETUP, UI_DEV_CREATE, UI_DEV_DESTROY = 0x405C5503, 0x5501, 0x5502
EV_SYN, EV_KEY = 0, 1
KEYS = [30, 31, 32, 33, 36, 37, 38]  # a s d f j k l
HERE = os.path.dirname(os.path.abspath(__file__))


def session_locked():
    sid = os.environ.get("XDG_SESSION_ID")
    if not sid:
        out = subprocess.run(["loginctl", "list-sessions", "--no-legend"], capture_output=True, text=True).stdout
        sids = [l.split()[0] for l in out.splitlines() if "seat0" in l]
        sid = sids[0] if sids else None
    if not sid:
        return True
    out = subprocess.run(["loginctl", "show-session", sid, "-p", "LockedHint"], capture_output=True, text=True).stdout
    return "yes" in out


def ev(fd, type_, code, value):
    os.write(fd, struct.pack("llHHi", 0, 0, type_, code, value))


def activate_kwin():
    js = os.path.join(HERE, "kwin_activate.js")
    sid = subprocess.run(["qdbus6", "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.loadScript", js, "spikeact"],
                         capture_output=True, text=True).stdout.strip()
    subprocess.run(["qdbus6", "org.kde.KWin", f"/Scripting/Script{sid}", "org.kde.kwin.Script.run"], capture_output=True)
    time.sleep(0.2)
    subprocess.run(["qdbus6", "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.unloadScript", "spikeact"], capture_output=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=os.environ.get("SPIKE_BIN") or os.path.join(
        os.environ.get("CARGO_TARGET_DIR", os.path.join(HERE, "..", "target")), "release", "spike-gpui-shell"))
    ap.add_argument("--count", type=int, default=300)
    ap.add_argument("--activate-kwin", action="store_true")
    a = ap.parse_args()
    if session_locked():
        sys.exit("session is locked: refusing to inject keys (they would go to the lock screen)")

    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    for k in KEYS:
        fcntl.ioctl(fd, UI_SET_KEYBIT, k)
    setup = struct.pack("HHHH80sI", 0x03, 0x1234, 0x5678, 1, b"niello-spike-keys", 0)
    fcntl.ioctl(fd, UI_DEV_SETUP, setup)
    fcntl.ioctl(fd, UI_DEV_CREATE)
    proc = None
    try:
        time.sleep(1.0)  # let libinput/the compositor pick up the device
        proc = subprocess.Popen([a.bin, "--key-probe"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        lines = queue.Queue()
        threading.Thread(target=lambda: [lines.put(l.strip()) for l in proc.stdout] + [lines.put(None)], daemon=True).start()
        if a.activate_kwin:
            time.sleep(0.5)
            activate_kwin()
        deadline = time.time() + 10
        ready = False
        while time.time() < deadline and not ready:
            try:
                l = lines.get(timeout=0.2)
            except queue.Empty:
                continue
            if l is None or l == "FOCUS-LOST":
                break
            ready = l == "READY"
        if not ready:
            sys.exit("app window never became active; not typing")
        time.sleep(0.5)
        inj_to_present, inj_to_handler, handler_to_present, render_to_present = [], [], [], []
        for i in range(a.count):
            time.sleep(random.uniform(0.030, 0.060))
            if session_locked():
                break
            k = KEYS[i % len(KEYS)]
            t = time.time_ns()
            ev(fd, EV_KEY, k, 1); ev(fd, EV_SYN, 0, 0)
            ev(fd, EV_KEY, k, 0); ev(fd, EV_SYN, 0, 0)
            sample = None
            end = time.time() + 1.0
            while time.time() < end:
                try:
                    l = lines.get(timeout=0.05)
                except queue.Empty:
                    continue
                if l is None or l == "FOCUS-LOST":
                    end = 0
                    break
                if l.startswith("{"):
                    sample = json.loads(l)
                    break
            if sample is None:
                print(f"no sample for key {i}; stopping", file=sys.stderr)
                break
            inj_to_present.append((sample["present_wall_ns"] - t) / 1e6)
            inj_to_handler.append((sample["handler_wall_ns"] - t) / 1e6)
            handler_to_present.append(sample["to_present_ns"] / 1e6)
            render_to_present.append(sample["render_to_present_ns"] / 1e6)

        def summ(xs):
            xs = sorted(xs)
            if not xs:
                return None
            pct = lambda p: xs[min(len(xs) - 1, max(0, int(-(-p * len(xs) // 100)) - 1))]
            return {"n": len(xs), "p50_ms": round(pct(50), 3), "p95_ms": round(pct(95), 3), "p99_ms": round(pct(99), 3),
                    "max_ms": round(xs[-1], 3), "min_ms": round(xs[0], 3), "mean_ms": round(statistics.mean(xs), 3)}
        print(json.dumps({
            "bench": "keys-uinput",
            "env": {k: os.environ.get(k, "") for k in ("WAYLAND_DISPLAY", "DISPLAY", "ZED_DEVICE_ID")},
            "inject_to_present": summ(inj_to_present),
            "inject_to_handler": summ(inj_to_handler),
            "handler_to_present": summ(handler_to_present),
            "render_to_present": summ(render_to_present),
        }))
    finally:
        if proc and proc.poll() is None:
            proc.terminate()
        fcntl.ioctl(fd, UI_DEV_DESTROY)
        os.close(fd)


if __name__ == "__main__":
    main()
