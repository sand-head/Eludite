//! CEF's browser process (feature `cef`, Linux for now): the app, one windowless browser per tab, the render handler
//! writing `OnPaint` frames into the tab's ring, the DevTools pass-through, input, and the control loop; and for the
//! Web Browser window (brief 0032): popups as tabs, `<select>` popups drawn into the view's frames, cursors, the
//! title, favicon and history state, JavaScript dialogs, file choosers, authentication and permission prompts held
//! until the shell answers, downloads into the workspace's folder, the shell's context menu, DevTools as a windowless
//! tab, and edit commands.
//!
//! Threads: CEF's UI thread is the process's main thread (`CefRunMessageLoop`); every request from stdin is posted to
//! it as a task, and every CEF callback used here runs on it (except authentication, on the IO thread, whose
//! callback waits in the same map as the other prompts), so the tabs live in a thread-local map. The reader
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
/// `--profile`: the download folder's default is beside it.
static PROFILE: OnceLock<PathBuf> = OnceLock::new();

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

/// The `<select>` list (or another popup widget) CEF paints separately: where, and its last pixels.
#[derive(Default)]
struct Popup {
    shown: bool,
    /// In CSS pixels of the view (OnPopupSize).
    rect: Option<Rect>,
    /// Device pixels of the last PET_POPUP paint.
    pixels: Vec<u8>,
    size: (u32, u32),
}

/// What a tab's handlers share (they may be called while the map is borrowed, so nothing here is in it).
struct TabShared {
    id: String,
    /// CSS width, height and the device scale factor.
    geometry: Mutex<(i32, i32, f32)>,
    ring: Mutex<Option<Region>>,
    sequence: AtomicU64,
    dropped: AtomicU64,
    popup: Mutex<Popup>,
    /// The favicon url last downloaded (a change downloads the new one).
    favicon: Mutex<String>,
}

impl TabShared {
    fn new(id: String, width: i32, height: i32, scale: f32) -> Arc<TabShared> {
        Arc::new(TabShared {
            id,
            geometry: Mutex::new((width, height, scale)),
            ring: Mutex::new(None),
            sequence: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            popup: Mutex::new(Popup::default()),
            favicon: Mutex::new(String::new()),
        })
    }

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

    /// The tab's first region, announced before anything else names the tab.
    fn first_region(&self) -> bool {
        let (dw, dh) = self.device_size();
        let region = self.new_region(dw, dh);
        let ok = region.is_some();
        *lock(&self.ring) = region;
        ok
    }
}

struct TabEntry {
    browser: Option<Browser>,
    shared: Arc<TabShared>,
    _devtools: Option<Registration>,
    /// For a page: its DevTools tab, while open.
    devtools: Option<String>,
    /// For a DevTools tab: the page it inspects.
    devtools_of: Option<String>,
    /// For a popup: the tab that opened it, until `tab/popup` is sent.
    opener: Option<(String, String, u32, bool)>,
}

impl TabEntry {
    fn new(shared: Arc<TabShared>) -> TabEntry {
        TabEntry {
            browser: None,
            shared,
            _devtools: None,
            devtools: None,
            devtools_of: None,
            opener: None,
        }
    }
}

#[derive(Default)]
struct Tabs {
    next: u64,
    map: HashMap<String, TabEntry>,
    /// The `shutdown` request waiting for the last tab to close.
    shutdown_reply: Option<Value>,
}

impl Tabs {
    fn next_id(&mut self) -> String {
        self.next += 1;
        self.next.to_string()
    }
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

// ---- prompts the shell answers ----

/// A held CEF callback, answered by `tab/dialogAnswer` or `tab/permissionAnswer`.
enum Held {
    Js(JsdialogCallback),
    File(FileDialogCallback),
    Auth(AuthCallback),
    Permission(PermissionPromptCallback),
    Media(MediaAccessCallback, u32),
}

struct Prompt {
    tab: String,
    held: Held,
    default_text: String,
    /// CEF's own prompt id (permission prompts), to match OnDismissPermissionPrompt.
    cef_id: Option<u64>,
}

// SAFETY: CEF's callback objects are reference counted and may be continued from any thread (cef_callback_capi.h);
// the map only moves them between the IO thread (authentication) and the UI thread.
unsafe impl Send for Prompt {}

static PROMPTS: Mutex<Option<HashMap<u64, Prompt>>> = Mutex::new(None);
static NEXT_PROMPT: AtomicU64 = AtomicU64::new(1);

fn hold(prompt: Prompt) -> u64 {
    let id = NEXT_PROMPT.fetch_add(1, Ordering::Relaxed);
    lock(&PROMPTS)
        .get_or_insert_with(HashMap::new)
        .insert(id, prompt);
    id
}

fn take_prompt(id: u64) -> Option<Prompt> {
    lock(&PROMPTS).as_mut().and_then(|m| m.remove(&id))
}

/// Drop the tab's held prompts (it navigated, reset its dialogs or closed), telling the shell.
fn drop_prompts(tab: &str, only: impl Fn(&Prompt) -> bool) {
    let gone: Vec<(u64, Prompt)> = {
        let mut guard = lock(&PROMPTS);
        let Some(m) = guard.as_mut() else { return };
        let ids: Vec<u64> = m
            .iter()
            .filter(|(_, p)| p.tab == tab && only(p))
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| m.remove(&id).map(|p| (id, p)))
            .collect()
    };
    for (id, p) in gone {
        // A page's own dialogs are canceled by CEF; the rest are refused here so nothing waits forever.
        match p.held {
            Held::Js(_) => {}
            Held::File(cb) => cb.cancel(),
            Held::Auth(cb) => cb.cancel(),
            Held::Permission(cb) => cb.cont(PermissionRequestResult::DISMISS),
            Held::Media(cb, _) => cb.cancel(),
        }
        out().notify("tab/dialogClosed", json!({"tab": tab, "id": id}));
    }
}

