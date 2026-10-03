//! [`EmbeddedChromium`]: the second [`Engine`] (brief 0031, proposal 0002 section 3, ADR-0008): `eludite-chromium`,
//! CEF's browser process, launched by the shell and spoken to over JSON-RPC on its stdio
//! (`protocol/schemas/browser-rpc/browser-rpc.md`). Nothing of CEF is loaded in this process.
//!
//! - **Frames.** Each tab renders off screen into a shared-memory ring (a 4096-byte header, two BGRA slots) whose
//!   descriptor arrives over a Unix socket handed to the engine as descriptor 3 (`SCM_RIGHTS`, once per region, never
//!   per frame). [`TabFrames`] maps it and is the tab's [`FrameSource`]: the sequence, size, pixels and dirty
//!   rectangles of the newest frame, and a listener the shell wakes its view with.
//! - **CDP.** `tab/cdp` carries a command to the tab's DevTools agent and `tab/cdpEvent` brings answers and events
//!   back; requests are correlated by id here, as [`crate::connection`] does on a websocket. A tab is its own session
//!   (the agent is the page target), so the commands of [`crate::Browser`] run unchanged.
//! - **Screenshots** come from the ring when the tab has painted and the clip lies in the viewport (after two
//!   animation frames, so the page's last change is painted); `Page.captureScreenshot` otherwise.
//! - **A crash** of the engine closes its stdout: pending requests fail, subscriptions end, the log says so, and the
//!   next [`Engine::launch`] starts a new engine (the tabs are lost, the shell is not).
//! - **Discovery** ([`ChromiumSearch`]): the engine in `ELUDITE_CHROMIUM`, beside this executable, then the cargo
//!   target folder (development); CEF in `ELUDITE_CEF`, `CEF_PATH`, the fetch script's cache, then beside the engine.
//!
//! - **The Web Browser window** (brief 0032). The engine's notifications for the window (state, cursors, popups,
//!   dialogs, permission prompts, downloads, context menus, closed tabs) reach an [`EngineObserver`] the shell sets
//!   ([`EmbeddedChromium::set_observer`]) as [`EngineEvent`]s, with [`EngineEvent::Started`] handing it a
//!   [`TabControl`] to draw and drive the tabs with. Dialogs and prompts are also kept here, so `dialog` and `input`
//!   see them ([`Engine::pending_dialog`]); [`download_line`] is a download's line for the Output window.
//!
//! Linux only so far: macOS (`shm_open` over the same socket) and Windows (named file mappings) are specified in
//! browser-rpc.md; on them [`Engine::launch`] fails with a message.

// Shared memory, descriptor passing and descriptor 3 in the child are system calls; each use is commented.
#![allow(unsafe_code)]
// Off Unix the engine cannot launch yet, so the frame ring and the control channel are unused there until the
// Windows and macOS transports of browser-rpc.md land.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::LogSink;
use crate::chrome::{NO_SANDBOX_ENV, prepare_profile};
use crate::connection::{CdpError, CdpEvent, DEFAULT_TIMEOUT};
use crate::engine::{
    DialogAnswer, Engine, EngineConfig, EngineError, LaunchInfo, PendingDialog, TabHistory,
    TargetInfo,
};

/// The engine's executable name.
pub const ENGINE_NAME: &str = "eludite-chromium";
/// Where the engine is named explicitly.
pub const ENGINE_ENV: &str = "ELUDITE_CHROMIUM";
/// Where CEF is named explicitly (before `CEF_PATH`, the `cef` crate's build variable).
pub const CEF_ENV: &str = "ELUDITE_CEF";
/// The CEF version of `tools/cef/PIN`, the fetch script's cache folder name.
pub const CEF_VERSION: &str = "154.0.32+g682c378+chromium-154.0.8037.58";
/// The protocol version of browser-rpc.md.
pub const PROTOCOL_VERSION: u64 = 1;
/// How long the engine may take to answer `initialize` (CEF initializes before it reads stdin).
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const STDERR_TAIL: usize = 30;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- discovery ----

/// Where to look for the engine and for CEF.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChromiumSearch {
    /// `ELUDITE_CHROMIUM`.
    pub engine: Option<PathBuf>,
    /// Folders that may hold the engine, in order: beside the running executable, then the cargo target folder.
    pub engine_dirs: Vec<PathBuf>,
    /// `ELUDITE_CEF`, then `CEF_PATH`.
    pub cef: Vec<PathBuf>,
    /// The fetch script's cache: `~/.cache/eludite/cef/<version>`.
    pub cef_cache: Option<PathBuf>,
}

/// The file whose presence makes a folder a CEF folder.
fn cef_library() -> &'static str {
    if cfg!(windows) {
        "libcef.dll"
    } else if cfg!(target_os = "macos") {
        "Chromium Embedded Framework.framework"
    } else {
        "libcef.so"
    }
}

fn exe_name() -> String {
    format!("{ENGINE_NAME}{}", std::env::consts::EXE_SUFFIX)
}

impl ChromiumSearch {
    /// From the environment and the running executable's location.
    pub fn defaults() -> Self {
        let mut engine_dirs = Vec::new();
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        {
            // Installed: beside eludite. Development: target/<profile>/ (and a test binary in target/<profile>/deps).
            engine_dirs.push(dir.clone());
            if dir.file_name().is_some_and(|n| n == "deps")
                && let Some(up) = dir.parent()
            {
                engine_dirs.push(up.to_path_buf());
            }
        }
        if let Some(t) = std::env::var_os("CARGO_TARGET_DIR") {
            engine_dirs.push(PathBuf::from(t).join("debug"));
        }
        let cef = [CEF_ENV, "CEF_PATH"]
            .iter()
            .filter_map(std::env::var_os)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .collect();
        let cef_cache = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| {
                PathBuf::from(h)
                    .join(".cache")
                    .join("eludite")
                    .join("cef")
                    .join(CEF_VERSION)
            });
        Self {
            engine: std::env::var_os(ENGINE_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            engine_dirs,
            cef,
            cef_cache,
        }
    }

    /// The engine executable.
    pub fn find_engine(&self) -> Result<PathBuf, String> {
        if let Some(p) = &self.engine {
            return if p.is_file() {
                Ok(p.clone())
            } else {
                Err(format!(
                    "{ENGINE_ENV} names {}, which does not exist",
                    p.display()
                ))
            };
        }
        let name = exe_name();
        self.engine_dirs
            .iter()
            .map(|d| d.join(&name))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                format!(
                    "{ENGINE_NAME} was not found ({ENGINE_ENV}, {}); build it with `cargo build -p eludite-chromium \
                     --features eludite-chromium/cef` after tools/cef/fetch.sh",
                    self.engine_dirs
                        .iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    /// CEF's folder, for the engine at `engine`.
    pub fn find_cef(&self, engine: &Path) -> Result<PathBuf, String> {
        let lib = cef_library();
        let beside = engine.parent().map(Path::to_path_buf);
        self.cef
            .iter()
            .chain(self.cef_cache.iter())
            .chain(beside.iter())
            .find(|d| d.join(lib).exists())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "CEF {CEF_VERSION} was not found ({CEF_ENV}, CEF_PATH, {}, beside {}); run tools/cef/fetch.sh",
                    self.cef_cache
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    engine.display()
                )
            })
    }
}

// ---- what the Web Browser window hears (brief 0032) ----

/// What the engine tells the shell, for the Web Browser window.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// The engine started: the handle the window draws and drives its tabs through.
    Started(TabControl),
    /// The engine exited or was closed: its tabs are gone.
    Stopped,
    /// A notification as the engine sent it: `tab/state`, `tab/cursor`, `tab/popup`, `tab/dialog`,
    /// `tab/permission`, `tab/dialogClosed`, `tab/download`, `tab/contextMenu` or `tab/closed`.
    Notification { method: String, params: Value },
    /// DevTools opened for tab `page` as tab `devtools` (engine tab ids).
    DevtoolsOpened { page: String, devtools: String },
}

/// Where [`EngineEvent`]s go; called on the engine's reader thread (or the thread that launched it), so it must
/// not block.
pub type EngineObserver = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// The notifications an [`EngineObserver`] hears.
pub const WINDOW_NOTIFICATIONS: [&str; 9] = [
    "tab/state",
    "tab/cursor",
    "tab/popup",
    "tab/dialog",
    "tab/permission",
    "tab/dialogClosed",
    "tab/download",
    "tab/contextMenu",
    "tab/closed",
];

