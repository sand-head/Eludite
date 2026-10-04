//! The Web Browser window (brief 0032, proposal 0002 sections 3 to 5): View > Other Windows > Web Browser opens it as
//! a document tab (id `web_browser`, as Visual Studio opens its Web Browser in the document well), one per shell,
//! holding the tabs of the workspace's browser.
//!
//! - **The same commands.** The tab strip follows the `eludite.browser.*` tabs ([`WindowEvent::Tabs`]); the toolbar
//!   (Back, Forward, Reload or Stop, Home, the address bar with Go and its history, DevTools), the tab strip (select,
//!   close, +), the keys and the context menu run the commands agents use (`navigate`, `tab_open`, `tab_close`,
//!   `tab_select`, `devtools`, `open_external`), audited as the person's, each on a thread of its own: the UI thread
//!   never waits on the browser. Copy, Paste, Select All and Save Image As... are the engine's `tab/action`.
//! - **The page** is a [`BrowserSurface`] per engine tab (frames from the ring, input to the tab), fitted to the view.
//!   DevTools is a tab of its own beside its page's (closed with it, or by its tab's close button).
//! - **Who drives.** While an agent's call is in flight the "Agent <name> is driving" strip shows a Stop button
//!   ([`BrowserBus::stop`]); the person's click, key or wheel in the page, or a toolbar command, interrupts the agent's
//!   wait ([`BrowserBus::person_acted`]).
//! - **Dialogs and prompts** of the page (`alert`, `confirm`, `prompt`, `beforeunload`, file choosers, authentication)
//!   and permission requests are the shell's Visual Studio-style dialogs, answered with `tab/dialogAnswer` and
//!   `tab/permissionAnswer`; an agent's `dialog` answer closes them (`tab/dialogClosed`).
//! - **Keys**, only while the window has the focus ([`CONTEXT`]): Ctrl+L the address bar, F5 Reload, Alt+Left and
//!   Alt+Right Back and Forward, Ctrl+T a new tab, Ctrl+W close it, F12 DevTools, Escape Stop.
//! - **Without the embedded engine** the window says why and what to run ([`super::browser::EngineStatus`]): the
//!   fetch script, the build command, and the package layout (brief 0039).
//! - **The sandbox** (brief 0039). When the engine refuses to start without Chromium's sandbox, the window shows a
//!   Visual Studio-style dialog, "Chromium's sandbox cannot start on this machine", with the two remedies and a "Run
//!   without the sandbox for this workspace" check box, once per workspace. OK with the box checked stores
//!   `browser.allowNoSandbox: true` in the person's state for the workspace (never the workspace's
//!   `.eludite/settings.json`, brief 0047) and opens the tab again (the engine starts with `--allow-no-sandbox`);
//!   otherwise the engine stays off and the window shows the message. While the engine runs without the sandbox, a
//!   strip reads "Browser running without Chromium's sandbox", and the start is audited
//!   (`eludite.browser.engine_start`, `sandbox: none`, `allowed_by`: `dialog`, `options` or `variable`). While the
//!   workspace's file carries the key, which is ignored, a strip says so ([`OPT_IN_IGNORED`]).
//! - Closing the window keeps the engine for a minute ([`super::browser::LINGER`]); the shell closes the engine with
//!   the workspace.
//! - **Tabs of debugging sessions** (brief 0037). F5 on a web project opens its page here as a tab of the session
//!   ([`WindowEvent::Tabs`]' `sessions`): a debug glyph before its title and the project's name in its tooltip.
//!   Restarting the session navigates the same tab; ending it leaves the tab open; closing the tab leaves the session
//!   running.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::CommandRegistry;
use eludite_commands::browser as cmds;
use eludite_ui::{Theme, check_box, dialog_panel, push_button, text_box};
use gpui::{
    AnimationExt as _, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, px, rgb,
};
use serde_json::{Value, json};

use super::browser::{BrowserBus, PageDriver, WindowEvent};
use super::browser_view::{BrowserSurface, FitSink, InputSink};

/// The window's GPUI key context: its keys apply only while it has the focus.
pub const CONTEXT: &str = "WebBrowser";

gpui::actions!(
    web_browser,
    [
        /// Ctrl+L.
        FocusAddressBar,
        /// F5.
        Reload,
        /// Alt+Left.
        Back,
        /// Alt+Right.
        Forward,
        /// Ctrl+T.
        NewTab,
        /// Ctrl+W.
        CloseTab,
        /// F12.
        OpenDevTools,
        /// Escape: stop loading (the address bar: undo the typing).
        StopLoading,
    ]
);

/// Bind the window's keys (Visual Studio's and the browsers'), scoped to [`CONTEXT`] so they win over the shell's
/// (F5 is Start Debugging, F12 Go To Definition elsewhere) only inside the window.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-l", FocusAddressBar, Some(CONTEXT)),
        KeyBinding::new("f5", Reload, Some(CONTEXT)),
        KeyBinding::new("alt-left", Back, Some(CONTEXT)),
        KeyBinding::new("alt-right", Forward, Some(CONTEXT)),
        KeyBinding::new("ctrl-t", NewTab, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", CloseTab, Some(CONTEXT)),
        KeyBinding::new("f12", OpenDevTools, Some(CONTEXT)),
        KeyBinding::new("escape", StopLoading, Some(CONTEXT)),
    ]);
}

/// A tab as the engine last described it (`tab/state`, `tab/cursor`).
#[derive(Debug, Clone, Default)]
pub struct TabView {
    pub url: String,
    pub title: String,
    pub loading: bool,
    /// The favicon's data url, and the image made from it.
    pub favicon_url: String,
    pub favicon: Option<Arc<gpui::Image>>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub cursor: String,
    pub status: String,
}

/// A dialog, prompt or permission request a page waits on.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The engine's tab.
    pub target: String,
    pub id: u64,
    /// `alert`, `confirm`, `prompt`, `beforeunload`, `file`, `auth`, or `permission`.
    pub kind: String,
    pub message: String,
    /// `prompt`: the text box; `auth`: the user name.
    pub text: String,
    /// `auth`: the password.
    pub password: String,
    /// `auth`: the password box has the keys.
    pub on_password: bool,
    /// `file`: `open`, `openMultiple`, `openFolder` or `save`.
    pub mode: String,
}

impl Prompt {
    /// The dialog's title, as Visual Studio's and the browsers' are.
    pub fn title(&self) -> &'static str {
        match self.kind.as_str() {
            "permission" => "Permission Request",
            "beforeunload" => "Leave Page",
            "file" => "Choose File",
            "auth" => "Sign In",
            _ => "Message from Webpage",
        }
    }
}

/// The page's context menu (`tab/contextMenu`): where, and what is under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    pub target: String,
    /// CSS pixels in the page.
    pub x: f32,
    pub y: f32,
    pub page_url: String,
    pub link_url: Option<String>,
    pub image_url: Option<String>,
}

/// The context menu's items, in Visual Studio's order (labels as the menu shows them).
pub const MENU_ITEMS: [&str; 10] = [
    "Back",
    "Forward",
    "Reload",
    "Copy",
    "Paste",
    "Select All",
    "Save Image As...",
    "Copy Link",
    "Open in External Browser",
    "Inspect",
];

/// The audit entry of an engine start without the sandbox (brief 0039).
pub const ENGINE_START: &str = "eludite.browser.engine_start";

/// The sandbox dialog's title (brief 0039).
pub const SANDBOX_TITLE: &str = "Chromium's sandbox cannot start on this machine";

/// The strip shown while the engine runs without the sandbox (brief 0039).
pub const NO_SANDBOX_STRIP: &str = "Browser running without Chromium's sandbox";

/// The warning while the workspace's `.eludite/settings.json` carries the opt-in, which is ignored (brief 0047); the
/// Output window's Browser pane gets the same line.
pub const OPT_IN_IGNORED: &str =
    "browser.allowNoSandbox in .eludite/settings.json is ignored: the sandbox opt-in is per person";

/// The dialog the engine's sandbox refusal opens (brief 0039).
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxPrompt {
    /// The engine's message (both remedies and the opt-in).
    pub message: String,
    /// "Run without the sandbox for this workspace".
    pub checked: bool,
}

