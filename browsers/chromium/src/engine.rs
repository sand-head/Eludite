//! CEF's browser process (feature `cef`, Linux for now): the app, one windowless browser per tab, the render handler
//! writing `OnPaint` frames into the tab's ring, the DevTools pass-through, input, and the control loop.
//!
//! Threads: CEF's UI thread is the process's main thread (`CefRunMessageLoop`); every request from stdin is posted to
//! it as a task, and every CEF callback used here runs on it, so the tabs live in a thread-local map. The reader
//! thread only parses and posts; the writer thread (`rpc::Out`) owns the protocol output, so neither `OnPaint` nor a
//! handler ever blocks on the pipe.

// CEF's C API: raw buffers from `OnPaint`, descriptor duplication and environment changes before threads start.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cef::*;
use serde_json::{Value, json};

use crate::rpc::{self, Incoming, Out};
use crate::sandbox::{self, Decision};
use crate::shm::{self, Region, Written};
use crate::{NAME, Options};

static OUT: OnceLock<Out> = OnceLock::new();
static FRAME_SOCKET: AtomicI32 = AtomicI32::new(-1);
static NEXT_REGION: AtomicU64 = AtomicU64::new(1);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static SANDBOXED: AtomicBool = AtomicBool::new(true);

fn out() -> &'static Out {
    OUT.get().expect("the writer starts before CEF")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn cef_version() -> String {
    String::from_utf8_lossy(sys::CEF_VERSION)
        .trim_end_matches('\0')
        .to_owned()
}

fn chromium_version() -> String {
    format!(
        "{}.{}.{}.{}",
        sys::CHROME_VERSION_MAJOR,
        sys::CHROME_VERSION_MINOR,
        sys::CHROME_VERSION_BUILD,
        sys::CHROME_VERSION_PATCH
    )
}

// ---- posting work to the UI thread ----

type JobFn = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

wrap_task! {
    struct Job {
        f: JobFn,
    }

    impl Task {
        fn execute(&self) {
            let f = lock(&self.f).take();
            if let Some(f) = f {
                f();
            }
        }
    }
}

fn on_ui(f: impl FnOnce() + Send + 'static) {
    let mut task = Job::new(Arc::new(Mutex::new(Some(Box::new(f)))));
    post_task(ThreadId::UI, Some(&mut task));
}

fn on_ui_after(ms: i64, f: impl FnOnce() + Send + 'static) {
    let mut task = Job::new(Arc::new(Mutex::new(Some(Box::new(f)))));
    post_delayed_task(ThreadId::UI, Some(&mut task), ms);
}

// ---- tabs ----

/// What a tab's handlers share (they may be called while the map is borrowed, so nothing here is in it).
struct TabShared {
    id: String,
    /// CSS width, height and the device scale factor.
    geometry: Mutex<(i32, i32, f32)>,
    ring: Mutex<Option<Region>>,
    sequence: AtomicU64,
    dropped: AtomicU64,
}

impl TabShared {
    fn device_size(&self) -> (u32, u32) {
        let (w, h, s) = *lock(&self.geometry);
        (
            (w as f32 * s).ceil().max(1.) as u32,
            (h as f32 * s).ceil().max(1.) as u32,
        )
    }

    /// Make a region for frames of `width` by `height`, pass its descriptor and announce it.
    fn new_region(&self, width: u32, height: u32) -> Option<Region> {
        let id = NEXT_REGION.fetch_add(1, Ordering::Relaxed);
        let region = match Region::create(id, width, height) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{NAME}: tab {}: shared memory: {e}", self.id);
                return None;
            }
        };
        let sock = FRAME_SOCKET.load(Ordering::Relaxed);
        if let Err(e) = shm::send_fd(sock, region.fd(), id) {
            eprintln!(
                "{NAME}: tab {}: passing region {id} over fd {sock}: {e}",
                self.id
            );
        }
        out().notify(
            "tab/resized",
            json!({
                "tab": self.id,
                "region": {
                    "id": id,
                    "size": region.size(),
                    "headerSize": shm::HEADER_SIZE,
                    "slotSize": region.slot_size,
                    "slotCount": shm::SLOTS,
                },
                "width": width,
                "height": height,
            }),
        );
        Some(region)
    }
}

