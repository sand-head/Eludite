#!/usr/bin/env python3
"""Brief 0030's recorded real run: Claude Code debugs a seeded-bug corpus program through the Agents window.

Three subcommands, run by tools/debug-agent-linux.sh:

  drive      Drive `eludite --solution S --agent "Claude Code" --bounds-out B --transcript-out T` (ELUDITE_TRACE_LSP=1,
             stderr in --log) on an X11 display with xdotool: wait for the window and the solution, show the Agents
             window (Ctrl+\\, Ctrl+C), click the prompt box, type the prompt, press Enter, answer permission prompts
             (Bash is denied: the prompt asks for Eludite's tools; anything else is allowed and recorded), wait for the
             turn's end, and take screenshots (`import -window root`). Prints one JSON object: the times (ms since the
             epoch) of the prompt and the turn's end, the permission decisions, the screenshots.
  summarize  One run's numbers from its transcript (the Agents window's rows, `--transcript-out`), the Claude Code
             stream captured below the ACP adapter (`--stream`, each line `<ms> <json>`) and the drive JSON: every
             tool call (name, input, answer bytes, time), the debug calls, whether a summary the agent received
             stopped at the README's faulting line, whether the answer names that line, tokens and wall time.
  report     All runs' summaries into numbers.md, calls.json and transcript.md (the full transcripts as the window
             showed them).

No Python packages beyond the standard library; xdotool and ImageMagick's `import` on PATH.
"""
import argparse, glob, json, os, re, subprocess, sys, time

LINE = re.compile(r"^\[lsp\] (\d+) (.*)$")


def ms():
    return int(time.time() * 1000)


def trace(log):
    try:
        with open(log, errors="replace") as f:
            for line in f:
                m = LINE.match(line.strip())
                if m:
                    yield int(m.group(1)), m.group(2)
    except OSError:
        return


def since(log, start):
    return [(t, s) for t, s in trace(log) if t >= start]


def wait_trace(log, pred, after_ms, timeout):
    end = time.time() + timeout
    while time.time() < end:
        for t, text in trace(log):
            if t >= after_ms and pred(text):
                return t, text
        time.sleep(0.05)
    return None


def xdo(*args):
    return subprocess.run(["xdotool", *args], capture_output=True, text=True).stdout.strip()


def find_window(title, timeout=120):
    end = time.time() + timeout
    while time.time() < end:
        ids = xdo("search", "--onlyvisible", "--name", "^" + re.escape(title) + "$").split()
        if ids:
            return ids[-1]
        time.sleep(0.3)
    raise RuntimeError(f"no window titled {title!r}")


def origin(win):
    geo = dict(l.split("=", 1) for l in xdo("getwindowgeometry", "--shell", win).splitlines() if "=" in l)
    return int(geo.get("X", 0)), int(geo.get("Y", 0))


