//! Acting on the page (brief 0024): `input`, `form_input` and `upload`.
//!
//! - **Where an action lands.** A ref is scrolled into view (`DOM.scrollIntoViewIfNeeded`) and its point is the
//!   center of its first content quad (`DOM.getContentQuads`, CSS pixels of the viewport, the space `screenshot`
//!   reports); a stale ref is refused as everywhere. A point is used as given, and the element there
//!   (`DOM.getNodeForLocation`) gets a ref, so the answer names what was hit.
//! - **Real input.** Mouse actions are `Input.dispatchMouseEvent` (moved, pressed, released, wheel), keys
//!   `Input.dispatchKeyEvent` ([`crate::keys`]), text `Input.insertText` or one key per character. Chrome answers
//!   each event once the page has handled it.
//! - **After the action.** `input` waits `wait_ms` for console errors and a navigation the action caused, ending
//!   early once such a navigation has loaded; the errors emitted since the action began are answered with their
//!   location. `form_input` waits only until the page's events settle.
//! - **Form fields** are set in the page (`Runtime.callFunctionOn` on each resolved element, [`SET_JS`]) through the
//!   prototype's `value` setter, so frameworks that wrap it (React) see the change, then get `input` and `change`;
//!   files go through `DOM.setFileInputFiles`.

use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::browser::{
    DropTarget, FieldResult, FieldValue, FormField, FormInputOutput, InputAction, InputOutput,
    InputTarget, KeyPress, Modifiers, Point, Target, UploadOutput,
};
use eludite_protocol::cdp::page;
use serde_json::{Value, json};

use super::{Browser, Tab, failed, invalid, ms};
use crate::keys;

/// The object group of an action's remote objects, released when it is done.
const GROUP: &str = "eludite-act";
/// Mouse moves between a drag's press and release.
const DRAG_STEPS: usize = 10;
/// `form_input` answers once the page's events have been quiet this long (at most [`SETTLE_MAX`]).
const SETTLE_QUIET: Duration = Duration::from_millis(15);
const SETTLE_MAX: Duration = Duration::from_millis(150);

/// Sets a form field (`this`) to a value: text, a checkbox's or radio's state, a select's options, or checks a file
/// input before `DOM.setFileInputFiles` (`{count}`). Answers `{ok, message?, file?, selected?}`.
pub const SET_JS: &str = r#"function(v) {
  const el = this;
  if (!el || el.nodeType !== 1) return {ok: false, message: 'the ref is not an element'};
  const tag = el.localName;
  const fire = () => {
    el.dispatchEvent(new Event('input', {bubbles: true}));
    el.dispatchEvent(new Event('change', {bubbles: true}));
  };
  const isFile = tag === 'input' && el.type === 'file';
  const files = v !== null && typeof v === 'object' && !Array.isArray(v);
  if (files || isFile) {
    if (!isFile) return {ok: false, message: `<${tag}> is not a file input; files go to an <input type=file>`};
    if (!files) return {ok: false, message: 'a file input takes {"files": [absolute paths]}'};
    if (el.disabled) return {ok: false, message: 'the file input is disabled'};
    if (v.count > 1 && !el.multiple) return {ok: false, message: 'the file input takes one file (it has no multiple attribute)'};
    return {ok: true, file: true};
  }
  if (el.disabled) return {ok: false, message: `the <${tag}> is disabled`};
  if (tag === 'select') {
    const values = Array.isArray(v) ? v : (typeof v === 'string' ? [v] : null);
    if (!values) return {ok: false, message: 'a select takes an option (its value or label) or, when it is multiple, a list of them'};
    return __select.call(el, values);
  }
  if (tag === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {
    if (typeof v !== 'boolean') return {ok: false, message: `a ${el.type} takes true or false`};
    if (el.checked !== v) { el.checked = v; fire(); }
    return {ok: true};
  }
  const noText = ['button', 'submit', 'reset', 'image', 'hidden', 'checkbox', 'radio', 'file'];
  if (tag === 'textarea' || (tag === 'input' && !noText.includes(el.type))) {
    if (typeof v !== 'string') return {ok: false, message: `a text field takes a string`};
    if (el.readOnly) return {ok: false, message: `the <${tag}> is read-only`};
    el.focus();
    const proto = tag === 'textarea' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v);
    fire();
    return {ok: true};
  }
  if (el.isContentEditable) {
    if (typeof v !== 'string') return {ok: false, message: 'an editable element takes a string'};
    el.focus();
    el.textContent = v;
    el.dispatchEvent(new InputEvent('input', {bubbles: true}));
    return {ok: true};
  }
  return {ok: false, message: `<${tag}${el.type ? ' type=' + el.type : ''}> is not a form field`};
}"#;

