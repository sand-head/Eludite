#!/usr/bin/env python3
"""Brief 0039's Xvfb run of the sandbox opt-in (see browser-sandbox-linux.sh): drives `eludite --bounds-out B` with
real X input (xdotool) and takes screenshots with ImageMagick's `import`. Run where Chromium's sandbox cannot start
(as root, as in this container, or with neither user namespaces nor the setuid helper) and without
ELUDITE_CHROME_NO_SANDBOX:

  1. View > Other Windows > Web Browser: the engine refuses to start without the sandbox, and the window shows the
     dialog "Chromium's sandbox cannot start on this machine" with both remedies (browser-sandbox-dialog.png).
  2. Space checks "Run without the sandbox for this workspace", Enter is OK: the setting is stored in the workspace's
     .eludite/settings.json, the engine starts with --allow-no-sandbox, and the strip "Browser running without
     Chromium's sandbox" shows above the page (browser-sandbox-strip.png).

Prints one JSON object: the window's log lines, the timings and the workspace's settings file.
Usage: browser_sandbox.py --pid PID --log eludite.log --bounds bounds.json --shots DIR --workspace DIR
"""
import argparse, json, os, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from browser_window import key, lines, open_window, shot, wait_line, window_origin  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    for k in ("--pid", "--log", "--bounds", "--shots", "--workspace"):
        ap.add_argument(k, required=True)
    a = ap.parse_args()
    out = {}
    origin = window_origin()
    time.sleep(1.0)
    t0 = time.time()
    open_window(a, origin)
    refused = wait_line(a, lambda l: l.startswith("the sandbox cannot start"), 0, 60)
    out["refused"] = refused[1] if refused else None
    out["refused_after_s"] = round(time.time() - t0, 2)
    if not refused:
        out["error"] = "the engine did not refuse: run as root or without user namespaces and the helper"
        out["lines"] = lines(a)
        print(json.dumps(out, indent=1), flush=True)
        return
    time.sleep(0.8)
    out["shot_dialog"] = shot(a, "browser-sandbox-dialog")
    key("space")
    time.sleep(0.4)
    key("Return")
    taken = wait_line(a, lambda l: l.startswith("the sandbox opt-in was taken"), refused[0], 10)
    started = wait_line(a, lambda l: l.startswith("running without Chromium's sandbox"), refused[0], 60)
    out["taken"] = bool(taken)
    out["started"] = started[1] if started else None
    pixel = wait_line(a, lambda l: l.startswith("first page pixel"), refused[0], 30)
    out["first_pixel"] = pixel[1] if pixel else None
    time.sleep(1.0)
    out["shot_strip"] = shot(a, "browser-sandbox-strip")
    try:
        with open(os.path.join(a.workspace, ".eludite", "settings.json")) as f:
            out["workspace_settings"] = json.load(f)
    except (OSError, ValueError) as e:
        out["workspace_settings"] = str(e)
    out["lines"] = lines(a)
    print(json.dumps(out, indent=1), flush=True)


if __name__ == "__main__":
    main()