impl SandboxPrompt {
    /// The remedies, as the dialog lists them: from the engine's message, which names the helper's command.
    pub fn remedies(&self) -> Vec<String> {
        if self.message.contains("runs as root") {
            return vec!["Run Eludite as a normal user: Chromium never sandboxes root.".into()];
        }
        let helper = self
            .message
            .split('`')
            .find(|part| part.starts_with("sudo chown"))
            .unwrap_or("sudo chown root:root chrome-sandbox && sudo chmod 4755 chrome-sandbox");
        vec![
            "Allow unprivileged user namespaces (sudo sysctl -w kernel.unprivileged_userns_clone=1, or on Ubuntu \
             23.10 and later an AppArmor profile that permits them for eludite-chromium)."
                .into(),
            format!("Or install Chromium's sandbox helper: {helper}"),
        ]
    }
}

/// The Web Browser window.
pub struct BrowserWindow {
    bus: BrowserBus,
    commands: Arc<CommandRegistry>,
    theme: Theme,
    open: bool,
    driver: Option<Arc<dyn PageDriver>>,
    /// The command tabs (`t1`, engine tab id), in order, and the selected one.
    tabs: Vec<(String, String)>,
    active: Option<String>,
    /// The tabs debugging sessions opened, by `t1` (brief 0037).
    sessions: BTreeMap<String, cmds::TabSession>,
    /// DevTools tabs: (page's engine tab, DevTools' engine tab).
    devtools: Vec<(String, String)>,
    /// A DevTools tab shown instead of its page.
    shown_devtools: Option<String>,
    info: HashMap<String, TabView>,
    surfaces: HashMap<String, Entity<BrowserSurface>>,
    /// A popup to select once the commands know it.
    select_when_known: Option<String>,
    address: String,
    /// The address bar holds the person's typing (not the page's url).
    address_edited: bool,
    /// The whole address is selected (Ctrl+L, a click into the bar, as browsers do): typing replaces it.
    address_all: bool,
    address_focus: FocusHandle,
    /// Addresses visited, newest first (the address bar's drop-down).
    history: Vec<String>,
    history_open: bool,
    driving: Vec<String>,
    prompts: Vec<Prompt>,
    prompt_focus: FocusHandle,
    /// A dialog was answered: the next render gives the keys back to the page (or the next dialog).
    refocus: bool,
    menu: Option<ContextMenu>,
    /// Why the window cannot show pages, with what to run.
    message: Option<String>,
    /// The last command's error, or a download's line.
    status: String,
    /// Commands the window sent that have not answered.
    in_flight: usize,
    /// The engine runs without Chromium's sandbox (brief 0039): the strip shows.
    no_sandbox: bool,
    /// The sandbox dialog, while it is open (brief 0039).
    sandbox_prompt: Option<SandboxPrompt>,
    sandbox_focus: FocusHandle,
    /// The workspaces whose person declined the opt-in this session: the dialog is offered once per workspace.
    sandbox_declined: Vec<Option<std::path::PathBuf>>,
    /// The workspace's file carries the opt-in, which is ignored: the warning strip shows (brief 0047).
    opt_in_ignored: bool,
    focus: FocusHandle,
    /// Window open to its first page pixel (the budget), and whether the engine was running then.
    opened_at: Option<(Instant, bool)>,
    first_pixel: Option<(Duration, bool)>,
    /// The commands the window ran, in order (tests and the run's log).
    ran: Vec<(String, Value)>,
}

/// Decode a favicon's `data:image/png;base64,...` url.
fn favicon_image(url: &str) -> Option<Arc<gpui::Image>> {
    let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
    if !head.ends_with(";base64") {
        return None;
    }
    let bytes = eludite_browser::browser::base64_decode(data)?;
    let format = if head.starts_with("image/png") {
        gpui::ImageFormat::Png
    } else if head.starts_with("image/jpeg") {
        gpui::ImageFormat::Jpeg
    } else if head.starts_with("image/gif") {
        gpui::ImageFormat::Gif
    } else {
        return None;
    };
    Some(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

/// What the person typed in the address bar, as a url: a scheme kept, a host given `http://` (`localhost:5000`,
/// `example.com/x`), anything else kept as typed for the command to refuse.
pub fn address_url(typed: &str) -> String {
    let t = typed.trim();
    if t.contains("://") || t.starts_with("about:") || t.starts_with("data:") {
        return t.to_owned();
    }
    let host = t.split('/').next().unwrap_or_default();
    if host.starts_with("localhost") || host.contains('.') || host.starts_with('[') {
        format!("http://{t}")
    } else {
        t.to_owned()
    }
}

/// A tab's tooltip (brief 0037).
struct TabTip {
    text: String,
    theme: Theme,
}

impl Render for TabTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .px_2()
            .py_1()
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .text_size(t.typography.ui)
            .text_color(t.text)
            .children(self.text.lines().map(|l| div().child(l.to_owned())))
    }
}

