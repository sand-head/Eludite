#!/usr/bin/env python3
"""Record a real `claude` headless session (stream-json both ways) for a new
conformance fixture. Uses the real model and your login: keep prompts few.

  record.py CWD RAW_OUT.jsonl MCP_CONFIG.json PROMPTS_JSON

PROMPTS_JSON is a list of [text, mode]; mode "interrupt" sends an interrupt
control request 0.5 s after the first streamed text; mode "control" sends
text (a JSON object such as {"subtype": "set_model", "model": "opus"}) as a
control request instead of a user message and waits for its reply, so a
recording can hold no model call at all. Permission requests for
`mcp__eludite__*` tools are allowed, all others denied. Then run redact.py on
the output. The brief 0006 fixture came from:

  record.py WS raw.jsonl mcp.json '[["List the current errors in the Error List and tell me which file has the most.","normal"],["Create a file named notes.txt in the current directory containing the word hello.","normal"],["Count from 1 to 300, one number per line, with no other text.","interrupt"]]'

with mcp.json naming `cargo run -p eludite-mcp --example fixture_stdio_server`'s
binary as the stdio server "eludite". The brief 0058 fixture (no model call:
two permission mode changes, a model change and the local command /effort)
came from:

  record.py WS raw.jsonl empty-mcp.json '[["{\"subtype\": \"set_permission_mode\", \"mode\": \"plan\"}","control"],["{\"subtype\": \"set_permission_mode\", \"mode\": \"default\"}","control"],["{\"subtype\": \"set_model\", \"model\": \"opus\"}","control"],["/effort high","normal"]]'

with empty-mcp.json holding {"mcpServers": {}}.

Mode "resume" (brief 0061) ends the running `claude` (stdin closed, its exit
recorded) and starts another with `--resume SESSION_ID` instead of
`--session-id`, which gets its own `initialize`; mode "resume-unknown" does the
same with an id no session has, to record the refusal. Each process starts with
a {"dir": "start", "m": {"flag", "label"}} record (label `new`, `resume` or
`refused`), and its stderr lines are kept as {"dir": "err", "m": {"line"}}. A
session that only ran `initialize` has nothing to resume (`claude` 2.1.289 says
"No conversation found with session ID"), so the brief 0061 fixture runs the
local command /effort first, which writes the session file with no model call:

  record.py WS raw.jsonl empty-mcp.json '[["/effort low","normal"],["","resume"],["","resume-unknown"]]'
"""
import json, os, shutil, subprocess, sys, threading, time, uuid

env = dict(os.environ)
for k in list(env):
    if k.startswith("CLAUDE_CODE_") or k in ("CLAUDECODE", "CLAUDE_PID", "CLAUDE_EFFORT"):
        del env[k]
cwd, out_path, mcp, prompts = sys.argv[1], sys.argv[2], os.path.abspath(sys.argv[3]), json.loads(sys.argv[4])
out = open(out_path, "w")
sid = str(uuid.uuid4())
claude = shutil.which("claude") or os.path.expanduser("~/.local/bin/claude")
def launch(flag, session):
    return [claude, "--print", "--input-format", "stream-json", "--output-format", "stream-json",
            "--include-partial-messages", "--replay-user-messages", "--verbose",
            "--permission-prompt-tool", "stdio", flag, session, "--permission-mode", "default",
            "--mcp-config", mcp, "--strict-mcp-config"]
t0 = time.time()
lock = threading.Lock()

def log(d, m):
    with lock:
        out.write(json.dumps({"t_ms": round((time.time() - t0) * 1000, 1), "dir": d, "m": m}) + "\n")
        out.flush()

def send(m):
    log("in", m)
    try:
        p.stdin.write((json.dumps(m) + "\n").encode())
        p.stdin.flush()
    except BrokenPipeError:
        pass

done = threading.Event()
inited = threading.Event()
state = {"mode": None, "interrupted": False}

def errors(proc):
    for l in proc.stderr:
        sys.stderr.write("STDERR " + l.decode())
        log("err", {"line": l.decode().rstrip("\n")})

def start(flag, session, label, n):
    global p
    log("start", {"flag": flag, "label": label})
    p = subprocess.Popen(launch(flag, session), cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env)
    threading.Thread(target=errors, args=(p,), daemon=True).start()
    inited.clear()
    threading.Thread(target=reader, daemon=True).start()
    send({"type": "control_request", "request_id": f"init_{n}", "request": {"subtype": "initialize"}})

def stop():
    try:
        p.stdin.close()
    except BrokenPipeError:
        pass
    p.wait(30)
    time.sleep(0.5)
    log("exit", p.returncode)

def reader():
    p = globals()["p"]
    for l in p.stdout:
        try:
            m = json.loads(l)
        except Exception:
            log("raw", l.decode())
            continue
        log("out", m)
        if m.get("type") == "control_response" and str(m["response"].get("request_id", "")).startswith("init_"):
            inited.set()
        if m.get("type") == "control_request" and m["request"].get("subtype") == "can_use_tool":
            r = m["request"]
            if r.get("tool_name", "").startswith("mcp__eludite__"):
                resp = {"behavior": "allow", "updatedInput": r.get("input", {})}
            else:
                resp = {"behavior": "deny", "message": "The user denied this tool call."}
            send({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": resp}})
        ev = m.get("event", {}) if m.get("type") == "stream_event" else {}
        if (state["mode"] == "interrupt" and not state["interrupted"] and ev.get("type") == "content_block_delta"
                and ev["delta"].get("type") == "text_delta"):
            state["interrupted"] = True
            time.sleep(0.5)
            send({"type": "control_request", "request_id": "int_1", "request": {"subtype": "interrupt"}})
        if m.get("type") == "result":
            done.set()
        if (state["mode"] == "control" and m.get("type") == "control_response"
                and m["response"].get("request_id") == state.get("control_id")):
            done.set()
    done.set()
    inited.set()

start("--session-id", sid, "new", 1)
for n, (text, mode) in enumerate(prompts):
    done.clear()
    state["mode"] = mode
    if mode in ("resume", "resume-unknown"):
        stop()
        unknown = mode == "resume-unknown"
        start("--resume", str(uuid.uuid4()) if unknown else sid, "refused" if unknown else "resume", n + 2)
        inited.wait(60)
        time.sleep(1)
        continue
    if mode == "control":
        state["control_id"] = f"ctl_{n + 1}"
        send({"type": "control_request", "request_id": state["control_id"], "request": json.loads(text)})
        done.wait(60)
        time.sleep(1)
        continue
    send({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]},
          "parent_tool_use_id": None, "session_id": sid})
    done.wait(300)
    time.sleep(1)
stop()
