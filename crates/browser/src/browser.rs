//! [`Browser`]: the `eludite.browser.*` commands over an [`Engine`]. Owned by one thread (the shell's `browser`
//! worker); every method may wait on the engine.
//!
//! Tabs get short ids (`t1`, `t2`, ... never reused). Attaching a tab enables `Page` (with lifecycle events),
//! `Runtime`, `Log`, `Network` and `DOM` on its session and starts a pump thread that feeds the tab's state
//! ([`crate::tab`]) from its events, so console messages and requests are recorded between commands.
//!
//! Acting on the page (`input`, `form_input`, `upload`) is in `act`, and `storage`, `network_body` and
//! `open_external` in `data` (brief 0024).
//!
//! Waiting (`navigate`'s `wait_until`, `wait`) is event-driven where CDP has the event (lifecycle events per loader,
//! requests in flight, console messages, main-frame navigations) and polls every 100 ms where it has not (a
//! selector, text, an expression). A navigation restored from the back/forward cache fires no lifecycle events, so
//! once the main frame has committed, `document.readyState` is checked as well.

mod act;
mod data;

pub use self::act::SET_JS;
pub use self::data::{OPENER_ENV, base64_decode, base64_encode, opener};

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::browser::{
    BoxRow, BrowserOutput, BrowserRequest, Clip, ConsoleOutput, EngineRow, EvaluateOutput, FindBy,
    FindMatch, FindOutput, ImageFormat, NavigateOutput, NavigateTo, NetworkOutput, PageNode,
    PageTextOutput, ReadFilter, ReadMode, ReadPageOutput, ResizeOutput, ScreenshotOutput,
    TabCloseOutput, TabOpenOutput, TabRow, TabsOutput, TextPattern, WaitFor, WaitOutput,
    WaitSatisfied, WaitUntil,
};
use eludite_protocol::cdp::{self, accessibility, dom, page, runtime};
use serde_json::{Value, json};

use crate::LogSink;
use crate::connection::DEFAULT_TIMEOUT;
use crate::engine::{Engine, EngineConfig, EngineError, TargetInfo};
use crate::page::{Row, flatten_ax, flatten_dom, image_size, quad_box};
use crate::tab::{RefError, Shared, TabState};

/// A `find` match: its backend node id, and its role and name when the search already knows them.
type FoundNode = (i64, Option<(String, String)>);

/// `read_page`'s rows read element by element, each with its box, and the total.
type CandidateRows = (Vec<(Row, Option<BoxRow>)>, usize);

/// How often waits poll what CDP has no event for.
const POLL: Duration = Duration::from_millis(100);
/// `network_idle`: no request in flight for this long.
pub const NETWORK_QUIET: Duration = Duration::from_millis(500);
/// How long one `evaluate` may run.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(30);
/// `read_page` reads the full accessibility tree of subtrees with at most this many elements, and otherwise the
/// accessibility nodes of [`CANDIDATES`] one by one ([`Browser::read_candidates`]).
const FULL_TREE_LIMIT: u64 = 1500;
/// Elements that can have an interactive role or be focusable with a name: a superset of what `interactive` keeps.
const CANDIDATES: &str = "a[href],area[href],button,input:not([type=hidden]),select,textarea,option,summary,\
    iframe,audio[controls],video[controls],[tabindex],[contenteditable]:not([contenteditable=false]),[role]";
/// On a document or element: `null` when its subtree has at most `limit` elements, else the elements matching
/// `sel` in document order (the root first when it matches), through open shadow roots.
const COLLECT_JS: &str = r#"function(sel, limit) {
  const root = this.nodeType === 9 ? this.documentElement : this;
  if (!root || root.getElementsByTagName('*').length <= limit) return null;
  const out = [];
  const visit = (el) => {
    if (el.matches(sel)) out.push(el);
    if (el.shadowRoot) for (const c of el.shadowRoot.children) visit(c);
    for (const c of el.children) visit(c);
  };
  visit(root);
  return out;
}"#;
/// On a slice of the candidate list: each element's border box (`getBoundingClientRect`, CSS pixels of the
/// viewport, as `DOM.getBoxModel` answers), or `null` when it is not rendered.
const RECTS_JS: &str = r#"function() {
  return this.map((e) => {
    if (e.getClientRects().length === 0) return null;
    const r = e.getBoundingClientRect();
    return [r.x, r.y, r.width, r.height];
  });
}"#;
/// On the candidate list: each one's nearest candidate ancestor (its index, or -1), across shadow roots.
const PARENTS_JS: &str = r#"function() {
  const ix = new Map(this.map((e, i) => [e, i]));
  return this.map((e) => {
    for (let p = e.parentElement || (e.parentNode && e.parentNode.host); p; p = p.parentElement || (p.parentNode && p.parentNode.host)) {
      if (ix.has(p)) return ix.get(p);
    }
    return -1;
  });
}"#;
const NO_TAB: &str = "no tab: the browser has no open tab; call eludite.browser.tab_open first";
/// `Network.enable`'s buffers for response bodies (`eludite.browser.network_body`): per resource and in all.
pub const RESOURCE_BUFFER: u64 = 50 * 1024 * 1024;
pub const TOTAL_BUFFER: u64 = 200 * 1024 * 1024;

#[derive(Clone)]
struct Tab {
    id: String,
    target: String,
    session: String,
    shared: Arc<Shared>,
}

impl Tab {
    fn state(&self) -> std::sync::MutexGuard<'_, TabState> {
        self.shared.lock()
    }
}

/// The browser commands' state: the engine and the tabs.
pub struct Browser {
    engine: Box<dyn Engine>,
    log: LogSink,
    tabs: Vec<Tab>,
    active: Option<String>,
    /// Tab ids, most recently selected last.
    recent: Vec<String>,
    next_tab: u64,
    /// Targets this browser closed: `Target.getTargets` may still list one for a moment after `closeTarget`
    /// answers, and it must not come back as a new tab.
    closed: std::collections::HashSet<String>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser")
            .field("engine", &self.engine.name())
            .field("tabs", &self.tabs.iter().map(|t| &t.id).collect::<Vec<_>>())
            .field("active", &self.active)
            .finish()
    }
}

fn failed(m: impl Into<String>) -> CommandError {
    CommandError::Failed(m.into())
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(m.into())
}

fn engine_err(e: EngineError) -> CommandError {
    match e {
        EngineError::NotRunning => failed(
            "the browser is not running (it exited or was closed); eludite.browser.tab_open starts it again",
        ),
        e => failed(e.to_string()),
    }
}

fn ref_err(e: RefError) -> CommandError {
    invalid(e.to_string())
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e4).round() / 10.
}

/// A compiled text pattern.
pub type Matcher = Box<dyn Fn(&str) -> bool + Send>;

/// A compiled `pattern`.
pub fn matcher(p: &TextPattern) -> Result<Matcher, CommandError> {
    Ok(match p {
        TextPattern::Substring(s) => {
            let s = s.clone();
            Box::new(move |t: &str| t.contains(&s))
        }
        TextPattern::Regex {
            source,
            case_insensitive,
        } => {
            let re = regex::RegexBuilder::new(source)
                .case_insensitive(*case_insensitive)
                .build()
                .map_err(|e| {
                    invalid(format!(
                        "the pattern /{source}/ is not a valid regular expression: {e}"
                    ))
                })?;
            Box::new(move |t: &str| re.is_match(t))
        }
    })
}

/// JavaScript truthiness of a JSON value (`undefined` arrives as `null`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0. && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

impl Browser {
    pub fn new(engine: Box<dyn Engine>, log: LogSink) -> Self {
        Self {
            engine,
            log,
            tabs: Vec::new(),
            active: None,
            recent: Vec::new(),
            next_tab: 1,
            closed: std::collections::HashSet::new(),
        }
    }