/// The Output window's line for a `tab/download` notification: its end (complete, refused, canceled, interrupted);
/// `None` for its start and progress, which only the Web Browser window shows.
pub fn download_line(p: &Value) -> Option<String> {
    let url = p["url"].as_str().unwrap_or_default();
    Some(match p["state"].as_str().unwrap_or_default() {
        "complete" => format!(
            "Downloaded {url} to {} ({} bytes)",
            p["path"].as_str().unwrap_or_default(),
            p["receivedBytes"].as_u64().unwrap_or(0)
        ),
        "refused" => format!(
            "Refused the download of {url}: {}",
            p["message"].as_str().unwrap_or("over the limit")
        ),
        "canceled" => format!("The download of {url} was canceled"),
        "interrupted" => format!(
            "The download of {url} stopped: {}",
            p["message"].as_str().unwrap_or("interrupted")
        ),
        _ => return None,
    })
}

/// A tab's state as the engine last reported it (`tab/state`, `tab/cursor`, `tab/popup`, DevTools).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabInfo {
    pub url: String,
    pub title: String,
    pub loading: bool,
    /// A `data:` url, or empty.
    pub favicon: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// CSS's cursor keyword.
    pub cursor: String,
    pub status: String,
    /// A popup: the tab whose page opened it.
    pub opener: Option<String>,
    /// A DevTools tab: the page it inspects.
    pub devtools_of: Option<String>,
}

// ---- which engine (brief 0032) ----

/// The setting `browser.engine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineChoice {
    /// Eludite's own Chromium, drawn in the Web Browser window, when it and CEF are found.
    #[default]
    Embedded,
    /// A Chrome or Chromium process with its own window (brief 0023).
    External,
}

impl EngineChoice {
    /// From the setting's value; anything else is the default, `embedded`.
    pub fn from_setting(value: &str) -> Self {
        match value.trim() {
            "external" => EngineChoice::External,
            _ => EngineChoice::Embedded,
        }
    }
}

/// The engine that runs for `choice`: the embedded one when it is chosen and `eludite-chromium` and CEF are found,
/// else the external Chrome. The second member says why the embedded engine is not used when it was chosen, with
/// what to run (the Web Browser window shows it).
pub fn select_engine(
    choice: EngineChoice,
    search: &ChromiumSearch,
) -> (EngineChoice, Option<String>) {
    if choice == EngineChoice::External {
        return (EngineChoice::External, None);
    }
    if !cfg!(target_os = "linux") {
        return (
            EngineChoice::External,
            Some(
                "the embedded browser runs on Linux only so far; the browser tools use the external Chrome".into(),
            ),
        );
    }
    match search.find_engine().and_then(|e| search.find_cef(&e)) {
        Ok(_) => (EngineChoice::Embedded, None),
        Err(why) => (
            EngineChoice::External,
            Some(format!(
                "{why}. Fetch CEF with `tools/cef/fetch.sh` and build the engine with `CEF_PATH=\"$(tools/cef/fetch.sh)\" \
                 cargo build -p eludite-chromium --features eludite-chromium/cef`; meanwhile the browser tools use \
                 the external Chrome"
            )),
        ),
    }
}

// ---- the frame ring (browser-rpc.md, "The frame ring") ----

const MAGIC: u32 = 0x5242_4C45;
const HEADER_SIZE: usize = 4096;
const SLOTS: usize = 2;
const SLOT_HEADER: usize = 64;
const SLOT_STRIDE: usize = 320;
const MAX_DIRTY: usize = 16;
const FREE: u32 = 0;
const READY: u32 = 2;
const READING: u32 = 3;

/// A dirty rectangle in device pixels of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirtyRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A frame as a [`FrameSource`] lends it: valid for the callback only.
#[derive(Debug)]
pub struct Frame<'a> {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// BGRA, premultiplied alpha, top row first.
    pub pixels: &'a [u8],
    pub dirty: &'a [DirtyRect],
    /// `CLOCK_MONOTONIC` nanoseconds when the engine's `OnPaint` began ([`monotonic_ns`] is the same clock).
    pub paint_ns: u64,
    /// The engine's copy into the slot.
    pub copy_ns: u64,
}

/// The frames of one tab, for a view that draws them.
pub trait FrameSource: Send + Sync {
    /// The newest frame sequence the engine announced (0: none yet).
    fn sequence(&self) -> u64;

    /// Lend the newest ready frame to `f` and free its slot (`consume`), or lend the newest frame whatever its state
    /// and leave it (`!consume`). False when there was none.
    fn read(&self, consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool;

    /// Called (on the engine's reader thread) whenever a frame is announced; replaces the previous listener.
    fn set_listener(&self, f: Option<Box<dyn Fn() + Send + Sync>>);

    /// The tab is gone: no frame will come.
    fn is_closed(&self) -> bool {
        false
    }
}

/// `CLOCK_MONOTONIC` in nanoseconds: the engine's `paintNs` clock.
#[cfg(unix)]
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime fills the timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

#[cfg(not(unix))]
pub fn monotonic_ns() -> u64 {
    0
}

#[cfg(unix)]
struct Map {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: plain shared memory; states are accessed atomically, pixels only while the slot is held in reading.
#[cfg(unix)]
unsafe impl Send for Map {}
#[cfg(unix)]
unsafe impl Sync for Map {}

#[cfg(unix)]
impl Map {
    fn new(fd: i32, len: usize, offset: usize, write: bool) -> std::io::Result<Map> {
        let prot = libc::PROT_READ | if write { libc::PROT_WRITE } else { 0 };
        // SAFETY: mapping a descriptor we own, within its size; the result is checked.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Map {
            ptr: ptr.cast(),
            len,
        })
    }

    fn u32_at(&self, off: usize) -> u32 {
        assert!(off + 4 <= self.len);
        // SAFETY: in bounds and 4-byte aligned by the layout.
        u32::from_le(unsafe { std::ptr::read_volatile(self.ptr.add(off).cast::<u32>()) })
    }

    fn u64_at(&self, off: usize) -> u64 {
        assert!(off + 8 <= self.len);
        // SAFETY: in bounds and 8-byte aligned by the layout.
        u64::from_le(unsafe { std::ptr::read_volatile(self.ptr.add(off).cast::<u64>()) })
    }

    fn state(&self, slot: usize) -> &std::sync::atomic::AtomicU32 {
        // SAFETY: the 4-byte aligned state word inside the header page, alive as long as the mapping; both processes
        // only touch it atomically.
        unsafe {
            &*self
                .ptr
                .add(SLOT_HEADER + slot * SLOT_STRIDE)
                .cast::<std::sync::atomic::AtomicU32>()
        }
    }
}

#[cfg(unix)]
impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmapping what Map::new mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// The shell's mapping of one region: the header page read-write (it changes `state`), the slots read-only.
#[cfg(unix)]
pub struct Ring {
    pub id: u64,
    pub slot_size: usize,
    header: Map,
    slots: Map,
}

#[cfg(unix)]
struct Meta {
    width: u32,
    height: u32,
    stride: u32,
    sequence: u64,
    paint_ns: u64,
    copy_ns: u64,
    dirty: Vec<DirtyRect>,
}

#[cfg(unix)]
impl Ring {
    /// Map a region from its descriptor (which may be closed afterwards).
    pub fn open(fd: i32) -> Result<Ring, String> {
        let header =
            Map::new(fd, HEADER_SIZE, 0, true).map_err(|e| format!("mapping the header: {e}"))?;
        if header.u32_at(0) != MAGIC || header.u32_at(4) != 1 || header.u32_at(8) != SLOTS as u32 {
            return Err("not a version 1 frame region".into());
        }
        let slot_size = header.u64_at(16) as usize;
        let slots = Map::new(fd, SLOTS * slot_size, HEADER_SIZE, false)
            .map_err(|e| format!("mapping the slots: {e}"))?;
        Ok(Ring {
            id: header.u64_at(32),
            slot_size,
            header,
            slots,
        })
    }

