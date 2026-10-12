//! The breakpoint glyph's interactions (brief 0077): its right-click menu (Delete Breakpoint, Disable or Enable
//! Breakpoint, Conditions..., Actions...), its hover tooltip (the location, the condition, the hit count, the hits and
//! the binding), and Visual Studio's inline Breakpoint Settings panel under the line (Conditions: a conditional
//! expression and a hit count; Actions: log a message to the Output window and continue). All three are editor popups
//! anchored under the breakpoint's line; every change is `eludite.debug.toggle_breakpoint` or
//! `eludite.debug.enable_breakpoints`.

use eludite_commands::debug::{self as cmds, BreakpointRow, HitCondition};
use eludite_commands::debug_inspect::ENABLE_BREAKPOINTS;
use eludite_ui::{RunCommand, Theme, text_box, toggle_button};
use gpui::{
    App, Context, EventEmitter, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use serde_json::json;

use super::windows::{InputKey, LineInput, ROW_HEIGHT};

/// The editor popup keys.
pub const MENU: &str = "breakpoint-menu";
pub const TOOLTIP: &str = "breakpoint-tooltip";
pub const SETTINGS: &str = "breakpoint-settings";

/// The glyph tooltip's lines for a breakpoint row.
pub fn tooltip_lines(r: &BreakpointRow) -> Vec<String> {
    let mut out = vec![r.label()];
    if let Some(c) = &r.condition {
        out.push(format!("When '{c}' is true"));
    }
    if let Some(h) = &r.hit_condition {
        out.push(format!("Hit count: {h}"));
    }
    if let Some(m) = &r.log_message {
        out.push(format!("Print: {m}"));
    }
    out.push(format!("Hits: {}", r.hits));
    if !r.enabled {
        out.push("Disabled".into());
    } else if !r.verified {
        out.push(format!(
            "The breakpoint will not currently be hit{}",
            r.message
                .as_deref()
                .map(|m| format!(": {m}"))
                .unwrap_or_else(|| ".".into())
        ));
    }
    out
}

fn command(window: &mut Window, cx: &mut App, id: &str, args: serde_json::Value) {
    window.dispatch_action(Box::new(RunCommand::new(id.to_owned(), args)), cx);
}

/// The tooltip.
pub struct GlyphTip {
    theme: Theme,
    pub lines: Vec<String>,
}

impl GlyphTip {
    pub fn new(theme: Theme, lines: Vec<String>) -> Self {
        Self { theme, lines }
    }
}

impl Render for GlyphTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        eludite_ui::popup::popup_panel(&t)
            .id(TOOLTIP)
            .debug_selector(|| TOOLTIP.to_owned())
            .px_2()
            .py_1()
            .text_size(t.typography.ui)
            .flex()
            .flex_col()
            .children(self.lines.iter().map(|l| div().child(l.clone())))
    }
}

/// What the glyph menu asks of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphAction {
    Delete,
    ToggleEnabled,
    Conditions,
    Actions,
    Close,
}

/// The glyph's right-click menu for the breakpoint at `path` and `line`.
pub struct GlyphMenu {
    theme: Theme,
    pub path: String,
    pub line: u32,
    pub enabled: bool,
}

impl EventEmitter<GlyphAction> for GlyphMenu {}

impl GlyphMenu {
    pub fn new(theme: Theme, path: String, line: u32, enabled: bool) -> Self {
        Self {
            theme,
            path,
            line,
            enabled,
        }
    }

    /// Run an item: Delete and Disable are commands; Conditions... and Actions... open the settings panel (the shell).
    pub fn act(&mut self, a: GlyphAction, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            GlyphAction::Delete => command(
                window,
                cx,
                cmds::TOGGLE_BREAKPOINT,
                json!({"path": self.path, "line": self.line, "action": "delete"}),
            ),
            GlyphAction::ToggleEnabled => command(
                window,
                cx,
                ENABLE_BREAKPOINTS,
                json!({"path": self.path, "line": self.line, "enabled": !self.enabled}),
            ),
            _ => {}
        }
        cx.emit(a);
    }
}

