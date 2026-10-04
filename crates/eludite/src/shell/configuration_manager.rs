//! Build > Configuration Manager... (brief 0049): Visual Studio's dialog. The Active solution configuration and
//! Active solution platform lists at the top select (`eludite.solution.select_configuration`), and the grid has one row
//! per project with its Configuration, Platform and Build for the active selection; a change runs
//! `eludite.solution.set_configuration` with that cell, which edits the solution file through eludite-host (its
//! formatting kept) and reloads the solution. Close (or Escape) closes it.

use eludite_commands::project::properties::ProjectConfigurationsRow;
use eludite_ui::{BoundsMap, Theme, check_box, dialog_panel, push_button, toggle_button};
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use super::Shell;
use super::project_properties::pages::probed;

pub const DIALOG: &str = "cm-dialog";
pub const CLOSE: &str = "cm-close";

pub fn configuration_selector(ix: usize) -> String {
    format!("cm-configuration-{ix}")
}

pub fn platform_selector(ix: usize) -> String {
    format!("cm-platform-{ix}")
}

/// Row `row`'s configuration choice `ix`.
pub fn cell_configuration_selector(row: usize, ix: usize) -> String {
    format!("cm-row-{row}-configuration-{ix}")
}

pub fn cell_platform_selector(row: usize, ix: usize) -> String {
    format!("cm-row-{row}-platform-{ix}")
}

pub fn build_selector(row: usize) -> String {
    format!("cm-row-{row}-build")
}

#[derive(Debug, Clone, PartialEq)]
pub enum ManagerEvent {
    /// Run `command` with `args` (the selection or a cell).
    Run {
        command: &'static str,
        args: Value,
    },
    Close,
}

pub struct ConfigurationManager {
    theme: Theme,
    rows: Vec<ProjectConfigurationsRow>,
    configurations: Vec<String>,
    platforms: Vec<String>,
    configuration: String,
    platform: String,
    /// Where the controls are drawn, while `--bounds-out` probes.
    pub probe: Option<BoundsMap>,
    focus: FocusHandle,
}

impl EventEmitter<ManagerEvent> for ConfigurationManager {}

impl Focusable for ConfigurationManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ConfigurationManager {
    pub fn new(
        theme: Theme,
        configurations: Vec<String>,
        platforms: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            configuration: configurations.first().cloned().unwrap_or_default(),
            platform: platforms.first().cloned().unwrap_or_default(),
            configurations,
            platforms,
            probe: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_lists(&mut self, configurations: Vec<String>, platforms: Vec<String>) {
        self.configurations = configurations;
        self.platforms = platforms;
    }

    /// The projects' mapping and the active selection.
    pub fn set_rows(
        &mut self,
        rows: Vec<ProjectConfigurationsRow>,
        configuration: String,
        platform: String,
        cx: &mut Context<Self>,
    ) {
        self.rows = rows;
        self.configuration = configuration;
        self.platform = platform;
        cx.notify();
    }

    /// A row's cell in the active selection: its configuration, platform and Build.
    pub fn cell(&self, row: usize) -> Option<(String, String, bool)> {
        let r = self.rows.get(row)?;
        let m = r.mappings.iter().find(|m| {
            m.solution_configuration
                .eq_ignore_ascii_case(&self.configuration)
                && m.solution_platform.eq_ignore_ascii_case(&self.platform)
        });
        Some(match m {
            Some(m) => (m.configuration.clone(), m.platform.clone(), m.build),
            None => (self.configuration.clone(), self.platform.clone(), false),
        })
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            cx.stop_propagation();
            cx.emit(ManagerEvent::Close);
        }
    }

    fn cell_event(&self, row: usize, field: &str, value: Value) -> ManagerEvent {
        let project = self.rows[row].path.clone();
        let mut cell = json!({ "project": project, "solution_configuration": self.configuration,
                               "solution_platform": self.platform });
        cell[field] = value;
        ManagerEvent::Run {
            command: eludite_commands::solution::SET_CONFIGURATION,
            args: json!({ "mappings": [cell] }),
        }
    }
}