/// `tab/dialogAnswer` and `tab/permissionAnswer`.
fn answer_prompt(p: &Value, accept: bool) {
    let Some(id) = p["id"].as_u64() else { return };
    let Some(prompt) = take_prompt(id) else {
        return;
    };
    let text = |k: &str| p[k].as_str().map(CefString::from);
    match prompt.held {
        Held::Js(cb) => {
            let input = p["text"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or(prompt.default_text);
            cb.cont(accept as i32, Some(&CefString::from(input.as_str())));
        }
        Held::File(cb) => {
            let files: Vec<&str> = p["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if accept && !files.is_empty() {
                let mut list = CefStringList::new();
                for f in files {
                    list.append(f);
                }
                cb.cont(Some(&mut list));
            } else {
                cb.cancel();
            }
        }
        Held::Auth(cb) => {
            if accept {
                cb.cont(text("username").as_ref(), text("password").as_ref());
            } else {
                cb.cancel();
            }
        }
        Held::Permission(cb) => cb.cont(if accept {
            PermissionRequestResult::ACCEPT
        } else {
            PermissionRequestResult::DENY
        }),
        Held::Media(cb, requested) => {
            if accept {
                cb.cont(requested);
            } else {
                cb.cancel();
            }
        }
    }
    out().notify(
        "tab/dialogClosed",
        json!({"tab": prompt.tab, "id": id, "accepted": accept}),
    );
}

// ---- downloads ----

static DOWNLOADS: Mutex<Option<(PathBuf, u64)>> = Mutex::new(None);

/// Progress last sent per download, to send it at most every 250 ms.
static DOWNLOAD_SENT: Mutex<Option<HashMap<u32, std::time::Instant>>> = Mutex::new(None);

fn download_dir() -> (PathBuf, u64) {
    lock(&DOWNLOADS).clone().unwrap_or_else(|| {
        (
            std::env::temp_dir().join("eludite-downloads"),
            crate::window::MAX_DOWNLOAD_BYTES,
        )
    })
}

/// A CEF string list's strings (its `IntoIterator` reads nothing for a list CEF lends a handler).
fn string_list(list: &mut CefStringList) -> Vec<String> {
    let raw: *mut sys::_cef_string_list_t = list.into();
    if raw.is_null() {
        return Vec::new();
    }
    // SAFETY: a list CEF handed to this callback, alive for its duration; values are copied out.
    unsafe {
        (0..sys::cef_string_list_size(raw))
            .filter_map(|i| {
                let mut value: sys::cef_string_t = std::mem::zeroed();
                (sys::cef_string_list_value(raw, i, &mut value) == 1).then(|| {
                    let s = CefString::from(std::ptr::from_ref(&value)).to_string();
                    sys::cef_string_utf16_clear(&mut value);
                    s
                })
            })
            .collect()
    }
}

fn user_string(s: CefStringUserfree) -> String {
    CefString::from(&s).to_string()
}

fn download_message(item: &DownloadItem, tab: &str, state: &str, extra: Value) {
    let path = user_string(item.full_path());
    let mut params = json!({
        "tab": tab,
        "id": item.id(),
        "url": user_string(item.url()),
        "file": Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "state": state,
        "receivedBytes": item.received_bytes().max(0),
        "totalBytes": item.total_bytes().max(-1),
        "mime": user_string(item.mime_type()),
    });
    if !path.is_empty() {
        params["path"] = json!(path);
    }
    if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    out().notify("tab/download", params);
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

        fn on_popup_show(&self, browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            {
                let mut p = lock(&self.tab.popup);
                p.shown = show != 0;
                if !p.shown {
                    *p = Popup::default();
                }
            }
            // The view repaints with (or without) the popup drawn over it.
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.invalidate(PaintElementType::VIEW);
            }
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            lock(&self.tab.popup).rect = rect.cloned();
        }

        fn on_paint(
            &self,
            browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if type_ == PaintElementType::POPUP {
                if buffer.is_null() || width <= 0 || height <= 0 {
                    return;
                }
                let (w, h) = (width as u32, height as u32);
                // SAFETY: CEF's OnPaint buffer is width * height * 4 bytes of BGRA for the duration of the call.
                let pixels = unsafe { std::slice::from_raw_parts(buffer, w as usize * h as usize * 4) };
                {
                    let mut p = lock(&self.tab.popup);
                    p.pixels.clear();
                    p.pixels.extend_from_slice(pixels);
                    p.size = (w, h);
                }
                if let Some(host) = browser.and_then(|b| b.host()) {
                    host.invalidate(PaintElementType::VIEW);
                }
                return;
            }
            paint(&self.tab, dirty_rects, buffer, width, height);
        }
    }
}

