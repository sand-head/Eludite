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
     (agents-permission-prompt.png). Brief 0059, still mid-turn: click the topmost tool card drawn (an earlier call)
     so it expands, with the status line ("Claude Code is working… 0:07 · Esc to stop"), the other cards collapsed
     and the usage strip of the earlier turns on screen (agents-polish-running.png); click Deny; wait for the turn
     to end (agents-permission-denied.png).

With --polish-light (brief 0059's second screenshot, a separate run with --theme light): show the Agents window,
send POLISH_PROMPT (Claude runs `ls`, allowed), wait for the turn's end, expand the topmost tool card and take
agents-polish-light.png (the transcript after a turn, the strip with the turn's usage, in VS Light). Any theme works:
the shot's name is --polish-name.

With --openai (brief 0060, a separate run with a fresh config directory and ELUDITE_OPENAI_ACP set): show the
Agents window, open the agent picker and click its last row, "Add server…": the Add server dialog opens on the
llama.cpp preset (agents-openai-add-server.png). The base URL box gets --openai-url (default: a loopback fake
OpenAI-compatible server this script starts, two llama.cpp-shaped models, a first answer that calls
eludite-diagnostics-list and a slow streamed second answer), Test lists its models ("2 models",
agents-openai-test.png), Save adds the server and selects it; Start runs `eludite-openai-acp`, the model picker fills,
OPENAI_PROMPT is sent; mid-answer (the tool call done, the usage strip filled) agents-openai-turn.png (with
--shot-name linux-agents-openai-llama against the owner's llama-server), and after the turn agents-openai-done.png.
SHOT_X11=1 takes the screenshots with ImageMagick's `import -window root` (an Xvfb display) instead of spectacle.

With --history (brief 0061, a separate run with a fresh config directory whose agents.json names the fake agents
HISTORY_AGENTS, see agents-linux.sh): show the Agents window; pick "Fake quick" in the agent picker (a first session),
send HISTORY_PROMPTS[0] and wait for the turn's end; pick "Fake asker" (a second session, the first kept), send
HISTORY_PROMPTS[1] and leave its shell command's permission prompt waiting; pick "Fake streamer" (a third session),
send HISTORY_PROMPTS[2] and, while it streams, click the clock in the header: the history list with three sessions,
the streaming one with its dot, the waiting one with `?`, the shown one checked (agents-history.png, or --shot-name).
The rows are agents-session-<id>; the buttons agents-history and agents-new; the list agents-history-menu.

Permission requests the run does not expect (anything before the pending change that is not a shell command) are
allowed and recorded. Prints one JSON object with what the trace showed.
Usage: agents.py --title "Eludite - Eludite" --log run.err --bounds B --shots DIR --file F --shell-prompt TEXT
       [--transcript T --model opus]
       agents.py --polish-light --title T --log run.err --bounds B --shots DIR [--polish-name agents-polish-light]
       agents.py --openai --title T --log run.err --bounds B --shots DIR [--openai-url URL] [--shot-name NAME]
       agents.py --history --title T --log run.err --bounds B --shots DIR [--shot-name linux-agents-history]
"""
import argparse, json, os, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, __import__("os").path.dirname(__file__))
from intellisense import bounds, find_window, press, shot, trace, type_text, wait_trace  # noqa: E402
from navigation import click, since  # noqa: E402

PROMPT = "List the current errors and fix the first one"
# Brief 0059: one tool call (`ls`, kind execute) and a short answer, for the screenshot after a turn.
POLISH_PROMPT = "Run ls in the solution folder with the shell and tell me in one sentence how many entries it lists"
# Brief 0057: long enough to wrap to three rows in the Agents window's default width.
WRAPPED = ("Explain what HostRpcTarget does when the host receives a ping, which fields the result carries, "
           "and where the timestamp comes from")


# Brief 0061: the fake agents of the history run (agents.json's `agents`, which agents-linux.sh writes with the same
# list; the command is eludite-fake-acp-agent) and
# the three sessions' first prompts (their titles in the list).
HISTORY_AGENTS = [
    ("Fake quick", ["--scenario", "stream", "--chunks", "40", "--rate", "200"]),
    ("Fake asker", ["--scenario", "diagnostics-then-shell"]),
    ("Fake streamer", ["--scenario", "stream", "--chunks", "20000", "--rate", "20"]),
]
HISTORY_PROMPTS = [
    "Summarize the build output of the last run",
    "List the errors, then clean the obj folder",
    "Stream the release notes for version 0.6",
]


# Brief 0060: one tool call (class read, no prompt) and an answer, against the fake or a real llama-server.
OPENAI_PROMPT = "List the current errors with your tools, then say in two sentences what you found"


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


def cards(a):
    """The tool cards drawn in the last frame, top to bottom: (row, [x, y, w, h])."""
    b = bounds(a.bounds)
    found = [(int(k.rsplit("-", 1)[1]), v) for k, v in b.items()
             if k.startswith("agents-tool-") and k.rsplit("-", 1)[1].isdigit()]
    return sorted(found, key=lambda kv: kv[1][1])


def expand_card(a, out, key, skip_last=False):
    """Brief 0059: click the line of the topmost tool card drawn (not the last one with `skip_last`: the call the
    permission prompt is about) so it shows its arguments and result."""
    drawn = cards(a)
    if skip_last and len(drawn) > 1:
        drawn = drawn[:-1]
    if not drawn:
        out[key + "_error"] = "no tool card drawn"
        return
    row, r = drawn[0]
    # The card's line is its first 22 px.
    click(a.title, [r[0], r[1], r[2], 22])
    out[key + "_expanded_row"] = row
    time.sleep(1.0)


def polish_light(a):
    """Brief 0059: the transcript after a turn with a tool call, in the theme this run started with."""
    out = {}
    find_window(a.title)
    time.sleep(3.0)
    press("backslash", ["Control_L"])
    press("c", ["Control_L"])
    time.sleep(1.0)
    props = rect(a, "close-properties", timeout=5)
    if props:
        click(a.title, props)
    time.sleep(0.5)
    start = ms()
    send(a, POLISH_PROMPT)
    out["polish_turn"] = turn(a, out, "polish", start, 300, lambda tool: "allow")
    time.sleep(1.5)
    expand_card(a, out, "polish")
    out["shot_polish"] = shot(a.shots, a.polish_name)
    out["polish_trace"] = since(a.log, start)[-20:]
    print(json.dumps(out), flush=True)


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


class FakeOpenAI(BaseHTTPRequestHandler):
    """A loopback OpenAI-compatible server for the screenshots (the tests' fake is agents/openai-acp/tests/
    fake_server.rs): GET /v1/models lists two llama.cpp-shaped models; a chat request whose last message is not a
    tool result calls eludite-diagnostics-list, else the answer streams slowly (about 6 s) so a shot lands mid-turn."""
    protocol_version = "HTTP/1.1"
    MODELS = ["qwen3-8b-q4_k_m", "llama-3.1-8b-instruct-q4_k_m"]
    ANSWER = ("The Error List has no errors right now, so the workspace builds cleanly as far as the language "
              "server can tell. I called eludite-diagnostics-list once and it returned an empty list.")

    def log_message(self, *args):
        pass

    def body(self, status, data, ctype="application/json"):
        raw = data.encode()
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def chunk(self, obj):
        data = ("data: " + (obj if isinstance(obj, str) else json.dumps(obj)) + "\n\n").encode()
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def do_GET(self):
        if self.path.rstrip("/").endswith("/models"):
            data = [{"id": m, "object": "model", "owned_by": "llamacpp", "meta": {"n_ctx_train": 32768}}
                    for m in self.MODELS]
            self.body(200, json.dumps({"object": "list", "data": data}))
        else:
            self.body(404, json.dumps({"error": {"message": "not found"}}))

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        model = req.get("model", self.MODELS[0])
        last = (req.get("messages") or [{}])[-1]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

        def delta(d, finish=None):
            return {"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": model,
                    "choices": [{"index": 0, "delta": d, "finish_reason": finish}]}
        try:
            if last.get("role") != "tool":
                self.chunk(delta({"reasoning_content": "The person wants the errors: the diagnostics tool lists them."}))
                self.chunk(delta({"tool_calls": [{"index": 0, "id": "call_diag", "type": "function",
                                                  "function": {"name": "eludite-diagnostics-list", "arguments": ""}}]}))
                self.chunk(delta({"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}))
                self.chunk(delta({}, "tool_calls"))
                usage = {"prompt_tokens": 6120, "completion_tokens": 24, "total_tokens": 6144}
            else:
                for word in self.ANSWER.split(" "):
                    self.chunk(delta({"content": word + " "}))
                    time.sleep(0.2)
                self.chunk(delta({}, "stop"))
                usage = {"prompt_tokens": 6410, "completion_tokens": 52, "total_tokens": 6462}
            self.chunk({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [], "usage": usage})
            self.chunk("[DONE]")
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


def start_fake_openai():
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeOpenAI)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}/v1"


def snap(a, name):
    """A screenshot: spectacle (the nested KWin run), or with SHOT_X11=1 the X root window (an Xvfb display)."""
    if os.environ.get("SHOT_X11"):
        path = os.path.join(a.shots, name + ".png")
        subprocess.run(["import", "-window", "root", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=30)
        return path
    return shot(a.shots, name)


def openai_run(a):
    """Brief 0060: the Add server dialog, Test, Save, Start and a turn through eludite-openai-acp."""
    out = {}
    url = a.openai_url or start_fake_openai()
    out["base_url"] = url
    find_window(a.title)
    time.sleep(3.0)
    press("backslash", ["Control_L"])
    press("c", ["Control_L"])
    time.sleep(1.0)
    props = rect(a, "close-properties", timeout=5)
    if props:
        click(a.title, props)
    time.sleep(0.5)
    # The picker's last row opens the dialog, on the llama.cpp preset.
    click(a.title, rect(a, "agents-picker"))
    time.sleep(0.5)
    add = rect(a, "agents-picker-add-server", timeout=10)
    if not add:
        out["error"] = "no Add server row in the bounds file"
        print(json.dumps(out), flush=True)
        return
    click(a.title, add)
    time.sleep(1.5)
    out["shot_add"] = snap(a, "agents-openai-add-server")
    # The server's URL over the preset's, then Test.
    box = rect(a, "agents-provider-url", timeout=10)
    click(a.title, box)
    press("a", ["Control_L"])
    press("BackSpace")
    type_text(url, delay=0.02)
    t = ms()
    click(a.title, rect(a, "agents-provider-test"))
    tested = wait_trace(a.log, lambda s: s.startswith("agents provider test"), t, 60)
    out["test"] = tested and tested[1]
    time.sleep(1.0)
    out["shot_test"] = snap(a, "agents-openai-test")
    # Save: the server is in the picker, selected; Start runs the adapter and the model picker fills.
    t = ms()
    click(a.title, rect(a, "agents-provider-save"))
    registry = wait_trace(a.log, lambda s: s.startswith("agents registry") and "llama.cpp" in s, t, 30)
    out["registry"] = registry and registry[1]
    time.sleep(1.0)
    t = ms()
    click(a.title, rect(a, "agents-start"))
    options = wait_trace(a.log, lambda s: s.startswith("agents options") and "model=" in s, t, 120)
    out["options"] = options and options[1]
    start = ms()
    send(a, OPENAI_PROMPT)
    # Mid-answer: the tool call has completed and the strip shows the first response's usage.
    # Up to 8 s for the tool call (a shell without the host offers no diagnostics tool: the fake model is told so and
    # answers in text; the shot is then simply mid-answer).
    mid = wait_trace(a.log, lambda s: s.startswith("agents mcp") and "diagnostics" in s, start, 8)
    out["tool"] = mid and mid[1]
    time.sleep(a.mid_delay)
    out["shot_turn"] = snap(a, a.shot_name)
    out["turn"] = turn(a, out, "openai", start, 600, lambda tool: "allow")
    time.sleep(1.5)
    out["shot_done"] = snap(a, "agents-openai-done")
    out["trace"] = since(a.log, start)[-25:]
    print(json.dumps(out), flush=True)


def pick_agent(a, name, registry):
    """Brief 0061: pick agent `name` in the header's agent picker (with a session shown, a new session)."""
    click(a.title, rect(a, "agents-picker"))
    time.sleep(0.5)
    row = rect(a, f"agents-agent-{registry.index(name)}", timeout=10)
    t = ms()
    click(a.title, row)
    shown = wait_trace(a.log, lambda s: s.startswith("agents session shown"), t, 30)
    ready = wait_trace(a.log, lambda s: s.startswith("agents ready"), t, 60)
    return shown and shown[1], ready and ready[1]


def history_run(a):
    """Brief 0061: three sessions, one streaming and one waiting for a permission answer, and the history list."""
    out = {}
    find_window(a.title)
    t0 = ms() - 600_000
    reg = wait_trace(a.log, lambda s: s.startswith("agents registry") and "Fake streamer" in s, t0, 60)
    out["registry"] = reg and reg[1]
    if not reg:
        out["error"] = "the fake agents are not in the registry (agents.json)"
        print(json.dumps(out), flush=True)
        return
    registry = reg[1][len("agents registry "):].split(", ")
    press("backslash", ["Control_L"])
    press("c", ["Control_L"])
    time.sleep(1.0)
    props = rect(a, "close-properties", timeout=5)
    if props:
        click(a.title, props)
    time.sleep(0.5)
    # 1. A finished session.
    out["quick"] = pick_agent(a, "Fake quick", registry)
    start = ms()
    send(a, HISTORY_PROMPTS[0])
    ended = wait_trace(a.log, lambda s: s.startswith("agents turn ended"), start, 60)
    out["quick_turn"] = ended and ended[1]
    # 2. A session waiting for an answer (its shell command's permission prompt).
    out["asker"] = pick_agent(a, "Fake asker", registry)
    start = ms()
    send(a, HISTORY_PROMPTS[1])
    asked = wait_trace(a.log, lambda s: s.startswith("agents permission asked"), start, 60)
    out["asked"] = asked and asked[1]
    # 3. A streaming session, shown.
    out["streamer"] = pick_agent(a, "Fake streamer", registry)
    start = ms()
    send(a, HISTORY_PROMPTS[2])
    time.sleep(3.0)
    click(a.title, rect(a, "agents-history"))
    time.sleep(1.0)
    out["menu"] = rect(a, "agents-history-menu", timeout=5)
    out["rows"] = sorted(k for k in bounds(a.bounds) if k.startswith("agents-session-"))
    out["shot"] = snap(a, a.shot_name if a.shot_name != "agents-openai-turn" else "agents-history")
    out["trace"] = since(a.log, start)[-15:]
    print(json.dumps(out), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--shots", required=True)
    ap.add_argument("--polish-light", action="store_true", help="brief 0059's screenshot after a turn, then exit")
    ap.add_argument("--polish-name", default="agents-polish-light")
    ap.add_argument("--file")
    ap.add_argument("--shell-prompt")
    ap.add_argument("--transcript", help="--transcript-out's file, to read the usage line after the model pick")
    ap.add_argument("--model", default="opus", help="the model picker's value to pick (brief 0058)")
    ap.add_argument("--dry", action="store_true", help="a scripted agent: skip waiting for the error to clear and the shell step")
    ap.add_argument("--openai", action="store_true", help="brief 0060's run: Add server, Test, Save, Start, a turn")
    ap.add_argument("--openai-url", help="the server's base URL (default: a loopback fake this script starts)")
    ap.add_argument("--shot-name", default="agents-openai-turn", help="the mid-turn shot's name")
    ap.add_argument("--mid-delay", type=float, default=2.0, help="seconds after the tool call before the mid-turn shot")
    ap.add_argument("--history", action="store_true", help="brief 0061's run: three sessions and the history list")
    a = ap.parse_args()
    if a.history:
        history_run(a)
        return
    if a.openai:
        openai_run(a)
        return
    if a.polish_light:
        polish_light(a)
        return
    if not a.file or not a.shell_prompt:
        ap.error("--file and --shell-prompt are required without --polish-light")
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
        # Brief 0059: mid-turn, an earlier card expanded, the status line and the strip.
        expand_card(a, out, "running", skip_last=True)
        out["shot_polish_running"] = shot(a.shots, "agents-polish-running")
    ended = turn(a, out, "shell", start, 300, lambda tool: "deny")
    out["shell_turn"] = ended
    time.sleep(1.5)
    out["shot_denied"] = shot(a.shots, "agents-permission-denied")
    out["shell_trace"] = since(a.log, start)[-25:]
    print(json.dumps(out), flush=True)


if __name__ == "__main__":
    main()