/// Selects options of a `<select>` (`this`) by value or label; answers `{ok, message?, selected?}`.
const SELECT_JS: &str = r#"function(values) {
  if (this.localName !== 'select') return {ok: false, message: `the element is a <${this.localName}>, not a <select>; click the options of a custom list instead`};
  if (this.disabled) return {ok: false, message: 'the select is disabled'};
  const opts = [...this.options];
  const pick = (w) => opts.find((o) => o.value === w) || opts.find((o) => o.label === w || o.text.trim() === w);
  const want = values.map(pick);
  const missing = values.filter((_, i) => !want[i]);
  if (missing.length) {
    const list = opts.slice(0, 50).map((o) => JSON.stringify(o.value) + (o.text.trim() !== o.value ? ' (' + o.text.trim() + ')' : '')).join(', ');
    return {ok: false, message: `no option has the value or label ${missing.map((m) => JSON.stringify(m)).join(', ')}; the options are ${list}`};
  }
  if (!this.multiple && values.length > 1) return {ok: false, message: 'the select takes one option (it is not multiple)'};
  for (const o of opts) o.selected = want.includes(o);
  this.dispatchEvent(new Event('input', {bubbles: true}));
  this.dispatchEvent(new Event('change', {bubbles: true}));
  return {ok: true, selected: opts.filter((o) => o.selected).map((o) => o.value)};
}"#;

/// [`SET_JS`] with [`SELECT_JS`] in reach.
fn set_js() -> String {
    format!("function(v) {{ const __select = {SELECT_JS}; return ({SET_JS}).call(this, v); }}")
}

/// Where an action landed.
#[derive(Debug, Default, Clone)]
struct Landed {
    /// Given as a ref (else as a point).
    from_ref: bool,
    backend: Option<i64>,
    point: Option<(f64, f64)>,
    ref_: Option<String>,
}

/// The center of a content quad (`[x1, y1, ..., x4, y4]`).
fn quad_center(q: &Value) -> Option<(f64, f64)> {
    let a = q.as_array()?;
    if a.len() < 8 {
        return None;
    }
    let n = |i: usize| a[i].as_f64();
    let x = (n(0)? + n(2)? + n(4)? + n(6)?) / 4.;
    let y = (n(1)? + n(3)? + n(5)? + n(7)?) / 4.;
    Some(((x * 10.).round() / 10., (y * 10.).round() / 10.))
}

fn mouse(kind: &str, (x, y): (f64, f64), button: &str, buttons: i64, clicks: i64, m: i64) -> Value {
    json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks, "modifiers": m})
}

fn click_events(point: (f64, f64), button: &str, clicks: i64, m: i64) -> Vec<Value> {
    let bit = if button == "right" { 2 } else { 1 };
    let mut out = vec![mouse("mouseMoved", point, "none", 0, 0, m)];
    for n in 1..=clicks {
        out.push(mouse("mousePressed", point, button, bit, n, m));
        out.push(mouse("mouseReleased", point, button, 0, n, m));
    }
    out
}

