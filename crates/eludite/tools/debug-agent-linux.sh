#!/usr/bin/env bash
# Brief 0030's recorded real run: a real Claude Code session in the Agents window debugs each seeded-bug corpus
# program (corpus/debugging) with Eludite's tools, RUNS times (default 3), on an X11 display (an Xvfb screen on
# DISPLAY :99 with Mesa's software Vulkan, as tools/xvfb-linux.sh starts it, or the display already set).
#
# Each run, in OUT_DIR/<Program>-<n>/:
#   1. work/: a copy of the program (and the corpus's Directory.Build.props) with work/<Program>.slnx, built there, so
#      the session's folder holds the program and nothing else (not the corpus README, which has the answers). Where
#      netcoredbg is missing (ELUDITE_NETCOREDBG unset and none on PATH), <Program>.csproj.user selects net472
#      (ActiveDebugFramework), which eludite-dbg-mono runs under Mono (ELUDITE_DBG_MONO).
#   2. work/.eludite/agents-policy.json: execute allowed, Claude's Bash denied (the prompt asks for Eludite's tools),
#      the debug policy's defaults (agents drive and evaluate).
#   3. eludite --solution work/<Program>.slnx --agent "Claude Code" --transcript-out transcript.json, with a fresh
#      config folder; `claude` runs through a wrapper that copies its stream-json output, stamped, to stream.jsonl
#      (Claude Code's own token counts: eludite-claude-acp sends no ACP usage events).
#   4. debug_agent.py drive: shows the Agents window, types the prompt, waits for the turn's end (screenshots
#      <Program>-<n>-prompt.png and -end.png), then debug_agent.py summarize writes summary.json.
# At the end, debug_agent.py report writes OUT_DIR/report/{numbers.md,calls.json,transcript.md}; copy them and the
# screenshots to docs/briefs/0030-run/.
#
# Usage: crates/eludite/tools/debug-agent-linux.sh OUT_DIR [PROGRAM...]   (default: OffByOne MissingCase NullField)
#   RUNS=3; ELUDITE_BIN (default target/debug/eludite), ELUDITE_CLAUDE_ACP (default
#   agents/claude-acp/target/release/eludite-claude-acp), ELUDITE_HOST, ELUDITE_NETCOREDBG, ELUDITE_DBG_MONO,
#   CLAUDE (the real `claude`, default from PATH). DRY=1 skips the prompt's real cost by pointing "Claude Code" at
#   ELUDITE_CLAUDE_ACP as given (a scripted agent) and is otherwise the same run.
# A real run costs subscription tokens: each program's prompt once per run.
# Needs: xvfb, mesa-vulkan-drivers, xdotool, imagemagick, the .NET SDK, and Mono for the net472 runs.
set -euo pipefail
out=$(realpath -m "$1"); shift; mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
programs=("$@"); [ ${#programs[@]} -eq 0 ] && programs=(OffByOne MissingCase NullField)
runs=${RUNS:-3}
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/debug/eludite}
adapter=${ELUDITE_CLAUDE_ACP:-$repo/agents/claude-acp/target/release/eludite-claude-acp}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
claude=${CLAUDE:-$(command -v claude || true)}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
[ -x "$adapter" ] || { echo "no eludite-claude-acp at $adapter (cd agents/claude-acp && cargo build --release)" >&2; exit 1; }
[ -n "$claude" ] || { echo "no claude on PATH (set CLAUDE)" >&2; exit 1; }
export ELUDITE_HOST="$host" ELUDITE_CLAUDE_ACP="$adapter" ELUDITE_TRACE_LSP=1 DOTNET_NOLOGO=1
if [ -z "${ELUDITE_NETCOREDBG:-}" ] && ncdb=$(command -v netcoredbg); then export ELUDITE_NETCOREDBG="$ncdb"; fi
if [ -n "${ELUDITE_NETCOREDBG:-}" ]; then
  framework=net10.0; adapter_name="netcoredbg ($ELUDITE_NETCOREDBG), net10.0"
else
  export ELUDITE_DBG_MONO=${ELUDITE_DBG_MONO:-$repo/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe}
  [ -f "$ELUDITE_DBG_MONO" ] || { echo "neither netcoredbg nor eludite-dbg-mono (dotnet build dotnet/Eludite.slnx)" >&2; exit 1; }
  framework=net472; adapter_name="eludite-dbg-mono under $(mono --version | head -1), net472"
fi

export DISPLAY=${DISPLAY:-:99}
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi

# `claude` with its stream-json output copied, each line stamped with the time it arrived (ms since the epoch).
wrapper="$out/claude-tee.sh"
cat >"$wrapper" <<WRAP
#!/usr/bin/env bash
exec $(printf %q "$claude") "\$@" > >(python3 -u -c '
import os, sys, time
log = open(os.environ["ELUDITE_CLAUDE_STREAM"], "ab")
for line in iter(sys.stdin.buffer.readline, b""):
    sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
    log.write(str(int(time.time() * 1000)).encode() + b" " + line); log.flush()
')
WRAP
chmod +x "$wrapper"
[ -n "${DRY:-}" ] || export ELUDITE_CLAUDE_PATH="$wrapper"

for program in "${programs[@]}"; do
  for n in $(seq 1 "$runs"); do
    run="$out/$program-$n"; rm -rf "$run"; mkdir -p "$run/work" "$run/config"
    cp "$repo/corpus/debugging/Directory.Build.props" "$run/work/"
    cp -r "$repo/corpus/debugging/$program" "$run/work/"
    rm -rf "$run/work/$program/bin" "$run/work/$program/obj" "$run/work/$program/$program.csproj.user"
    printf '<Solution>\n  <Project Path="%s/%s.csproj" />\n</Solution>\n' "$program" "$program" >"$run/work/$program.slnx"
    if [ "$framework" = net472 ]; then
      printf '<Project>\n  <PropertyGroup>\n    <ActiveDebugFramework>net472</ActiveDebugFramework>\n  </PropertyGroup>\n</Project>\n' \
        >"$run/work/$program/$program.csproj.user"
    fi
    dotnet build "$run/work/$program/$program.csproj" --configuration Debug --nologo >"$run/build.log" 2>&1
    mkdir -p "$run/work/.eludite"
    cat >"$run/work/.eludite/agents-policy.json" <<'POLICY'
{
  "version": 1,
  "execute": "allow",
  "rules": [{"tool": "Bash", "decision": "deny"}],
  "debug": {"drive": "allow", "evaluate": "allow"}
}
POLICY
    prompt="The program \`$run/work/$program/$program.csproj\` fails its self-check when run. Debug it with Eludite's tools, find the statement that produces the wrong value, and tell me the statement, the line and the local variable values that show it. Do not edit files."
    echo "$prompt" >"$run/prompt.txt"
    echo "$(date -Is) $program run $n" >&2
    cut -d' ' -f1-3 /proc/loadavg >"$run/loadavg.txt"
    ELUDITE_CONFIG_DIR="$run/config" ELUDITE_CLAUDE_STREAM="$run/stream.jsonl" \
      "$bin" --reset-layout --solution "$run/work/$program.slnx" --open-file "$run/work/$program/Program.cs" \
      --agent "Claude Code" --bounds-out "$run/bounds.json" --transcript-out "$run/transcript.json" \
      >"$run/eludite.out" 2>"$run/eludite.err" &
    pid=$!
    python3 "$here/debug_agent.py" drive --title "$program - Eludite" --log "$run/eludite.err" \
      --bounds "$run/bounds.json" --shots "$run" --name "$program-$n" --prompt "$prompt" \
      >"$run/drive.json" 2>"$run/driver.err" || true
    sleep 1
    kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
    # The debuggee, the adapter and the host the session may leave behind.
    pkill -f "$run/work" 2>/dev/null || true
    [ -n "${ELUDITE_DBG_MONO:-}" ] && { pkill -f "$ELUDITE_DBG_MONO" 2>/dev/null || true; }
    pkill -f "$host" 2>/dev/null || true
    python3 "$here/debug_agent.py" summarize --transcript "$run/transcript.json" --stream "$run/stream.jsonl" \
      --drive "$run/drive.json" --readme "$repo/corpus/debugging/README.md" --program "$program" --run "$n" \
      --out "$run/summary.json" || true
  done
done

mkdir -p "$out/report"
python3 "$here/debug_agent.py" report --runs "$out" --dest "$out/report" --date "$(date -I)" \
  --machine "$(uname -sr), $(nproc) cores, Xvfb $DISPLAY" --adapter "$adapter_name" \
  --claude "$("$claude" --version 2>/dev/null | head -1)"
ls "$out/report"