    /// The configuration of the next launch.
    pub fn configure(&mut self, config: EngineConfig) {
        self.engine.configure(config);
    }

    pub fn is_running(&self) -> bool {
        self.engine.is_running()
    }

    /// Close the browser and forget its tabs (the workspace closed, or the shell exits).
    pub fn shutdown(&mut self) {
        self.engine.shutdown();
        self.forget_tabs();
    }

    fn forget_tabs(&mut self) {
        self.tabs.clear();
        self.closed.clear();
        self.active = None;
        self.recent.clear();
    }

    /// Run one command.
    pub fn apply(&mut self, request: BrowserRequest) -> Result<BrowserOutput, CommandError> {
        // A browser that went away (crashed, killed) takes its tabs with it.
        if !self.tabs.is_empty() && !self.engine.is_running() {
            self.forget_tabs();
        }
        let out = match request {
            BrowserRequest::Tabs => BrowserOutput::Tabs(self.tabs_output()?),
            BrowserRequest::TabOpen { url } => {
                BrowserOutput::TabOpen(self.tab_open(url.as_deref())?)
            }
            BrowserRequest::TabClose { tab } => {
                BrowserOutput::TabClose(self.tab_close(tab.as_deref())?)
            }
            BrowserRequest::TabSelect { tab } => BrowserOutput::TabSelect(self.tab_select(&tab)?),
            BrowserRequest::Navigate {
                tab,
                to,
                wait_until,
                wait_ms,
            } => {
                BrowserOutput::Navigate(self.navigate(tab.as_deref(), &to, wait_until, wait_ms)?)
            }
            BrowserRequest::Resize {
                tab,
                width,
                height,
                device_scale_factor,
                mobile,
                user_agent,
            } => BrowserOutput::Resize(self.resize(
                tab.as_deref(),
                width,
                height,
                device_scale_factor,
                mobile,
                user_agent,
            )?),
            BrowserRequest::Screenshot {
                tab,
                full_page,
                clip,
                max_width,
                format,
                quality,
            } => BrowserOutput::Screenshot(self.screenshot(
                tab.as_deref(),
                full_page,
                clip,
                max_width,
                format,
                quality,
            )?),
            BrowserRequest::ReadPage {
                tab,
                mode,
                filter,
                root,
                max_nodes,
            } => BrowserOutput::ReadPage(self.read_page(
                tab.as_deref(),
                mode,
                filter,
                root.as_deref(),
                max_nodes,
            )?),
            BrowserRequest::Find { tab, by, max } => {
                BrowserOutput::Find(self.find(tab.as_deref(), &by, max)?)
            }
            BrowserRequest::PageText {
                tab,
                root,
                max_chars,
                cursor,
            } => BrowserOutput::PageText(self.page_text(
                tab.as_deref(),
                root.as_deref(),
                max_chars,
                cursor,
            )?),
            BrowserRequest::Console {
                tab,
                since,
                level,
                pattern,
                max,
            } => {
                let t = self.resolve(tab.as_deref())?;
                let keep = pattern.as_ref().map(matcher).transpose()?;
                let s = t.state();
                let page = s.console.read(since, max, |m| {
                    level.is_none_or(|l| m.level >= l) && keep.as_ref().is_none_or(|k| k(&m.text))
                });
                BrowserOutput::Console(ConsoleOutput {
                    tab: t.id.clone(),
                    messages: page.items.into_iter().map(|(_, m)| m).collect(),
                    next: page.next,
                    dropped: page.dropped,
                })
            }
            BrowserRequest::Network {
                tab,
                since,
                url_pattern,
                status,
                resource_type,
                max,
            } => {
                let t = self.resolve(tab.as_deref())?;
                let keep = url_pattern.as_ref().map(matcher).transpose()?;
                let s = t.state();
                let page = s.network.read(since, max, |e| {
                    keep.as_ref().is_none_or(|k| k(&e.row.url))
                        && status.is_none_or(|st| e.row.status == Some(st))
                        && resource_type
                            .as_ref()
                            .is_none_or(|rt| e.row.resource_type.eq_ignore_ascii_case(rt))
                });
                BrowserOutput::Network(NetworkOutput {
                    tab: t.id.clone(),
                    requests: page.items.into_iter().map(|(_, e)| e.row).collect(),
                    next: page.next,
                    dropped: page.dropped,
                })
            }
            BrowserRequest::Wait {
                tab,
                condition,
                wait_ms,
            } => BrowserOutput::Wait(self.wait(tab.as_deref(), &condition, wait_ms)?),
            BrowserRequest::Evaluate {
                tab,
                expression,
                await_promise,
                return_by_value,
                max_chars,
            } => BrowserOutput::Evaluate(self.evaluate(
                tab.as_deref(),
                &expression,
                await_promise,
                return_by_value,
                max_chars,
            )?),
            BrowserRequest::Input {
                tab,
                action,
                target,
                text,
                per_key,
                keys,
                delta,
                to,
                values,
                modifiers,
                wait_ms,
            } => BrowserOutput::Input(self.input(
                tab.as_deref(),
                action,
                target.as_ref(),
                text.as_deref(),
                per_key,
                &keys,
                delta,
                to.as_ref(),
                &values,
                modifiers,
                wait_ms,
            )?),
            BrowserRequest::FormInput { tab, fields } => {
                BrowserOutput::FormInput(self.form_input(tab.as_deref(), &fields)?)
            }
            BrowserRequest::Upload { tab, ref_, paths } => {
                BrowserOutput::Upload(self.upload(tab.as_deref(), &ref_, &paths)?)
            }
            BrowserRequest::Storage {
                tab,
                kind,
                action,
                origin,
            } => BrowserOutput::Storage(self.storage(
                tab.as_deref(),
                kind,
                action,
                origin.as_deref(),
            )?),
            BrowserRequest::NetworkBody {
                tab,
                request_id,
                max_bytes,
            } => BrowserOutput::NetworkBody(self.network_body(
                tab.as_deref(),
                &request_id,
                max_bytes,
            )?),
            BrowserRequest::OpenExternal { url } => {
                BrowserOutput::OpenExternal(self.open_external(url.as_deref())?)
            }
        };
        Ok(out)
    }

    // ---- tabs ----

    fn send(&self, tab: &Tab, method: &str, params: Value) -> Result<Value, CommandError> {
        self.engine
            .send(&tab.session, method, params, DEFAULT_TIMEOUT)
            .map_err(engine_err)
    }

    fn call<C: cdp::Command>(&self, tab: &Tab, params: C) -> Result<C::Returns, CommandError> {
        let params = serde_json::to_value(&params).map_err(|e| failed(e.to_string()))?;
        let v = self
            .engine
            .send(&tab.session, C::METHOD, params, DEFAULT_TIMEOUT)
            .map_err(engine_err)?;
        serde_json::from_value(v)
            .map_err(|e| failed(format!("unexpected answer to {}: {e}", C::METHOD)))
    }

