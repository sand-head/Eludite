"""A minimal DAP client over TCP for eludite-dbg-netfx: attach, breakpoint, stop, stack, locals, continue, detach."""
import json, socket, sys, time

class Dap:
    def __init__(self, host, port):
        self.s = socket.create_connection((host, port), timeout=60)
        self.buf = b""; self.seq = 0; self.events = []
    def _read(self):
        while b"\r\n\r\n" not in self.buf:
            d = self.s.recv(65536)
            if not d: raise EOFError("adapter closed")
            self.buf += d
        head, rest = self.buf.split(b"\r\n\r\n", 1)
        n = int([l for l in head.split(b"\r\n") if l.lower().startswith(b"content-length")][0].split(b":")[1])
        while len(rest) < n:
            d = self.s.recv(65536)
            if not d: raise EOFError("adapter closed")
            rest += d
        self.buf = rest[n:]
        return json.loads(rest[:n])
    def request(self, command, arguments=None):
        self.seq += 1
        body = json.dumps({"seq": self.seq, "type": "request", "command": command, "arguments": arguments or {}}).encode()
        self.s.sendall(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        t0 = time.perf_counter()
        while True:
            m = self._read()
            if m.get("type") == "response" and m.get("request_seq") == self.seq:
                ms = (time.perf_counter() - t0) * 1000
                print(f"<- {command}: success={m.get('success')} {ms:.1f} ms body={json.dumps(m.get('body'))[:300]} {m.get('message') or ''}")
                return m
            self.events.append(m); print("   event:", json.dumps(m)[:300])
    def wait_event(self, name, timeout=30):
        for i, e in enumerate(self.events):
            if e.get("event") == name: return self.events.pop(i)
        self.s.settimeout(timeout)
        t0 = time.perf_counter()
        while True:
            m = self._read()
            if m.get("type") == "event" and m.get("event") == name:
                print(f"<- event {name} after {(time.perf_counter()-t0)*1000:.1f} ms: {json.dumps(m.get('body'))[:300]}")
                return m
            self.events.append(m); print("   event:", json.dumps(m)[:300])

host, port, pid, source, line = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], int(sys.argv[5])
d = Dap(host, port)
d.request("initialize", {"clientID": "spike", "adapterID": "netfx", "linesStartAt1": True, "columnsStartAt1": True, "pathFormat": "path"})
t = time.perf_counter()
d.request("attach", {"processId": pid})
d.wait_event("initialized")
print(f"RESULT attach->initialized {(time.perf_counter()-t)*1000:.1f} ms")
d.request("setBreakpoints", {"source": {"path": source}, "breakpoints": [{"line": line}]})
d.request("configurationDone")
for n in range(2):
    st = d.wait_event("stopped")
    tid = st["body"]["threadId"]
    d.request("threads")
    tr = d.request("stackTrace", {"threadId": tid, "startFrame": 0, "levels": 20})
    frames = tr["body"]["stackFrames"]
    for f in frames[:4]: print("   frame:", f.get("name"), f.get("line"), (f.get("source") or {}).get("path"))
    sc = d.request("scopes", {"frameId": frames[0]["id"]})
    for s in sc["body"]["scopes"]:
        v = d.request("variables", {"variablesReference": s["variablesReference"]})
        print(f"RESULT stop {n}: locals", [(x["name"], x["value"]) for x in v["body"]["variables"]])
    d.request("continue", {"threadId": tid})
d.request("disconnect", {})
print("RESULT done")
