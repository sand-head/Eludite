#!/usr/bin/env python3
"""Brief 0020 manual run: drive `eludite --solution dotnet/Eludite.slnx --open-file HostRpcTarget.cs --bounds-out B`
on X11 (ELUDITE_TRACE_LSP=1, stderr in --log) with real XTest key and pointer events:

  1. Tools > Options (the menu, then the item), the page Projects and Solutions > Build and Run
     (integration-options.png); its "Build the project after saving" check box clicked on and off again, each a
     settings.json write; OK.
  2. Settings reload: the user settings file rewritten from outside the IDE N times; the time from the write to the
     shell's "settings applied" trace line.
  3. An error typed at the end of HostRpcTarget.cs, Ctrl+S, F5: the startup project's build fails and nothing launches
     (integration-f5-build-failed.png: the Error List forward, the status bar's reason). Backspace, Ctrl+S, F5: the
     build succeeds and the program launches under netcoredbg (integration-f5-launched.png: the Output window's Debug
     source, the status bar); Shift+F5.

Prints one JSON object with the trace lines and timings. The file is restored by the caller (git checkout).
Usage: integration.py --title "Eludite - Eludite" --log run.err --bounds B --shots DIR --settings USER_SETTINGS_JSON
"""
import argparse, json, os, sys, time

sys.path.insert(0, os.path.dirname(__file__))
from intellisense import bounds, press, shot, type_text, wait_loaded, wait_trace  # noqa: E402
from navigation import click, since  # noqa: E402


def now():
    return int(time.time() * 1000)


def rect(a, key, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        b = bounds(a.bounds).get(key)
        if b:
            return b
        time.sleep(0.2)
    return None


def options(a, out):
    tools = rect(a, "menu-Tools")
    out["menu_tools"] = tools
    if not tools:
        out["error"] = "no Tools menu bounds"
        return
    click(a.title, tools)
    item = rect(a, "menu-item-Tools-Options...")
    out["menu_item_options"] = item
    if not item:
        out["error"] = "no Options... item"
        return
    click(a.title, item)
    page = rect(a, "options-section-1")
    if not page:
        out["error"] = "the Options dialog did not open"
        return
    click(a.title, page)
    time.sleep(0.8)
    out["shot_options"] = shot(a.shots, "integration-options")
    box = rect(a, "options-setting-build.onSave")
    if box:
        for on in (True, False):
            at = now()
            click(a.title, box)
            applied = wait_trace(a.log, lambda s: s.startswith("settings applied"), at, 10)
            out[f"options_on_save_{str(on).lower()}_applied_ms"] = applied and applied[0] - at
            time.sleep(0.4)
            try:
                with open(a.settings) as f:
                    out[f"settings_file_after_{str(on).lower()}"] = json.load(f)
            except (OSError, ValueError) as e:
                out[f"settings_file_after_{str(on).lower()}"] = str(e)
    ok = rect(a, "options-ok")
    out["options_ok"] = ok
    if ok:
        click(a.title, ok)
    time.sleep(0.5)
    out["options_closed"] = rect(a, "options-section-1", timeout=1) is None


def reload(a, out, n=10):
    times = []
    for i in range(n):
        value = {"build.showOutputOnStart": True, "build.onSave": i % 2 == 0}
        tmp = a.settings + ".tmp"
        with open(tmp, "w") as f:
            json.dump(value, f)
        at = now()
        os.replace(tmp, a.settings)
        applied = wait_trace(a.log, lambda s: s.startswith("settings applied"), at, 5)
        times.append(applied and applied[0] - at)
        time.sleep(0.3)
    out["reload_write_to_applied_ms"] = times
    with open(a.settings, "w") as f:
        json.dump({}, f)
    time.sleep(0.3)


def f5(a, out, key):
    at = now()
    press("F5")
    out[key + "_at"] = at
    return at


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--title", required=True)
    p.add_argument("--log", required=True)
    p.add_argument("--bounds", required=True)
    p.add_argument("--shots", required=True)
    p.add_argument("--settings", required=True)
    a = p.parse_args()
    out = {}
    wait_loaded(a, out)

    options(a, out)
    reload(a, out)

    # The editor has the focus again; an error at the end of the file, saved.
    caret = rect(a, "editor-caret")
    if caret:
        click(a.title, caret)
    press("End", ["Control_L"])
    type_text("x")
    press("s", ["Control_L"])
    time.sleep(0.8)
    at = f5(a, out, "f5_failing")
    failed = wait_trace(a.log, lambda s: s.startswith("debug start: Not started"), at, 600)
    out["f5_failing_result"] = failed and failed[1]
    out["f5_failing_ms"] = failed and failed[0] - at
    time.sleep(1.5)
    out["shot_failed"] = shot(a.shots, "integration-f5-build-failed")
    out["f5_failing_trace"] = [t for t in since(a.log, at) if t.startswith(("debug", "build"))][-12:]

    # Fixed, saved, F5 again: the build succeeds and the program launches.
    caret = rect(a, "editor-caret")
    if caret:
        click(a.title, caret)
    press("End", ["Control_L"])
    press("BackSpace")
    press("s", ["Control_L"])
    time.sleep(0.8)
    at = f5(a, out, "f5_fixed")
    launched = wait_trace(a.log, lambda s: s.startswith("debug start: launched"), at, 600)
    out["f5_fixed_launch"] = launched and launched[1]
    running = wait_trace(a.log, lambda s: s.startswith("debug running"), at, 120)
    out["f5_to_running_ms"] = running and running[0] - at
    time.sleep(1.0)
    # The Output window's Debug source (the debugger's Locals tab is in front of it after F5).
    tab = rect(a, "tab-output")
    if tab:
        click(a.title, tab)
    time.sleep(1.0)
    out["shot_launched"] = shot(a.shots, "integration-f5-launched")
    out["f5_fixed_trace"] = [t for t in since(a.log, at) if t.startswith(("debug", "build"))][-16:]
    stop_at = now()
    press("F5", ["Shift_L"])
    ended = wait_trace(a.log, lambda s: s.startswith("debug ended"), stop_at, 30)
    out["shift_f5_to_ended_ms"] = ended and ended[0] - stop_at
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
