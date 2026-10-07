//! The toolbar row under the title bar: Visual Studio's Standard and Debug toolbars (navigation, undo, the run
//! controls, the steps, Build and Agents) around one control holding the Standard toolbar's lists (brief 0049): Solution Configurations
//! (the solution's own, Debug and Release for a Cargo workspace, then Configuration Manager...), Solution Platforms
//! (the solution's own, then Configuration Manager...), the Target Framework list when the startup project is
//! multi-targeted, and the Debug toolbar's Start button with the launch profile list of the startup project. Every
//! choice runs a command (`eludite.solution.select_configuration`, `eludite.solution.set_configuration`,
//! `eludite.project.set_launch_profile` with `select`, `eludite.debug.start`), so agents and the toolbar agree.

use eludite_commands::project::properties::{self as props};
use eludite_commands::{build, debug, view, workspace};
use eludite_ui::{Theme, toolbar_button};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, anchored, deferred, div, prelude::FluentBuilder, px,
};
use serde_json::{Value, json};

use super::Shell;
use super::build::{CONFIGURATION_BUTTON, PLATFORM_BUTTON, toolbar_item_selector};
use super::project_properties::ToolbarList;
use super::project_properties::pages::probed;

/// Debug selectors of the Target Framework list, the profile list and Start.
pub const FRAMEWORK_BUTTON: &str = "build-framework";
pub const PROFILE_BUTTON: &str = "debug-profile";
pub const START_BUTTON: &str = "debug-start";
/// Debug selectors of the toolbar's buttons.
pub const BACK_BUTTON: &str = "toolbar-back";
pub const FORWARD_BUTTON: &str = "toolbar-forward";
pub const UNDO_BUTTON: &str = "toolbar-undo";
pub const REDO_BUTTON: &str = "toolbar-redo";
pub const START_WITHOUT_DEBUGGING_BUTTON: &str = "toolbar-start-without-debugging";
pub const PAUSE_BUTTON: &str = "toolbar-break-all";
pub const STOP_BUTTON: &str = "toolbar-stop";
pub const RESTART_BUTTON: &str = "toolbar-restart";
pub const STEP_OVER_BUTTON: &str = "toolbar-step-over";
pub const STEP_INTO_BUTTON: &str = "toolbar-step-into";
pub const STEP_OUT_BUTTON: &str = "toolbar-step-out";
pub const BUILD_BUTTON: &str = "toolbar-build";
pub const AGENTS_BUTTON: &str = "toolbar-agents";

/// The toolbar row's height, and its configuration control's.
pub const TOOLBAR_HEIGHT: gpui::Pixels = px(36.);
const SEGMENT_HEIGHT: f32 = 26.;

/// The label of the lists' last entry.
pub const CONFIGURATION_MANAGER: &str = "Configuration Manager...";

impl ToolbarList {
    fn kind(self) -> &'static str {
        match self {
            ToolbarList::Configuration => "configuration",
            ToolbarList::Platform => "platform",
            ToolbarList::Framework => "framework",
            ToolbarList::Profile => "profile",
        }
    }
}

