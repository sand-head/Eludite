//! The browser's commands (brief 0023, proposal 0002 section 4): `eludite.browser.tabs`, `tab_open`, `tab_close`,
//! `tab_select`, `navigate`, `resize`, `screenshot`, `read_page`, `find`, `page_text`, `console`, `network`, `wait`
//! and `evaluate`, all agent-visible, with `tab` optional (the active tab) except for `tab_select`.
//!
//! This module parses and validates their input into a [`BrowserRequest`], types their outputs
//! ([`BrowserOutput`]), and registers them against a [`BrowserTarget`]: the shell, which runs them on its `browser`
//! worker thread through `eludite-browser`. The schemas are the files in `protocol/schemas/browser-*.json`
//! (checked in first, CLAUDE.md invariant 4).
//!
//! Brief 0024 adds acting on the page: `input`, `form_input`, `upload`, `storage`, `network_body` and
//! `open_external`.
//!
//! Classes (proposal 0002 section 4): reading the page is `read`; opening, closing and selecting tabs, navigating,
//! resizing, evaluating JavaScript and acting on the page are `execute`. The escalation hooks ([`escalation`],
//! ADR-0009) apply the solution policy's `browser` object per call: a url off the allowed origins makes `navigate`,
//! `tab_open` and `open_external` dangerous; a file outside the workspace makes `upload` and `form_input` dangerous;
//! `browser.evaluate` makes `evaluate` dangerous (`prompt`) or refuses it (`deny`); `browser.network_bodies: deny`
//! refuses `network_body`; `storage` with `clear` is execute.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{AlwaysAllow, EvaluatePolicy, NetworkBodiesPolicy, ParsedUrl, PolicyView};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const TABS: &str = "eludite.browser.tabs";
pub const TAB_OPEN: &str = "eludite.browser.tab_open";
pub const TAB_CLOSE: &str = "eludite.browser.tab_close";
pub const TAB_SELECT: &str = "eludite.browser.tab_select";
pub const NAVIGATE: &str = "eludite.browser.navigate";
pub const RESIZE: &str = "eludite.browser.resize";
pub const SCREENSHOT: &str = "eludite.browser.screenshot";
pub const READ_PAGE: &str = "eludite.browser.read_page";
pub const FIND: &str = "eludite.browser.find";
pub const PAGE_TEXT: &str = "eludite.browser.page_text";
pub const CONSOLE: &str = "eludite.browser.console";
pub const NETWORK: &str = "eludite.browser.network";
pub const WAIT: &str = "eludite.browser.wait";
pub const EVALUATE: &str = "eludite.browser.evaluate";
pub const INPUT: &str = "eludite.browser.input";
pub const FORM_INPUT: &str = "eludite.browser.form_input";
pub const UPLOAD: &str = "eludite.browser.upload";
pub const STORAGE: &str = "eludite.browser.storage";
pub const NETWORK_BODY: &str = "eludite.browser.network_body";
pub const OPEN_EXTERNAL: &str = "eludite.browser.open_external";

pub const ALL: [&str; 20] = [
    TABS,
    TAB_OPEN,
    TAB_CLOSE,
    TAB_SELECT,
    NAVIGATE,
    RESIZE,
    SCREENSHOT,
    READ_PAGE,
    FIND,
    PAGE_TEXT,
    CONSOLE,
    NETWORK,
    WAIT,
    EVALUATE,
    INPUT,
    FORM_INPUT,
    UPLOAD,
    STORAGE,
    NETWORK_BODY,
    OPEN_EXTERNAL,
];

/// Defaults and limits from the schemas.
pub const NAVIGATE_WAIT_MS: u64 = 30_000;
pub const NAVIGATE_MAX_WAIT_MS: u64 = 120_000;
pub const WAIT_MS: u64 = 10_000;
pub const WAIT_MAX_MS: u64 = 60_000;
pub const MAX_WIDTH: u32 = 1280;
pub const MAX_NODES: usize = 500;
pub const FIND_MAX: usize = 20;
pub const PAGE_TEXT_CHARS: usize = 20_000;
pub const LOG_MAX: usize = 100;
pub const EVALUATE_CHARS: usize = 10_000;
pub const JPEG_QUALITY: u8 = 80;
pub const INPUT_WAIT_MS: u64 = 500;
pub const INPUT_MAX_WAIT_MS: u64 = 10_000;
/// `scroll`'s default delta: down 400 CSS pixels.
pub const SCROLL_DELTA: (f64, f64) = (0., 400.);
pub const FORM_FIELDS: usize = 100;
pub const UPLOAD_FILES: usize = 50;
/// Storage values longer than this are cut.
pub const STORAGE_VALUE_CHARS: usize = 1000;
pub const NETWORK_BODY_BYTES: usize = 65_536;
pub const NETWORK_BODY_MAX_BYTES: usize = 10 * 1024 * 1024;

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! s {
        ($f:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/browser-",
                    $f,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/browser-",
                    $f,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output), permission) = match id {
        TABS => ("Browser: Tabs", s!("tabs"), Read),
        TAB_OPEN => ("Browser: New Tab", s!("tab-open"), Execute),
        TAB_CLOSE => ("Browser: Close Tab", s!("tab-close"), Execute),
        TAB_SELECT => ("Browser: Select Tab", s!("tab-select"), Execute),
        NAVIGATE => ("Browser: Navigate", s!("navigate"), Execute),
        RESIZE => ("Browser: Resize Viewport", s!("resize"), Execute),
        SCREENSHOT => ("Browser: Screenshot", s!("screenshot"), Read),
        READ_PAGE => ("Browser: Read Page", s!("read-page"), Read),
        FIND => ("Browser: Find Elements", s!("find"), Read),
        PAGE_TEXT => ("Browser: Page Text", s!("page-text"), Read),
        CONSOLE => ("Browser: Console", s!("console"), Read),
        NETWORK => ("Browser: Network", s!("network"), Read),
        WAIT => ("Browser: Wait", s!("wait"), Read),
        EVALUATE => ("Browser: Evaluate JavaScript", s!("evaluate"), Execute),
        INPUT => ("Browser: Input", s!("input"), Execute),
        FORM_INPUT => ("Browser: Fill Form", s!("form-input"), Execute),
        UPLOAD => ("Browser: Upload Files", s!("upload"), Execute),
        STORAGE => ("Browser: Storage", s!("storage"), Read),
        NETWORK_BODY => ("Browser: Response Body", s!("network-body"), Read),
        OPEN_EXTERNAL => (
            "Browser: Open in External Browser",
            s!("open-external"),
            Execute,
        ),
        other => unreachable!("not a browser command: {other}"),
    };
    (title, input, output, permission)
}

/// Where `navigate` goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigateTo {
    Url(String),
    Back,
    Forward,
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    #[default]
    Load,
    Domcontentloaded,
    NetworkIdle,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

impl ImageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
        }
    }
}

/// A rectangle in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What `screenshot` captures when it is not the viewport or the page.
#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    Ref(String),
    Rect(Rect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    #[default]
    Accessibility,
    Dom,
}

impl ReadMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ReadMode::Accessibility => "accessibility",
            ReadMode::Dom => "dom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFilter {
    #[default]
    Interactive,
    All,
}

/// How `find` looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindBy {
    Css(String),
    Text(String),
    Role { role: String, name: Option<String> },
}

/// Console levels, least severe first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Debug,
    Log,
    Info,
    Warning,
    Error,
}

impl ConsoleLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            ConsoleLevel::Debug => "debug",
            ConsoleLevel::Log => "log",
            ConsoleLevel::Info => "info",
            ConsoleLevel::Warning => "warning",
            ConsoleLevel::Error => "error",
        }
    }
}

/// A `pattern` or `url_pattern`: a substring, or `/regex/` with an optional `i` flag. The regular expression is
/// compiled where it is used (`eludite-browser`), which refuses one that does not compile as invalid input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPattern {
    Substring(String),
    Regex {
        source: String,
        case_insensitive: bool,
    },
}

impl TextPattern {
    pub fn parse(s: &str) -> TextPattern {
        if s.len() >= 2 && s.starts_with('/') {
            if let Some(body) = s.strip_suffix("/i").filter(|b| b.len() > 1) {
                return TextPattern::Regex {
                    source: body[1..].to_owned(),
                    case_insensitive: true,
                };
            }
            if let Some(body) = s.strip_suffix('/') {
                return TextPattern::Regex {
                    source: body[1..].to_owned(),
                    case_insensitive: false,
                };
            }
        }
        TextPattern::Substring(s.to_owned())
    }
}

/// What `wait` waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitFor {
    Selector(String),
    Text(String),
    Navigation,
    NetworkIdle,
    Console(TextPattern),
    Function(String),
}

impl WaitFor {
    pub fn as_str(&self) -> &'static str {
        match self {
            WaitFor::Selector(_) => "selector",
            WaitFor::Text(_) => "text",
            WaitFor::Navigation => "navigation",
            WaitFor::NetworkIdle => "network_idle",
            WaitFor::Console(_) => "console",
            WaitFor::Function(_) => "function",
        }
    }
}

/// `input`'s actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputAction {
    Click,
    DoubleClick,
    RightClick,
    Hover,
    Type,
    Key,
    Scroll,
    Drag,
    Select,
    Focus,
}

impl InputAction {
    pub fn as_str(self) -> &'static str {
        match self {
            InputAction::Click => "click",
            InputAction::DoubleClick => "double_click",
            InputAction::RightClick => "right_click",
            InputAction::Hover => "hover",
            InputAction::Type => "type",
            InputAction::Key => "key",
            InputAction::Scroll => "scroll",
            InputAction::Drag => "drag",
            InputAction::Select => "select",
            InputAction::Focus => "focus",
        }
    }

    /// The action needs a `ref` or a point.
    pub fn needs_target(self) -> bool {
        !matches!(
            self,
            InputAction::Type | InputAction::Key | InputAction::Scroll
        )
    }
}

/// Where an action lands: an element or a point in CSS pixels of the viewport.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Ref(String),
    Point { x: f64, y: f64 },
}

/// Keys held during an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    /// CDP's bit field: Alt 1, Ctrl 2, Meta 4, Shift 8.
    pub fn bits(self) -> i64 {
        i64::from(self.alt)
            | (i64::from(self.control) << 1)
            | (i64::from(self.meta) << 2)
            | (i64::from(self.shift) << 3)
    }

    pub fn union(self, o: Modifiers) -> Modifiers {
        Modifiers {
            shift: self.shift || o.shift,
            control: self.control || o.control,
            alt: self.alt || o.alt,
            meta: self.meta || o.meta,
        }
    }

    fn set(&mut self, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "shift" => self.shift = true,
            "control" | "ctrl" => self.control = true,
            "alt" | "option" => self.alt = true,
            "meta" | "cmd" | "command" | "super" => self.meta = true,
            _ => return false,
        }
        true
    }
}

