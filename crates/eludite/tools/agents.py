#!/usr/bin/env python3
"""Brief 0016 manual run: drive `eludite --solution S --open-file F --agent "Claude Code" --bounds-out B` on X11
(ELUDITE_TRACE_LSP=1, stderr in --log) with real XTest key and pointer events, against the real native Claude Code
adapter, and take the three screenshots.

  1. Wait for the solution and the injected error's diagnostics; Ctrl+\\, Ctrl+C shows the Agents window.
     Brief 0057, before any prompt (no model call): type a prompt that wraps to three rows, click in its second row
     (agents-prompt-editor.png), clear it; Start the agent, wait for its slash commands, type `/mo`
     (agents-slash-menu.png), Escape, clear.
     Brief 0058, still with no prompt sent: wait for the agent's options, click the model picker under the prompt
     box (agents-model-picker.png, the native adapter's model list), pick Opus and wait for the agent's answer. The
     next turn's usage line (in --transcript-out's transcript) then names the picked model.
  2. Click the prompt box, type "List the current errors and fix the first one", Enter. Claude calls
     diagnostics-list (no prompt: class read) and proposes an edit, held as a pending change; its review view opens
     (screenshot agents-pending-diff.png). Click Accept in the review view; wait for the error to clear and the turn
     to end; click the transcript's link to the change, which opens the file at it (agents-error-cleared.png).
  3. Type a prompt that makes Claude run a shell command; the permission prompt appears in the window
     (agents-permission-prompt.png); click Deny; wait for the turn to end (agents-permission-denied.png).

Permission requests the run does not expect (anything before the pending change that is not a shell command) are
allowed and recorded. Prints one JSON object with what the trace showed.
Usage: agents.py --title "Eludite - Eludite" --log run.err --bounds B --shots DIR --file F --shell-prompt TEXT
       [--transcript T --model opus]
"""
import argparse, json, sys, time

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import bounds, find_window, press, shot, trace, type_text, wait_trace  # noqa: E402
from navigation import click, since  # noqa: E402

PROMPT = "List the current errors and fix the first one"
# Brief 0057: long enough to wrap to three rows in the Agents window's default width.
WRAPPED = ("Explain what HostRpcTarget does when the host receives a ping, which fields the result carries, "
           "and where the timestamp comes from")


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


def errors_in(text):
    # "publishDiagnostics URI version N: total (E errors)"
    try:
        return int(text.rsplit("(", 1)[1].split()[0])
    except (IndexError, ValueError):
        return None


def send(a, text):
    box = rect(a, "agents-prompt")
    if not box:
        raise RuntimeError("no prompt box in the bounds file")
    click(a.title, box)
    type_text(text, delay=0.04)
    press("Return")


def editor_shots(a, out):
    """Brief 0057: the prompt editor with a wrapped prompt and a clicked caret, then the slash menu on `/mo`."""
    box = rect(a, "agents-prompt")
    if not box:
        out["editor_error"] = "no prompt box in the bounds file"
        return
    click(a.title, box)
    type_text(WRAPPED, delay=0.02)
    time.sleep(1.0)
    box = rect(a, "agents-prompt")
    out["editor_box"] = box
    # The box is its rows plus 2 px of padding and 1 px of border above and below: click in the second row, at 40% of
    # its width.
    rows = 3
    row_h = (box[3] - 6) / rows
    at = [box[0] + box[2] * 0.4, box[1] + 3 + row_h * 1.5, 0, 0]
    out["editor_click"] = at
    click(a.title, at)
    time.sleep(0.8)
    out["shot_editor"] = shot(a.shots, "agents-prompt-editor")
    press("a", ["Control_L"])
    press("BackSpace")
    # The slash menu needs the agent's commands: Start, then wait for them.
    t = ms()
    start = rect(a, "agents-start", timeout=5)
    if start:
        click(a.title, start)
    commands = wait_trace(a.log, lambda s: s.startswith("agents commands"), t, 90)
    out["commands"] = commands and commands[1]
    click(a.title, rect(a, "agents-prompt"))
    type_text("/mo", delay=0.1)
    time.sleep(1.0)
    out["shot_slash"] = shot(a.shots, "agents-slash-menu")
    press("Escape")
    press("a", ["Control_L"])
    press("BackSpace")
    time.sleep(0.5)


def model_shot(a, out):
    """Brief 0058: the model picker open with the real adapter's list, then a pick (no model call)."""
    t = ms() - 120_000
    options = wait_trace(a.log, lambda s: s.startswith("agents options") and "model=" in s, t, 90)
    out["options"] = options and options[1]
    button = rect(a, "agents-option-model", timeout=10)
    if not button:
        out["model_error"] = "no model picker in the bounds file"
        return
    click(a.title, button)
    time.sleep(1.0)
    out["shot_model_picker"] = shot(a.shots, "agents-model-picker")
    row = rect(a, f"agents-option-model-{a.model}", timeout=5)
    if not row:
        out["model_error"] = f"no row for {a.model}"
        press("Escape")
        return
    t = ms()
    click(a.title, row)
    picked = wait_trace(a.log, lambda s: s == f"agents option model = {a.model}", t, 10)
    out["model_picked_ms"] = picked and picked[0] - t
    time.sleep(0.5)


def usage_models(path):
    """The models of the usage lines in the transcript written by --transcript-out, in order."""
    try:
        rows = json.load(open(path))
    except (OSError, ValueError):
        return []
    return [r["usage"].get("model") for r in rows if isinstance(r, dict) and isinstance(r.get("usage"), dict)]