impl Render for GlyphMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let items = [
            (GlyphAction::Delete, "delete", "Delete Breakpoint"),
            (
                GlyphAction::ToggleEnabled,
                "toggle",
                if self.enabled {
                    "Disable Breakpoint"
                } else {
                    "Enable Breakpoint"
                },
            ),
            (GlyphAction::Conditions, "conditions", "Conditions..."),
            (GlyphAction::Actions, "actions", "Actions..."),
        ]
        .into_iter()
        .map(|(a, slug, label)| {
            let sel = format!("{MENU}-{slug}");
            div()
                .id(SharedString::from(sel.clone()))
                .debug_selector(move || sel)
                .px_2()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.act(a, window, cx)))
        });
        eludite_ui::popup::popup_panel(&t)
            .id(MENU)
            .debug_selector(|| MENU.to_owned())
            .occlude()
            .min_w(px(180.))
            .py_1()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .flex()
            .flex_col()
            .children(items)
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(GlyphAction::Close)))
    }
}

/// Visual Studio's hit count kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    Equal,
    MultipleOf,
    AtLeast,
}

/// Which box of the panel a key is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    Condition,
    HitCount,
    Message,
}

/// The inline Breakpoint Settings panel.
pub struct BreakpointSettings {
    theme: Theme,
    pub path: String,
    pub line: u32,
    pub conditions: bool,
    pub actions: bool,
    condition: LineInput,
    hit_kind: HitKind,
    hit_count: LineInput,
    message: LineInput,
}

/// The panel closed (its changes were written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseSettings;

impl EventEmitter<CloseSettings> for BreakpointSettings {}

impl BreakpointSettings {
    /// The panel for breakpoint row `r`, with Conditions or Actions open (both when the breakpoint has both).
    pub fn new(theme: Theme, r: &BreakpointRow, actions: bool, cx: &mut App) -> Self {
        let mut condition = LineInput::new(cx);
        condition.text = r.condition.clone().unwrap_or_default();
        let mut hit_count = LineInput::new(cx);
        let mut message = LineInput::new(cx);
        message.text = r.log_message.clone().unwrap_or_default();
        let hit = r.hit_condition.as_deref().and_then(HitCondition::parse);
        let hit_kind = match hit {
            Some(HitCondition::MultipleOf(n)) => {
                hit_count.text = n.to_string();
                HitKind::MultipleOf
            }
            Some(HitCondition::AtLeast(n)) => {
                hit_count.text = n.to_string();
                HitKind::AtLeast
            }
            Some(HitCondition::Equal(n)) => {
                hit_count.text = n.to_string();
                HitKind::Equal
            }
            None => HitKind::Equal,
        };
        Self {
            theme,
            path: r.path.clone().unwrap_or_default(),
            line: r.line.unwrap_or(1),
            conditions: !actions || r.condition.is_some() || r.hit_condition.is_some(),
            actions: actions || r.log_message.is_some(),
            condition,
            hit_kind,
            hit_count,
            message,
        }
    }

    fn input(&mut self, f: SettingsField) -> &mut LineInput {
        match f {
            SettingsField::Condition => &mut self.condition,
            SettingsField::HitCount => &mut self.hit_count,
            SettingsField::Message => &mut self.message,
        }
    }