struct TabEntry {
    browser: Option<Browser>,
    shared: Arc<TabShared>,
    _devtools: Option<Registration>,
}

#[derive(Default)]
struct Tabs {
    next: u64,
    map: HashMap<String, TabEntry>,
    /// The `shutdown` request waiting for the last tab to close.
    shutdown_reply: Option<Value>,
}

thread_local! {
    // The UI thread's tabs: requests and CEF callbacks all run there.
    static TABS: RefCell<Tabs> = RefCell::new(Tabs::default());
}

fn browser_of(tab: &str) -> Option<Browser> {
    TABS.with_borrow(|t| t.map.get(tab).and_then(|e| e.browser.clone()))
}

fn host_of(tab: &str) -> Option<BrowserHost> {
    browser_of(tab).and_then(|b| b.host())
}

fn shared_of(tab: &str) -> Option<Arc<TabShared>> {
    TABS.with_borrow(|t| t.map.get(tab).map(|e| e.shared.clone()))
}

// ---- CEF handlers ----

wrap_render_handler! {
    struct TabRender {
        tab: Arc<TabShared>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (w, h, _) = *lock(&self.tab.geometry);
                rect.x = 0;
                rect.y = 0;
                rect.width = w.max(1);
                rect.height = h.max(1);
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(info) = screen_info else {
                return 0;
            };
            let (w, h, s) = *lock(&self.tab.geometry);
            info.device_scale_factor = s;
            info.depth = 24;
            info.depth_per_component = 8;
            let r = Rect { x: 0, y: 0, width: w.max(1), height: h.max(1) };
            info.rect = r.clone();
            info.available_rect = r;
            1
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            paint(&self.tab, type_, dirty_rects, buffer, width, height);
        }
    }
}

fn paint(
    tab: &TabShared,
    type_: PaintElementType,
    dirty_rects: Option<&[Rect]>,
    buffer: *const u8,
    width: i32,
    height: i32,
) {
    let paint_ns = shm::monotonic_ns();
    // Popups (a <select>'s list) are a second element CEF paints separately; the spike draws the view only.
    if type_ != PaintElementType::VIEW || buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let (w, h) = (width as u32, height as u32);
    // SAFETY: CEF's OnPaint buffer is width * height * 4 bytes of BGRA for the duration of the call.
    let pixels = unsafe { std::slice::from_raw_parts(buffer, w as usize * h as usize * 4) };
    let dirty: Vec<shm::Rect> = dirty_rects
        .unwrap_or_default()
        .iter()
        .map(|r| shm::Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        })
        .collect();
    let mut ring = lock(&tab.ring);
    if !ring.as_ref().is_some_and(|r| r.fits(w, h)) {
        *ring = tab.new_region(w, h);
    }
    let Some(region) = ring.as_ref() else {
        return;
    };
    let sequence = tab.sequence.fetch_add(1, Ordering::Relaxed) + 1;
    match region.write(pixels, w, h, sequence, paint_ns, &dirty) {
        Written::Slot { slot, copy_ns } => out().notify(
            "tab/frame",
            json!({
                "tab": tab.id,
                "region": region.id,
                "slot": slot,
                "sequence": sequence,
                "width": w,
                "height": h,
                "dirty": shm::clamp_dirty(&dirty)
                    .iter()
                    .map(|r| json!({"x": r.x, "y": r.y, "width": r.width, "height": r.height}))
                    .collect::<Vec<_>>(),
                "paintNs": paint_ns,
                "copyNs": copy_ns,
            }),
        ),
        Written::Dropped => {
            tab.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

wrap_life_span_handler! {
    struct TabLife {
        tab: Arc<TabShared>,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // No popup windows: a window.open or target=_blank loads in the tab (brief B decides tabs).
            if let (Some(browser), Some(url)) = (browser, target_url)
                && let Some(frame) = browser.main_frame()
            {
                frame.load_url(Some(url));
            }
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            closed(&self.tab.id, "closed");
        }
    }
}

fn closed(tab: &str, reason: &str) {
    let removed = TABS.with_borrow_mut(|t| t.map.remove(tab));
    if removed.is_none() {
        return;
    }
    out().notify("tab/closed", json!({"tab": tab, "reason": reason}));
    drop(removed);
    maybe_finish_shutdown();
}

wrap_display_handler! {
    struct TabDisplay {
        tab: Arc<TabShared>,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() == 1) {
                let url = url.map(ToString::to_string).unwrap_or_default();
                out().notify("tab/state", json!({"tab": self.tab.id, "url": url}));
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(ToString::to_string).unwrap_or_default();
            out().notify("tab/state", json!({"tab": self.tab.id, "title": title}));
        }
    }
}

