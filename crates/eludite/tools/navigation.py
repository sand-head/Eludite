#!/usr/bin/env python3
"""Brief 0014 manual run: drive `eludite --solution S --open-file F --bounds-out B` on X11 (ELUDITE_TRACE_LSP=1,
stderr in --log) with real XTest key and pointer events, and take the navigation screenshots.

  --mode definition  in HostServer.cs: the caret on `JsonRpc` of `JsonRpc CreateConnection`, F12 (metadata as source:
                     a read-only `JsonRpc [from metadata]` tab, screenshot definition-metadata.png), then Ctrl+- back
  --mode references  in HostRpcTarget.cs: the caret on `HostRpcTarget` of its declaration, Shift+F12 (the Find All
                     References window, screenshot find-all-references.png)
  --mode errors      in HostRpcTarget.cs: a new line with two unused locals (warnings, with WarningsNotAsErrors set by
                     the run script) and an unknown name (an error), then the Error List's Errors and Messages buttons
                     clicked off (warnings only, screenshots error-list-all.png and error-list-warnings.png)

Prints one JSON object with what the trace log showed. The file is never saved.
Usage: navigation.py --mode definition|references|errors --title "Eludite - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, json, sys, time

from Xlib import X
from Xlib.ext import xtest

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import (bounds, d, find_window, press, root, shot, trace, type_text, wait_loaded,  # noqa: E402
                          wait_trace)


def caret_on(text, then=()):
    """Find `text` with Ctrl+F, close the find bar with the match selected, collapse to its start, then press `then`."""
    press("Home", ["Control_L"])
    press("f", ["Control_L"])
    type_text(text, delay=0.03)
    press("Return")
    press("Escape")
    press("Left")
    for k in then:
        press(k)
    time.sleep(0.4)


def window_origin(title):
    w = find_window(title)
    t = w.translate_coords(root, 0, 0)
    return -t.x, -t.y


def click(title, rect):
    ox, oy = window_origin(title)
    x, y = int(ox + rect[0] + rect[2] / 2), int(oy + rect[1] + rect[3] / 2)
    xtest.fake_input(d, X.MotionNotify, x=x, y=y)
    d.sync()
    time.sleep(0.1)
    xtest.fake_input(d, X.ButtonPress, 1)
    xtest.fake_input(d, X.ButtonRelease, 1)
    d.sync()
    time.sleep(0.3)


def since(log, start):
    return [t for t0, t in trace(log) if t0 >= start]


def definition(a, out):
    caret_on("JsonRpc CreateConnection")
    start = int(time.time() * 1000)
    press("F12")
    reply = wait_trace(a.log, lambda s: s.startswith("definition reply"), start, 60)
    out["definition"] = reply and reply[1]
    out["definition_ms"] = reply and reply[0] - start
    time.sleep(2.0)
    out["trace"] = since(a.log, start)[-6:]
    out["shot_definition"] = shot(a.shots, "definition-metadata")
    # Back to HostServer.cs.
    start = int(time.time() * 1000)
    press("minus", ["Control_L"])
    time.sleep(1.0)
    out["back_trace"] = since(a.log, start)[-3:]
    out["shot_back"] = shot(a.shots, "definition-back")


def references(a, out):
    caret_on("sealed class HostRpcTarget", ["End", "Left"])
    start = int(time.time() * 1000)
    press("F12", ["Shift_L"])
    reply = wait_trace(a.log, lambda s: s.startswith("references reply"), start, 60)
    out["references"] = reply and reply[1]
    out["references_ms"] = reply and reply[0] - start
    time.sleep(2.0)
    out["trace"] = since(a.log, start)[-4:]
    out["shot_references"] = shot(a.shots, "find-all-references")


def errors(a, out):
    press("Home", ["Control_L"])
    for _ in range(75):
        press("Down")
    press("End")
    press("Return")
    start = int(time.time() * 1000)
    type_text("int unusedLocal = 42; int neverRead; undefinedName();", delay=0.05)
    # The host's warming pull publishes the new diagnostics.
    reply = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and "HostRpcTarget" in s
                       and "(0 errors)" not in s, start, 60)
    out["diagnostics"] = reply and reply[1]
    time.sleep(1.5)
    b = bounds(a.bounds)
    missing = [k for k in ("error-list-errors", "error-list-messages") if k not in b]
    if missing:
        out["error"] = f"no bounds for {missing}"
        return
    out["shot_before"] = shot(a.shots, "error-list-all")
    click(a.title, b["error-list-errors"])
    click(a.title, b["error-list-messages"])
    time.sleep(0.8)
    out["shot_warnings"] = shot(a.shots, "error-list-warnings")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["definition", "references", "errors"], required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    a = ap.parse_args()
    out = {"mode": a.mode}
    find_window(a.title)
    wait_loaded(a, out)
    {"definition": definition, "references": references, "errors": errors}[a.mode](a, out)
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