    fn meta(&self, slot: usize) -> Meta {
        let o = SLOT_HEADER + slot * SLOT_STRIDE;
        let h = &self.header;
        let n = (h.u32_at(o + 40) as usize).min(MAX_DIRTY);
        Meta {
            width: h.u32_at(o + 4),
            height: h.u32_at(o + 8),
            stride: h.u32_at(o + 12),
            sequence: h.u64_at(o + 16),
            paint_ns: h.u64_at(o + 24),
            copy_ns: h.u64_at(o + 32),
            dirty: (0..n)
                .map(|i| {
                    let r = o + 48 + i * 16;
                    DirtyRect {
                        x: h.u32_at(r) as i32,
                        y: h.u32_at(r + 4) as i32,
                        width: h.u32_at(r + 8) as i32,
                        height: h.u32_at(r + 12) as i32,
                    }
                })
                .collect(),
        }
    }

    /// Lend the newest frame in one of `states` to `f`; the slot is freed after (`consume`) or left as found.
    pub fn read(&self, consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool {
        let states: &[u32] = if consume { &[READY] } else { &[READY, FREE] };
        let mut order: Vec<usize> = (0..SLOTS).collect();
        order.sort_by_key(|&s| std::cmp::Reverse(self.meta(s).sequence));
        for s in order {
            for &from in states {
                if self
                    .header
                    .state(s)
                    .compare_exchange(from, READING, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                let m = self.meta(s);
                let len = m.stride as usize * m.height as usize;
                if m.sequence == 0 || len > self.slot_size || m.stride < m.width * 4 {
                    self.header.state(s).store(from, Ordering::Release);
                    return false;
                }
                // SAFETY: inside the read-only slot mapping, held in reading so the engine does not write it.
                let pixels = unsafe {
                    std::slice::from_raw_parts(self.slots.ptr.add(s * self.slot_size), len)
                };
                f(&Frame {
                    sequence: m.sequence,
                    width: m.width,
                    height: m.height,
                    stride: m.stride,
                    pixels,
                    dirty: &m.dirty,
                    paint_ns: m.paint_ns,
                    copy_ns: m.copy_ns,
                });
                self.header
                    .state(s)
                    .store(if consume { FREE } else { from }, Ordering::Release);
                return true;
            }
        }
        false
    }
}

/// A tab's frames and state, shared by the reader thread and the shell's view.
#[derive(Default)]
pub struct TabFrames {
    #[cfg(unix)]
    ring: Mutex<Option<Arc<Ring>>>,
    sequence: AtomicU64,
    listener: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    created: Mutex<Option<Instant>>,
    first_frame: Mutex<Option<Instant>>,
    frames: AtomicU64,
    info: Mutex<TabInfo>,
    closed: AtomicBool,
}

impl std::fmt::Debug for TabFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabFrames")
            .field("sequence", &self.sequence())
            .field("frames", &self.frames.load(Ordering::Relaxed))
            .finish()
    }
}

impl TabFrames {
    /// From `tab/create` to the first `tab/frame` of the tab.
    pub fn first_frame_after_create(&self) -> Option<Duration> {
        let c = (*lock(&self.created))?;
        let f = (*lock(&self.first_frame))?;
        Some(f.saturating_duration_since(c))
    }

    /// `tab/frame` notifications received.
    pub fn frames_announced(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The tab's state as the engine last reported it.
    pub fn info(&self) -> TabInfo {
        lock(&self.info).clone()
    }

    fn announce(&self, sequence: u64) {
        self.sequence.fetch_max(sequence, Ordering::AcqRel);
        self.frames.fetch_add(1, Ordering::Relaxed);
        lock(&self.first_frame).get_or_insert_with(Instant::now);
        let l = lock(&self.listener).clone();
        if let Some(l) = l {
            l();
        }
    }
}

impl FrameSource for TabFrames {
    fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    #[cfg(unix)]
    fn read(&self, consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool {
        let ring = lock(&self.ring).clone();
        ring.is_some_and(|r| r.read(consume, f))
    }

    #[cfg(not(unix))]
    fn read(&self, _consume: bool, _f: &mut dyn FnMut(&Frame<'_>)) -> bool {
        false
    }

    fn set_listener(&self, f: Option<Box<dyn Fn() + Send + Sync>>) {
        *lock(&self.listener) = f.map(Arc::from);
    }

    fn is_closed(&self) -> bool {
        TabFrames::is_closed(self)
    }
}

// ---- the control channel ----

type RpcReply = mpsc::Sender<Result<Value, String>>;
type CdpReply = mpsc::Sender<Result<Value, CdpError>>;

/// One running engine: its stdin, the requests and CDP calls in flight, the tabs.
struct Control {
    stdin: Mutex<Option<ChildStdin>>,
    /// The Web Browser window's ears (brief 0032).
    observer: Option<EngineObserver>,
    /// Dialogs and prompts pages wait on, by the engine's id.
    prompts: Mutex<BTreeMap<u64, (String, PendingDialog)>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, RpcReply>>,
    cdp_next: AtomicU64,
    cdp_pending: Mutex<HashMap<u64, (String, CdpReply)>>,
    subscribers: Mutex<HashMap<String, Vec<mpsc::Sender<CdpEvent>>>>,
    tabs: Mutex<Vec<(String, Arc<TabFrames>)>>,
    closed: AtomicBool,
}

impl Control {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn write(&self, v: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        let mut guard = lock(&self.stdin);
        let w = guard.as_mut().ok_or("the engine is not running")?;
        eludite_protocol::framing::write_message(w, &body)
            .and_then(|_| w.flush())
            .map_err(|e| format!("writing to the engine: {e}"))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the engine is not running".into());
        }
        let id = self.next_id.fetch_add(1, Ordering::AcqRel) + 1;
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        if let Err(e) =
            self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                Err(format!(
                    "the engine did not answer {method} in {} ms",
                    timeout.as_millis()
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("the engine is not running".into()),
        }
    }

    fn cdp_send(
        &self,
        tab: &str,
        method: &str,
        params: Value,
    ) -> Result<mpsc::Receiver<Result<Value, CdpError>>, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.cdp_next.fetch_add(1, Ordering::AcqRel) + 1;
        let (tx, rx) = mpsc::channel();
        lock(&self.cdp_pending).insert(id, (method.to_owned(), tx));
        let params = if params.is_null() { json!({}) } else { params };
        let msg = json!({"tab": tab, "message": {"id": id, "method": method, "params": params}});
        if let Err(e) = self.notify("tab/cdp", msg) {
            lock(&self.cdp_pending).remove(&id);
            return Err(if self.is_closed() {
                CdpError::Closed
            } else {
                CdpError::Io(e)
            });
        }
        Ok(rx)
    }

    fn tab(&self, tab: &str) -> Option<Arc<TabFrames>> {
        lock(&self.tabs)
            .iter()
            .find(|(t, _)| t == tab)
            .map(|(_, f)| f.clone())
    }

    /// Fail everything pending and end every subscription.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        for (_, tx) in lock(&self.pending).drain() {
            let _ = tx.send(Err("the engine exited".into()));
        }
        for (_, (_, tx)) in lock(&self.cdp_pending).drain() {
            let _ = tx.send(Err(CdpError::Closed));
        }
        lock(&self.subscribers).clear();
        for (_, f) in lock(&self.tabs).drain(..) {
            f.closed.store(true, Ordering::Release);
        }
    }

