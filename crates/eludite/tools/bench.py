#!/usr/bin/env python3
"""Brief 0008 measurements: cold start and drag frame cost, release build.

Usage: bench.py --label NAME --out FILE [--bin PATH] [--starts 20] [--drags 3] [--frames 600]

Runs `eludite --bench-start` STARTS times (0.3 s apart; the first run is
reported separately) and `eludite --bench-drag FRAMES` DRAGS times, and writes
every raw JSON line plus a summary to FILE. Run it inside the display you want
to measure (tools/manual-linux.sh shows how to start a nested KWin).
"""
import argparse, json, os, statistics, subprocess, time

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(HERE, "..", "..", "..", "target")),
                           "release", "eludite")


def run(binary, args, env=None):
    e = dict(os.environ)
    e.update(env or {})
    load = os.getloadavg()[0]
    p = subprocess.run([binary] + args, capture_output=True, text=True, timeout=120, env=e)
    lines = [l for l in p.stdout.splitlines() if l.startswith("{")]
    if p.returncode != 0 or not lines:
        raise RuntimeError(f"{args} exited {p.returncode}: {p.stderr[-2000:]}")
    out = json.loads(lines[-1])
    out["loadavg_1min_before"] = round(load, 2)
    return out


def summary(xs):
    xs = sorted(xs)
    return {"n": len(xs), "median": round(statistics.median(xs), 3), "min": round(xs[0], 3), "max": round(xs[-1], 3)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--starts", type=int, default=20)
    ap.add_argument("--drags", type=int, default=3)
    ap.add_argument("--frames", type=int, default=600)
    a = ap.parse_args()
    res = {"label": a.label, "time": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
           "env": {k: os.environ.get(k, "") for k in ("WAYLAND_DISPLAY", "DISPLAY", "XDG_SESSION_TYPE")}}
    starts = []
    for _ in range(a.starts):
        starts.append(run(a.bin, ["--bench-start"], {"ELUDITE_LAUNCH_WALL_NS": str(time.time_ns())}))
        time.sleep(0.3)
    res["start_runs"] = starts
    warm = starts[1:] or starts
    res["start_summary"] = {
        "first_run_launch_to_first_present_ms": starts[0]["launch_to_first_present_ms"],
        "warm_launch_to_first_present_ms": summary([s["launch_to_first_present_ms"] for s in warm]),
        "warm_main_to_first_present_ms": summary([s["main_to_first_present_ms"] for s in warm]),
        "rss_mib_at_first_present": summary([s["rss"]["rss_mib"] for s in warm]),
    }
    drags = [run(a.bin, ["--bench-drag", str(a.frames)]) for _ in range(a.drags)]
    res["drag_runs"] = drags
    res["drag_summary"] = {
        "frame_cost_p99_ms": [d["frame_cost"]["p99_ms"] for d in drags],
        "frame_cost_p50_ms": [d["frame_cost"]["p50_ms"] for d in drags],
        "frame_cost_max_ms": [d["frame_cost"]["max_ms"] for d in drags],
        "render_to_present_p99_ms": [d["render_to_present"]["p99_ms"] for d in drags],
        "frame_interval_p99_ms": [d["frame_interval"]["p99_ms"] for d in drags],
        "loadavg_1min_before": [d["loadavg_1min_before"] for d in drags],
        "guides_visible_fraction": [round(d["renders_with_guides_visible"] / max(d["renders"], 1), 4) for d in drags],
    }
    with open(a.out, "w") as f:
        json.dump(res, f, indent=1)
    print(json.dumps({"label": a.label, "start": res["start_summary"], "drag": res["drag_summary"]}))


if __name__ == "__main__":
    main()