    /// Attach to a target and start following its events.
    fn adopt(&mut self, target: &str) -> Result<Tab, CommandError> {
        if let Some(t) = self.tabs.iter().find(|t| t.target == target) {
            return Ok(t.clone());
        }
        let session = self.engine.attach(target).map_err(engine_err)?;
        let events = self.engine.subscribe(&session).map_err(engine_err)?;
        let id = format!("t{}", self.next_tab);
        self.next_tab += 1;
        let shared = Arc::new(Shared::new(&id));
        let pump_shared = shared.clone();
        let log = self.log.clone();
        std::thread::Builder::new()
            .name("browser-tab-events".into())
            .spawn(move || {
                for e in events {
                    let line = pump_shared.lock().apply(&e);
                    pump_shared.changed.notify_all();
                    if let Some(line) = line {
                        log(&line);
                    }
                }
                pump_shared.lock().gone = true;
                pump_shared.changed.notify_all();
            })
            .map_err(|e| failed(e.to_string()))?;
        let tab = Tab {
            id,
            target: target.to_owned(),
            session,
            shared,
        };
        let enables = vec![
            ("Page.enable".to_owned(), json!({})),
            (
                "Page.setLifecycleEventsEnabled".to_owned(),
                json!({"enabled": true}),
            ),
            ("Runtime.enable".to_owned(), json!({})),
            ("Log.enable".to_owned(), json!({})),
            (
                "Network.enable".to_owned(),
                json!({"maxResourceBufferSize": RESOURCE_BUFFER, "maxTotalBufferSize": TOTAL_BUFFER}),
            ),
            ("DOM.enable".to_owned(), json!({})),
            ("Page.getFrameTree".to_owned(), json!({})),
        ];
        let answers = self
            .engine
            .send_many(&tab.session, enables, DEFAULT_TIMEOUT);
        for a in &answers {
            if let Err(e) = a {
                return Err(engine_err(e.clone()));
            }
        }
        if let Some(Ok(tree)) = answers.last() {
            let frame = &tree["frameTree"]["frame"];
            let mut s = tab.state();
            if s.main_frame.is_none() {
                s.main_frame = frame["id"].as_str().map(str::to_owned);
                s.main_loader = frame["loaderId"].as_str().map(str::to_owned);
                s.url = frame["url"].as_str().unwrap_or_default().to_owned();
            }
        }
        self.tabs.push(tab.clone());
        Ok(tab)
    }

    /// Follow the browser's page targets: adopt new ones (opened by a page or by the person), drop closed ones.
    fn sync(&mut self) -> Result<Vec<TargetInfo>, CommandError> {
        let targets = self.engine.targets().map_err(engine_err)?;
        self.tabs
            .retain(|t| targets.iter().any(|x| x.target_id == t.target) && !t.state().gone);
        for t in &targets {
            if !self.tabs.iter().any(|x| x.target == t.target_id)
                && !self.closed.contains(&t.target_id)
            {
                self.adopt(&t.target_id)?;
            }
        }
        self.closed
            .retain(|c| targets.iter().any(|t| t.target_id == *c));
        let ids: Vec<String> = self.tabs.iter().map(|t| t.id.clone()).collect();
        self.recent.retain(|r| ids.contains(r));
        if self.active.as_ref().is_none_or(|a| !ids.contains(a)) {
            self.active = self.recent.last().cloned().or_else(|| ids.first().cloned());
        }
        Ok(targets)
    }

    fn resolve(&mut self, id: Option<&str>) -> Result<Tab, CommandError> {
        let pick = |b: &Self| -> Option<Tab> {
            let want = id.map(str::to_owned).or_else(|| b.active.clone())?;
            b.tabs
                .iter()
                .find(|t| t.id == want && !t.state().gone)
                .cloned()
        };
        if let Some(t) = pick(self) {
            return Ok(t);
        }
        if self.engine.is_running() {
            self.sync()?;
            if let Some(t) = pick(self) {
                return Ok(t);
            }
        }
        Err(match id {
            Some(id) if !self.tabs.is_empty() => failed(format!(
                "no tab `{id}`; eludite.browser.tabs lists the open tabs"
            )),
            _ => failed(NO_TAB),
        })
    }

    fn select(&mut self, tab: &Tab) {
        self.active = Some(tab.id.clone());
        self.recent.retain(|r| *r != tab.id);
        self.recent.push(tab.id.clone());
    }

    fn row(&self, tab: &Tab, targets: &[TargetInfo]) -> TabRow {
        let info = targets.iter().find(|t| t.target_id == tab.target);
        let s = tab.state();
        TabRow {
            id: tab.id.clone(),
            url: info.map_or_else(|| s.url.clone(), |i| i.url.clone()),
            title: info.map(|i| i.title.clone()).unwrap_or_default(),
            active: self.active.as_deref() == Some(tab.id.as_str()),
            loading: s.loading,
            page_generation: s.page_generation,
        }
    }

    fn tabs_output(&mut self) -> Result<TabsOutput, CommandError> {
        let mut engine = EngineRow {
            name: self.engine.name().to_owned(),
            ..EngineRow::default()
        };
        if !self.engine.is_running() {
            self.forget_tabs();
            return Ok(TabsOutput {
                running: false,
                engine,
                active: None,
                tabs: Vec::new(),
            });
        }
        let targets = self.sync()?;
        if let Some(info) = self.engine.info() {
            engine.version = Some(info.version);
            engine.executable = Some(info.executable);
        }
        Ok(TabsOutput {
            running: true,
            engine,
            active: self.active.clone(),
            tabs: self.tabs.iter().map(|t| self.row(t, &targets)).collect(),
        })
    }

    fn tab_open(&mut self, url: Option<&str>) -> Result<TabOpenOutput, CommandError> {
        let launched = self.engine.launch().map_err(engine_err)?.is_some();
        if launched {
            self.forget_tabs();
        }
        // A fresh browser opens one blank tab: use it rather than leave it beside the new one.
        let reuse = if launched {
            let targets = self.engine.targets().map_err(engine_err)?;
            match targets.as_slice() {
                [only] if only.url == "about:blank" => Some(only.target_id.clone()),
                _ => None,
            }
        } else {
            None
        };
        let target = match reuse {
            Some(t) => t,
            None => self.engine.open_tab("about:blank").map_err(engine_err)?,
        };
        let tab = self.adopt(&target)?;
        let _ = self.engine.activate_tab(&target);
        self.select(&tab);
        (self.log)(&format!(
            "Opened tab {}{}",
            tab.id,
            url.map(|u| format!(": {u}")).unwrap_or_default()
        ));
        let mut status = None;
        if let Some(url) = url {
            let nav = self.navigate_tab(
                &tab,
                &NavigateTo::Url(url.to_owned()),
                WaitUntil::Load,
                30_000,
            )?;
            status = nav.status;
        }
        let targets = self.engine.targets().map_err(engine_err)?;
        Ok(TabOpenOutput {
            tab: self.row(&tab, &targets),
            launched,
            status,
        })
    }

    fn tab_close(&mut self, id: Option<&str>) -> Result<TabCloseOutput, CommandError> {
        let tab = self.resolve(id)?;
        self.engine.close_tab(&tab.target).map_err(engine_err)?;
        self.closed.insert(tab.target.clone());
        self.tabs.retain(|t| t.id != tab.id);
        self.recent.retain(|r| *r != tab.id);
        if self.active.as_deref() == Some(tab.id.as_str()) {
            self.active = self
                .recent
                .last()
                .cloned()
                .or_else(|| self.tabs.last().map(|t| t.id.clone()));
            if let Some(next) = self
                .active
                .clone()
                .and_then(|a| self.tabs.iter().find(|t| t.id == a).cloned())
            {
                let _ = self.engine.activate_tab(&next.target);
            }
        }
        (self.log)(&format!("Closed tab {}", tab.id));
        Ok(TabCloseOutput {
            closed: tab.id,
            active: self.active.clone(),
            tabs: self.tabs.len(),
        })
    }

    fn tab_select(&mut self, id: &str) -> Result<TabRow, CommandError> {
        let tab = self.resolve(Some(id))?;
        self.engine.activate_tab(&tab.target).map_err(engine_err)?;
        self.select(&tab);
        let targets = self.engine.targets().map_err(engine_err)?;
        Ok(self.row(&tab, &targets))
    }

    // ---- navigation ----