wrap_load_handler! {
    struct TabLoad {
        tab: Arc<TabShared>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            _can_go_back: ::std::os::raw::c_int,
            _can_go_forward: ::std::os::raw::c_int,
        ) {
            out().notify("tab/state", json!({"tab": self.tab.id, "loading": is_loading != 0}));
        }
    }
}

wrap_request_handler! {
    struct TabRequests {
        tab: Arc<TabShared>,
    }

    impl RequestHandler {
        fn on_render_process_terminated(
            &self,
            browser: Option<&mut Browser>,
            _status: TerminationStatus,
            error_code: ::std::os::raw::c_int,
            error_string: Option<&CefString>,
        ) {
            eprintln!(
                "{NAME}: tab {}: the renderer exited ({error_code}: {})",
                self.tab.id,
                error_string.map(ToString::to_string).unwrap_or_default()
            );
            // The tab is lost; the shell opens a new one if it wants (proposal 0002: a crash loses tabs).
            let id = self.tab.id.clone();
            out().notify("tab/closed", json!({"tab": id, "reason": "crashed"}));
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.close_browser(1);
            }
            TABS.with_borrow_mut(|t| t.map.remove(&id));
            maybe_finish_shutdown();
        }
    }
}

wrap_client! {
    struct TabClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        display: DisplayHandler,
        load: LoadHandler,
        requests: RequestHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.requests.clone())
        }
    }
}

wrap_dev_tools_message_observer! {
    struct TabDevTools {
        tab: Arc<str>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            if let Some(m) = message {
                out().cdp_event(&self.tab, m);
            }
            // Handled: CEF does not split it into OnDevToolsMethodResult and OnDevToolsEvent.
            1
        }
    }
}

wrap_app! {
    struct EngineApp {
        gpu: bool,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefStringUtf16>,
            command_line: Option<&mut CommandLine>,
        ) {
            let browser_process = process_type.is_none_or(|t| t.to_string().is_empty());
            let Some(cl) = command_line else {
                return;
            };
            if !browser_process {
                return;
            }
            let has_display = ["DISPLAY", "WAYLAND_DISPLAY"]
                .iter()
                .any(|v| std::env::var_os(v).is_some_and(|d| !d.is_empty()));
            for switch in browser_switches(self.gpu, has_display) {
                match switch.split_once('=') {
                    Some((k, v)) => cl.append_switch_with_value(
                        Some(&CefString::from(k)),
                        Some(&CefString::from(v)),
                    ),
                    None => cl.append_switch(Some(&CefString::from(switch))),
                }
            }
        }
    }
}

/// The browser process's switches. Software compositing unless `ELUDITE_CHROMIUM_GPU=1`: off-screen frames come back
/// through `OnPaint` either way, and without a GPU (or under Xvfb) the GPU process only adds a copy.
pub fn browser_switches(gpu: bool, has_display: bool) -> Vec<&'static str> {
    let mut s = vec![
        "enable-logging=stderr",
        "disable-background-networking",
        "disable-component-update",
        "disable-default-apps",
        "disable-sync",
        "no-first-run",
        "no-default-browser-check",
        "noerrdialogs",
        // No desktop keyring prompt for a profile that keeps no passwords of the user's.
        "password-store=basic",
        // Chrome's background services CEF 154 keeps: fewer requests to Google at startup (brief 0031 found
        // network time, which these stop, and four more endpoints they do not; the report lists them).
        "no-pings",
        "no-service-autorun",
        "disable-breakpad",
        "disable-client-side-phishing-detection",
        "disable-domain-reliability",
        "disable-field-trial-config",
        "disable-search-engine-choice-screen",
        "metrics-recording-only",
        "disable-features=NetworkTimeServiceQuerying,OptimizationHints,MediaRouter,DialMediaRouteProvider,\
         Translate,CertificateTransparencyComponentUpdater,LensOverlay,AutofillServerCommunication",
    ];
    if !gpu {
        s.extend(["disable-gpu", "disable-gpu-compositing"]);
    }
    // Windowless tabs need no display, but Chromium's default Ozone platform (X11 or Wayland) refuses to start
    // without one. With neither, use the headless platform: rendering is the same; the system clipboard is lost.
    if !has_display {
        s.push("ozone-platform=headless");
    }
    s
}

