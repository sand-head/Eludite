#!/usr/bin/env python3
"""Brief 0019 manual run: drive `eludite --folder REPO --open-file crates/editor/src/buffer.rs --bounds-out B` on X11
(ELUDITE_TRACE_LSP=1, stderr in --log) with real XTest key and pointer events, against the real eludite-host (the
solution) and the real rust-analyzer, and take the screenshots.

  1. The folder opens: the Workspace window shows the .NET solution and the Cargo workspace side by side under the
     folder root (workspace-side-by-side.png, once the host's tree and `cargo metadata` arrived).
  2. rust-analyzer starts with buffer.rs and becomes quiescent. In `Buffer::len`, before `self.text.len()`, typing
     `self.text.` opens rust-analyzer's completion list (rust-completion.png); Escape and Backspace remove it.
  3. `let _n: usize = "len";` there is a type error: rust-analyzer's diagnostics (squiggle, live Error List row,
     rust-diagnostics.png), Ctrl+S, Ctrl+Shift+B: cargo builds the workspace, the build fails, the Error List comes
     forward with cargo's row; Ctrl+Home moves the caret away and a double-click on the row opens the file at the
     error (rust-build-error.png).

Prints one JSON object with the trace lines and the latencies. The run script restores buffer.rs from git.
Usage: rust.py --title "0019-rust-workspace - Eludite" --log run.err --bounds B --shots DIR
"""
import argparse, json, sys, time

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import bounds, find_window, press, shot, type_text, wait_trace  # noqa: E402
from navigation import caret_on, since  # noqa: E402
from build import double_click, rect  # noqa: E402


def ms():
    return int(time.time() * 1000)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    a = ap.parse_args()
    out = {}
    find_window(a.title, timeout=120)
    t0 = ms() - 600_000

    # 1. The folder's parts.
    opened = wait_trace(a.log, lambda s: s.startswith("open folder"), t0, 120)
    out["opened"] = opened
    cargo = wait_trace(a.log, lambda s: s.startswith("cargo metadata:"), t0, 120)
    out["cargo_metadata"] = cargo and cargo[1]
    out["open_to_cargo_metadata_ms"] = opened and cargo and cargo[0] - opened[0]
    tree = wait_trace(a.log, lambda s: s.startswith("tree generation"), t0, 300)
    out["tree"] = tree and tree[1]
    out["open_to_tree_ms"] = opened and tree and tree[0] - opened[0]
    time.sleep(2.0)
    out["shot_workspace"] = shot(a.shots, "workspace-side-by-side")

    # 2. rust-analyzer ready, then completion in Buffer::len.
    quiet = wait_trace(a.log, lambda s: s.startswith("rust-analyzer|") and " quiescent" in s, t0, 900)
    out["rust_analyzer_quiescent"] = quiet and quiet[1]
    out["open_to_quiescent_ms"] = opened and quiet and quiet[0] - opened[0]
    first = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and "buffer.rs" in s, t0, 60)
    out["open_to_first_buffer_diagnostics_ms"] = opened and first and first[0] - opened[0]
    time.sleep(3.0)
    caret_on("self.text.len()")
    start = ms()
    type_text("self.text.", delay=0.08)
    reply = wait_trace(a.log, lambda s: s.startswith("completion reply") and "items" in s
                       and not s.endswith(": 0 items (host 0.0 ms)"), start, 30)
    out["completion_reply"] = reply and reply[1]
    out["key_to_completion_reply_ms"] = reply and reply[0] - start
    time.sleep(1.5)
    out["shot_completion"] = shot(a.shots, "rust-completion")
    press("Escape")
    for _ in range(len("self.text.")):
        press("BackSpace")
    time.sleep(0.5)

    # 3. A type error: rust-analyzer's live diagnostics, then cargo's.
    start = ms()
    type_text('let _n: usize = "len";', delay=0.05)
    press("Return")
    diag = wait_trace(a.log, lambda s: s.startswith("publishDiagnostics") and "buffer.rs" in s
                      and "(0 errors)" not in s, start, 60)
    out["live_diagnostics"] = diag and diag[1]
    out["edit_to_live_diagnostics_ms"] = diag and diag[0] - start
    time.sleep(1.5)
    tab = rect(a, "tab-error_list")
    if tab:
        from navigation import click  # noqa: E402
        click(a.title, tab)
        time.sleep(1.0)
    out["shot_diagnostics"] = shot(a.shots, "rust-diagnostics")
    caret = rect(a, "editor-caret")
    if caret:
        from navigation import click  # noqa: E402
        click(a.title, caret)
    press("s", ["Control_L"])
    time.sleep(0.5)
    start = ms()
    press("b", ["Control_L", "Shift_L"])
    first = wait_trace(a.log, lambda s: s.startswith("build first output"), start, 60)
    out["key_to_first_output_ms"] = first and first[0] - start
    done = wait_trace(a.log, lambda s: s.startswith("build finished"), start, 1800)
    out["build_finished"] = done and done[1]
    out["key_to_build_finished_ms"] = done and done[0] - start
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
    out["build_trace"] = [l for l in since(a.log, start) if l.startswith("build")][-8:]
    out["shot_build_error"] = shot(a.shots, "rust-build-error")
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