    fn navigate(
        &mut self,
        id: Option<&str>,
        to: &NavigateTo,
        wait_until: WaitUntil,
        wait_ms: u64,
    ) -> Result<NavigateOutput, CommandError> {
        let tab = match self.resolve(id) {
            Ok(t) => t,
            // With no tab at all, a url opens one (launching the browser when needed).
            Err(_) if id.is_none() && matches!(to, NavigateTo::Url(_)) && self.tabs.is_empty() => {
                self.tab_open(None)?;
                self.resolve(None)?
            }
            Err(e) => return Err(e),
        };
        self.navigate_tab(&tab, to, wait_until, wait_ms)
    }

    fn navigate_tab(
        &mut self,
        tab: &Tab,
        to: &NavigateTo,
        wait_until: WaitUntil,
        wait_ms: u64,
    ) -> Result<NavigateOutput, CommandError> {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(wait_ms);
        let (seq0, nav0) = {
            let s = tab.state();
            (s.console.last_seq(), s.navigations)
        };
        let mut same_document = false;
        let loader = match to {
            NavigateTo::Url(url) => {
                let r = self.call(
                    tab,
                    page::NavigateParams {
                        url: url.clone(),
                        ..Default::default()
                    },
                )?;
                if let Some(err) = r.error_text.filter(|e| !e.is_empty()) {
                    (self.log)(&format!("[{}] Navigation to {url} failed: {err}", tab.id));
                    return Err(failed(format!("navigation to {url} failed: {err}")));
                }
                same_document = r.loader_id.is_none();
                r.loader_id
            }
            NavigateTo::Back | NavigateTo::Forward => {
                let h = self.call(tab, page::GetNavigationHistoryParams {})?;
                let delta: i64 = if matches!(to, NavigateTo::Back) {
                    -1
                } else {
                    1
                };
                let ix = h.current_index + delta;
                let entry = usize::try_from(ix)
                    .ok()
                    .and_then(|i| h.entries.get(i))
                    .ok_or_else(|| {
                        failed(format!(
                            "there is no page to go {} to",
                            if delta < 0 { "back" } else { "forward" }
                        ))
                    })?;
                self.call(
                    tab,
                    page::NavigateToHistoryEntryParams { entry_id: entry.id },
                )?;
                None
            }
            NavigateTo::Reload => {
                self.call(tab, page::ReloadParams::default())?;
                None
            }
        };
        let reached = same_document
            || self.wait_loaded(tab, wait_until, loader.as_deref(), nav0, deadline)?;
        let here = self
            .eval_value(
                tab,
                "({u: location.href, t: document.title})",
                Duration::from_secs(5),
            )
            .unwrap_or(Value::Null);
        let s = tab.state();
        let doc_loader = loader.clone().or_else(|| {
            (s.navigations > nav0)
                .then(|| s.main_loader.clone())
                .flatten()
        });
        Ok(NavigateOutput {
            tab: tab.id.clone(),
            url: here["u"]
                .as_str()
                .map_or_else(|| s.url.clone(), str::to_owned),
            title: here["t"].as_str().unwrap_or_default().to_owned(),
            status: doc_loader.and_then(|l| s.status_of(&l)),
            page_generation: s.page_generation,
            console_errors: s.console_errors_since(seq0),
            timed_out: !reached,
            elapsed_ms: ms(started.elapsed()),
        })
    }

