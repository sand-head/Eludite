//! A property grid (brief 0081): Visual Studio's Properties window body, reusable by anything that shows named values.
//!
//! [`PropertyGrid`] is an entity holding a title (what the values belong to: `Program.cs  File Properties`), rows
//! grouped by category (categories in the order the rows first name them, rows in the order given), the selected
//! row, an open drop-down and an in-place text edit. The caller owns the values: it gives rows with
//! [`PropertyGrid::set_rows`] and hears [`PropertyGridEvent::Changed`] when the person picks a choice or commits an
//! edit (Enter or a click elsewhere commits, Escape cancels); it shows the new value once it set the rows again.
//! The bottom pane describes the selected row, as Visual Studio's does.
//!
//! Debug selectors: `pg-row-<name>`, `pg-value-<name>` (click: open the drop-down, or start editing a text value),
//! `pg-choice-<name>-<value>` (the drop-down's entries), `pg-edit-<name>` (the box being edited), `pg-description`.

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};

use crate::Theme;

/// A row's value and how it is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyValue {
    /// Read only, drawn muted.
    ReadOnly(String),
    /// Free text, edited in place.
    Text(String),
    /// One of `choices` (value, label); the row shows the label of `value`.
    Choice {
        value: String,
        choices: Vec<(String, String)>,
    },
}

impl PropertyValue {
    /// What the row shows.
    pub fn display(&self) -> String {
        match self {
            PropertyValue::ReadOnly(v) | PropertyValue::Text(v) => v.clone(),
            PropertyValue::Choice { value, choices } => choices
                .iter()
                .find(|(v, _)| v == value)
                .map(|(_, l)| l.clone())
                .unwrap_or_else(|| value.clone()),
        }
    }

    pub fn editable(&self) -> bool {
        !matches!(self, PropertyValue::ReadOnly(_))
    }
}

/// One row of the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyRow {
    pub category: String,
    /// The key the caller hears back.
    pub name: String,
    pub label: String,
    pub value: PropertyValue,
    pub description: String,
}

impl PropertyRow {
    pub fn read_only(
        category: &str,
        name: &str,
        label: &str,
        value: &str,
        description: &str,
    ) -> Self {
        Self {
            category: category.into(),
            name: name.into(),
            label: label.into(),
            value: PropertyValue::ReadOnly(value.into()),
            description: description.into(),
        }
    }
}

/// What the grid tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyGridEvent {
    /// The person picked a choice or committed an edit that differs from the row's value.
    Changed { name: String, value: String },
}

/// A line of the grid: a category heading or a row (an index into the rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridLine {
    Category(String),
    Row(usize),
}

/// The lines for `rows`: each category once, in first-seen order, with its rows in order.
pub fn grid_lines(rows: &[PropertyRow]) -> Vec<GridLine> {
    let mut categories: Vec<&str> = Vec::new();
    for r in rows {
        if !categories.contains(&r.category.as_str()) {
            categories.push(&r.category);
        }
    }
    let mut out = Vec::new();
    for c in categories {
        out.push(GridLine::Category(c.to_owned()));
        out.extend(
            rows.iter()
                .enumerate()
                .filter(|(_, r)| r.category == c)
                .map(|(i, _)| GridLine::Row(i)),
        );
    }
    out
}

/// Visual Studio's Properties window body.
pub struct PropertyGrid {
    theme: Theme,
    title: String,
    rows: Vec<PropertyRow>,
    selected: Option<String>,
    /// The row whose drop-down is open.
    open_choice: Option<String>,
    /// The row being edited and the text so far.
    editing: Option<(String, String)>,
    focus: FocusHandle,
}

impl EventEmitter<PropertyGridEvent> for PropertyGrid {}

