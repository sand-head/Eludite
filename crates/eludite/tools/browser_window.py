#!/usr/bin/env python3
"""Brief 0032's Xvfb run of the Web Browser window (see browser-window-linux.sh): drives `eludite --bounds-out B`
with real X input through xdotool and takes screenshots with ImageMagick's `import`.

  1. View > Other Windows > Web Browser (the menu, clicked): the window opens with a tab (engine cold); the shell logs
     the window open to the first page pixel.
  2. Ctrl+L, the order form's url, Enter; a click in the page and Tab to its Name field; a name typed by hand
     (browser-window-page.png).
  3. F12: DevTools as a tab beside the page (browser-window-devtools.png); Ctrl+W closes it.
  4. A page whose button asks confirm(): clicked; the shell's dialog (browser-window-dialog.png); Enter accepts.
  5. Ctrl+\\, Ctrl+C: the Agents window; the prompt sent to the fake agent "Form Filler", which opens the form in a
     new tab, reads it, fills it, submits it and waits for the result (the fixture answers after 4 s): the "Agent is
     driving" strip while it waits (browser-window-agent-driving.png), then the agent's answer.
  6. The window's document tab closed and View > Other Windows > Web Browser again within the minute the engine
     lingers: window open to the first page pixel with the engine running.

Prints one JSON object: the steps seen in the shell's log, the timings and the shell's resident memory.
Usage: browser_window.py --pid PID --log eludite.log --bounds bounds.json --shots DIR --port PORT --fixtures DIR
"""
import argparse, functools, http.server, json, os, re, subprocess, threading, time

ASK = b"""<!doctype html><html><head><title>Ask</title></head><body style="margin:0;font:16px sans-serif">
<button id="b" style="width:320px;height:120px;font-size:18px"
 onclick="document.title = confirm('Delete the order?') ? 'Deleted' : 'Kept'">Delete the order</button>
</body></html>"""


def serve(port, fixtures):
    form = open(os.path.join(fixtures, "act.html"), "rb").read()
    # The result appears 4 s after Submit, so the agent's wait is in flight long enough to see the strip.
    form = form.replace(b"}, 150);", b"}, 4000);")
    pages = {"/form.html": form, "/ask.html": ASK}

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = pages.get(self.path.split("?")[0])
            self.send_response(200 if body else 404)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body or b"")))
            self.end_headers()
            self.wfile.write(body or b"")

        def log_message(self, *a):
            pass

    srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def xdo(*args):
    return subprocess.run(["xdotool", *map(str, args)], capture_output=True, text=True).stdout.strip()


def key(*names):
    for n in names:
        xdo("key", "--clearmodifiers", n)
        time.sleep(0.15)


def type_text(text):
    xdo("type", "--delay", 40, text)


def window_origin():
    wid = None
    end = time.time() + 60
    while time.time() < end and not wid:
        wid = xdo("search", "--onlyvisible", "--name", "^Eludite$").split("\n")[0]
        time.sleep(0.2)
    if not wid:
        raise RuntimeError("no Eludite window")
    geo = dict(l.split("=", 1) for l in xdo("getwindowgeometry", "--shell", wid).splitlines() if "=" in l)
    return int(geo["X"]), int(geo["Y"])


