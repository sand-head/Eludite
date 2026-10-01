#!/usr/bin/env python3
"""Record a real `claude` headless session (stream-json both ways) for a new
conformance fixture. Uses the real model and your login: keep prompts few.

  record.py CWD RAW_OUT.jsonl MCP_CONFIG.json PROMPTS_JSON

PROMPTS_JSON is a list of [text, mode]; mode "interrupt" sends an interrupt
control request 0.5 s after the first streamed text. Permission requests for
`mcp__eludite__*` tools are allowed, all others denied. Then run redact.py on
the output. The brief 0006 fixture came from:

  record.py WS raw.jsonl mcp.json '[["List the current errors in the Error List and tell me which file has the most.","normal"],["Create a file named notes.txt in the current directory containing the word hello.","normal"],["Count from 1 to 300, one number per line, with no other text.","interrupt"]]'

with mcp.json naming `cargo run -p eludite-mcp --example fixture_stdio_server`'s
binary as the stdio server "eludite".
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
args = [claude, "--print", "--input-format", "stream-json", "--output-format", "stream-json",
        "--include-partial-messages", "--replay-user-messages", "--verbose",
        "--permission-prompt-tool", "stdio", "--session-id", sid, "--permission-mode", "default",
        "--mcp-config", mcp, "--strict-mcp-config"]
t0 = time.time()
p = subprocess.Popen(args, cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
lock = threading.Lock()

def log(d, m):
    with lock:
        out.write(json.dumps({"t_ms": round((time.time() - t0) * 1000, 1), "dir": d, "m": m}) + "\n")
        out.flush()

def send(m):
    log("in", m)
    p.stdin.write((json.dumps(m) + "\n").encode())
    p.stdin.flush()

threading.Thread(target=lambda: [sys.stderr.write("STDERR " + l.decode()) for l in p.stderr], daemon=True).start()
send({"type": "control_request", "request_id": "init_1", "request": {"subtype": "initialize"}})
done = threading.Event()
state = {"mode": None, "interrupted": False}

def reader():
    for l in p.stdout:
        try:
            m = json.loads(l)
        except Exception:
            log("raw", l.decode())
            continue
        log("out", m)
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
    done.set()

threading.Thread(target=reader, daemon=True).start()
for text, mode in prompts:
    done.clear()
    state["mode"] = mode
    send({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]},
          "parent_tool_use_id": None, "session_id": sid})
    done.wait(300)
    time.sleep(1)
p.stdin.close()
p.wait(30)
log("exit", p.returncode)
