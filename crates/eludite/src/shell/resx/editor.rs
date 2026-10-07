//! The `.resx` editor as a document tab (proposal 0005): a resource set's grid, a row per key with the Key and
//! Comment columns and one column per culture (the neutral culture first), edited in place; the toolbar with Add
//! Key, Delete, Rename, the Missing, Warnings and Invariant filters, the search box, the Access Modifier list and
//! Save; a status line with the counts and the selected cell's warning; and Other Resources, the non-string entries,
//! listed read-only under a collapsed section.
//!
//! The view owns the set ([`SetModel`]) once the shell loaded it off the UI thread; an edit changes the model and
//! marks the tab dirty; Save (Ctrl+S, the button) asks the shell to write the dirty files through the workspace-edit
//! applier. A missing cell is tinted, a cell with a rule warning carries a glyph, an invariant key is dimmed in the
//! culture columns.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::ops::Range;

use eludite_commands::resx::AccessModifier;
use eludite_lsp::host;
use eludite_resx::Row;
use eludite_ui::{BoundsMap, Theme, push_button, text_box, toggle_button, toolbar_button};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, ScrollStrategy,
    SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div, px,
    uniform_list,
};

use super::model::SetModel;

pub const ROW_HEIGHT: f32 = 22.;
const KEY_WIDTH: f32 = 220.;
const COMMENT_WIDTH: f32 = 200.;

// Debug selectors.
pub const ADD_KEY: &str = "resx-add-key";
pub const DELETE: &str = "resx-delete";
pub const RENAME: &str = "resx-rename";
pub const FILTER_MISSING: &str = "resx-filter-missing";
pub const FILTER_WARNINGS: &str = "resx-filter-warnings";
pub const FILTER_INVARIANT: &str = "resx-filter-invariant";
pub const SEARCH_BOX: &str = "resx-search";
pub const SAVE: &str = "resx-save";
pub const OTHERS: &str = "resx-others";
pub const NEW_KEY: &str = "resx-new-key";

pub fn modifier_selector(modifier: AccessModifier) -> String {
    format!("resx-modifier-{}", modifier.as_str())
}

/// A value on one line: a line break becomes a return mark.
fn one_line(text: &str) -> String {
    if text.contains(['\n', '\r']) {
        text.replace("\r\n", " \u{21B5} ")
            .replace(['\n', '\r'], " \u{21B5} ")
    } else {
        text.to_owned()
    }
}

/// A cell's selector: the row's index among the visible rows and its column.
pub fn cell_selector(ix: usize, column: &Column) -> String {
    format!("resx-cell-{ix}-{}", column.selector_part())
}

/// A column of the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Column {
    Key,
    Comment,
    /// The empty string is the neutral culture.
    Culture(String),
}

impl Column {
    fn selector_part(&self) -> String {
        match self {
            Column::Key => "key".into(),
            Column::Comment => "comment".into(),
            Column::Culture(c) if c.is_empty() => "neutral".into(),
            Column::Culture(c) => c.clone(),
        }
    }
}

/// What the editor asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEvent {
    /// The tab became dirty or clean.
    Dirty(bool),
    /// Save the dirty files.
    Save,
    /// The Access Modifier list: `eludite.resx.access_modifier`.
    Modifier(AccessModifier),
}

struct Editing {
    /// The visible row, or `None` for the new key being added.
    row: Option<usize>,
    column: Column,
    text: String,
}

#[derive(Default)]
struct Filter {
    missing: bool,
    warnings: bool,
    invariant: bool,
    query: String,
}