def bounds(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def rect(a, k, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        b = bounds(a.bounds).get(k)
        if b:
            return b
        time.sleep(0.2)
    return None


def click_at(origin, x, y, button=1):
    xdo("mousemove", origin[0] + int(x), origin[1] + int(y))
    time.sleep(0.1)
    xdo("click", button)
    time.sleep(0.3)


def click(origin, r):
    click_at(origin, r[0] + r[2] / 2, r[1] + r[3] / 2)


def shot(a, name):
    path = os.path.join(a.shots, name + ".png")
    subprocess.run(["import", "-window", "root", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   timeout=30)
    return path


def lines(a):
    try:
        with open(a.log, errors="replace") as f:
            return [l.rstrip("\n") for l in f if l.startswith("eludite: web browser: ")]
    except OSError:
        return []


def wait_line(a, pred, after, timeout):
    """The first line of the window's log past index `after` that `pred` accepts: (index, line) or None."""
    end = time.time() + timeout
    while time.time() < end:
        ls = lines(a)
        for i, l in enumerate(ls[after:], after):
            if pred(l[len("eludite: web browser: "):]):
                return i, l
        time.sleep(0.05)
    return None


def rss_mib(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for l in f:
                if l.startswith("VmRSS:"):
                    return round(int(l.split()[1]) / 1024, 1)
    except OSError:
        return None


def open_window(a, origin):
    view = rect(a, "menu-View")
    click(origin, view)
    item = rect(a, "menu-item-View-Other Windows > Web Browser", timeout=5)
    if not item:
        raise RuntimeError("no View > Other Windows > Web Browser item")
    click(origin, item)


PIXEL = re.compile(r"first page pixel ([\d.]+) ms after the window opened \(engine (\w+)\)")


def main():
    ap = argparse.ArgumentParser()
    for k in ("--pid", "--log", "--bounds", "--shots", "--port", "--fixtures"):
        ap.add_argument(k, required=True)
    a = ap.parse_args()
    port = int(a.port)
    serve(port, a.fixtures)
    base = f"http://127.0.0.1:{port}"
    out = {"steps": {}}
    origin = window_origin()
    out["window_origin"] = origin
    if not rect(a, "menu-View", timeout=60):
        raise RuntimeError("no menu bar in the bounds file")
    time.sleep(2.0)
    out["shell_rss_before_mib"] = rss_mib(a.pid)

    # 1. The window, the engine cold.
    n = len(lines(a))
    open_window(a, origin)
    first = wait_line(a, lambda s: PIXEL.match(s), n, 60)
    out["steps"]["open_cold"] = first and first[1]
    m = first and PIXEL.match(first[1][len("eludite: web browser: "):])
    out["open_to_first_pixel_cold_ms"] = m and float(m.group(1))
    page = rect(a, "web-browser-page", timeout=20)
    out["page_rect"] = page

    # 2. The address bar, the form, a name typed by hand.
    key("ctrl+l")
    type_text(f"{base}/form.html")
    key("Return")
    time.sleep(2.5)
    click_at(origin, page[0] + page[2] - 60, page[1] + page[3] - 40)
    key("Tab")
    type_text("Grace Hopper")
    time.sleep(1.5)
    out["shell_rss_with_window_mib"] = rss_mib(a.pid)
    out["shot_page"] = shot(a, "browser-window-page")

    # 3. DevTools as a tab.
    n = len(lines(a))
    key("F12")
    dt = wait_line(a, lambda s: s.startswith("devtools opened"), n, 20)
    out["steps"]["devtools"] = dt and dt[1]
    time.sleep(4.0)
    out["shot_devtools"] = shot(a, "browser-window-devtools")
    key("ctrl+w")
    time.sleep(1.0)

    # 4. A JavaScript confirm() in the shell's dialog.
    key("ctrl+l")
    type_text(f"{base}/ask.html")
    key("Return")
    time.sleep(2.0)
    n = len(lines(a))
    click_at(origin, page[0] + 100, page[1] + 50)
    dialog = wait_line(a, lambda s: s.startswith("dialog "), n, 15)
    out["steps"]["dialog"] = dialog and dialog[1]
    time.sleep(0.8)
    out["shot_dialog"] = shot(a, "browser-window-dialog")
    key("Return")
    time.sleep(1.0)

    # 5. The agent fills the form; the strip shows while its wait is in flight.
    key("ctrl+backslash", "ctrl+c")
    time.sleep(1.5)
    box = rect(a, "agents-prompt", timeout=20)
    out["agents_prompt"] = box
    n = len(lines(a))
    if box:
        click(origin, box)
        type_text("Fill in the order form")
        key("Return")
        driving = wait_line(a, lambda s: s.startswith("driving: "), n, 60)
        out["steps"]["driving"] = driving and driving[1]
        # The agent's calls before its wait (tab_open, screenshot, read_page, form_input, input) take about a second.
        time.sleep(2.0)
        out["shot_driving"] = shot(a, "browser-window-agent-driving")
        done = wait_line(a, lambda s: s == "nobody is driving", (driving or (n, ""))[0], 30)
        out["steps"]["agent_done"] = done and done[1]
        time.sleep(1.5)
        out["shot_agent_done"] = shot(a, "browser-window-agent-done")

    # 6. Closed and reopened while the engine lingers.
    tab = rect(a, "doc-tab-web_browser", timeout=5)
    out["doc_tab"] = tab
    if tab:
        click_at(origin, tab[0] + tab[2] - 10, tab[1] + tab[3] / 2)
        time.sleep(1.5)
        n = len(lines(a))
        open_window(a, origin)
        again = wait_line(a, lambda s: PIXEL.match(s), n, 30)
        out["steps"]["open_running"] = again and again[1]
        m = again and PIXEL.match(again[1][len("eludite: web browser: "):])
        out["open_to_first_pixel_running_ms"] = m and float(m.group(1))
    out["log"] = lines(a)
    print(json.dumps(out, indent=1), flush=True)


if __name__ == "__main__":
    main()
