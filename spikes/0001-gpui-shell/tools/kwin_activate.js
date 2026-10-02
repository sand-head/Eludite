// KWin script used by inject_keys.py --activate-kwin: focus the spike window
// (KWin focus-stealing prevention may leave a newly launched window unfocused).
const list = workspace.windowList();
for (let i = 0; i < list.length; i++) {
    const w = list[i];
    if (w.resourceClass == "eludite-spike" || w.caption.indexOf("Eludite spike") === 0) {
        workspace.currentDesktop = w.desktops.length ? w.desktops[0] : workspace.currentDesktop;
        w.minimized = false;
        workspace.activeWindow = w;
        print("eludite-spike activated " + w.caption);
    }
}
