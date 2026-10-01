#!/usr/bin/env python3
"""Drive a running `niello --bounds-out FILE` with real X11 pointer and key
events (XTest, through the X server like a physical device), checking each
step against the element bounds the app reports.

Run against the X11 backend (WAYLAND_DISPLAY unset), for example inside a
nested KWin's Xwayland (tools/manual-linux.sh). Writes one JSON line per step
to stdout and exits non-zero if a step fails.

Usage: drive.py --bounds FILE --title "Demo - Niello" [--shots DIR]
"""
import argparse, json, os, subprocess, sys, time

from Xlib import X, XK, display
from Xlib.ext import xtest

d = display.Display()
root = d.screen().root


def find_window(title, timeout=10):
    end = time.time() + timeout
    while time.time() < end:
        stack = [root]
        while stack:
            w = stack.pop()
            try:
                name = w.get_wm_name()
                if name == title and w.get_attributes().map_state == X.IsViewable:
                    return w
                stack.extend(w.query_tree().children)
            except Exception:
                pass
        time.sleep(0.1)
    raise RuntimeError(f"no window titled {title!r}")


def close_window(title):
    """Ask the window to close, as the title bar's close button does."""
    w = find_window(title)
    proto = d.intern_atom("WM_PROTOCOLS")
    delete = d.intern_atom("WM_DELETE_WINDOW")
    from Xlib.protocol import event
    ev = event.ClientMessage(window=w, client_type=proto, data=(32, [delete, X.CurrentTime, 0, 0, 0]))
    w.send_event(ev)
    d.sync()


def origin(w):
    t = w.translate_coords(root, 0, 0)
    return -t.x, -t.y


