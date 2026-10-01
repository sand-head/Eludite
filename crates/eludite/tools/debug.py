#!/usr/bin/env python3
"""Brief 0018 manual run: drive `eludite --solution dotnet/Eludite.slnx --open-file HostRpcTarget.cs --bounds-out B` on
X11 (ELUDITE_TRACE_LSP=1, stderr in --log) with real XTest key and pointer events, debugging eludite-host itself:

  F9 on Ping's first statement, F5; once the program runs and the breakpoint is bound, an eludite/ping is written into
  the debuggee's stdin from this process (a second terminal, not the IDE); the break with Locals and the Call Stack
  (debug-break.png); F10, then the pointer rests on `timestamp` for a data tip (debug-step-datatip.png); the
  Breakpoints tab, still in break mode (debug-breakpoints.png); F5, the Debug Console tab with the pong
  (debug-continue-console.png); Shift+F5.

Prints one JSON object with the trace lines and timings. The file is never saved.
Usage: debug.py --title "Eludite - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, json, re, sys, time

from Xlib import X
from Xlib.ext import xtest

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import bounds, d, press, shot, trace, wait_loaded, wait_trace  # noqa: E402
from navigation import caret_on, click, window_origin  # noqa: E402


def now():
    return int(time.time() * 1000)


def since(log, start):
    return [t for t0, t in trace(log) if t0 >= start]


def ping(pid, n):
    body = json.dumps({"jsonrpc": "2.0", "id": n, "method": "eludite/ping"}).encode()
    with open(f"/proc/{pid}/fd/0", "wb") as f:
        f.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--title", required=True)
    p.add_argument("--log", required=True)
    p.add_argument("--bounds", required=True)
    p.add_argument("--shots", required=True)
    a = p.parse_args()
    out = {}
    wait_loaded(a, out)
    caret_on("var timestamp = _timeProvider")
    start = now()
    press("F9")
    time.sleep(0.5)
    press("F5")
    running = wait_trace(a.log, lambda s: s.startswith("debug running"), start, 60)
    bound = wait_trace(a.log, lambda s: s.startswith("debug breakpoint") and "verified=true" in s, start, 60)
    proc = wait_trace(a.log, lambda s: s.startswith("debug process Some("), start, 60)
    out["f5_to_running_ms"] = running and running[0] - start
    out["f5_to_bound_ms"] = bound and bound[0] - start
    if not proc:
        out["error"] = "no debuggee process"
        out["trace"] = since(a.log, start)[-20:]
        print(json.dumps(out))
        return
    pid = int(re.search(r"Some\((\d+)\)", proc[1]).group(1))
    out["debuggee_pid"] = pid
    time.sleep(0.5)
    ping_at = now()
    ping(pid, 1)
    brk = wait_trace(a.log, lambda s: s.startswith("debug locals stop 1"), ping_at, 30)
    out["ping_to_break_ms"] = brk and brk[0] - ping_at
    out["break"] = brk and brk[1]
    time.sleep(1.0)
    out["shot_break"] = shot(a.shots, "debug-break")
    step_at = now()
    press("F10")
    st = wait_trace(a.log, lambda s: s.startswith("debug locals stop 2"), step_at, 30)
    out["f10_to_locals_ms"] = st and st[0] - step_at
    out["step"] = st and st[1]
    time.sleep(0.5)
    # A data tip: the pointer rests on `timestamp` in `return new PingResult(true, timestamp);`.
    caret_on("timestamp);")
    time.sleep(0.6)
    b = bounds(a.bounds)
    caret = b.get("editor-caret")
    if caret:
        ox, oy = window_origin(a.title)
        x, y = int(ox + caret[0] + 12), int(oy + caret[1] + caret[3] / 2)
        xtest.fake_input(d, X.MotionNotify, x=x, y=y)
        d.sync()
        time.sleep(0.15)
        xtest.fake_input(d, X.MotionNotify, x=x + 2, y=y)
        d.sync()
        time.sleep(1.5)
    out["shot_step"] = shot(a.shots, "debug-step-datatip")
    xtest.fake_input(d, X.MotionNotify, x=5, y=5)
    d.sync()
    tab = bounds(a.bounds).get("tab-breakpoints")
    if tab:
        click(a.title, tab)
    time.sleep(0.8)
    out["shot_breakpoints"] = shot(a.shots, "debug-breakpoints")
    cont_at = now()
    press("F5")
    time.sleep(1.0)
    tab = bounds(a.bounds).get("tab-debug_console")
    if tab:
        click(a.title, tab)
    time.sleep(0.8)
    out["shot_continue"] = shot(a.shots, "debug-continue-console")
    out["continue_trace"] = since(a.log, cont_at)[-5:]
    stop_at = now()
    press("F5", ["Shift_L"])
    ended = wait_trace(a.log, lambda s: s.startswith("debug ended"), stop_at, 30)
    out["shift_f5_to_ended_ms"] = ended and ended[0] - stop_at
    out["trace"] = [t for t in since(a.log, start) if t.startswith("debug")][-40:]
    print(json.dumps(out))


if __name__ == "__main__":
    main()