// ---- requests ----

fn reply(id: &Option<Value>, r: Result<Value, (i64, String)>) {
    if let Some(id) = id {
        match r {
            Ok(v) => out().result(id, v),
            Err((code, msg)) => out().error(id, code, &msg),
        }
    }
}

fn tab_param(p: &Value) -> Result<String, (i64, String)> {
    p["tab"]
        .as_str()
        .map(str::to_owned)
        .ok_or((rpc::INVALID_PARAMS, "missing tab".into()))
}

fn no_tab(tab: &str) -> (i64, String) {
    (rpc::NO_SUCH_TAB, format!("no tab {tab}"))
}

fn dimension(p: &Value, name: &str) -> Result<i32, (i64, String)> {
    p[name]
        .as_i64()
        .filter(|v| (1..=16384).contains(v))
        .map(|v| v as i32)
        .ok_or((
            rpc::INVALID_PARAMS,
            format!("{name} must be 1 to 16384 CSS pixels"),
        ))
}

fn handle(m: Incoming) {
    let p = &m.params;
    let r = match m.method.as_str() {
        "initialize" => Ok(json!({
            "engineName": NAME,
            "engineVersion": env!("CARGO_PKG_VERSION"),
            "protocolVersion": rpc::PROTOCOL_VERSION,
            "cefVersion": cef_version(),
            "chromiumVersion": chromium_version(),
            "pid": std::process::id(),
            "sandbox": SANDBOXED.load(Ordering::Relaxed),
            "frameTransport": "memfd+scm_rights",
        })),
        "tab/create" => create_tab(p),
        "tab/close" => tab_param(p).and_then(|tab| {
            let host = host_of(&tab).ok_or_else(|| no_tab(&tab))?;
            host.close_browser(1);
            Ok(json!({}))
        }),
        "tab/resize" => resize_tab(p),
        "tab/navigate" => tab_param(p).and_then(|tab| {
            let url = p["url"]
                .as_str()
                .ok_or((rpc::INVALID_PARAMS, "missing url".into()))?;
            let frame = browser_of(&tab)
                .and_then(|b| b.main_frame())
                .ok_or_else(|| no_tab(&tab))?;
            frame.load_url(Some(&CefString::from(url)));
            Ok(json!({}))
        }),
        "tab/input" => {
            if let Ok(tab) = tab_param(p)
                && let Some(host) = host_of(&tab)
            {
                input(&host, &p["event"]);
            }
            return;
        }
        "tab/cdp" => {
            cdp(p);
            return;
        }
        "shutdown" => {
            begin_shutdown(m.id);
            return;
        }
        other => Err((rpc::METHOD_NOT_FOUND, format!("unknown method {other}"))),
    };
    reply(&m.id, r);
}