    /// One message from the engine (on the reader thread).
    fn dispatch(&self, v: Value, sock: Option<i32>) {
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            if let Some(tx) = lock(&self.pending).remove(&id) {
                let r = match v.get("error") {
                    Some(e) => Err(e["message"].as_str().unwrap_or("error").to_owned()),
                    None => Ok(v["result"].clone()),
                };
                let _ = tx.send(r);
            }
            return;
        }
        let p = &v["params"];
        let tab = p["tab"].as_str().unwrap_or_default().to_owned();
        match v["method"].as_str().unwrap_or_default() {
            "tab/resized" => self.resized(&tab, p, sock),
            "tab/frame" => {
                if let Some(f) = self.tab(&tab) {
                    f.announce(p["sequence"].as_u64().unwrap_or(0));
                }
            }
            "tab/cdpEvent" => self.cdp_event(&tab, &p["message"]),
            "tab/state" => {
                if let Some(f) = self.tab(&tab) {
                    let mut i = lock(&f.info);
                    let text = |k: &str, to: &mut String| {
                        if let Some(v) = p[k].as_str() {
                            v.clone_into(to);
                        }
                    };
                    text("url", &mut i.url);
                    text("title", &mut i.title);
                    text("favicon", &mut i.favicon);
                    text("statusText", &mut i.status);
                    let flag = |k: &str, to: &mut bool| {
                        if let Some(v) = p[k].as_bool() {
                            *to = v;
                        }
                    };
                    flag("loading", &mut i.loading);
                    flag("canGoBack", &mut i.can_go_back);
                    flag("canGoForward", &mut i.can_go_forward);
                }
            }
            "tab/cursor" => {
                if let Some(f) = self.tab(&tab)
                    && let Some(c) = p["cursor"].as_str()
                {
                    c.clone_into(&mut lock(&f.info).cursor);
                }
            }
            "tab/popup" => {
                if let Some(f) = self.tab(&tab) {
                    lock(&f.info).opener = p["opener"].as_str().map(str::to_owned);
                }
            }
            "tab/dialog" | "tab/permission" => {
                if let Some(id) = p["id"].as_u64() {
                    let permission = v["method"] == "tab/permission";
                    let message = if permission {
                        format!(
                            "{} wants to use: {}",
                            p["origin"].as_str().unwrap_or("the page"),
                            p["permissions"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    } else {
                        p["message"].as_str().unwrap_or_default().to_owned()
                    };
                    let d = PendingDialog {
                        id,
                        kind: if permission {
                            "permission".into()
                        } else {
                            p["kind"].as_str().unwrap_or("alert").to_owned()
                        },
                        message,
                        default_text: p["defaultText"].as_str().map(str::to_owned),
                    };
                    lock(&self.prompts).insert(id, (tab.clone(), d));
                }
            }
            "tab/dialogClosed" => {
                if let Some(id) = p["id"].as_u64() {
                    lock(&self.prompts).remove(&id);
                }
            }
            "tab/closed" => {
                lock(&self.subscribers).remove(&tab);
                lock(&self.prompts).retain(|_, (t, _)| *t != tab);
                let mut tabs = lock(&self.tabs);
                if let Some(i) = tabs.iter().position(|(t, _)| *t == tab) {
                    tabs.remove(i).1.closed.store(true, Ordering::Release);
                }
            }
            _ => {}
        }
        if let Some(o) = &self.observer
            && let Some(m) = v["method"].as_str()
            && WINDOW_NOTIFICATIONS.contains(&m)
        {
            o(EngineEvent::Notification {
                method: m.to_owned(),
                params: p.clone(),
            });
        }
    }

    /// The first dialog or prompt `tab`'s page waits on.
    fn pending(&self, tab: &str) -> Option<PendingDialog> {
        lock(&self.prompts)
            .values()
            .find(|(t, _)| t == tab)
            .map(|(_, d)| d.clone())
    }

    #[cfg(unix)]
    fn resized(&self, tab: &str, p: &Value, sock: Option<i32>) {
        let Some(sock) = sock else { return };
        let want = p["region"]["id"].as_u64().unwrap_or(0);
        match recv_fd(sock) {
            Ok((id, fd)) if id == want => {
                let ring = Ring::open(std::os::fd::AsRawFd::as_raw_fd(&fd));
                match ring {
                    Ok(r) => {
                        let frames = self.tab(tab).unwrap_or_else(|| {
                            // The region arrives before tab/create's (or tab/devtools') answer, and before tab/popup:
                            // the tab starts here.
                            let f = Arc::new(TabFrames::default());
                            lock(&self.tabs).push((tab.to_owned(), f.clone()));
                            f
                        });
                        *lock(&frames.ring) = Some(Arc::new(r));
                    }
                    Err(e) => eprintln!("eludite-browser: tab {tab}: region {id}: {e}"),
                }
            }
            Ok((id, _)) => {
                eprintln!("eludite-browser: tab {tab}: expected region {want}, got {id}")
            }
            Err(e) => eprintln!("eludite-browser: tab {tab}: no descriptor for region {want}: {e}"),
        }
    }

    #[cfg(not(unix))]
    fn resized(&self, _tab: &str, _p: &Value, _sock: Option<i32>) {}

    fn cdp_event(&self, tab: &str, m: &Value) {
        if let Some(id) = m.get("id").and_then(Value::as_u64) {
            if let Some((method, tx)) = lock(&self.cdp_pending).remove(&id) {
                let r = match m.get("error") {
                    Some(e) => Err(CdpError::Protocol {
                        method,
                        code: e["code"].as_i64().unwrap_or(-32000),
                        message: e["message"].as_str().unwrap_or("error").to_owned(),
                    }),
                    None => Ok(m.get("result").cloned().unwrap_or(json!({}))),
                };
                let _ = tx.send(r);
            }
            return;
        }
        let Some(method) = m["method"].as_str() else {
            return;
        };
        let event = CdpEvent {
            method: method.to_owned(),
            params: m.get("params").cloned().unwrap_or(json!({})),
            session_id: Some(tab.to_owned()),
        };
        if let Some(subs) = lock(&self.subscribers).get_mut(tab) {
            subs.retain(|s| s.send(event.clone()).is_ok());
        }
    }
}

fn wait_cdp(
    rx: &mpsc::Receiver<Result<Value, CdpError>>,
    method: &str,
    deadline: Instant,
    timeout: Duration,
) -> Result<Value, CdpError> {
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CdpError::Timeout {
            method: method.to_owned(),
            after: timeout,
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CdpError::Closed),
    }
}

// ---- descriptors (Linux) ----

#[cfg(unix)]
fn socket_pair() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0i32; 2];
    // SAFETY: socketpair fills the array; the result is checked.
    let r = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: two new descriptors this process owns.
    Ok(unsafe {
        (
            std::os::fd::OwnedFd::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    })
}

/// One descriptor and its 8-byte payload (the region id), as the engine sends them.
#[cfg(unix)]
fn recv_fd(sock: i32) -> std::io::Result<(u64, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut payload = [0u8; 8];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: a size computation.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: plain data; the pointers set below outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: a valid msghdr; the result is checked.
    let n = unsafe { libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n <= 0 {
        return Err(if n == 0 {
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the frame socket closed")
        } else {
            std::io::Error::last_os_error()
        });
    }
    // SAFETY: reading the control message recvmsg filled in.
    let fd = unsafe {
        let c = libc::CMSG_FIRSTHDR(&msg);
        if c.is_null() || (*c).cmsg_level != libc::SOL_SOCKET || (*c).cmsg_type != libc::SCM_RIGHTS
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no descriptor in the message",
            ));
        }
        std::ptr::read_unaligned(libc::CMSG_DATA(c).cast::<i32>())
    };
    // SAFETY: SCM_RIGHTS gave this process a new descriptor.
    Ok((u64::from_le_bytes(payload), unsafe {
        std::os::fd::OwnedFd::from_raw_fd(fd)
    }))
}

// ---- the engine ----

struct Running {
    control: Arc<Control>,
    child: Arc<Mutex<Option<Child>>>,
    info: LaunchInfo,
    stopping: Arc<AtomicBool>,
    pid: u32,
    #[cfg(unix)]
    _sock: std::os::fd::OwnedFd,
}

/// Counters a test or the shell's bench keeps after handing the engine to a [`crate::Browser`].
#[derive(Debug, Default)]
pub struct EmbeddedStats {
    /// Screenshots answered from the frame ring.
    pub ring_screenshots: AtomicU64,
    /// Screenshots answered by `Page.captureScreenshot`.
    pub cdp_screenshots: AtomicU64,
    /// The running engine's process id (0: none).
    pub pid: std::sync::atomic::AtomicU32,
    /// Spawn to `initialize`'s answer of the last launch, microseconds.
    pub launch_us: AtomicU64,
}

/// `eludite-chromium` as an [`Engine`].
pub struct EmbeddedChromium {
    config: EngineConfig,
    search: ChromiumSearch,
    no_sandbox: bool,
    log: LogSink,
    running: Option<Running>,
    /// From spawn to `initialize`'s answer, of the last launch.
    last_launch: Option<Duration>,
    stats: Arc<EmbeddedStats>,
    /// The Web Browser window's ears (brief 0032).
    observer: Option<EngineObserver>,
}

impl std::fmt::Debug for EmbeddedChromium {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedChromium")
            .field("config", &self.config)
            .field("running", &self.is_running())
            .finish()
    }
}