pub struct ResxEditor {
    theme: Theme,
    pub tab: String,
    pub title: String,
    pub model: Option<SetModel>,
    pub error: Option<String>,
    /// What the host knows of the set (the neutral language, the designer, the modifier), when it lists it.
    pub info: Option<host::ResxSet>,
    pub message: Option<String>,
    /// Indexes into the model's rows that pass the filters.
    visible: Vec<usize>,
    filter: Filter,
    /// The selected cell: a visible row and a column.
    cursor: Option<(usize, Column)>,
    editing: Option<Editing>,
    hidden_cultures: BTreeSet<String>,
    show_others: bool,
    /// Keys, missing cells and warnings over the whole set, and the non-string entries, computed when the model or
    /// the filters change (never per frame).
    counts: (usize, usize, usize),
    others_count: usize,
    pub probe: Option<BoundsMap>,
    rows_drawn: Cell<usize>,
    focus: FocusHandle,
    search_focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<EditorEvent> for ResxEditor {}

impl Focusable for ResxEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ResxEditor {
    pub fn new(theme: Theme, tab: String, title: String, cx: &mut Context<Self>) -> Self {
        ResxEditor {
            theme,
            tab,
            title,
            model: None,
            error: None,
            info: None,
            message: None,
            visible: Vec::new(),
            filter: Filter::default(),
            cursor: None,
            editing: None,
            hidden_cultures: BTreeSet::new(),
            show_others: false,
            counts: (0, 0, 0),
            others_count: 0,
            probe: None,
            rows_drawn: Cell::new(0),
            focus: cx.focus_handle(),
            search_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.model.as_ref().is_some_and(SetModel::is_dirty)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows_drawn(&self) -> usize {
        self.rows_drawn.get()
    }

    /// The set arrived (or failed to load).
    pub fn set_model(&mut self, model: Result<SetModel, String>, cx: &mut Context<Self>) {
        match model {
            Ok(m) => {
                self.model = Some(m);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.refilter();
        cx.notify();
    }

    pub fn set_info(&mut self, info: Option<host::ResxSet>, cx: &mut Context<Self>) {
        self.info = info;
        cx.notify();
    }

    pub fn set_message(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.message = Some(message.into());
        cx.notify();
    }

    /// The files were written.
    pub fn saved(
        &mut self,
        writes: &[super::model::FileWrite],
        message: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = self.model.as_mut() {
            m.saved(writes);
        }
        self.message = Some(message);
        self.refilter();
        cx.emit(EditorEvent::Dirty(self.is_dirty()));
        cx.notify();
    }

    /// The cultures shown, in the grid's order.
    pub fn columns(&self) -> Vec<String> {
        self.model
            .as_ref()
            .map(|m| {
                m.cultures()
                    .into_iter()
                    .filter(|c| !self.hidden_cultures.contains(c))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The visible rows (the filters applied).
    pub fn visible_rows(&self) -> Vec<&Row> {
        let Some(m) = self.model.as_ref() else {
            return Vec::new();
        };
        self.visible
            .iter()
            .filter_map(|ix| m.rows().get(*ix))
            .collect()
    }

    /// The visible row at `ix`.
    fn visible_row(&self, ix: usize) -> Option<&Row> {
        let m = self.model.as_ref()?;
        m.rows().get(*self.visible.get(ix)?)
    }

    pub fn selected_key(&self) -> Option<String> {
        let (ix, _) = self.cursor.as_ref()?;
        self.visible_row(*ix).map(|r| r.key.clone())
    }

    fn refilter(&mut self) {
        let Some(m) = self.model.as_ref() else {
            self.visible.clear();
            self.counts = (0, 0, 0);
            self.others_count = 0;
            return;
        };
        let rows = m.rows();
        self.counts = (
            rows.len(),
            rows.iter().map(Row::missing_count).sum(),
            rows.iter().map(Row::warning_count).sum(),
        );
        self.others_count = m.others().len();
        let q = self.filter.query.to_lowercase();
        self.visible = m
            .rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                (!self.filter.missing || r.missing_count() > 0)
                    && (!self.filter.warnings || r.warning_count() > 0)
                    && (!self.filter.invariant || r.invariant)
                    && (q.is_empty()
                        || r.key.to_lowercase().contains(&q)
                        || r.cells.iter().any(|c| {
                            c.value
                                .as_deref()
                                .is_some_and(|v| v.to_lowercase().contains(&q))
                                || c.comment
                                    .as_deref()
                                    .is_some_and(|v| v.to_lowercase().contains(&q))
                        }))
            })
            .map(|(ix, _)| ix)
            .collect();
        if let Some((ix, _)) = &self.cursor
            && *ix >= self.visible.len()
        {
            self.cursor = None;
        }
    }

    /// The model changed under the editor (the shell applied a command to it).
    pub fn set_model_changed(&mut self, cx: &mut Context<Self>) {
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let was = self.is_dirty();
        self.refilter();
        cx.emit(EditorEvent::Dirty(self.is_dirty()));
        if !was && self.is_dirty() {
            self.message = None;
        }
        cx.notify();
    }

    // ---------------------------------------------------------------- edits

    fn start_editing(
        &mut self,
        row: Option<usize>,
        column: Column,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = match (row, &column) {
            (None, _) => String::new(),
            (Some(ix), Column::Key) => self
                .visible_rows()
                .get(ix)
                .map(|r| r.key.clone())
                .unwrap_or_default(),
            (Some(ix), Column::Comment) => self
                .visible_rows()
                .get(ix)
                .and_then(|r| r.cell("").and_then(|c| c.comment.clone()))
                .map(|c| eludite_resx::with_invariant(Some(&c), false).unwrap_or_default())
                .unwrap_or_default(),
            (Some(ix), Column::Culture(c)) => self
                .visible_rows()
                .get(ix)
                .and_then(|r| r.cell(c).and_then(|cell| cell.value.clone()))
                .unwrap_or_default(),
        };
        if let Some(ix) = row {
            self.cursor = Some((ix, column.clone()));
        }
        self.editing = Some(Editing { row, column, text });
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Commit the text being edited (Enter).
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(Editing { row, column, text }) = self.editing.take() else {
            return;
        };
        let key = match row {
            Some(ix) => match self.visible_rows().get(ix) {
                Some(r) => r.key.clone(),
                None => return,
            },
            None => {
                let key = text.trim().to_owned();
                if key.is_empty() {
                    cx.notify();
                    return;
                }
                let result = self
                    .model
                    .as_mut()
                    .map(|m| m.add_key(&key, "", None, false));
                match result {
                    Some(Ok((eludite_commands::resx::AddStatus::Exists, _))) => {
                        self.message = Some(format!("{key} already exists."));
                    }
                    Some(Ok(_)) => {
                        self.changed(cx);
                        if let Some(ix) = self.visible_rows().iter().position(|r| r.key == key) {
                            self.cursor = Some((ix, Column::Culture(String::new())));
                            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
                        }
                    }
                    Some(Err(e)) => self.message = Some(e),
                    None => {}
                }
                cx.notify();
                return;
            }
        };
        let Some(m) = self.model.as_mut() else {
            return;
        };
        let outcome = match column {
            Column::Key => {
                let new_key = text.trim().to_owned();
                if new_key.is_empty() || new_key == key {
                    Ok(())
                } else {
                    m.rename_key(&key, &new_key).map(|(status, _)| {
                        if status == eludite_commands::resx::RenameStatus::Exists {
                            self.message = Some(format!("{new_key} already exists."));
                        }
                    })
                }
            }
            Column::Comment => m.set_comment(&key, "", Some(&text)).map(|_| ()),
            Column::Culture(c) => m.set_value(&key, &c, Some(&text), false).map(|_| ()),
        };
        if let Err(e) = outcome {
            self.message = Some(e);
        }
        self.changed(cx);
    }

    /// Delete the selected key (the toolbar's Delete, the Delete key).
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else {
            return;
        };
        if let Some(m) = self.model.as_mut()
            && let Err(e) = m.remove_keys(&[key])
        {
            self.message = Some(e);
        }
        self.changed(cx);
    }

    pub fn toggle_invariant_selected(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else {
            return;
        };
        if let Some(m) = self.model.as_mut() {
            let on = !m.row(&key).is_some_and(|r| r.invariant);
            if let Err(e) = m.set_invariant(&key, on) {
                self.message = Some(e);
            }
        }
        self.changed(cx);
    }

    /// Start adding a key (the toolbar's Add Key): the new row's key box.
    pub fn add_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_editing(None, Column::Key, window, cx);
        let count = self.visible.len();
        self.scroll.scroll_to_item(count, ScrollStrategy::Center);
    }

    /// Rename the selected key (the toolbar's Rename, F2).
    pub fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((ix, _)) = self.cursor.clone() {
            self.start_editing(Some(ix), Column::Key, window, cx);
        }
    }

    /// Type into the text being edited (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(e) = self.editing.as_mut() {
            e.text.push_str(text);
            cx.notify();
        }
    }

