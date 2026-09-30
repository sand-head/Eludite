#!/usr/bin/env python3
"""Run the brief 0001 measurements and write one JSON file of raw results.

Usage: bench_all.py --label NAME [--bin PATH] [--refresh-hz HZ] [--quick]

Runs, in order:
  start   x20  cold start (process launch -> first frame presented); first run reported separately
  scroll  x3   --bench-scroll at 40 lines/frame (100k lines top to bottom)
  scroll  x1   --bench-scroll at 8 lines/frame
  keys    x3   --bench-keys 500 (synthetic keystrokes at line 50,000)
Any extra environment (WAYLAND_DISPLAY, DISPLAY, ZED_DEVICE_ID) is inherited.
"""
import argparse, json, os, statistics, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_BIN = os.environ.get("SPIKE_BIN") or os.path.join(
    os.environ.get("CARGO_TARGET_DIR", os.path.join(HERE, "..", "target")), "release", "spike-gpui-shell")


def run(binary, args, timeout, env=None):
    e = dict(os.environ)
    if env:
        e.update(env)
    p = subprocess.run([binary] + args, capture_output=True, text=True, timeout=timeout, env=e)
    lines = [l for l in p.stdout.splitlines() if l.startswith("{")]
    if p.returncode != 0 or not lines:
        raise RuntimeError(f"{args} exited {p.returncode}: {p.stderr[-2000:]}")
    return json.loads(lines[-1]), p.stderr


def summary(xs):
    xs = sorted(xs)
    return {"n": len(xs), "median": round(statistics.median(xs), 3), "min": round(xs[0], 3), "max": round(xs[-1], 3)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--refresh-hz", type=float)
    ap.add_argument("--out", default=os.path.join(HERE, "..", "results"))
    ap.add_argument("--quick", action="store_true")
    a = ap.parse_args()
    hz = ["--refresh-hz", str(a.refresh_hz)] if a.refresh_hz else []
    res = {"label": a.label, "time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
           "env": {k: os.environ.get(k, "") for k in ("WAYLAND_DISPLAY", "DISPLAY", "XDG_SESSION_TYPE", "ZED_DEVICE_ID")}}

    starts, adapter = [], None
    for i in range(3 if a.quick else 20):
        launch = time.time_ns()
        out, err = run(a.bin, ["--bench-start"], 60, {"SPIKE_LAUNCH_WALL_NS": str(launch)})
        for line in err.splitlines():
            if "Selected GPU adapter" in line:
                adapter = line.split("] ", 1)[-1]
        starts.append(out)
        time.sleep(0.3)
    res["adapter"] = adapter
    res["start_runs"] = starts
    warm = starts[1:]
    res["start_summary"] = {
        "first_run_launch_to_first_present_ms": starts[0]["launch_to_first_present_ms"],
        "warm_launch_to_first_present_ms": summary([s["launch_to_first_present_ms"] for s in warm]),
        "warm_main_to_first_present_ms": summary([s["main_to_first_present_ms"] for s in warm]),
        "warm_rss_at_first_present_mib": summary([s["rss"]["rss_mib"] for s in warm if s["rss"]]),
    }
    print(json.dumps(res["start_summary"]), file=sys.stderr)

    res["scroll_runs"] = []
    for lpf, n in ((40, 1 if a.quick else 3), (8, 0 if a.quick else 1)):
        for _ in range(n):
            out, _ = run(a.bin, ["--bench-scroll", "--scroll-lines-per-frame", str(lpf)] + hz, 600)
            res["scroll_runs"].append(out)
            print(json.dumps({k: out[k] for k in ("lines_per_frame", "refresh_hz", "frames", "frame_interval", "frame_work", "intervals_over_2x_refresh", "intervals_over_1_5x_refresh")}), file=sys.stderr)

    res["key_runs"] = []
    for _ in range(1 if a.quick else 3):
        out, _ = run(a.bin, ["--bench-keys", "100" if a.quick else "500"], 600)
        res["key_runs"].append(out)
        print(json.dumps({"key_to_present": out["key_to_present"], "render_to_present": out["render_to_present"]}), file=sys.stderr)

    os.makedirs(a.out, exist_ok=True)
    path = os.path.join(a.out, f"{a.label}.json")
    with open(path, "w") as f:
        json.dump(res, f, indent=1)
    print(path)


if __name__ == "__main__":
    main()
