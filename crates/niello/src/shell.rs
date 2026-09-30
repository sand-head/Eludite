//! The root view: menu bar, docked layout and status bar.

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
};
use niello_commands::{CommandRegistry, builtins};
use niello_docking::{DockLayout, ToolWindow, ids};
use niello_ui::{Theme, menu_bar, status_bar};

pub const MENU_LABELS: [&str; 13] = [
    "File",
    "Edit",
    "View",
    "Git",
    "Project",
    "Build",
    "Debug",
    "Test",
    "Analyze",
    "Tools",
    "Extensions",
    "Window",
    "Help",
];

pub struct Shell {
    theme: Theme,
    layout: DockLayout,
    #[expect(
        dead_code,
        reason = "menus and the MCP server will dispatch through it"
    )]
    commands: CommandRegistry,
    version: SharedString,
}

impl Shell {
    pub fn new(commands: CommandRegistry) -> Self {
        // The status bar reads the version through the command bus, like an agent would.
        let version = commands
            .invoke(builtins::ABOUT, serde_json::json!({}))
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .unwrap_or_else(|| builtins::VERSION.to_owned());
        Self {
            theme: Theme::vs_dark(),
            layout: DockLayout::default_vs(),
            commands,
            version: format!("Niello {version}").into(),
        }
    }
}

fn tool_body(window: &ToolWindow) -> AnyElement {
    let text = match window.id.as_str() {
        ids::SOLUTION_EXPLORER => "No solution open",
        ids::ERROR_LIST => "0 Errors   0 Warnings   0 Messages",
        ids::OUTPUT => "Show output from: Build",
        _ => "",
    };
    div().child(text).into_any_element()
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .child(menu_bar(theme, MENU_LABELS))
            .child(niello_docking::render_layout(
                &self.layout,
                theme,
                &tool_body,
                div().child("Niello — nothing is open").into_any_element(),
            ))
            .child(status_bar(theme, "Ready", self.version.clone()))
    }
}
