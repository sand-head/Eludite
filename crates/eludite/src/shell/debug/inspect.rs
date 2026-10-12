//! Brief 0077's half of the debugger in the shell: the state the inspection windows and helpers keep beside the
//! session model, the one answer router for their adapter requests (`Pending::Inspect`), and the hooks the debugger's
//! paths call (in marked blocks of `debug.rs`): the window refresh, the `exceptionInfo` answer, a hover's answer, an
//! Immediate line's answer, `module` events, the margin's events, the session's start and end.
//!
//! What it adds, each in its module: the exception helper ([`super::exception_helper`]), DataTips as trees
//! ([`super::datatips`]), the Immediate window ([`super::immediate`]), QuickWatch ([`super::quickwatch`]), Autos
//! ([`super::autos`]), the Modules window ([`super::modules`]), the glyph's menu, tooltip and Breakpoint Settings
//! ([`super::breakpoint_settings`]); and here: Watch 2 to 4, changed values, `[External Code]`, every thread's
//! location, Show Next Statement, Enable/Disable (All) Breakpoints, Hexadecimal Display, the debugging options and the
//! Design/Debug layouts.
//!
//! Every answer quotes the session generation and stop it was asked in and is dropped when stale (invariant 12); no
//! request waits on the UI thread (invariant 1).

use std::collections::HashMap;

use eludite_commands::debug_inspect::{InspectRequest, WATCH_WINDOWS, WatchOp};
use eludite_docking::model::DockLayout;
use gpui::{App, Entity};

use super::breakpoint_settings::{
    self as bps, BreakpointSettings, GlyphAction, GlyphMenu, GlyphTip,
};
use super::datatips::{self, DataTipPanel};
use super::exception_helper::{self as helper, ExceptionHelperView, HelperContent};
use super::modules::Modules;
use super::quickwatch::{CloseQuickWatch, QuickWatchDialog};
use super::windows::{ToggleVariable, VarsEvent, VarsKind, VarsWindow};
use super::*;

/// What an inspection request's answer is for.
pub(super) enum Target {
    /// An expression of a tree evaluated (row `ix` at the top level).
    Eval { kind: VarsKind, ix: usize },
    /// The members of the row at `path` of a tree.
    Vars { kind: VarsKind, path: Vec<usize> },
    /// Another thread's top frame (the Threads window's Location).
    ThreadTop { thread: i64 },
}

/// The settings the debugging options page sets (brief 0077).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InspectSettings {
    pub just_my_code: bool,
    pub property_evaluation: bool,
    pub show_external_code: bool,
    pub hexadecimal: bool,
}

impl Default for InspectSettings {
    fn default() -> Self {
        Self {
            just_my_code: true,
            property_evaluation: true,
            show_external_code: false,
            hexadecimal: false,
        }
    }
}

/// Brief 0077's state in the [`Debugger`].
#[derive(Default)]
pub struct Inspect {
    /// Watch 2 to 4's expressions and values.
    pub watches: [Vec<VarNode>; 3],
    pub autos: Vec<VarNode>,
    pub quickwatch: Vec<VarNode>,
    pub datatip: Vec<VarNode>,
    /// The values of the variables windows at the previous stop and now, by frame function and row names.
    prev_values: HashMap<String, String>,
    cur_values: HashMap<String, String>,
    /// The (generation, stop, frame) the trees were last evaluated for.
    seen: Option<(u64, u64, usize)>,
    /// The (generation, stop) the threads' top frames were asked for, and the answers.
    tops_for: Option<(u64, u64)>,
    pub tops: HashMap<i64, String>,
    /// Per session id: its modules.
    pub modules: HashMap<u32, Modules>,
    /// The exception helper: its document, the stop it is for, its view.
    pub helper: Option<(String, u64, Entity<ExceptionHelperView>)>,
    /// The helper's content of the last exception stop, which Show Next Statement reopens.
    pub helper_content: Option<(u64, HelperContent)>,
    /// The DataTip: its document, its stop and panel.
    pub datatip_panel: Option<(String, u64, Entity<DataTipPanel>)>,
    pub quickwatch_dialog: Option<Entity<QuickWatchDialog>>,
    /// The glyph popups: their document.
    pub glyph_doc: Option<String>,
    pub glyph_tip_row: Option<u32>,
    /// The Breakpoint Settings panel, while open.
    pub glyph_settings: Option<Entity<BreakpointSettings>>,
    pub glyph_menu: Option<Entity<GlyphMenu>>,
    pub settings: InspectSettings,
    /// The Design and Debug layouts (brief 0077), and whether the Debug one is applied.
    pub design_layout: Option<DockLayout>,
    pub debug_layout: Option<DockLayout>,
    pub in_debug_layout: bool,
}

/// The key of a row's value for the changed-values comparison.
fn value_key(scope: &str, kind: VarsKind, names: &[String]) -> String {
    format!("{scope}\u{1}{}\u{1}{}", kind.prefix(), names.join("\u{1}"))
}

