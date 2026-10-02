//! The root view: menu bar, docking area and status bar.
//!
//! Keys and menu items both produce [`RunCommand`]; this view's action handler
//! is the one place the UI turns that into a command-bus invocation.

use std::rc::Rc;
use std::sync::Arc;

use eludite_commands::{CommandRegistry, builtins};
use eludite_docking::{DockController, DockHost, DocumentTab, Persistence, Probe};
use eludite_ui::{
    MenuBar, RunCommand, SHELL_CONTEXT, StatusBar, Theme, menu_bar_with, slots, vs_keymap,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Window, div,
};

type AfterPresent = Box<dyn FnOnce(&mut Window, &mut App)>;

pub struct Shell {
    theme: Theme,
    menu: Entity<MenuBar>,
    dock: Entity<DockHost>,
    status: StatusBar,
    focus: FocusHandle,
    on_first_render: Option<AfterPresent>,
}

/// Tool window bodies: titled empty panels until later briefs fill them.
fn tool_body(_id: &str, _theme: &Theme) -> AnyElement {
    div().into_any_element()
}

fn document_body(tab: &DocumentTab, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_size(gpui::px(20.)).child(tab.title.clone()))
        .child(
            div()
                .text_color(theme.text_muted)
                .child("Nothing is open. Documents arrive with the editor."),
        )
        .into_any_element()
}

impl Shell {
    pub fn new(
        commands: Arc<CommandRegistry>,
        controller: DockController,
        theme: Theme,
        persistence: Option<Persistence>,
        cx: &mut Context<Self>,
    ) -> Self {
        let registry = commands.clone();
        let menu = cx.new(|_| {
            menu_bar_with(theme, vs_keymap(), move |cmd| {
                registry.lookup(cmd).is_some()
            })
        });
        let dock = cx.new(|cx| {
            DockHost::new(
                controller,
                commands.clone(),
                theme,
                Rc::new(tool_body),
                Rc::new(document_body),
                persistence,
                cx,
            )
        });
        cx.observe(&dock, |_, _, cx| cx.notify()).detach();
        let mut status = StatusBar::vs_default();
        // The status bar reads the version through the command bus, like an agent would.
        let version = commands
            .invoke(builtins::ABOUT, serde_json::json!({}))
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .unwrap_or_else(|| builtins::VERSION.to_owned());
        status.set(slots::VERSION, format!("Eludite {version}"));
        Self {
            theme,
            menu,
            dock,
            status,
            focus: cx.focus_handle(),
            on_first_render: None,
        }
    }

    pub fn dock(&self) -> &Entity<DockHost> {
        &self.dock
    }

    #[cfg(test)]
    pub fn menu(&self) -> &Entity<MenuBar> {
        &self.menu
    }

    #[cfg(test)]
    pub fn status(&self) -> &StatusBar {
        &self.status
    }

    pub fn set_probe(&mut self, probe: Option<Probe>, cx: &mut Context<Self>) {
        self.dock.update(cx, |d, _| d.set_probe(probe));
    }

    /// Run `f` once, after the first frame is presented.
    pub fn after_first_present(&mut self, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.on_first_render = Some(Box::new(f));
    }

    fn run_command(&mut self, action: &RunCommand, _: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .dock
            .update(cx, |d, _| d.invoke(&action.command, action.args.clone()));
        let text = match (&result, action.command.as_ref()) {
            (Ok(v), builtins::ABOUT) => format!(
                "{} {}",
                v["name"].as_str().unwrap_or("Eludite"),
                v["version"].as_str().unwrap_or_default()
            ),
            (Ok(_), _) => "Ready".to_owned(),
            (Err(e), _) => e.to_string(),
        };
        self.status.set(slots::STATE, text);
        cx.notify();
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(f) = self.on_first_render.take() {
            // Deferred effects run after this frame is presented.
            let handle = window.window_handle();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| f(window, cx));
            });
        }
        let t = self.theme;
        div()
            .id("shell")
            .key_context(SHELL_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::run_command))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.chrome)
            .text_color(t.text)
            .child(self.menu.clone())
            .child(self.dock.clone())
            .child(self.status.render(&t))
    }
}