    /// Wait until the navigation (of `loader`, or the next main-frame navigation after `nav0`) reaches `until`.
    fn wait_loaded(
        &self,
        tab: &Tab,
        until: WaitUntil,
        loader: Option<&str>,
        nav0: u64,
        deadline: Instant,
    ) -> Result<bool, CommandError> {
        if until == WaitUntil::None {
            return Ok(true);
        }
        let mut tick = 0u32;
        loop {
            let (done, committed) = {
                let s = tab.state();
                if s.gone {
                    return Err(failed("the tab closed while loading"));
                }
                let l = loader.map(str::to_owned).or_else(|| {
                    (s.navigations > nav0)
                        .then(|| s.main_loader.clone())
                        .flatten()
                });
                match l {
                    Some(l) => {
                        let lc = s.lifecycle_of(&l);
                        let done = match until {
                            WaitUntil::Domcontentloaded => lc.dom_content_loaded || lc.load,
                            _ => lc.load,
                        };
                        (done, s.main_loader.as_deref() == Some(l.as_str()))
                    }
                    None => (false, false),
                }
            };
            // Restored from the back/forward cache: committed, but no lifecycle events. Ask the document.
            let done = done
                || (committed && tick % 3 == 2 && {
                    let state = self
                        .eval_value(tab, "document.readyState", Duration::from_secs(2))
                        .unwrap_or(Value::Null);
                    match until {
                        WaitUntil::Domcontentloaded => {
                            state == "interactive" || state == "complete"
                        }
                        _ => state == "complete",
                    }
                });
            if done {
                break;
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            self.wait_change(tab, deadline);
            tick += 1;
        }
        if until == WaitUntil::NetworkIdle {
            return Ok(self.wait_network_idle(tab, deadline));
        }
        Ok(true)
    }

    /// Wait for the tab's next event, at most one poll interval and never past `deadline`.
    fn wait_change(&self, tab: &Tab, deadline: Instant) {
        let left = deadline.saturating_duration_since(Instant::now()).min(POLL);
        let s = tab.state();
        let _ = tab.shared.changed.wait_timeout(s, left);
    }

    fn wait_network_idle(&self, tab: &Tab, deadline: Instant) -> bool {
        loop {
            {
                let s = tab.state();
                if s.in_flight.is_empty() && s.last_network_activity.elapsed() >= NETWORK_QUIET {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            self.wait_change(tab, deadline);
        }
    }

    fn resize(
        &mut self,
        id: Option<&str>,
        width: u32,
        height: u32,
        device_scale_factor: Option<f64>,
        mobile: bool,
        user_agent: Option<String>,
    ) -> Result<ResizeOutput, CommandError> {
        let tab = self.resolve(id)?;
        self.send(
            &tab,
            "Emulation.setDeviceMetricsOverride",
            json!({"width": width, "height": height, "deviceScaleFactor": device_scale_factor.unwrap_or(0.), "mobile": mobile}),
        )?;
        if let Some(ua) = &user_agent {
            self.send(
                &tab,
                "Emulation.setUserAgentOverride",
                json!({"userAgent": ua}),
            )?;
        }
        let v = self.eval_value(
            &tab,
            "({w: innerWidth, h: innerHeight, d: devicePixelRatio, ua: navigator.userAgent})",
            DEFAULT_TIMEOUT,
        )?;
        Ok(ResizeOutput {
            tab: tab.id.clone(),
            width: v["w"].as_u64().unwrap_or(u64::from(width)) as u32,
            height: v["h"].as_u64().unwrap_or(u64::from(height)) as u32,
            device_scale_factor: v["d"].as_f64().unwrap_or(1.),
            mobile,
            user_agent: v["ua"].as_str().unwrap_or_default().to_owned(),
        })
    }

    // ---- reading ----

    fn backend_of(&self, tab: &Tab, r: &str) -> Result<i64, CommandError> {
        let s = tab.state();
        s.refs.resolve(r, s.page_generation).map_err(ref_err)
    }

    fn screenshot(
        &mut self,
        id: Option<&str>,
        full_page: bool,
        clip: Option<Clip>,
        max_width: u32,
        format: ImageFormat,
        quality: Option<u8>,
    ) -> Result<ScreenshotOutput, CommandError> {
        let tab = self.resolve(id)?;
        if let Some(Clip::Ref(r)) = &clip {
            let backend = self.backend_of(&tab, r)?;
            let _ = self.send(
                &tab,
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": backend}),
            );
        }
        let m = self.call(&tab, page::GetLayoutMetricsParams {})?;
        let dpr = self
            .eval_value(&tab, "devicePixelRatio", DEFAULT_TIMEOUT)?
            .as_f64()
            .filter(|d| *d > 0.)
            .unwrap_or(1.);
        let vv = &m.css_visual_viewport;
        // The region in CSS pixels of the document.
        let (x, y, w, h) = match &clip {
            _ if full_page => (0., 0., m.css_content_size.width, m.css_content_size.height),
            Some(Clip::Ref(r)) => {
                let backend = self.backend_of(&tab, r)?;
                let b = self
                    .boxes(&tab, &[backend])
                    .pop()
                    .flatten()
                    .ok_or_else(|| {
                        failed(format!("the element {r} has no box (it is not rendered)"))
                    })?;
                (
                    b.x + vv.page_x,
                    b.y + vv.page_y,
                    b.width.max(1.),
                    b.height.max(1.),
                )
            }
            Some(Clip::Rect(r)) => (r.x + vv.page_x, r.y + vv.page_y, r.width, r.height),
            None => (vv.page_x, vv.page_y, vv.client_width, vv.client_height),
        };
        let scale = (f64::from(max_width) / (w * dpr)).min(1.);
        let mut params = json!({
            "format": format.as_str(),
            "clip": {"x": x, "y": y, "width": w, "height": h, "scale": scale},
            "captureBeyondViewport": full_page,
            "optimizeForSpeed": true,
        });
        if let Some(q) = quality {
            params["quality"] = json!(q);
        }
        let image = self
            .engine
            .screenshot(&tab.session, params, DEFAULT_TIMEOUT)
            .map_err(engine_err)?;
        let (iw, ih) =
            image_size(&image).unwrap_or(((w * dpr * scale) as u32, (h * dpr * scale) as u32));
        Ok(ScreenshotOutput {
            image,
            format: format.as_str().to_owned(),
            width: iw,
            height: ih,
            scale: (f64::from(iw) / w * 1000.).round() / 1000.,
            page_generation: tab.state().page_generation,
            tab: tab.id.clone(),
        })
    }

    /// Border boxes of backend nodes (pipelined); `None` for a node without one.
    fn boxes(&self, tab: &Tab, backends: &[i64]) -> Vec<Option<BoxRow>> {
        let calls = backends
            .iter()
            .map(|b| ("DOM.getBoxModel".to_owned(), json!({"backendNodeId": b})))
            .collect();
        self.engine
            .send_many(&tab.session, calls, DEFAULT_TIMEOUT)
            .into_iter()
            .map(|r| {
                let v = r.ok()?;
                let model: dom::GetBoxModelReturns = serde_json::from_value(v).ok()?;
                quad_box(&model.model.border)
            })
            .collect()
    }

    fn read_page(
        &mut self,
        id: Option<&str>,
        mode: ReadMode,
        filter: ReadFilter,
        root: Option<&str>,
        max_nodes: usize,
    ) -> Result<ReadPageOutput, CommandError> {
        let tab = self.resolve(id)?;
        let root_backend = root.map(|r| self.backend_of(&tab, r)).transpose()?;
        let generation = tab.state().page_generation;
        let mut counted = None;
        let mut known_boxes = None;
        let rows: Vec<Row> = match mode {
            ReadMode::Accessibility => match filter {
                ReadFilter::Interactive => {
                    match self.read_candidates(&tab, root_backend, max_nodes)? {
                        Some((rows, total)) => {
                            counted = Some(total);
                            let (rows, boxes): (Vec<Row>, Vec<Option<BoxRow>>) =
                                rows.into_iter().unzip();
                            known_boxes = Some(boxes);
                            rows
                        }
                        None => self.full_tree_rows(&tab, filter, root_backend)?,
                    }
                }
                ReadFilter::All => self.full_tree_rows(&tab, filter, root_backend)?,
            },
            ReadMode::Dom => {
                let node = match root_backend {
                    Some(b) => self.send(
                        &tab,
                        "DOM.describeNode",
                        json!({"backendNodeId": b, "depth": -1}),
                    )?["node"]
                        .take(),
                    None => {
                        self.send(&tab, "DOM.getDocument", json!({"depth": -1}))?["root"].take()
                    }
                };
                flatten_dom(&node, filter)
            }
        };
        let total = counted.unwrap_or(rows.len());
        let rows: Vec<Row> = rows.into_iter().take(max_nodes).collect();
        let boxes = match known_boxes {
            Some(b) => b,
            None => self.boxes(
                &tab,
                &rows.iter().map(|r| r.backend_node_id).collect::<Vec<_>>(),
            ),
        };
        let mut s = tab.state();
        if s.page_generation != generation {
            return Err(failed(format!(
                "the page changed while it was read (generation {} became {}); call read_page again",
                generation, s.page_generation
            )));
        }
        let nodes = rows
            .into_iter()
            .zip(boxes)
            .map(|(r, b)| PageNode {
                ref_: s.refs.issue(r.backend_node_id, generation),
                role: r.role,
                name: r.name,
                value: r.value,
                description: r.description,
                state: (!r.state.is_empty()).then_some(r.state),
                box_: b,
                depth: r.depth,
            })
            .collect::<Vec<_>>();
        let url = s.url.clone();
        drop(s);
        Ok(ReadPageOutput {
            tab: tab.id.clone(),
            url,
            mode: mode.as_str().to_owned(),
            truncated: total > nodes.len(),
            nodes,
            total,
            page_generation: generation,
        })
    }

    fn full_tree_rows(
        &self,
        tab: &Tab,
        filter: ReadFilter,
        root_backend: Option<i64>,
    ) -> Result<Vec<Row>, CommandError> {
        let tree = self.call(tab, accessibility::GetFullAXTreeParams::default())?;
        flatten_ax(&tree.nodes, filter, root_backend).map_err(failed)
    }

    /// `read_page`'s `interactive` rows of a large page without `Accessibility.getFullAXTree`, which Chrome answers
    /// at about 60 microseconds per accessibility node (two seconds for the 5,000-link fixture's 30,000 nodes).
    /// The page collects the elements that can be interactive ([`CANDIDATES`], in document order, through open
    /// shadow roots) and their nearest candidate ancestors; their accessibility nodes are read one by one
    /// (`Accessibility.getPartialAXTree`, pipelined, in batches) until `max_nodes` rows are kept, and the same rule
    /// as the full tree's filter keeps them. `None` when the subtree has at most [`FULL_TREE_LIMIT`] elements: the
    /// full tree is then cheap and exact. The total counts the candidates not examined as rows (an upper bound
    /// when the listing is truncated).
    fn read_candidates(
        &self,
        tab: &Tab,
        root_backend: Option<i64>,
        max_nodes: usize,
    ) -> Result<Option<CandidateRows>, CommandError> {
        const GROUP: &str = "eludite-read";
        let root = match root_backend {
            Some(b) => self.send(
                tab,
                "DOM.resolveNode",
                json!({"backendNodeId": b, "objectGroup": GROUP}),
            )?,
            None => self.send(
                tab,
                "Runtime.evaluate",
                json!({"expression": "document", "objectGroup": GROUP}),
            )?,
        };
        let result = (|| {
            let root = root
                .pointer("/object/objectId")
                .or_else(|| root.pointer("/result/objectId"))
                .and_then(Value::as_str)
                .ok_or_else(|| failed("the page's root has no object"))?;
            let list = self.send(
                tab,
                "Runtime.callFunctionOn",
                json!({"objectId": root, "functionDeclaration": COLLECT_JS, "objectGroup": GROUP,
                    "arguments": [{"value": CANDIDATES}, {"value": FULL_TREE_LIMIT}]}),
            )?;
            if let Some(e) = list.get("exceptionDetails") {
                return Err(failed(exception_text(e)));
            }
            let Some(list) = list.pointer("/result/objectId").and_then(Value::as_str) else {
                return Ok(None);
            };
            let parents = self.send(
                tab,
                "Runtime.callFunctionOn",
                json!({"objectId": list, "functionDeclaration": PARENTS_JS, "returnByValue": true}),
            )?;
            let parents: Vec<i64> = parents
                .pointer("/result/value")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect())
                .unwrap_or_default();
            let count = parents.len();
            let mut kept: Vec<bool> = Vec::new();
            let mut rows: Vec<(usize, Row, Option<BoxRow>)> = Vec::new();
            let mut overflow = 0;
            while kept.len() < count && rows.len() < max_nodes {
                let from = kept.len();
                // A few more than needed: some candidates are not interactive (a [role=presentation], a hidden input).
                let to = count.min(from + (max_nodes - rows.len()).max(16) + 8);
                let slice = self.send(
                    tab,
                    "Runtime.callFunctionOn",
                    json!({"objectId": list, "functionDeclaration": "function(a, b) { return this.slice(a, b); }",
                        "objectGroup": GROUP, "arguments": [{"value": from}, {"value": to}]}),
                )?;
                let slice = slice
                    .pointer("/result/objectId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| failed("the page's candidate list went away"))?;
                // Boxes in the same CSS pixels as DOM.getBoxModel's border quads, one call for the batch.
                let rects = self.send(
                    tab,
                    "Runtime.callFunctionOn",
                    json!({"objectId": slice, "functionDeclaration": RECTS_JS, "returnByValue": true}),
                )?;
                let rects: Vec<Option<BoxRow>> = rects
                    .pointer("/result/value")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(rect_box).collect())
                    .unwrap_or_default();
                let props = self.send(
                    tab,
                    "Runtime.getProperties",
                    json!({"objectId": slice, "ownProperties": true}),
                )?;
                let mut objects: Vec<(usize, String)> = props["result"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| {
                        let ix: usize = p["name"].as_str()?.parse().ok()?;
                        Some((ix, p.pointer("/value/objectId")?.as_str()?.to_owned()))
                    })
                    .collect();
                objects.sort_by_key(|(ix, _)| *ix);
                let calls = objects
                    .iter()
                    .map(|(_, o)| {
                        (
                            accessibility::GetPartialAXTreeParams::METHOD.to_owned(),
                            json!({"objectId": o, "fetchRelatives": false}),
                        )
                    })
                    .collect();
                let answers = self.engine.send_many(&tab.session, calls, DEFAULT_TIMEOUT);
                let mut batch: Vec<Option<Row>> = vec![None; to - from];
                for ((ix, _), a) in objects.iter().zip(answers) {
                    let a = a.map_err(engine_err)?;
                    let tree: accessibility::GetPartialAXTreeReturns = serde_json::from_value(a)
                        .map_err(|e| {
                            failed(format!(
                                "unexpected answer to Accessibility.getPartialAXTree: {e}"
                            ))
                        })?;
                    if let Some(slot) = batch.get_mut(*ix) {
                        *slot = tree.nodes.first().and_then(crate::page::interactive_row);
                    }
                }
                for (i, row) in batch.into_iter().enumerate() {
                    match row {
                        Some(row) if rows.len() < max_nodes => {
                            rows.push((kept.len(), row, rects.get(i).cloned().flatten()));
                            kept.push(true);
                        }
                        Some(_) => {
                            overflow += 1;
                            kept.push(false);
                        }
                        None => kept.push(false),
                    }
                }
            }
            let depths = crate::page::candidate_depths(&parents, &kept);
            let unexamined = count - kept.len();
            let total = rows.len() + overflow + unexamined;
            Ok(Some((
                rows.into_iter()
                    .map(|(i, mut r, b)| {
                        r.depth = depths[i];
                        (r, b)
                    })
                    .collect(),
                total,
            )))
        })();
        let _ = self.send(
            tab,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": GROUP}),
        );
        result
    }

    /// Backend node ids of nodes known by `nodeId` (pipelined `DOM.describeNode`), with text nodes replaced by
    /// their parent elements.
    fn backends_of_node_ids(&self, tab: &Tab, ids: &[i64]) -> Vec<i64> {
        let calls = ids
            .iter()
            .map(|n| ("DOM.describeNode".to_owned(), json!({"nodeId": n})))
            .collect();
        let described = self.engine.send_many(&tab.session, calls, DEFAULT_TIMEOUT);
        let mut out = Vec::new();
        let mut text_nodes = Vec::new();
        for (i, d) in described.into_iter().enumerate() {
            let Ok(d) = d else { continue };
            match d["node"]["nodeType"].as_i64() {
                Some(1) => out.push((i, d["node"]["backendNodeId"].as_i64().unwrap_or_default())),
                Some(3) => text_nodes.push((i, ids[i])),
                _ => {}
            }
        }
        // A text match answers its element: resolve the text node, take its parentElement, describe that.
        let resolved = self.engine.send_many(
            &tab.session,
            text_nodes
                .iter()
                .map(|(_, n)| {
                    (
                        "DOM.resolveNode".to_owned(),
                        json!({"nodeId": n, "objectGroup": "eludite-find"}),
                    )
                })
                .collect(),
            DEFAULT_TIMEOUT,
        );
        let parents = self.engine.send_many(
            &tab.session,
            resolved
                .iter()
                .map(|r| {
                    let object = r.as_ref().ok().and_then(|v| v["object"]["objectId"].as_str()).unwrap_or_default();
                    (
                        "Runtime.callFunctionOn".to_owned(),
                        json!({"objectId": object, "functionDeclaration": "function() { return this.parentElement; }", "objectGroup": "eludite-find"}),
                    )
                })
                .collect(),
            DEFAULT_TIMEOUT,
        );
        let described = self.engine.send_many(
            &tab.session,
            parents
                .iter()
                .map(|r| {
                    let object = r
                        .as_ref()
                        .ok()
                        .and_then(|v| v["result"]["objectId"].as_str())
                        .unwrap_or_default();
                    ("DOM.describeNode".to_owned(), json!({"objectId": object}))
                })
                .collect(),
            DEFAULT_TIMEOUT,
        );
        for ((i, _), d) in text_nodes.iter().zip(described) {
            if let Some(b) = d.ok().and_then(|d| d["node"]["backendNodeId"].as_i64()) {
                out.push((*i, b));
            }
        }
        let _ = self.send(
            tab,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": "eludite-find"}),
        );
        out.sort_by_key(|(i, _)| *i);
        let mut seen = std::collections::HashSet::new();
        out.into_iter()
            .map(|(_, b)| b)
            .filter(|b| seen.insert(*b))
            .collect()
    }

    fn find(
        &mut self,
        id: Option<&str>,
        by: &FindBy,
        max: usize,
    ) -> Result<FindOutput, CommandError> {
        let tab = self.resolve(id)?;
        let generation = tab.state().page_generation;
        // (backend node id, role, name), in document order.
        let (matches, total): (Vec<FoundNode>, usize) = match by {
            FindBy::Css(css) => {
                let doc = self.send(&tab, "DOM.getDocument", json!({"depth": 0}))?;
                let root = doc["root"]["nodeId"].as_i64().unwrap_or_default();
                let found = self
                    .send(
                        &tab,
                        "DOM.querySelectorAll",
                        json!({"nodeId": root, "selector": css}),
                    )
                    .map_err(|e| invalid(format!("the selector `{css}` did not run: {e}")))?;
                let ids: Vec<i64> = found["nodeIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .collect();
                let total = ids.len();
                let ids: Vec<i64> = ids.into_iter().take(max).collect();
                (
                    self.backends_of_node_ids(&tab, &ids)
                        .into_iter()
                        .map(|b| (b, None))
                        .collect(),
                    total,
                )
            }
            FindBy::Text(text) => {
                self.send(&tab, "DOM.getDocument", json!({"depth": 0}))?;
                let search = self.send(&tab, "DOM.performSearch", json!({"query": text}))?;
                let search_id = search["searchId"].as_str().unwrap_or_default().to_owned();
                let count = search["resultCount"].as_u64().unwrap_or(0) as usize;
                // Text nodes and their elements may both match: take more than `max` before merging.
                let take = count.min(max.saturating_mul(4).max(max));
                let ids: Vec<i64> = if take == 0 {
                    Vec::new()
                } else {
                    self.send(
                        &tab,
                        "DOM.getSearchResults",
                        json!({"searchId": search_id, "fromIndex": 0, "toIndex": take}),
                    )?["nodeIds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                        .collect()
                };
                let _ = self.send(
                    &tab,
                    "DOM.discardSearchResults",
                    json!({"searchId": search_id}),
                );
                let backends = self.backends_of_node_ids(&tab, &ids);
                let total = backends.len().max(count.min(backends.len()));
                (
                    backends.into_iter().take(max).map(|b| (b, None)).collect(),
                    total,
                )
            }
            FindBy::Role { role, name } => {
                let tree = self.call(&tab, accessibility::GetFullAXTreeParams::default())?;
                let rows = flatten_ax(&tree.nodes, ReadFilter::All, None).map_err(failed)?;
                let want_name = name.as_ref().map(|n| n.to_lowercase());
                let hits: Vec<Row> = rows
                    .into_iter()
                    .filter(|r| r.role.eq_ignore_ascii_case(role))
                    .filter(|r| {
                        want_name
                            .as_ref()
                            .is_none_or(|n| r.name.to_lowercase().contains(n.as_str()))
                    })
                    .collect();
                let total = hits.len();
                (
                    hits.into_iter()
                        .take(max)
                        .map(|r| (r.backend_node_id, Some((r.role, r.name))))
                        .collect(),
                    total,
                )
            }
        };
        // Role and name of css and text matches from their accessibility nodes (pipelined).
        let missing: Vec<i64> = matches
            .iter()
            .filter(|(_, rn)| rn.is_none())
            .map(|(b, _)| *b)
            .collect();
        let infos = self.engine.send_many(
            &tab.session,
            missing
                .iter()
                .map(|b| {
                    (
                        "Accessibility.getPartialAXTree".to_owned(),
                        json!({"backendNodeId": b, "fetchRelatives": false}),
                    )
                })
                .collect(),
            DEFAULT_TIMEOUT,
        );
        let mut info_by_backend = std::collections::HashMap::new();
        for (b, r) in missing.iter().zip(infos) {
            let node = r.ok().map(|v| v["nodes"][0].clone()).unwrap_or(Value::Null);
            let role = node["role"]["value"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let name = crate::page::collapse(node["name"]["value"].as_str().unwrap_or_default());
            info_by_backend.insert(*b, (role, name));
        }
        let fallback_tags = if info_by_backend
            .values()
            .any(|(r, _)| r.is_empty() || r == "none" || r == "generic")
        {
            let calls = missing
                .iter()
                .map(|b| ("DOM.describeNode".to_owned(), json!({"backendNodeId": b})))
                .collect();
            missing
                .iter()
                .zip(self.engine.send_many(&tab.session, calls, DEFAULT_TIMEOUT))
                .filter_map(|(b, r)| Some((*b, r.ok()?["node"]["localName"].as_str()?.to_owned())))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        let boxes = self.boxes(&tab, &matches.iter().map(|(b, _)| *b).collect::<Vec<_>>());
        let mut s = tab.state();
        if s.page_generation != generation {
            return Err(failed("the page changed during find; call find again"));
        }
        let out: Vec<FindMatch> = matches
            .into_iter()
            .zip(boxes)
            .map(|((b, rn), bx)| {
                let (role, name) = rn.unwrap_or_else(|| {
                    let (role, name) = info_by_backend.get(&b).cloned().unwrap_or_default();
                    let role = if role.is_empty() || role == "none" || role == "generic" {
                        fallback_tags.get(&b).cloned().unwrap_or(role)
                    } else {
                        role
                    };
                    (role, name)
                });
                FindMatch {
                    ref_: s.refs.issue(b, generation),
                    role,
                    name,
                    box_: bx,
                }
            })
            .collect();
        drop(s);
        Ok(FindOutput {
            tab: tab.id.clone(),
            truncated: total > out.len(),
            total: total.max(out.len()),
            matches: out,
            page_generation: generation,
        })
    }

    fn page_text(
        &mut self,
        id: Option<&str>,
        root: Option<&str>,
        max_chars: usize,
        cursor: usize,
    ) -> Result<PageTextOutput, CommandError> {
        let tab = self.resolve(id)?;
        let text = match root {
            Some(r) => {
                let backend = self.backend_of(&tab, r)?;
                let obj = self.send(&tab, "DOM.resolveNode", json!({"backendNodeId": backend, "objectGroup": "eludite-text"}))?;
                let object = obj["object"]["objectId"].as_str().unwrap_or_default().to_owned();
                let v = self.send(
                    &tab,
                    "Runtime.callFunctionOn",
                    json!({"objectId": object, "returnByValue": true,
                        "functionDeclaration": "function() { return this.innerText ?? this.textContent ?? ''; }"}),
                )?;
                let _ = self.send(&tab, "Runtime.releaseObjectGroup", json!({"objectGroup": "eludite-text"}));
                v["result"]["value"].as_str().unwrap_or_default().to_owned()
            }
            None => self
                .eval_value(
                    &tab,
                    "document.body ? document.body.innerText : (document.documentElement ? document.documentElement.innerText || '' : '')",
                    DEFAULT_TIMEOUT,
                )?
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        };
        let total = text.chars().count();
        let page: String = text.chars().skip(cursor).take(max_chars).collect();
        let end = cursor.saturating_add(page.chars().count());
        Ok(PageTextOutput {
            tab: tab.id.clone(),
            text: page,
            cursor,
            next: (end < total).then_some(end),
            total_chars: total,
            page_generation: tab.state().page_generation,
        })
    }

    /// Evaluate `expression` by value; an exception fails.
    fn eval_value(
        &self,
        tab: &Tab,
        expression: &str,
        timeout: Duration,
    ) -> Result<Value, CommandError> {
        let v = self
            .engine
            .send(
                &tab.session,
                runtime::EvaluateParams::METHOD,
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
                timeout,
            )
            .map_err(engine_err)?;
        if let Some(e) = v.get("exceptionDetails") {
            return Err(failed(exception_text(e)));
        }
        Ok(v["result"]["value"].clone())
    }

    fn wait(
        &mut self,
        id: Option<&str>,
        condition: &WaitFor,
        wait_ms: u64,
    ) -> Result<WaitOutput, CommandError> {
        let tab = self.resolve(id)?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(wait_ms);
        let (seq0, nav0) = {
            let s = tab.state();
            (s.console.last_seq(), s.navigations)
        };
        let console_match = match condition {
            WaitFor::Console(p) => Some(matcher(p)?),
            _ => None,
        };
        let mut satisfied: Option<WaitSatisfied> = None;
        loop {
            let found = match condition {
                WaitFor::Selector(css) => {
                    let present = self
                        .eval_value(
                            &tab,
                            &format!("!!document.querySelector({})", json!(css)),
                            DEFAULT_TIMEOUT,
                        )
                        .map_err(|e| invalid(format!("the selector `{css}` did not run: {e}")))?;
                    if truthy(&present) {
                        let doc = self.send(&tab, "DOM.getDocument", json!({"depth": 0}))?;
                        let root = doc["root"]["nodeId"].as_i64().unwrap_or_default();
                        let node = self.send(
                            &tab,
                            "DOM.querySelector",
                            json!({"nodeId": root, "selector": css}),
                        )?;
                        let backend = self
                            .backends_of_node_ids(
                                &tab,
                                &[node["nodeId"].as_i64().unwrap_or_default()],
                            )
                            .first()
                            .copied();
                        let mut s = tab.state();
                        let generation = s.page_generation;
                        Some(WaitSatisfied {
                            for_: "selector".into(),
                            ref_: backend.map(|b| s.refs.issue(b, generation)),
                            ..WaitSatisfied::default()
                        })
                    } else {
                        None
                    }
                }
                WaitFor::Text(text) => {
                    let has = self.eval_value(
                        &tab,
                        &format!(
                            "((document.body && document.body.innerText) || '').includes({})",
                            json!(text)
                        ),
                        DEFAULT_TIMEOUT,
                    )?;
                    truthy(&has).then(|| WaitSatisfied {
                        for_: "text".into(),
                        ..WaitSatisfied::default()
                    })
                }
                WaitFor::Function(expr) => {
                    let v = self.eval_value(&tab, expr, DEFAULT_TIMEOUT)?;
                    truthy(&v).then(|| WaitSatisfied {
                        for_: "function".into(),
                        value: Some(v),
                        ..WaitSatisfied::default()
                    })
                }
                WaitFor::Navigation => {
                    let navigated = tab.state().navigations > nav0;
                    if navigated && self.wait_loaded(&tab, WaitUntil::Load, None, nav0, deadline)? {
                        Some(WaitSatisfied {
                            for_: "navigation".into(),
                            url: Some(tab.state().url.clone()),
                            ..WaitSatisfied::default()
                        })
                    } else {
                        None
                    }
                }
                WaitFor::NetworkIdle => {
                    let s = tab.state();
                    (s.in_flight.is_empty() && s.last_network_activity.elapsed() >= NETWORK_QUIET)
                        .then(|| WaitSatisfied {
                            for_: "network_idle".into(),
                            ..WaitSatisfied::default()
                        })
                }
                WaitFor::Console(_) => {
                    let keep = console_match.as_ref().expect("compiled above");
                    let s = tab.state();
                    s.console
                        .since(seq0)
                        .find(|(_, m)| keep(&m.text))
                        .map(|(seq, m)| WaitSatisfied {
                            for_: "console".into(),
                            message: Some(m.text.clone()),
                            seq: Some(seq),
                            ..WaitSatisfied::default()
                        })
                }
            };
            if found.is_some() {
                satisfied = found;
                break;
            }
            if Instant::now() >= deadline || tab.state().gone {
                break;
            }
            match condition {
                // Polled: sleep the interval (events do not tell us when the condition holds).
                WaitFor::Selector(_) | WaitFor::Text(_) | WaitFor::Function(_) => {
                    std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())))
                }
                _ => self.wait_change(&tab, deadline),
            }
        }
        Ok(WaitOutput {
            tab: tab.id.clone(),
            timeout: satisfied.is_none(),
            satisfied,
            page_generation: tab.state().page_generation,
            elapsed_ms: ms(started.elapsed()),
        })
    }

    fn evaluate(
        &mut self,
        id: Option<&str>,
        expression: &str,
        await_promise: bool,
        return_by_value: bool,
        max_chars: usize,
    ) -> Result<EvaluateOutput, CommandError> {
        let tab = self.resolve(id)?;
        let v = self
            .engine
            .send(
                &tab.session,
                runtime::EvaluateParams::METHOD,
                json!({"expression": expression, "awaitPromise": await_promise, "returnByValue": return_by_value, "userGesture": true}),
                EVALUATE_TIMEOUT,
            )
            .map_err(engine_err)?;
        if let Some(e) = v.get("exceptionDetails") {
            return Err(failed(exception_text(e)));
        }
        let r = &v["result"];
        let type_ = r["type"].as_str().unwrap_or("undefined").to_owned();
        let mut result = if let Some(u) = r["unserializableValue"].as_str() {
            Value::String(u.to_owned())
        } else if return_by_value || r.get("value").is_some() {
            r.get("value").cloned().unwrap_or(Value::Null)
        } else {
            r["description"]
                .as_str()
                .map_or(Value::Null, |d| Value::String(d.to_owned()))
        };
        let mut truncated = false;
        if let Value::String(s) = &result {
            if s.chars().count() > max_chars {
                result = Value::String(s.chars().take(max_chars).collect());
                truncated = true;
            }
        } else if !result.is_null() {
            let text = result.to_string();
            if text.chars().count() > max_chars {
                result = Value::String(text.chars().take(max_chars).collect());
                truncated = true;
            }
        }
        Ok(EvaluateOutput {
            tab: tab.id.clone(),
            result,
            type_,
            subtype: r["subtype"].as_str().map(str::to_owned),
            truncated,
            page_generation: tab.state().page_generation,
        })
    }
}

/// `[x, y, width, height]` from the page as a box, rounded to tenths of a CSS pixel as [`quad_box`] rounds.
fn rect_box(v: &Value) -> Option<BoxRow> {
    let a = v.as_array()?;
    let n = |i: usize| a.get(i).and_then(Value::as_f64);
    let (x, y, w, h) = (n(0)?, n(1)?, n(2)?, n(3)?);
    quad_box(&[x, y, x + w, y, x + w, y + h, x, y + h])
}

/// An exception's message and stack, from `exceptionDetails`.
fn exception_text(e: &Value) -> String {
    let description = e["exception"]["description"].as_str().unwrap_or_default();
    let mut out = if description.is_empty() {
        e["text"].as_str().unwrap_or("exception").to_owned()
    } else {
        format!(
            "{} {}",
            e["text"].as_str().unwrap_or("Uncaught"),
            description
        )
    };
    if description.lines().count() <= 1
        && let Some(st) = crate::tab::stack_text(&e["stackTrace"])
    {
        out.push('\n');
        out.push_str(&st);
    }
    out
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn javascript_truthiness() {
        assert!(!truthy(&Value::Null));
        assert!(!truthy(&json!(0)));
        assert!(!truthy(&json!("")));
        assert!(!truthy(&json!(false)));
        assert!(truthy(&json!([])));
        assert!(truthy(&json!({})));
        assert!(truthy(&json!("x")));
        assert!(truthy(&json!(0.5)));
    }

    #[test]
    fn patterns_match() {
        let m = matcher(&TextPattern::parse("/fail(ed)?/i")).unwrap();
        assert!(m("Request FAILED"));
        assert!(!m("ok"));
        let m = matcher(&TextPattern::parse("404")).unwrap();
        assert!(m("status 404"));
        assert!(matches!(
            matcher(&TextPattern::parse("/(/")),
            Err(CommandError::InvalidInput(_))
        ));
    }

    #[test]
    fn exceptions_read_as_text_and_stack() {
        let e = json!({"text": "Uncaught", "exception": {"description": "Error: nope\n    at <anonymous>:1:7"}});
        assert_eq!(
            exception_text(&e),
            "Uncaught Error: nope\n    at <anonymous>:1:7"
        );
        let e = json!({"text": "Uncaught", "exception": {"description": "nope"},
            "stackTrace": {"callFrames": [{"functionName": "f", "url": "u", "lineNumber": 0, "columnNumber": 0}]}});
        assert_eq!(exception_text(&e), "Uncaught nope\nat f (u:1:1)");
    }
}