fn create_tab(p: &Value) -> Result<Value, (i64, String)> {
    let url = p["url"].as_str().unwrap_or("about:blank").to_owned();
    let width = dimension(p, "width")?;
    let height = dimension(p, "height")?;
    let scale = p["scale"].as_f64().unwrap_or(1.).clamp(0.25, 8.) as f32;
    let rate = p["frameRate"].as_i64().unwrap_or(60).clamp(1, 240) as i32;
    let id = TABS.with_borrow_mut(|t| {
        t.next += 1;
        t.next.to_string()
    });
    let shared = Arc::new(TabShared {
        id: id.clone(),
        geometry: Mutex::new((width, height, scale)),
        ring: Mutex::new(None),
        sequence: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
    });
    let (dw, dh) = shared.device_size();
    let region = shared
        .new_region(dw, dh)
        .ok_or((rpc::INTERNAL_ERROR, "no shared memory for the tab".into()))?;
    *lock(&shared.ring) = Some(region);
    let window_info = WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 0,
        external_begin_frame_enabled: 0,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    };
    let settings = BrowserSettings {
        windowless_frame_rate: rate,
        // Opaque white until the page paints, as a browser shows.
        background_color: 0xFFFF_FFFF,
        ..Default::default()
    };
    let mut client = TabClient::new(
        TabRender::new(shared.clone()),
        TabLife::new(shared.clone()),
        TabDisplay::new(shared.clone()),
        TabLoad::new(shared.clone()),
        TabRequests::new(shared.clone()),
    );
    // Registered before the browser exists, so the callbacks of its creation find the tab.
    TABS.with_borrow_mut(|t| {
        t.map.insert(
            id.clone(),
            TabEntry {
                browser: None,
                shared: shared.clone(),
                _devtools: None,
            },
        )
    });
    let Some(browser) = browser_host_create_browser_sync(
        Some(&window_info),
        Some(&mut client),
        Some(&CefString::from(url.as_str())),
        Some(&settings),
        None,
        None,
    ) else {
        TABS.with_borrow_mut(|t| t.map.remove(&id));
        return Err((rpc::INTERNAL_ERROR, "CEF did not create the browser".into()));
    };
    let devtools = browser.host().and_then(|host| {
        host.was_hidden(0);
        host.set_focus(1);
        let mut observer = TabDevTools::new(Arc::from(id.as_str()));
        host.add_dev_tools_message_observer(Some(&mut observer))
    });
    TABS.with_borrow_mut(|t| {
        if let Some(e) = t.map.get_mut(&id) {
            e.browser = Some(browser);
            e._devtools = devtools;
        }
    });
    Ok(json!({"tab": id}))
}

fn resize_tab(p: &Value) -> Result<Value, (i64, String)> {
    let tab = tab_param(p)?;
    let width = dimension(p, "width")?;
    let height = dimension(p, "height")?;
    let shared = shared_of(&tab).ok_or_else(|| no_tab(&tab))?;
    let scale_changed = {
        let mut g = lock(&shared.geometry);
        let scale = p["scale"]
            .as_f64()
            .map_or(g.2, |s| s.clamp(0.25, 8.) as f32);
        let changed = scale != g.2;
        *g = (width, height, scale);
        changed
    };
    if let Some(host) = host_of(&tab) {
        if scale_changed {
            host.notify_screen_info_changed();
        }
        host.was_resized();
    }
    Ok(json!({}))
}

fn cdp(p: &Value) {
    let Ok(tab) = tab_param(p) else {
        return;
    };
    let message = &p["message"];
    match host_of(&tab) {
        Some(host) => {
            let bytes = serde_json::to_vec(message).unwrap_or_default();
            if host.send_dev_tools_message(Some(&bytes)) != 1 {
                cdp_error(&tab, message, "DevTools did not accept the message");
            }
        }
        None => cdp_error(&tab, message, &format!("no tab {tab}")),
    }
}

fn cdp_error(tab: &str, message: &Value, text: &str) {
    let body = json!({"id": message["id"], "error": {"code": -32000, "message": text}});
    out().cdp_event(tab, &serde_json::to_vec(&body).unwrap_or_default());
}

fn mouse(e: &Value) -> MouseEvent {
    MouseEvent {
        x: e["x"].as_i64().unwrap_or(0) as i32,
        y: e["y"].as_i64().unwrap_or(0) as i32,
        modifiers: e["modifiers"].as_u64().unwrap_or(0) as u32,
    }
}

fn button(e: &Value) -> MouseButtonType {
    match e["button"].as_str() {
        Some("middle") => MouseButtonType::MIDDLE,
        Some("right") => MouseButtonType::RIGHT,
        _ => MouseButtonType::LEFT,
    }
}

