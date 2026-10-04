//! Tools > Options (brief 0020, PLAN.md 4.12 and 8): Visual Studio's Options dialog, generated from the settings
//! schema. The page tree on the left is the schema's sections (`Environment > Keyboard` is the page Keyboard under
//! Environment); each page lists its settings with the editor their type asks for: a check box for a switch, one
//! button per choice for an enum, a text box for text, a text box with Browse... for a path, and a count for the
//! agent list (edited in the file). Every change runs `eludite.settings.set` through the bus, as an agent's would,
//! and applies at once; OK (or Escape) closes the dialog. The values come from `eludite.settings.get`'s rows, so the
//! dialog shows where each value comes from (the solution's file, an environment variable that overrides it, the
//! person's state for the workspace) and when the solution's file sets a per-person setting, which is ignored there
//! (brief 0047's `browser.allowNoSandbox`, whose label says "for this workspace, on this machine").

use std::sync::Arc;

use eludite_commands::settings::{SettingKind, SettingRow, SettingSource, SettingsSchema};
use eludite_ui::{Theme, check_box, dialog_panel, push_button, text_box, toggle_button};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, point, px,
};
use serde_json::Value;

/// Debug selectors.
pub const DIALOG: &str = "options-dialog";
pub const OK: &str = "options-ok";

/// The page `ix` of the tree (the schema's sections, in order).
pub fn section_selector(ix: usize) -> String {
    format!("options-section-{ix}")
}

/// The editor of setting `key`: the check box, the text box, or (with [`choice_selector`]) a choice.
pub fn setting_selector(key: &str) -> String {
    format!("options-setting-{key}")
}

pub fn choice_selector(key: &str, ix: usize) -> String {
    format!("options-setting-{key}-{ix}")
}

pub fn browse_selector(key: &str) -> String {
    format!("options-browse-{key}")
}

/// What the dialog asks the shell to do.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionsEvent {
    /// Run `eludite.settings.set` with this key and value.
    Set {
        key: String,
        value: Value,
    },
    /// Pick a file for path setting `key`.
    Browse {
        key: String,
    },
    Close,
}

pub struct OptionsDialog {
    theme: Theme,
    schema: Arc<SettingsSchema>,
    rows: Vec<SettingRow>,
    /// The page shown (an index into the schema's sections).
    section: usize,
    /// The text setting being edited, and its text so far.
    editing: Option<(String, String)>,
    user_file: String,
    solution_file: Option<String>,
    /// Per-person keys the solution's file sets, ignored there (brief 0047).
    ignored: Vec<String>,
    focus: FocusHandle,
    /// Records where the pages, editors and OK are drawn (`eludite --bounds-out`).
    probe: Option<eludite_ui::BoundsMap>,
}

