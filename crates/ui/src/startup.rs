//! The Startup Projects dialog (brief 0028): Visual Studio's multiple startup projects, Project > Set Startup
//! Projects.... A list of the solution's projects with an Action column (None, Start, Start without debugging); OK
//! asks the caller to apply every row (`eludite.workspace.set_startup_project` with `projects`), Cancel and Escape
//! close it. The dialog holds only its rows; the caller opens it with the current choice and runs the command.

use gpui::{
    App, Context, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px,
};

use crate::Theme;

/// The Action column's values, in the order the dialog offers them; a row's `action` indexes this.
pub const STARTUP_ACTIONS: [&str; 3] = ["None", "Start", "Start without debugging"];
/// `STARTUP_ACTIONS` indices.
pub const ACTION_NONE: usize = 0;
pub const ACTION_START: usize = 1;
pub const ACTION_START_WITHOUT_DEBUGGING: usize = 2;

/// Debug selectors.
pub const STARTUP_DIALOG: &str = "startup-dialog";
pub const STARTUP_OK: &str = "startup-ok";
pub const STARTUP_CANCEL: &str = "startup-cancel";

/// The selector of row `row`'s option for action `action` (an index into [`STARTUP_ACTIONS`]).
pub fn startup_action_selector(row: usize, action: usize) -> String {
    format!("startup-row-{row}-action-{action}")
}

/// One project of the dialog: its name, its project file and its action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupRow {
    pub name: String,
    pub path: String,
    /// An index into [`STARTUP_ACTIONS`].
    pub action: usize,
}

/// What the dialog asks of its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupProjectsEvent {
    /// OK: set these as the startup projects.
    Apply(Vec<StartupRow>),
    Close,
}