fn paint(
    tab: &TabShared,
    dirty_rects: Option<&[Rect]>,
    buffer: *const u8,
    width: i32,
    height: i32,
) {
    let paint_ns = shm::monotonic_ns();
    if buffer.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let (w, h) = (width as u32, height as u32);
    // SAFETY: CEF's OnPaint buffer is width * height * 4 bytes of BGRA for the duration of the call.
    let pixels = unsafe { std::slice::from_raw_parts(buffer, w as usize * h as usize * 4) };
    let mut dirty: Vec<shm::Rect> = dirty_rects
        .unwrap_or_default()
        .iter()
        .map(|r| shm::Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        })
        .collect();
    // A `<select>` list open: drawn over the view's frame (a copy only while one is shown).
    let mut composed: Option<Vec<u8>> = None;
    let mut popup_rect: Option<shm::Rect> = None;
    {
        let p = lock(&tab.popup);
        if p.shown
            && !p.pixels.is_empty()
            && let Some(r) = &p.rect
        {
            let scale = lock(&tab.geometry).2;
            let at = shm::Rect {
                x: (r.x as f32 * scale).round() as i32,
                y: (r.y as f32 * scale).round() as i32,
                width: p.size.0 as i32,
                height: p.size.1 as i32,
            };
            let mut buf = pixels.to_vec();
            crate::window::overlay(&mut buf, w, h, &p.pixels, p.size.0, p.size.1, at.x, at.y);
            composed = Some(buf);
            dirty.push(at);
            popup_rect = Some(at);
        }
    }
    let pixels = composed.as_deref().unwrap_or(pixels);
    let mut ring = lock(&tab.ring);
    if !ring.as_ref().is_some_and(|r| r.fits(w, h)) {
        *ring = tab.new_region(w, h);
    }
    let Some(region) = ring.as_ref() else {
        return;
    };
    let sequence = tab.sequence.fetch_add(1, Ordering::Relaxed) + 1;
    match region.write(pixels, w, h, sequence, paint_ns, &dirty) {
        Written::Slot { slot, copy_ns } => {
            let rect = |r: &shm::Rect| json!({"x": r.x.max(0), "y": r.y.max(0), "width": r.width.max(0), "height": r.height.max(0)});
            let mut params = json!({
                "tab": tab.id,
                "region": region.id,
                "slot": slot,
                "sequence": sequence,
                "width": w,
                "height": h,
                "dirty": shm::clamp_dirty(&dirty).iter().map(rect).collect::<Vec<_>>(),
                "paintNs": paint_ns,
                "copyNs": copy_ns,
            });
            if let Some(r) = popup_rect {
                params["popup"] = rect(&r);
            }
            out().notify("tab/frame", params)
        }
        Written::Dropped => {
            tab.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A new tab's client: the handlers of this file, for a page, a popup or a DevTools tab.
fn client_for(shared: &Arc<TabShared>) -> Client {
    TabClient::new(
        TabRender::new(shared.clone()),
        TabLife::new(shared.clone()),
        TabDisplay::new(shared.clone()),
        TabLoad::new(shared.clone()),
        TabRequests::new(shared.clone()),
        TabJsDialogs::new(shared.clone()),
        TabFileDialogs::new(shared.clone()),
        TabDownloads::new(shared.clone()),
        TabPermissions::new(shared.clone()),
        TabMenus::new(shared.clone()),
    )
}

fn windowless_info() -> WindowInfo {
    WindowInfo {
        windowless_rendering_enabled: 1,
        shared_texture_enabled: 0,
        external_begin_frame_enabled: 0,
        runtime_style: RuntimeStyle::ALLOY,
        ..Default::default()
    }
}

fn tab_settings(rate: i32) -> BrowserSettings {
    BrowserSettings {
        windowless_frame_rate: rate,
        // Opaque white until the page paints, as a browser shows.
        background_color: 0xFFFF_FFFF,
        ..Default::default()
    }
}

/// A browser of ours exists: keep it, watch its DevTools agent, and (for a popup) announce it.
fn adopt_browser(tab: &str, browser: &Browser) {
    let devtools_observer =
        TABS.with_borrow(|t| t.map.get(tab).is_some_and(|e| e.devtools_of.is_none()));
    let registration = browser.host().and_then(|host| {
        host.was_hidden(0);
        if !devtools_observer {
            return None;
        }
        let mut observer = TabDevTools::new(Arc::from(tab));
        host.add_dev_tools_message_observer(Some(&mut observer))
    });
    let opener = TABS.with_borrow_mut(|t| {
        t.map.get_mut(tab).and_then(|e| {
            e.browser = Some(browser.clone());
            e._devtools = registration;
            e.opener.take()
        })
    });
    if let Some((opener, url, disposition, gesture)) = opener {
        out().notify(
            "tab/popup",
            json!({
                "tab": tab,
                "opener": opener,
                "url": url,
                "disposition": crate::window::disposition_name(disposition),
                "userGesture": gesture,
            }),
        );
    }
}

wrap_life_span_handler! {
    struct TabLife {
        tab: Arc<TabShared>,
    }

    impl LifeSpanHandler {
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // A popup becomes a windowless tab of its own, the opener's page keeps `window.opener` (brief 0032).
            let (Some(window_info), Some(client)) = (window_info, client) else {
                return 1;
            };
            let (w, h, s) = *lock(&self.tab.geometry);
            let id = TABS.with_borrow_mut(Tabs::next_id);
            let shared = TabShared::new(id.clone(), w, h, s);
            if !shared.first_region() {
                return 1;
            }
            let mut entry = TabEntry::new(shared.clone());
            entry.opener = Some((
                self.tab.id.clone(),
                target_url.map(ToString::to_string).unwrap_or_default(),
                target_disposition.get_raw(),
                user_gesture != 0,
            ));
            TABS.with_borrow_mut(|t| t.map.insert(id, entry));
            *window_info = windowless_info();
            *client = Some(client_for(&shared));
            if let Some(settings) = settings {
                *settings = tab_settings(60);
            }
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                adopt_browser(&self.tab.id, browser);
            }
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            closed(&self.tab.id, "closed");
        }
    }
}

fn closed(tab: &str, reason: &str) {
    let removed = TABS.with_borrow_mut(|t| {
        let e = t.map.remove(tab)?;
        // A DevTools tab's page forgets it.
        if let Some(page) = &e.devtools_of
            && let Some(p) = t.map.get_mut(page)
        {
            p.devtools = None;
        }
        Some(e)
    });
    let Some(entry) = removed.as_ref() else {
        return;
    };
    // DevTools closes with its page; its bridge closes with it.
    if let Some(page) = &entry.devtools_of {
        drop_bridge(page);
    }
    if let Some(devtools) = &entry.devtools {
        drop_bridge(tab);
        if let Some(host) = host_of(devtools) {
            host.close_browser(1);
        }
    }
    drop_prompts(tab, |_| true);
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

        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            let text = value.map(ToString::to_string).unwrap_or_default();
            out().notify("tab/state", json!({"tab": self.tab.id, "statusText": text}));
        }

        fn on_favicon_urlchange(
            &self,
            browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            let first = icon_urls
                .map(string_list)
                .unwrap_or_default()
                .into_iter()
                .next()
                .unwrap_or_default();
            {
                let mut last = lock(&self.tab.favicon);
                if *last == first {
                    return;
                }
                last.clone_from(&first);
            }
            if first.is_empty() {
                out().notify("tab/state", json!({"tab": self.tab.id, "favicon": ""}));
                return;
            }
            if let Some(host) = browser.and_then(|b| b.host()) {
                let mut cb = TabFavicon::new(self.tab.clone());
                host.download_image(Some(&CefString::from(first.as_str())), 1, 32, 0, Some(&mut cb));
            }
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: ::std::os::raw::c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            out().notify(
                "tab/cursor",
                json!({"tab": self.tab.id, "cursor": crate::window::cursor_name(type_.get_raw())}),
            );
            // Handled: windowless, there is no native cursor to set.
            1
        }
    }
}

