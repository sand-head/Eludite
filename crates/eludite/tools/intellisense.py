#!/usr/bin/env python3
"""Brief 0013 manual run: drive `eludite --solution S --open-file F --bounds-out B` on X11 (ELUDITE_TRACE_LSP=1,
stderr in --log) with real XTest key and pointer events, and take the IntelliSense screenshots.

  --mode edit   in HostRpcTarget.cs: a new line after `var sdks = await _sdkDiscoverer...`, type `_sdkDiscoverer.`
                (completion list from Roslyn, screenshot completion.png), then `string.Join(",", ` (Parameter Info
                on the second parameter, screenshot signature-help.png)
  --mode hover  in HostServer.cs: find `JsonRpc CreateConnection`, put the caret on `JsonRpc`, rest the pointer
                there (Quick Info after the hover delay, screenshot quick-info.png)

Prints one JSON object with what the trace log showed. The file is never saved.
Usage: intellisense.py --mode edit|hover --title "Eludite - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, json, os, re, subprocess, sys, time

from Xlib import X, XK, display
from Xlib.ext import xtest

d = display.Display()
root = d.screen().root
LINE = re.compile(r"^\[lsp\] (\d+) (.*)$")
SHIFTED = {'_': 'underscore', '"': 'quotedbl', '(': 'parenleft', ')': 'parenright', ':': 'colon', '<': 'less',
           '>': 'greater', '?': 'question'}
PLAIN = {'.': 'period', ',': 'comma', ' ': 'space', ';': 'semicolon', '/': 'slash', '-': 'minus', '=': 'equal'}


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


def keysym(name):
    return d.keysym_to_keycode(XK.string_to_keysym(name))


def press(name, mods=()):
    codes = [keysym(m) for m in mods]
    for c in codes:
        xtest.fake_input(d, X.KeyPress, c)
    k = keysym(name)
    xtest.fake_input(d, X.KeyPress, k)
    xtest.fake_input(d, X.KeyRelease, k)
    for c in reversed(codes):
        xtest.fake_input(d, X.KeyRelease, c)
    d.sync()
    time.sleep(0.03)


def type_text(text, delay=0.06):
    for ch in text:
        if ch in SHIFTED:
            press(SHIFTED[ch], ["Shift_L"])
        elif ch in PLAIN:
            press(PLAIN[ch])
        elif ch.isupper():
            press(ch.lower(), ["Shift_L"])
        else:
            press(ch)
        time.sleep(delay)


def shot(dir_, name):
    env = dict(os.environ)
    if os.environ.get("SHOT_WAYLAND_DISPLAY"):
        env["WAYLAND_DISPLAY"] = os.environ["SHOT_WAYLAND_DISPLAY"]
    path = os.path.join(dir_, name + ".png")
    subprocess.run(["spectacle", "-b", "-n", "-f", "-o", path], env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    return path


def bounds(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def wait_loaded(a, out):
    t0 = int(time.time() * 1000) - 600_000
    loaded = wait_trace(a.log, lambda s: s.startswith("solution Loaded"), t0, 300)
    if not loaded:
        print(json.dumps({"error": "the solution did not load"}))
        sys.exit(1)
    out["loaded_ms"] = loaded[0]
    settled = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics"), loaded[0], 120)
    out["diagnostics_after_load"] = settled and settled[1]
    time.sleep(2)


def edit(a, out):
    press("Home", ["Control_L"])
    for _ in range(75):
        press("Down")
    press("End")
    press("Return")
    start = int(time.time() * 1000)
    type_text("_sdkDiscoverer.")
    reply = wait_trace(a.log, lambda s: s.startswith("completion reply") and "items" in s, start + 1, 30)
    time.sleep(1.5)
    out["completion"] = reply and reply[1]
    out["shot_completion"] = shot(a.shots, "completion")
    press("Escape")
    for _ in range(len("_sdkDiscoverer.")):
        press("BackSpace")
    start = int(time.time() * 1000)
    type_text('string.Join(",", ', delay=0.12)
    time.sleep(1.5)
    out["signature_trace"] = [t for _, t in trace(a.log) if "signature" in t][-3:]
    out["shot_signature"] = shot(a.shots, "signature-help")
    press("Escape")
    press("Escape")


def hover(a, out):
    press("Home", ["Control_L"])
    press("f", ["Control_L"])
    type_text("JsonRpc CreateConnection", delay=0.03)
    press("Return")
    press("Escape")
    press("Left")
    time.sleep(0.6)
    b = bounds(a.bounds)
    caret = b.get("editor-caret")
    if not caret:
        out["error"] = "no caret position in the bounds file"
        return
    w = find_window(a.title)
    t = w.translate_coords(root, 0, 0)
    ox, oy = -t.x, -t.y
    x, y = int(ox + caret[0] + 20), int(oy + caret[1] + caret[3] / 2)
    xtest.fake_input(d, X.MotionNotify, x=x - 30, y=y + 60)
    d.sync()
    time.sleep(0.3)
    start = int(time.time() * 1000)
    xtest.fake_input(d, X.MotionNotify, x=x, y=y)
    d.sync()
    time.sleep(2.0)
    out["pointer"] = [x - ox, y - oy]
    out["hover_trace"] = [t for t0, t in trace(a.log) if t0 >= start][-5:]
    out["shot_hover"] = shot(a.shots, "quick-info")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["edit", "hover"], required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    a = ap.parse_args()
    out = {"mode": a.mode}
    find_window(a.title)
    wait_loaded(a, out)
    if a.mode == "edit":
        edit(a, out)
    else:
        hover(a, out)
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