impl BrowserWindow {
    pub fn new(
        bus: BrowserBus,
        commands: Arc<CommandRegistry>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            bus,
            commands,
            theme,
            open: false,
            driver: None,
            tabs: Vec::new(),
            active: None,
            sessions: BTreeMap::new(),
            devtools: Vec::new(),
            shown_devtools: None,
            info: HashMap::new(),
            surfaces: HashMap::new(),
            select_when_known: None,
            address: String::new(),
            address_edited: false,
            address_all: false,
            address_focus: cx.focus_handle(),
            history: Vec::new(),
            history_open: false,
            driving: Vec::new(),
            prompts: Vec::new(),
            prompt_focus: cx.focus_handle(),
            refocus: false,
            menu: None,
            message: None,
            status: String::new(),
            in_flight: 0,
            no_sandbox: false,
            sandbox_prompt: None,
            sandbox_focus: cx.focus_handle(),
            sandbox_declined: Vec::new(),
            opt_in_ignored: false,
            focus: cx.focus_handle(),
            opened_at: None,
            first_pixel: None,
            ran: Vec::new(),
        }
    }

    // ---- what tests and the run read ----

    /// Where the shown page is drawn (window coordinates), for `--bounds-out` (the Xvfb run clicks into the page).
    pub fn painted_bounds(&self, cx: &App) -> Vec<(&'static str, gpui::Bounds<gpui::Pixels>)> {
        self.shown_target()
            .and_then(|t| self.surfaces.get(&t))
            .and_then(|s| s.read(cx).bounds())
            .map(|b| vec![("web-browser-page", b)])
            .unwrap_or_default()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The tab strip: (label, engine tab id, selected), DevTools tabs beside their pages.
    pub fn strip(&self) -> Vec<(String, String, bool)> {
        let shown = self.shown_target();
        let mut out = Vec::new();
        for (_, target) in &self.tabs {
            let title = self.title_of(target);
            out.push((
                title.clone(),
                target.clone(),
                shown.as_deref() == Some(target),
            ));
            for (_, d) in self.devtools.iter().filter(|(p, _)| p == target) {
                out.push((
                    format!("DevTools - {title}"),
                    d.clone(),
                    shown.as_deref() == Some(d),
                ));
            }
        }
        out
    }

    /// Whether the window (its page, its address bar or a prompt) has the keyboard focus (brief 0038's test that a
    /// launch leaves it with the editor).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self.address_focus.is_focused(window)
            || self.prompt_focus.is_focused(window)
            || self
                .surfaces
                .values()
                .any(|s| s.read(cx).focus_handle().is_focused(window))
    }

    /// The command tabs with their titles, in the strip's order (brief 0038: a browser session is named after its
    /// tab's title and ends when the tab closes).
    pub fn tab_titles(&self) -> Vec<(String, Option<String>)> {
        self.tabs
            .iter()
            .map(|(id, target)| {
                let title = self
                    .info
                    .get(target)
                    .map(|i| i.title.clone())
                    .filter(|t| !t.is_empty());
                (id.clone(), title)
            })
            .collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn driving(&self) -> &[String] {
        &self.driving
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn prompts(&self) -> &[Prompt] {
        &self.prompts
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn context_menu(&self) -> Option<&ContextMenu> {
        self.menu.as_ref()
    }

    /// The sandbox dialog, while it is open (brief 0039).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sandbox_prompt(&self) -> Option<&SandboxPrompt> {
        self.sandbox_prompt.as_ref()
    }

    /// The workspace's `.eludite/settings.json` carries the opt-in, which is ignored (brief 0047): the warning shows.
    pub fn set_opt_in_ignored(&mut self, ignored: bool, cx: &mut Context<Self>) {
        if self.opt_in_ignored != ignored {
            self.opt_in_ignored = ignored;
            cx.notify();
        }
    }

    /// The warning about the workspace's ignored opt-in, while it shows (brief 0047).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn opt_in_ignored(&self) -> bool {
        self.opt_in_ignored
    }

    /// The engine runs without the sandbox: the strip shows (brief 0039).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running_without_sandbox(&self) -> bool {
        self.no_sandbox
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn address(&self) -> &str {
        &self.address
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn status_line(&self) -> &str {
        &self.status
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ran(&self) -> &[(String, Value)] {
        &self.ran
    }

    /// Replace the address bar's text, as typing does.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_address(&mut self, text: &str, cx: &mut Context<Self>) {
        self.address = text.to_owned();
        self.address_edited = true;
        cx.notify();
    }

    /// Back and Forward enabled, for the shown tab.
    pub fn can_go(&self) -> (bool, bool) {
        self.shown_target()
            .and_then(|t| self.info.get(&t))
            .map_or((false, false), |i| (i.can_go_back, i.can_go_forward))
    }

    /// Window open to the first page pixel (the first paint of a page image since the open), and whether the engine
    /// was running at the open.
    pub fn first_pixel(&self, cx: &App) -> Option<(Duration, bool)> {
        if self.first_pixel.is_some() {
            return self.first_pixel;
        }
        let (at, running) = self.opened_at?;
        let s = self
            .shown_target()
            .and_then(|t| self.surfaces.get(&t).cloned())?;
        let stats = s.read(cx).stats();
        let when = stats
            .borrow()
            .painted
            .iter()
            .find(|(when, _)| *when >= at)
            .map(|(when, _)| *when)?;
        Some((when - at, running))
    }

    /// The engine tab drawn: the selected DevTools tab, else the selected command tab.
    pub fn shown_target(&self) -> Option<String> {
        if let Some(d) = &self.shown_devtools {
            return Some(d.clone());
        }
        let active = self.active.as_ref()?;
        self.tabs
            .iter()
            .find(|(id, _)| id == active)
            .map(|(_, t)| t.clone())
    }

    /// The debugging session that opened the tab of engine tab `target` (brief 0037).
    pub fn tab_session(&self, target: &str) -> Option<&cmds::TabSession> {
        self.sessions.get(&self.command_tab(target)?)
    }

    /// A tab's tooltip: its title and url, and for a session's tab the project being debugged.
    pub fn tab_tooltip(&self, target: &str) -> String {
        let mut text = self.title_of(target);
        if let Some(url) = self
            .info
            .get(target)
            .map(|i| i.url.clone())
            .filter(|u| !u.is_empty() && *u != text)
        {
            text.push('\n');
            text.push_str(&url);
        }
        if let Some(s) = self.tab_session(target) {
            text.push_str(&format!(
                "\n{}: opened by debugging session {} (Restart reloads it)",
                s.name, s.id
            ));
        }
        text
    }

    fn title_of(&self, target: &str) -> String {
        match self.info.get(target) {
            Some(i) if !i.title.is_empty() => i.title.clone(),
            Some(i) if !i.url.is_empty() => i.url.clone(),
            _ => "New Tab".to_owned(),
        }
    }

    fn command_tab(&self, target: &str) -> Option<String> {
        self.tabs
            .iter()
            .find(|(_, t)| t == target)
            .map(|(id, _)| id.clone())
    }

    // ---- opening and closing ----

    /// The document tab opened (true) or closed.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open == self.open {
            return;
        }
        self.open = open;
        if !open {
            self.menu = None;
            self.history_open = false;
            self.bus.window_closed();
            cx.notify();
            return;
        }
        self.bus.window_opened();
        let status = self.bus.status();
        self.message = status.message.clone();
        let running = self.driver.is_some();
        let at = Instant::now();
        self.opened_at = Some((at, running));
        self.first_pixel = None;
        // A browser with tabs (a debugging session's page, opened before the window; brief 0037) gets no blank one.
        if status.embedded && self.tabs.is_empty() && self.in_flight == 0 && !self.bus.has_tabs() {
            self.new_tab(cx);
        }
        // The first page pixel after the open (the budget): the first paint of a page image since then.
        if status.embedded {
            cx.spawn(async move |this, cx| {
                let deadline = at + Duration::from_secs(30);
                while Instant::now() < deadline {
                    cx.background_executor()
                        .timer(Duration::from_millis(2))
                        .await;
                    let Ok(done) = this.update(cx, |w, cx| w.find_first_pixel(at, cx)) else {
                        return;
                    };
                    if done {
                        return;
                    }
                }
            })
            .detach();
        }
        // A debugging session's launch opens the window without taking the keys from the editor (brief 0038): F5
        // there still continues, and F5 in the window reloads the page once the person clicks into it.
        if !self.bus.quiet() {
            self.focus_page(window, cx);
        }
        cx.notify();
    }

    /// Whether the shown page painted since the open at `at` (and then record it); false while waiting, true when
    /// done (found, or the window reopened since).
    fn find_first_pixel(&mut self, at: Instant, cx: &mut Context<Self>) -> bool {
        let Some((opened, _)) = self.opened_at else {
            return true;
        };
        if opened != at || self.first_pixel.is_some() {
            return true;
        }
        let Some((took, running)) = self.first_pixel(cx) else {
            return false;
        };
        self.first_pixel = Some((took, running));
        eprintln!(
            "eludite: web browser: first page pixel {:.1} ms after the window opened (engine {})",
            took.as_secs_f64() * 1e3,
            if running { "running" } else { "cold" }
        );
        true
    }

    fn focus_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self
            .shown_target()
            .and_then(|t| self.surfaces.get(&t).cloned())
        {
            Some(s) => {
                let f = s.read(cx).focus_handle();
                window.focus(&f, cx);
            }
            None => window.focus(&self.focus, cx),
        }
    }

    // ---- what the browser says ----

    pub fn on_event(&mut self, event: WindowEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            WindowEvent::Started(driver) => {
                self.no_sandbox = !driver.sandboxed();
                if self.no_sandbox {
                    self.audit_no_sandbox();
                }
                self.driver = Some(driver);
                self.surfaces.clear();
                self.message = None;
                self.sandbox_prompt = None;
            }
            WindowEvent::Stopped => self.forget(),
            WindowEvent::SandboxRefused(message) => self.sandbox_refused(message, window, cx),
            WindowEvent::Notification { method, params } => {
                self.on_notification(&method, &params, window, cx)
            }
            WindowEvent::DevtoolsOpened { page, devtools } => {
                let page_log = page.clone();
                if !self.devtools.iter().any(|(_, d)| *d == devtools) {
                    self.devtools.push((page, devtools.clone()));
                }
                eprintln!("eludite: web browser: devtools opened for {page_log}");
                self.shown_devtools = Some(devtools);
                self.focus_page(window, cx);
            }
            WindowEvent::Tabs {
                tabs,
                active,
                sessions,
            } => {
                let shown_before = self.shown_target();
                self.tabs = tabs;
                self.active = active;
                self.sessions = sessions;
                let pages: Vec<String> = self.tabs.iter().map(|(_, t)| t.clone()).collect();
                self.devtools.retain(|(p, _)| pages.contains(p));
                if let Some(d) = &self.shown_devtools
                    && !self.devtools.iter().any(|(_, x)| x == d)
                {
                    self.shown_devtools = None;
                }
                if let Some(want) = self.select_when_known.clone()
                    && let Some(id) = self.command_tab(&want)
                {
                    self.select_when_known = None;
                    if self.active.as_deref() != Some(id.as_str()) {
                        self.run(cmds::TAB_SELECT, json!({"tab": id}), cx);
                    }
                }
                if self.tabs.is_empty() && self.driver.is_none() {
                    self.surfaces.clear();
                }
                if self.shown_target() != shown_before {
                    self.address_edited = false;
                    self.sync_address();
                    if self.open && !self.bus.quiet() {
                        self.focus_page(window, cx);
                    }
                }
            }
            WindowEvent::Driving(agents) => {
                if agents != self.driving {
                    // The Xvfb run (tools/browser_window.py) reads these.
                    if agents.is_empty() {
                        eprintln!("eludite: web browser: nobody is driving");
                    } else {
                        eprintln!("eludite: web browser: driving: {}", agents.join(", "));
                    }
                }
                self.driving = agents;
            }
        }
        cx.notify();
    }

    /// The engine runs without the sandbox: the audit log says so, with what allowed it (brief 0039): `dialog`,
    /// `options` or `variable`, as the worker recorded it before the launch (brief 0047).
    fn audit_no_sandbox(&self) {
        let allowed_by = self
            .bus
            .launch_allowed_by()
            .unwrap_or(super::browser::ALLOWED_BY_OPTIONS);
        self.commands.audit_log().record_call(
            ENGINE_START,
            None,
            eludite_commands::Outcome::Ok,
            eludite_commands::Caller::User,
            Some(json!({"sandbox": "none", "allowed_by": allowed_by})),
        );
        // The Xvfb run (tools/browser-sandbox-linux.sh) reads this.
        eprintln!("eludite: web browser: running without Chromium's sandbox ({allowed_by})");
    }

    /// The engine refused to start without the sandbox (brief 0039): the engine stays off and the window says why;
    /// the dialog offers the opt-in once per workspace, while the window is open.
    fn sandbox_refused(&mut self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("eludite: web browser: the sandbox cannot start: {message}");
        self.message = Some(format!(
            "{message}\n\nTo run the browser without the sandbox in this workspace, turn on \"Run without Chromium's \
             sandbox for this workspace\" in Tools > Options > Web Browser."
        ));
        let workspace = self.bus.workspace();
        if self.open && self.sandbox_prompt.is_none() && !self.sandbox_declined.contains(&workspace)
        {
            self.sandbox_prompt = Some(SandboxPrompt {
                message,
                checked: false,
            });
            window.focus(&self.sandbox_focus, cx);
        }
    }

    /// The sandbox dialog's answer: OK (`accept`) with the box checked takes the opt-in; anything else declines it
    /// for this workspace, leaving the engine off.
    pub fn answer_sandbox(&mut self, accept: bool, cx: &mut Context<Self>) {
        let Some(p) = self.sandbox_prompt.take() else {
            return;
        };
        self.refocus = true;
        if !(accept && p.checked) {
            self.sandbox_declined.push(self.bus.workspace());
            eprintln!("eludite: web browser: the sandbox opt-in was declined");
            cx.notify();
            return;
        }
        eprintln!("eludite: web browser: the sandbox opt-in was taken for this workspace");
        self.bus.opt_in_no_sandbox();
        self.message = None;
        // The tab again: its launch passes --allow-no-sandbox now.
        if self.tabs.is_empty() && self.in_flight == 0 {
            self.new_tab(cx);
        }
        // The person's answer, in their own state for the workspace (brief 0047), never the workspace's file.
        if self.bus.workspace().is_some() {
            self.run(
                eludite_commands::settings::SET,
                json!({"key": "browser.allowNoSandbox", "value": true, "scope": "user-workspace"}),
                cx,
            );
        }
        cx.notify();
    }

    /// The dialog's check box.
    pub fn toggle_sandbox_check(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.sandbox_prompt.as_mut() {
            p.checked = !p.checked;
            cx.notify();
        }
    }

    /// The engine is gone: so are its tabs.
    fn forget(&mut self) {
        self.no_sandbox = false;
        self.driver = None;
        self.tabs.clear();
        self.active = None;
        self.devtools.clear();
        self.shown_devtools = None;
        self.info.clear();
        self.surfaces.clear();
        self.prompts.clear();
        self.menu = None;
    }

    fn sync_address(&mut self) {
        if self.address_edited {
            return;
        }
        self.address = self
            .shown_target()
            .and_then(|t| self.info.get(&t))
            .map(|i| i.url.clone())
            .unwrap_or_default();
    }

    fn on_notification(
        &mut self,
        method: &str,
        p: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = p["tab"].as_str().unwrap_or_default().to_owned();
        match method {
            "tab/state" => {
                let i = self.info.entry(tab.clone()).or_default();
                if let Some(u) = p["url"].as_str() {
                    u.clone_into(&mut i.url);
                }
                if let Some(t) = p["title"].as_str() {
                    t.clone_into(&mut i.title);
                }
                if let Some(f) = p["favicon"].as_str()
                    && f != i.favicon_url
                {
                    f.clone_into(&mut i.favicon_url);
                    i.favicon = favicon_image(f);
                }
                if let Some(s) = p["statusText"].as_str() {
                    s.clone_into(&mut i.status);
                }
                for (k, to) in [
                    ("loading", &mut i.loading),
                    ("canGoBack", &mut i.can_go_back),
                    ("canGoForward", &mut i.can_go_forward),
                ] {
                    if let Some(v) = p[k].as_bool() {
                        *to = v;
                    }
                }
                let url = i.url.clone();
                if p["url"].is_string()
                    && !url.is_empty()
                    && !url.starts_with("devtools:")
                    && url != "about:blank"
                {
                    self.history.retain(|h| *h != url);
                    self.history.insert(0, url);
                    self.history.truncate(20);
                }
                if self.shown_target().as_deref() == Some(tab.as_str()) {
                    self.sync_address();
                }
            }
            "tab/cursor" => {
                let c = p["cursor"].as_str().unwrap_or("default").to_owned();
                if let Some(s) = self.surfaces.get(&tab) {
                    s.update(cx, |s, cx| s.set_cursor(&c, cx));
                }
                self.info.entry(tab).or_default().cursor = c;
            }
            "tab/popup" => {
                // The page opened a tab: the commands adopt it (tabs), and the window shows it.
                self.select_when_known = Some(tab);
                self.run(cmds::TABS, json!({}), cx);
            }
            "tab/dialog" | "tab/permission" => {
                let Some(id) = p["id"].as_u64() else { return };
                let permission = method == "tab/permission";
                let message = if permission {
                    let what: Vec<&str> = p["permissions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    format!(
                        "{} wants to use: {}",
                        p["origin"].as_str().unwrap_or("This page"),
                        what.join(", ")
                    )
                } else {
                    match p["kind"].as_str() {
                        Some("auth") => format!(
                            "{} asks for a user name and password{}",
                            p["host"].as_str().unwrap_or("The server"),
                            p["realm"]
                                .as_str()
                                .filter(|r| !r.is_empty())
                                .map(|r| format!(" ({r})"))
                                .unwrap_or_default()
                        ),
                        Some("beforeunload") => {
                            "This page asks to confirm that you want to leave. Changes you made may not be saved."
                                .to_owned()
                        }
                        Some("file") => format!(
                            "The page asks for {}.",
                            match p["mode"].as_str() {
                                Some("openMultiple") => "files",
                                Some("openFolder") => "a folder",
                                Some("save") => "where to save a file",
                                _ => "a file",
                            }
                        ),
                        _ => p["message"].as_str().unwrap_or_default().to_owned(),
                    }
                };
                self.prompts.retain(|x| x.id != id);
                self.prompts.push(Prompt {
                    target: tab,
                    id,
                    kind: if permission {
                        "permission".into()
                    } else {
                        p["kind"].as_str().unwrap_or("alert").to_owned()
                    },
                    message,
                    text: p["defaultText"].as_str().unwrap_or_default().to_owned(),
                    password: String::new(),
                    on_password: false,
                    mode: p["mode"].as_str().unwrap_or("open").to_owned(),
                });
                eprintln!(
                    "eludite: web browser: dialog {} shown",
                    self.prompts.last().map_or("", |p| p.kind.as_str())
                );
                if self.open {
                    window.focus(&self.prompt_focus, cx);
                }
            }
            "tab/dialogClosed" => {
                if let Some(id) = p["id"].as_u64() {
                    self.prompts.retain(|x| x.id != id);
                }
            }
            "tab/download" => {
                let file = p["file"].as_str().unwrap_or_default();
                self.status = match p["state"].as_str().unwrap_or_default() {
                    "started" => format!("Downloading {file}\u{2026}"),
                    "progress" => match (p["receivedBytes"].as_u64(), p["totalBytes"].as_u64()) {
                        (Some(r), Some(t)) if t > 0 => {
                            format!("Downloading {file}: {}%", r * 100 / t)
                        }
                        _ => format!("Downloading {file}\u{2026}"),
                    },
                    _ => eludite_browser::embedded::download_line(p).unwrap_or_default(),
                };
            }
            "tab/contextMenu" => {
                let s = |k: &str| p[k].as_str().filter(|v| !v.is_empty()).map(str::to_owned);
                self.menu = Some(ContextMenu {
                    target: tab,
                    x: p["x"].as_f64().unwrap_or(0.) as f32,
                    y: p["y"].as_f64().unwrap_or(0.) as f32,
                    page_url: s("pageUrl").unwrap_or_default(),
                    link_url: s("linkUrl"),
                    image_url: s("imageUrl"),
                });
            }
            "tab/closed" => {
                self.info.remove(&tab);
                self.surfaces.remove(&tab);
                self.prompts.retain(|x| x.target != tab);
                let was_devtools = self.devtools.iter().any(|(_, d)| *d == tab);
                self.devtools.retain(|(p, d)| *p != tab && *d != tab);
                if self.shown_devtools.as_deref() == Some(tab.as_str()) {
                    self.shown_devtools = None;
                }
                if !was_devtools && self.command_tab(&tab).is_some() {
                    // A page closed itself (window.close()): the commands forget it.
                    self.run(cmds::TABS, json!({}), cx);
                }
            }
            _ => {}
        }
    }

    // ---- running commands ----

    /// Run a browser command as the person, on a thread of its own; its answer comes back to the window.
    pub fn run(&mut self, id: &'static str, args: Value, cx: &mut Context<Self>) {
        self.ran.push((id.to_owned(), args.clone()));
        self.in_flight += 1;
        let commands = self.commands.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("browser-window-command".into())
            .spawn(move || {
                let _ = tx.send(commands.invoke(id, args));
            });
        if spawned.is_err() {
            self.in_flight -= 1;
            return;
        }
        cx.spawn(async move |this, cx| {
            let answer = rx.await;
            let _ = this.update(cx, |w, cx| {
                w.in_flight = w.in_flight.saturating_sub(1);
                match answer {
                    Ok(Ok(out)) => w.answered(id, &out, cx),
                    Ok(Err(e)) => {
                        eprintln!("eludite: web browser: {id}: {e}");
                        w.status = e.to_string();
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn answered(&mut self, id: &str, out: &Value, cx: &mut Context<Self>) {
        if id == cmds::TAB_OPEN
            && self.bus.settings().show_devtools_tab
            && let Some(tab) = out["id"].as_str()
        {
            self.run(cmds::DEVTOOLS, json!({"tab": tab}), cx);
        }
        if id == cmds::NAVIGATE || id == cmds::TAB_OPEN {
            self.status.clear();
        }
    }

    /// A toolbar command: the person's hand first (an agent's wait ends), then the command.
    fn person_runs(&mut self, id: &'static str, args: Value, cx: &mut Context<Self>) {
        self.bus.person_acted();
        self.menu = None;
        self.history_open = false;
        self.run(id, args, cx);
    }

    fn active_args(&self) -> Value {
        match &self.active {
            Some(t) => json!({"tab": t}),
            None => json!({}),
        }
    }

    pub fn navigate_action(&mut self, action: &str, cx: &mut Context<Self>) {
        let mut args = self.active_args();
        args["action"] = json!(action);
        if action != "stop" {
            args["wait_until"] = json!("none");
        }
        self.person_runs(cmds::NAVIGATE, args, cx);
    }

    /// Go to what the address bar holds.
    pub fn go(&mut self, cx: &mut Context<Self>) {
        let url = address_url(&self.address);
        if url.is_empty() {
            return;
        }
        self.address_edited = false;
        self.address = url.clone();
        self.go_to(url, cx);
    }

    fn go_to(&mut self, url: String, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            self.person_runs(cmds::TAB_OPEN, json!({"url": url}), cx);
            return;
        }
        let mut args = self.active_args();
        args["url"] = json!(url);
        args["wait_until"] = json!("none");
        self.person_runs(cmds::NAVIGATE, args, cx);
    }

    /// Focus the address bar with its whole text selected (Ctrl+L, a click into it).
    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.address_all = true;
        window.focus(&self.address_focus, cx);
        cx.notify();
    }

    /// A key typed in the address bar: `key` as GPUI names it, `typed` its text. The selected address is replaced.
    pub fn address_key(&mut self, key: &str, typed: Option<&str>) {
        let all = std::mem::take(&mut self.address_all);
        if key == "backspace" {
            if all {
                self.address.clear();
            } else {
                self.address.pop();
            }
        } else if let Some(c) = typed {
            if all {
                self.address.clear();
            }
            self.address.push_str(c);
        } else {
            self.address_all = all;
            return;
        }
        self.address_edited = true;
    }

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        let home = self.bus.settings().home_page;
        self.shown_devtools = None;
        self.person_runs(cmds::TAB_OPEN, json!({"url": home}), cx);
    }

    pub fn home(&mut self, cx: &mut Context<Self>) {
        let home = self.bus.settings().home_page;
        self.go_to(home, cx);
    }

    /// Close the shown tab (a DevTools tab closes alone).
    pub fn close_shown(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.shown_target() {
            self.close_target(&t, cx);
        }
    }

    pub fn close_target(&mut self, target: &str, cx: &mut Context<Self>) {
        if self.devtools.iter().any(|(_, d)| d == target) {
            if let Some(d) = &self.driver {
                d.close(target);
            }
            self.devtools.retain(|(_, d)| d != target);
            if self.shown_devtools.as_deref() == Some(target) {
                self.shown_devtools = None;
                self.address_edited = false;
                self.sync_address();
            }
            cx.notify();
            return;
        }
        if let Some(id) = self.command_tab(target) {
            self.person_runs(cmds::TAB_CLOSE, json!({"tab": id}), cx);
        }
    }

    pub fn select_target(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if self.devtools.iter().any(|(_, d)| d == target) {
            self.shown_devtools = Some(target.to_owned());
            self.address_edited = false;
            self.sync_address();
            self.focus_page(window, cx);
            cx.notify();
            return;
        }
        self.shown_devtools = None;
        if let Some(id) = self.command_tab(target) {
            if self.active.as_deref() != Some(id.as_str()) {
                self.person_runs(cmds::TAB_SELECT, json!({"tab": id}), cx);
            } else {
                self.address_edited = false;
                self.sync_address();
                self.focus_page(window, cx);
            }
        }
        cx.notify();
    }

    pub fn open_devtools(&mut self, inspect: Option<(f32, f32)>, cx: &mut Context<Self>) {
        let page = match &self.shown_devtools {
            Some(d) => self
                .devtools
                .iter()
                .find(|(_, x)| x == d)
                .map(|(p, _)| p.clone()),
            None => self.shown_target(),
        };
        let Some(id) = page.and_then(|p| self.command_tab(&p)) else {
            return;
        };
        let mut args = json!({"tab": id});
        if let Some((x, y)) = inspect {
            args["inspect"] = json!({"x": x.max(0.), "y": y.max(0.)});
        }
        self.person_runs(cmds::DEVTOOLS, args, cx);
    }

    /// The strip's Stop.
    pub fn stop_agent(&mut self, cx: &mut Context<Self>) {
        self.bus.stop();
        cx.notify();
    }

    fn page_action(&mut self, target: &str, action: &str, url: Option<&str>) {
        self.bus.person_acted();
        if let Some(d) = &self.driver {
            let mut p = json!({"tab": target, "action": action});
            if let Some(u) = url {
                p["url"] = json!(u);
            }
            d.notify("tab/action", p);
        }
    }

    /// Run a context menu item.
    pub fn menu_item(&mut self, label: &str, cx: &mut Context<Self>) {
        let Some(m) = self.menu.take() else { return };
        let tab = self.command_tab(&m.target);
        let with_tab = |mut a: Value| {
            if let Some(t) = &tab {
                a["tab"] = json!(t);
            }
            a
        };
        match label {
            "Back" => self.person_runs(
                cmds::NAVIGATE,
                with_tab(json!({"action": "back", "wait_until": "none"})),
                cx,
            ),
            "Forward" => self.person_runs(
                cmds::NAVIGATE,
                with_tab(json!({"action": "forward", "wait_until": "none"})),
                cx,
            ),
            "Reload" => self.person_runs(
                cmds::NAVIGATE,
                with_tab(json!({"action": "reload", "wait_until": "none"})),
                cx,
            ),
            "Copy" => self.page_action(&m.target, "copy", None),
            "Paste" => self.page_action(&m.target, "paste", None),
            "Select All" => self.page_action(&m.target, "selectAll", None),
            "Save Image As..." => {
                if let Some(u) = &m.image_url {
                    self.page_action(&m.target, "download", Some(u));
                }
            }
            "Copy Link" => {
                if let Some(u) = &m.link_url {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(u.clone()));
                }
            }
            "Open in External Browser" => {
                let url = m.link_url.clone().unwrap_or(m.page_url.clone());
                self.person_runs(cmds::OPEN_EXTERNAL, json!({"url": url}), cx);
            }
            "Inspect" => {
                if let Some(t) = tab {
                    self.person_runs(
                        cmds::DEVTOOLS,
                        json!({"tab": t, "inspect": {"x": m.x.max(0.), "y": m.y.max(0.)}}),
                        cx,
                    );
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn menu_enabled(&self, m: &ContextMenu, label: &str) -> bool {
        let i = self.info.get(&m.target);
        match label {
            "Back" => i.is_some_and(|i| i.can_go_back),
            "Forward" => i.is_some_and(|i| i.can_go_forward),
            "Save Image As..." => m.image_url.is_some(),
            "Copy Link" => m.link_url.is_some(),
            "Inspect" => self.command_tab(&m.target).is_some(),
            _ => true,
        }
    }

    /// Answer the dialog shown (OK, Allow, Leave: `accept`; Cancel, Block, Stay: not).
    pub fn answer_prompt(&mut self, accept: bool, files: Vec<String>, cx: &mut Context<Self>) {
        let Some(ix) = self.shown_prompt_index() else {
            return;
        };
        let p = self.prompts.remove(ix);
        self.refocus = true;
        self.bus.person_acted();
        if let Some(d) = &self.driver {
            if p.kind == "permission" {
                d.notify(
                    "tab/permissionAnswer",
                    json!({"tab": p.target, "id": p.id, "allow": accept}),
                );
            } else {
                let mut a = json!({"tab": p.target, "id": p.id, "accept": accept});
                match p.kind.as_str() {
                    "prompt" if accept => a["text"] = json!(p.text),
                    "auth" if accept => {
                        a["username"] = json!(p.text);
                        a["password"] = json!(p.password);
                    }
                    "file" if accept => a["files"] = json!(files),
                    _ => {}
                }
                d.notify("tab/dialogAnswer", a);
            }
        }
        cx.notify();
    }

    /// The prompt shown: the shown tab's first, else any.
    fn shown_prompt_index(&self) -> Option<usize> {
        let shown = self.shown_target();
        self.prompts
            .iter()
            .position(|p| Some(&p.target) == shown.as_ref())
            .or((!self.prompts.is_empty()).then_some(0))
    }

    /// File chooser: the system's dialog, then the answer.
    fn choose_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.shown_prompt_index() else {
            return;
        };
        let mode = self.prompts[ix].mode.clone();
        let picked = if mode == "save" {
            let dir = std::env::current_dir().unwrap_or_default();
            let rx = cx.prompt_for_new_path(&dir, None);
            cx.spawn_in(window, async move |this, cx| {
                let path = match rx.await {
                    Ok(Ok(Some(p))) => Some(vec![p.to_string_lossy().into_owned()]),
                    _ => None,
                };
                let _ = this.update(cx, |w, cx| match path {
                    Some(p) => w.answer_prompt(true, p, cx),
                    None => w.answer_prompt(false, Vec::new(), cx),
                });
            })
        } else {
            let rx = cx.prompt_for_paths(PathPromptOptions {
                files: mode != "openFolder",
                directories: mode == "openFolder",
                multiple: mode == "openMultiple",
                prompt: Some("Open".into()),
            });
            cx.spawn_in(window, async move |this, cx| {
                let paths = match rx.await {
                    Ok(Ok(Some(p))) => Some(
                        p.into_iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                };
                let _ = this.update(cx, |w, cx| match paths {
                    Some(p) => w.answer_prompt(true, p, cx),
                    None => w.answer_prompt(false, Vec::new(), cx),
                });
            })
        };
        picked.detach();
    }

    // ---- the surfaces ----

    /// The shown tab's surface, made on first sight.
    fn surface_for(
        &mut self,
        target: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<BrowserSurface>> {
        if let Some(s) = self.surfaces.get(target) {
            return Some(s.clone());
        }
        let driver = self.driver.clone()?;
        let frames = driver.frames(target)?;
        let input: InputSink = {
            let (driver, bus, target) = (driver.clone(), self.bus.clone(), target.to_owned());
            std::rc::Rc::new(move |e: Value| {
                // The person's hand in the page: clicks, keys, wheel and IME, not the pointer passing by.
                if !matches!(e["type"].as_str(), Some("mouseMove" | "mouseUp" | "keyUp")) {
                    bus.person_acted();
                }
                driver.input(&target, e)
            })
        };
        let fit: FitSink = {
            let (driver, target) = (driver.clone(), target.to_owned());
            std::rc::Rc::new(move |w, h, s| driver.resize(&target, w, h, s))
        };
        let surface = cx.new(|cx| BrowserSurface::new(frames, Some(input), Some(fit), cx));
        if let Some(c) = self.info.get(target).map(|i| i.cursor.clone()) {
            surface.update(cx, |s, cx| s.set_cursor(&c, cx));
        }
        self.surfaces.insert(target.to_owned(), surface.clone());
        Some(surface)
    }

    // ---- drawing ----

    fn render_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let tabs = self.strip();
        let mut row = div()
            .id("web-browser-tabs")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(26.))
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .overflow_x_scroll();
        for (i, (title, target, selected)) in tabs.into_iter().enumerate() {
            let info = self.info.get(&target);
            let loading = info.is_some_and(|i| i.loading);
            let icon: AnyElement = match info.and_then(|i| i.favicon.clone()) {
                _ if loading => div()
                    .w(px(16.))
                    .text_color(t.accent)
                    .child("\u{25CC}")
                    .with_animation(
                        ("web-browser-spinner", i),
                        gpui::Animation::new(Duration::from_millis(900)).repeat(),
                        |el, delta| el.opacity(0.3 + 0.7 * (1. - (delta * 2. - 1.).abs())),
                    )
                    .into_any_element(),
                Some(image) => img(image).w(px(16.)).h(px(16.)).into_any_element(),
                None => div().w(px(16.)).into_any_element(),
            };
            let short: SharedString = if title.chars().count() > 28 {
                format!("{}\u{2026}", title.chars().take(27).collect::<String>()).into()
            } else {
                title.into()
            };
            let (select, close) = (target.clone(), target.clone());
            // A debugging session's tab (brief 0037): Visual Studio's green run glyph before its title.
            let glyph = self.tab_session(&target).is_some().then(|| {
                div()
                    .id(("web-browser-tab-debug", i))
                    .debug_selector(move || format!("web-browser-tab-debug-{i}"))
                    .text_color(rgb(0x388A34))
                    .child("\u{25B6}")
            });
            let tip = self.tab_tooltip(&target);
            let theme = self.theme;
            let mut tab = div()
                .id(("web-browser-tab", i))
                .debug_selector(move || format!("web-browser-tab-{i}"))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h_full()
                .px_2()
                .border_r_1()
                .border_color(t.border)
                .cursor_pointer()
                .text_size(t.typography.ui)
                .tooltip(move |_, cx| {
                    let text = tip.clone();
                    cx.new(|_| TabTip { text, theme }).into()
                })
                .child(icon)
                .children(glyph)
                .child(short)
                .child(
                    div()
                        .id(("web-browser-tab-close", i))
                        .debug_selector(move || format!("web-browser-tab-close-{i}"))
                        .px_1()
                        .text_color(t.text_muted)
                        .hover(|s| s.bg(t.menu_hover))
                        .child("\u{2715}")
                        .on_click(cx.listener(move |w, _, _, cx| {
                            cx.stop_propagation();
                            w.close_target(&close, cx)
                        })),
                )
                .on_click(
                    cx.listener(move |w, _, window, cx| w.select_target(&select, window, cx)),
                );
            tab = if selected {
                tab.bg(t.accent).text_color(t.text_on_accent)
            } else {
                tab.text_color(t.chrome_text).hover(|s| s.bg(t.menu_hover))
            };
            row = row.child(tab);
        }
        row.child(
            div()
                .id("web-browser-new-tab")
                .debug_selector(|| "web-browser-new-tab".into())
                .px_2()
                .cursor_pointer()
                .text_color(t.chrome_text)
                .hover(|s| s.bg(t.menu_hover))
                .child("+")
                .on_click(cx.listener(|w, _, _, cx| w.new_tab(cx))),
        )
        .into_any_element()
    }

    fn tool_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let t = self.theme;
        let el = div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .min_w(px(24.))
            .h(px(22.))
            .px_1()
            .text_size(t.typography.ui)
            .child(label);
        if !enabled {
            return el.text_color(t.text_disabled).into_any_element();
        }
        el.text_color(t.text)
            .cursor_pointer()
            .hover(|s| s.bg(t.menu_hover))
            .on_click(cx.listener(move |w, _, window, cx| f(w, window, cx)))
            .into_any_element()
    }

    fn render_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let (back, forward) = self.can_go();
        let loading = self
            .shown_target()
            .and_then(|x| self.info.get(&x))
            .is_some_and(|i| i.loading);
        let has_tab = self.shown_target().is_some();
        let focused = self.address_focus.is_focused(window);
        let address = text_box(
            "web-browser-address",
            &self.address,
            "Enter a URL (localhost:5000)",
            focused,
            &t,
        );
        // The whole address selected: drawn as a selection.
        let address = if focused && self.address_all && !self.address.is_empty() {
            address.bg(t.accent).text_color(t.text_on_accent)
        } else {
            address
        };
        let address = address
            .flex_1()
            .w_auto()
            .track_focus(&self.address_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|w, _, window, cx| {
                    if !w.address_focus.is_focused(window) {
                        w.focus_address(window, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(|w, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                match k.key.as_str() {
                    "enter" => {
                        w.address_all = false;
                        w.go(cx);
                        w.focus_page(window, cx);
                    }
                    _ if !k.modifiers.control && !k.modifiers.alt => {
                        w.address_key(&k.key, k.key_char.as_deref());
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }));
        div()
            .id("web-browser-toolbar")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(28.))
            .px_1()
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .child(
                self.tool_button("web-browser-back", "\u{2190}", back, cx, |w, _, cx| {
                    w.navigate_action("back", cx)
                }),
            )
            .child(self.tool_button(
                "web-browser-forward",
                "\u{2192}",
                forward,
                cx,
                |w, _, cx| w.navigate_action("forward", cx),
            ))
            .child(if loading {
                self.tool_button("web-browser-stop", "\u{2715}", true, cx, |w, _, cx| {
                    w.navigate_action("stop", cx)
                })
            } else {
                self.tool_button("web-browser-reload", "\u{21BB}", has_tab, cx, |w, _, cx| {
                    w.navigate_action("reload", cx)
                })
            })
            .child(
                self.tool_button("web-browser-home", "\u{2302}", true, cx, |w, _, cx| {
                    w.home(cx)
                }),
            )
            .child(address)
            .child(self.tool_button("web-browser-go", "Go", true, cx, |w, _, cx| w.go(cx)))
            .child(self.tool_button(
                "web-browser-history",
                "\u{25BE}",
                !self.history.is_empty(),
                cx,
                |w, _, cx| {
                    w.history_open = !w.history_open;
                    cx.notify();
                },
            ))
            .child(self.tool_button(
                "web-browser-devtools",
                "DevTools",
                has_tab,
                cx,
                |w, _, cx| w.open_devtools(None, cx),
            ))
            .into_any_element()
    }

    fn render_agent_strip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.driving.is_empty() {
            return None;
        }
        let t = self.theme;
        let who = self.driving.join(", ");
        let text = if self.driving.len() == 1 {
            format!("Agent {who} is driving this browser")
        } else {
            format!("Agents {who} are driving this browser")
        };
        Some(
            div()
                .id("web-browser-agent-strip")
                .debug_selector(|| "web-browser-agent-strip".into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                // Visual Studio's yellow info bar.
                .bg(rgb(0xFFF29D))
                .text_color(rgb(0x1E1E1E))
                .text_size(t.typography.ui)
                .child(div().font_weight(FontWeight::SEMIBOLD).child("\u{25B6}"))
                .child(div().flex_1().child(text))
                .child(
                    push_button("web-browser-agent-stop", "Stop", false, true, &t)
                        .on_click(cx.listener(|w, _, _, cx| w.stop_agent(cx))),
                )
                .into_any_element(),
        )
    }

    fn render_sandbox_strip(&self) -> Option<AnyElement> {
        if !self.no_sandbox {
            return None;
        }
        let t = self.theme;
        Some(
            div()
                .id("web-browser-sandbox-strip")
                .debug_selector(|| "web-browser-sandbox-strip".into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                // Visual Studio's yellow info bar, as the agent strip's.
                .bg(rgb(0xFFF29D))
                .text_color(rgb(0x1E1E1E))
                .text_size(t.typography.ui)
                .child(div().font_weight(FontWeight::SEMIBOLD).child("\u{26A0}"))
                .child(div().flex_1().child(NO_SANDBOX_STRIP))
                .child(
                    div()
                        .text_size(t.typography.small)
                        .child("Tools > Options > Web Browser turns it off"),
                )
                .into_any_element(),
        )
    }

    /// The workspace's file carries the opt-in, which is ignored (brief 0047).
    fn render_opt_in_ignored(&self) -> Option<AnyElement> {
        if !self.opt_in_ignored {
            return None;
        }
        let t = self.theme;
        Some(
            div()
                .id("web-browser-opt-in-ignored")
                .debug_selector(|| "web-browser-opt-in-ignored".into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .bg(rgb(0xFFF29D))
                .text_color(rgb(0x1E1E1E))
                .text_size(t.typography.ui)
                .child(div().font_weight(FontWeight::SEMIBOLD).child("\u{26A0}"))
                .child(div().flex_1().child(OPT_IN_IGNORED))
                .into_any_element(),
        )
    }

    fn render_sandbox_prompt(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.sandbox_prompt.clone()?;
        let t = self.theme;
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .max_w(px(560.))
            .child(div().child(
                "The Web Browser runs pages in Chromium's sandbox, which cannot start here. To run it sandboxed, do \
                 one of these and open the Web Browser again:",
            ));
        for (i, r) in p.remedies().into_iter().enumerate() {
            body = body.child(
                div()
                    .id(("web-browser-sandbox-remedy", i))
                    .pl_2()
                    .child(format!("\u{2022} {r}")),
            );
        }
        body = body
            .child(
                div().flex().flex_row().child(
                    check_box(
                        "web-browser-sandbox-check",
                        "Run without the sandbox for this workspace",
                        p.checked,
                        &t,
                    )
                    .on_click(cx.listener(|w, _, _, cx| w.toggle_sandbox_check(cx))),
                ),
            )
            .child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(
                        "Pages then run without Chromium's protection. Eludite remembers this in the workspace's \
                         settings; Tools > Options > Web Browser turns it off.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        push_button("web-browser-sandbox-ok", "OK", true, true, &t)
                            .on_click(cx.listener(|w, _, _, cx| w.answer_sandbox(true, cx))),
                    )
                    .child(
                        push_button("web-browser-sandbox-cancel", "Cancel", false, true, &t)
                            .on_click(cx.listener(|w, _, _, cx| w.answer_sandbox(false, cx))),
                    ),
            );
        let panel = dialog_panel(&t, SANDBOX_TITLE)
            .id("web-browser-sandbox")
            .debug_selector(|| "web-browser-sandbox".into())
            .track_focus(&self.sandbox_focus)
            .on_key_down(cx.listener(|w, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "enter" => w.answer_sandbox(true, cx),
                    "escape" => w.answer_sandbox(false, cx),
                    // The check box, as Space toggles a focused one in Visual Studio's dialogs.
                    "space" => w.toggle_sandbox_check(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(body);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let muted = |text: String| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .bg(t.background)
                .text_color(t.text_muted)
                .child(text)
                .into_any_element()
        };
        if let Some(m) = &self.message {
            return div()
                .id("web-browser-message")
                .debug_selector(|| "web-browser-message".into())
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .bg(t.background)
                .text_color(t.text)
                .child(
                    div()
                        .text_size(px(16.))
                        .child("The Web Browser cannot show pages here"),
                )
                .child(m.clone())
                .into_any_element();
        }
        let Some(target) = self.shown_target() else {
            return muted(if self.in_flight > 0 {
                "Starting the browser\u{2026}".into()
            } else {
                "No tab is open. Press Ctrl+T or + to open one.".into()
            });
        };
        match self.surface_for(&target, cx) {
            Some(s) => div().size_full().child(s).into_any_element(),
            None => muted("Loading\u{2026}".into()),
        }
    }

    fn render_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ix = self.shown_prompt_index()?;
        let p = self.prompts[ix].clone();
        let t = self.theme;
        let focused = self.prompt_focus.is_focused(window);
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .max_w(px(520.))
            .child(div().child(p.message.clone()));
        if p.kind == "prompt" || p.kind == "auth" {
            body = body.child(
                text_box(
                    "web-browser-prompt-text",
                    &p.text,
                    if p.kind == "auth" { "User name" } else { "" },
                    focused && !p.on_password,
                    &t,
                )
                .w(px(400.)),
            );
        }
        if p.kind == "auth" {
            let dots = "\u{2022}".repeat(p.password.chars().count());
            body = body.child(
                text_box(
                    "web-browser-prompt-password",
                    &dots,
                    "Password",
                    focused && p.on_password,
                    &t,
                )
                .w(px(400.)),
            );
        }
        let (ok, cancel) = match p.kind.as_str() {
            "alert" => ("OK", None),
            "permission" => ("Allow", Some("Block")),
            "beforeunload" => ("Leave", Some("Stay")),
            "file" => ("Choose...", Some("Cancel")),
            "auth" => ("Sign In", Some("Cancel")),
            _ => ("OK", Some("Cancel")),
        };
        let file = p.kind == "file";
        let mut buttons = div().flex().flex_row().justify_end().gap_2().child(
            push_button("web-browser-prompt-ok", ok, true, true, &t).on_click(cx.listener(
                move |w, _, window, cx| {
                    if file {
                        w.choose_files(window, cx)
                    } else {
                        w.answer_prompt(true, Vec::new(), cx)
                    }
                },
            )),
        );
        if let Some(c) = cancel {
            buttons = buttons.child(
                push_button("web-browser-prompt-cancel", c, false, true, &t)
                    .on_click(cx.listener(|w, _, _, cx| w.answer_prompt(false, Vec::new(), cx))),
            );
        }
        let panel = dialog_panel(&t, p.title())
            .id("web-browser-prompt")
            .debug_selector(|| "web-browser-prompt".into())
            .track_focus(&self.prompt_focus)
            .on_key_down(cx.listener(move |w, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                let Some(ix) = w.shown_prompt_index() else {
                    return;
                };
                match k.key.as_str() {
                    "enter" if !file => w.answer_prompt(true, Vec::new(), cx),
                    "escape" => w.answer_prompt(false, Vec::new(), cx),
                    "tab" => {
                        let p = &mut w.prompts[ix];
                        p.on_password = p.kind == "auth" && !p.on_password;
                    }
                    "backspace" => {
                        let p = &mut w.prompts[ix];
                        if p.on_password {
                            p.password.pop();
                        } else {
                            p.text.pop();
                        }
                    }
                    _ => {
                        let p = &mut w.prompts[ix];
                        if (p.kind == "prompt" || p.kind == "auth")
                            && !k.modifiers.control
                            && let Some(c) = k.key_char.as_deref()
                        {
                            if p.on_password {
                                p.password.push_str(c);
                            } else {
                                p.text.push_str(c);
                            }
                        } else {
                            return;
                        }
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(body.child(buttons));
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.25))
                .occlude()
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.menu.clone()?;
        let t = self.theme;
        let rows = MENU_ITEMS.iter().enumerate().map(|(i, label)| {
            let enabled = self.menu_enabled(&m, label);
            let el = div()
                .id(("web-browser-menu-item", i))
                .debug_selector(move || format!("web-browser-menu-{label}"))
                .h(px(22.))
                .px_3()
                .flex()
                .items_center()
                .child(*label);
            let el = if enabled {
                el.text_color(t.menu_text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .on_click(cx.listener(move |w, _, _, cx| w.menu_item(label, cx)))
            } else {
                el.text_color(t.text_disabled)
            };
            let sep = matches!(*label, "Copy" | "Save Image As..." | "Inspect");
            div()
                .children(sep.then(|| div().h(px(1.)).my_1().mx_2().bg(t.popup_border)))
                .child(el)
        });
        // The page's point, inside the content area (the strip and the toolbar are above it).
        let top = 26. + 28. + if self.driving.is_empty() { 0. } else { 28. };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|w, _, _, cx| {
                        w.menu = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("web-browser-menu")
                        .debug_selector(|| "web-browser-menu".into())
                        .absolute()
                        .left(px(m.x))
                        .top(px(m.y + top))
                        .min_w(px(200.))
                        .py_1()
                        .bg(t.popup_background)
                        .border_1()
                        .border_color(t.popup_border)
                        .shadow_md()
                        .text_size(t.typography.ui)
                        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                            cx.stop_propagation()
                        })
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.history_open || self.history.is_empty() {
            return None;
        }
        let t = self.theme;
        let rows = self
            .history
            .clone()
            .into_iter()
            .enumerate()
            .map(|(i, url)| {
                let go = url.clone();
                div()
                    .id(("web-browser-history-row", i))
                    .h(px(22.))
                    .px_2()
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .child(url)
                    .on_click(cx.listener(move |w, _, _, cx| {
                        w.history_open = false;
                        w.address = go.clone();
                        w.address_edited = false;
                        w.go_to(go.clone(), cx);
                    }))
            });
        Some(
            div()
                .id("web-browser-history-list")
                .debug_selector(|| "web-browser-history-list".into())
                .absolute()
                .top(px(26. + 28.))
                .left(px(120.))
                .w(px(480.))
                .py_1()
                .bg(t.popup_background)
                .border_1()
                .border_color(t.popup_border)
                .shadow_md()
                .text_size(t.typography.ui)
                .children(rows)
                .into_any_element(),
        )
    }
}

impl Focusable for BrowserWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BrowserWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        if std::mem::take(&mut self.refocus) && self.open {
            if self.prompts.is_empty() {
                self.focus_page(window, cx);
            } else {
                window.focus(&self.prompt_focus, cx);
            }
        }
        let status = self
            .shown_target()
            .and_then(|x| self.info.get(&x))
            .map(|i| i.status.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.status.clone());
        let strip = self.render_strip(cx);
        let toolbar = self.render_toolbar(window, cx);
        let agent = self.render_agent_strip(cx);
        let sandbox_strip = self.render_sandbox_strip();
        let opt_in_ignored = self.render_opt_in_ignored();
        let content = self.render_content(cx);
        let prompt = self.render_prompt(window, cx);
        let sandbox_prompt = self.render_sandbox_prompt(cx);
        let menu = self.render_menu(cx);
        let history = self.render_history(cx);
        div()
            .id("web-browser")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|w, _: &FocusAddressBar, window, cx| w.focus_address(window, cx)),
            )
            .on_action(cx.listener(|w, _: &Reload, _, cx| w.navigate_action("reload", cx)))
            .on_action(cx.listener(|w, _: &Back, _, cx| w.navigate_action("back", cx)))
            .on_action(cx.listener(|w, _: &Forward, _, cx| w.navigate_action("forward", cx)))
            .on_action(cx.listener(|w, _: &NewTab, _, cx| w.new_tab(cx)))
            .on_action(cx.listener(|w, _: &CloseTab, _, cx| w.close_shown(cx)))
            .on_action(cx.listener(|w, _: &OpenDevTools, _, cx| w.open_devtools(None, cx)))
            .on_action(cx.listener(|w, _: &StopLoading, window, cx| {
                // Escape is the sandbox dialog's Cancel while it shows (brief 0039).
                if w.sandbox_prompt.is_some() {
                    w.answer_sandbox(false, cx);
                } else if w.address_focus.is_focused(window) {
                    w.address_edited = false;
                    w.sync_address();
                    w.focus_page(window, cx);
                } else if w.menu.take().is_some() || std::mem::take(&mut w.history_open) {
                } else if w
                    .shown_target()
                    .and_then(|x| w.info.get(&x))
                    .is_some_and(|i| i.loading)
                {
                    w.navigate_action("stop", cx);
                } else if let (Some(d), Some(x)) = (w.driver.clone(), w.shown_target()) {
                    // Not loading: the page gets its Escape.
                    for ty in ["rawKeyDown", "keyUp"] {
                        d.input(
                            &x,
                            json!({"type": ty, "windowsKeyCode": 27, "nativeKeyCode": 9}),
                        );
                    }
                }
                cx.notify();
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(strip)
            .child(toolbar)
            .children(agent)
            .children(sandbox_strip)
            .children(opt_in_ignored)
            .child(div().flex_1().min_h_0().child(content))
            .child(
                div()
                    .flex_none()
                    .h(px(20.))
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(status),
            )
            .children(history)
            .children(menu)
            .children(prompt)
            .children(sandbox_prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_addresses_become_urls() {
        assert_eq!(address_url("localhost:5000"), "http://localhost:5000");
        assert_eq!(address_url(" example.com/a "), "http://example.com/a");
        assert_eq!(address_url("https://x.org"), "https://x.org");
        assert_eq!(address_url("about:blank"), "about:blank");
        assert_eq!(address_url("[::1]:8080/"), "http://[::1]:8080/");
        assert_eq!(address_url("hello"), "hello");
        let png = "data:image/png;base64,iVBORw0KGgo=";
        assert!(favicon_image(png).is_some());
        assert!(favicon_image("data:text/plain,hi").is_none());
        assert_eq!(
            Prompt {
                target: "1".into(),
                id: 1,
                kind: "permission".into(),
                message: String::new(),
                text: String::new(),
                password: String::new(),
                on_password: false,
                mode: String::new(),
            }
            .title(),
            "Permission Request"
        );
    }
}