impl Render for ConfigurationManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let configs = self.configurations.iter().enumerate().map(|(ix, c)| {
            let c = c.clone();
            let el = toggle_button(
                configuration_selector(ix),
                c.clone(),
                c == self.configuration,
                &t,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ManagerEvent::Run {
                    command: eludite_commands::solution::SELECT_CONFIGURATION,
                    args: json!({ "configuration": c }),
                })
            }));
            probed(self.probe.as_ref(), el, configuration_selector(ix))
        });
        let platforms = self.platforms.iter().enumerate().map(|(ix, p)| {
            let p = p.clone();
            toggle_button(platform_selector(ix), p.clone(), p == self.platform, &t).on_click(
                cx.listener(move |_, _, _, cx| {
                    cx.emit(ManagerEvent::Run {
                        command: eludite_commands::solution::SELECT_CONFIGURATION,
                        args: json!({ "platform": p }),
                    })
                }),
            )
        });
        let header = div()
            .flex()
            .flex_row()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(t.border)
            .text_color(t.text_muted)
            .child(div().w(px(200.)).child("Project"))
            .child(div().w(px(220.)).child("Configuration"))
            .child(div().w(px(220.)).child("Platform"))
            .child(div().w(px(60.)).child("Build"));
        let mut grid = Vec::new();
        for (row, r) in self.rows.iter().enumerate() {
            let (configuration, platform, build) = self.cell(row).unwrap_or_default();
            let cfg_choices = r.configurations.iter().enumerate().map(|(ix, c)| {
                let ev = self.cell_event(row, "configuration", json!(c));
                let el = toggle_button(
                    cell_configuration_selector(row, ix),
                    c.clone(),
                    *c == configuration,
                    &t,
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ev.clone())));
                probed(
                    self.probe.as_ref(),
                    el,
                    cell_configuration_selector(row, ix),
                )
            });
            let plat_choices = r.platforms.iter().enumerate().map(|(ix, p)| {
                let ev = self.cell_event(row, "platform", json!(p));
                toggle_button(
                    cell_platform_selector(row, ix),
                    p.clone(),
                    *p == platform,
                    &t,
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ev.clone())))
            });
            let build_ev = self.cell_event(row, "build", json!(!build));
            grid.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .child(div().w(px(200.)).child(r.name.clone()))
                    .child(
                        div()
                            .w(px(220.))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_1()
                            .children(cfg_choices),
                    )
                    .child(
                        div()
                            .w(px(220.))
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_1()
                            .children(plat_choices),
                    )
                    .child(div().w(px(60.)).child(
                        probed(
                            self.probe.as_ref(),
                            check_box(build_selector(row), "", build, &t).on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(build_ev.clone())),
                            ),
                            build_selector(row),
                        ),
                    )),
            );
        }
        let panel = dialog_panel(&t, "Configuration Manager")
            .id(DIALOG)
            .debug_selector(|| DIALOG.into())
            .track_focus(&self.focus)
            .key_context("ConfigurationManager")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(780.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child("Active solution configuration:")
                            .children(configs),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child("Active solution platform:")
                            .children(platforms),
                    )
                    .child(div().text_color(t.text_muted).child(
                        "Project contexts (check the project configurations to build or deploy):",
                    ))
                    .child(
                        div()
                            .id("cm-grid")
                            .flex()
                            .flex_col()
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .child(header)
                            .children(grid),
                    ),
            )
            .child(
                div().flex().flex_row().justify_end().p_3().child(probed(
                    self.probe.as_ref(),
                    push_button(CLOSE, "Close", true, true, &t)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ManagerEvent::Close))),
                    CLOSE.into(),
                )),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(780.)) / 2.).max(px(0.)),
            (viewport.height / 8.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(5)
    }
}

impl Shell {
    pub(super) fn open_configuration_manager(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (configurations, platforms) =
            (self.solution_configurations(), self.solution_platforms());
        let manager = match self.properties.manager.clone() {
            Some(m) => m,
            None => {
                let theme = self.theme;
                let (c, p) = (configurations.clone(), platforms.clone());
                let probe = self.ui_bounds.clone();
                let m = cx.new(|cx| {
                    let mut m = ConfigurationManager::new(theme, c, p, cx);
                    m.probe = probe;
                    m
                });
                cx.subscribe_in(&m, window, Self::on_manager_event).detach();
                self.properties.manager = Some(m.clone());
                m
            }
        };
        let rows = self.configuration_rows();
        let (sc, sp) = self.active_selection();
        manager.update(cx, |m, cx| {
            m.set_lists(configurations, platforms);
            m.set_rows(rows, sc, sp, cx)
        });
        manager.focus_handle(cx).focus(window, cx);
        self.request_configurations(window, cx);
        cx.notify();
    }

    fn on_manager_event(
        &mut self,
        _: &gpui::Entity<ConfigurationManager>,
        event: &ManagerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ManagerEvent::Run { command, args } => self.run(command, args.clone(), window, cx),
            ManagerEvent::Close => {
                self.properties.manager = None;
                self.focus.focus(window, cx);
                cx.notify();
            }
        }
    }

    /// The dialog, when open (drawn over the window).
    pub(super) fn configuration_manager_overlay(
        &self,
    ) -> Option<gpui::Entity<ConfigurationManager>> {
        self.properties.manager.clone()
    }
}
