#!/usr/bin/env bash
# Brief 0016 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with the
# real eludite-host, Roslyn and the native Claude Code adapter (agents/claude-acp, release build):
#   1. An error is put into HostRpcTarget.cs (`timestamp` misspelled in Ping), which the run opens.
#   2. X11 backend on the nested Xwayland, real XTest input (tools/agents.py): Ctrl+\, Ctrl+C shows the Agents window;
#      brief 0057 first, with no prompt sent: a prompt that wraps to three rows with the caret clicked into its second
#      row (agents-prompt-editor.png), then Start and the slash menu on `/mo` with the adapter's commands
#      (agents-slash-menu.png); brief 0058, still before any prompt: the model picker under the prompt box open with
#      the adapter's model list (agents-model-picker.png), then Opus picked (no model call), so the next turn's usage
#      line (transcript.json) names the picked model; then
#      the prompt "List the current errors and fix the first one"; Claude calls diagnostics-list and proposes an edit
#      held as a pending change (agents-pending-diff.png); Accept; the error clears (agents-error-cleared.png); then a
#      prompt that makes Claude run a shell command: the permission prompt (agents-permission-prompt.png); brief
#      0059, still mid-turn: an earlier tool card expanded, the status line and the usage strip
#      (agents-polish-running.png); Deny (agents-permission-denied.png). Three real prompts.
#   2b. Brief 0059, a second run with --theme light (SKIP_POLISH=1 skips it): one prompt that makes Claude run `ls`
#      (allowed), then after the turn a tool card expanded and the strip with the turn's usage
#      (agents-polish-light.png). One real prompt. POLISH_THEMES="light blue" takes one shot per theme
#      (agents-polish-<theme>.png).
#   3. Wayland backend, RUNS runs each: --bench-agent-ready 5 (the real adapter, no prompt), --bench-agent-stream (the
#      fake agent at 200 chunks/s), --bench-agent-prompt (2,000 keystrokes into the prompt box, then the stream with
#      500 characters in it; brief 0057), --bench-diff 20                             -> OUT_DIR/agents-bench.jsonl
#   The 1-minute load average before each step goes to OUT_DIR/loadavg.txt.
# Usage: tools/agents-linux.sh OUT_DIR    (RUNS=3; SKIP_DRIVE=1 skips 2, SKIP_POLISH=1 skips 2b, SKIP_BENCH=1 skips 3;
#        DRY=1 with
#        ELUDITE_CLAUDE_ACP pointing at a scripted agent checks the driving without a real prompt)
# The run needs dotnet/ clean in git and restores it with `git checkout -- dotnet/` at the end.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
if [[ -n "$(git -C "$repo" status --porcelain -- dotnet/)" ]]; then
  echo "dotnet/ has changes; commit or stash them first (the run restores dotnet/ from git)" >&2
  exit 1
fi
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/release/eludite}
fake=$target_dir/release/eludite-fake-acp-agent
adapter=${ELUDITE_CLAUDE_ACP:-$repo/agents/claude-acp/target/release/eludite-claude-acp}
sln=$repo/dotnet/Eludite.slnx
file=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
runs=${RUNS:-3}
title="Eludite - Eludite"
shell_prompt=${SHELL_PROMPT:-"Run the shell command touch eludite-denied.txt in the solution folder"}
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
export ELUDITE_CLAUDE_ACP=$(q "$adapter") DRY=${DRY:-}
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
restore() { git -C $(q "$repo") checkout -q -- dotnet/; }
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  sed -i 's/return new PingResult(true, timestamp);/return new PingResult(true, timestmp);/' $(q "$file")
  load drive
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$file") \\
    --agent "Claude Code" --bounds-out $(q "$out")/bounds.json --transcript-out $(q "$out")/transcript.json \\
    >$(q "$out")/drive.out 2>$(q "$out")/drive.err &
  pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/agents.py") --title $(q "$title") \\
    --log $(q "$out")/drive.err --bounds $(q "$out")/bounds.json --shots $(q "$out") --file $(q "$file") \\
    --shell-prompt $(q "$shell_prompt") --transcript $(q "$out")/transcript.json \${DRY:+--dry} \
    >$(q "$out")/drive.json 2>$(q "$out")/driver.err || true
  sleep 1
  kill \$pid; wait \$pid
  git -C $(q "$repo") diff -- dotnet/ >$(q "$out")/dotnet-after.diff
  ls -la $(q "$repo")/dotnet/eludite-denied.txt >$(q "$out")/denied-file.txt 2>&1 || true
  restore
fi
if [[ -z "${SKIP_POLISH:-}" ]]; then
  for theme in ${POLISH_THEMES:-light}; do
    load "polish-\$theme"
    env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --no-persist --theme "\$theme" --solution $(q "$sln") \\
      --agent "Claude Code" --bounds-out $(q "$out")/bounds-\$theme.json \\
      --transcript-out $(q "$out")/transcript-\$theme.json >$(q "$out")/polish-\$theme.out \\
      2>$(q "$out")/polish-\$theme.err &
    pid=\$!
    SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/agents.py") --polish-light \\
      --title $(q "$title") --log $(q "$out")/polish-\$theme.err --bounds $(q "$out")/bounds-\$theme.json \\
      --shots $(q "$out") --polish-name "agents-polish-\$theme" \\
      >$(q "$out")/polish-\$theme.json 2>$(q "$out")/polish-driver-\$theme.err || true
    sleep 1
    kill \$pid; wait \$pid
  done
  restore
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  for run in \$(seq 1 $runs); do
    load "ready-\$run"
    $(q "$bin") --reset-layout --no-persist --agent "Claude Code" --bench-agent-ready 5 \\
      >>$(q "$out/agents-bench.jsonl") 2>$(q "$out")/ready-\$run.err
    load "stream-\$run"
    $(q "$bin") --reset-layout --no-persist --bench-agent-stream $(q "$fake") \\
      >>$(q "$out/agents-bench.jsonl") 2>$(q "$out")/stream-\$run.err
    load "prompt-\$run"
    $(q "$bin") --reset-layout --no-persist --bench-agent-prompt $(q "$fake") \\
      >>$(q "$out/agents-bench.jsonl") 2>$(q "$out")/prompt-\$run.err
    load "diff-\$run"
    $(q "$bin") --reset-layout --no-persist --bench-diff 20 \\
      >>$(q "$out/agents-bench.jsonl") 2>$(q "$out")/diff-\$run.err
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 3600 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
git -C "$repo" checkout -q -- dotnet/
rm -f "$repo/dotnet/eludite-denied.txt"
git -C "$repo" status --porcelain -- dotnet/
ls "$out"