class Driver:
    def __init__(self, bounds_file, title, shots):
        self.bounds_file, self.title, self.shots = bounds_file, title, shots
        self.failed = 0

    def bounds(self):
        try:
            with open(self.bounds_file) as f:
                return json.load(f)
        except (OSError, ValueError):
            return {}

    def wait_for(self, present=(), absent=(), timeout=5):
        end = time.time() + timeout
        while time.time() < end:
            b = self.bounds()
            if all(k in b for k in present) and not any(k in b for k in absent):
                return b
            time.sleep(0.1)
        return None

    def point(self, key, window_title=None):
        b = self.wait_for([key])
        if b is None:
            raise RuntimeError(f"element {key} not on screen")
        x, y, w, h = b[key]
        ox, oy = origin(find_window(window_title or self.title))
        return int(ox + x + w / 2), int(oy + y + h / 2)

    def move(self, x, y):
        xtest.fake_input(d, X.MotionNotify, x=x, y=y)
        d.sync()

    def button(self, press):
        xtest.fake_input(d, X.ButtonPress if press else X.ButtonRelease, 1)
        d.sync()

    def click(self, key, window_title=None):
        x, y = self.point(key, window_title)
        self.move(x, y)
        time.sleep(0.08)
        self.button(True)
        time.sleep(0.05)
        self.button(False)
        time.sleep(0.3)

    def hover(self, key):
        x, y = self.point(key)
        self.move(x - 3, y)
        time.sleep(0.05)
        self.move(x, y)
        time.sleep(0.4)

    def drag(self, src, dst):
        x, y = self.point(src)
        self.move(x, y)
        time.sleep(0.1)
        self.button(True)
        # Past GPUI's drag threshold, in steps like a hand would.
        for i in range(1, 9):
            self.move(x + 4 * i, y - 4 * i)
            time.sleep(0.02)
        guides = self.wait_for(["guide-left"])
        tx, ty = self.point(dst)
        for i in range(1, 13):
            self.move(x + (tx - x) * i // 12, y + (ty - y) * i // 12)
            time.sleep(0.02)
        time.sleep(0.2)
        self.button(False)
        time.sleep(0.4)
        return guides is not None

    def keys(self, *chords):
        for chord in chords:
            codes = [d.keysym_to_keycode(XK.string_to_keysym(k)) for k in chord.split("+")]
            for c in codes:
                xtest.fake_input(d, X.KeyPress, c)
            for c in reversed(codes):
                xtest.fake_input(d, X.KeyRelease, c)
            d.sync()
            time.sleep(0.1)
        time.sleep(0.3)

    def shot(self, name):
        if self.shots:
            path = os.path.join(self.shots, name + ".png")
            env = dict(os.environ)
            if os.environ.get("SHOT_WAYLAND_DISPLAY"):
                env["WAYLAND_DISPLAY"] = os.environ["SHOT_WAYLAND_DISPLAY"]
            subprocess.run(["spectacle", "-b", "-n", "-f", "-o", path], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)

    def step(self, name, action, present=(), absent=()):
        try:
            extra = action()
            ok = self.wait_for(present, absent) is not None
            if extra is False:
                ok = False
            err = None
        except Exception as e:  # noqa: BLE001
            ok, err = False, str(e)
        self.failed += not ok
        print(json.dumps({"step": name, "ok": ok, "error": err,
                          "expect_present": list(present), "expect_absent": list(absent)}), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bounds", required=True)
    ap.add_argument("--title", required=True)
    ap.add_argument("--shots")
    ap.add_argument("--close", action="store_true", help="close the main window at the end")
    a = ap.parse_args()
    dr = Driver(a.bounds, a.title, a.shots)
    find_window(a.title, 20)
    dr.wait_for(["documents"], timeout=10)
    s = dr.step
    s("drag Output tab to the Dock Left guide (guides shown)",
      lambda: dr.drag("tab-output", "guide-left"), present=["head-output"], absent=["tab-output"])
    s("drag Properties title bar onto the Error List group (tab)",
      lambda: dr.drag("head-properties", "group-error_list"), present=["tab-properties", "tab-error_list"])
    s("click a tab (Error List)", lambda: dr.click("tab-error_list"), present=["head-error_list"])
    s("drag the Properties tab out to the Dock Right guide (untab)",
      lambda: dr.drag("tab-properties", "guide-right"), present=["head-properties"], absent=["tab-properties"])
    s("Auto Hide Solution Explorer", lambda: dr.click("hide-solution_explorer"),
      present=["strip-solution_explorer"], absent=["flyout-pin"])
    s("hover the strip: fly-out slides out", lambda: dr.hover("strip-solution_explorer"), present=["flyout-pin"])
    s("click the document area: fly-out slides in", lambda: dr.click("documents"), absent=["flyout-pin"])
    s("click the Toolbox strip: fly-out stays out", lambda: dr.click("strip-toolbox"), present=["flyout-pin"])
    s("Pin Toolbox", lambda: dr.click("flyout-pin"), present=["head-toolbox"], absent=["strip-toolbox"])
    s("drag the Git Changes tab onto the document area: floats",
      lambda: dr.drag("head-git_changes", "documents"), present=["floating/head-git_changes"])
    s("Dock button in the floating window re-docks",
      lambda: dr.click("floating/dock-git_changes", "Git Changes"), absent=["floating/head-git_changes"])
    s("Float button floats again", lambda: dr.click("float-git_changes"), present=["floating/head-git_changes"])
    s("Close (x) Properties", lambda: dr.click("close-properties"), absent=["head-properties"])
    s("F4 shows Properties again", lambda: dr.keys("F4"), present=["head-properties"])
    s("Ctrl+Alt+O activates Output", lambda: dr.keys("Control_L+Alt_L+o"), present=["head-output"])
    s("Ctrl+\\, Ctrl+E activates Error List",
      lambda: dr.keys("Control_L+backslash", "Control_L+e"), present=["head-error_list"])
    s("Ctrl+Alt+L shows Solution Explorer (fly-out)", lambda: dr.keys("Control_L+Alt_L+l"), present=["flyout-pin"])
    s("Pin Solution Explorer", lambda: dr.click("flyout-pin"), present=["head-solution_explorer"])
    s("Ctrl+Alt+X shows Toolbox", lambda: dr.keys("Control_L+Alt_L+x"), present=["head-toolbox"])
    dr.click("documents")
    dr.shot("customized-before-exit")
    print(json.dumps({"done": True, "failed": dr.failed}), flush=True)
    if a.close:
        close_window(a.title)
    sys.exit(1 if dr.failed else 0)


if __name__ == "__main__":
    main()