impl Focusable for PropertyGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PropertyGrid {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            title: String::new(),
            rows: Vec::new(),
            selected: None,
            open_choice: None,
            editing: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Show `rows` under `title`. The selection stays on a row of the same name; an open drop-down or edit of a row
    /// whose value changed is closed.
    pub fn set_rows(
        &mut self,
        title: impl Into<String>,
        rows: Vec<PropertyRow>,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        if title != self.title {
            self.selected = None;
            self.open_choice = None;
            self.editing = None;
        }
        self.title = title;
        let still = |n: &Option<String>| {
            n.as_ref()
                .is_some_and(|n| rows.iter().any(|r| &r.name == n && r.value.editable()))
        };
        if !still(&self.open_choice) {
            self.open_choice = None;
        }
        if !still(&self.editing.as_ref().map(|(n, _)| n.clone())) {
            self.editing = None;
        }
        if let Some(s) = &self.selected
            && !rows.iter().any(|r| &r.name == s)
        {
            self.selected = None;
        }
        self.rows = rows;
        cx.notify();
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn rows(&self) -> &[PropertyRow] {
        &self.rows
    }

    pub fn row(&self, name: &str) -> Option<&PropertyRow> {
        self.rows.iter().find(|r| r.name == name)
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn open_choice(&self) -> Option<&str> {
        self.open_choice.as_deref()
    }

    /// The row being edited and its text.
    pub fn editing(&self) -> Option<(&str, &str)> {
        self.editing.as_ref().map(|(n, t)| (n.as_str(), t.as_str()))
    }

    pub fn select(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.selected.as_deref() != Some(name) {
            self.commit(cx);
            self.open_choice = None;
        }
        self.selected = Some(name.to_owned());
        cx.notify();
    }

    /// A click on a row's value: open (or close) its drop-down, or start editing its text.
    pub fn activate(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.select(name, cx);
        let Some(row) = self.row(name).cloned() else {
            return;
        };
        match row.value {
            PropertyValue::Choice { .. } => {
                self.open_choice = if self.open_choice.as_deref() == Some(name) {
                    None
                } else {
                    Some(name.to_owned())
                };
            }
            PropertyValue::Text(v) => {
                if self.editing.as_ref().is_none_or(|(n, _)| n != name) {
                    self.editing = Some((name.to_owned(), v));
                }
                window.focus(&self.focus, cx);
            }
            PropertyValue::ReadOnly(_) => {}
        }
        cx.notify();
    }

    /// Pick `value` of row `name`'s choices.
    pub fn choose(&mut self, name: &str, value: &str, cx: &mut Context<Self>) {
        self.open_choice = None;
        let current = self.row(name).map(|r| match &r.value {
            PropertyValue::Choice { value, .. } => value.clone(),
            other => other.display(),
        });
        if current.as_deref() != Some(value) {
            cx.emit(PropertyGridEvent::Changed {
                name: name.to_owned(),
                value: value.to_owned(),
            });
        }
        cx.notify();
    }

    /// End the edit, telling the owner when the text changed.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        if let Some((name, text)) = self.editing.take() {
            let current = self.row(&name).map(|r| r.value.display());
            if current.as_deref() != Some(text.as_str()) {
                cx.emit(PropertyGridEvent::Changed { name, value: text });
            }
            cx.notify();
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.open_choice = None;
        cx.notify();
    }

    /// Type into the edit (tests and the key handler).
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some((_, t)) = &mut self.editing {
            t.push_str(text);
            cx.notify();
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            return;
        }
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "enter" => self.commit(cx),
            "escape" => self.cancel(cx),
            "backspace" => {
                if let Some((_, t)) = &mut self.editing {
                    t.pop();
                    cx.notify();
                }
            }
            _ if !ks.modifiers.control && !ks.modifiers.platform && !ks.modifiers.alt => {
                if let Some(ch) = &ks.key_char {
                    self.type_text(ch, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for PropertyGrid {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut lines = Vec::new();
        for line in grid_lines(&self.rows) {
            match line {
                GridLine::Category(c) => lines.push(
                    div()
                        .flex()
                        .items_center()
                        .h(px(20.))
                        .px_1()
                        .bg(t.panel_header)
                        .text_color(t.panel_header_text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(c)
                        .into_any_element(),
                ),
                GridLine::Row(i) => {
                    let row = self.rows[i].clone();
                    let name = row.name.clone();
                    let selected = self.selected.as_deref() == Some(name.as_str());
                    let editing = self
                        .editing
                        .as_ref()
                        .filter(|(n, _)| *n == name)
                        .map(|(_, t)| t.clone());
                    let value_text = match &editing {
                        Some(text) => format!("{text}\u{2502}"),
                        None => match &row.value {
                            PropertyValue::Choice { .. } => {
                                format!("{}  \u{25BE}", row.value.display())
                            }
                            v => v.display(),
                        },
                    };
                    let n1 = name.clone();
                    let n2 = name.clone();
                    let mut value = div()
                        .id(SharedString::from(format!("pg-value-{name}")))
                        .debug_selector({
                            let n = name.clone();
                            let edit = editing.is_some();
                            move || {
                                if edit {
                                    format!("pg-edit-{n}")
                                } else {
                                    format!("pg-value-{n}")
                                }
                            }
                        })
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .px_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .border_l_1()
                        .border_color(t.border)
                        .text_color(if row.value.editable() {
                            t.text
                        } else {
                            t.text_muted
                        })
                        .child(value_text)
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.activate(&n2, window, cx)),
                        );
                    if editing.is_some() {
                        value = value.bg(t.background).border_1().border_color(t.accent);
                    }
                    lines.push(
                        div()
                            .id(SharedString::from(format!("pg-row-{name}")))
                            .debug_selector({
                                let n = name.clone();
                                move || format!("pg-row-{n}")
                            })
                            .flex()
                            .flex_row()
                            .h(px(20.))
                            .bg(if selected { t.highlight } else { t.panel })
                            .child(
                                div()
                                    .w(px(150.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .pl_3()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(row.label.clone())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| this.select(&n1, cx)),
                                    ),
                            )
                            .child(value)
                            .into_any_element(),
                    );
                    if self.open_choice.as_deref() == Some(name.as_str())
                        && let PropertyValue::Choice { value, choices } = &row.value
                    {
                        let mut list = crate::popup::popup_panel(&t).ml(px(150.)).py_1();
                        for (v, label) in choices {
                            let (n, v2) = (name.clone(), v.clone());
                            list = list.child(
                                div()
                                    .id(SharedString::from(format!("pg-choice-{name}-{v}")))
                                    .debug_selector({
                                        let s = format!("pg-choice-{name}-{v}");
                                        move || s.clone()
                                    })
                                    .px_2()
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .bg(if v == value {
                                        t.highlight
                                    } else {
                                        t.popup_background
                                    })
                                    .hover(|s| s.bg(t.menu_hover))
                                    .child(label.clone())
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.choose(&n, &v2, cx)),
                                    ),
                            );
                        }
                        lines.push(list.into_any_element());
                    }
                }
            }
        }
        let description = self
            .selected
            .as_deref()
            .and_then(|s| self.row(s))
            .map(|r| (r.label.clone(), r.description.clone()));
        div()
            .id("property-grid")
            .key_context("PropertyGrid")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(
                div()
                    .flex_none()
                    .px_1()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.border)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id("property-grid-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            // A click elsewhere commits an edit (the value cells handle their own clicks first).
                            if this.editing.is_some() && !cx.has_active_drag() {
                                this.commit(cx);
                            }
                        }),
                    )
                    .children(lines),
            )
            .child(
                div()
                    .debug_selector(|| "pg-description".into())
                    .flex_none()
                    .h(px(56.))
                    .p_1()
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(t.typography.small)
                    .children(description.map(|(label, text)| {
                        div()
                            .flex()
                            .flex_col()
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(label))
                            .child(div().text_color(t.text_muted).child(text))
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let at = cx.debug_bounds(selector).expect(selector).center();
        cx.simulate_click(at, gpui::Modifiers::none());
    }
    use std::cell::RefCell;
    use std::rc::Rc;

    fn rows() -> Vec<PropertyRow> {
        vec![
            PropertyRow {
                category: "Advanced".into(),
                name: "build_action".into(),
                label: "Build Action".into(),
                value: PropertyValue::Choice {
                    value: "None".into(),
                    choices: vec![
                        ("None".into(), "None".into()),
                        ("Content".into(), "Content".into()),
                    ],
                },
                description: "How the file relates to the build.".into(),
            },
            PropertyRow {
                category: "Advanced".into(),
                name: "custom_tool".into(),
                label: "Custom Tool".into(),
                value: PropertyValue::Text("Gen".into()),
                description: "The tool that generates code from the file.".into(),
            },
            PropertyRow::read_only("Misc", "full_path", "Full Path", "/s/a.txt", "Where it is."),
        ]
    }

    #[test]
    fn lines_group_rows_by_category_in_first_seen_order() {
        let mut r = rows();
        r.push(PropertyRow::read_only("Advanced", "late", "Late", "x", ""));
        assert_eq!(
            grid_lines(&r),
            vec![
                GridLine::Category("Advanced".into()),
                GridLine::Row(0),
                GridLine::Row(1),
                GridLine::Row(3),
                GridLine::Category("Misc".into()),
                GridLine::Row(2),
            ]
        );
        assert_eq!(r[0].value.display(), "None");
        assert!(!r[2].value.editable());
    }

    #[gpui::test]
    fn choices_and_edits_tell_the_owner_and_read_only_rows_do_not(cx: &mut TestAppContext) {
        let theme = Theme::dark();
        let (grid, cx) = cx.add_window_view(|_, cx| PropertyGrid::new(theme, cx));
        let cx: &mut VisualTestContext = cx;
        let heard = Rc::new(RefCell::new(Vec::new()));
        let h = heard.clone();
        cx.update(|_, app| {
            app.subscribe(&grid, move |_, e: &PropertyGridEvent, _| {
                h.borrow_mut().push(e.clone())
            })
            .detach();
        });
        grid.update(cx, |g, cx| g.set_rows("a.txt File Properties", rows(), cx));
        cx.run_until_parked();

        // The drop-down opens on a click of the value and a pick is heard.
        click(cx, "pg-value-build_action");
        assert_eq!(
            grid.read_with(cx, |g, _| g.open_choice().map(str::to_owned))
                .as_deref(),
            Some("build_action")
        );
        click(cx, "pg-choice-build_action-Content");
        assert_eq!(
            heard.borrow().as_slice(),
            [PropertyGridEvent::Changed {
                name: "build_action".into(),
                value: "Content".into()
            }]
        );
        // The same value again is not a change.
        grid.update(cx, |g, cx| g.choose("build_action", "None", cx));
        assert_eq!(heard.borrow().len(), 1);

        // A text edit: typed, committed with Enter.
        click(cx, "pg-value-custom_tool");
        assert!(cx.debug_bounds("pg-edit-custom_tool").is_some());
        cx.simulate_keystrokes("backspace backspace backspace x enter");
        assert_eq!(
            heard.borrow().last(),
            Some(&PropertyGridEvent::Changed {
                name: "custom_tool".into(),
                value: "x".into()
            })
        );
        // Escape cancels.
        grid.update(cx, |g, cx| {
            g.select("custom_tool", cx);
        });
        click(cx, "pg-value-custom_tool");
        cx.simulate_keystrokes("y escape");
        assert_eq!(heard.borrow().len(), 2);
        assert!(grid.read_with(cx, |g, _| g.editing().is_none()));

        // A read-only value does nothing; the description follows the selection.
        click(cx, "pg-value-full_path");
        assert_eq!(
            grid.read_with(cx, |g, _| g.selected().map(str::to_owned))
                .as_deref(),
            Some("full_path")
        );
        assert_eq!(heard.borrow().len(), 2);

        // New rows for another selection close what was open.
        grid.update(cx, |g, cx| {
            g.activate_for_test("build_action", cx);
            g.set_rows("Other", rows(), cx);
        });
        assert!(grid.read_with(cx, |g, _| g.open_choice().is_none()
            && g.selected().is_none()));
    }

    impl PropertyGrid {
        fn activate_for_test(&mut self, name: &str, cx: &mut Context<Self>) {
            self.open_choice = Some(name.to_owned());
            cx.notify();
        }
    }
}