pub struct StartupProjectsDialog {
    theme: Theme,
    title: String,
    rows: Vec<StartupRow>,
    message: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<StartupProjectsEvent> for StartupProjectsDialog {}

impl gpui::Focusable for StartupProjectsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl StartupProjectsDialog {
    /// The dialog for the solution `title` names, with its projects in solution order.
    pub fn new(theme: Theme, title: String, rows: Vec<StartupRow>, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            title,
            rows,
            message: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn rows(&self) -> &[StartupRow] {
        &self.rows
    }

    /// Set row `row`'s action (an index into [`STARTUP_ACTIONS`]).
    pub fn set_action(&mut self, row: usize, action: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.get_mut(row)
            && action < STARTUP_ACTIONS.len()
        {
            r.action = action;
            self.message = None;
            cx.notify();
        }
    }

    /// Why OK did not apply (the command's refusal, or no project starting).
    pub fn set_message(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.message = message;
        cx.notify();
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.rows.iter().all(|r| r.action == ACTION_NONE) {
            self.message =
                Some("Choose Start or Start without debugging for at least one project.".into());
            cx.notify();
            return;
        }
        cx.emit(StartupProjectsEvent::Apply(self.rows.clone()));
    }

    fn key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => cx.emit(StartupProjectsEvent::Close),
            "enter" => self.apply(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for StartupProjectsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let cell = |text: SharedString, width: Option<f32>| {
            let c = div()
                .px_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text);
            match width {
                Some(w) => c.flex_none().w(px(w)),
                None => c.flex_1().min_w_0(),
            }
        };
        let header = div()
            .flex()
            .flex_row()
            .h(px(22.))
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(t.border)
            .child(cell("Project".into(), Some(220.)))
            .child(cell("Action".into(), None));
        let rows: Vec<_> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, r)| {
                let options = STARTUP_ACTIONS.iter().enumerate().map(|(a, label)| {
                    crate::selector_option(
                        startup_action_selector(ix, a),
                        *label,
                        r.action == a,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.set_action(ix, a, cx)))
                });
                div()
                    .flex()
                    .flex_row()
                    .h(px(26.))
                    .items_center()
                    .child(cell(r.name.clone().into(), Some(220.)).font_weight(
                        if r.action == ACTION_NONE {
                            FontWeight::NORMAL
                        } else {
                            FontWeight::BOLD
                        },
                    ))
                    .child(div().flex().flex_row().gap_1().children(options))
            })
            .collect();
        let panel = crate::dialog_panel(&t, format!("Startup Projects ({})", self.title))
            .id(STARTUP_DIALOG)
            .debug_selector(|| STARTUP_DIALOG.into())
            .track_focus(&self.focus)
            .key_context("StartupProjectsDialog")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(640.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .child("Multiple startup projects: F5 starts each project whose action is Start (debugged) or Start without debugging, in solution order.")
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .child(header)
                            .child(
                                div()
                                    .id("startup-rows")
                                    .flex()
                                    .flex_col()
                                    .max_h(px(320.))
                                    .overflow_y_scroll()
                                    .children(rows),
                            ),
                    )
                    .children(
                        self.message
                            .clone()
                            .map(|m| div().text_color(t.text_muted).child(m)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(
                                crate::push_button(STARTUP_OK, "OK", true, true, &t)
                                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                            )
                            .child(
                                crate::push_button(STARTUP_CANCEL, "Cancel", false, true, &t).on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(StartupProjectsEvent::Close)),
                                ),
                            ),
                    ),
            );
        let viewport = window.viewport_size();
        let at = gpui::point(
            ((viewport.width - px(640.)) / 2.).max(px(0.)),
            (viewport.height / 8.).max(px(0.)),
        );
        gpui::deferred(gpui::anchored().position(at).child(panel)).with_priority(5)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, size};

    use super::*;

    /// The rows' action options set the action; OK applies every row, refused while none starts; Escape closes.
    #[gpui::test]
    fn the_action_column_sets_actions_and_ok_applies_them(cx: &mut TestAppContext) {
        let rows = vec![
            StartupRow {
                name: "App".into(),
                path: "/s/App/App.csproj".into(),
                action: ACTION_NONE,
            },
            StartupRow {
                name: "Web".into(),
                path: "/s/Web/Web.csproj".into(),
                action: ACTION_NONE,
            },
        ];
        let (view, vcx) = cx.add_window_view(|_, cx| {
            StartupProjectsDialog::new(Theme::dark(), "S".into(), rows, cx)
        });
        vcx.simulate_resize(size(px(1024.), px(768.)));
        let events: std::rc::Rc<std::cell::RefCell<Vec<StartupProjectsEvent>>> = Default::default();
        let seen = events.clone();
        vcx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &StartupProjectsEvent, _| {
                seen.borrow_mut().push(e.clone())
            })
            .detach()
        });
        vcx.run_until_parked();
        let click = |vcx: &mut gpui::VisualTestContext, sel: String| {
            let sel: &'static str = Box::leak(sel.into_boxed_str());
            let at = vcx.debug_bounds(sel).expect("drawn").center();
            vcx.simulate_click(at, Modifiers::none());
            vcx.run_until_parked();
        };
        // OK with nothing starting is refused with a message.
        click(vcx, STARTUP_OK.into());
        assert!(events.borrow().is_empty());
        click(vcx, startup_action_selector(0, ACTION_START));
        click(
            vcx,
            startup_action_selector(1, ACTION_START_WITHOUT_DEBUGGING),
        );
        assert_eq!(
            view.read_with(vcx, |d, _| d
                .rows()
                .iter()
                .map(|r| r.action)
                .collect::<Vec<_>>()),
            [ACTION_START, ACTION_START_WITHOUT_DEBUGGING]
        );
        click(vcx, STARTUP_OK.into());
        let applied = events.borrow().clone();
        match applied.as_slice() {
            [StartupProjectsEvent::Apply(rows)] => {
                assert_eq!(rows[0].action, ACTION_START);
                assert_eq!(rows[1].action, ACTION_START_WITHOUT_DEBUGGING);
            }
            other => panic!("{other:?}"),
        }
        click(vcx, STARTUP_CANCEL.into());
        assert_eq!(events.borrow().last(), Some(&StartupProjectsEvent::Close));
    }
}
