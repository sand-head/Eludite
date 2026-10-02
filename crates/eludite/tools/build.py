#!/usr/bin/env python3
"""Brief 0017 manual run: drive `eludite --solution S --open-file F --bounds-out B` on X11 (ELUDITE_TRACE_LSP=1,
stderr in --log) with real XTest key and pointer events against the real eludite-host, and take the screenshots.

  1. Wait for the solution to load. Ctrl+Shift+B builds it: the Output window comes forward and streams the log
     (build-streaming.png, while it runs); when it finishes, the status bar says so (build-succeeded.png); a click on
     the Error List tab shows it empty (build-error-list-empty.png).
  2. Ctrl+End in the open file, type `x` (an error at the end of the file), Ctrl+S, Ctrl+Shift+B. The build fails and
     the Error List comes forward with the build's row; Ctrl+Home moves the caret away; a double-click on the row
     opens the file at the error (build-error-click-through.png).

Prints one JSON object with the trace lines and the key-to-trace latencies.
Usage: build.py --title "Eludite - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, json, sys, time

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import bounds, find_window, press, shot, type_text, wait_trace  # noqa: E402
from navigation import click, since, window_origin  # noqa: E402
from Xlib import X  # noqa: E402
from Xlib.ext import xtest  # noqa: E402
from intellisense import d  # noqa: E402


def ms():
    return int(time.time() * 1000)


def rect(a, key, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        b = bounds(a.bounds).get(key)
        if b:
            return b
        time.sleep(0.2)
    return None


def double_click(title, x, y):
    ox, oy = window_origin(title)
    xtest.fake_input(d, X.MotionNotify, x=int(ox + x), y=int(oy + y))
    d.sync()
    time.sleep(0.1)
    for _ in range(2):
        xtest.fake_input(d, X.ButtonPress, 1)
        xtest.fake_input(d, X.ButtonRelease, 1)
        d.sync()
        time.sleep(0.05)
    time.sleep(0.3)


def build(a, out, key):
    start = ms()
    press("b", ["Control_L", "Shift_L"])
    first = wait_trace(a.log, lambda s: s.startswith("build first output"), start, 60)
    out[key + "_first_output_ms"] = first and first[0] - start
    return start, first


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    a = ap.parse_args()
    out = {}
    find_window(a.title)
    t0 = ms() - 600_000
    loaded = wait_trace(a.log, lambda s: s.startswith("solution Loaded"), t0, 300)
    if not loaded:
        print(json.dumps({"error": "the solution did not load"}))
        sys.exit(1)
    time.sleep(3)

    # 1. Build the solution.
    start, first = build(a, out, "build1")
    time.sleep(1.2)
    out["shot_streaming"] = shot(a.shots, "build-streaming")
    done = wait_trace(a.log, lambda s: s.startswith("build finished"), start, 600)
    out["build1_finished"] = done and done[1]
    out["build1_key_to_finished_ms"] = done and done[0] - start
    time.sleep(1.0)
    out["shot_succeeded"] = shot(a.shots, "build-succeeded")
    tab = rect(a, "tab-error_list")
    out["error_list_tab"] = tab
    if tab:
        click(a.title, tab)
        time.sleep(1.0)
        out["shot_error_list_empty"] = shot(a.shots, "build-error-list-empty")
    out["build1_trace"] = [l for l in since(a.log, start) if l.startswith("build")]

    # 2. An error at the end of the open file, saved, and a rebuild.
    caret = rect(a, "editor-caret")
    if caret:
        click(a.title, caret)
    press("End", ["Control_L"])
    type_text("x")
    press("s", ["Control_L"])
    time.sleep(0.5)
    start, first = build(a, out, "build2")
    done = wait_trace(a.log, lambda s: s.startswith("build finished"), start, 600)
    out["build2_finished"] = done and done[1]
    out["build2_key_to_finished_ms"] = done and done[0] - start
    time.sleep(1.5)
    press("Home", ["Control_L"])
    time.sleep(0.5)
    errors = rect(a, "error-list-errors")
    out["error_list_errors_button"] = errors
    if errors:
        # Row 0: under the toolbar (26 px, the button centered in it) and the column headers (20 px).
        x = errors[0] + 300
        y = errors[1] + errors[3] / 2 + 13 + 20 + 10
        double_click(a.title, x, y)
        time.sleep(1.5)
    out["build2_trace"] = [l for l in since(a.log, start) if not l.startswith("publishDiagnostics")][-12:]
    out["shot_click_through"] = shot(a.shots, "build-error-click-through")
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