/// The named keys `key` knows besides single characters.
pub const KEY_NAMES: [&str; 28] = [
    "Enter",
    "Tab",
    "Escape",
    "Backspace",
    "Delete",
    "Insert",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Space",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "ContextMenu",
];

/// One key of `keys`: a name from [`KEY_NAMES`] (canonical case) or one character, and its modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub key: String,
    pub modifiers: Modifiers,
}

impl KeyPress {
    /// `Enter`, `a`, `Control+a`, `Shift+Tab`, `Control++`.
    pub fn parse(s: &str) -> Result<KeyPress, String> {
        let (mods, key) = if s == "+" {
            ("", "+")
        } else if let Some(m) = s.strip_suffix("++") {
            (m, "+")
        } else {
            match s.rsplit_once('+') {
                Some((m, k)) => (m, k),
                None => ("", s),
            }
        };
        let mut modifiers = Modifiers::default();
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            if !modifiers.set(m.trim()) {
                return Err(format!(
                    "`{m}` in `{s}` is not a modifier (Shift, Control, Alt, Meta)"
                ));
            }
        }
        let key = if key.chars().count() == 1 {
            key.to_owned()
        } else if let Some(name) = KEY_NAMES.iter().find(|n| n.eq_ignore_ascii_case(key)) {
            (*name).to_owned()
        } else if key.eq_ignore_ascii_case("esc") {
            "Escape".into()
        } else if key.eq_ignore_ascii_case("return") {
            "Enter".into()
        } else {
            return Err(format!(
                "`{key}` is not a key: give one character or a name such as Enter, Tab, Escape, ArrowDown, F5"
            ));
        };
        Ok(KeyPress { key, modifiers })
    }
}

/// A `form_input` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// Text, or a single select's option.
    Text(String),
    /// A checkbox's or a radio's state.
    Checked(bool),
    /// A multiple select's options.
    Options(Vec<String>),
    /// A file input's files (absolute paths).
    Files(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub ref_: String,
    pub value: FieldValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Cookies,
    Local,
    Session,
    #[default]
    All,
}

impl StorageKind {
    pub fn cookies(self) -> bool {
        matches!(self, StorageKind::Cookies | StorageKind::All)
    }
    pub fn local(self) -> bool {
        matches!(self, StorageKind::Local | StorageKind::All)
    }
    pub fn session(self) -> bool {
        matches!(self, StorageKind::Session | StorageKind::All)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageAction {
    #[default]
    Get,
    Clear,
}

impl StorageAction {
    pub fn as_str(self) -> &'static str {
        match self {
            StorageAction::Get => "get",
            StorageAction::Clear => "clear",
        }
    }
}

/// A parsed, validated browser command. `tab` is `None` for the active tab.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserRequest {
    Tabs,
    TabOpen {
        url: Option<String>,
    },
    TabClose {
        tab: Option<String>,
    },
    TabSelect {
        tab: String,
    },
    Navigate {
        tab: Option<String>,
        to: NavigateTo,
        wait_until: WaitUntil,
        wait_ms: u64,
    },
    Resize {
        tab: Option<String>,
        width: u32,
        height: u32,
        device_scale_factor: Option<f64>,
        mobile: bool,
        user_agent: Option<String>,
    },
    Screenshot {
        tab: Option<String>,
        full_page: bool,
        clip: Option<Clip>,
        max_width: u32,
        format: ImageFormat,
        quality: Option<u8>,
    },
    ReadPage {
        tab: Option<String>,
        mode: ReadMode,
        filter: ReadFilter,
        root: Option<String>,
        max_nodes: usize,
    },
    Find {
        tab: Option<String>,
        by: FindBy,
        max: usize,
    },
    PageText {
        tab: Option<String>,
        root: Option<String>,
        max_chars: usize,
        cursor: usize,
    },
    Console {
        tab: Option<String>,
        since: u64,
        level: Option<ConsoleLevel>,
        pattern: Option<TextPattern>,
        max: usize,
    },
    Network {
        tab: Option<String>,
        since: u64,
        url_pattern: Option<TextPattern>,
        status: Option<u16>,
        resource_type: Option<String>,
        max: usize,
    },
    Wait {
        tab: Option<String>,
        condition: WaitFor,
        wait_ms: u64,
    },
    Evaluate {
        tab: Option<String>,
        expression: String,
        await_promise: bool,
        return_by_value: bool,
        max_chars: usize,
    },
    Input {
        tab: Option<String>,
        action: InputAction,
        target: Option<Target>,
        /// For `type`.
        text: Option<String>,
        per_key: bool,
        /// For `key`.
        keys: Vec<KeyPress>,
        /// For `scroll`, in CSS pixels.
        delta: (f64, f64),
        /// For `drag`.
        to: Option<Target>,
        /// For `select`.
        values: Vec<String>,
        modifiers: Modifiers,
        wait_ms: u64,
    },
    FormInput {
        tab: Option<String>,
        fields: Vec<FormField>,
    },
    Upload {
        tab: Option<String>,
        ref_: String,
        paths: Vec<String>,
    },
    Storage {
        tab: Option<String>,
        kind: StorageKind,
        action: StorageAction,
        /// Normalized: `scheme://host[:port]`.
        origin: Option<String>,
    },
    NetworkBody {
        tab: Option<String>,
        request_id: String,
        max_bytes: usize,
    },
    OpenExternal {
        url: Option<String>,
    },
}

impl BrowserRequest {
    pub fn command(&self) -> &'static str {
        match self {
            BrowserRequest::Tabs => TABS,
            BrowserRequest::TabOpen { .. } => TAB_OPEN,
            BrowserRequest::TabClose { .. } => TAB_CLOSE,
            BrowserRequest::TabSelect { .. } => TAB_SELECT,
            BrowserRequest::Navigate { .. } => NAVIGATE,
            BrowserRequest::Resize { .. } => RESIZE,
            BrowserRequest::Screenshot { .. } => SCREENSHOT,
            BrowserRequest::ReadPage { .. } => READ_PAGE,
            BrowserRequest::Find { .. } => FIND,
            BrowserRequest::PageText { .. } => PAGE_TEXT,
            BrowserRequest::Console { .. } => CONSOLE,
            BrowserRequest::Network { .. } => NETWORK,
            BrowserRequest::Wait { .. } => WAIT,
            BrowserRequest::Evaluate { .. } => EVALUATE,
            BrowserRequest::Input { .. } => INPUT,
            BrowserRequest::FormInput { .. } => FORM_INPUT,
            BrowserRequest::Upload { .. } => UPLOAD,
            BrowserRequest::Storage { .. } => STORAGE,
            BrowserRequest::NetworkBody { .. } => NETWORK_BODY,
            BrowserRequest::OpenExternal { .. } => OPEN_EXTERNAL,
        }
    }

    /// The tab named, if any.
    pub fn tab(&self) -> Option<&str> {
        match self {
            BrowserRequest::Tabs
            | BrowserRequest::TabOpen { .. }
            | BrowserRequest::OpenExternal { .. } => None,
            BrowserRequest::TabSelect { tab } => Some(tab),
            BrowserRequest::TabClose { tab }
            | BrowserRequest::Navigate { tab, .. }
            | BrowserRequest::Resize { tab, .. }
            | BrowserRequest::Screenshot { tab, .. }
            | BrowserRequest::ReadPage { tab, .. }
            | BrowserRequest::Find { tab, .. }
            | BrowserRequest::PageText { tab, .. }
            | BrowserRequest::Console { tab, .. }
            | BrowserRequest::Network { tab, .. }
            | BrowserRequest::Wait { tab, .. }
            | BrowserRequest::Evaluate { tab, .. }
            | BrowserRequest::Input { tab, .. }
            | BrowserRequest::FormInput { tab, .. }
            | BrowserRequest::Upload { tab, .. }
            | BrowserRequest::Storage { tab, .. }
            | BrowserRequest::NetworkBody { tab, .. } => tab.as_deref(),
        }
    }

    /// The longest the command may legitimately take, for a caller that waits on another thread: its own wait plus
    /// a launch and a margin.
    pub fn budget_ms(&self) -> u64 {
        let own = match self {
            BrowserRequest::Navigate { wait_ms, .. }
            | BrowserRequest::Wait { wait_ms, .. }
            | BrowserRequest::Input { wait_ms, .. } => *wait_ms,
            BrowserRequest::TabOpen { .. } => NAVIGATE_WAIT_MS,
            _ => 0,
        };
        own + 60_000
    }
}

// ---- outputs ----

/// A tab (`browser-tabs.output.json`'s rows).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabRow {
    pub id: String,
    pub url: String,
    pub title: String,
    pub active: bool,
    pub loading: bool,
    pub page_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EngineRow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabsOutput {
    pub running: bool,
    pub engine: EngineRow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub tabs: Vec<TabRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabOpenOutput {
    #[serde(flatten)]
    pub tab: TabRow,
    pub launched: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TabCloseOutput {
    pub closed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub tabs: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NavigateOutput {
    pub tab: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub page_generation: u64,
    pub console_errors: u64,
    pub timed_out: bool,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ResizeOutput {
    pub tab: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
    pub user_agent: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ScreenshotOutput {
    /// Base64.
    pub image: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub page_generation: u64,
    pub tab: String,
}

/// `checked`: true, false or "mixed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    True,
    False,
    Mixed,
}

impl Serialize for Checked {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Checked::True => s.serialize_bool(true),
            Checked::False => s.serialize_bool(false),
            Checked::Mixed => s.serialize_str("mixed"),
        }
    }
}

impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Bool(true) => Ok(Checked::True),
            Value::Bool(false) => Ok(Checked::False),
            Value::String(s) if s == "mixed" => Ok(Checked::Mixed),
            other => Err(serde::de::Error::custom(format!("checked: {other}"))),
        }
    }
}

/// The states a node has (`read_page`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NodeState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<Checked>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid: Option<bool>,
}

impl NodeState {
    pub fn is_empty(&self) -> bool {
        *self == NodeState::default()
    }
}