/// One `tab/input` event (browser-rpc.md, Input).
fn input(host: &BrowserHost, e: &Value) {
    let clicks = e["clickCount"].as_i64().unwrap_or(1).clamp(1, 3) as i32;
    match e["type"].as_str().unwrap_or_default() {
        "mouseMove" => host.send_mouse_move_event(
            Some(&mouse(e)),
            e["leave"].as_bool().unwrap_or(false) as i32,
        ),
        "mouseDown" => host.send_mouse_click_event(Some(&mouse(e)), button(e), 0, clicks),
        "mouseUp" => host.send_mouse_click_event(Some(&mouse(e)), button(e), 1, clicks),
        "wheel" => host.send_mouse_wheel_event(
            Some(&mouse(e)),
            e["deltaX"].as_i64().unwrap_or(0) as i32,
            e["deltaY"].as_i64().unwrap_or(0) as i32,
        ),
        t @ ("keyDown" | "rawKeyDown" | "keyUp" | "char") => {
            let code = e["windowsKeyCode"].as_i64().unwrap_or(0) as i32;
            let character: u16 = e["character"]
                .as_str()
                .and_then(|s| s.encode_utf16().next())
                .unwrap_or(0);
            let event = KeyEvent {
                type_: match t {
                    "keyUp" => KeyEventType::KEYUP,
                    "char" => KeyEventType::CHAR,
                    _ => KeyEventType::RAWKEYDOWN,
                },
                modifiers: e["modifiers"].as_u64().unwrap_or(0) as u32,
                windows_key_code: if t == "char" && code == 0 {
                    i32::from(character)
                } else {
                    code
                },
                native_key_code: e["nativeKeyCode"].as_i64().unwrap_or(0) as i32,
                is_system_key: e["isSystemKey"].as_bool().unwrap_or(false) as i32,
                character,
                unmodified_character: character,
                ..Default::default()
            };
            host.send_key_event(Some(&event));
        }
        "imeSetComposition" => {
            let text = CefString::from(e["text"].as_str().unwrap_or_default());
            let start = e["selectionStart"].as_u64().unwrap_or(0) as u32;
            let end = e["selectionEnd"].as_u64().unwrap_or(start as u64) as u32;
            let selection = Range {
                from: start,
                to: end,
            };
            host.ime_set_composition(Some(&text), None, None, Some(&selection));
        }
        "imeCommitText" => {
            let text = CefString::from(e["text"].as_str().unwrap_or_default());
            host.ime_commit_text(
                Some(&text),
                None,
                e["relativeCursorPos"].as_i64().unwrap_or(0) as i32,
            );
        }
        "imeFinishComposing" => {
            host.ime_finish_composing_text(e["keepSelection"].as_bool().unwrap_or(false) as i32)
        }
        "imeCancelComposition" => host.ime_cancel_composition(),
        "focus" => host.set_focus(e["focused"].as_bool().unwrap_or(true) as i32),
        _ => {}
    }
}

// ---- shutdown ----

fn begin_shutdown(reply_id: Option<Value>) {
    if SHUTTING_DOWN.swap(true, Ordering::SeqCst) {
        if let Some(id) = reply_id {
            out().result(&id, json!({}));
        }
        return;
    }
    let hosts: Vec<BrowserHost> = TABS.with_borrow_mut(|t| {
        t.shutdown_reply = reply_id;
        t.map
            .values()
            .filter_map(|e| e.browser.as_ref().and_then(|b| b.host()))
            .collect()
    });
    for host in hosts {
        host.close_browser(1);
    }
    maybe_finish_shutdown();
    // A tab that never closes does not keep the engine alive.
    on_ui_after(5000, || {
        TABS.with_borrow_mut(|t| t.map.clear());
        finish_shutdown();
    });
}

fn maybe_finish_shutdown() {
    if SHUTTING_DOWN.load(Ordering::SeqCst) && TABS.with_borrow(|t| t.map.is_empty()) {
        finish_shutdown();
    }
}

fn finish_shutdown() {
    if let Some(id) = TABS.with_borrow_mut(|t| t.shutdown_reply.take()) {
        out().result(&id, json!({}));
    }
    quit_message_loop();
}

// ---- main ----