impl Shell {
    /// What a list offers: (label, command, args) per entry.
    fn toolbar_entries(&self, list: ToolbarList) -> Vec<(String, &'static str, Value)> {
        let manager = (
            CONFIGURATION_MANAGER.to_owned(),
            eludite_commands::solution::SET_CONFIGURATION,
            json!({}),
        );
        match list {
            ToolbarList::Configuration => {
                let mut v: Vec<_> = self
                    .solution_configurations()
                    .into_iter()
                    .map(|c| {
                        let args = json!({ "configuration": c });
                        (c, eludite_commands::solution::SELECT_CONFIGURATION, args)
                    })
                    .collect();
                if self.solution.is_some() {
                    v.push(manager);
                }
                v
            }
            ToolbarList::Platform => {
                let mut v: Vec<_> = self
                    .solution_platforms()
                    .into_iter()
                    .map(|p| {
                        let args = json!({ "platform": p });
                        (p, eludite_commands::solution::SELECT_CONFIGURATION, args)
                    })
                    .collect();
                if self.solution.is_some() {
                    v.push(manager);
                }
                v
            }
            ToolbarList::Framework => {
                let Some(project) = self.toolbar_project() else {
                    return Vec::new();
                };
                self.project_frameworks(&project)
                    .into_iter()
                    .map(|f| {
                        let args = json!({ "framework": f, "project": project });
                        (f, eludite_commands::solution::SELECT_CONFIGURATION, args)
                    })
                    .collect()
            }
            ToolbarList::Profile => {
                let Some(project) = self.toolbar_project() else {
                    return Vec::new();
                };
                self.properties
                    .launch
                    .get(&project)
                    .map(|l| {
                        l.profiles
                            .iter()
                            .map(|p| {
                                let args =
                                    json!({ "project": project, "profile": p.name, "action": "select" });
                                (p.name.clone(), props::SET_LAUNCH_PROFILE, args)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
    }

    /// The startup project's launch profile F5 uses, for Start's label.
    fn toolbar_profile(&self) -> Option<String> {
        let project = self.toolbar_project()?;
        self.selected_profile(&project).or_else(|| {
            self.properties.launch.get(&project).and_then(|l| {
                l.profiles
                    .iter()
                    .find(|p| p.command_name == "Project")
                    .map(|p| p.name.clone())
            })
        })
    }

    /// The toolbar row under the title bar: Visual Studio's Standard and Debug toolbars. Navigate Backward and
    /// Forward, Undo and Redo; one control holding the configuration, platform, target framework and launch profile
    /// lists with Start at its end (Continue in break mode, Stop while a session runs); Start Without Debugging; Break
    /// All, Stop Debugging and Restart; the steps; and Build Solution and the Agents window at the right. Every button
    /// runs a command, enabled as the menu's item is.
    pub(super) fn build_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t: Theme = self.theme;
        let (configuration, platform) = self.active_selection();
        let project = self.toolbar_project();
        let frameworks = project
            .as_deref()
            .map(|p| self.project_frameworks(p))
            .unwrap_or_default();
        let framework = (frameworks.len() > 1).then(|| {
            project
                .as_deref()
                .and_then(|p| self.selected_framework(p))
                .unwrap_or_else(|| frameworks[0].clone())
        });
        let profile = self.toolbar_profile();
        let mut lists: Vec<(ToolbarList, &'static str, String)> = vec![
            (
                ToolbarList::Configuration,
                CONFIGURATION_BUTTON,
                configuration,
            ),
            (ToolbarList::Platform, PLATFORM_BUTTON, platform),
        ];
        if let Some(f) = framework {
            lists.push((ToolbarList::Framework, FRAMEWORK_BUTTON, f));
        }
        if let Some(p) = &profile {
            lists.push((ToolbarList::Profile, PROFILE_BUTTON, p.clone()));
        }
        let probe = self.ui_bounds.clone();
        let open = self.properties.toolbar_menu;
        let building = self
            .builds
            .building
            .load(std::sync::atomic::Ordering::SeqCst);
        let enabled = |command: &str| {
            self.debug.menu.enabled(command) && super::build::menu_enabled(command, building)
        };
        let menu = |list: ToolbarList| {
            let items = self.toolbar_entries(list).into_iter().enumerate().map(
                |(ix, (label, command, args))| {
                    let sel = toolbar_item_selector(list.kind(), ix);
                    let probe = eludite_ui::bounds_canvas(probe.as_ref(), sel.clone());
                    div()
                        .id(SharedString::from(sel.clone()))
                        .debug_selector(move || sel)
                        .relative()
                        .children(probe)
                        .px_2()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.menu_hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.properties.toolbar_menu = None;
                            this.run(command, args.clone(), window, cx);
                        }))
                },
            );
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("build-toolbar-menu")
                        .occlude()
                        .min_w(px(140.))
                        .py_1()
                        .mt(px(SEGMENT_HEIGHT + 2.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.properties.toolbar_menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        };
        let segments: Vec<_> = lists
            .into_iter()
            .map(|(list, id, label)| {
                let button = segment(id, &t)
                    .when(open == Some(list), |s| s.bg(t.menu_hover))
                    .font_weight(if list == ToolbarList::Profile {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(label)
                    .child(div().text_color(t.text_muted).child("\u{25BE}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.properties.toolbar_menu =
                            (this.properties.toolbar_menu != Some(list)).then_some(list);
                        cx.notify();
                    }));
                div()
                    .relative()
                    .flex()
                    .child(probed(probe.as_ref(), button, id.into()))
                    .children((open == Some(list)).then(|| menu(list)))
            })
            .collect();
        // Start ends the control: Continue in break mode, Stop while a session runs (Visual Studio's F5 and
        // Shift+F5).
        let (glyph, label, command) = if enabled(debug::CONTINUE) {
            ("\u{25B6}", "Continue", debug::CONTINUE)
        } else if self.debug.menu.enabled(debug::STOP) {
            ("\u{25A0}", "Stop", debug::STOP)
        } else {
            ("\u{25B6}", "Start", debug::START)
        };
        let start_enabled = command != debug::START || enabled(debug::START);
        let start =
            segment(START_BUTTON, &t)
                .border_r_0()
                .px(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .bg(t.accent)
                .text_color(t.text_on_accent)
                .when(!start_enabled, |s| s.opacity(0.5))
                .child(glyph)
                .child(label)
                .when(start_enabled, |s| {
                    s.on_click(cx.listener(move |this, _, window, cx| {
                        this.run(command, json!({}), window, cx)
                    }))
                });
        let control = div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(SEGMENT_HEIGHT))
            .rounded(px(6.))
            .border_1()
            .border_color(t.border)
            .bg(t.panel)
            .overflow_hidden()
            .children(segments)
            .child(probed(probe.as_ref(), start, START_BUTTON.into()));
        let button = |id: &'static str, label: &'static str, command: &'static str, args: Value| {
            let on = enabled(command);
            let el = toolbar_button(id, label, on, &t)
                .h(px(SEGMENT_HEIGHT))
                .min_w(px(SEGMENT_HEIGHT))
                .justify_center()
                .rounded(px(5.));
            let el = if on {
                el.on_click(cx.listener(move |this, _, window, cx| {
                    this.run(command, args.clone(), window, cx)
                }))
            } else {
                el
            };
            probed(probe.as_ref(), el, id.into())
        };
        let separator = || div().flex_none().w(px(1.)).h(px(18.)).mx_1().bg(t.border);
        div()
            .id("build-toolbar")
            .debug_selector(|| "build-toolbar".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(TOOLBAR_HEIGHT)
            .px_2()
            .bg(t.menu_background)
            .border_b_1()
            .border_color(t.border)
            .text_size(t.typography.ui)
            .child(button(
                BACK_BUTTON,
                "\u{2190}",
                workspace::NAVIGATION_BACK,
                json!({}),
            ))
            .child(button(
                FORWARD_BUTTON,
                "\u{2192}",
                workspace::NAVIGATION_FORWARD,
                json!({}),
            ))
            .child(separator())
            .child(button(
                UNDO_BUTTON,
                "\u{21B6}",
                workspace::EDITOR_UNDO,
                json!({}),
            ))
            .child(button(
                REDO_BUTTON,
                "\u{21B7}",
                workspace::EDITOR_REDO,
                json!({}),
            ))
            .child(separator())
            .child(control)
            .child(button(
                START_WITHOUT_DEBUGGING_BUTTON,
                "\u{25B7}",
                debug::START,
                json!({ "debug": false }),
            ))
            .child(separator())
            .child(button(
                PAUSE_BUTTON,
                "\u{275A}\u{275A}",
                debug::PAUSE,
                json!({}),
            ))
            .child(button(STOP_BUTTON, "\u{25A0}", debug::STOP, json!({})))
            .child(button(
                RESTART_BUTTON,
                "\u{21BB}",
                debug::RESTART,
                json!({}),
            ))
            .child(separator())
            .child(button(
                STEP_OVER_BUTTON,
                "Step Over",
                debug::STEP_OVER,
                json!({}),
            ))
            .child(button(
                STEP_INTO_BUTTON,
                "Step Into",
                debug::STEP_INTO,
                json!({}),
            ))
            .child(button(
                STEP_OUT_BUTTON,
                "Step Out",
                debug::STEP_OUT,
                json!({}),
            ))
            .child(div().flex_1())
            .child(button(BUILD_BUTTON, "Build", build::SOLUTION, json!({})))
            .child(button(
                AGENTS_BUTTON,
                "Agents",
                view::SHOW,
                json!({ "id": eludite_docking::ids::AGENTS }),
            ))
    }
}

/// One part of the configuration and Start control.
fn segment(id: &'static str, t: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h_full()
        .px(px(10.))
        .border_r_1()
        .border_color(t.border)
        .text_color(t.text)
        .cursor_pointer()
        .hover(|s| s.bg(t.menu_hover))
}