/// A box in CSS pixels, rounded to tenths.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BoxRow {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PageNode {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<NodeState>,
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<BoxRow>,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReadPageOutput {
    pub tab: String,
    pub url: String,
    pub mode: String,
    pub nodes: Vec<PageNode>,
    pub total: usize,
    pub truncated: bool,
    pub page_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FindMatch {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub role: String,
    pub name: String,
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<BoxRow>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FindOutput {
    pub tab: String,
    pub matches: Vec<FindMatch>,
    pub total: usize,
    pub truncated: bool,
    pub page_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PageTextOutput {
    pub tab: String,
    pub text: String,
    pub cursor: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<usize>,
    pub total_chars: usize,
    pub page_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub seq: u64,
    pub level: ConsoleLevel,
    pub source: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    pub timestamp: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ConsoleOutput {
    pub tab: String,
    pub messages: Vec<ConsoleMessage>,
    pub next: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InitiatorRow {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NetworkRequestRow {
    pub seq: u64,
    pub request_id: String,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub resource_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub started_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoded_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    pub initiator: InitiatorRow,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NetworkOutput {
    pub tab: String,
    pub requests: Vec<NetworkRequestRow>,
    pub next: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WaitSatisfied {
    #[serde(rename = "for")]
    pub for_: String,
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WaitOutput {
    pub tab: String,
    pub timeout: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfied: Option<WaitSatisfied>,
    pub page_generation: u64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EvaluateOutput {
    pub tab: String,
    pub result: Value,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    pub truncated: bool,
    pub page_generation: u64,
}

/// A point in CSS pixels of the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Where an `input` action landed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InputTarget {
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
}

/// Where a `drag` was released.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DropTarget {
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
}

/// A console error emitted during an action.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConsoleError {
    pub text: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

impl From<&ConsoleMessage> for ConsoleError {
    fn from(m: &ConsoleMessage) -> Self {
        ConsoleError {
            text: m.text.clone(),
            source: m.source.clone(),
            url: m.url.clone().filter(|u| !u.is_empty()),
            line: m.line,
            column: m.column,
            stack: m.stack.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InputOutput {
    pub tab: String,
    pub action: String,
    pub target: InputTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<DropTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Vec<String>>,
    pub page_generation: u64,
    pub navigated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub console_errors: Vec<ConsoleError>,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FieldResult {
    #[serde(rename = "ref")]
    pub ref_: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FormInputOutput {
    pub tab: String,
    pub fields: Vec<FieldResult>,
    pub page_generation: u64,
    pub console_errors: Vec<ConsoleError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UploadOutput {
    pub tab: String,
    pub count: usize,
    pub page_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CookieRow {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: f64,
    pub http_only: bool,
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageItem {
    pub key: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl StorageItem {
    /// An item with its value cut at [`STORAGE_VALUE_CHARS`].
    pub fn new(key: String, value: &str) -> Self {
        let (value, truncated) = cut(value);
        StorageItem {
            key,
            value,
            truncated,
        }
    }
}

/// `s` cut at [`STORAGE_VALUE_CHARS`] characters, and whether it was.
pub fn cut(s: &str) -> (String, bool) {
    match s.char_indices().nth(STORAGE_VALUE_CHARS) {
        Some((i, _)) => (s[..i].to_owned(), true),
        None => (s.to_owned(), false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cleared {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StorageOutput {
    pub tab: String,
    pub origin: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<Vec<CookieRow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<Vec<StorageItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Vec<StorageItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<Cleared>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetworkBodyOutput {
    pub tab: String,
    pub request_id: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub headers: BTreeMap<String, String>,
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    pub size: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OpenExternalOutput {
    pub url: String,
    pub command: String,
}

/// What a browser command answers.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserOutput {
    Tabs(TabsOutput),
    TabOpen(TabOpenOutput),
    TabClose(TabCloseOutput),
    TabSelect(TabRow),
    Navigate(NavigateOutput),
    Resize(ResizeOutput),
    Screenshot(ScreenshotOutput),
    ReadPage(ReadPageOutput),
    Find(FindOutput),
    PageText(PageTextOutput),
    Console(ConsoleOutput),
    Network(NetworkOutput),
    Wait(WaitOutput),
    Evaluate(EvaluateOutput),
    Input(InputOutput),
    FormInput(FormInputOutput),
    Upload(UploadOutput),
    Storage(StorageOutput),
    NetworkBody(NetworkBodyOutput),
    OpenExternal(OpenExternalOutput),
}

impl BrowserOutput {
    pub fn to_json(&self) -> Value {
        match self {
            BrowserOutput::Tabs(o) => serde_json::to_value(o),
            BrowserOutput::TabOpen(o) => serde_json::to_value(o),
            BrowserOutput::TabClose(o) => serde_json::to_value(o),
            BrowserOutput::TabSelect(o) => serde_json::to_value(o),
            BrowserOutput::Navigate(o) => serde_json::to_value(o),
            BrowserOutput::Resize(o) => serde_json::to_value(o),
            BrowserOutput::Screenshot(o) => serde_json::to_value(o),
            BrowserOutput::ReadPage(o) => serde_json::to_value(o),
            BrowserOutput::Find(o) => serde_json::to_value(o),
            BrowserOutput::PageText(o) => serde_json::to_value(o),
            BrowserOutput::Console(o) => serde_json::to_value(o),
            BrowserOutput::Network(o) => serde_json::to_value(o),
            BrowserOutput::Wait(o) => serde_json::to_value(o),
            BrowserOutput::Evaluate(o) => serde_json::to_value(o),
            BrowserOutput::Input(o) => serde_json::to_value(o),
            BrowserOutput::FormInput(o) => serde_json::to_value(o),
            BrowserOutput::Upload(o) => serde_json::to_value(o),
            BrowserOutput::Storage(o) => serde_json::to_value(o),
            BrowserOutput::NetworkBody(o) => serde_json::to_value(o),
            BrowserOutput::OpenExternal(o) => serde_json::to_value(o),
        }
        .expect("browser outputs serialize")
    }
}

/// Whatever runs the browser (the shell, on its `browser` thread). Called on the invoking thread.
pub trait BrowserTarget: Send + Sync {
    fn apply(&self, request: BrowserRequest) -> Result<BrowserOutput, CommandError>;
}

// ---- input ----

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TabOpenIn {
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TabIn {
    tab: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Action {
    Back,
    Forward,
    Reload,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NavigateIn {
    url: Option<String>,
    action: Option<Action>,
    #[serde(default)]
    wait_until: WaitUntil,
    wait_ms: Option<u64>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResizeIn {
    width: u32,
    height: u32,
    device_scale_factor: Option<f64>,
    #[serde(default)]
    mobile: bool,
    user_agent: Option<String>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClipIn {
    Ref(String),
    Rect(Rect),
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ScreenshotIn {
    #[serde(default)]
    full_page: bool,
    clip: Option<ClipIn>,
    max_width: Option<u32>,
    #[serde(default)]
    format: ImageFormat,
    quality: Option<u8>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ReadPageIn {
    #[serde(default)]
    mode: ReadMode,
    #[serde(default)]
    filter: ReadFilter,
    root: Option<String>,
    max_nodes: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FindIn {
    css: Option<String>,
    text: Option<String>,
    role: Option<String>,
    name: Option<String>,
    max: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PageTextIn {
    root: Option<String>,
    max_chars: Option<usize>,
    #[serde(default)]
    cursor: usize,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConsoleIn {
    #[serde(default)]
    since: u64,
    level: Option<ConsoleLevel>,
    pattern: Option<String>,
    max: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NetworkIn {
    #[serde(default)]
    since: u64,
    url_pattern: Option<String>,
    status: Option<u16>,
    resource_type: Option<String>,
    max: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum WaitKind {
    Selector,
    Text,
    Navigation,
    NetworkIdle,
    Console,
    Function,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitIn {
    #[serde(rename = "for")]
    for_: WaitKind,
    css: Option<String>,
    text: Option<String>,
    pattern: Option<String>,
    expression: Option<String>,
    wait_ms: Option<u64>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateIn {
    expression: String,
    #[serde(rename = "await")]
    await_: Option<bool>,
    return_by_value: Option<bool>,
    max_chars: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PointIn {
    #[serde(rename = "ref")]
    ref_: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DeltaIn {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum KeysIn {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ModifierIn {
    Shift,
    Control,
    Alt,
    Meta,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputIn {
    action: InputAction,
    #[serde(rename = "ref")]
    ref_: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
    text: Option<String>,
    per_key: Option<bool>,
    keys: Option<KeysIn>,
    delta: Option<DeltaIn>,
    to: Option<PointIn>,
    values: Option<Vec<String>>,
    #[serde(default)]
    modifiers: Vec<ModifierIn>,
    wait_ms: Option<u64>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesIn {
    files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ValueIn {
    Text(String),
    Checked(bool),
    Options(Vec<String>),
    Files(FilesIn),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldIn {
    #[serde(rename = "ref")]
    ref_: String,
    value: ValueIn,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormInputIn {
    fields: Vec<FieldIn>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadIn {
    #[serde(rename = "ref")]
    ref_: String,
    paths: Vec<String>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StorageIn {
    #[serde(default)]
    kind: StorageKind,
    #[serde(default)]
    action: StorageAction,
    origin: Option<String>,
    tab: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkBodyIn {
    request_id: String,
    max_bytes: Option<usize>,
    tab: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OpenExternalIn {
    url: Option<String>,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(m.into())
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}

fn required<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CommandError> {
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}

fn is_id(s: &str, prefix: char) -> bool {
    s.len() > 1 && s.starts_with(prefix) && s[1..].bytes().all(|b| b.is_ascii_digit())
}

fn check_tab(tab: Option<String>) -> Result<Option<String>, CommandError> {
    match tab {
        Some(t) if !is_id(&t, 't') => Err(invalid(format!(
            "`tab` is a tab id from eludite.browser.tabs (`t1`, `t2`, ...), not `{t}`"
        ))),
        t => Ok(t),
    }
}

fn check_ref(field: &str, r: Option<String>) -> Result<Option<String>, CommandError> {
    match r {
        Some(r) if !is_id(&r, 'e') => Err(invalid(format!(
            "`{field}` is a ref from eludite.browser.read_page or find (`e1`, `e2`, ...), not `{r}`"
        ))),
        r => Ok(r),
    }
}

/// An absolute URL: a scheme (a letter, then letters, digits, `+`, `-` or `.`) and a colon.
pub fn is_absolute_url(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        None => false,
    }
}

fn check_url(url: Option<String>) -> Result<Option<String>, CommandError> {
    match url {
        Some(u) if !is_absolute_url(u.trim()) => Err(invalid(format!(
            "`url` must be absolute with a scheme (`http://localhost:5000/`, `file:///...`, `about:blank`), not `{u}`"
        ))),
        u => Ok(u.map(|u| u.trim().to_owned())),
    }
}

fn non_empty(field: &str, s: Option<String>) -> Result<Option<String>, CommandError> {
    match s {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{field}` must not be empty"))),
        s => Ok(s),
    }
}

fn range<T: PartialOrd + std::fmt::Display + Copy>(
    field: &str,
    v: Option<T>,
    min: T,
    max: T,
    default: T,
) -> Result<T, CommandError> {
    match v {
        Some(v) if v < min || v > max => Err(invalid(format!(
            "`{field}` is from {min} to {max}, not {v}"
        ))),
        Some(v) => Ok(v),
        None => Ok(default),
    }
}

fn pattern(field: &str, s: Option<String>) -> Result<Option<TextPattern>, CommandError> {
    Ok(non_empty(field, s)?.map(|s| TextPattern::parse(&s)))
}

/// A `ref` or a point (`x` and `y`, both finite and not negative).
fn parse_target(
    field: &str,
    ref_: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<Option<Target>, CommandError> {
    let ref_ = check_ref(field, ref_)?;
    match (ref_, x, y) {
        (None, None, None) => Ok(None),
        (Some(r), None, None) => Ok(Some(Target::Ref(r))),
        (None, Some(x), Some(y)) if x.is_finite() && y.is_finite() && x >= 0. && y >= 0. => {
            Ok(Some(Target::Point { x, y }))
        }
        (None, Some(_), Some(_)) => Err(invalid(
            "`x` and `y` are CSS pixels of the viewport, not negative",
        )),
        (Some(_), _, _) => Err(invalid(format!("give `{field}` or `x` and `y`, not both"))),
        _ => Err(invalid("give both `x` and `y`")),
    }
}

/// Absolute paths, 1 to [`UPLOAD_FILES`].
fn files(field: &str, paths: Vec<String>) -> Result<Vec<String>, CommandError> {
    if paths.is_empty() || paths.len() > UPLOAD_FILES {
        return Err(invalid(format!(
            "`{field}` lists 1 to {UPLOAD_FILES} files, not {}",
            paths.len()
        )));
    }
    for p in &paths {
        if !std::path::Path::new(p).is_absolute() {
            return Err(invalid(format!("`{field}`: `{p}` is not an absolute path")));
        }
    }
    Ok(paths)
}

/// An origin (`http://localhost:5000`) from an origin or a url.
fn origin(s: Option<String>) -> Result<Option<String>, CommandError> {
    let Some(s) = s else { return Ok(None) };
    match ParsedUrl::parse(&s)
        .filter(|u| !u.host.is_empty())
        .and_then(|u| u.origin())
    {
        Some(o) => Ok(Some(o)),
        None => Err(invalid(format!(
            "`origin` is a scheme, host and port such as `http://localhost:5000`, not `{s}`"
        ))),
    }
}

fn parse_input(i: InputIn) -> Result<BrowserRequest, CommandError> {
    use InputAction as A;
    let action = i.action;
    let target = parse_target("ref", i.ref_, i.x, i.y)?;
    if action.needs_target() && target.is_none() {
        return Err(invalid(format!(
            "`{}` needs a target: `ref` or `x` and `y`",
            action.as_str()
        )));
    }
    let only = |field: &str, given: bool, owner: A| -> Result<(), CommandError> {
        if given && action != owner {
            return Err(invalid(format!(
                "`{field}` goes with `action: {}`",
                owner.as_str()
            )));
        }
        Ok(())
    };
    only("text", i.text.is_some(), A::Type)?;
    only("per_key", i.per_key.is_some(), A::Type)?;
    only("keys", i.keys.is_some(), A::Key)?;
    only("delta", i.delta.is_some(), A::Scroll)?;
    only("to", i.to.is_some(), A::Drag)?;
    only("values", i.values.is_some(), A::Select)?;
    let text = match action {
        A::Type => Some(
            i.text
                .filter(|t| !t.is_empty())
                .ok_or_else(|| invalid("`type` needs `text`"))?,
        ),
        _ => None,
    };
    let keys = match (action, i.keys) {
        (A::Key, Some(k)) => {
            let list = match k {
                KeysIn::One(s) => vec![s],
                KeysIn::Many(v) => v,
            };
            if list.is_empty() || list.len() > 50 {
                return Err(invalid("`keys` lists 1 to 50 keys"));
            }
            list.iter()
                .map(|k| KeyPress::parse(k).map_err(invalid))
                .collect::<Result<Vec<_>, _>>()?
        }
        (A::Key, None) => return Err(invalid("`key` needs `keys`")),
        _ => Vec::new(),
    };
    let to = match (action, i.to) {
        (A::Drag, Some(p)) => Some(
            parse_target("to.ref", p.ref_, p.x, p.y)?
                .ok_or_else(|| invalid("`to` is a `ref` or `x` and `y`"))?,
        ),
        (A::Drag, None) => return Err(invalid("`drag` needs `to`")),
        _ => None,
    };
    let values = match (action, i.values) {
        (A::Select, Some(v)) if !v.is_empty() && v.len() <= 500 => v,
        (A::Select, _) => return Err(invalid("`select` needs `values` (1 to 500)")),
        _ => Vec::new(),
    };
    let delta = i.delta.map_or(SCROLL_DELTA, |d| (d.x, d.y));
    if !(delta.0.is_finite() && delta.1.is_finite()) {
        return Err(invalid("`delta` is finite"));
    }
    let mut modifiers = Modifiers::default();
    for m in i.modifiers {
        match m {
            ModifierIn::Shift => modifiers.shift = true,
            ModifierIn::Control => modifiers.control = true,
            ModifierIn::Alt => modifiers.alt = true,
            ModifierIn::Meta => modifiers.meta = true,
        }
    }
    Ok(BrowserRequest::Input {
        tab: check_tab(i.tab)?,
        action,
        target,
        text,
        per_key: i.per_key.unwrap_or(false),
        keys,
        delta,
        to,
        values,
        modifiers,
        wait_ms: range("wait_ms", i.wait_ms, 0, INPUT_MAX_WAIT_MS, INPUT_WAIT_MS)?,
    })
}

/// Parse and validate the input of browser command `id`.
pub fn parse(id: &str, value: Value) -> Result<BrowserRequest, CommandError> {
    Ok(match id {
        TABS => {
            let _: Empty = input(value)?;
            BrowserRequest::Tabs
        }
        TAB_OPEN => {
            let i: TabOpenIn = input(value)?;
            BrowserRequest::TabOpen {
                url: check_url(i.url)?,
            }
        }
        TAB_CLOSE => {
            let i: TabIn = input(value)?;
            BrowserRequest::TabClose {
                tab: check_tab(i.tab)?,
            }
        }
        TAB_SELECT => {
            let i: TabIn = input(value)?;
            BrowserRequest::TabSelect {
                tab: check_tab(i.tab)?.ok_or_else(|| invalid("`tab` is required"))?,
            }
        }
        NAVIGATE => {
            let i: NavigateIn = input(value)?;
            let to = match (check_url(i.url)?, i.action) {
                (Some(url), None) => NavigateTo::Url(url),
                (None, Some(Action::Back)) => NavigateTo::Back,
                (None, Some(Action::Forward)) => NavigateTo::Forward,
                (None, Some(Action::Reload)) => NavigateTo::Reload,
                _ => return Err(invalid("give exactly one of `url` and `action`")),
            };
            BrowserRequest::Navigate {
                tab: check_tab(i.tab)?,
                to,
                wait_until: i.wait_until,
                wait_ms: range(
                    "wait_ms",
                    i.wait_ms,
                    0,
                    NAVIGATE_MAX_WAIT_MS,
                    NAVIGATE_WAIT_MS,
                )?,
            }
        }
        RESIZE => {
            let i: ResizeIn = required(value)?;
            if let Some(f) = i.device_scale_factor
                && !(f > 0. && f <= 10.)
            {
                return Err(invalid(format!(
                    "`device_scale_factor` is above 0 and at most 10, not {f}"
                )));
            }
            BrowserRequest::Resize {
                tab: check_tab(i.tab)?,
                width: range("width", Some(i.width), 1, 10_000, 1)?,
                height: range("height", Some(i.height), 1, 10_000, 1)?,
                device_scale_factor: i.device_scale_factor,
                mobile: i.mobile,
                user_agent: non_empty("user_agent", i.user_agent)?,
            }
        }
        SCREENSHOT => {
            let i: ScreenshotIn = input(value)?;
            let clip = match i.clip {
                None => None,
                Some(ClipIn::Ref(r)) => {
                    Some(Clip::Ref(check_ref("clip", Some(r))?.unwrap_or_default()))
                }
                Some(ClipIn::Rect(r)) => {
                    if !(r.width > 0. && r.height > 0.) {
                        return Err(invalid(
                            "a `clip` rectangle has a positive width and height",
                        ));
                    }
                    Some(Clip::Rect(r))
                }
            };
            if i.full_page && clip.is_some() {
                return Err(invalid("give `full_page` or `clip`, not both"));
            }
            if i.quality.is_some() && i.format != ImageFormat::Jpeg {
                return Err(invalid("`quality` goes with `format: \"jpeg\"`"));
            }
            BrowserRequest::Screenshot {
                tab: check_tab(i.tab)?,
                full_page: i.full_page,
                clip,
                max_width: range("max_width", i.max_width, 16, 4096, MAX_WIDTH)?,
                format: i.format,
                quality: match i.format {
                    ImageFormat::Jpeg => Some(range("quality", i.quality, 0, 100, JPEG_QUALITY)?),
                    ImageFormat::Png => None,
                },
            }
        }
        READ_PAGE => {
            let i: ReadPageIn = input(value)?;
            BrowserRequest::ReadPage {
                tab: check_tab(i.tab)?,
                mode: i.mode,
                filter: i.filter,
                root: check_ref("root", i.root)?,
                max_nodes: range("max_nodes", i.max_nodes, 1, 5000, MAX_NODES)?,
            }
        }
        FIND => {
            let i: FindIn = input(value)?;
            let by = match (
                non_empty("css", i.css)?,
                non_empty("text", i.text)?,
                non_empty("role", i.role)?,
            ) {
                (Some(css), None, None) if i.name.is_none() => FindBy::Css(css),
                (None, Some(text), None) if i.name.is_none() => FindBy::Text(text),
                (None, None, Some(role)) => FindBy::Role {
                    role: role.trim().to_owned(),
                    name: i.name.filter(|n| !n.trim().is_empty()),
                },
                (None, None, None) if i.name.is_some() => {
                    return Err(invalid("`name` goes with `role`"));
                }
                _ => {
                    return Err(invalid(
                        "give exactly one of `css`, `text` and `role` (`name` only with `role`)",
                    ));
                }
            };
            BrowserRequest::Find {
                tab: check_tab(i.tab)?,
                by,
                max: range("max", i.max, 1, 500, FIND_MAX)?,
            }
        }
        PAGE_TEXT => {
            let i: PageTextIn = input(value)?;
            BrowserRequest::PageText {
                tab: check_tab(i.tab)?,
                root: check_ref("root", i.root)?,
                max_chars: range("max_chars", i.max_chars, 1, 200_000, PAGE_TEXT_CHARS)?,
                cursor: i.cursor,
            }
        }
        CONSOLE => {
            let i: ConsoleIn = input(value)?;
            BrowserRequest::Console {
                tab: check_tab(i.tab)?,
                since: i.since,
                level: i.level,
                pattern: pattern("pattern", i.pattern)?,
                max: range("max", i.max, 1, 2000, LOG_MAX)?,
            }
        }
        NETWORK => {
            let i: NetworkIn = input(value)?;
            BrowserRequest::Network {
                tab: check_tab(i.tab)?,
                since: i.since,
                url_pattern: pattern("url_pattern", i.url_pattern)?,
                status: match i.status {
                    Some(s) if s > 999 => return Err(invalid("`status` is at most 999")),
                    s => s,
                },
                resource_type: non_empty("resource_type", i.resource_type)?,
                max: range("max", i.max, 1, 5000, LOG_MAX)?,
            }
        }
        WAIT => {
            let i: WaitIn = required(value)?;
            let need = |field: &str, v: Option<String>| -> Result<String, CommandError> {
                non_empty(field, v)?.ok_or_else(|| invalid(format!("this `for` needs `{field}`")))
            };
            let given = [&i.css, &i.text, &i.pattern, &i.expression]
                .iter()
                .filter(|v| v.is_some())
                .count();
            let condition = match i.for_ {
                WaitKind::Selector => WaitFor::Selector(need("css", i.css)?),
                WaitKind::Text => WaitFor::Text(need("text", i.text)?),
                WaitKind::Navigation => WaitFor::Navigation,
                WaitKind::NetworkIdle => WaitFor::NetworkIdle,
                WaitKind::Console => {
                    WaitFor::Console(TextPattern::parse(&need("pattern", i.pattern)?))
                }
                WaitKind::Function => WaitFor::Function(need("expression", i.expression)?),
            };
            let expected = usize::from(!matches!(
                condition,
                WaitFor::Navigation | WaitFor::NetworkIdle
            ));
            if given != expected {
                return Err(invalid(
                    "give only the field `for` needs: `css` (selector), `text` (text), `pattern` (console) or `expression` (function)",
                ));
            }
            BrowserRequest::Wait {
                tab: check_tab(i.tab)?,
                condition,
                wait_ms: range("wait_ms", i.wait_ms, 0, WAIT_MAX_MS, WAIT_MS)?,
            }
        }
        EVALUATE => {
            let i: EvaluateIn = required(value)?;
            if i.expression.trim().is_empty() {
                return Err(invalid("`expression` must not be empty"));
            }
            BrowserRequest::Evaluate {
                tab: check_tab(i.tab)?,
                expression: i.expression,
                await_promise: i.await_.unwrap_or(true),
                return_by_value: i.return_by_value.unwrap_or(true),
                max_chars: range("max_chars", i.max_chars, 1, 1_000_000, EVALUATE_CHARS)?,
            }
        }
        INPUT => parse_input(required(value)?)?,
        FORM_INPUT => {
            let i: FormInputIn = required(value)?;
            if i.fields.is_empty() || i.fields.len() > FORM_FIELDS {
                return Err(invalid(format!(
                    "`fields` lists 1 to {FORM_FIELDS} fields, not {}",
                    i.fields.len()
                )));
            }
            let fields = i
                .fields
                .into_iter()
                .map(|f| {
                    Ok(FormField {
                        ref_: check_ref("ref", Some(f.ref_))?.unwrap_or_default(),
                        value: match f.value {
                            ValueIn::Text(t) => FieldValue::Text(t),
                            ValueIn::Checked(b) => FieldValue::Checked(b),
                            ValueIn::Options(o) => FieldValue::Options(o),
                            ValueIn::Files(f) => FieldValue::Files(files("files", f.files)?),
                        },
                    })
                })
                .collect::<Result<Vec<_>, CommandError>>()?;
            BrowserRequest::FormInput {
                tab: check_tab(i.tab)?,
                fields,
            }
        }
        UPLOAD => {
            let i: UploadIn = required(value)?;
            BrowserRequest::Upload {
                tab: check_tab(i.tab)?,
                ref_: check_ref("ref", Some(i.ref_))?.unwrap_or_default(),
                paths: files("paths", i.paths)?,
            }
        }
        STORAGE => {
            let i: StorageIn = input(value)?;
            BrowserRequest::Storage {
                tab: check_tab(i.tab)?,
                kind: i.kind,
                action: i.action,
                origin: origin(i.origin)?,
            }
        }
        NETWORK_BODY => {
            let i: NetworkBodyIn = required(value)?;
            BrowserRequest::NetworkBody {
                tab: check_tab(i.tab)?,
                request_id: non_empty("request_id", Some(i.request_id))?.unwrap_or_default(),
                max_bytes: range(
                    "max_bytes",
                    i.max_bytes,
                    1,
                    NETWORK_BODY_MAX_BYTES,
                    NETWORK_BODY_BYTES,
                )?,
            }
        }
        OPEN_EXTERNAL => {
            let i: OpenExternalIn = input(value)?;
            BrowserRequest::OpenExternal {
                url: check_url(i.url)?,
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

// ---- escalation (ADR-0009) ----

/// A url off the allowed origins: dangerous, Always Allow adding its origin.
fn off_origin(view: &PolicyView, url: &str, what: &str) -> Option<Escalation> {
    let off = view.check_url(url).err()?;
    Some(Escalation::Raise {
        class: PermissionClass::Dangerous,
        reason: format!("{what} off the allowed origins: {}", off.shown),
        always_allow: off.origin.map_or(AlwaysAllow::Never, AlwaysAllow::Origin),
    })
}

/// The first of `paths` outside the workspace: dangerous, Always Allow allowing once.
fn outside_workspace<'a>(
    view: &PolicyView,
    paths: impl IntoIterator<Item = &'a str>,
    what: &str,
) -> Option<Escalation> {
    let path = paths.into_iter().find(|p| !view.in_workspace(p))?;
    Some(Escalation::Raise {
        class: PermissionClass::Dangerous,
        reason: format!("{what} a file outside the workspace: {path}"),
        always_allow: AlwaysAllow::Never,
    })
}

fn str_array(v: &Value) -> impl Iterator<Item = &str> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str)
}

/// The escalation hook of browser command `id`, if it has one: the solution policy's `browser` object applied to
/// the call's input (ADR-0009).
pub fn escalation(id: &str) -> Option<EscalationHook> {
    let hook: EscalationHook = match id {
        NAVIGATE => Arc::new(|input: &Value, view: &PolicyView| {
            off_origin(view, input.get("url")?.as_str()?.trim(), "navigate")
        }),
        TAB_OPEN => Arc::new(|input: &Value, view: &PolicyView| {
            off_origin(view, input.get("url")?.as_str()?.trim(), "open a tab")
        }),
        OPEN_EXTERNAL => Arc::new(|input: &Value, view: &PolicyView| {
            off_origin(
                view,
                input.get("url")?.as_str()?.trim(),
                "open in the system browser",
            )
        }),
        EVALUATE => Arc::new(|_: &Value, view: &PolicyView| {
            match view.browser().evaluate.unwrap_or_default() {
                EvaluatePolicy::Allow => None,
                EvaluatePolicy::Prompt => Some(Escalation::raise(
                    PermissionClass::Dangerous,
                    "the solution's policy asks before running JavaScript in the page (browser.evaluate: prompt)",
                )),
                EvaluatePolicy::Deny => Some(Escalation::Refuse(
                    "the solution's policy sets browser.evaluate to deny".into(),
                )),
            }
        }),
        NETWORK_BODY => Arc::new(|_: &Value, view: &PolicyView| {
            (view.browser().network_bodies.unwrap_or_default() == NetworkBodiesPolicy::Deny).then(
                || {
                    Escalation::Refuse(
                        "the solution's policy sets browser.network_bodies to deny".into(),
                    )
                },
            )
        }),
        UPLOAD => Arc::new(|input: &Value, view: &PolicyView| {
            outside_workspace(view, str_array(input.get("paths")?), "upload")
        }),
        FORM_INPUT => Arc::new(|input: &Value, view: &PolicyView| {
            let paths = input
                .get("fields")?
                .as_array()?
                .iter()
                .filter_map(|f| f.pointer("/value/files"))
                .flat_map(str_array);
            outside_workspace(view, paths, "fill a file input with")
        }),
        STORAGE => Arc::new(|input: &Value, _: &PolicyView| {
            (input.get("action").and_then(Value::as_str) == Some("clear")).then(|| {
                Escalation::raise(
                    PermissionClass::Execute,
                    "storage clear deletes the site's cookies and storage",
                )
            })
        }),
        _ => return None,
    };
    Some(hook)
}

/// The public description of browser command `id` (one of [`ALL`]). All are agent-visible.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register every browser command, applying them to `target`, with their escalation hooks.
pub fn register(registry: &CommandRegistry, target: Arc<dyn BrowserTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Absolute on this platform: `/w` is not absolute on Windows.
    const W: &str = if cfg!(windows) { "C:/w" } else { "/w" };
    const W_A: &str = if cfg!(windows) {
        "C:/w/a.txt"
    } else {
        "/w/a.txt"
    };
    const OUTSIDE: &str = if cfg!(windows) {
        "C:/etc/passwd"
    } else {
        "/etc/passwd"
    };
    use serde_json::json;

    /// Checks `value` against `schema` for what these outputs use: `required`, `additionalProperties: false`,
    /// nested objects and arrays of objects.
    fn conforms(schema: &Value, value: &Value, at: &str) {
        if let Some(obj) = value.as_object() {
            for r in schema["required"].as_array().into_iter().flatten() {
                assert!(obj.contains_key(r.as_str().unwrap()), "{at}: missing {r}");
            }
            if schema["additionalProperties"] == json!(false) {
                for (k, v) in obj {
                    let sub = &schema["properties"][k];
                    assert!(!sub.is_null(), "{at}: unexpected {k}");
                    conforms(sub, v, &format!("{at}.{k}"));
                }
            }
        }
        if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
            for (i, v) in arr.iter().enumerate() {
                conforms(items, v, &format!("{at}[{i}]"));
            }
        }
        if let Some(Value::Array(e)) = schema.get("enum") {
            assert!(e.contains(value), "{at}: {value} not in {e:?}");
        }
    }

    #[test]
    fn every_command_has_its_schemas_and_class() {
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible, "{id}");
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["type"], "object");
            assert!(
                s.input_schema["description"].as_str().unwrap().len() > 150,
                "{id}: the description is written for a model"
            );
        }
        let read = [
            TABS,
            SCREENSHOT,
            READ_PAGE,
            FIND,
            PAGE_TEXT,
            CONSOLE,
            NETWORK,
            WAIT,
            STORAGE,
            NETWORK_BODY,
        ];
        for id in ALL {
            let expected = if read.contains(&id) {
                PermissionClass::Read
            } else {
                PermissionClass::Execute
            };
            assert_eq!(spec(id).permission, expected, "{id}");
        }
        assert_eq!(
            spec(SCREENSHOT).output_schema["properties"]["image"]["x-eludite-mcp-content"],
            "image"
        );
        // A command with an escalation hook says when in its schema, and only those do.
        for id in ALL {
            assert_eq!(
                escalation(id).is_some(),
                spec(id).escalates().is_some(),
                "{id}"
            );
        }
        assert_eq!(ALL.iter().filter(|id| escalation(id).is_some()).count(), 8);
    }

    fn view(policy: Value, workspace: Option<&str>) -> crate::policy::PolicyView {
        crate::policy::PolicyView::of(crate::policy::PolicySnapshot {
            policy: serde_json::from_value(policy).unwrap(),
            workspace: workspace.map(Into::into),
            launch_urls: vec!["https://localhost:7001".into()],
            ..Default::default()
        })
    }

    fn class_of(id: &str, input: Value, v: &crate::policy::PolicyView) -> Option<Escalation> {
        escalation(id).and_then(|h| h(&input, v))
    }

    #[test]
    fn hooks_apply_the_browser_policy() {
        use crate::policy::AlwaysAllow;
        let d = view(json!({"version": 1}), Some(W));
        // navigate, tab_open, open_external: the origin rule.
        assert_eq!(
            class_of(NAVIGATE, json!({"url": "http://127.0.0.1:4000/"}), &d),
            None
        );
        assert_eq!(class_of(NAVIGATE, json!({"action": "reload"}), &d), None);
        assert_eq!(
            class_of(NAVIGATE, json!({"url": "https://example.com/x"}), &d),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "navigate off the allowed origins: https://example.com".into(),
                always_allow: AlwaysAllow::Origin("https://example.com".into()),
            })
        );
        assert!(matches!(
            class_of(TAB_OPEN, json!({"url": "data:text/html,x"}), &d),
            Some(Escalation::Raise {
                always_allow: AlwaysAllow::Never,
                ..
            })
        ));
        assert_eq!(class_of(TAB_OPEN, json!({}), &d), None);
        assert!(class_of(OPEN_EXTERNAL, json!({"url": "https://example.com"}), &d).is_some());
        assert_eq!(
            class_of(OPEN_EXTERNAL, json!({"url": "https://localhost:7001/"}), &d),
            None
        );
        // The file rule.
        assert_eq!(
            class_of(UPLOAD, json!({"ref": "e1", "paths": [W_A]}), &d),
            None
        );
        let up = class_of(UPLOAD, json!({"ref": "e1", "paths": [W_A, OUTSIDE]}), &d);
        assert_eq!(
            up,
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: format!("upload a file outside the workspace: {OUTSIDE}"),
                always_allow: AlwaysAllow::Never,
            })
        );
        let fields = json!({"fields": [{"ref": "e1", "value": "x"}, {"ref": "e2", "value": {"files": [OUTSIDE]}}]});
        assert!(class_of(FORM_INPUT, fields.clone(), &d).is_some());
        assert_eq!(
            class_of(
                FORM_INPUT,
                json!({"fields": [{"ref": "e1", "value": "x"}]}),
                &d
            ),
            None
        );
        assert!(
            class_of(
                UPLOAD,
                json!({"ref": "e1", "paths": [W_A]}),
                &view(json!({"version": 1}), None)
            )
            .is_some(),
            "without a workspace every file is outside it"
        );
        // evaluate and network_body follow their policies.
        assert_eq!(class_of(EVALUATE, json!({"expression": "1"}), &d), None);
        assert!(matches!(
            class_of(
                EVALUATE,
                json!({"expression": "1"}),
                &view(
                    json!({"version": 1, "browser": {"evaluate": "prompt"}}),
                    None
                )
            ),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                always_allow: AlwaysAllow::Rule,
                ..
            })
        ));
        assert_eq!(
            class_of(
                EVALUATE,
                json!({"expression": "1"}),
                &view(json!({"version": 1, "browser": {"evaluate": "deny"}}), None)
            ),
            Some(Escalation::Refuse(
                "the solution's policy sets browser.evaluate to deny".into()
            ))
        );
        assert_eq!(class_of(NETWORK_BODY, json!({"request_id": "1"}), &d), None);
        assert!(matches!(
            class_of(NETWORK_BODY, json!({"request_id": "1"}), &view(json!({"version": 1, "browser": {"network_bodies": "deny"}}), None)),
            Some(Escalation::Refuse(m)) if m.contains("browser.network_bodies")
        ));
        // storage: get is read, clear is execute.
        assert_eq!(class_of(STORAGE, json!({"action": "get"}), &d), None);
        assert!(matches!(
            class_of(STORAGE, json!({"action": "clear"}), &d),
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                ..
            })
        ));
        assert_eq!(
            class_of(INPUT, json!({"action": "click", "ref": "e1"}), &d),
            None
        );
    }

    #[test]
    fn registered_hooks_raise_the_class_of_a_call() {
        struct Nothing;
        impl BrowserTarget for Nothing {
            fn apply(&self, r: BrowserRequest) -> Result<BrowserOutput, CommandError> {
                Err(CommandError::Failed(format!("{} not here", r.command())))
            }
        }
        let r = CommandRegistry::new();
        register(&r, Arc::new(Nothing));
        r.set_policy_source(Arc::new(|| crate::policy::PolicySnapshot {
            workspace: Some(W.into()),
            ..Default::default()
        }));
        let c = r
            .classify(NAVIGATE, &json!({"url": "https://example.com"}))
            .unwrap();
        assert_eq!(c.class, PermissionClass::Dangerous);
        let c = r
            .classify(NAVIGATE, &json!({"url": "http://localhost:1/"}))
            .unwrap();
        assert_eq!(c.class, PermissionClass::Execute);
        let c = r.classify(STORAGE, &json!({"action": "clear"})).unwrap();
        assert_eq!(c.class, PermissionClass::Execute);
        assert_eq!(
            r.classify(STORAGE, &json!({})).unwrap().class,
            PermissionClass::Read
        );
    }

    #[test]
    fn parses_and_validates() {
        assert_eq!(parse(TABS, Value::Null).unwrap(), BrowserRequest::Tabs);
        assert!(parse(TABS, json!({"x": 1})).is_err());
        assert_eq!(
            parse(TAB_OPEN, json!({"url": "http://127.0.0.1:8080/"})).unwrap(),
            BrowserRequest::TabOpen {
                url: Some("http://127.0.0.1:8080/".into())
            }
        );
        assert!(
            parse(TAB_OPEN, json!({"url": "localhost:5000"})).is_ok(),
            "a scheme-like host is passed on"
        );
        assert!(parse(TAB_OPEN, json!({"url": "example.com/x"})).is_err());
        assert!(parse(TAB_OPEN, json!({"url": "/relative"})).is_err());
        assert_eq!(
            parse(TAB_CLOSE, json!({})).unwrap(),
            BrowserRequest::TabClose { tab: None }
        );
        assert!(parse(TAB_CLOSE, json!({"tab": "x1"})).is_err());
        assert!(parse(TAB_SELECT, json!({})).is_err());
        assert_eq!(
            parse(TAB_SELECT, json!({"tab": "t2"})).unwrap(),
            BrowserRequest::TabSelect { tab: "t2".into() }
        );
        assert_eq!(
            parse(NAVIGATE, json!({"url": "about:blank"})).unwrap(),
            BrowserRequest::Navigate {
                tab: None,
                to: NavigateTo::Url("about:blank".into()),
                wait_until: WaitUntil::Load,
                wait_ms: NAVIGATE_WAIT_MS
            }
        );
        assert_eq!(
            parse(
                NAVIGATE,
                json!({"action": "back", "wait_until": "network_idle", "wait_ms": 5, "tab": "t1"})
            )
            .unwrap(),
            BrowserRequest::Navigate {
                tab: Some("t1".into()),
                to: NavigateTo::Back,
                wait_until: WaitUntil::NetworkIdle,
                wait_ms: 5
            }
        );
        assert!(parse(NAVIGATE, json!({})).is_err());
        assert!(parse(NAVIGATE, json!({"url": "about:blank", "action": "reload"})).is_err());
        assert!(parse(NAVIGATE, json!({"action": "reload", "wait_ms": 200000})).is_err());
        assert!(parse(RESIZE, json!({"width": 390})).is_err());
        assert!(
            parse(
                RESIZE,
                json!({"width": 390, "height": 844, "device_scale_factor": 0})
            )
            .is_err()
        );
        assert!(matches!(
            parse(
                RESIZE,
                json!({"width": 390, "height": 844, "mobile": true, "device_scale_factor": 3})
            )
            .unwrap(),
            BrowserRequest::Resize {
                width: 390,
                height: 844,
                mobile: true,
                device_scale_factor: Some(3.0),
                ..
            }
        ));
        assert_eq!(
            parse(SCREENSHOT, json!({})).unwrap(),
            BrowserRequest::Screenshot {
                tab: None,
                full_page: false,
                clip: None,
                max_width: MAX_WIDTH,
                format: ImageFormat::Png,
                quality: None
            }
        );
        assert!(matches!(
            parse(SCREENSHOT, json!({"clip": "e4", "format": "jpeg"})).unwrap(),
            BrowserRequest::Screenshot {
                clip: Some(Clip::Ref(_)),
                quality: Some(JPEG_QUALITY),
                ..
            }
        ));
        assert!(matches!(
            parse(
                SCREENSHOT,
                json!({"clip": {"x": 0, "y": 0, "width": 10, "height": 5}})
            )
            .unwrap(),
            BrowserRequest::Screenshot {
                clip: Some(Clip::Rect(_)),
                ..
            }
        ));
        assert!(parse(SCREENSHOT, json!({"clip": "button"})).is_err());
        assert!(
            parse(
                SCREENSHOT,
                json!({"clip": {"x": 0, "y": 0, "width": 0, "height": 5}})
            )
            .is_err()
        );
        assert!(parse(SCREENSHOT, json!({"quality": 50})).is_err());
        assert!(parse(SCREENSHOT, json!({"max_width": 8})).is_err());
        assert!(parse(SCREENSHOT, json!({"full_page": true, "clip": "e1"})).is_err());
        assert_eq!(
            parse(
                READ_PAGE,
                json!({"mode": "dom", "filter": "all", "root": "e3", "max_nodes": 10})
            )
            .unwrap(),
            BrowserRequest::ReadPage {
                tab: None,
                mode: ReadMode::Dom,
                filter: ReadFilter::All,
                root: Some("e3".into()),
                max_nodes: 10
            }
        );
        assert!(parse(READ_PAGE, json!({"max_nodes": 0})).is_err());
        assert!(parse(READ_PAGE, json!({"root": "3"})).is_err());
        assert_eq!(
            parse(FIND, json!({"role": "button", "name": "Save"})).unwrap(),
            BrowserRequest::Find {
                tab: None,
                by: FindBy::Role {
                    role: "button".into(),
                    name: Some("Save".into())
                },
                max: FIND_MAX
            }
        );
        assert!(parse(FIND, json!({"css": "a", "text": "b"})).is_err());
        assert!(parse(FIND, json!({"css": "a", "name": "b"})).is_err());
        assert!(parse(FIND, json!({"name": "b"})).is_err());
        assert!(parse(FIND, json!({})).is_err());
        assert!(parse(FIND, json!({"css": " "})).is_err());
        assert!(matches!(
            parse(PAGE_TEXT, json!({"cursor": 20000})).unwrap(),
            BrowserRequest::PageText {
                cursor: 20000,
                max_chars: PAGE_TEXT_CHARS,
                ..
            }
        ));
        assert!(matches!(
            parse(
                CONSOLE,
                json!({"since": 4, "level": "warning", "pattern": "/fail(ed)?/i"})
            )
            .unwrap(),
            BrowserRequest::Console {
                since: 4,
                level: Some(ConsoleLevel::Warning),
                pattern: Some(TextPattern::Regex {
                    case_insensitive: true,
                    ..
                }),
                max: LOG_MAX,
                ..
            }
        ));
        assert!(parse(CONSOLE, json!({"level": "fatal"})).is_err());
        assert!(matches!(
            parse(NETWORK, json!({"status": 404, "url_pattern": "missing"})).unwrap(),
            BrowserRequest::Network {
                status: Some(404),
                url_pattern: Some(TextPattern::Substring(_)),
                ..
            }
        ));
        assert_eq!(
            parse(WAIT, json!({"for": "selector", "css": "#late"})).unwrap(),
            BrowserRequest::Wait {
                tab: None,
                condition: WaitFor::Selector("#late".into()),
                wait_ms: WAIT_MS
            }
        );
        assert!(parse(WAIT, json!({"for": "selector"})).is_err());
        assert!(parse(WAIT, json!({"for": "navigation", "css": "x"})).is_err());
        assert!(parse(WAIT, json!({"for": "network_idle", "wait_ms": 60001})).is_err());
        assert!(parse(WAIT, json!({})).is_err());
        assert_eq!(
            parse(EVALUATE, json!({"expression": "1+1", "await": false})).unwrap(),
            BrowserRequest::Evaluate {
                tab: None,
                expression: "1+1".into(),
                await_promise: false,
                return_by_value: true,
                max_chars: EVALUATE_CHARS
            }
        );
        assert!(parse(EVALUATE, json!({})).is_err());
        assert!(parse(EVALUATE, json!({"expression": "  "})).is_err());
        assert!(parse("eludite.browser.bogus", json!({})).is_err());
    }

    #[test]
    fn parses_and_validates_the_actions() {
        // input
        assert_eq!(
            parse(INPUT, json!({"action": "click", "ref": "e3"})).unwrap(),
            BrowserRequest::Input {
                tab: None,
                action: InputAction::Click,
                target: Some(Target::Ref("e3".into())),
                text: None,
                per_key: false,
                keys: vec![],
                delta: SCROLL_DELTA,
                to: None,
                values: vec![],
                modifiers: Modifiers::default(),
                wait_ms: INPUT_WAIT_MS,
            }
        );
        assert!(matches!(
            parse(INPUT, json!({"action": "double_click", "x": 10, "y": 20.5, "modifiers": ["shift", "control"], "wait_ms": 100})).unwrap(),
            BrowserRequest::Input { target: Some(Target::Point { x, y }), modifiers, wait_ms: 100, .. }
                if x == 10. && y == 20.5 && modifiers.bits() == 10
        ));
        assert!(
            parse(INPUT, json!({"action": "click"})).is_err(),
            "needs a target"
        );
        assert!(parse(INPUT, json!({"action": "click", "x": 1})).is_err());
        assert!(
            parse(
                INPUT,
                json!({"action": "click", "ref": "e1", "x": 1, "y": 2})
            )
            .is_err()
        );
        assert!(parse(INPUT, json!({"action": "click", "x": -1, "y": 2})).is_err());
        assert!(parse(INPUT, json!({"action": "click", "ref": "button"})).is_err());
        assert!(parse(INPUT, json!({"action": "click", "ref": "e1", "text": "x"})).is_err());
        assert!(parse(INPUT, json!({"action": "tap", "ref": "e1"})).is_err());
        assert!(
            parse(
                INPUT,
                json!({"action": "click", "ref": "e1", "wait_ms": 10001})
            )
            .is_err()
        );
        assert!(matches!(
            parse(INPUT, json!({"action": "type", "text": "hi", "per_key": true})).unwrap(),
            BrowserRequest::Input { target: None, per_key: true, text: Some(t), .. } if t == "hi"
        ));
        assert!(parse(INPUT, json!({"action": "type"})).is_err());
        let BrowserRequest::Input { keys, .. } =
            parse(INPUT, json!({"action": "key", "keys": ["enter", "Control+a", "Shift+Tab", "Control++", "+", "Esc"]})).unwrap()
        else {
            panic!()
        };
        let names: Vec<_> = keys
            .iter()
            .map(|k| (k.key.as_str(), k.modifiers.bits()))
            .collect();
        assert_eq!(
            names,
            [
                ("Enter", 0),
                ("a", 2),
                ("Tab", 8),
                ("+", 2),
                ("+", 0),
                ("Escape", 0)
            ]
        );
        assert!(matches!(
            parse(INPUT, json!({"action": "key", "keys": "ArrowDown"})).unwrap(),
            BrowserRequest::Input { keys, .. } if keys[0].key == "ArrowDown"
        ));
        assert!(parse(INPUT, json!({"action": "key", "keys": ["Hyper+a"]})).is_err());
        assert!(parse(INPUT, json!({"action": "key", "keys": ["Enterr"]})).is_err());
        assert!(parse(INPUT, json!({"action": "key"})).is_err());
        assert!(matches!(
            parse(INPUT, json!({"action": "scroll", "delta": {"y": -200}})).unwrap(),
            BrowserRequest::Input {
                delta: (0., -200.),
                target: None,
                ..
            }
        ));
        assert!(matches!(
            parse(
                INPUT,
                json!({"action": "drag", "ref": "e1", "to": {"x": 300, "y": 10}})
            )
            .unwrap(),
            BrowserRequest::Input {
                to: Some(Target::Point { .. }),
                ..
            }
        ));
        assert!(parse(INPUT, json!({"action": "drag", "ref": "e1"})).is_err());
        assert!(parse(INPUT, json!({"action": "drag", "ref": "e1", "to": {}})).is_err());
        assert!(matches!(
            parse(INPUT, json!({"action": "select", "ref": "e1", "values": ["pro"]})).unwrap(),
            BrowserRequest::Input { values, .. } if values == ["pro"]
        ));
        assert!(parse(INPUT, json!({"action": "select", "ref": "e1"})).is_err());
        assert!(
            parse(
                INPUT,
                json!({"action": "select", "ref": "e1", "values": []})
            )
            .is_err()
        );
        // form_input
        assert_eq!(
            parse(
                FORM_INPUT,
                json!({"fields": [
                    {"ref": "e1", "value": "Ada"},
                    {"ref": "e2", "value": true},
                    {"ref": "e3", "value": ["a", "b"]},
                    {"ref": "e4", "value": {"files": [W_A]}}
                ], "tab": "t2"})
            )
            .unwrap(),
            BrowserRequest::FormInput {
                tab: Some("t2".into()),
                fields: vec![
                    FormField {
                        ref_: "e1".into(),
                        value: FieldValue::Text("Ada".into())
                    },
                    FormField {
                        ref_: "e2".into(),
                        value: FieldValue::Checked(true)
                    },
                    FormField {
                        ref_: "e3".into(),
                        value: FieldValue::Options(vec!["a".into(), "b".into()])
                    },
                    FormField {
                        ref_: "e4".into(),
                        value: FieldValue::Files(vec![W_A.into()])
                    },
                ]
            }
        );
        assert!(parse(FORM_INPUT, json!({"fields": []})).is_err());
        assert!(parse(FORM_INPUT, json!({"fields": [{"ref": "e1"}]})).is_err());
        assert!(parse(FORM_INPUT, json!({"fields": [{"ref": "e1", "value": 3}]})).is_err());
        assert!(
            parse(
                FORM_INPUT,
                json!({"fields": [{"ref": "e1", "value": {"files": ["rel.txt"]}}]})
            )
            .is_err()
        );
        let many: Vec<Value> = (0..101)
            .map(|i| json!({"ref": format!("e{i}"), "value": "x"}))
            .collect();
        assert!(parse(FORM_INPUT, json!({"fields": many})).is_err());
        // upload
        assert_eq!(
            parse(UPLOAD, json!({"ref": "e9", "paths": [W_A]})).unwrap(),
            BrowserRequest::Upload {
                tab: None,
                ref_: "e9".into(),
                paths: vec![W_A.into()]
            }
        );
        assert!(parse(UPLOAD, json!({"ref": "e9", "paths": []})).is_err());
        assert!(parse(UPLOAD, json!({"paths": ["/a"]})).is_err());
        // storage
        assert_eq!(
            parse(STORAGE, Value::Null).unwrap(),
            BrowserRequest::Storage {
                tab: None,
                kind: StorageKind::All,
                action: StorageAction::Get,
                origin: None
            }
        );
        assert_eq!(
            parse(
                STORAGE,
                json!({"kind": "cookies", "action": "clear", "origin": "HTTP://LocalHost:5000/x"})
            )
            .unwrap(),
            BrowserRequest::Storage {
                tab: None,
                kind: StorageKind::Cookies,
                action: StorageAction::Clear,
                origin: Some("http://localhost:5000".into())
            }
        );
        assert!(parse(STORAGE, json!({"origin": "localhost"})).is_err());
        assert!(parse(STORAGE, json!({"kind": "indexeddb"})).is_err());
        // network_body
        assert_eq!(
            parse(NETWORK_BODY, json!({"request_id": "12.3"})).unwrap(),
            BrowserRequest::NetworkBody {
                tab: None,
                request_id: "12.3".into(),
                max_bytes: NETWORK_BODY_BYTES
            }
        );
        assert!(parse(NETWORK_BODY, json!({"request_id": ""})).is_err());
        assert!(
            parse(
                NETWORK_BODY,
                json!({"request_id": "1", "max_bytes": 10485761})
            )
            .is_err()
        );
        // open_external
        assert_eq!(
            parse(OPEN_EXTERNAL, json!({})).unwrap(),
            BrowserRequest::OpenExternal { url: None }
        );
        assert!(parse(OPEN_EXTERNAL, json!({"url": "example.com"})).is_err());
        assert_eq!(
            parse(
                INPUT,
                json!({"action": "click", "ref": "e1", "wait_ms": 300})
            )
            .unwrap()
            .budget_ms(),
            60_300
        );
    }

    #[test]
    fn patterns() {
        assert_eq!(
            TextPattern::parse("boom"),
            TextPattern::Substring("boom".into())
        );
        assert_eq!(
            TextPattern::parse("/a+b/"),
            TextPattern::Regex {
                source: "a+b".into(),
                case_insensitive: false
            }
        );
        assert_eq!(
            TextPattern::parse("/x/i"),
            TextPattern::Regex {
                source: "x".into(),
                case_insensitive: true
            }
        );
        assert_eq!(TextPattern::parse("/"), TextPattern::Substring("/".into()));
        assert_eq!(
            TextPattern::parse("/api"),
            TextPattern::Substring("/api".into())
        );
        assert_eq!(
            TextPattern::parse("/i"),
            TextPattern::Substring("/i".into())
        );
    }

    #[test]
    fn outputs_follow_their_schemas() {
        let tab = TabRow {
            id: "t1".into(),
            url: "http://127.0.0.1/".into(),
            title: "Form".into(),
            active: true,
            loading: false,
            page_generation: 2,
        };
        let samples = [
            (
                TABS,
                BrowserOutput::Tabs(TabsOutput {
                    running: true,
                    engine: EngineRow {
                        name: "external-chrome".into(),
                        version: Some("HeadlessChrome/141.0.7390.37".into()),
                        executable: Some("/opt/chrome".into()),
                    },
                    active: Some("t1".into()),
                    tabs: vec![tab.clone()],
                }),
            ),
            (
                TAB_OPEN,
                BrowserOutput::TabOpen(TabOpenOutput {
                    tab: tab.clone(),
                    launched: true,
                    status: Some(200),
                }),
            ),
            (
                TAB_CLOSE,
                BrowserOutput::TabClose(TabCloseOutput {
                    closed: "t1".into(),
                    active: None,
                    tabs: 0,
                }),
            ),
            (TAB_SELECT, BrowserOutput::TabSelect(tab.clone())),
            (
                NAVIGATE,
                BrowserOutput::Navigate(NavigateOutput {
                    tab: "t1".into(),
                    url: "http://127.0.0.1/".into(),
                    title: "Form".into(),
                    status: Some(200),
                    page_generation: 3,
                    console_errors: 1,
                    timed_out: false,
                    elapsed_ms: 12.5,
                }),
            ),
            (
                RESIZE,
                BrowserOutput::Resize(ResizeOutput {
                    tab: "t1".into(),
                    width: 390,
                    height: 844,
                    device_scale_factor: 3.,
                    mobile: true,
                    user_agent: "UA".into(),
                }),
            ),
            (
                SCREENSHOT,
                BrowserOutput::Screenshot(ScreenshotOutput {
                    image: "iVBORw0KGgo=".into(),
                    format: "png".into(),
                    width: 1280,
                    height: 800,
                    scale: 1.,
                    page_generation: 3,
                    tab: "t1".into(),
                }),
            ),
            (
                READ_PAGE,
                BrowserOutput::ReadPage(ReadPageOutput {
                    tab: "t1".into(),
                    url: "http://127.0.0.1/".into(),
                    mode: "accessibility".into(),
                    nodes: vec![PageNode {
                        ref_: "e1".into(),
                        role: "checkbox".into(),
                        name: "Agree".into(),
                        value: None,
                        description: None,
                        state: Some(NodeState {
                            checked: Some(Checked::Mixed),
                            required: Some(true),
                            ..NodeState::default()
                        }),
                        box_: Some(BoxRow {
                            x: 8.,
                            y: 10.,
                            width: 13.,
                            height: 13.,
                        }),
                        depth: 0,
                    }],
                    total: 1,
                    truncated: false,
                    page_generation: 3,
                }),
            ),
            (
                FIND,
                BrowserOutput::Find(FindOutput {
                    tab: "t1".into(),
                    matches: vec![FindMatch {
                        ref_: "e2".into(),
                        role: "button".into(),
                        name: "Save".into(),
                        box_: None,
                    }],
                    total: 1,
                    truncated: false,
                    page_generation: 3,
                }),
            ),
            (
                PAGE_TEXT,
                BrowserOutput::PageText(PageTextOutput {
                    tab: "t1".into(),
                    text: "Hello".into(),
                    cursor: 0,
                    next: Some(5),
                    total_chars: 10,
                    page_generation: 3,
                }),
            ),
            (
                CONSOLE,
                BrowserOutput::Console(ConsoleOutput {
                    tab: "t1".into(),
                    messages: vec![ConsoleMessage {
                        seq: 1,
                        level: ConsoleLevel::Error,
                        source: "console".into(),
                        text: "boom".into(),
                        url: Some("http://127.0.0.1/a.js".into()),
                        line: Some(3),
                        column: Some(9),
                        stack: Some("at f (http://127.0.0.1/a.js:3:9)".into()),
                        timestamp: 1.0,
                    }],
                    next: 1,
                    dropped: 0,
                }),
            ),
            (
                NETWORK,
                BrowserOutput::Network(NetworkOutput {
                    tab: "t1".into(),
                    requests: vec![NetworkRequestRow {
                        seq: 1,
                        request_id: "9.1".into(),
                        method: "GET".into(),
                        url: "http://127.0.0.1/missing.json".into(),
                        status: Some(404),
                        resource_type: "Fetch".into(),
                        mime_type: Some("text/plain".into()),
                        started_at: 1.0,
                        duration_ms: Some(2.),
                        encoded_bytes: Some(120),
                        failed: None,
                        initiator: InitiatorRow {
                            type_: "script".into(),
                            url: Some("http://127.0.0.1/".into()),
                        },
                    }],
                    next: 1,
                    dropped: 0,
                }),
            ),
            (
                WAIT,
                BrowserOutput::Wait(WaitOutput {
                    tab: "t1".into(),
                    timeout: false,
                    satisfied: Some(WaitSatisfied {
                        for_: "selector".into(),
                        ref_: Some("e9".into()),
                        ..WaitSatisfied::default()
                    }),
                    page_generation: 3,
                    elapsed_ms: 300.,
                }),
            ),
            (
                EVALUATE,
                BrowserOutput::Evaluate(EvaluateOutput {
                    tab: "t1".into(),
                    result: json!({"a": 1}),
                    type_: "object".into(),
                    subtype: None,
                    truncated: false,
                    page_generation: 3,
                }),
            ),
        ];
        let console = vec![ConsoleError {
            text: "Uncaught Error: boom".into(),
            source: "exception".into(),
            url: Some("http://127.0.0.1/act.html".into()),
            line: Some(30),
            column: Some(7),
            stack: Some("at onclick (http://127.0.0.1/act.html:30:7)".into()),
        }];
        let actions = [
            (
                INPUT,
                BrowserOutput::Input(InputOutput {
                    tab: "t1".into(),
                    action: "drag".into(),
                    target: InputTarget {
                        ref_: Some("e4".into()),
                        role: Some("slider".into()),
                        name: Some("Volume".into()),
                        point: Some(Point { x: 10., y: 20. }),
                    },
                    to: Some(DropTarget {
                        ref_: None,
                        point: Some(Point { x: 200., y: 20. }),
                    }),
                    selected: Some(vec!["pro".into()]),
                    page_generation: 4,
                    navigated: true,
                    url: Some("http://127.0.0.1/act.html?q=x".into()),
                    console_errors: console.clone(),
                    elapsed_ms: 101.5,
                }),
            ),
            (
                FORM_INPUT,
                BrowserOutput::FormInput(FormInputOutput {
                    tab: "t1".into(),
                    fields: vec![
                        FieldResult {
                            ref_: "e1".into(),
                            ok: true,
                            message: None,
                        },
                        FieldResult {
                            ref_: "e2".into(),
                            ok: false,
                            message: Some("disabled".into()),
                        },
                    ],
                    page_generation: 4,
                    console_errors: console,
                }),
            ),
            (
                UPLOAD,
                BrowserOutput::Upload(UploadOutput {
                    tab: "t1".into(),
                    count: 2,
                    page_generation: 4,
                }),
            ),
            (
                STORAGE,
                BrowserOutput::Storage(StorageOutput {
                    tab: "t1".into(),
                    origin: "http://127.0.0.1:8080".into(),
                    action: "get".into(),
                    cookies: Some(vec![CookieRow {
                        name: "sid".into(),
                        value: "1".into(),
                        domain: "127.0.0.1".into(),
                        path: "/".into(),
                        expires: -1.,
                        http_only: false,
                        secure: false,
                        same_site: Some("Lax".into()),
                        truncated: false,
                    }]),
                    local: Some(vec![StorageItem::new("k".into(), &"v".repeat(1001))]),
                    session: Some(vec![]),
                    cleared: None,
                    total: 2,
                }),
            ),
            (
                STORAGE,
                BrowserOutput::Storage(StorageOutput {
                    tab: "t1".into(),
                    origin: "http://127.0.0.1:8080".into(),
                    action: "clear".into(),
                    cleared: Some(Cleared {
                        cookies: Some(1),
                        local: Some(2),
                        session: None,
                    }),
                    total: 3,
                    ..Default::default()
                }),
            ),
            (
                NETWORK_BODY,
                BrowserOutput::NetworkBody(NetworkBodyOutput {
                    tab: "t1".into(),
                    request_id: "9.1".into(),
                    url: "http://127.0.0.1/data.json".into(),
                    status: Some(200),
                    headers: BTreeMap::from([("Content-Type".into(), "application/json".into())]),
                    mime_type: "application/json".into(),
                    body: Some("{}".into()),
                    body_base64: None,
                    size: 2,
                    truncated: false,
                }),
            ),
            (
                OPEN_EXTERNAL,
                BrowserOutput::OpenExternal(OpenExternalOutput {
                    url: "http://127.0.0.1/".into(),
                    command: "xdg-open http://127.0.0.1/".into(),
                }),
            ),
        ];
        for (id, out) in samples.into_iter().chain(actions) {
            conforms(&spec(id).output_schema, &out.to_json(), id);
        }
        let item = StorageItem::new("k".into(), &"\u{e9}".repeat(1200));
        assert!(item.truncated);
        assert_eq!(item.value.chars().count(), STORAGE_VALUE_CHARS);
    }

    #[test]
    fn register_runs_the_target() {
        struct Echo;
        impl BrowserTarget for Echo {
            fn apply(&self, request: BrowserRequest) -> Result<BrowserOutput, CommandError> {
                match request {
                    BrowserRequest::Tabs => Ok(BrowserOutput::Tabs(TabsOutput {
                        running: false,
                        engine: EngineRow {
                            name: "external-chrome".into(),
                            ..EngineRow::default()
                        },
                        active: None,
                        tabs: vec![],
                    })),
                    other => Err(CommandError::Failed(format!(
                        "{} not here",
                        other.command()
                    ))),
                }
            }
        }
        let r = CommandRegistry::new();
        register(&r, Arc::new(Echo));
        assert_eq!(r.list().len(), ALL.len());
        assert_eq!(
            r.invoke(TABS, json!({})).unwrap(),
            json!({"running": false, "engine": {"name": "external-chrome"}, "tabs": []})
        );
        assert!(matches!(
            r.invoke(FIND, json!({})),
            Err(CommandError::InvalidInput(_))
        ));
    }
}