impl EmbeddedChromium {
    /// An engine that starts nothing until [`Engine::launch`]. `--no-sandbox` follows `ELUDITE_CHROME_NO_SANDBOX`.
    pub fn new(config: EngineConfig, search: ChromiumSearch, log: LogSink) -> Self {
        Self {
            config,
            search,
            no_sandbox: crate::chrome::no_sandbox_from_env(),
            log,
            running: None,
            last_launch: None,
            stats: Arc::default(),
            observer: None,
        }
    }

    /// Tell `observer` what the engine says for the Web Browser window, from the next launch on.
    pub fn set_observer(&mut self, observer: Option<EngineObserver>) {
        self.observer = observer;
    }

    /// The engine's counters, shared.
    pub fn stats(&self) -> Arc<EmbeddedStats> {
        self.stats.clone()
    }

    /// Pass `ELUDITE_CHROME_NO_SANDBOX=1` to the engine (or not) regardless of this process's environment: for tests
    /// running as root, which cannot set the variable for themselves without `unsafe`. The shell never calls it.
    pub fn no_sandbox(mut self, on: bool) -> Self {
        self.no_sandbox = on;
        self
    }

    fn control(&self) -> Result<&Arc<Control>, EngineError> {
        match &self.running {
            Some(r) if !r.control.is_closed() => Ok(&r.control),
            _ => Err(EngineError::NotRunning),
        }
    }

    /// The engine's process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.running
            .as_ref()
            .filter(|r| !r.control.is_closed())
            .map(|r| r.pid)
    }

    /// Spawn to `initialize`'s answer, of the last launch.
    pub fn last_launch(&self) -> Option<Duration> {
        self.last_launch
    }

    /// A tab's frames (for a view that draws them).
    pub fn frames(&self, tab: &str) -> Option<Arc<TabFrames>> {
        self.control().ok()?.tab(tab)
    }

    /// Forward one input event (browser-rpc.md, `tab/input`); never waits for the engine.
    pub fn input(&self, tab: &str, event: Value) -> Result<(), EngineError> {
        self.control()?
            .notify("tab/input", json!({"tab": tab, "event": event}))
            .map_err(EngineError::Launch)
    }

    /// The tab's view size in CSS pixels and its scale.
    pub fn resize_tab(
        &self,
        tab: &str,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<(), EngineError> {
        self.control()?
            .request(
                "tab/resize",
                json!({"tab": tab, "width": width, "height": height, "scale": scale}),
                DEFAULT_TIMEOUT,
            )
            .map(|_| ())
            .map_err(EngineError::Launch)
    }

    /// A cheap handle the shell can keep on another thread to forward input and resizes without the engine.
    pub fn handle(&self) -> Option<TabControl> {
        Some(TabControl(self.control().ok()?.clone()))
    }

    #[cfg(not(unix))]
    fn start(&mut self) -> Result<LaunchInfo, EngineError> {
        Err(EngineError::Launch(
            "the embedded browser runs on Linux only so far (brief 0031); use the external Chrome"
                .into(),
        ))
    }

    #[cfg(unix)]
    fn start(&mut self) -> Result<LaunchInfo, EngineError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let exe = self.search.find_engine().map_err(EngineError::Launch)?;
        let cef = self.search.find_cef(&exe).map_err(EngineError::Launch)?;
        prepare_profile(&self.config.profile_dir).map_err(|e| {
            EngineError::Launch(format!(
                "cannot create the browser profile {}: {e}",
                self.config.profile_dir.display()
            ))
        })?;
        let (ours, theirs) =
            socket_pair().map_err(|e| EngineError::Launch(format!("socketpair: {e}")))?;
        let theirs_fd = theirs.as_raw_fd();
        let mut cmd = Command::new(&exe);
        cmd.arg("--profile")
            .arg(&self.config.profile_dir)
            .arg("--cef-dir")
            .arg(&cef)
            .args(["--frame-socket", "3"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let lib_var = if cfg!(target_os = "macos") {
            "DYLD_FALLBACK_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut lib_path = std::ffi::OsString::from(cef.as_os_str());
        if let Some(old) = std::env::var_os(lib_var).filter(|v| !v.is_empty()) {
            lib_path.push(":");
            lib_path.push(old);
        }
        cmd.env(lib_var, lib_path);
        if self.no_sandbox {
            cmd.env(NO_SANDBOX_ENV, "1");
        } else {
            cmd.env_remove(NO_SANDBOX_ENV);
        }
        // SAFETY: dup2 is async-signal-safe; it gives the child the socket as descriptor 3 (the copy is not
        // close-on-exec), which is all browser-rpc.md asks.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(theirs_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let started = Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| EngineError::Launch(format!("cannot start {}: {e}", exe.display())))?;
        drop(theirs);
        let pid = child.id();
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stdin = child.stdin.take().expect("piped");
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let drain_tail = tail.clone();
        let _ = std::thread::Builder::new()
            .name("chromium-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    let mut t = lock(&drain_tail);
                    if t.len() == STDERR_TAIL {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            });
        let control = Arc::new(Control {
            stdin: Mutex::new(Some(stdin)),
            observer: self.observer.clone(),
            prompts: Mutex::default(),
            next_id: AtomicU64::new(0),
            pending: Mutex::default(),
            cdp_next: AtomicU64::new(0),
            cdp_pending: Mutex::default(),
            subscribers: Mutex::default(),
            tabs: Mutex::default(),
            closed: AtomicBool::new(false),
        });
        let child = Arc::new(Mutex::new(Some(child)));
        let stopping = Arc::new(AtomicBool::new(false));
        let reader_control = control.clone();
        let sock_fd = ours.as_raw_fd();
        let (hook_child, hook_stopping, hook_log, hook_observer) = (
            child.clone(),
            stopping.clone(),
            self.log.clone(),
            self.observer.clone(),
        );
        std::thread::Builder::new()
            .name("chromium-control".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Ok(Some(body)) = eludite_protocol::framing::read_message(&mut r) {
                    match serde_json::from_slice::<Value>(&body) {
                        Ok(v) => reader_control.dispatch(v, Some(sock_fd)),
                        Err(e) => eprintln!("eludite-browser: a bad message from the engine: {e}"),
                    }
                }
                reader_control.close();
                if let Some(o) = &hook_observer {
                    o(EngineEvent::Stopped);
                }
                if hook_stopping.load(Ordering::Acquire) {
                    return;
                }
                // The engine went away on its own (a crash, a kill): say how; the next command relaunches it.
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut status = None;
                while Instant::now() < deadline && status.is_none() {
                    match lock(&hook_child).as_mut().map(Child::try_wait) {
                        Some(Ok(Some(s))) => status = Some(s.to_string()),
                        Some(Ok(None)) => std::thread::sleep(Duration::from_millis(20)),
                        _ => break,
                    }
                }
                if status.is_none()
                    && let Some(c) = lock(&hook_child).as_mut()
                {
                    let _ = c.kill();
                    status = c.wait().ok().map(|s| s.to_string());
                }
                hook_log(&format!(
                    "The embedded browser exited unexpectedly ({}); the next browser command starts it again.",
                    status.unwrap_or_else(|| "status unknown".into())
                ));
            })
            .map_err(|e| EngineError::Launch(e.to_string()))?;
        let fail = |why: String| {
            stopping.store(true, Ordering::Release);
            if let Some(c) = lock(&child).as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
            std::thread::sleep(Duration::from_millis(50));
            let tail: Vec<String> = lock(&tail).iter().cloned().collect();
            EngineError::Launch(format!(
                "{why} ({}).{}",
                exe.display(),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" It said:\n{}", tail.join("\n"))
                }
            ))
        };
        // Downloads go beside the profile: the workspace's .eludite/browser/downloads (brief 0032).
        let downloads = self
            .config
            .profile_dir
            .parent()
            .map(|p| p.join("downloads"))
            .unwrap_or_else(|| self.config.profile_dir.join("downloads"));
        let init = control.request(
            "initialize",
            json!({
                "clientName": "eludite",
                "clientVersion": env!("CARGO_PKG_VERSION"),
                "protocolVersion": PROTOCOL_VERSION,
                "downloadDir": downloads,
            }),
            LAUNCH_TIMEOUT,
        );
        let init = match init {
            Ok(v) => v,
            Err(e) => {
                // Give stderr a moment: the engine says why it refused (no CEF, no sandbox helper).
                std::thread::sleep(Duration::from_millis(100));
                return Err(fail(format!("the embedded browser did not start: {e}")));
            }
        };
        let launch = started.elapsed();
        self.last_launch = Some(launch);
        self.stats.pid.store(pid, Ordering::Relaxed);
        self.stats
            .launch_us
            .store(launch.as_micros() as u64, Ordering::Relaxed);
        let info = LaunchInfo {
            executable: exe.display().to_string(),
            version: format!(
                "Chromium/{} (CEF {})",
                init["chromiumVersion"].as_str().unwrap_or("?"),
                init["cefVersion"].as_str().unwrap_or("?")
            ),
            endpoint: format!("stdio (pid {pid})"),
        };
        (self.log)(&format!(
            "Launched the embedded browser {} ({}) in {} ms; CEF {}; profile {}{}",
            info.executable,
            info.version,
            launch.as_millis(),
            cef.display(),
            self.config.profile_dir.display(),
            if init["sandbox"] == json!(false) {
                "; no sandbox (ELUDITE_CHROME_NO_SANDBOX=1)"
            } else {
                ""
            }
        ));
        if let Some(o) = &self.observer {
            o(EngineEvent::Started(TabControl(control.clone())));
        }
        self.running = Some(Running {
            control,
            child,
            info: info.clone(),
            stopping,
            pid,
            _sock: ours,
        });
        Ok(info)
    }

    fn stop(&mut self, say: bool) {
        let Some(r) = self.running.take() else { return };
        r.stopping.store(true, Ordering::Release);
        if !r.control.is_closed() {
            let _ = r
                .control
                .request("shutdown", json!({}), Duration::from_secs(2));
        }
        // A closed stdin is shutdown too, if the request was not answered.
        lock(&r.control.stdin).take();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let mut guard = lock(&r.child);
        if let Some(c) = guard.as_mut() {
            loop {
                match c.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => {
                        let _ = c.kill();
                        let _ = c.wait();
                        break;
                    }
                }
            }
        }
        guard.take();
        drop(guard);
        r.control.close();
        if say {
            (self.log)("Closed the embedded browser.");
        }
    }

    /// The newest frame of the tab, cropped to `clip` (CSS pixels of the document) and encoded, when it is in the
    /// viewport: `None` means use `Page.captureScreenshot`.
    fn ring_screenshot(
        &self,
        tab: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Option<String>, EngineError> {
        let control = self.control()?;
        let Some(frames) = control.tab(tab) else {
            return Ok(None);
        };
        if frames.sequence() == 0 {
            return Ok(None);
        }
        // Let the page paint its last change: two animation frames, then the frame that follows (if any change).
        let before = frames.sequence();
        let _ = self.send(
            tab,
            "Runtime.evaluate",
            json!({
                "expression": "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(0))))",
                "awaitPromise": true,
            }),
            timeout,
        );
        let settle = Instant::now() + Duration::from_millis(17);
        while frames.sequence() == before && Instant::now() < settle {
            std::thread::sleep(Duration::from_millis(2));
        }
        let m = self.send(tab, "Page.getLayoutMetrics", json!({}), timeout)?;
        let css_w = m["cssLayoutViewport"]["clientWidth"].as_f64().unwrap_or(0.);
        let dev_w = m["layoutViewport"]["clientWidth"].as_f64().unwrap_or(0.);
        let dpr = if css_w > 0. && dev_w > 0. {
            dev_w / css_w
        } else {
            1.
        };
        let vv = &m["cssVisualViewport"];
        let (px, py) = (
            vv["pageX"].as_f64().unwrap_or(0.),
            vv["pageY"].as_f64().unwrap_or(0.),
        );
        let clip = &params["clip"];
        let (cx, cy, cw, ch) = (
            clip["x"].as_f64().unwrap_or(px),
            clip["y"].as_f64().unwrap_or(py),
            clip["width"]
                .as_f64()
                .unwrap_or(vv["clientWidth"].as_f64().unwrap_or(0.)),
            clip["height"]
                .as_f64()
                .unwrap_or(vv["clientHeight"].as_f64().unwrap_or(0.)),
        );
        let scale = clip["scale"].as_f64().unwrap_or(1.).clamp(0.01, 1.);
        let format = params["format"].as_str().unwrap_or("png").to_owned();
        let quality = params["quality"].as_u64().unwrap_or(80).clamp(1, 100) as u8;
        let mut out: Option<Result<String, String>> = None;
        frames.read(false, &mut |f| {
            let x0 = ((cx - px) * dpr).round();
            let y0 = ((cy - py) * dpr).round();
            let w = (cw * dpr).round();
            let h = (ch * dpr).round();
            if x0 < 0.
                || y0 < 0.
                || w < 1.
                || h < 1.
                || x0 + w > f64::from(f.width) + 1.
                || y0 + h > f64::from(f.height) + 1.
            {
                return;
            }
            let (x0, y0) = (x0 as u32, y0 as u32);
            let w = (w as u32).min(f.width - x0);
            let h = (h as u32).min(f.height - y0);
            out = Some(encode_bgra(f, x0, y0, w, h, scale, &format, quality));
        });
        match out {
            Some(Ok(b64)) => Ok(Some(b64)),
            Some(Err(e)) => Err(EngineError::Launch(e)),
            None => Ok(None),
        }
    }
}