/// The protocol's copy of stdout; descriptor 1 then points at stderr for everything else.
fn take_stdout() -> std::fs::File {
    // SAFETY: duplicating and redirecting this process's standard descriptors before any other thread exists.
    unsafe {
        let proto = libc::dup(1);
        assert!(proto >= 0, "dup(stdout)");
        libc::fcntl(proto, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::dup2(2, 1);
        std::fs::File::from_raw_fd(proto)
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

pub fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    let gpu = std::env::var("ELUDITE_CHROMIUM_GPU").as_deref() == Ok("1");
    let args = args::Args::new();
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let mut app = EngineApp::new(gpu);
    if Options::is_subprocess(&argv) {
        // A renderer, GPU, utility or zygote process: CEF runs it and answers its exit code.
        let code = execute_process(
            Some(args.as_main_args()),
            Some(&mut app),
            std::ptr::null_mut(),
        );
        return ExitCode::from(code.clamp(0, 255) as u8);
    }
    let opts = Options::parse(argv);
    let Some(profile) = opts.profile.clone() else {
        eprintln!("{NAME}: --profile DIR is required (the workspace's .eludite/browser/profile)");
        return ExitCode::from(2);
    };
    let cef_dir = opts.cef_dir.clone().unwrap_or_else(exe_dir);
    let decision = match sandbox::decide(
        std::env::var(sandbox::NO_SANDBOX_ENV).ok().as_deref(),
        &cef_dir,
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{NAME}: {e}");
            return ExitCode::from(3);
        }
    };
    let no_sandbox = decision == Decision::NoSandbox;
    if let Decision::Sandboxed {
        helper: Some(helper),
    } = &decision
    {
        // SAFETY: no other thread exists yet.
        unsafe { std::env::set_var("CHROME_DEVEL_SANDBOX", helper) };
    }
    SANDBOXED.store(!no_sandbox, Ordering::Relaxed);
    if no_sandbox {
        eprintln!(
            "{NAME}: running without Chromium's sandbox ({}=1)",
            sandbox::NO_SANDBOX_ENV
        );
    }
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if code >= 0 {
        return ExitCode::from(code.clamp(0, 255) as u8);
    }
    let sock = opts.frame_socket.unwrap_or(3);
    // SAFETY: marking a descriptor close-on-exec so CEF's subprocesses do not inherit the socket.
    if unsafe { libc::fcntl(sock, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        eprintln!("{NAME}: no frame socket on fd {sock}: frames will not reach a shell");
    }
    FRAME_SOCKET.store(sock, Ordering::Relaxed);
    let _ = OUT.set(Out::spawn(take_stdout()));
    if let Err(e) = std::fs::create_dir_all(&profile) {
        eprintln!("{NAME}: profile {}: {e}", profile.display());
        return ExitCode::from(2);
    }
    let path = |p: &Path| CefString::from(p.to_string_lossy().as_ref());
    let exe = std::env::current_exe().unwrap_or_default();
    let settings = Settings {
        no_sandbox: no_sandbox as i32,
        browser_subprocess_path: path(&exe),
        windowless_rendering_enabled: 1,
        multi_threaded_message_loop: 0,
        external_message_pump: 0,
        root_cache_path: path(&profile),
        cache_path: path(&profile),
        resources_dir_path: path(&cef_dir),
        locales_dir_path: path(&cef_dir.join("locales")),
        log_severity: LogSeverity::WARNING,
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        eprintln!(
            "{NAME}: CEF did not initialize (exit code {}); is another engine using the profile {}?",
            get_exit_code(),
            profile.display()
        );
        return ExitCode::from(4);
    }
    std::thread::Builder::new()
        .name("rpc-reader".into())
        .spawn(|| {
            rpc::read_loop(
                std::io::stdin().lock(),
                |m| on_ui(move || handle(m)),
                |e| eprintln!("{NAME}: {e}"),
            );
            // A closed stdin is shutdown.
            on_ui(|| begin_shutdown(None));
        })
        .expect("spawn the reader thread");
    run_message_loop();
    shutdown();
    out().flush(std::time::Duration::from_secs(2));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::browser_switches;

    #[test]
    fn the_switches_never_drop_the_sandbox_and_follow_gpu_and_display() {
        for gpu in [false, true] {
            for display in [false, true] {
                let s = browser_switches(gpu, display);
                assert!(!s.iter().any(|x| x.contains("sandbox")), "{s:?}");
                assert_eq!(s.contains(&"disable-gpu"), !gpu);
                assert_eq!(s.contains(&"ozone-platform=headless"), !display);
                assert!(s.contains(&"enable-logging=stderr"));
                assert!(s.iter().any(|x| x.starts_with("disable-features=")
                    && x.contains("NetworkTimeServiceQuerying,")));
            }
        }
    }
}