wrap_download_image_callback! {
    struct TabFavicon {
        tab: Arc<TabShared>,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            image_url: Option<&CefString>,
            _http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            // A later favicon replaced this one meanwhile: keep the newer.
            let url = image_url.map(ToString::to_string).unwrap_or_default();
            if *lock(&self.tab.favicon) != url {
                return;
            }
            let png = image.and_then(|i| {
                let (mut w, mut h) = (0, 0);
                i.as_png(1.0, 1, Some(&mut w), Some(&mut h))
            });
            let data = png
                .map(|b| {
                    let mut bytes = vec![0u8; b.size()];
                    let n = b.data(Some(&mut bytes), 0);
                    bytes.truncate(n);
                    bytes
                })
                .filter(|b| !b.is_empty())
                .map(|b| format!("data:image/png;base64,{}", crate::window::base64(&b)))
                .unwrap_or_default();
            out().notify("tab/state", json!({"tab": self.tab.id, "favicon": data}));
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
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            out().notify(
                "tab/state",
                json!({
                    "tab": self.tab.id,
                    "loading": is_loading != 0,
                    "canGoBack": can_go_back != 0,
                    "canGoForward": can_go_forward != 0,
                }),
            );
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
            drop_prompts(&id, |_| true);
            out().notify("tab/closed", json!({"tab": id, "reason": "crashed"}));
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.close_browser(1);
            }
            TABS.with_borrow_mut(|t| t.map.remove(&id));
            maybe_finish_shutdown();
        }

        fn auth_credentials(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            is_proxy: ::std::os::raw::c_int,
            host: Option<&CefString>,
            port: ::std::os::raw::c_int,
            realm: Option<&CefString>,
            scheme: Option<&CefString>,
            callback: Option<&mut AuthCallback>,
        ) -> ::std::os::raw::c_int {
            // Called on the IO thread; the shell's dialog answers through tab/dialogAnswer.
            let Some(cb) = callback else { return 0 };
            let host = host.map(ToString::to_string).unwrap_or_default();
            let realm = realm.map(ToString::to_string).unwrap_or_default();
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::Auth(cb.clone()),
                default_text: String::new(),
                cef_id: None,
            });
            out().notify(
                "tab/dialog",
                json!({
                    "tab": self.tab.id,
                    "id": id,
                    "kind": "auth",
                    "message": format!("{host}:{port} asks for a user name and password ({realm})"),
                    "url": origin_url.map(ToString::to_string).unwrap_or_default(),
                    "host": format!("{host}:{port}"),
                    "realm": realm,
                    "scheme": scheme.map(ToString::to_string).unwrap_or_default(),
                    "isProxy": is_proxy != 0,
                }),
            );
            1
        }
    }
}

wrap_jsdialog_handler! {
    struct TabJsDialogs {
        tab: Arc<TabShared>,
    }

    impl JsdialogHandler {
        fn on_jsdialog(
            &self,
            _browser: Option<&mut Browser>,
            origin_url: Option<&CefString>,
            dialog_type: JsdialogType,
            message_text: Option<&CefString>,
            default_prompt_text: Option<&CefString>,
            callback: Option<&mut JsdialogCallback>,
            _suppress_message: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let Some(cb) = callback else { return 0 };
            let kind = if dialog_type == JsdialogType::CONFIRM {
                "confirm"
            } else if dialog_type == JsdialogType::PROMPT {
                "prompt"
            } else {
                "alert"
            };
            let default_text = default_prompt_text.map(ToString::to_string).unwrap_or_default();
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::Js(cb.clone()),
                default_text: default_text.clone(),
                cef_id: None,
            });
            let mut params = json!({
                "tab": self.tab.id,
                "id": id,
                "kind": kind,
                "message": message_text.map(ToString::to_string).unwrap_or_default(),
                "url": origin_url.map(ToString::to_string).unwrap_or_default(),
            });
            if kind == "prompt" {
                params["defaultText"] = json!(default_text);
            }
            out().notify("tab/dialog", params);
            // Handled: the page waits for the callback (the shell's dialog, or an agent's eludite.browser.dialog).
            1
        }

        fn on_before_unload_dialog(
            &self,
            _browser: Option<&mut Browser>,
            message_text: Option<&CefString>,
            is_reload: ::std::os::raw::c_int,
            callback: Option<&mut JsdialogCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(cb) = callback else { return 0 };
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::Js(cb.clone()),
                default_text: String::new(),
                cef_id: None,
            });
            out().notify(
                "tab/dialog",
                json!({
                    "tab": self.tab.id,
                    "id": id,
                    "kind": "beforeunload",
                    "message": message_text.map(ToString::to_string).unwrap_or_default(),
                    "isReload": is_reload != 0,
                }),
            );
            1
        }

        fn on_reset_dialog_state(&self, _browser: Option<&mut Browser>) {
            drop_prompts(&self.tab.id, |p| matches!(p.held, Held::Js(_)));
        }
    }
}

wrap_dialog_handler! {
    struct TabFileDialogs {
        tab: Arc<TabShared>,
    }

    impl DialogHandler {
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: FileDialogMode,
            title: Option<&CefString>,
            default_file_path: Option<&CefString>,
            accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(cb) = callback else { return 0 };
            let mode = if mode == FileDialogMode::OPEN_MULTIPLE {
                "openMultiple"
            } else if mode == FileDialogMode::OPEN_FOLDER {
                "openFolder"
            } else if mode == FileDialogMode::SAVE {
                "save"
            } else {
                "open"
            };
            let accept: Vec<String> = accept_filters.map(string_list).unwrap_or_default();
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::File(cb.clone()),
                default_text: String::new(),
                cef_id: None,
            });
            out().notify(
                "tab/dialog",
                json!({
                    "tab": self.tab.id,
                    "id": id,
                    "kind": "file",
                    "message": title.map(ToString::to_string).unwrap_or_default(),
                    "mode": mode,
                    "accept": accept,
                    "defaultPath": default_file_path.map(ToString::to_string).unwrap_or_default(),
                }),
            );
            1
        }
    }
}