/// Crop, scale and encode a BGRA frame as PNG or JPEG, base64.
#[allow(clippy::too_many_arguments)]
fn encode_bgra(
    f: &Frame<'_>,
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    scale: f64,
    format: &str,
    quality: u8,
) -> Result<String, String> {
    let mut raw = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let row = ((y0 + y) * f.stride + x0 * 4) as usize;
        raw.extend_from_slice(&f.pixels[row..row + w as usize * 4]);
    }
    // BGRA to RGBA in place.
    for px in raw.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let rgba = image::RgbaImage::from_raw(w, h, raw).ok_or("a frame smaller than its size")?;
    let img = if scale < 0.999 {
        let sw = ((f64::from(w) * scale).round() as u32).max(1);
        let sh = ((f64::from(h) * scale).round() as u32).max(1);
        image::imageops::resize(&rgba, sw, sh, image::imageops::FilterType::Triangle)
    } else {
        rgba
    };
    let mut bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut bytes);
    match format {
        "jpeg" => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality)
                .encode_image(&rgb)
                .map_err(|e| e.to_string())?;
        }
        _ => {
            // Fast compression, as Page.captureScreenshot's optimizeForSpeed does.
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new_with_quality(
                &mut cursor,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Sub,
            )
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?
        }
    }
    Ok(base64(&bytes))
}

/// Standard base64 with padding (as `Page.captureScreenshot` answers).
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                A[(n >> (18 - 6 * i)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A handle on a running engine's control channel: input and resizes from any thread, without the engine object
/// (the shell's view keeps one).
#[derive(Clone)]
pub struct TabControl(Arc<Control>);

impl std::fmt::Debug for TabControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabControl")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl TabControl {
    /// Forward one input event; never waits.
    pub fn input(&self, tab: &str, event: Value) {
        let _ = self
            .0
            .notify("tab/input", json!({"tab": tab, "event": event}));
    }

    /// Ask for a new view size (CSS pixels), as a notification: no answer is waited for.
    pub fn resize(&self, tab: &str, width: u32, height: u32, scale: f32) {
        let _ = self.0.notify(
            "tab/resize",
            json!({"tab": tab, "width": width, "height": height, "scale": scale}),
        );
    }

    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// A tab's frames and state.
    pub fn frames(&self, tab: &str) -> Option<Arc<TabFrames>> {
        self.0.tab(tab)
    }

    /// Any shell-to-engine notification (`tab/action`, `tab/dialogAnswer`, `tab/permissionAnswer`); never waits.
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.0.notify(method, params);
    }

    /// The dialog or prompt `tab`'s page waits on.
    pub fn pending_dialog(&self, tab: &str) -> Option<PendingDialog> {
        self.0.pending(tab)
    }

    /// Close a tab (the Web Browser window's DevTools tab, which no command closes); never waits: the request runs on
    /// a thread of its own.
    pub fn close(&self, tab: &str) {
        let (c, tab) = (self.0.clone(), tab.to_owned());
        let _ = std::thread::Builder::new()
            .name("chromium-close-tab".into())
            .spawn(move || {
                let _ = c.request("tab/close", json!({"tab": tab}), DEFAULT_TIMEOUT);
            });
    }

    /// The open tabs, in the engine's order.
    pub fn tabs(&self) -> Vec<String> {
        lock(&self.0.tabs)
            .iter()
            .filter(|(_, f)| !f.is_closed())
            .map(|(t, _)| t.clone())
            .collect()
    }
}

