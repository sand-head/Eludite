#!/usr/bin/env python3
"""Redact a recorded `claude` stream-json session (from record.py) into a
fixture for the fake `claude` (src/fake_claude.rs).

  redact.py RAW.jsonl CWD OUT.jsonl

- The session id becomes {{SESSION_ID}} and the working directory {{CWD}};
  the fake substitutes its own at replay time.
- Message, request, tool-use and event ids become stable placeholders.
- Thinking signatures, the account in the initialize reply, rate-limit
  figures, memory paths, sockets, and the user's own skills, plugins, agents
  and commands are removed; Claude Code's built-in slash commands
  (`builtin: true` in the initialize reply) and its model list (`models`,
  brief 0058) are kept.
- Fails if the home directory or the user name is still present.
"""
import json, os, re, sys

raw, cwd, out = sys.argv[1], os.path.realpath(sys.argv[2]), sys.argv[3]
home = os.path.expanduser("~")
user = os.path.basename(home)
ids = {}

def ph(kind, value):
    key = (kind, value)
    if key not in ids:
        n = sum(1 for k in ids if k[0] == kind) + 1
        ids[key] = (f"00000000-0000-4000-8000-{n:012d}" if kind == "uuid" else f"{kind}_redacted_{n:02d}")
    return ids[key]

UUID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
session_ids = set()

def scrub_str(s):
    s = s.replace(cwd, "{{CWD}}")
    for sid in session_ids:
        s = s.replace(sid, "{{SESSION_ID}}")
    s = re.sub(r"\b(msg|req|toolu)_0[0-9A-Za-z]{10,}", lambda m: ph(m.group(1), m.group(0)), s)
    return s

def scrub(v, key=None):
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in ("signature",):
                out[k] = "REDACTED" if x else x
            elif k == "uuid" and isinstance(x, str):
                out[k] = ph("uuid", x)
            elif k in ("memory_paths", "skills", "plugins", "agents", "slash_commands", "terminal_slash_commands"):
                out[k] = []
            elif k == "messaging_socket_path":
                out[k] = None
            else:
                out[k] = scrub(x, k)
        return out
    if isinstance(v, list):
        return [scrub(x, key) for x in v]
    if isinstance(v, str):
        if key == "session_id" or v in session_ids:
            return "{{SESSION_ID}}"
        return scrub_str(v)
    return v

records = [json.loads(l) for l in open(raw)]
for r in records:
    m = r.get("m")
    if isinstance(m, dict) and isinstance(m.get("session_id"), str) and UUID.match(m["session_id"]):
        session_ids.add(m["session_id"])

with open(out, "w") as f:
    for r in records:
        m = r["m"]
        if r["dir"] == "exit":
            f.write(json.dumps({"t_ms": r["t_ms"], "dir": "exit", "code": m}) + "\n")
            continue
        if not isinstance(m, dict):
            continue
        t = m.get("type")
        if t == "control_response" and r["dir"] == "out":
            resp = m.get("response", {}).get("response")
            if isinstance(resp, dict) and "account" in resp:
                acct = resp["account"]
                logged_in = acct.get("tokenSource") != "none"
                m["response"]["response"] = {
                    "commands": [c for c in resp.get("commands", []) if c.get("builtin") is True],
                    "agents": [], "output_style": "default", "models": resp.get("models", []),
                    "account": ({"subscriptionType": "redacted", "apiProvider": "firstParty"} if logged_in
                                else {"tokenSource": "none", "apiProvider": "firstParty"}),
                    "pid": 0, "current_permission_mode": "default", "session_state": "idle",
                }
        if t == "rate_limit_event":
            m["rate_limit_info"] = {"status": "allowed", "rateLimitType": "five_hour", "resetsAt": 0}
        m = scrub(m)
        f.write(json.dumps({"t_ms": r["t_ms"], "dir": r["dir"], "m": m}) + "\n")

text = open(out).read()
for needle in (home, user, cwd, "/tmp/claude-"):
    if needle in text:
        sys.exit(f"redaction left {needle!r} in {out}")
print(f"wrote {out}: {len(records)} records, {len(ids)} ids replaced")