def bounds(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def rect(path, key, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        b = bounds(path).get(key)
        if b:
            return b
        time.sleep(0.2)
    return None


def click(win, r):
    ox, oy = origin(win)
    x, y = int(ox + r[0] + r[2] / 2), int(oy + r[1] + r[3] / 2)
    xdo("mousemove", "--sync", str(x), str(y))
    time.sleep(0.1)
    xdo("click", "1")
    time.sleep(0.3)


def shot(dir_, name):
    path = os.path.join(dir_, name + ".png")
    subprocess.run(["import", "-window", "root", path], capture_output=True, timeout=60)
    return path


def drive(a):
    out = {"permissions": [], "shots": []}
    win = find_window(a.title)
    t0 = ms() - 600_000
    loaded = wait_trace(a.log, lambda s: s.startswith("solution Loaded") or s.startswith("solution Failed"), t0, 240)
    out["solution"] = loaded and loaded[1]
    time.sleep(1.0)
    xdo("windowactivate", "--sync", win)
    xdo("windowfocus", "--sync", win)
    # View > Agents with its key; close Properties so the Agents window has the right dock's height.
    xdo("key", "--window", win, "ctrl+backslash", "ctrl+c")
    time.sleep(1.0)
    props = rect(a.bounds, "close-properties", timeout=5)
    if props:
        click(win, props)
    box = rect(a.bounds, "agents-prompt")
    if not box:
        out["error"] = "no prompt box in the bounds file"
        print(json.dumps(out), flush=True)
        return
    click(win, box)
    typed = ms()
    xdo("type", "--delay", "8", a.prompt)
    time.sleep(0.3)
    xdo("key", "Return")
    # On software rendering the window works through the typed keys after xdotool has sent them: the prompt is sent
    # when the window handles Enter, which starts the agent ("agents spawned"). The wall time starts there.
    sent = wait_trace(a.log, lambda s: s.startswith("agents spawned") or s.startswith("agents ready"), typed, 300)
    start = sent[0] if sent else ms()
    out["prompt_sent_ms"] = start
    out["typing_backlog_ms"] = start - typed
    time.sleep(3.0)
    out["shots"].append(shot(a.shots, a.name + "-started"))
    end = time.time() + a.timeout
    seen = 0
    ended = None
    while time.time() < end and not ended:
        lines = [s for _, s in since(a.log, start)]
        asked = [l for l in lines if l.startswith("agents permission asked")]
        for l in asked[seen:]:
            seen += 1
            tool = l[len("agents permission asked "):]
            decision = "deny" if tool.startswith("Bash") else "allow"
            out["permissions"].append([tool, decision])
            b = rect(a.bounds, f"agents-permission-{decision}", timeout=10)
            if b:
                click(win, b)
        done = [(t, s) for t, s in since(a.log, start) if s.startswith("agents turn ended")]
        if done:
            ended = done[-1]
        time.sleep(0.2)
    out["turn_ended"] = ended and ended[1]
    out["turn_ended_ms"] = ended and ended[0]
    out["wall_ms"] = ended and ended[0] - start
    time.sleep(2.0)
    out["shots"].append(shot(a.shots, a.name + "-end"))
    print(json.dumps(out), flush=True)


def readme_expected(readme, program):
    for line in open(readme):
        if line.startswith(f"| `{program}` |"):
            cells = line.split(" | ")
            q = lambda c: c.split("`")[1::2]
            return {"statement": q(cells[3])[0], "line": int(q(cells[4])[0].rsplit(":", 1)[1]),
                    "locals": [p[0] for p in zip(q(cells[5])[0::2], q(cells[5])[1::2])]}
    raise SystemExit(f"no {program} row in {readme}")


def stream_events(path):
    """The Claude Code stream (`<ms> <json>` per line)."""
    events = []
    try:
        with open(path, errors="replace") as f:
            for line in f:
                t, _, js = line.partition(" ")
                try:
                    events.append((int(t), json.loads(js)))
                except ValueError:
                    pass
    except OSError:
        pass
    return events


def summarize(a):
    rows = json.load(open(a.transcript))
    drive_out = json.load(open(a.drive)) if a.drive and os.path.exists(a.drive) else {}
    expected = readme_expected(a.readme, a.program)
    events = stream_events(a.stream)
    # Tool use and result times from the stream: the tool_use's message time to its tool_result's.
    used, results = {}, {}
    usage, result, model = None, None, None
    for t, e in events:
        if e.get("type") == "system" and e.get("subtype") == "init":
            model = e.get("model")
        if e.get("type") == "assistant":
            for b in e.get("message", {}).get("content", []) or []:
                if b.get("type") == "tool_use":
                    used.setdefault(b["id"], t)
        elif e.get("type") == "user":
            content = e.get("message", {}).get("content", [])
            for b in content if isinstance(content, list) else []:
                if b.get("type") == "tool_result":
                    results.setdefault(b.get("tool_use_id"), t)
        elif e.get("type") == "result":
            result = e
            usage = e.get("usage")
    calls = []
    for r in rows:
        tc = r.get("tool_call")
        if not tc:
            continue
        text = tc.get("result") or ""
        t_use, t_res = used.get(tc["id"]), results.get(tc["id"])
        calls.append({
            "id": tc["id"], "name": tc["tool"], "status": tc["status"], "input": tc.get("arguments"),
            "output_bytes": len(text.encode()), "ms": (t_res - t_use) if t_use and t_res else None,
            "debug": tc.get("debug"), "debug_location": tc.get("debug_location"),
        })
    debug = [c for c in calls if "eludite-debug-" in c["name"]]
    # Reached: a debug answer the agent received stopped at the faulting line of the program's Program.cs.
    reached_at = None
    for n, c in enumerate(debug, 1):
        loc = c.get("debug_location") or {}
        if loc.get("line") == expected["line"] and str(loc.get("path", "")).endswith(f"{a.program}/Program.cs"):
            reached_at = n
            break
        try:
            s = json.loads(next(r["tool_call"]["result"] for r in rows if r.get("tool_call", {}).get("id") == c["id"]))
        except (StopIteration, ValueError, TypeError):
            continue
        loc = (s.get("stopped") or {}).get("location") or {}
        if s.get("mode") == "break" and loc.get("line") == expected["line"] and \
                str(loc.get("path", "")).endswith(f"{a.program}/Program.cs"):
            reached_at = n
            break
    answer = "\n".join(r["agent"] for r in rows if "agent" in r)
    final = answer.rsplit("\n\n", 1)[-1] if answer else ""
    # Named: the answer gives the line and the statement (whitespace aside, or its telling part).
    squash = lambda s: re.sub(r"\s+", "", s).rstrip(";")
    telling = {"OffByOne": "count - 1", "MissingCase": "return 0", "NullField": "parent != null"}.get(a.program, "")
    named = bool(re.search(rf"(line\s*{expected['line']}\b|:{expected['line']}\b)", answer, re.I)) and (
        squash(expected["statement"]) in squash(answer) or (telling and squash(telling) in squash(answer)))
    tokens = None
    if usage:
        tokens = {
            "input": usage.get("input_tokens", 0),
            "cache_creation_input": usage.get("cache_creation_input_tokens", 0),
            "cache_read_input": usage.get("cache_read_input_tokens", 0),
            "output": usage.get("output_tokens", 0),
        }
        tokens["input_total"] = tokens["input"] + tokens["cache_creation_input"] + tokens["cache_read_input"]
    out = {
        "program": a.program, "run": a.run, "expected": expected,
        "calls": calls,
        "debug_calls": len(debug), "debug_call_names": [c["name"].rsplit("eludite-debug-", 1)[-1] for c in debug],
        "reached": reached_at is not None, "reached_at_debug_call": reached_at,
        "within_eight": reached_at is not None and reached_at <= 8,
        "named_in_answer": named,
        "tokens": tokens, "acp_usage_events": 0, "model": model,
        "cost_usd": result and result.get("total_cost_usd"),
        "num_turns": result and result.get("num_turns"),
        "claude_duration_ms": result and result.get("duration_ms"),
        "transcript_bytes": len(json.dumps(rows).encode()),
        "wall_ms": drive_out.get("wall_ms"),
        "permissions": drive_out.get("permissions"),
        "turn_ended": drive_out.get("turn_ended"),
        "answer": final,
    }
    json.dump(out, open(a.out, "w"), indent=1)
    print(json.dumps({k: out[k] for k in ("program", "run", "debug_calls", "reached", "reached_at_debug_call",
                                           "named_in_answer", "tokens", "wall_ms")}))


def transcript_md(rows):
    lines = []
    for r in rows:
        if "user" in r:
            lines.append("**You:** " + r["user"])
        elif "agent" in r:
            lines.append(r["agent"])
        elif "thought" in r:
            lines.append("> *Thinking:* " + r["thought"].replace("\n", "\n> "))
        elif "tool_call" in r:
            t = r["tool_call"]
            head = f"**Tool** `{t['tool']}` ({t['status']})"
            if t.get("debug"):
                head += f": {t['debug']}"
            lines.append(head)
            lines.append("```json\n" + json.dumps(t.get("arguments"), indent=1) + "\n```")
            res = t.get("result") or ""
            if res:
                lines.append(f"<details><summary>Result ({len(res.encode())} bytes)</summary>\n\n```\n{res}\n```\n\n</details>")
        elif "notice" in r:
            lines.append("*" + r["notice"] + "*")
        elif "error" in r:
            lines.append("**Error:** " + r["error"])
        elif "plan" in r:
            lines.append("Plan: " + "; ".join(e.get("content", "") for e in r["plan"]))
    return "\n\n".join(lines)


def report(a):
    runs = [json.load(open(p)) for p in sorted(glob.glob(os.path.join(a.runs, "*", "summary.json")))]
    order = {"OffByOne": 0, "MissingCase": 1, "NullField": 2}
    runs.sort(key=lambda r: (order.get(r["program"], 9), r["run"]))
    calls = [{"program": r["program"], "run": r["run"], "calls": r["calls"]} for r in runs]
    short = lambda text: text.replace(os.path.realpath(a.runs) + "/", "$OUT/")
    open(os.path.join(a.dest, "calls.json"), "w").write(short(json.dumps(calls, indent=1)))
    md = ["# Brief 0030's recorded run: the numbers", "",
          f"Recorded {a.date} on {a.machine}. Adapter: {a.adapter}. Claude Code {a.claude}, model "
          f"{', '.join(sorted({str(r.get('model')) for r in runs}))}. "
          "Tokens are Claude Code's own count for the turn (its stream's `result` message, captured below the ACP "
          "adapter; `eludite-claude-acp` sends no ACP usage events). Wall time: from the window taking the prompt (the "
          "agent's start, about 0.5 s of it) to the turn's end. Debug calls: every `eludite.debug.*` call of the turn, "
          "cleanup included. Reached: a stop summary the agent received (a start, wait, continue, step or trace answer) "
          "located at the README's faulting line, and the debug call that brought it. Named: the answer gives that "
          "line and the statement. Bytes: each debug answer's text as the agent received it. Paths under the run "
          "folder are written `$OUT/`.", "",
          "| Program | Run | Debug calls | Debug calls, in order | Reached (at debug call) | Named in the answer | "
          "Input tokens (cache read, cache write) | Output tokens | Wall time | Debug answer bytes |",
          "|---|---|---|---|---|---|---|---|---|---|"]
    for r in runs:
        t = r["tokens"] or {}
        sizes = ", ".join(str(c["output_bytes"]) for c in r["calls"] if "eludite-debug-" in c["name"])
        md.append(
            f"| {r['program']} | {r['run']} | {r['debug_calls']} | {', '.join(r['debug_call_names'])} | "
            f"{'yes (' + str(r['reached_at_debug_call']) + ')' if r['reached'] else 'no'} | "
            f"{'yes' if r['named_in_answer'] else 'no'} | "
            f"{t.get('input_total', '?')} ({t.get('cache_read_input', '?')}, {t.get('cache_creation_input', '?')}) | "
            f"{t.get('output', '?')} | {(r['wall_ms'] or 0) / 1000:.1f} s | {sizes} |")
    hit = sum(1 for r in runs if r["within_eight"])
    md += ["", f"Runs that reached the faulting statement within eight debug calls: **{hit} of {len(runs)}**.", ""]
    md += ["## Tool calls per run", ""]
    for r in runs:
        md.append(f"### {r['program']}, run {r['run']}")
        md.append("")
        md.append("| # | Tool | Answer bytes | Time |")
        md.append("|---|---|---|---|")
        for n, c in enumerate(r["calls"], 1):
            md.append(f"| {n} | `{c['name']}` | {c['output_bytes']} | "
                      f"{str(c['ms']) + ' ms' if c['ms'] is not None else ''} |")
        md.append("")
        md.append("Answer: " + (r["answer"] or "").replace("\n", " ")[:2000])
        md.append("")
    open(os.path.join(a.dest, "numbers.md"), "w").write(short("\n".join(md) + "\n"))
    tm = ["# Brief 0030's recorded run: the transcripts", "",
          "Each run's Agents window transcript (`--transcript-out`), as the window showed it: the prompt, Claude's "
          "messages, and each tool call with its arguments and the answer it received (folded).", ""]
    for r in runs:
        rows = json.load(open(os.path.join(a.runs, f"{r['program']}-{r['run']}", "transcript.json")))
        tm += [f"## {r['program']}, run {r['run']}", "", transcript_md(rows), ""]
    open(os.path.join(a.dest, "transcript.md"), "w").write(short("\n".join(tm) + "\n"))
    print(f"{hit} of {len(runs)} within eight debug calls")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("drive")
    d.add_argument("--title", required=True)
    d.add_argument("--log", required=True)
    d.add_argument("--bounds", required=True)
    d.add_argument("--shots", required=True)
    d.add_argument("--name", required=True)
    d.add_argument("--prompt", required=True)
    d.add_argument("--timeout", type=int, default=900)
    s = sub.add_parser("summarize")
    for k in ("transcript", "stream", "drive", "readme", "program", "out"):
        s.add_argument("--" + k, required=k not in ("drive", "stream"))
    s.add_argument("--run", type=int, required=True)
    r = sub.add_parser("report")
    for k in ("runs", "dest", "date", "machine", "adapter", "claude"):
        r.add_argument("--" + k, required=True)
    a = ap.parse_args()
    {"drive": drive, "summarize": summarize, "report": report}[a.cmd](a)


if __name__ == "__main__":
    main()