/// Each visible row's names from the top, and expression (the adapter's evaluate name, else the names joined).
fn row_names(nodes: &[VarNode]) -> Vec<(Vec<String>, String)> {
    fn walk(
        nodes: &[VarNode],
        prefix: &mut Vec<String>,
        exprs: &mut Vec<String>,
        out: &mut Vec<(Vec<String>, String)>,
    ) {
        for n in nodes {
            prefix.push(n.name.clone());
            let expr = n
                .evaluate_name
                .clone()
                .unwrap_or_else(|| match exprs.last() {
                    Some(p) if n.name.starts_with('[') => format!("{p}{}", n.name),
                    Some(p) => format!("{p}.{}", n.name),
                    None => n.name.clone(),
                });
            out.push((prefix.clone(), expr.clone()));
            if n.expanded
                && let Some(c) = &n.children
            {
                exprs.push(expr);
                walk(c, prefix, exprs, out);
                exprs.pop();
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(nodes, &mut Vec::new(), &mut Vec::new(), &mut out);
    out
}

/// The Call Stack's rows with consecutive external frames (no source, or `subtle`) as one `[External Code]` row unless
/// they are shown; each row's frame index (`None` for that row) and file and line.
pub fn stack_rows(
    frames: &[Frame],
    selected: usize,
    show_external: bool,
) -> (
    Vec<StackRow>,
    Vec<Option<usize>>,
    Vec<Option<(String, u32)>>,
) {
    let (mut rows, mut ix, mut targets) = (Vec::new(), Vec::new(), Vec::new());
    for f in frames {
        let external = f.row.path.is_none() || f.subtle;
        if external && !show_external {
            if ix.last() == Some(&None) {
                continue;
            }
            rows.push(StackRow {
                name: "[External Code]".into(),
                location: String::new(),
                selected: false,
            });
            ix.push(None);
            targets.push(None);
            continue;
        }
        rows.push(StackRow {
            name: f.row.name.clone(),
            location: match (&f.row.path, f.row.line) {
                (Some(p), Some(l)) => format!("{}, line {l}", file_name(p)),
                _ => String::new(),
            },
            selected: f.row.index == selected,
        });
        ix.push(Some(f.row.index));
        targets.push(f.row.path.clone().zip(f.row.line));
    }
    (rows, ix, targets)
}

/// Subscribe the shell to the variables windows' events (called once, when the shell is made).
pub(in crate::shell) fn subscribe(windows: &DebugWindows, cx: &mut Context<Shell>) {
    for kind in [
        VarsKind::Locals,
        VarsKind::Watch,
        VarsKind::Watch2,
        VarsKind::Watch3,
        VarsKind::Watch4,
        VarsKind::Autos,
    ] {
        let Some(v) = windows.vars(kind) else {
            continue;
        };
        if !matches!(kind, VarsKind::Locals | VarsKind::Watch) {
            cx.subscribe(v, move |shell, _, e: &ToggleVariable, cx| {
                shell.inspect_toggle(kind, &e.0, cx)
            })
            .detach();
        }
        cx.subscribe(v, move |shell, _, e: &VarsEvent, cx| {
            shell.inspect_vars_event(kind, e, cx)
        })
        .detach();
    }
}

impl Inspect {
    fn tree_mut<'a>(
        &'a mut self,
        model: &'a mut DebugModel,
        kind: VarsKind,
    ) -> &'a mut Vec<VarNode> {
        match kind {
            VarsKind::Locals => &mut model.locals,
            VarsKind::Watch => &mut model.watches,
            VarsKind::Watch2 => &mut self.watches[0],
            VarsKind::Watch3 => &mut self.watches[1],
            VarsKind::Watch4 => &mut self.watches[2],
            VarsKind::Autos => &mut self.autos,
            VarsKind::QuickWatch => &mut self.quickwatch,
            VarsKind::DataTip => &mut self.datatip,
        }
    }
}

impl Debugger {
    /// Evaluate row `ix` of tree `kind` in the selected frame (context `watch`).
    fn inspect_eval(&mut self, kind: VarsKind, ix: usize) {
        if self.model.mode != Mode::Break {
            return;
        }
        let Some(frame) = self.model.frames.get(self.model.frame).map(|f| f.id) else {
            return;
        };
        let (generation, stop) = (self.generation(), self.model.stop);
        let tree = self.inspect.tree_mut(&mut self.model, kind);
        let Some(n) = tree.get_mut(ix) else {
            return;
        };
        *n = VarNode::watch(&n.name);
        n.loading = true;
        let expression = n.name.clone();
        let _ = self.send(
            "evaluate",
            json!({"expression": expression, "frameId": frame, "context": "watch"}),
            Pending::Inspect {
                generation,
                stop,
                target: Target::Eval { kind, ix },
            },
        );
    }
}

impl Shell {
    // ----- wiring -----

    /// The debugging options (Tools > Options > Debugging > General).
    pub(in crate::shell) fn inspect_apply_settings(
        &mut self,
        s: InspectSettings,
        cx: &mut Context<Self>,
    ) {
        eludite_dap::options::set_just_my_code(s.just_my_code);
        if self.debug.inspect.settings != s {
            self.debug.inspect.settings = s;
            self.refresh_debug(cx);
        }
    }

    // ----- the commands -----

    /// Apply one of brief 0077's commands.
    pub(super) fn apply_inspect(
        &mut self,
        request: InspectRequest,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<Follow>), CommandError> {
        let in_break = || -> Result<(), CommandError> { Ok(()) };
        let _ = in_break;
        match request {
            InspectRequest::Modules => {
                let sid = self.debug.active;
                let out = self
                    .debug
                    .inspect
                    .modules
                    .get(&sid)
                    .map(Modules::output)
                    .unwrap_or_default();
                Ok((DebugOutput::Modules(out), None))
            }
            InspectRequest::ShowNextStatement => {
                self.need_break("Show Next Statement")?;
                let thread = self.debug.model.stopped.as_ref().map(|s| s.thread);
                let current = self.debug.model.thread;
                if thread.is_some() && thread != current {
                    self.debug_select(thread, Some(0), window, cx)?;
                } else {
                    self.debug_show_frame(0, window, cx);
                }
                // `debug_show_frame` brings the document forward; Show Next Statement also moves the caret there.
                if let Some(e) = self.debug.exec.clone() {
                    let _ = self.open_file(&e.path, Some((e.line, e.column)), window, cx);
                }
                self.exception_helper_open(cx);
                self.refresh_debug(cx);
                Ok((self.debug_state(), None))
            }
            InspectRequest::QuickWatch { expression } => {
                self.need_break("QuickWatch")?;
                let expression = match expression {
                    Some(e) => e,
                    None => self.expression_at_caret(cx).ok_or_else(|| {
                        CommandError::InvalidInput(
                            "no expression: select one, or put the caret on one, or give `expression`".into(),
                        )
                    })?,
                };
                if !caller.is_agent() {
                    self.quickwatch_open(&expression, window, cx);
                }
                let d = &mut self.debug;
                d.inspect.quickwatch = vec![VarNode::watch(&expression)];
                d.inspect_eval(VarsKind::QuickWatch, 0);
                self.refresh_debug(cx);
                if agent {
                    // An agent's answer is the evaluation, as `eludite.debug.evaluate` gives it.
                    return self.apply_debug(
                        None,
                        DebugRequest::Evaluate {
                            expression,
                            frame: None,
                            context: cmds::EvalContext::Watch,
                            expand: false,
                            stop: None,
                        },
                        caller,
                        agent,
                        window,
                        cx,
                    );
                }
                Ok((
                    DebugOutput::Evaluate(EvaluateOutput {
                        expression,
                        state: "pending".into(),
                        stop: self.debug.model.stop,
                        ..Default::default()
                    }),
                    None,
                ))
            }
            InspectRequest::EnableBreakpoints {
                all,
                enabled,
                path,
                line,
            } => {
                if all {
                    let on = enabled.unwrap_or(true);
                    let files = self.debug.model.breakpoints.files();
                    for b in self.debug.model.breakpoints.iter_mut() {
                        b.enabled = on;
                    }
                    let names: Vec<String> = self
                        .debug
                        .model
                        .breakpoints
                        .functions()
                        .iter()
                        .map(|f| f.name.clone())
                        .collect();
                    for n in &names {
                        if let Some(f) = self.debug.model.breakpoints.function_mut(n) {
                            f.enabled = on;
                        }
                    }
                    for f in &files {
                        self.debug_send_breakpoints(f);
                    }
                    if !names.is_empty() {
                        self.debug.send_function_breakpoints();
                    }
                } else {
                    let (path, line) = match (path, line) {
                        (Some(p), Some(l)) => (p, l),
                        (p, l) => {
                            let (cp, cl) = self.caret_line(cx).ok_or_else(|| {
                                CommandError::InvalidInput(
                                    "no `path` and `line`, and no document is active".into(),
                                )
                            })?;
                            (p.unwrap_or(cp), l.unwrap_or(cl))
                        }
                    };
                    let path = norm(&self.resolve_file(&path).to_string_lossy());
                    let b = self
                        .debug
                        .model
                        .breakpoints
                        .at_mut(&path, line)
                        .ok_or_else(|| {
                            CommandError::InvalidInput(format!(
                                "there is no breakpoint at {}, line {line}",
                                file_name(&path)
                            ))
                        })?;
                    b.enabled = enabled.unwrap_or(!b.enabled);
                    self.debug_send_breakpoints(&path);
                }
                self.refresh_glyphs(cx);
                self.debug_persist(cx);
                self.refresh_debug(cx);
                Ok((self.debug_state(), None))
            }
            InspectRequest::Watch { window: n, op } => {
                self.apply_watch(n, op, cx)?;
                Ok((self.debug_state(), None))
            }
        }
    }

    fn need_break(&self, what: &str) -> Result<(), CommandError> {
        if self.debug.model.mode == Mode::Break {
            Ok(())
        } else {
            Err(CommandError::Failed(format!(
                "{what} needs break mode (the debugger is in {} mode)",
                self.debug.model.mode.as_str()
            )))
        }
    }

    /// The active document and its caret line (1-based).
    fn caret_line(&self, cx: &App) -> Option<(String, u32)> {
        let id = self.controller.active_document()?;
        let doc = self.documents.get(&id)?;
        let row = doc.view.read(cx).editor().primary_head().row;
        Some((id, row + 1))
    }

    /// QuickWatch's default: the active document's selection, else the expression at the caret.
    fn expression_at_caret(&self, cx: &App) -> Option<String> {
        let id = self.controller.active_document()?;
        let view = self.documents.get(&id)?.view.read(cx);
        let s = view.editor().primary_selection();
        if s.head != s.tail {
            let r = s.head.min(s.tail)..s.head.max(s.tail);
            let t = view.editor().buffer().text_for_range(r);
            return Some(t.trim().to_owned()).filter(|t| !t.is_empty());
        }
        view.expression_at(s.head).map(|(_, e)| e)
    }

    /// `eludite.debug.watch` on Watch `n` (1 to 4): add, remove, edit in place, move.
    fn apply_watch(
        &mut self,
        n: usize,
        op: WatchOp,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        let kind = VarsKind::watch(n).ok_or_else(|| {
            CommandError::InvalidInput(format!("`window` is 1 to {WATCH_WINDOWS}, not {n}"))
        })?;
        let d = &mut self.debug;
        let list = d.inspect.tree_mut(&mut d.model, kind);
        let len = list.len();
        let missing = |ix: usize| {
            CommandError::InvalidInput(format!(
                "there is no watch {ix} in Watch {n} (there are {len})"
            ))
        };
        let eval = match op {
            WatchOp::Add(e) => {
                list.push(VarNode::watch(&e));
                Some(len)
            }
            WatchOp::Remove(ix) => {
                if ix >= len {
                    return Err(missing(ix));
                }
                list.remove(ix);
                None
            }
            WatchOp::Edit { index, expression } => {
                if index >= len {
                    return Err(missing(index));
                }
                list[index] = VarNode::watch(&expression);
                Some(index)
            }
            WatchOp::Move { from, to } => {
                if from >= len || to >= len {
                    return Err(missing(from.max(to)));
                }
                let w = list.remove(from);
                list.insert(to, w);
                None
            }
        };
        if let Some(ix) = eval {
            if kind == VarsKind::Watch {
                self.debug_eval_watch(ix);
            } else {
                self.debug.inspect_eval(kind, ix);
            }
        }
        if kind == VarsKind::Watch {
            self.debug_persist(cx);
        }
        self.refresh_debug(cx);
        Ok(())
    }

    // ----- trees -----

    /// Expand or collapse a row of a tree other than Locals and Watch 1 (which `debug_toggle_variable` serves).
    pub(super) fn inspect_toggle(
        &mut self,
        kind: VarsKind,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        if matches!(kind, VarsKind::Locals | VarsKind::Watch) {
            return self.debug_toggle_variable(kind == VarsKind::Watch, path, cx);
        }
        let d = &mut self.debug;
        let (generation, stop) = (d.generation(), d.model.stop);
        let tree = d.inspect.tree_mut(&mut d.model, kind);
        let Some(node) = node_mut(tree, path) else {
            return;
        };
        if node.expanded {
            node.expanded = false;
        } else {
            node.expanded = true;
            if node.children.is_none() && node.reference > 0 && !node.loading {
                node.loading = true;
                let reference = node.reference;
                let _ = d.send(
                    "variables",
                    json!({ "variablesReference": reference }),
                    Pending::Inspect {
                        generation,
                        stop,
                        target: Target::Vars {
                            kind,
                            path: path.to_vec(),
                        },
                    },
                );
            }
        }
        self.refresh_debug(cx);
    }

    fn inspect_vars_event(&mut self, kind: VarsKind, e: &VarsEvent, cx: &mut Context<Self>) {
        match e {
            VarsEvent::ExpandAll(path) => {
                // The row and its loaded members.
                let d = &mut self.debug;
                let tree = d.inspect.tree_mut(&mut d.model, kind);
                let mut todo = Vec::new();
                if let Some(n) = node_mut(tree, path) {
                    if !n.expanded {
                        todo.push(path.clone());
                    }
                    if let Some(c) = &n.children {
                        for (i, m) in c.iter().enumerate() {
                            if m.reference > 0 && !m.expanded {
                                let mut p = path.clone();
                                p.push(i);
                                todo.push(p);
                            }
                        }
                    }
                }
                for p in todo {
                    self.inspect_toggle(kind, &p, cx);
                }
            }
            VarsEvent::CollapseAll => {
                let d = &mut self.debug;
                let tree = d.inspect.tree_mut(&mut d.model, kind);
                fn collapse(nodes: &mut [VarNode]) {
                    for n in nodes {
                        n.expanded = false;
                        if let Some(c) = n.children.as_mut() {
                            collapse(c);
                        }
                    }
                }
                collapse(tree);
                self.refresh_debug(cx);
            }
        }
    }

    /// An answer to a `Pending::Inspect` request.
    pub(super) fn on_inspect_response(
        &mut self,
        generation: u64,
        stop: u64,
        target: Target,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let d = &mut self.debug;
        if generation != d.generation() || stop != d.model.stop {
            return;
        }
        match target {
            Target::Eval { kind, ix } => {
                let r: Result<EvaluateResponse, String> =
                    result.and_then(|b| serde_json::from_value(b).map_err(|e| e.to_string()));
                let tree = d.inspect.tree_mut(&mut d.model, kind);
                if let Some(w) = tree.get_mut(ix) {
                    match r {
                        Ok(e) => {
                            w.value = cmds::null_spelling(e.result);
                            w.type_name = e.type_name.filter(|t| !t.is_empty());
                            w.reference = e.variables_reference;
                            w.error = false;
                        }
                        Err(m) => {
                            w.value = m;
                            w.reference = 0;
                            w.error = true;
                        }
                    }
                    w.loading = false;
                    w.children = None;
                    w.expanded = false;
                }
            }
            Target::Vars { kind, path } => {
                let vars: Vec<VarNode> = result
                    .ok()
                    .and_then(|b| serde_json::from_value::<VariablesResponse>(b).ok())
                    .map(|v| v.variables.iter().map(VarNode::from_dap).collect())
                    .unwrap_or_default();
                let tree = d.inspect.tree_mut(&mut d.model, kind);
                if let Some(n) = node_mut(tree, &path) {
                    n.loading = false;
                }
                set_children(tree, &path, vars);
            }
            Target::ThreadTop { thread } => {
                if let Ok(st) = result.and_then(|b| {
                    serde_json::from_value::<StackTraceResponse>(b).map_err(|e| e.to_string())
                }) {
                    let name = st
                        .stack_frames
                        .first()
                        .map(|f| f.name.clone())
                        .unwrap_or_default();
                    d.inspect.tops.insert(thread, name);
                }
            }
        }
        self.refresh_debug(cx);
    }

    // ----- the refresh -----

    /// Push brief 0077's state into the windows (after `refresh_debug_shown` pushed the rest), and react to a new stop
    /// or frame: evaluate Watch 2 to 4 and Autos, ask for the threads' top frames, close the helpers of an old stop.
    pub(super) fn refresh_inspect(&mut self, cx: &mut Context<Self>) {
        let mode = self.debug.model.mode;
        let key = (
            self.debug.generation(),
            self.debug.model.stop,
            self.debug.model.frame,
        );
        let in_break = mode == Mode::Break;
        // The popups of a stop end with it.
        let stop = self.debug.model.stop;
        if !in_break
            || self
                .debug
                .inspect
                .helper
                .as_ref()
                .is_some_and(|h| h.1 != stop)
        {
            self.exception_helper_close(cx);
        }
        if !in_break
            || self
                .debug
                .inspect
                .datatip_panel
                .as_ref()
                .is_some_and(|h| h.1 != stop)
        {
            self.datatip_close(cx);
        }
        if !in_break && let Some(q) = self.debug.inspect.quickwatch_dialog.take() {
            drop(q);
            cx.notify();
        }
        if in_break && self.debug.inspect.seen.map(|s| (s.0, s.1)) != Some((key.0, key.1)) {
            // A new stop: the values so far are the previous stop's.
            self.debug.inspect.prev_values = std::mem::take(&mut self.debug.inspect.cur_values);
            self.debug.inspect.tops.clear();
        }
        if in_break && self.debug.inspect.seen != Some(key) {
            self.debug.inspect.seen = Some(key);
            for kind in [VarsKind::Watch2, VarsKind::Watch3, VarsKind::Watch4] {
                let n = self
                    .debug
                    .inspect
                    .tree_mut(&mut self.debug.model, kind)
                    .len();
                for ix in 0..n {
                    self.debug.inspect_eval(kind, ix);
                }
            }
            self.autos_refresh(cx);
        }
        if !in_break {
            self.debug.inspect.seen = None;
        }
        // Every thread's top frame, once per stop.
        if in_break
            && self.debug.inspect.tops_for != Some((key.0, key.1))
            && !self.debug.model.threads.is_empty()
        {
            self.debug.inspect.tops_for = Some((key.0, key.1));
            let current = self.debug.model.thread;
            let others: Vec<i64> = self
                .debug
                .model
                .threads
                .iter()
                .map(|t| t.id)
                .filter(|t| Some(*t) != current)
                .take(64)
                .collect();
            for thread in others {
                let _ = self.debug.send(
                    "stackTrace",
                    json!({"threadId": thread, "startFrame": 0, "levels": 1}),
                    Pending::Inspect {
                        generation: key.0,
                        stop: key.1,
                        target: Target::ThreadTop { thread },
                    },
                );
            }
        }
        self.inspect_push(cx);
        self.inspect_layout_end(cx);
    }

    fn inspect_push(&mut self, cx: &mut Context<Self>) {
        let d = &mut self.debug;
        let in_break = d.model.mode == Mode::Break;
        let hex = d.inspect.settings.hexadecimal;
        let scope = d
            .model
            .frames
            .get(d.model.frame)
            .map(|f| f.row.name.clone())
            .unwrap_or_default();
        let w = d.windows.clone();
        let note =
            |what: &str| (!in_break).then(|| format!("{what} are shown when debugging breaks."));
        for kind in [
            VarsKind::Locals,
            VarsKind::Watch,
            VarsKind::Watch2,
            VarsKind::Watch3,
            VarsKind::Watch4,
            VarsKind::Autos,
            VarsKind::QuickWatch,
            VarsKind::DataTip,
        ] {
            let nodes = d.inspect.tree_mut(&mut d.model, kind).clone();
            let names = row_names(&nodes);
            let rows = flatten(&nodes);
            let mut changed = Vec::with_capacity(rows.len());
            for ((n, _), r) in names.iter().zip(&rows) {
                let k = value_key(&scope, kind, n);
                let was = d.inspect.prev_values.get(&k);
                changed.push(in_break && !r.error && was.is_some_and(|v| *v != r.value));
                if in_break && !r.value.is_empty() && r.value != "\u{2026}" {
                    d.inspect.cur_values.insert(k, r.value.clone());
                }
            }
            let exprs: Vec<String> = names.into_iter().map(|(_, e)| e).collect();
            let parents = state::flatten_parents(&nodes);
            let view = match kind {
                VarsKind::QuickWatch => d
                    .inspect
                    .quickwatch_dialog
                    .as_ref()
                    .map(|q| q.read(cx).tree.clone()),
                VarsKind::DataTip => d
                    .inspect
                    .datatip_panel
                    .as_ref()
                    .map(|p| p.2.read(cx).tree.clone()),
                _ => w.vars(kind).cloned(),
            };
            let Some(view) = view else { continue };
            let fresh = !matches!(kind, VarsKind::Locals | VarsKind::Watch);
            view.update(cx, |v, cx| {
                if fresh {
                    let n = match kind {
                        VarsKind::Autos => note("Autos"),
                        _ => None,
                    };
                    v.set_parents(parents);
                    v.set_rows(rows, n, cx);
                }
                v.set_marks(changed, exprs, cx);
                v.set_hex(hex, cx);
            });
        }
        // The Call Stack with `[External Code]`, the Threads window with every thread's location.
        let m = &d.model;
        let (rows, frames, targets) =
            stack_rows(&m.frames, m.frame, d.inspect.settings.show_external_code);
        w.call_stack.update(cx, |v, cx| {
            v.set_rows(rows, cx);
            v.set_frames(frames, targets, cx);
        });
        let top = m
            .frames
            .first()
            .map(|f| f.row.name.clone())
            .unwrap_or_default();
        let threads: Vec<ThreadLine> = m
            .threads
            .iter()
            .map(|t| ThreadLine {
                id: t.id,
                name: t.name.clone(),
                location: if Some(t.id) == m.thread {
                    top.clone()
                } else {
                    d.inspect.tops.get(&t.id).cloned().unwrap_or_default()
                },
                current: Some(t.id) == m.thread,
            })
            .collect();
        w.threads.update(cx, |v, cx| v.set_rows(threads, cx));
        let modules = d
            .inspect
            .modules
            .get(&d.active)
            .map(|m| m.rows().to_vec())
            .unwrap_or_default();
        w.modules.update(cx, |v, cx| v.set_rows(&modules, cx));
        w.immediate.update(cx, |v, _| v.set_in_break(in_break));
    }

    // ----- Autos -----

    fn autos_refresh(&mut self, cx: &mut Context<Self>) {
        let d = &self.debug;
        let Some(f) = d.model.frames.get(d.model.frame) else {
            return;
        };
        let (Some(path), Some(line)) = (f.row.path.as_deref(), f.row.line) else {
            self.debug.inspect.autos.clear();
            return;
        };
        let Some(doc) = self.documents.get(&norm(path)) else {
            return;
        };
        let text = doc.view.read(cx).editor().text();
        let lines: Vec<&str> = text.lines().collect();
        let row = line.saturating_sub(1) as usize;
        let mut pick = Vec::new();
        if let Some(p) = super::autos::previous_line(&lines, row) {
            pick.push(lines[p]);
        }
        if let Some(l) = lines.get(row) {
            pick.push(l);
        }
        let exprs = super::autos::expressions(&pick);
        self.debug.inspect.autos = exprs.iter().map(|e| VarNode::watch(e)).collect();
        for ix in 0..exprs.len() {
            self.debug.inspect_eval(VarsKind::Autos, ix);
        }
    }

    // ----- the exception helper -----

    /// The `exceptionInfo` answer arrived: open the helper at the execution point.
    pub(super) fn exception_helper_answer(&mut self, cx: &mut Context<Self>) {
        let m = &self.debug.model;
        let Some(info) = m.exception_info.as_ref() else {
            return;
        };
        let content = HelperContent::new(info, &m.exceptions);
        self.debug.inspect.helper_content = Some((m.stop, content));
        self.exception_helper_open(cx);
    }

    /// Open (or reopen) the helper of the current exception stop.
    pub(super) fn exception_helper_open(&mut self, cx: &mut Context<Self>) {
        let stop = self.debug.model.stop;
        let Some((_, content)) = self
            .debug
            .inspect
            .helper_content
            .clone()
            .filter(|(s, _)| *s == stop)
        else {
            return;
        };
        // The current settings decide the box (they may have changed since).
        let content = match self.debug.model.exception_info.as_ref() {
            Some(info) => HelperContent::new(info, &self.debug.model.exceptions),
            None => content,
        };
        let Some(top) = self.debug.model.frames.first() else {
            return;
        };
        let (Some(path), Some(line)) = (top.row.path.as_deref().map(norm), top.row.line) else {
            return;
        };
        let Some(doc) = self.documents.get(&path) else {
            return;
        };
        let view = doc.view.clone();
        let offset = {
            let b = view.read(cx).editor().buffer();
            let row = line.saturating_sub(1).min(b.line_count().saturating_sub(1));
            b.point_to_offset(eludite_editor::text::Point::new(row, 0))
        };
        let theme = self.theme;
        let helper = match self.debug.inspect.helper.take() {
            Some((_, _, h)) => {
                h.update(cx, |h, cx| h.set_content(content, cx));
                h
            }
            None => {
                let h = cx.new(|_| ExceptionHelperView::new(theme, content));
                cx.subscribe(&h, |shell, _, _: &helper::CloseHelper, cx| {
                    shell.exception_helper_close(cx)
                })
                .detach();
                h
            }
        };
        view.update(cx, |v, cx| {
            v.set_anchored_popup(helper::POPUP, Some((offset, helper.clone().into())), cx)
        });
        self.debug.inspect.helper = Some((path, stop, helper));
    }

    pub(super) fn exception_helper_close(&mut self, cx: &mut Context<Self>) {
        if let Some((doc, _, _)) = self.debug.inspect.helper.take()
            && let Some(d) = self.documents.get(&doc)
        {
            d.view
                .update(cx, |v, cx| v.set_anchored_popup(helper::POPUP, None, cx));
        }
    }

    // ----- DataTips -----

    /// A hover's evaluation answered: a success opens the DataTip tree under the expression; a failure shows no tip.
    pub(super) fn datatip_answer(
        &mut self,
        doc: String,
        id: u64,
        expression: String,
        r: Result<EvaluateResponse, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.documents.get(&doc).map(|d| d.view.clone()) else {
            return;
        };
        let offset = view
            .read(cx)
            .hover()
            .filter(|h| h.id == id)
            .map(|h| h.offset);
        // The Quick Info popup that held the request closes either way.
        view.update(cx, |v, cx| v.set_hover(id, None, None, cx));
        let (Ok(e), Some(offset)) = (r, offset) else {
            return;
        };
        let node = VarNode {
            name: expression,
            value: cmds::null_spelling(e.result),
            type_name: e.type_name.filter(|t| !t.is_empty()),
            reference: e.variables_reference,
            ..VarNode::default()
        };
        self.datatip_close(cx);
        self.debug.inspect.datatip = vec![node];
        let theme = self.theme;
        let tree = cx.new(|cx| VarsWindow::new(theme, VarsKind::DataTip, cx));
        cx.subscribe(&tree, |shell, _, e: &ToggleVariable, cx| {
            shell.inspect_toggle(VarsKind::DataTip, &e.0, cx)
        })
        .detach();
        cx.subscribe(&tree, |shell, _, e: &VarsEvent, cx| {
            shell.inspect_vars_event(VarsKind::DataTip, e, cx)
        })
        .detach();
        let panel = cx.new(|_| DataTipPanel::new(theme, tree));
        view.update(cx, |v, cx| {
            v.set_anchored_popup(datatips::POPUP, Some((offset, panel.clone().into())), cx)
        });
        self.debug.inspect.datatip_panel = Some((doc, self.debug.model.stop, panel));
        self.refresh_debug(cx);
    }

    pub(super) fn datatip_close(&mut self, cx: &mut Context<Self>) {
        if let Some((doc, _, _)) = self.debug.inspect.datatip_panel.take() {
            self.debug.inspect.datatip.clear();
            if let Some(d) = self.documents.get(&doc) {
                d.view
                    .update(cx, |v, cx| v.set_anchored_popup(datatips::POPUP, None, cx));
            }
        }
    }

    // ----- the Immediate window -----

    /// An Immediate line's answer.
    pub(super) fn immediate_answer(
        &mut self,
        r: Result<EvaluateResponse, String>,
        cx: &mut Context<Self>,
    ) {
        let (text, error) = match r {
            Ok(e) => (
                match e.type_name.filter(|t| !t.is_empty()) {
                    Some(t) if e.variables_reference > 0 => {
                        format!("{} ({t})", cmds::null_spelling(e.result))
                    }
                    _ => cmds::null_spelling(e.result),
                },
                false,
            ),
            Err(m) => (m, true),
        };
        self.debug
            .windows
            .immediate
            .update(cx, |w, cx| w.push_result(text, error, cx));
    }

    // ----- QuickWatch -----

    fn quickwatch_open(&mut self, expression: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = match &self.debug.inspect.quickwatch_dialog {
            Some(d) => d.clone(),
            None => {
                let theme = self.theme;
                let tree = cx.new(|cx| VarsWindow::new(theme, VarsKind::QuickWatch, cx));
                cx.subscribe(&tree, |shell, _, e: &ToggleVariable, cx| {
                    shell.inspect_toggle(VarsKind::QuickWatch, &e.0, cx)
                })
                .detach();
                cx.subscribe(&tree, |shell, _, e: &VarsEvent, cx| {
                    shell.inspect_vars_event(VarsKind::QuickWatch, e, cx)
                })
                .detach();
                let d = cx.new(|cx| QuickWatchDialog::new(theme, tree, cx));
                cx.subscribe(&d, |shell, _, _: &CloseQuickWatch, cx| {
                    shell.debug.inspect.quickwatch_dialog = None;
                    cx.notify();
                })
                .detach();
                self.debug.inspect.quickwatch_dialog = Some(d.clone());
                d
            }
        };
        dialog.update(cx, |d, cx| d.show(expression, cx));
        gpui::Focusable::focus_handle(dialog.read(cx), cx).focus(window, cx);
        cx.notify();
    }

    // ----- module events -----

    /// A `module` event of the current session.
    pub(super) fn on_module_event(&mut self, event: &str, body: &Value) {
        let Some(e) = eludite_dap::types::ModuleEvent::decode(event, body) else {
            return;
        };
        let sid = self.debug.session_id;
        let line = self.debug.inspect.modules.entry(sid).or_default().apply(&e);
        if let Some(line) = line {
            self.debug.console_line(line);
        }
    }

    // ----- the glyph -----

    /// The editor's margin events: the glyph's menu and tooltip. True when handled.
    pub(super) fn inspect_on_editor_event(
        &mut self,
        id: &str,
        event: &EditorEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            EditorEvent::BreakpointMarginContext { row, .. } => {
                self.glyph_menu_open(id, row + 1, cx);
                true
            }
            EditorEvent::BreakpointMarginHovered { row } => {
                self.glyph_tip(id, row.map(|r| r + 1), cx);
                true
            }
            _ => false,
        }
    }

    fn breakpoint_row(&self, path: &str, line: u32) -> Option<cmds::BreakpointRow> {
        self.debug
            .model
            .breakpoints
            .rows_for(&self.debug.live_families())
            .into_iter()
            .find(|r| r.path.as_deref().map(norm).as_deref() == Some(path) && r.line == Some(line))
    }

    fn glyph_popups_close(&mut self, keep: &[&str], cx: &mut Context<Self>) {
        if let Some(doc) = self.debug.inspect.glyph_doc.clone()
            && let Some(d) = self.documents.get(&doc)
        {
            d.view.update(cx, |v, cx| {
                for k in [bps::MENU, bps::TOOLTIP, bps::SETTINGS] {
                    if !keep.contains(&k) {
                        v.set_anchored_popup(k, None, cx);
                    }
                }
            });
        }
    }

    fn glyph_offset(
        &self,
        id: &str,
        line: u32,
        cx: &App,
    ) -> Option<(Entity<eludite_editor::EditorView>, usize)> {
        let view = self.documents.get(id)?.view.clone();
        let offset = {
            let b = view.read(cx).editor().buffer();
            b.point_to_offset(eludite_editor::text::Point::new(
                line.saturating_sub(1).min(b.line_count().saturating_sub(1)),
                0,
            ))
        };
        Some((view, offset))
    }

    fn glyph_tip(&mut self, id: &str, line: Option<u32>, cx: &mut Context<Self>) {
        let row = line.and_then(|l| self.breakpoint_row(id, l).map(|r| (l, r)));
        if self.debug.inspect.glyph_tip_row == row.as_ref().map(|r| r.0) {
            return;
        }
        self.glyph_popups_close(&[bps::MENU, bps::SETTINGS], cx);
        self.debug.inspect.glyph_tip_row = row.as_ref().map(|r| r.0);
        let Some((line, r)) = row else {
            return;
        };
        let Some((view, offset)) = self.glyph_offset(id, line, cx) else {
            return;
        };
        let theme = self.theme;
        let tip = cx.new(|_| GlyphTip::new(theme, bps::tooltip_lines(&r)));
        view.update(cx, |v, cx| {
            v.set_anchored_popup(bps::TOOLTIP, Some((offset, tip.into())), cx)
        });
        self.debug.inspect.glyph_doc = Some(id.to_owned());
    }

    /// Open the glyph's menu on the breakpoint at `line` (none: nothing).
    pub(super) fn glyph_menu_open(&mut self, id: &str, line: u32, cx: &mut Context<Self>) {
        let Some(r) = self.breakpoint_row(id, line) else {
            return;
        };
        self.glyph_popups_close(&[], cx);
        self.debug.inspect.glyph_tip_row = None;
        let Some((view, offset)) = self.glyph_offset(id, line, cx) else {
            return;
        };
        let theme = self.theme;
        let path = id.to_owned();
        let menu = cx.new(|_| GlyphMenu::new(theme, path.clone(), line, r.enabled));
        cx.subscribe(&menu, move |shell, _, a: &GlyphAction, cx| {
            shell.glyph_popups_close(&[], cx);
            match a {
                GlyphAction::Conditions => shell.breakpoint_settings_open(&path, line, false, cx),
                GlyphAction::Actions => shell.breakpoint_settings_open(&path, line, true, cx),
                _ => {}
            }
        })
        .detach();
        self.debug.inspect.glyph_menu = Some(menu.clone());
        view.update(cx, |v, cx| {
            v.set_anchored_popup(bps::MENU, Some((offset, menu.into())), cx)
        });
        self.debug.inspect.glyph_doc = Some(id.to_owned());
    }

    /// Open the inline Breakpoint Settings panel under the breakpoint's line.
    pub(super) fn breakpoint_settings_open(
        &mut self,
        id: &str,
        line: u32,
        actions: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.breakpoint_row(id, line) else {
            return;
        };
        let Some((view, offset)) = self.glyph_offset(id, line, cx) else {
            return;
        };
        let theme = self.theme;
        let panel = cx.new(|cx| BreakpointSettings::new(theme, &r, actions, cx));
        cx.subscribe(&panel, |shell, _, _: &bps::CloseSettings, cx| {
            shell.debug.inspect.glyph_settings = None;
            shell.glyph_popups_close(&[], cx)
        })
        .detach();
        self.debug.inspect.glyph_settings = Some(panel.clone());
        view.update(cx, |v, cx| {
            v.set_anchored_popup(bps::SETTINGS, Some((offset, panel.into())), cx)
        });
        self.debug.inspect.glyph_doc = Some(id.to_owned());
    }

    // ----- the Design and Debug layouts -----

    /// A session starts from design mode: keep the Design layout and apply the Debug one when there is one. True when it
    /// was applied (the default debug windows are then not shown).
    pub(super) fn inspect_layout_start(&mut self) -> bool {
        let i = &mut self.debug.inspect;
        if i.in_debug_layout {
            return false;
        }
        let current = self.controller.layout();
        i.design_layout = Some(current.clone());
        i.in_debug_layout = true;
        match i.debug_layout.clone() {
            Some(mut l) => {
                l.documents = current.documents;
                self.controller.replace_layout(l);
                true
            }
            None => false,
        }
    }

    /// The last session ended: keep the Debug layout and apply the Design one.
    fn inspect_layout_end(&mut self, cx: &mut Context<Self>) {
        if !self.debug.inspect.in_debug_layout
            || !self.debug.live_ids().is_empty()
            || self.debug.model.mode != Mode::Design
        {
            return;
        }
        let i = &mut self.debug.inspect;
        i.in_debug_layout = false;
        let current = self.controller.layout();
        i.debug_layout = Some(current.clone());
        if let Some(mut l) = i.design_layout.take() {
            l.documents = current.documents;
            self.controller.replace_layout(l);
            cx.notify();
        }
    }
}