wrap_download_handler! {
    struct TabDownloads {
        tab: Arc<TabShared>,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(cb)) = (download_item, callback) else {
                return 0;
            };
            let (dir, max) = download_dir();
            let total = item.total_bytes();
            if total > 0 && total as u64 > max {
                download_message(item, &self.tab.id, "refused", json!({
                    "message": format!(
                        "the file is {} MB, over the {} MB limit for downloads in the Web Browser window",
                        total as u64 / (1024 * 1024), max / (1024 * 1024)
                    ),
                }));
                // Unhandled: an Alloy-style browser cancels it.
                return 0;
            }
            let name = crate::window::safe_file_name(
                &suggested_name.map(ToString::to_string).unwrap_or_default(),
                &user_string(item.url()),
            );
            if let Err(e) = std::fs::create_dir_all(&dir) {
                download_message(item, &self.tab.id, "refused", json!({
                    "message": format!("cannot create {}: {e}", dir.display()),
                }));
                return 0;
            }
            let path = crate::window::unique_path(&dir, &name);
            cb.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
            download_message(item, &self.tab.id, "started", json!({
                "path": path.to_string_lossy(),
                "file": name,
            }));
            1
        }

        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else { return };
            let (_, max) = download_dir();
            let id = item.id();
            let forget = || {
                if let Some(m) = lock(&DOWNLOAD_SENT).as_mut() {
                    m.remove(&id);
                }
            };
            if item.is_in_progress() == 1 && item.received_bytes() > 0 && item.received_bytes() as u64 > max {
                if let Some(cb) = callback {
                    cb.cancel();
                }
                forget();
                download_message(item, &self.tab.id, "refused", json!({
                    "message": format!("the download passed the {} MB limit and was canceled", max / (1024 * 1024)),
                }));
                return;
            }
            if item.is_complete() == 1 {
                forget();
                download_message(item, &self.tab.id, "complete", json!({}));
            } else if item.is_canceled() == 1 {
                forget();
                download_message(item, &self.tab.id, "canceled", json!({}));
            } else if item.is_interrupted() == 1 {
                forget();
                download_message(item, &self.tab.id, "interrupted", json!({
                    "message": format!("interrupted (reason {})", item.interrupt_reason().get_raw()),
                }));
            } else if item.is_in_progress() == 1 {
                let now = std::time::Instant::now();
                let due = {
                    let mut guard = lock(&DOWNLOAD_SENT);
                    let m = guard.get_or_insert_with(HashMap::new);
                    let due = m.get(&id).is_none_or(|t| now.duration_since(*t).as_millis() >= 250);
                    if due {
                        m.insert(id, now);
                    }
                    due
                };
                if due {
                    download_message(item, &self.tab.id, "progress", json!({}));
                }
            }
        }
    }
}

wrap_permission_handler! {
    struct TabPermissions {
        tab: Arc<TabShared>,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(cb) = callback else { return 0 };
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::Media(cb.clone(), requested_permissions),
                default_text: String::new(),
                cef_id: None,
            });
            out().notify(
                "tab/permission",
                json!({
                    "tab": self.tab.id,
                    "id": id,
                    "origin": requesting_origin.map(ToString::to_string).unwrap_or_default(),
                    "permissions": crate::window::media_permission_names(requested_permissions),
                }),
            );
            1
        }

        fn on_show_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(cb) = callback else { return 0 };
            let id = hold(Prompt {
                tab: self.tab.id.clone(),
                held: Held::Permission(cb.clone()),
                default_text: String::new(),
                cef_id: Some(prompt_id),
            });
            out().notify(
                "tab/permission",
                json!({
                    "tab": self.tab.id,
                    "id": id,
                    "origin": requesting_origin.map(ToString::to_string).unwrap_or_default(),
                    "permissions": crate::window::permission_names(requested_permissions),
                }),
            );
            1
        }

        fn on_dismiss_permission_prompt(
            &self,
            _browser: Option<&mut Browser>,
            prompt_id: u64,
            _result: PermissionRequestResult,
        ) {
            let id = lock(&PROMPTS).as_ref().and_then(|m| {
                m.iter()
                    .find(|(_, p)| p.cef_id == Some(prompt_id))
                    .map(|(id, _)| *id)
            });
            if let Some(id) = id
                && take_prompt(id).is_some()
            {
                out().notify("tab/dialogClosed", json!({"tab": self.tab.id, "id": id}));
            }
        }
    }
}

wrap_context_menu_handler! {
    struct TabMenus {
        tab: Arc<TabShared>,
    }

    impl ContextMenuHandler {
        fn run_context_menu(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            // The shell's menu, never Chromium's: say what is under the pointer and cancel CEF's.
            if let Some(cb) = callback {
                cb.cancel();
            }
            let Some(p) = params else { return 1 };
            let flags = p.edit_state_flags().as_ref().0;
            let has = |bit: u32| flags & bit != 0;
            let (back, forward) = browser
                .map(|b| (b.can_go_back() == 1, b.can_go_forward() == 1))
                .unwrap_or_default();
            let mut m = json!({
                "tab": self.tab.id,
                "x": p.xcoord(),
                "y": p.ycoord(),
                "pageUrl": user_string(p.page_url()),
                "editable": p.is_editable() == 1,
                "edit": {
                    "undo": has(1), "redo": has(2), "cut": has(4), "copy": has(8),
                    "paste": has(16), "delete": has(32), "selectAll": has(64),
                },
                "canGoBack": back,
                "canGoForward": forward,
            });
            let link = user_string(p.link_url());
            if !link.is_empty() {
                m["linkUrl"] = json!(link);
            }
            if p.has_image_contents() == 1 {
                let src = user_string(p.source_url());
                if !src.is_empty() {
                    m["imageUrl"] = json!(src);
                }
            }
            let selection = user_string(p.selection_text());
            if !selection.is_empty() {
                m["selectionText"] = json!(selection);
            }
            out().notify("tab/contextMenu", m);
            1
        }
    }
}

/// The client of every tab: its `new` takes one handler of each kind.
#[allow(clippy::too_many_arguments)]
mod client {
    use super::*;