    /// Set a box's text (tests, and the keys).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_text(&mut self, f: SettingsField, text: &str, cx: &mut Context<Self>) {
        self.input(f).text = text.to_owned();
        cx.notify();
    }

    pub fn set_hit_kind(&mut self, k: HitKind, cx: &mut Context<Self>) {
        self.hit_kind = k;
        cx.notify();
    }

    /// The `toggle_breakpoint` arguments the panel's state stands for.
    pub fn args(&self) -> serde_json::Value {
        let count = self.hit_count.text.trim();
        let hit = if !self.conditions || count.is_empty() {
            String::new()
        } else {
            match self.hit_kind {
                HitKind::Equal => count.to_owned(),
                HitKind::MultipleOf => format!("%{count}"),
                HitKind::AtLeast => format!(">={count}"),
            }
        };
        json!({
            "path": self.path,
            "line": self.line,
            "action": "set",
            "condition": if self.conditions { self.condition.text.trim() } else { "" },
            "hit_condition": hit,
            "log_message": if self.actions { self.message.text.trim() } else { "" },
        })
    }

    /// Write the settings and close.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        command(window, cx, cmds::TOGGLE_BREAKPOINT, self.args());
        cx.emit(CloseSettings);
    }

    fn key(
        &mut self,
        f: SettingsField,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e.keystroke.key.as_str() {
            "escape" => cx.emit(CloseSettings),
            "enter" => self.apply(window, cx),
            _ => {
                if matches!(self.input(f).key(e), InputKey::Ignored) {
                    return;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn field(
        &self,
        f: SettingsField,
        id: &'static str,
        placeholder: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        let input = match f {
            SettingsField::Condition => &self.condition,
            SettingsField::HitCount => &self.hit_count,
            SettingsField::Message => &self.message,
        };
        let focus = input.focus.clone();
        text_box(
            id,
            &input.text,
            placeholder,
            input.focus.is_focused(window),
            &t,
        )
        .flex_1()
        .track_focus(&input.focus)
        .key_context("DebugBreakpointSettings")
        .on_key_down(cx.listener(move |this, e, window, cx| this.key(f, e, window, cx)))
        .on_click(cx.listener(move |_, _, window, cx| {
            focus.focus(window, cx);
            cx.notify();
        }))
    }
}

impl Render for BreakpointSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let check = |on: bool| if on { "\u{2611}" } else { "\u{2610}" };
        let hit_kinds = [
            (HitKind::Equal, "=", "breakpoint-settings-hit-equal"),
            (
                HitKind::MultipleOf,
                "Is a multiple of",
                "breakpoint-settings-hit-multiple",
            ),
            (HitKind::AtLeast, ">=", "breakpoint-settings-hit-at-least"),
        ]
        .into_iter()
        .map(|(k, label, id)| {
            toggle_button(id, label, self.hit_kind == k, &t)
                .on_click(cx.listener(move |this, _, _, cx| this.set_hit_kind(k, cx)))
        })
        .collect::<Vec<_>>();
        let conditions = self.conditions.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child("Conditional Expression: Is true")
                        .child(self.field(
                            SettingsField::Condition,
                            "breakpoint-settings-condition",
                            "e.g. x == 5",
                            window,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child("Hit Count:")
                        .children(hit_kinds)
                        .child(self.field(
                            SettingsField::HitCount,
                            "breakpoint-settings-hit",
                            "1",
                            window,
                            cx,
                        )),
                )
        });
        let actions = self.actions.then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child("Show a message in the Output Window:")
                        .child(self.field(
                            SettingsField::Message,
                            "breakpoint-settings-message",
                            "{x}, $FUNCTION, $TID",
                            window,
                            cx,
                        )),
                )
                // A tracepoint always continues here (the model's When Hit is "print and continue").
                .child(
                    div()
                        .text_color(t.text_muted)
                        .child(format!("{} Continue code execution", check(true))),
                )
        });
        div()
            .id(SETTINGS)
            .debug_selector(|| SETTINGS.to_owned())
            .occlude()
            .w(px(620.))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Breakpoint Settings"),
            )
            .child(
                div()
                    .id("breakpoint-settings-conditions")
                    .debug_selector(|| "breakpoint-settings-conditions".to_owned())
                    .cursor_pointer()
                    .child(format!("{} Conditions", check(self.conditions)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.conditions = !this.conditions;
                        cx.notify();
                    })),
            )
            .children(conditions)
            .child(
                div()
                    .id("breakpoint-settings-actions")
                    .debug_selector(|| "breakpoint-settings-actions".to_owned())
                    .cursor_pointer()
                    .child(format!("{} Actions", check(self.actions)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.actions = !this.actions;
                        cx.notify();
                    })),
            )
            .children(actions)
            .child(
                div().flex().flex_row().justify_end().child(
                    toggle_button("breakpoint-settings-close", "Close", false, &t)
                        .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> BreakpointRow {
        serde_json::from_value(json!({
            "path": "/s/Program.cs", "line": 6, "enabled": true, "verified": false, "hits": 2,
            "condition": "x > 1", "hit_condition": ">=3", "message": "No executable code at line 6.",
            "kind": "breakpoint"
        }))
        .unwrap()
    }

    #[test]
    fn the_tooltip_names_the_location_the_conditions_hits_and_binding() {
        let lines = tooltip_lines(&row());
        assert_eq!(lines[1], "When 'x > 1' is true");
        assert_eq!(lines[2], "Hit count: >=3");
        assert_eq!(lines[3], "Hits: 2");
        assert_eq!(
            lines[4],
            "The breakpoint will not currently be hit: No executable code at line 6."
        );
    }
}