impl Browser {
    /// `input`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn input(
        &mut self,
        id: Option<&str>,
        action: InputAction,
        target: Option<&Target>,
        text: Option<&str>,
        per_key: bool,
        key_list: &[KeyPress],
        delta: (f64, f64),
        to: Option<&Target>,
        values: &[String],
        modifiers: Modifiers,
        wait_ms: u64,
    ) -> Result<InputOutput, CommandError> {
        use InputAction as A;
        let tab = self.resolve(id)?;
        let started = Instant::now();
        let (seq0, loader0) = {
            let s = tab.state();
            (s.console.last_seq(), s.main_loader.clone())
        };
        let needs_point = matches!(
            action,
            A::Click | A::DoubleClick | A::RightClick | A::Hover | A::Drag | A::Scroll
        );
        let landed = target
            .map(|t| self.land(&tab, t, needs_point))
            .transpose()?;
        let (role, name) = landed
            .as_ref()
            .and_then(|l| l.backend)
            .map(|b| self.role_name(&tab, b))
            .unwrap_or_default();
        let m = modifiers.bits();
        let mut to_out = None;
        let mut selected = None;
        let point = landed.as_ref().and_then(|l| l.point);
        let mut point_out = point;
        match action {
            A::Click | A::DoubleClick | A::RightClick => {
                let p = point.ok_or_else(|| failed("no point to click"))?;
                let (button, clicks) = match action {
                    A::DoubleClick => ("left", 2),
                    A::RightClick => ("right", 1),
                    _ => ("left", 1),
                };
                self.mouse_events(&tab, click_events(p, button, clicks, m))?;
            }
            A::Hover => {
                let p = point.ok_or_else(|| failed("no point to hover"))?;
                self.mouse_events(&tab, vec![mouse("mouseMoved", p, "none", 0, 0, m)])?;
            }
            A::Scroll => {
                let p = match point {
                    Some(p) => p,
                    None => {
                        let lm = self.call(&tab, page::GetLayoutMetricsParams {})?;
                        let vv = &lm.css_visual_viewport;
                        (
                            (vv.client_width / 2.).round(),
                            (vv.client_height / 2.).round(),
                        )
                    }
                };
                point_out = Some(p);
                self.mouse_events(
                    &tab,
                    vec![
                        mouse("mouseMoved", p, "none", 0, 0, m),
                        json!({"type": "mouseWheel", "x": p.0, "y": p.1, "deltaX": delta.0, "deltaY": delta.1, "modifiers": m}),
                    ],
                )?;
            }
            A::Drag => {
                let from = point.ok_or_else(|| failed("no point to drag from"))?;
                let dest =
                    self.land(&tab, to.ok_or_else(|| invalid("`drag` needs `to`"))?, true)?;
                let end = dest.point.ok_or_else(|| failed("no point to drag to"))?;
                let mut events = vec![
                    mouse("mouseMoved", from, "none", 0, 0, m),
                    mouse("mousePressed", from, "left", 1, 1, m),
                ];
                for i in 1..=DRAG_STEPS {
                    let t = i as f64 / DRAG_STEPS as f64;
                    let p = (from.0 + (end.0 - from.0) * t, from.1 + (end.1 - from.1) * t);
                    events.push(mouse("mouseMoved", p, "left", 1, 0, m));
                }
                events.push(mouse("mouseReleased", end, "left", 0, 1, m));
                self.mouse_events(&tab, events)?;
                to_out = Some(DropTarget {
                    ref_: dest.ref_,
                    point: Some(Point { x: end.0, y: end.1 }),
                });
            }
            A::Type => {
                if let Some(l) = &landed {
                    self.focus_landed(&tab, l, m)?;
                }
                let text = text.ok_or_else(|| invalid("`type` needs `text`"))?;
                if per_key {
                    self.type_per_key(&tab, text)?;
                } else {
                    self.send(&tab, "Input.insertText", json!({"text": text}))?;
                }
            }
            A::Key => {
                if let Some(l) = &landed {
                    self.focus_landed(&tab, l, m)?;
                }
                let mut events = Vec::new();
                for k in key_list {
                    events.extend(keys::press(&k.key, k.modifiers.union(modifiers)));
                }
                self.key_events(&tab, events)?;
            }
            A::Select => {
                let backend = landed
                    .as_ref()
                    .and_then(|l| l.backend)
                    .ok_or_else(|| failed("there is no element at that point"))?;
                let r = self.call_on(&tab, backend, SELECT_JS, json!(values))?;
                if r["ok"] != true {
                    return Err(invalid(
                        r["message"]
                            .as_str()
                            .unwrap_or("the options were not selected"),
                    ));
                }
                selected = Some(
                    r["selected"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                );
            }
            A::Focus => {
                let backend = landed
                    .as_ref()
                    .and_then(|l| l.backend)
                    .ok_or_else(|| failed("there is no element at that point"))?;
                self.send(&tab, "DOM.focus", json!({"backendNodeId": backend}))
                    .map_err(|e| failed(format!("the element cannot take focus: {e}")))?;
            }
        }
        // Console errors and a navigation the action caused.
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            {
                let s = tab.state();
                if s.gone {
                    break;
                }
                if s.main_loader != loader0
                    && let Some(l) = &s.main_loader
                    && s.lifecycle_of(l).load
                {
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            self.wait_change(&tab, deadline);
        }
        let s = tab.state();
        let navigated = s.main_loader != loader0;
        Ok(InputOutput {
            tab: tab.id.clone(),
            action: action.as_str().to_owned(),
            target: InputTarget {
                ref_: landed.and_then(|l| l.ref_),
                role,
                name,
                point: point_out.map(|(x, y)| Point { x, y }),
            },
            to: to_out,
            selected,
            page_generation: s.page_generation,
            navigated,
            url: navigated.then(|| s.url.clone()),
            console_errors: s.console_error_list_since(seq0),
            elapsed_ms: ms(started.elapsed()),
        })
    }

    /// Resolve a target: a ref scrolled into view and its center, or a point and the element there.
    fn land(&self, tab: &Tab, target: &Target, need_point: bool) -> Result<Landed, CommandError> {
        match target {
            Target::Ref(r) => {
                let backend = self.backend_of(tab, r)?;
                let _ = self.send(
                    tab,
                    "DOM.scrollIntoViewIfNeeded",
                    json!({"backendNodeId": backend}),
                );
                let point = self
                    .send(
                        tab,
                        "DOM.getContentQuads",
                        json!({"backendNodeId": backend}),
                    )
                    .ok()
                    .and_then(|v| {
                        v["quads"]
                            .as_array()
                            .and_then(|q| q.first())
                            .and_then(quad_center)
                    });
                if need_point && point.is_none() {
                    return Err(failed(format!(
                        "the element {r} has no box on the page (it is hidden, empty or not rendered), so there is no point to act on"
                    )));
                }
                Ok(Landed {
                    from_ref: true,
                    backend: Some(backend),
                    point,
                    ref_: Some(r.clone()),
                })
            }
            Target::Point { x, y } => {
                let backend = self
                    .send(
                        tab,
                        "DOM.getNodeForLocation",
                        json!({"x": x.round() as i64, "y": y.round() as i64, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": true}),
                    )
                    .ok()
                    .and_then(|v| v["backendNodeId"].as_i64());
                let ref_ = backend.map(|b| {
                    let mut s = tab.state();
                    let g = s.page_generation;
                    s.refs.issue(b, g)
                });
                Ok(Landed {
                    from_ref: false,
                    backend,
                    point: Some((*x, *y)),
                    ref_,
                })
            }
        }
    }

    /// The accessible role and name of a node.
    fn role_name(&self, tab: &Tab, backend: i64) -> (Option<String>, Option<String>) {
        let Ok(v) = self.send(
            tab,
            "Accessibility.getPartialAXTree",
            json!({"backendNodeId": backend, "fetchRelatives": false}),
        ) else {
            return (None, None);
        };
        let n = &v["nodes"][0];
        let role = n["role"]["value"]
            .as_str()
            .filter(|r| !r.is_empty())
            .map(str::to_owned);
        let name = n["name"]["value"]
            .as_str()
            .map(crate::page::collapse)
            .filter(|n| !n.is_empty());
        (role, name)
    }

    /// Focus where a `type` or `key` lands: the element of a ref, or a click at a point.
    fn focus_landed(&self, tab: &Tab, l: &Landed, m: i64) -> Result<(), CommandError> {
        if l.from_ref
            && let Some(b) = l.backend
            && self
                .send(tab, "DOM.focus", json!({"backendNodeId": b}))
                .is_ok()
        {
            return Ok(());
        }
        match l.point {
            Some(p) => self.mouse_events(tab, click_events(p, "left", 1, m)),
            None => Err(failed(
                "the element cannot take focus and has no point to click",
            )),
        }
    }

    fn type_per_key(&self, tab: &Tab, text: &str) -> Result<(), CommandError> {
        for c in text.chars() {
            let d = keys::char_def(c, false);
            if d.key_code == 0 {
                self.send(tab, "Input.insertText", json!({"text": c.to_string()}))?;
            } else {
                self.key_events(
                    tab,
                    vec![keys::key_event(&d, true, 0), keys::key_event(&d, false, 0)],
                )?;
            }
        }
        Ok(())
    }

    fn dispatch_all(
        &self,
        tab: &Tab,
        method: &str,
        events: Vec<Value>,
    ) -> Result<(), CommandError> {
        let calls = events.into_iter().map(|e| (method.to_owned(), e)).collect();
        for r in self
            .engine
            .send_many(&tab.session, calls, crate::connection::DEFAULT_TIMEOUT)
        {
            r.map_err(|e| failed(format!("{method}: {e}")))?;
        }
        Ok(())
    }

    fn mouse_events(&self, tab: &Tab, events: Vec<Value>) -> Result<(), CommandError> {
        self.dispatch_all(tab, "Input.dispatchMouseEvent", events)
    }

    fn key_events(&self, tab: &Tab, events: Vec<Value>) -> Result<(), CommandError> {
        self.dispatch_all(tab, "Input.dispatchKeyEvent", events)
    }

    /// Run `function` on a node with one argument, by value.
    fn call_on(
        &self,
        tab: &Tab,
        backend: i64,
        function: &str,
        arg: Value,
    ) -> Result<Value, CommandError> {
        let obj = self.send(
            tab,
            "DOM.resolveNode",
            json!({"backendNodeId": backend, "objectGroup": GROUP}),
        )?;
        let object = obj["object"]["objectId"]
            .as_str()
            .ok_or_else(|| failed("the element is gone"))?
            .to_owned();
        let r = self.send(
            tab,
            "Runtime.callFunctionOn",
            json!({"objectId": object, "functionDeclaration": function, "arguments": [{"value": arg}], "returnByValue": true, "userGesture": true}),
        );
        let _ = self.send(
            tab,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": GROUP}),
        );
        let r = r?;
        if let Some(e) = r.get("exceptionDetails") {
            return Err(failed(super::exception_text(e)));
        }
        Ok(r["result"]["value"].clone())
    }

    /// Wait until the tab's events have been quiet for [`SETTLE_QUIET`], at most [`SETTLE_MAX`].
    fn settle(&self, tab: &Tab) {
        let start = Instant::now();
        loop {
            let s = tab.state();
            let (guard, timeout) = tab
                .shared
                .changed
                .wait_timeout(s, SETTLE_QUIET)
                .unwrap_or_else(|e| e.into_inner());
            drop(guard);
            if timeout.timed_out() || start.elapsed() >= SETTLE_MAX {
                return;
            }
        }
    }

    /// `form_input`.
    pub(super) fn form_input(
        &mut self,
        id: Option<&str>,
        fields: &[FormField],
    ) -> Result<FormInputOutput, CommandError> {
        let tab = self.resolve(id)?;
        let seq0 = tab.state().console.last_seq();
        // A stale ref refuses the call: the agent's view of the page is out of date.
        let backends = fields
            .iter()
            .map(|f| self.backend_of(&tab, &f.ref_))
            .collect::<Result<Vec<_>, _>>()?;
        let resolved = self.engine.send_many(
            &tab.session,
            backends
                .iter()
                .map(|b| {
                    (
                        "DOM.resolveNode".to_owned(),
                        json!({"backendNodeId": b, "objectGroup": GROUP}),
                    )
                })
                .collect(),
            crate::connection::DEFAULT_TIMEOUT,
        );
        let js = set_js();
        let calls = fields
            .iter()
            .zip(&resolved)
            .map(|(f, r)| {
                let object = r
                    .as_ref()
                    .ok()
                    .and_then(|v| v["object"]["objectId"].as_str())
                    .unwrap_or_default();
                let value = match &f.value {
                    FieldValue::Text(t) => json!(t),
                    FieldValue::Checked(b) => json!(b),
                    FieldValue::Options(o) => json!(o),
                    FieldValue::Files(p) => json!({"count": p.len()}),
                };
                (
                    "Runtime.callFunctionOn".to_owned(),
                    json!({"objectId": object, "functionDeclaration": js, "arguments": [{"value": value}], "returnByValue": true, "userGesture": true}),
                )
            })
            .collect();
        let answers =
            self.engine
                .send_many(&tab.session, calls, crate::connection::DEFAULT_TIMEOUT);
        let mut out = Vec::with_capacity(fields.len());
        for ((f, backend), answer) in fields.iter().zip(&backends).zip(answers) {
            let result = match answer {
                Err(e) => Err(format!("the element could not be reached: {e}")),
                Ok(v) if v.get("exceptionDetails").is_some() => {
                    Err(super::exception_text(&v["exceptionDetails"]))
                }
                Ok(v) => {
                    let r = &v["result"]["value"];
                    if r["ok"] != true {
                        Err(r["message"]
                            .as_str()
                            .unwrap_or("the field was not set")
                            .to_owned())
                    } else if let FieldValue::Files(paths) = &f.value {
                        self.set_files(&tab, *backend, paths)
                    } else {
                        Ok(())
                    }
                }
            };
            out.push(FieldResult {
                ref_: f.ref_.clone(),
                ok: result.is_ok(),
                message: result.err(),
            });
        }
        let _ = self.send(
            &tab,
            "Runtime.releaseObjectGroup",
            json!({"objectGroup": GROUP}),
        );
        self.settle(&tab);
        let s = tab.state();
        Ok(FormInputOutput {
            tab: tab.id.clone(),
            fields: out,
            page_generation: s.page_generation,
            console_errors: s.console_error_list_since(seq0),
        })
    }

    /// `DOM.setFileInputFiles` with existing files.
    fn set_files(&self, tab: &Tab, backend: i64, paths: &[String]) -> Result<(), String> {
        for p in paths {
            match std::fs::metadata(p) {
                Ok(m) if m.is_file() => {}
                Ok(_) => return Err(format!("{p} is not a file")),
                Err(e) => return Err(format!("{p}: {e}")),
            }
        }
        self.send(
            tab,
            "DOM.setFileInputFiles",
            json!({"files": paths, "backendNodeId": backend}),
        )
        .map(drop)
        .map_err(|e| e.to_string())
    }

    /// `upload`.
    pub(super) fn upload(
        &mut self,
        id: Option<&str>,
        r: &str,
        paths: &[String],
    ) -> Result<UploadOutput, CommandError> {
        let tab = self.resolve(id)?;
        let backend = self.backend_of(&tab, r)?;
        let check = self.call_on(&tab, backend, &set_js(), json!({"count": paths.len()}))?;
        if check["ok"] != true {
            return Err(invalid(format!(
                "{r}: {}",
                check["message"].as_str().unwrap_or("not a file input")
            )));
        }
        self.set_files(&tab, backend, paths).map_err(failed)?;
        let count = self
            .call_on(
                &tab,
                backend,
                "function() { return this.files ? this.files.length : 0; }",
                Value::Null,
            )?
            .as_u64()
            .unwrap_or(paths.len() as u64) as usize;
        Ok(UploadOutput {
            tab: tab.id.clone(),
            count,
            page_generation: tab.state().page_generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_have_centers() {
        assert_eq!(
            quad_center(&json!([10, 20, 30, 20, 30, 40, 10, 40])),
            Some((20., 30.))
        );
        assert_eq!(quad_center(&json!([1, 2])), None);
    }

    #[test]
    fn clicks_press_and_release_per_click() {
        let e = click_events((5., 6.), "left", 2, 8);
        let kinds: Vec<_> = e
            .iter()
            .map(|e| {
                (
                    e["type"].as_str().unwrap(),
                    e["clickCount"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("mouseMoved", 0),
                ("mousePressed", 1),
                ("mouseReleased", 1),
                ("mousePressed", 2),
                ("mouseReleased", 2)
            ]
        );
        assert_eq!(e[1]["modifiers"], 8);
        assert_eq!(click_events((0., 0.), "right", 1, 0)[1]["buttons"], 2);
        assert!(set_js().contains("__select"));
    }
}
