#!/usr/bin/env python3
"""Brief 0015 manual run: drive `eludite --solution S --open-file F --bounds-out B` on X11 (ELUDITE_TRACE_LSP=1,
stderr in --log) with real XTest keys, and take the rename, code action and completion screenshots.

  --mode rename      in HostRpcTarget.cs: the caret on `Ping` of `PingResult Ping()`, Ctrl+R, Ctrl+R, type `PingHost`
                     (the Rename dialog's preview, screenshot rename-preview.png), Enter (rename-applied.png)
  --mode action      in DotnetCliSdkDiscoverer.cs: the caret on the constructor's name (the light bulb,
                     lightbulb.png), Ctrl+. (the menu, code-action-menu.png), Down to "Use primary constructor",
                     Enter (primary-constructor.png)
  --mode completion  in DotnetCliSdkDiscoverer.cs: a new line before `var startInfo`, `var sb = new StringBuil`,
                     Ctrl+Space, Tab (StringBuilder, an unimported type: `using System.Text;` is added), then Ctrl+Home
                     (completion-using.png)

Prints one JSON object with what the trace log showed. Files are saved only by the applier (closed files a rename
touches); the run script restores them.
Usage: refactor.py --mode rename|action|completion --title "Eludite - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, ast, json, sys, time

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import find_window, press, shot, trace, type_text, wait_loaded, wait_trace  # noqa: E402
from navigation import caret_on, since  # noqa: E402


def ms():
    return int(time.time() * 1000)


def rename(a, out):
    caret_on("Ping()", ["Right"])
    start = ms()
    press("r", ["Control_L"])
    press("r", ["Control_L"])
    prepared = wait_trace(a.log, lambda s: s.startswith("prepareRename") and '"' in s, start, 60)
    out["prepare"] = prepared and prepared[1]
    out["prepare_ms"] = prepared and prepared[0] - start
    time.sleep(0.5)
    start = ms()
    type_text("PingHost", delay=0.08)
    preview = wait_trace(a.log, lambda s: s.startswith("rename ") and "edits in" in s, start, 60)
    out["preview"] = preview and preview[1]
    time.sleep(1.5)
    out["shot_preview"] = shot(a.shots, "rename-preview")
    start = ms()
    press("Return")
    applied = wait_trace(a.log, lambda s: s.startswith("workspace edit") and "Rename" in s, start, 60)
    out["applied"] = applied and applied[1]
    time.sleep(1.5)
    out["trace"] = since(a.log, start)[-6:]
    out["shot_applied"] = shot(a.shots, "rename-applied")


def action(a, out):
    start = ms()
    caret_on("DotnetCliSdkDiscoverer(string", ["Right"])
    bulb = wait_trace(a.log, lambda s: s.startswith("codeAction reply") and "actions" in s, start, 60)
    out["lightbulb"] = bulb and bulb[1]
    time.sleep(1.0)
    out["shot_lightbulb"] = shot(a.shots, "lightbulb")
    start = ms()
    press("period", ["Control_L"])
    menu = wait_trace(a.log, lambda s: s.startswith("code action menu:"), start, 60)
    out["menu"] = menu and menu[1]
    if not menu:
        out["error"] = "no menu"
        return
    selected, titles = menu[1][len("code action menu: selected "):].split("; ", 1)
    selected, titles = ast.literal_eval(selected), ast.literal_eval(titles)
    target = "Use primary constructor"
    if target not in titles:
        out["error"] = f"{target!r} is not in the menu"
        out["shot_menu"] = shot(a.shots, "code-action-menu")
        return
    for _ in range(titles.index(target) - titles.index(selected)):
        press("Down")
    time.sleep(1.0)
    out["shot_menu"] = shot(a.shots, "code-action-menu")
    start = ms()
    press("Return")
    applied = wait_trace(a.log, lambda s: s.startswith("workspace edit") and target in s, start, 60)
    out["applied"] = applied and applied[1]
    time.sleep(1.5)
    out["trace"] = since(a.log, start)[-6:]
    out["shot_applied"] = shot(a.shots, "primary-constructor")


def completion(a, out):
    caret_on("var startInfo")
    press("Home")
    press("Return")
    press("Up")
    start = ms()
    type_text("var sb = new StringBuil", delay=0.08)
    time.sleep(1.5)
    # The unimported types are in the list once Roslyn's import cache is ready: ask again explicitly.
    press("Escape")
    press("space", ["Control_L"])
    reply = wait_trace(a.log, lambda s: s.startswith("completion reply") and "items" in s, ms() - 50, 30)
    out["completion"] = reply and reply[1]
    time.sleep(1.5)
    out["shot_list"] = shot(a.shots, "completion-list")
    press("Tab")
    accepted = wait_trace(a.log, lambda s: s.startswith("accept completion"), start, 30)
    out["accepted"] = accepted and accepted[1]
    edits = wait_trace(a.log, lambda s: "additional edits" in s, start, 30)
    out["additional"] = edits and edits[1]
    type_text("();", delay=0.08)
    time.sleep(1.0)
    press("Home", ["Control_L"])
    time.sleep(1.0)
    out["trace"] = since(a.log, start)[-8:]
    out["shot_using"] = shot(a.shots, "completion-using")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["rename", "action", "completion"], required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    a = ap.parse_args()
    out = {"mode": a.mode}
    find_window(a.title)
    wait_loaded(a, out)
    {"rename": rename, "action": action, "completion": completion}[a.mode](a, out)
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