    /// Type into the search box (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_query(&mut self, text: &str, cx: &mut Context<Self>) {
        self.filter.query.push_str(text);
        self.refilter();
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_filter(
        &mut self,
        missing: bool,
        warnings: bool,
        invariant: bool,
        cx: &mut Context<Self>,
    ) {
        self.filter.missing = missing;
        self.filter.warnings = warnings;
        self.filter.invariant = invariant;
        self.refilter();
        cx.notify();
    }

    /// Select a visible row's cell (tests, the shell's reveal).
    pub fn select(&mut self, ix: usize, column: Column, cx: &mut Context<Self>) {
        if ix < self.visible.len() {
            self.cursor = Some((ix, column));
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn cell_click(
        &mut self,
        ix: usize,
        column: Column,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor = Some((ix, column.clone()));
        if click_count >= 2 {
            self.start_editing(Some(ix), column, window, cx);
        } else {
            self.editing = None;
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        if let Some(e) = self.editing.as_mut() {
            match k.key.as_str() {
                "escape" => self.editing = None,
                "enter" => self.commit(cx),
                "backspace" => {
                    e.text.pop();
                }
                _ => {
                    let typed = k.key_char.clone().or_else(|| {
                        (k.key.chars().count() == 1).then(|| {
                            if k.modifiers.shift {
                                k.key.to_uppercase()
                            } else {
                                k.key.clone()
                            }
                        })
                    });
                    match typed {
                        Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                            e.text.push_str(&c)
                        }
                        _ => return,
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some((ix, column)) = self.cursor.clone() else {
            return;
        };
        match k.key.as_str() {
            "enter" => self.start_editing(Some(ix), column, window, cx),
            "f2" => self.start_editing(Some(ix), Column::Key, window, cx),
            "delete" => self.delete_selected(cx),
            "down" => self.select(ix + 1, column, cx),
            "up" => self.select(ix.saturating_sub(1), column, cx),
            "backspace" | "escape" | "tab" | "left" | "right" | "space"
                if column == Column::Key =>
            {
                return;
            }
            _ => {
                // Typing on a selected value or comment replaces it, as a grid does; the key column needs F2.
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c)
                        if column != Column::Key
                            && !c.is_empty()
                            && !c.chars().any(char::is_control) =>
                    {
                        self.start_editing(Some(ix), column, window, cx);
                        if let Some(e) = self.editing.as_mut() {
                            e.text = c;
                        }
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn search_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "backspace" => {
                self.filter.query.pop();
            }
            "escape" => self.filter.query.clear(),
            "space" => self.filter.query.push(' '),
            "enter" => {}
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        self.filter.query.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        self.refilter();
        cx.stop_propagation();
        cx.notify();
    }

    // -------------------------------------------------------------- drawing

    fn probed<E: ParentElement + Styled + IntoElement>(
        &self,
        el: E,
        key: &str,
    ) -> gpui::AnyElement {
        super::super::project_properties::pages::probed(self.probe.as_ref(), el, key.to_owned())
    }

    fn cell(&self, text: impl Into<SharedString>, width: Option<f32>) -> gpui::Div {
        // No ellipsis: truncating costs a second shaping of every cell's text, and the grid draws two hundred a frame.
        let c = div()
            .px_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(text.into());
        match width {
            Some(w) => c.flex_none().w(px(w)),
            None => c.flex_1().min_w(px(100.)),
        }
    }

    fn render_rows(
        &self,
        range: Range<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let columns = self.columns();
        let neutral_label = self.neutral_label();
        let mut out = Vec::new();
        for ix in range {
            if ix == self.visible.len() {
                // The key being added.
                if let Some(e) = self.editing.as_ref().filter(|e| e.row.is_none()) {
                    out.push(
                        div()
                            .w_full()
                            .h(px(ROW_HEIGHT))
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(
                                text_box(NEW_KEY, &e.text, "New key name", focused, &t)
                                    .w(px(KEY_WIDTH)),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .text_color(t.text_muted)
                                    .child("Enter adds the key"),
                            )
                            .into_any_element(),
                    );
                }
                continue;
            }
            let Some(row) = self.visible_row(ix) else {
                continue;
            };
            self.rows_drawn.set(self.rows_drawn.get() + 1);
            let mut el = div()
                .w_full()
                .h(px(ROW_HEIGHT))
                .flex()
                .flex_row()
                .items_center()
                .text_size(t.typography.ui)
                .border_b_1()
                .border_color(t.border);
            let mut cells: Vec<gpui::AnyElement> = Vec::new();
            let key_cell = self.grid_cell(
                ix,
                Column::Key,
                &row.key,
                false,
                false,
                Some(KEY_WIDTH),
                focused,
                cx,
            );
            cells.push(key_cell);
            let comment = row
                .cell("")
                .and_then(|c| c.comment.clone())
                .map(|c| eludite_resx::with_invariant(Some(&c), false).unwrap_or_default())
                .unwrap_or_default();
            let comment = one_line(&comment);
            let comment_text = if row.invariant {
                format!("{{Invariant}} {comment}")
            } else {
                comment
            };
            cells.push(self.grid_cell(
                ix,
                Column::Comment,
                &comment_text,
                false,
                false,
                Some(COMMENT_WIDTH),
                focused,
                cx,
            ));
            for culture in &columns {
                let c = row.cell(culture);
                let (text, missing, warning) = match c {
                    Some(c) => (
                        c.value.clone().unwrap_or_default(),
                        c.missing(),
                        !c.warnings.is_empty(),
                    ),
                    None => (String::new(), true, false),
                };
                let label = if culture.is_empty() {
                    neutral_label.clone()
                } else {
                    culture.clone()
                };
                let _ = label;
                let dim = row.invariant && !culture.is_empty();
                // A value's line breaks show as a mark: the row is one line tall.
                let text = one_line(&text);
                let shown = if warning {
                    format!("\u{26A0} {text}")
                } else {
                    text
                };
                let el = self.grid_cell(
                    ix,
                    Column::Culture(culture.clone()),
                    &shown,
                    missing && !row.invariant,
                    dim,
                    None,
                    focused,
                    cx,
                );
                cells.push(el);
            }
            el = el.children(cells);
            out.push(el.into_any_element());
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn grid_cell(
        &self,
        ix: usize,
        column: Column,
        text: &str,
        missing: bool,
        dim: bool,
        width: Option<f32>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let selected = self
            .cursor
            .as_ref()
            .is_some_and(|(r, c)| *r == ix && *c == column);
        let editing = self
            .editing
            .as_ref()
            .filter(|e| e.row == Some(ix) && e.column == column)
            .map(|e| e.text.clone());
        let selector = cell_selector(ix, &column);
        if let Some(text) = editing {
            let el = text_box(selector.clone(), &text, "", focused, &t);
            let el = match width {
                Some(w) => el.flex_none().w(px(w)),
                None => el.flex_1().min_w(px(100.)),
            };
            return el.into_any_element();
        }
        // A plain element with a mouse-down listener, not a stateful one: the grid draws some two hundred cells a
        // frame, and per-element state would cost the frame budget.
        let mut el = self
            .cell(text.to_owned(), width)
            .debug_selector(move || selector)
            .h_full()
            .flex()
            .items_center()
            .cursor_pointer()
            .border_r_1()
            .border_color(t.border);
        if selected {
            el = el.bg(t.accent).text_color(t.text_on_accent);
        } else if missing {
            el = el.bg(gpui::rgba(0xE5A50A33)).text_color(t.text);
        } else if dim {
            el = el.text_color(t.text_disabled);
        } else {
            el = el.text_color(t.text);
        }
        let col = column.clone();
        let el = el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                this.cell_click(ix, col.clone(), e.click_count, window, cx)
            }),
        );
        // The first rows' cells are probed for the Xvfb run (`--bounds-out`); the rest stay plain.
        if self.probe.is_some() && ix < 8 {
            return self.probed(el, &cell_selector(ix, &column));
        }
        el.into_any_element()
    }

    fn neutral_label(&self) -> String {
        self.info
            .as_ref()
            .and_then(|i| i.neutral_language.clone())
            .map(|l| format!("Neutral ({l})"))
            .unwrap_or_else(|| "Neutral".into())
    }

    fn status_text(&self) -> String {
        if let Some(e) = &self.error {
            return format!("Cannot open: {e}");
        }
        if self.model.is_none() {
            return "Loading\u{2026}".into();
        }
        let (keys, missing, warnings) = self.counts;
        let mut s = format!("{keys} keys, {missing} missing, {warnings} warnings");
        if let Some((ix, Column::Culture(c))) = &self.cursor
            && let Some(row) = self.visible_row(*ix)
            && let Some(cell) = row.cell(c)
            && let Some(w) = cell.warnings.first()
        {
            s.push_str(&format!(". {}", w.message));
        }
        if let Some(m) = &self.message {
            s.push_str(&format!(". {m}"));
        }
        s
    }

    fn modifier(&self) -> AccessModifier {
        match self.info.as_ref().map(|i| i.access_modifier) {
            Some(host::AccessModifier::Internal) => AccessModifier::Internal,
            Some(host::AccessModifier::Public) => AccessModifier::Public,
            _ => AccessModifier::None,
        }
    }
}

impl Render for ResxEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.rows_drawn.set(0);
        let has_selection = self.cursor.is_some();
        let loaded = self.model.is_some();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .pt_2()
            .child(div().text_size(px(18.)).child(self.title.clone()))
            .children(self.info.as_ref().map(|i| {
                div()
                    .text_color(t.text_muted)
                    .child(format!("{} \u{2014} {}", i.project_name, i.namespace))
            }))
            .child(div().flex_1())
            .child(
                self.probed(
                    push_button(SAVE, "Save (Ctrl+S)", false, self.is_dirty(), &t)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::Save))),
                    SAVE,
                ),
            );
        let search_focused = self.search_focus.is_focused(window);
        let search = text_box(
            SEARCH_BOX,
            &self.filter.query,
            "Search keys and values",
            search_focused,
            &t,
        )
        .w(px(220.))
        .track_focus(&self.search_focus)
        .key_context("ResxSearch")
        .on_key_down(cx.listener(Self::search_key))
        .on_click(cx.listener(|this, _, window, cx| {
            this.search_focus.focus(window, cx);
            cx.notify();
        }));
        let modifier = self.modifier();
        let mut toolbar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .child(
                self.probed(
                    toolbar_button(ADD_KEY, "Add Key", loaded, &t)
                        .on_click(cx.listener(|this, _, window, cx| this.add_key(window, cx))),
                    ADD_KEY,
                ),
            )
            .child(
                self.probed(
                    toolbar_button(DELETE, "Delete", has_selection, &t)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))),
                    DELETE,
                ),
            )
            .child(
                self.probed(
                    toolbar_button(RENAME, "Rename (F2)", has_selection, &t).on_click(
                        cx.listener(|this, _, window, cx| this.rename_selected(window, cx)),
                    ),
                    RENAME,
                ),
            )
            .child(
                self.probed(
                    toolbar_button("resx-invariant", "Invariant", has_selection, &t)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_invariant_selected(cx))),
                    "resx-invariant",
                ),
            )
            .child(div().w(px(8.)))
            .child(self.probed(
                toggle_button(FILTER_MISSING, "Missing", self.filter.missing, &t).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filter.missing = !this.filter.missing;
                        this.refilter();
                        cx.notify();
                    }),
                ),
                FILTER_MISSING,
            ))
            .child(self.probed(
                toggle_button(FILTER_WARNINGS, "Warnings", self.filter.warnings, &t).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filter.warnings = !this.filter.warnings;
                        this.refilter();
                        cx.notify();
                    }),
                ),
                FILTER_WARNINGS,
            ))
            .child(self.probed(
                toggle_button(FILTER_INVARIANT, "Invariant", self.filter.invariant, &t).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.filter.invariant = !this.filter.invariant;
                        this.refilter();
                        cx.notify();
                    }),
                ),
                FILTER_INVARIANT,
            ))
            .child(search)
            .child(div().flex_1())
            .child(div().text_color(t.text_muted).child("Access Modifier:"));
        for m in [
            AccessModifier::Internal,
            AccessModifier::Public,
            AccessModifier::None,
        ] {
            let label = match m {
                AccessModifier::Internal => "Internal",
                AccessModifier::Public => "Public",
                AccessModifier::None => "No code generation",
            };
            let sel = modifier_selector(m);
            toolbar =
                toolbar.child(self.probed(
                    toggle_button(sel.clone(), label, modifier == m, &t).on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(EditorEvent::Modifier(m))),
                    ),
                    &sel,
                ));
        }
        let columns = self.columns();
        let neutral_label = self.neutral_label();
        let mut heading = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .text_size(t.typography.small)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text_muted)
            .child(self.cell("Key", Some(KEY_WIDTH)))
            .child(self.cell("Comment", Some(COMMENT_WIDTH)));
        for c in &columns {
            let label = if c.is_empty() {
                neutral_label.clone()
            } else {
                c.clone()
            };
            heading = heading.child(self.cell(label, None));
        }
        let count = self.visible.len()
            + usize::from(self.editing.as_ref().is_some_and(|e| e.row.is_none()));
        let others_count = self.others_count;
        let others = if self.show_others {
            self.model
                .as_ref()
                .map(SetModel::others)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let others_section = div()
            .flex()
            .flex_col()
            .flex_none()
            .border_t_1()
            .border_color(t.border)
            .child(
                toolbar_button(
                    OTHERS,
                    format!("Other Resources ({others_count})"),
                    others_count > 0,
                    &t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_others = !this.show_others;
                    cx.notify();
                })),
            )
            .children(self.show_others.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .px_2()
                    .pb_1()
                    .text_color(t.text_muted)
                    .children(
                        others
                            .iter()
                            .map(|(name, kind)| div().child(format!("{name}: {kind} (read-only)"))),
                    )
            }));
        div()
            .id(SharedString::from(format!("resx-{}", self.tab)))
            .debug_selector(|| "resx-editor".into())
            .track_focus(&self.focus)
            .key_context("ResxEditor")
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_color(t.text_muted)
                    .text_size(t.typography.small)
                    .child(self.status_text()),
            )
            .child(heading)
            .child(
                uniform_list(
                    "resx-rows",
                    count,
                    cx.processor(move |this, range: Range<usize>, window, cx| {
                        this.render_rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(others_section)
    }
}