impl Drop for EmbeddedChromium {
    fn drop(&mut self) {
        self.stop(false);
    }
}

impl Engine for EmbeddedChromium {
    fn name(&self) -> &'static str {
        "embedded-chromium"
    }

    fn configure(&mut self, config: EngineConfig) {
        self.config = config;
    }

    fn is_running(&self) -> bool {
        self.control().is_ok()
    }

    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        if self.is_running() {
            return Ok(None);
        }
        // A crashed engine leaves its state behind: drop it before starting again.
        self.stop(false);
        self.start().map(Some)
    }

    fn info(&self) -> Option<LaunchInfo> {
        self.running
            .as_ref()
            .filter(|r| !r.control.is_closed())
            .map(|r| r.info.clone())
    }

    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        let c = self.control()?;
        // DevTools tabs are the window's, not page targets.
        Ok(lock(&c.tabs)
            .iter()
            .filter(|(_, f)| !f.is_closed())
            .filter_map(|(id, f)| {
                let i = f.info();
                i.devtools_of.is_none().then(|| TargetInfo {
                    target_id: id.clone(),
                    url: i.url,
                    title: i.title,
                })
            })
            .collect())
    }

    fn open_tab(&mut self, url: &str) -> Result<String, EngineError> {
        let c = self.control()?.clone();
        let t0 = Instant::now();
        let (w, h) = self.config.viewport;
        let r = c
            .request(
                "tab/create",
                json!({"url": url, "width": w, "height": h, "scale": 1.0, "frameRate": 60}),
                DEFAULT_TIMEOUT,
            )
            .map_err(EngineError::Launch)?;
        let tab = r["tab"]
            .as_str()
            .ok_or_else(|| EngineError::Launch("tab/create answered no tab".into()))?
            .to_owned();
        let frames = c.tab(&tab).unwrap_or_else(|| {
            let f = Arc::new(TabFrames::default());
            lock(&c.tabs).push((tab.clone(), f.clone()));
            f
        });
        *lock(&frames.created) = Some(t0);
        lock(&frames.info).url = url.to_owned();
        Ok(tab)
    }

    fn close_tab(&mut self, target_id: &str) -> Result<(), EngineError> {
        let c = self.control()?;
        c.request("tab/close", json!({"tab": target_id}), DEFAULT_TIMEOUT)
            .map_err(EngineError::Launch)?;
        lock(&c.subscribers).remove(target_id);
        Ok(())
    }

    fn activate_tab(&mut self, target_id: &str) -> Result<(), EngineError> {
        let c = self.control()?;
        c.notify(
            "tab/input",
            json!({"tab": target_id, "event": {"type": "focus", "focused": true}}),
        )
        .map_err(EngineError::Launch)
    }

    fn attach(&mut self, target_id: &str) -> Result<String, EngineError> {
        // The tab's DevTools agent is the page target: the tab is its own session.
        let c = self.control()?;
        if c.tab(target_id).is_none() {
            return Err(EngineError::Launch(format!("no tab {target_id}")));
        }
        Ok(target_id.to_owned())
    }

    fn send(
        &self,
        session: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, EngineError> {
        let rx = self.control()?.cdp_send(session, method, params)?;
        Ok(wait_cdp(&rx, method, Instant::now() + timeout, timeout)?)
    }

    fn send_many(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
    ) -> Vec<Result<Value, EngineError>> {
        let c = match self.control() {
            Ok(c) => c,
            Err(_) => return calls.iter().map(|_| Err(EngineError::NotRunning)).collect(),
        };
        let deadline = Instant::now() + timeout;
        let sent: Vec<_> = calls
            .into_iter()
            .map(|(m, p)| {
                let r = c.cdp_send(session, &m, p);
                (m, r)
            })
            .collect();
        sent.into_iter()
            .map(|(m, r)| {
                r.and_then(|rx| wait_cdp(&rx, &m, deadline, timeout))
                    .map_err(EngineError::from)
            })
            .collect()
    }

    fn send_many_unless(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
        give_up: &dyn Fn() -> bool,
    ) -> Vec<Result<Value, EngineError>> {
        let c = match self.control() {
            Ok(c) => c,
            Err(_) => return calls.iter().map(|_| Err(EngineError::NotRunning)).collect(),
        };
        let sent: Vec<_> = calls
            .into_iter()
            .map(|(m, p)| {
                let r = c.cdp_send(session, &m, p);
                (m, r)
            })
            .collect();
        crate::engine::collect_unless(sent, timeout, give_up)
    }

    fn frames(&self, target_id: &str) -> Option<Arc<dyn FrameSource>> {
        let f: Arc<dyn FrameSource> = self.control().ok()?.tab(target_id)?;
        Some(f)
    }

    fn pending_dialog(&self, target_id: &str) -> Option<PendingDialog> {
        self.control().ok()?.pending(target_id)
    }

    fn answer_dialog(
        &self,
        target_id: &str,
        answer: &DialogAnswer,
    ) -> Result<PendingDialog, EngineError> {
        let c = self.control()?;
        let d = c.pending(target_id).ok_or_else(|| {
            EngineError::Launch(
                "no dialog is open in the tab (it was answered, perhaps by the person, or the page went on)"
                    .into(),
            )
        })?;
        let (method, params) = if d.kind == "permission" {
            (
                "tab/permissionAnswer",
                json!({"tab": target_id, "id": d.id, "allow": answer.accept}),
            )
        } else {
            let mut p = json!({"tab": target_id, "id": d.id, "accept": answer.accept});
            if let Some(t) = &answer.text {
                p["text"] = json!(t);
            }
            if !answer.files.is_empty() {
                p["files"] = json!(answer.files);
            }
            if let Some(u) = &answer.username {
                p["username"] = json!(u);
            }
            if let Some(pw) = &answer.password {
                p["password"] = json!(pw);
            }
            ("tab/dialogAnswer", p)
        };
        c.notify(method, params).map_err(EngineError::Launch)?;
        // Answered: the next look sees no dialog even before the engine's tab/dialogClosed arrives.
        lock(&c.prompts).remove(&d.id);
        Ok(d)
    }

    fn devtools(
        &mut self,
        target_id: &str,
        inspect: Option<(f64, f64)>,
    ) -> Result<bool, EngineError> {
        let c = self.control()?.clone();
        let frames = c
            .tab(target_id)
            .ok_or_else(|| EngineError::Launch(format!("no tab {target_id}")))?;
        if frames.info().devtools_of.is_some() {
            return Err(EngineError::Launch(
                "that tab is DevTools; open DevTools for its page".into(),
            ));
        }
        // DevTools takes the page's view size (the window shows one tab at a time).
        let (w, h) = self.config.viewport;
        let mut params = json!({"tab": target_id, "width": w.max(400), "height": h.max(300)});
        if let Some((x, y)) = inspect {
            params["inspectAt"] = json!({"x": x.round() as i64, "y": y.round() as i64});
        }
        let r = c
            .request("tab/devtools", params, DEFAULT_TIMEOUT)
            .map_err(EngineError::Launch)?;
        let devtools = r["devtools"]
            .as_str()
            .ok_or_else(|| EngineError::Launch("tab/devtools answered no tab".into()))?
            .to_owned();
        if let Some(f) = c.tab(&devtools) {
            lock(&f.info).devtools_of = Some(target_id.to_owned());
        }
        let created = r["created"].as_bool().unwrap_or(true);
        if let Some(o) = &self.observer {
            o(EngineEvent::DevtoolsOpened {
                page: target_id.to_owned(),
                devtools,
            });
        }
        Ok(created)
    }

    fn history(&self, target_id: &str) -> Option<TabHistory> {
        let i = self.control().ok()?.tab(target_id)?.info();
        Some(TabHistory {
            can_go_back: i.can_go_back,
            can_go_forward: i.can_go_forward,
            favicon: i.favicon,
        })
    }

    fn subscribe(&self, session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let c = self.control()?;
        let (tx, rx) = mpsc::channel();
        lock(&c.subscribers)
            .entry(session.to_owned())
            .or_default()
            .push(tx);
        Ok(rx)
    }

    fn screenshot(
        &self,
        session: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<String, EngineError> {
        if !params["captureBeyondViewport"].as_bool().unwrap_or(false)
            && let Some(b64) = self.ring_screenshot(session, &params, timeout)?
        {
            self.stats.ring_screenshots.fetch_add(1, Ordering::Relaxed);
            return Ok(b64);
        }
        self.stats.cdp_screenshots.fetch_add(1, Ordering::Relaxed);
        let r = self.send(session, "Page.captureScreenshot", params, timeout)?;
        r["data"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| EngineError::Launch("Page.captureScreenshot answered no data".into()))
    }

    fn shutdown(&mut self) {
        self.stop(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn discovery_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let cef_env = dir.path().join("cef-env");
        let cache = dir.path().join("cache");
        for d in [&a, &b, &cef_env, &cache] {
            std::fs::create_dir_all(d).unwrap();
        }
        let mut s = ChromiumSearch {
            engine: None,
            engine_dirs: vec![a.clone(), b.clone()],
            cef: vec![cef_env.clone()],
            cef_cache: Some(cache.clone()),
        };
        let e = s.find_engine().unwrap_err();
        assert!(
            e.contains("tools/cef/fetch.sh") && e.contains("--features"),
            "{e}"
        );
        std::fs::write(b.join(exe_name()), b"").unwrap();
        assert_eq!(s.find_engine().unwrap(), b.join(exe_name()));
        std::fs::write(a.join(exe_name()), b"").unwrap();
        assert_eq!(
            s.find_engine().unwrap(),
            a.join(exe_name()),
            "beside the shell first"
        );
        s.engine = Some(dir.path().join("nope"));
        assert!(s.find_engine().unwrap_err().contains(ENGINE_ENV));

        let engine = a.join(exe_name());
        let e = s.find_cef(&engine).unwrap_err();
        assert!(e.contains(CEF_VERSION) && e.contains("fetch.sh"), "{e}");
        std::fs::write(a.join(cef_library()), b"").unwrap();
        assert_eq!(s.find_cef(&engine).unwrap(), a, "beside the engine last");
        std::fs::write(cache.join(cef_library()), b"").unwrap();
        assert_eq!(s.find_cef(&engine).unwrap(), cache);
        std::fs::write(cef_env.join(cef_library()), b"").unwrap();
        assert_eq!(
            s.find_cef(&engine).unwrap(),
            cef_env,
            "ELUDITE_CEF and CEF_PATH first"
        );
    }

    #[test]
    fn the_engine_follows_the_setting_and_what_is_found() {
        assert_eq!(
            EngineChoice::from_setting("external"),
            EngineChoice::External
        );
        assert_eq!(
            EngineChoice::from_setting("embedded"),
            EngineChoice::Embedded
        );
        assert_eq!(EngineChoice::from_setting("??"), EngineChoice::Embedded);
        let dir = tempfile::tempdir().unwrap();
        let mut s = ChromiumSearch {
            engine: None,
            engine_dirs: vec![dir.path().to_path_buf()],
            cef: vec![],
            cef_cache: None,
        };
        assert_eq!(
            select_engine(EngineChoice::External, &s),
            (EngineChoice::External, None)
        );
        let (kind, why) = select_engine(EngineChoice::Embedded, &s);
        assert_eq!(
            kind,
            EngineChoice::External,
            "nothing found: the external Chrome"
        );
        assert!(why.unwrap().contains("tools/cef/fetch.sh"));
        if cfg!(target_os = "linux") {
            std::fs::write(dir.path().join(exe_name()), b"").unwrap();
            std::fs::write(dir.path().join(cef_library()), b"").unwrap();
            s.cef = vec![dir.path().to_path_buf()];
            assert_eq!(
                select_engine(EngineChoice::Embedded, &s),
                (EngineChoice::Embedded, None)
            );
        }
    }

    #[test]
    fn a_downloads_end_makes_an_output_line_and_its_progress_none() {
        let line = |v: Value| download_line(&v);
        assert_eq!(
            line(
                json!({"url": "http://h/a.zip", "state": "complete", "path": "/w/.eludite/browser/downloads/a.zip",
                "receivedBytes": 12})
            ),
            Some(
                "Downloaded http://h/a.zip to /w/.eludite/browser/downloads/a.zip (12 bytes)"
                    .into()
            )
        );
        assert!(
            line(json!({"url": "u", "state": "refused", "message": "over 100 MB"}))
                .unwrap()
                .contains("over 100 MB")
        );
        assert!(line(json!({"url": "u", "state": "progress"})).is_none());
        assert!(line(json!({"url": "u", "state": "started"})).is_none());
    }

    #[test]
    fn the_pinned_version_is_the_fetch_scripts() {
        let pin = include_str!("../../../tools/cef/PIN");
        let version = pin
            .lines()
            .find_map(|l| l.strip_prefix("version "))
            .unwrap();
        assert_eq!(version.trim(), CEF_VERSION);
    }

    #[test]
    fn every_method_used_here_has_a_schema() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/schemas/browser-rpc");
        let mut methods = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "json") {
                let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
                methods.push(v["x-eludite-method"].as_str().unwrap().to_owned());
            }
        }
        for m in [
            "initialize",
            "shutdown",
            "tab/create",
            "tab/close",
            "tab/resize",
            "tab/resized",
            "tab/navigate",
            "tab/input",
            "tab/cdp",
            "tab/cdpEvent",
            "tab/frame",
            "tab/state",
            "tab/closed",
            "tab/devtools",
            "tab/dialogAnswer",
            "tab/permissionAnswer",
            "tab/action",
        ]
        .into_iter()
        .chain(WINDOW_NOTIFICATIONS)
        {
            assert!(methods.iter().any(|x| x == m), "{m} has no schema");
        }
    }

    #[test]
    fn answers_and_events_are_routed_by_id_and_tab() {
        let c = Control {
            stdin: Mutex::new(None),
            observer: None,
            prompts: Mutex::default(),
            next_id: AtomicU64::new(0),
            pending: Mutex::default(),
            cdp_next: AtomicU64::new(0),
            cdp_pending: Mutex::default(),
            subscribers: Mutex::default(),
            tabs: Mutex::new(vec![("1".into(), Arc::new(TabFrames::default()))]),
            closed: AtomicBool::new(false),
        };
        let (tx, rx) = mpsc::channel();
        lock(&c.cdp_pending).insert(4, ("Runtime.evaluate".into(), tx));
        let (etx, erx) = mpsc::channel();
        lock(&c.subscribers).insert("1".into(), vec![etx]);
        c.dispatch(
            json!({"method": "tab/cdpEvent", "params": {"tab": "1", "message": {"id": 4, "error": {"code": -32000, "message": "nope"}}}}),
            None,
        );
        assert!(matches!(
            rx.recv().unwrap(),
            Err(CdpError::Protocol { code: -32000, .. })
        ));
        c.dispatch(
            json!({"method": "tab/cdpEvent", "params": {"tab": "1", "message": {"method": "Page.loadEventFired", "params": {"timestamp": 1}}}}),
            None,
        );
        let e = erx.recv().unwrap();
        assert_eq!(e.method, "Page.loadEventFired");
        assert_eq!(e.session_id.as_deref(), Some("1"));
        let frames = c.tab("1").unwrap();
        let woke = Arc::new(AtomicU64::new(0));
        let w = woke.clone();
        frames.set_listener(Some(Box::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        })));
        c.dispatch(
            json!({"method": "tab/frame", "params": {"tab": "1", "sequence": 3}}),
            None,
        );
        assert_eq!(frames.sequence(), 3);
        assert_eq!(woke.load(Ordering::Relaxed), 1);
        c.dispatch(
            json!({"method": "tab/state", "params": {"tab": "1", "title": "T"}}),
            None,
        );
        assert_eq!(frames.info().title, "T");
        c.dispatch(
            json!({"method": "tab/closed", "params": {"tab": "1", "reason": "crashed"}}),
            None,
        );
        assert!(frames.is_closed());
        assert!(erx.recv().is_err(), "the subscription ended with the tab");
        let (tx, rx) = mpsc::channel();
        lock(&c.cdp_pending).insert(5, ("X".into(), tx));
        c.close();
        assert_eq!(rx.recv().unwrap(), Err(CdpError::Closed));
    }
}