    wrap_client! {
        pub(super) struct TabClient {
            render: RenderHandler,
            life: LifeSpanHandler,
            display: DisplayHandler,
            load: LoadHandler,
            requests: RequestHandler,
            js: JsdialogHandler,
            files: DialogHandler,
            downloads: DownloadHandler,
            permissions: PermissionHandler,
            menus: ContextMenuHandler,
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

            fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
                Some(self.js.clone())
            }

            fn dialog_handler(&self) -> Option<DialogHandler> {
                Some(self.files.clone())
            }

            fn download_handler(&self) -> Option<DownloadHandler> {
                Some(self.downloads.clone())
            }

            fn permission_handler(&self) -> Option<PermissionHandler> {
                Some(self.permissions.clone())
            }

            fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
                Some(self.menus.clone())
            }
        }
    }
}
use client::TabClient;

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
            if let Some(m) = message
                && !bridge_route(&self.tab, m)
            {
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
                    Some((k, v)) => {
                        let key = CefString::from(k);
                        // CEF sets a feature list of its own: add to it, since a second switch replaces the first.
                        let mut value = v.to_owned();
                        if k == "disable-features"
                            && cl.has_switch(Some(&key)) == 1
                        {
                            let old = CefString::from(&cl.switch_value(Some(&key))).to_string();
                            value = crate::privacy::merge_feature_lists(&old, v);
                        }
                        cl.append_switch_with_value(Some(&key), Some(&CefString::from(value.as_str())))
                    }
                    None => cl.append_switch(Some(&CefString::from(switch.as_str()))),
                }
            }
        }
    }
}

