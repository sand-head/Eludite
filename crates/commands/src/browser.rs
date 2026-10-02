//! The browser's commands (brief 0023, proposal 0002 section 4): `eludite.browser.tabs`, `tab_open`, `tab_close`,
//! `tab_select`, `navigate`, `resize`, `screenshot`, `read_page`, `find`, `page_text`, `console`, `network`, `wait`
//! and `evaluate`, all agent-visible, with `tab` optional (the active tab) except for `tab_select`.
//!
//! This module parses and validates their input into a [`BrowserRequest`], types their outputs
//! ([`BrowserOutput`]), and registers them against a [`BrowserTarget`]: the shell, which runs them on its `browser`
//! worker thread through `eludite-browser`. The schemas are the files in `protocol/schemas/browser-*.json`
//! (checked in first, CLAUDE.md invariant 4).
//!
//! Classes (proposal 0002 section 4): reading the page is `read`; opening, closing and selecting tabs, navigating,
//! resizing and evaluating JavaScript are `execute`. Brief 0024's browser policy makes `navigate` `dangerous`
//! outside the allowed origins.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

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

pub const ALL: [&str; 14] = [
    TABS, TAB_OPEN, TAB_CLOSE, TAB_SELECT, NAVIGATE, RESIZE, SCREENSHOT, READ_PAGE, FIND,
    PAGE_TEXT, CONSOLE, NETWORK, WAIT, EVALUATE,
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
        }
    }

    /// The tab named, if any.
    pub fn tab(&self) -> Option<&str> {
        match self {
            BrowserRequest::Tabs | BrowserRequest::TabOpen { .. } => None,
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
            | BrowserRequest::Evaluate { tab, .. } => tab.as_deref(),
        }
    }

    /// The longest the command may legitimately take, for a caller that waits on another thread: its own wait plus
    /// a launch and a margin.
    pub fn budget_ms(&self) -> u64 {
        let own = match self {
            BrowserRequest::Navigate { wait_ms, .. } | BrowserRequest::Wait { wait_ms, .. } => {
                *wait_ms
            }
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
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
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

/// Register every browser command, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn BrowserTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            TABS, SCREENSHOT, READ_PAGE, FIND, PAGE_TEXT, CONSOLE, NETWORK, WAIT,
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
        assert!(parse("eludite.browser.input", json!({})).is_err());
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
        for (id, out) in samples {
            conforms(&spec(id).output_schema, &out.to_json(), id);
        }
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