def turn(a, out, key, start, timeout, on_permission):
    """Wait for the turn's end, answering permission prompts with `on_permission(tool) -> 'allow'|'deny'`."""
    end = time.time() + timeout
    seen = 0
    while time.time() < end:
        lines = since(a.log, start)
        asked = [l for l in lines if l.startswith("agents permission asked")]
        for l in asked[seen:]:
            seen += 1
            tool = l[len("agents permission asked "):]
            decision = on_permission(tool)
            out.setdefault(key + "_permissions", []).append([tool, decision])
            if decision is None:
                continue
            b = rect(a, f"agents-permission-{decision}")
            if b:
                click(a.title, b)
        ended = [l for l in lines if l.startswith("agents turn ended")]
        if ended:
            return ended[-1]
        time.sleep(0.2)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    ap.add_argument("--file", required=True)
    ap.add_argument("--shell-prompt", required=True)
    ap.add_argument("--transcript", help="--transcript-out's file, to read the usage line after the model pick")
    ap.add_argument("--model", default="opus", help="the model picker's value to pick (brief 0058)")
    ap.add_argument("--dry", action="store_true", help="a scripted agent: skip waiting for the error to clear and the shell step")
    a = ap.parse_args()
    out = {}
    find_window(a.title)
    t0 = ms() - 600_000
    loaded = wait_trace(a.log, lambda s: s.startswith("solution Loaded"), t0, 300)
    out["loaded"] = loaded and loaded[1]
    name = a.file.rsplit("/", 1)[-1]
    diag = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and name in s and (errors_in(s) or 0) > 0,
                      t0, 180)
    out["error_before"] = diag and diag[1]
    time.sleep(1.0)

    # View > Agents with its key; close Properties so the Agents window has the right dock's height.
    press("backslash", ["Control_L"])
    press("c", ["Control_L"])
    time.sleep(1.0)
    props = rect(a, "close-properties", timeout=5)
    if props:
        click(a.title, props)
    time.sleep(0.5)

    # 0. Brief 0057: the prompt editor and the slash menu. Brief 0058: the model picker, then a pick.
    editor_shots(a, out)
    model_shot(a, out)

    # 1. The error: diagnostics-list, a pending change, Accept.
    start = ms()
    send(a, PROMPT)
    out["prompt_sent_ms"] = start
    pending = None
    end = time.time() + 300
    seen = 0
    while time.time() < end and not pending:
        lines = since(a.log, start)
        for l in [l for l in lines if l.startswith("agents permission asked")][seen:]:
            seen += 1
            tool = l[len("agents permission asked "):]
            decision = "deny" if tool.startswith("Bash") else "allow"
            out.setdefault("fix_permissions", []).append([tool, decision])
            b = rect(a, f"agents-permission-{decision}")
            if b:
                click(a.title, b)
        p = [l for l in lines if l.startswith("agents pending change #")]
        if p:
            pending = p[0]
        if [l for l in lines if l.startswith("agents turn ended")]:
            break
        time.sleep(0.2)
    out["pending"] = pending
    out["fix_trace_before_review"] = since(a.log, start)[-25:]
    if not pending:
        out["error"] = "no pending change"
        out["shot_no_change"] = shot(a.shots, "agents-no-change")
        print(json.dumps(out), flush=True)
        return
    change = pending.split("#", 1)[1].split()[0]
    time.sleep(2.0)
    out["shot_pending"] = shot(a.shots, "agents-pending-diff")
    accept = rect(a, f"review-{change}-accept")
    out["accept_rect"] = accept
    t_accept = ms()
    click(a.title, accept)
    accepted = wait_trace(a.log, lambda s: s.startswith(f"agents change #{change} "), t_accept, 60)
    out["decision"] = accepted and accepted[1]
    cleared = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and name in s and errors_in(s) == 0,
                         t_accept, 10 if a.dry else 120)
    out["error_after"] = cleared and cleared[1]
    out["accept_to_clear_ms"] = cleared and cleared[0] - t_accept
    ended = turn(a, out, "fix", t_accept, 300, lambda tool: "deny" if tool.startswith("Bash") else "allow")
    out["fix_turn"] = ended
    out["fix_trace"] = since(a.log, start)[-40:]
    if a.transcript:
        # Brief 0058: the turn after the pick ran on the picked model.
        time.sleep(1.0)
        out["usage_after_pick"] = usage_models(a.transcript)[-1:]
    time.sleep(1.0)
    link = rect(a, f"agents-change-{change}", timeout=5)
    if link:
        click(a.title, link)
    time.sleep(2.0)
    out["shot_cleared"] = shot(a.shots, "agents-error-cleared")
    if a.dry:
        print(json.dumps(out), flush=True)
        return

    # 2. A shell command: the permission prompt, denied.
    start = ms()
    send(a, a.shell_prompt)
    asked = wait_trace(a.log, lambda s: s.startswith("agents permission asked"), start, 300)
    out["shell_asked"] = asked and asked[1]
    if asked:
        time.sleep(1.5)
        out["shot_prompt"] = shot(a.shots, "agents-permission-prompt")
    ended = turn(a, out, "shell", start, 300, lambda tool: "deny")
    out["shell_turn"] = ended
    time.sleep(1.5)
    out["shot_denied"] = shot(a.shots, "agents-permission-denied")
    out["shell_trace"] = since(a.log, start)[-25:]
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