/// The browser process's switches. Software compositing unless `ELUDITE_CHROMIUM_GPU=1`: off-screen frames come back
/// through `OnPaint` either way, and without a GPU (or under Xvfb) the GPU process only adds a copy.
pub fn browser_switches(gpu: bool, has_display: bool) -> Vec<String> {
    let mut s: Vec<String> = vec![
        "enable-logging=stderr".into(),
        "noerrdialogs".into(),
        // Authentication challenges go to CefRequestHandler::GetAuthCredentials (the shell's dialog), never to
        // Chrome's own login prompt (brief 0032).
        "disable-chrome-login-prompt".into(),
    ];
    // Chrome's background services off (brief 0032, `privacy`): no request the person did not ask for.
    s.extend(crate::privacy::SWITCHES.iter().map(|x| (*x).to_owned()));
    s.push(crate::privacy::disable_features_switch());
    if !gpu {
        s.extend(["disable-gpu".into(), "disable-gpu-compositing".into()]);
    }
    // Windowless tabs need no display, but Chromium's default Ozone platform (X11 or Wayland) refuses to start
    // without one. With neither, use the headless platform: rendering is the same; the system clipboard is lost.
    if !has_display {
        s.push("ozone-platform=headless".into());
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

/// The page a DevTools tab inspects, if `tab` is one.
fn devtools_page(tab: &str) -> Option<String> {
    TABS.with_borrow(|t| t.map.get(tab).and_then(|e| e.devtools_of.clone()))
}

fn handle(m: Incoming) {
    let p = &m.params;
    let r = match m.method.as_str() {
        "initialize" => {
            initialize_downloads(p);
            Ok(json!({
                "engineName": NAME,
                "engineVersion": env!("CARGO_PKG_VERSION"),
                "protocolVersion": rpc::PROTOCOL_VERSION,
                "cefVersion": cef_version(),
                "chromiumVersion": chromium_version(),
                "pid": std::process::id(),
                "sandbox": SANDBOXED.load(Ordering::Relaxed),
                "frameTransport": "memfd+scm_rights",
            }))
        }
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
        "tab/devtools" => open_devtools(p),
        "tab/input" => {
            if let Ok(tab) = tab_param(p)
                && let Some(host) = host_of(&tab)
            {
                input(&host, &p["event"]);
            }
            return;
        }
        "tab/action" => {
            if let Ok(tab) = tab_param(p)
                && let Some(browser) = browser_of(&tab)
            {
                action(&browser, p);
            }
            return;
        }
        "tab/dialogAnswer" => {
            answer_prompt(p, p["accept"].as_bool().unwrap_or(false));
            return;
        }
        "tab/permissionAnswer" => {
            answer_prompt(p, p["allow"].as_bool().unwrap_or(false));
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

/// `initialize`'s download folder and limit (default: `downloads` beside the profile, 100 MB).
fn initialize_downloads(p: &Value) {
    let profile = PROFILE.get().cloned().unwrap_or_default();
    let dir = p["downloadDir"]
        .as_str()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::window::default_download_dir(&profile));
    let max = p["maxDownloadBytes"]
        .as_u64()
        .filter(|m| *m > 0)
        .unwrap_or(crate::window::MAX_DOWNLOAD_BYTES);
    *lock(&DOWNLOADS) = Some((dir, max));
}

fn create_tab(p: &Value) -> Result<Value, (i64, String)> {
    let url = p["url"].as_str().unwrap_or("about:blank").to_owned();
    let width = dimension(p, "width")?;
    let height = dimension(p, "height")?;
    let scale = p["scale"].as_f64().unwrap_or(1.).clamp(0.25, 8.) as f32;
    let rate = p["frameRate"].as_i64().unwrap_or(60).clamp(1, 240) as i32;
    let id = TABS.with_borrow_mut(Tabs::next_id);
    let shared = TabShared::new(id.clone(), width, height, scale);
    if !shared.first_region() {
        return Err((rpc::INTERNAL_ERROR, "no shared memory for the tab".into()));
    }
    let mut client = client_for(&shared);
    // Registered before the browser exists, so the callbacks of its creation find the tab.
    TABS.with_borrow_mut(|t| t.map.insert(id.clone(), TabEntry::new(shared.clone())));
    let Some(browser) = browser_host_create_browser_sync(
        Some(&windowless_info()),
        Some(&mut client),
        Some(&CefString::from(url.as_str())),
        Some(&tab_settings(rate)),
        None,
        None,
    ) else {
        TABS.with_borrow_mut(|t| t.map.remove(&id));
        return Err((rpc::INTERNAL_ERROR, "CEF did not create the browser".into()));
    };
    adopt_browser(&id, &browser);
    if let Some(host) = browser.host() {
        host.set_focus(1);
    }
    Ok(json!({"tab": id}))
}

/// `tab/devtools`: DevTools for a page as a windowless tab of its own. CEF 154 cannot show DevTools windowless
/// (`ShowDevTools` makes a Chrome-style window), so the engine loads Chromium's DevTools front end
/// (`devtools://devtools/bundled/inspector.html`) in a windowless tab and connects it to the page through a websocket
/// bridge on 127.0.0.1 with an unguessable path ([`Bridge`]), which exists only while that tab does.
fn open_devtools(p: &Value) -> Result<Value, (i64, String)> {
    let tab = tab_param(p)?;
    if devtools_page(&tab).is_some() {
        return Err((
            rpc::INVALID_PARAMS,
            format!("tab {tab} is a DevTools tab; open DevTools for its page"),
        ));
    }
    if host_of(&tab).is_none() {
        return Err(no_tab(&tab));
    }
    let inspect = p["inspectAt"].as_object().map(|o| {
        (
            o.get("x").and_then(Value::as_i64).unwrap_or(0),
            o.get("y").and_then(Value::as_i64).unwrap_or(0),
        )
    });
    let open = TABS.with_borrow(|t| t.map.get(&tab).and_then(|e| e.devtools.clone()));
    if let Some(devtools) = open {
        if let (Some(at), Some(b)) = (inspect, bridge_of(&tab)) {
            b.inspect(&tab, at);
        }
        return Ok(json!({"tab": tab, "devtools": devtools, "created": false}));
    }
    let width = dimension(p, "width")?;
    let height = dimension(p, "height")?;
    let scale = p["scale"].as_f64().unwrap_or(1.).clamp(0.25, 8.) as f32;
    let bridge = Bridge::open(&tab, inspect)
        .map_err(|e| (rpc::INTERNAL_ERROR, format!("the DevTools bridge: {e}")))?;
    let url = format!(
        "devtools://devtools/bundled/inspector.html?ws=127.0.0.1:{}/{}",
        bridge.port, bridge.token
    );
    let id = TABS.with_borrow_mut(Tabs::next_id);
    let shared = TabShared::new(id.clone(), width, height, scale);
    if !shared.first_region() {
        bridge.close();
        return Err((rpc::INTERNAL_ERROR, "no shared memory for DevTools".into()));
    }
    let mut entry = TabEntry::new(shared.clone());
    entry.devtools_of = Some(tab.clone());
    TABS.with_borrow_mut(|t| {
        t.map.insert(id.clone(), entry);
        if let Some(page) = t.map.get_mut(&tab) {
            page.devtools = Some(id.clone());
        }
    });
    lock(&BRIDGES)
        .get_or_insert_with(HashMap::new)
        .insert(tab.clone(), bridge.clone());
    let mut client = client_for(&shared);
    let Some(browser) = browser_host_create_browser_sync(
        Some(&windowless_info()),
        Some(&mut client),
        Some(&CefString::from(url.as_str())),
        Some(&tab_settings(60)),
        None,
        None,
    ) else {
        TABS.with_borrow_mut(|t| t.map.remove(&id));
        drop_bridge(&tab);
        return Err((
            rpc::INTERNAL_ERROR,
            "CEF did not create the DevTools tab".into(),
        ));
    };
    adopt_browser(&id, &browser);
    Ok(json!({"tab": tab, "devtools": id, "created": true}))
}

// ---- the DevTools bridge ----

/// The front end's message ids are moved above this, so its answers and the shell's never mix on the page's one
/// DevTools session (CDP ids are 32-bit integers).
const FRONT_END_IDS: u64 = 1 << 29;
/// The engine's own calls for the front end (Inspect's node lookup).
const BRIDGE_IDS: u64 = 1 << 30;

static BRIDGES: Mutex<Option<HashMap<String, Arc<Bridge>>>> = Mutex::new(None);

/// A websocket on 127.0.0.1 that DevTools' front end connects to, carrying its messages to the page's DevTools agent
/// (`SendDevToolsMessage`, on the UI thread) and the agent's answers and events back.
struct Bridge {
    port: u16,
    token: String,
    closed: AtomicBool,
    /// The connected front end's outgoing messages (a writer thread sends them).
    to_front: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    /// Inspect this point once the front end has enabled its overlay.
    inspect_at: Mutex<Option<(i64, i64)>>,
}

fn bridge_of(page: &str) -> Option<Arc<Bridge>> {
    lock(&BRIDGES).as_ref().and_then(|m| m.get(page).cloned())
}

fn drop_bridge(page: &str) {
    if let Some(b) = lock(&BRIDGES).as_mut().and_then(|m| m.remove(page)) {
        b.close();
    }
}

fn random_token() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if ok.is_err() {
        let t = shm::monotonic_ns() ^ u64::from(std::process::id()) << 32;
        bytes[..8].copy_from_slice(&t.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Bridge {
    fn open(page: &str, inspect_at: Option<(i64, i64)>) -> std::io::Result<Arc<Bridge>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let bridge = Arc::new(Bridge {
            port: listener.local_addr()?.port(),
            token: random_token(),
            closed: AtomicBool::new(false),
            to_front: Mutex::new(None),
            inspect_at: Mutex::new(inspect_at),
        });
        let (b, page) = (bridge.clone(), page.to_owned());
        std::thread::Builder::new()
            .name("devtools-bridge".into())
            .spawn(move || b.accept_loop(listener, page))?;
        Ok(bridge)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        lock(&self.to_front).take();
    }

    fn accept_loop(self: Arc<Self>, listener: std::net::TcpListener, page: String) {
        while !self.closed.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let b = self.clone();
                    let page = page.clone();
                    let _ = std::thread::Builder::new()
                        .name("devtools-bridge-conn".into())
                        .spawn(move || b.serve(stream, page));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
    }

    /// One front end connection: only the path with the token is accepted.
    #[allow(clippy::result_large_err)]
    fn serve(self: Arc<Self>, stream: std::net::TcpStream, page: String) {
        use tungstenite::handshake::server::{ErrorResponse, Request, Response};
        let want = format!("/{}", self.token);
        let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            if req.uri().path() == want {
                Ok(resp)
            } else {
                let mut e = ErrorResponse::new(Some("not found".into()));
                *e.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
                Err(e)
            }
        };
        let Ok(writer_stream) = stream.try_clone() else {
            return;
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else {
            return;
        };
        let mut writer = tungstenite::WebSocket::from_raw_socket(
            writer_stream,
            tungstenite::protocol::Role::Server,
            None,
        );
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        *lock(&self.to_front) = Some(tx);
        let _ = std::thread::Builder::new()
            .name("devtools-bridge-out".into())
            .spawn(move || {
                for text in rx {
                    if writer.send(tungstenite::Message::text(text)).is_err() {
                        break;
                    }
                }
            });
        while !self.closed.load(Ordering::Acquire) {
            let Ok(msg) = ws.read() else { break };
            let text = match msg {
                tungstenite::Message::Text(t) => t.as_str().to_owned(),
                tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            let Ok(mut v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let enables_overlay = v["method"] == "Overlay.enable";
            if let Some(id) = v["id"].as_u64() {
                v["id"] = json!(id + FRONT_END_IDS);
            }
            let page = page.clone();
            let body = serde_json::to_vec(&v).unwrap_or_default();
            let b = self.clone();
            on_ui(move || {
                if let Some(host) = host_of(&page) {
                    host.send_dev_tools_message(Some(&body));
                    if enables_overlay {
                        b.inspect_pending(&page);
                    }
                }
            });
        }
        lock(&self.to_front).take();
    }

    /// Inspect a point: once the front end is up, now; else when it enables its overlay.
    fn inspect(&self, page: &str, at: (i64, i64)) {
        *lock(&self.inspect_at) = Some(at);
        if lock(&self.to_front).is_some() {
            self.inspect_pending(page);
        }
    }

    /// Ask the page for the node at the pending point (UI thread); [`bridge_route`] tells the front end to select it.
    fn inspect_pending(&self, page: &str) {
        let Some((x, y)) = lock(&self.inspect_at).take() else {
            return;
        };
        if let Some(host) = host_of(page) {
            let msg = json!({"id": BRIDGE_IDS + 1, "method": "DOM.getNodeForLocation",
                "params": {"x": x, "y": y, "includeUserAgentShadowDOM": false}});
            host.send_dev_tools_message(Some(&serde_json::to_vec(&msg).unwrap_or_default()));
        }
    }

    fn send(&self, text: String) {
        if let Some(tx) = lock(&self.to_front).as_ref() {
            let _ = tx.send(text);
        }
    }
}

/// A message from the page's DevTools agent while DevTools is open: the front end's answers go to it alone (with
/// its own ids), events to both. True when the shell should not see it.
fn bridge_route(page: &str, message: &[u8]) -> bool {
    let Some(bridge) = bridge_of(page) else {
        return false;
    };
    // Answers start with their id; events with their method.
    let head = &message[..message.len().min(32)];
    let is_answer = head.windows(5).any(|w| w == b"\"id\":");
    if !is_answer {
        bridge.send(String::from_utf8_lossy(message).into_owned());
        return false;
    }
    let Ok(mut v) = serde_json::from_slice::<Value>(message) else {
        return false;
    };
    let id = v["id"].as_u64().unwrap_or(0);
    if id > BRIDGE_IDS {
        if let Some(node) = v["result"]["backendNodeId"].as_i64() {
            bridge.send(
                json!({"method": "Overlay.inspectNodeRequested", "params": {"backendNodeId": node}})
                    .to_string(),
            );
        }
        return true;
    }
    if id > FRONT_END_IDS {
        v["id"] = json!(id - FRONT_END_IDS);
        bridge.send(v.to_string());
        return true;
    }
    false
}

/// `tab/action`: an edit command on the focused frame, Stop, or a download.
fn action(browser: &Browser, p: &Value) {
    let frame = || browser.focused_frame().or_else(|| browser.main_frame());
    match p["action"].as_str().unwrap_or_default() {
        "undo" => frame().inspect(|f| f.undo()),
        "redo" => frame().inspect(|f| f.redo()),
        "cut" => frame().inspect(|f| f.cut()),
        "copy" => frame().inspect(|f| f.copy()),
        "paste" => frame().inspect(|f| f.paste()),
        "delete" => frame().inspect(|f| f.del()),
        "selectAll" => frame().inspect(|f| f.select_all()),
        "stop" => {
            browser.stop_load();
            None
        }
        "download" => {
            if let (Some(host), Some(url)) = (browser.host(), p["url"].as_str()) {
                host.start_download(Some(&CefString::from(url)));
            }
            None
        }
        _ => None,
    };
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
    if devtools_page(&tab).is_some() {
        cdp_error(&tab, message, &format!("tab {tab} is DevTools, not a page"));
        return;
    }
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

/// CEF's "no range": IME text replaces the current selection.
const INVALID_RANGE: Range = Range {
    from: u32::MAX,
    to: u32::MAX,
};

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
            let underlines: Vec<CompositionUnderline> = e["underlines"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|u| CompositionUnderline {
                    range: Range {
                        from: u["from"].as_u64().unwrap_or(0) as u32,
                        to: u["to"].as_u64().unwrap_or(0) as u32,
                    },
                    thick: u["thick"].as_bool().unwrap_or(false) as i32,
                    ..Default::default()
                })
                .collect();
            // The current selection is replaced (CEF's invalid range), as CEF's own clients pass.
            host.ime_set_composition(
                Some(&text),
                Some(&underlines),
                Some(&INVALID_RANGE),
                Some(&selection),
            );
        }
        "imeCommitText" => {
            let text = CefString::from(e["text"].as_str().unwrap_or_default());
            host.ime_commit_text(
                Some(&text),
                Some(&INVALID_RANGE),
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
    let _ = PROFILE.set(profile.clone());
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
    // Chrome's background services read these preferences at startup (brief 0032).
    if let Err(e) = crate::privacy::seed_profile(&profile) {
        eprintln!(
            "{NAME}: the profile's preferences ({}): {e}",
            profile.display()
        );
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
        // Chrome's policies from the profile's policy folder (brief 0032, `privacy`).
        chrome_policy_id: path(&crate::privacy::policy_dir(&profile)),
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
                assert_eq!(s.iter().any(|x| x == "disable-gpu"), !gpu);
                assert_eq!(s.iter().any(|x| x == "ozone-platform=headless"), !display);
                assert!(s.iter().any(|x| x == "enable-logging=stderr"));
                assert!(s.iter().any(|x| x == "disable-component-update"));
                assert!(s.iter().any(|x| x.starts_with("disable-features=")
                    && x.contains("NetworkTimeServiceQuerying,")));
            }
        }
    }
}
