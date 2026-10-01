#!/usr/bin/env python3
"""Brief 0012 manual run: with `eludite --solution S --open-file F` running on X11 (ELUDITE_TRACE_LSP=1, stderr in
--log), wait for the solution to load and the file's diagnostics, type an error at the caret with real XTest key
events, wait for the error's diagnostics, take a screenshot, delete it, wait for them to clear, take another.
Prints one JSON object with the latencies (key press to the diagnostics that show or clear the error).

Usage: edit_error.py --title "Eludite - Eludite" --log run.err --shots DIR [--text x]
"""
import argparse, json, os, re, subprocess, sys, time

from Xlib import X, XK, display
from Xlib.ext import xtest

d = display.Display()
root = d.screen().root
LINE = re.compile(r"^\[lsp\] (\d+) (.*)$")


def find_window(title, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        stack = [root]
        while stack:
            w = stack.pop()
            try:
                if w.get_wm_name() == title and w.get_attributes().map_state == X.IsViewable:
                    return w
                stack.extend(w.query_tree().children)
            except Exception:
                pass
        time.sleep(0.2)
    raise RuntimeError(f"no window titled {title!r}")


def trace(log):
    try:
        with open(log, errors="replace") as f:
            for line in f:
                m = LINE.match(line.strip())
                if m:
                    yield int(m.group(1)), m.group(2)
    except OSError:
        return


def wait_trace(log, pred, after_ms, timeout):
    end = time.time() + timeout
    while time.time() < end:
        for t, text in trace(log):
            if t >= after_ms and pred(text):
                return t, text
        time.sleep(0.02)
    return None


def key(name):
    code = d.keysym_to_keycode(XK.string_to_keysym(name))
    xtest.fake_input(d, X.KeyPress, code)
    xtest.fake_input(d, X.KeyRelease, code)
    d.sync()


def shot(dir_, name):
    env = dict(os.environ)
    if os.environ.get("SHOT_WAYLAND_DISPLAY"):
        env["WAYLAND_DISPLAY"] = os.environ["SHOT_WAYLAND_DISPLAY"]
    path = os.path.join(dir_, name + ".png")
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", path], env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    return path


def errors(text):
    m = re.search(r"\((\d+) errors\)", text)
    return int(m.group(1)) if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--shots", required=True)
    ap.add_argument("--text", default="x")
    a = ap.parse_args()
    out = {}
    find_window(a.title)
    t0 = int(time.time() * 1000) - 600_000
    loaded = wait_trace(a.log, lambda s: s.startswith("solution Loaded"), t0, 300)
    if not loaded:
        print(json.dumps({"error": "the solution did not load"}))
        sys.exit(1)
    out["loaded_ms"] = loaded[0]
    # The host re-pulls diagnostics for open documents once the solution has loaded.
    settled = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics"), loaded[0], 120)
    out["diagnostics_after_load"] = settled and settled[1]
    time.sleep(2)
    base = errors(settled[1]) if settled else 0
    key_ms = int(time.time() * 1000)
    for ch in a.text:
        key(ch)
    out["typed"] = a.text
    shown = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and (errors(s) or 0) > (base or 0),
                       key_ms, 60)
    out["key_to_error_diagnostics_ms"] = shown and shown[0] - key_ms
    out["error_diagnostics"] = shown and shown[1]
    time.sleep(1.0)
    out["shot_error"] = shot(a.shots, "error-squiggle-and-error-list")
    fix_ms = int(time.time() * 1000)
    for _ in a.text:
        key("BackSpace")
    cleared = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and (errors(s) or 0) <= (base or 0),
                         fix_ms, 60)
    out["key_to_cleared_diagnostics_ms"] = cleared and cleared[0] - fix_ms
    out["cleared_diagnostics"] = cleared and cleared[1]
    time.sleep(1.0)
    out["shot_fixed"] = shot(a.shots, "error-fixed")
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