impl OptionsDialog {
    pub fn new(
        theme: Theme,
        schema: Arc<SettingsSchema>,
        section: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Self {
        let section = section
            .and_then(|s| schema.sections.iter().position(|x| x == s))
            .unwrap_or(0);
        Self {
            theme,
            schema,
            rows: Vec::new(),
            section,
            editing: None,
            user_file: String::new(),
            solution_file: None,
            ignored: Vec::new(),
            focus: cx.focus_handle(),
            probe: None,
        }
    }

    pub fn set_probe(&mut self, probe: Option<eludite_ui::BoundsMap>) {
        self.probe = probe;
    }

    /// The current values (after every change, the shell passes `eludite.settings.get`'s output).
    pub fn set_values(
        &mut self,
        out: eludite_commands::settings::SettingsGetOutput,
        cx: &mut Context<Self>,
    ) {
        self.rows = out.settings;
        self.user_file = out.user_file.path;
        self.solution_file = out.solution_file.map(|f| f.path);
        self.ignored = out.ignored_keys;
        cx.notify();
    }

    /// The page shown.
    pub fn section(&self) -> &str {
        &self.schema.sections[self.section]
    }

    pub fn show_section(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.schema.sections.len() {
            self.section = ix;
            self.editing = None;
            cx.notify();
        }
    }

    fn row(&self, key: &str) -> Option<&SettingRow> {
        self.rows.iter().find(|r| r.key == key)
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        if let Some((key, text)) = self.editing.take() {
            // A whole number is set as one (brief 0040's git.autoFetchMinutes); text that is not one is set as
            // text, which the setting refuses with its message.
            let value = match self.schema.get(&key).map(|s| &s.kind) {
                Some(SettingKind::Integer { .. }) => text
                    .trim()
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(text)),
                _ => Value::String(text),
            };
            cx.emit(OptionsEvent::Set { key, value });
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match (k.key.as_str(), self.editing.as_mut()) {
            ("escape", Some(_)) => self.editing = None,
            ("escape", None) | ("enter", None) => cx.emit(OptionsEvent::Close),
            ("enter", Some(_)) => self.commit_edit(cx),
            ("backspace", Some((_, text))) => {
                text.pop();
            }
            (_, Some((_, text))) => {
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
                        text.push_str(&c)
                    }
                    _ => return,
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Where a value comes from, when not the user file or the default.
    fn source_note(row: &SettingRow) -> Option<String> {
        match row.source {
            SettingSource::Environment => Some(format!(
                "Set by {} (it overrides the settings files while set).",
                row.env.as_deref().unwrap_or("an environment variable")
            )),
            SettingSource::Solution => Some("Set in the solution's .eludite/settings.json.".into()),
            SettingSource::UserWorkspace => Some(
                "Your answer for this workspace, on this machine (kept in Eludite's state for the workspace, never \
                 in .eludite/settings.json)."
                    .into(),
            ),
            SettingSource::User | SettingSource::Default => None,
        }
    }

    /// The notes under setting `key`: where its value comes from, and that the solution's file sets it in vain.
    pub fn notes(&self, key: &str) -> Vec<String> {
        let mut notes: Vec<String> = self
            .row(key)
            .and_then(Self::source_note)
            .into_iter()
            .collect();
        if self.ignored.iter().any(|k| k == key) {
            notes.push(format!(
                "{key} in .eludite/settings.json is ignored: this setting is per person, so only your answer here counts."
            ));
        }
        notes
    }

    fn setting_row(
        &self,
        spec: &eludite_commands::settings::SettingSpec,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let row = self.row(&spec.key);
        let value = row.map_or(spec.default.clone(), |r| r.value.clone());
        let key = spec.key.clone();
        let editor = match &spec.kind {
            SettingKind::Bool => {
                let on = value.as_bool().unwrap_or(false);
                // In a row, so the check box is as wide as its label, not the page.
                div()
                    .flex()
                    .flex_row()
                    .child(
                        check_box(setting_selector(&key), spec.label.clone(), on, &t)
                            .relative()
                            .children(eludite_ui::bounds_canvas(
                                self.probe.as_ref(),
                                setting_selector(&key),
                            ))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OptionsEvent::Set {
                                    key: key.clone(),
                                    value: Value::Bool(!on),
                                })
                            })),
                    )
                    .into_any_element()
            }
            SettingKind::Enum { values, labels } => {
                let current = value.as_str().unwrap_or_default().to_owned();
                let choices = values
                    .iter()
                    .zip(labels)
                    .enumerate()
                    .map(|(ix, (v, label))| {
                        let (key, v) = (key.clone(), v.clone());
                        toggle_button(choice_selector(&key, ix), label.clone(), v == current, &t)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OptionsEvent::Set {
                                    key: key.clone(),
                                    value: Value::String(v.clone()),
                                })
                            }))
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(spec.label.clone())
                    .child(div().flex().flex_row().gap_1().children(choices))
                    .into_any_element()
            }
            SettingKind::Text | SettingKind::Path | SettingKind::Integer { .. } => {
                let editing = self
                    .editing
                    .as_ref()
                    .filter(|(k, _)| *k == key)
                    .map(|(_, text)| text.clone());
                let shown = editing.clone().unwrap_or_else(|| match &value {
                    Value::Number(n) => n.to_string(),
                    v => v.as_str().unwrap_or_default().to_owned(),
                });
                let placeholder = if spec.kind == SettingKind::Path {
                    "(found automatically)"
                } else {
                    "(none)"
                };
                let start = (key.clone(), shown.clone());
                let text = text_box(
                    setting_selector(&key),
                    &shown,
                    placeholder,
                    editing.is_some() && focused,
                    &t,
                )
                .w(px(360.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.editing = Some(start.clone());
                    this.focus.focus(window, cx);
                    cx.notify();
                }));
                let browse = (spec.kind == SettingKind::Path).then(|| {
                    let key = key.clone();
                    push_button(browse_selector(&key), "Browse...", false, true, &t).on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(OptionsEvent::Browse { key: key.clone() })
                        }),
                    )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(spec.label.clone())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(text)
                            .children(browse),
                    )
                    .into_any_element()
            }
            SettingKind::List => {
                let n = value.as_array().map_or(0, Vec::len);
                div()
                    .id(SharedString::from(setting_selector(&key)))
                    .debug_selector({
                        let key = key.clone();
                        move || setting_selector(&key)
                    })
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w_full()
                    .child(spec.label.clone())
                    .child(div().text_color(t.text_muted).child(format!(
                        "{n} agent{} configured. Edit `{}` in the settings file.",
                        if n == 1 { "" } else { "s" },
                        spec.key
                    )))
                    .into_any_element()
            }
        };
        let notes = self.notes(&spec.key);
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .py_1()
            .child(editor)
            .child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(spec.description.clone()),
            )
            .children(notes.into_iter().map(|n| {
                div()
                    .text_size(t.typography.small)
                    .text_color(t.accent)
                    .child(n)
            }))
            .into_any_element()
    }
}

