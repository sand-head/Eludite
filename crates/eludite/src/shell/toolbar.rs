//! The Standard toolbar's lists (brief 0049, drawn at the right of the menu bar's row): Solution Configurations
//! (the solution's own, Debug and Release for a Cargo workspace, then Configuration Manager...), Solution Platforms
//! (the solution's own, then Configuration Manager...), the Target Framework list when the startup project is
//! multi-targeted, and the Debug toolbar's Start button with the launch profile list of the startup project. Every
//! choice runs a command (`eludite.solution.select_configuration`, `eludite.solution.set_configuration`,
//! `eludite.project.set_launch_profile` with `select`, `eludite.debug.start`), so agents and the toolbar agree.

use eludite_commands::project::properties::{self as props};
use eludite_ui::{Theme, toggle_button};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, anchored, deferred, div, px,
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

    /// Visual Studio's Standard toolbar lists and the Debug toolbar's Start, at the right of the menu bar's row.
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
        let mut buttons: Vec<(ToolbarList, &'static str, String)> = vec![
            (
                ToolbarList::Configuration,
                CONFIGURATION_BUTTON,
                configuration,
            ),
            (ToolbarList::Platform, PLATFORM_BUTTON, platform),
        ];
        if let Some(f) = framework {
            buttons.push((ToolbarList::Framework, FRAMEWORK_BUTTON, f));
        }
        let profile_at = buttons.len();
        if let Some(p) = &profile {
            buttons.push((ToolbarList::Profile, PROFILE_BUTTON, p.clone()));
        }
        let probe = self.ui_bounds.clone();
        let open = self.properties.toolbar_menu;
        let open_at = open.and_then(|o| buttons.iter().position(|(l, ..)| *l == o));
        let dropdowns: Vec<_> = buttons
            .iter()
            .enumerate()
            .map(|(ix, (list, id, label))| {
                let list = *list;
                let start = (ix == profile_at).then(|| {
                    let el = toggle_button(START_BUTTON, "\u{25B6}", false, &t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.run(eludite_commands::debug::START, json!({}), window, cx)
                        }),
                    );
                    probed(probe.as_ref(), el, START_BUTTON.into())
                });
                let button =
                    toggle_button(*id, format!("{label} \u{25BE}"), open == Some(list), &t)
                        .min_w(px(96.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.properties.toolbar_menu =
                                (this.properties.toolbar_menu != Some(list)).then_some(list);
                            cx.notify();
                        }));
                div().flex().flex_row().children(start).child(probed(
                    probe.as_ref(),
                    button,
                    (*id).into(),
                ))
            })
            .collect();
        let menu = open.zip(open_at).map(|(list, at)| {
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
                        .mt(px(22.))
                        .ml(px(at as f32 * 104.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.properties.toolbar_menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        div()
            .id("build-toolbar")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(t.typography.menu_bar_height)
            .px_2()
            .bg(t.menu_background)
            .text_size(t.typography.ui)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .children(dropdowns)
                    .children(menu),
            )
    }
}