impl EventEmitter<OptionsEvent> for OptionsDialog {}

impl Focusable for OptionsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OptionsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        // The page tree: a parent label once, its pages indented under it.
        let mut tree = Vec::new();
        let mut parent: Option<String> = None;
        for (ix, section) in self.schema.sections.iter().enumerate() {
            let (p, leaf) = match section.split_once(" > ") {
                Some((p, leaf)) => (Some(p.to_owned()), leaf.to_owned()),
                None => (None, section.clone()),
            };
            if p.is_some() && p != parent {
                tree.push(
                    div()
                        .px_2()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .child(format!("\u{25E2} {}", p.clone().unwrap_or_default()))
                        .into_any_element(),
                );
            }
            let indent = if p.is_some() { 20. } else { 8. };
            parent = p;
            let selected = ix == self.section;
            let sel = section_selector(ix);
            let probed = eludite_ui::bounds_canvas(self.probe.as_ref(), sel.clone());
            let item = div()
                .id(SharedString::from(sel.clone()))
                .debug_selector(move || sel)
                .relative()
                .children(probed)
                .pl(px(indent))
                .h(px(20.))
                .flex()
                .items_center()
                .cursor_pointer()
                .child(leaf)
                .on_click(cx.listener(move |this, _, _, cx| this.show_section(ix, cx)));
            tree.push(if selected {
                item.bg(t.accent)
                    .text_color(t.text_on_accent)
                    .into_any_element()
            } else {
                item.hover(|s| s.bg(t.menu_hover)).into_any_element()
            });
        }
        let section = self.section().to_owned();
        let specs: Vec<_> = self.schema.section(&section).cloned().collect();
        let rows: Vec<_> = specs
            .iter()
            .map(|spec| self.setting_row(spec, focused, cx))
            .collect();
        let files = match &self.solution_file {
            Some(s) => format!("User settings: {}\nWorkspace settings: {s}", self.user_file),
            None => format!("User settings: {}", self.user_file),
        };
        let ok = push_button(OK, "OK", true, true, &t)
            .relative()
            .children(eludite_ui::bounds_canvas(self.probe.as_ref(), OK))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(OptionsEvent::Close)));
        let panel = dialog_panel(&t, "Options")
            .id(DIALOG)
            .debug_selector(|| DIALOG.into())
            .track_focus(&self.focus)
            .key_context("OptionsDialog")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(760.))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(420.))
                    .child(
                        div()
                            .id("options-tree")
                            .flex()
                            .flex_col()
                            .w(px(220.))
                            .py_1()
                            .overflow_y_scroll()
                            .bg(t.background)
                            .border_1()
                            .border_color(t.border)
                            .m_2()
                            .children(tree),
                    )
                    .child(
                        div()
                            .id("options-page")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .p_2()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .text_size(t.typography.ui)
                                    .text_color(t.text_muted)
                                    .child(section.replace(" > ", " \u{25B8} ")),
                            )
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .child(
                        // Long paths wrap: OK stays inside the dialog.
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(files),
                    )
                    .child(ok),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(760.)) / 2.).max(px(0.)),
            (viewport.height / 8.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(5)
    }
}
