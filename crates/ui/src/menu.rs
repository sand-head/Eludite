//! The menu bar: Visual Studio's menus, each item bound to a command id.
//!
//! Items whose command is registered on the bus are enabled and dispatch
//! [`RunCommand`] (the same action key bindings produce). Items for commands
//! that do not exist yet are drawn disabled and do nothing.

use std::rc::Rc;

use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use crate::keymap::{KeyBindingSpec, RunCommand, shortcut_for};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    Item {
        label: &'static str,
        command: &'static str,
        args: Value,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: &'static str,
    pub entries: Vec<MenuEntry>,
}

fn item(label: &'static str, command: &'static str) -> MenuEntry {
    MenuEntry::Item {
        label,
        command,
        args: json!({}),
    }
}

fn show(label: &'static str, id: &str) -> MenuEntry {
    MenuEntry::Item {
        label,
        command: "eludite.view.show",
        args: json!({ "id": id }),
    }
}

/// The menu titles, in Visual Studio's order (PLAN.md 8).
pub const MENU_TITLES: [&str; 13] = [
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

/// Visual Studio's menus. Command ids that are not registered yet are stubs:
/// the bar shows them disabled until a later brief registers the command.
pub fn vs_menus() -> Vec<Menu> {
    use MenuEntry::Separator;
    let menu = |title, entries| Menu { title, entries };
    vec![
        menu(
            "File",
            vec![
                item("New Project...", "eludite.file.new_project"),
                // A workspace is any folder: a Cargo workspace, a .NET solution, both, or neither.
                item("Open Workspace...", "eludite.workspace.open_folder"),
                // .NET-specific: open a solution or project file directly.
                item("Open Solution or Project File...", "eludite.solution.open"),
                item("Open File...", "eludite.file.open_file_dialog"),
                Separator,
                item("Save", "eludite.editor.save"),
                item("Save All", "eludite.file.save_all"),
                Separator,
                item("Close Workspace", "eludite.workspace.close"),
                item("Exit", "eludite.file.exit"),
            ],
        ),
        menu(
            "Edit",
            vec![
                item("Undo", "eludite.editor.undo"),
                item("Redo", "eludite.editor.redo"),
                Separator,
                item("Cut", "eludite.edit.cut"),
                item("Copy", "eludite.edit.copy"),
                item("Paste", "eludite.edit.paste"),
                Separator,
                item("Find and Replace", "eludite.editor.find"),
                item("Go To All", "eludite.edit.go_to_all"),
                item("Go To Definition", "eludite.editor.go_to_definition"),
                item("Find All References", "eludite.editor.find_references"),
                Separator,
                // Visual Studio's Edit > Refactor > Rename and the editor's Quick Actions and Refactorings.
                item("Rename...", "eludite.editor.rename"),
                item(
                    "Quick Actions and Refactorings...",
                    "eludite.editor.code_actions",
                ),
                Separator,
                // Visual Studio's Edit > IntelliSense items.
                item("Complete Word", "eludite.editor.complete"),
                item("Parameter Info", "eludite.editor.signature_help"),
                item("Quick Info", "eludite.editor.hover"),
            ],
        ),
        menu(
            "View",
            vec![
                show("Workspace", "workspace"),
                show("Git Changes", "git_changes"),
                show("Agents", "agents"),
                Separator,
                show("Error List", "error_list"),
                show("Output", "output"),
                show("Properties Window", "properties"),
                show("Toolbox", "toolbox"),
                Separator,
                item("Navigate Backward", "eludite.navigation.back"),
                item("Navigate Forward", "eludite.navigation.forward"),
                Separator,
                item("Command Palette", "eludite.view.command_palette"),
            ],
        ),
        menu(
            "Git",
            vec![
                item("Commit or Stash...", "eludite.git.commit"),
                item("Fetch", "eludite.git.fetch"),
                item("Pull", "eludite.git.pull"),
                item("Push", "eludite.git.push"),
                Separator,
                item("Manage Branches", "eludite.git.branches"),
            ],
        ),
        menu(
            "Project",
            vec![
                item("Add Class...", "eludite.project.add_class"),
                item("Add New Item...", "eludite.project.add_item"),
                Separator,
                item("Manage NuGet Packages...", "eludite.nuget.manage"),
                item("Properties", "eludite.project.properties"),
            ],
        ),
        menu(
            "Build",
            vec![
                item("Build Solution", "eludite.build.solution"),
                item("Rebuild Solution", "eludite.build.rebuild"),
                item("Clean Solution", "eludite.build.clean"),
                Separator,
                // The active document's project (Visual Studio names it in the label).
                item("Build Project", "eludite.build.project"),
                MenuEntry::Item {
                    label: "Rebuild Project",
                    command: "eludite.build.project",
                    args: json!({ "target": "rebuild" }),
                },
                MenuEntry::Item {
                    label: "Clean Project",
                    command: "eludite.build.project",
                    args: json!({ "target": "clean" }),
                },
                Separator,
                item("Cancel", "eludite.build.cancel"),
            ],
        ),
        menu(
            "Debug",
            vec![
                item("Start Debugging", "eludite.debug.start"),
                MenuEntry::Item {
                    label: "Start Without Debugging",
                    command: "eludite.debug.start",
                    args: json!({ "debug": false }),
                },
                item("Stop Debugging", "eludite.debug.stop"),
                item("Continue", "eludite.debug.continue"),
                item("Attach to Process...", "eludite.debug.attach"),
                Separator,
                item("Step Into", "eludite.debug.step_into"),
                item("Step Over", "eludite.debug.step_over"),
                item("Step Out", "eludite.debug.step_out"),
                item("Run To Cursor", "eludite.debug.run_to_cursor"),
                Separator,
                item("Toggle Breakpoint", "eludite.debug.toggle_breakpoint"),
                MenuEntry::Item {
                    label: "Delete All Breakpoints",
                    command: "eludite.debug.toggle_breakpoint",
                    args: json!({ "action": "delete_all" }),
                },
                Separator,
                show("Windows > Breakpoints", "breakpoints"),
                show("Windows > Exception Settings", "exception_settings"),
                show("Windows > Output", "output"),
                show("Windows > Watch 1", "watch"),
                show("Windows > Locals", "locals"),
                show("Windows > Call Stack", "call_stack"),
                show("Windows > Threads", "threads"),
            ],
        ),
        menu(
            "Test",
            vec![
                item("Run All Tests", "eludite.test.run_all"),
                item("Debug All Tests", "eludite.test.debug_all"),
                Separator,
                item("Test Explorer", "eludite.test.explorer"),
            ],
        ),
        menu(
            "Analyze",
            vec![
                item("Code Cleanup", "eludite.analyze.cleanup"),
                item("Run Code Analysis", "eludite.analyze.run"),
            ],
        ),
        menu(
            "Tools",
            vec![
                item("Command Line", "eludite.tools.terminal"),
                Separator,
                item("Options...", "eludite.tools.options"),
            ],
        ),
        menu(
            "Extensions",
            vec![item("Manage Extensions", "eludite.extensions.manage")],
        ),
        menu(
            "Window",
            vec![
                item("Float", "eludite.view.float"),
                item("Dock", "eludite.view.dock"),
                item("Auto Hide", "eludite.view.auto_hide"),
                item("Hide", "eludite.view.hide"),
                Separator,
                item("Pin Tab", "eludite.window.pin_tab"),
                Separator,
                item("Save Window Layout", "eludite.window.save_layout"),
                item("Apply Window Layout", "eludite.window.apply_layout"),
                item("Reset Window Layout", "eludite.view.reset_layout"),
            ],
        ),
        menu(
            "Help",
            vec![
                item("View Help", "eludite.help.view"),
                Separator,
                item("About Eludite", "eludite.help.about"),
            ],
        ),
    ]
}

/// Whether a command id is registered (the item is enabled).
pub type IsEnabled = Rc<dyn Fn(&str) -> bool>;

/// The menu bar view: titles, and the open drop-down.
pub struct MenuBar {
    menus: Vec<Menu>,
    keymap: Vec<KeyBindingSpec>,
    theme: Theme,
    is_enabled: IsEnabled,
    open: Option<usize>,
    /// Records where the titles (`menu-<title>`) and the open menu's items (`menu-item-<title>-<label>`) are drawn
    /// (the manual runs' real-input drivers).
    probe: Option<crate::BoundsMap>,
}

impl MenuBar {
    pub fn new(
        menus: Vec<Menu>,
        keymap: Vec<KeyBindingSpec>,
        theme: Theme,
        is_enabled: IsEnabled,
    ) -> Self {
        Self {
            menus,
            keymap,
            theme,
            is_enabled,
            open: None,
            probe: None,
        }
    }

    /// Record where the titles and the open menu's items are drawn (`eludite --bounds-out`).
    pub fn set_probe(&mut self, probe: Option<crate::BoundsMap>) {
        self.probe = probe;
    }

    pub fn open_menu(&self) -> Option<&str> {
        self.open.map(|ix| self.menus[ix].title)
    }

    pub fn set_open(&mut self, title: Option<&str>, cx: &mut Context<Self>) {
        self.open = title.and_then(|t| self.menus.iter().position(|m| m.title == t));
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Whether the item labelled `label` in menu `title` is enabled.
    pub fn is_item_enabled(&self, title: &str, label: &str) -> Option<bool> {
        self.menus
            .iter()
            .find(|m| m.title == title)?
            .entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Item {
                    label: l, command, ..
                } if *l == label => Some((self.is_enabled)(command)),
                _ => None,
            })
    }

    /// The labels of `title`'s enabled items.
    pub fn enabled_labels(&self, title: &str) -> Vec<&'static str> {
        self.menus
            .iter()
            .find(|m| m.title == title)
            .map(|m| {
                m.entries
                    .iter()
                    .filter_map(|e| match e {
                        MenuEntry::Item { label, command, .. } if (self.is_enabled)(command) => {
                            Some(*label)
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn render_popup(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let ty = t.typography;
        let menu = &self.menus[ix];
        let title = menu.title;
        let rows = menu
            .entries
            .iter()
            .enumerate()
            .map(|(row, entry)| match entry {
                MenuEntry::Separator => div()
                    .h(px(1.))
                    .my_1()
                    .mx_2()
                    .bg(t.popup_border)
                    .into_any_element(),
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => {
                    let enabled = (self.is_enabled)(command);
                    let shortcut = shortcut_for(&self.keymap, command, args).unwrap_or_default();
                    let selector = format!("menu-item-{title}-{label}");
                    let probed = crate::bounds_canvas(self.probe.as_ref(), selector.clone());
                    let mut el = div()
                        .id(("menu-item", row))
                        .relative()
                        .children(probed)
                        .debug_selector(move || selector.clone())
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(24.))
                        .pl_6()
                        .pr_3()
                        .gap_8()
                        .child(div().flex_1().child(*label))
                        .child(div().text_size(ty.small).child(shortcut));
                    if enabled {
                        let action = RunCommand::new(*command, args.clone());
                        el = el
                            .text_color(t.menu_text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.menu_hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open = None;
                                cx.notify();
                                window.dispatch_action(Box::new(action.clone()), cx);
                            }));
                    } else {
                        el = el.text_color(t.text_disabled);
                    }
                    el.into_any_element()
                }
            })
            .collect::<Vec<_>>();
        div()
            .id("menu-popup")
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .py_1()
            .bg(t.popup_background)
            .border_1()
            .border_color(t.popup_border)
            .shadow_md()
            .text_size(ty.ui)
            .children(rows)
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let ty = t.typography;
        let open = self.open;
        let titles = self.menus.iter().enumerate().map(|(ix, m)| {
            let title = m.title;
            let is_open = open == Some(ix);
            let mut el = div()
                .id(("menu", ix))
                .debug_selector(move || format!("menu-{title}"))
                .relative()
                .flex()
                .items_center()
                .h_full()
                .px_2()
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.open = if this.open == Some(ix) {
                            None
                        } else {
                            Some(ix)
                        };
                        cx.notify();
                    }),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.open.is_some() && this.open != Some(ix) {
                        this.open = Some(ix);
                        cx.notify();
                    }
                }))
                .children(crate::bounds_canvas(
                    self.probe.as_ref(),
                    format!("menu-{title}"),
                ))
                .child(title);
            if is_open {
                el = el.bg(t.menu_hover).child(
                    deferred(
                        div()
                            .absolute()
                            .top(ty.menu_bar_height)
                            .left_0()
                            .child(self.render_popup(ix, cx)),
                    )
                    .with_priority(2),
                );
            }
            el
        });

        let mut bar = div()
            .id("menu-bar")
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(ty.menu_bar_height)
            .px_1()
            .bg(t.menu_background)
            .text_size(ty.ui)
            .text_color(t.menu_text)
            .font_weight(FontWeight::NORMAL)
            .children(titles);
        if open.is_some() {
            // Clicking anywhere below the bar closes the menu.
            let size = window.viewport_size();
            let top = ty.menu_bar_height;
            bar = bar.child(
                deferred(
                    anchored().position(point(px(0.), top)).child(
                        div()
                            .id("menu-backdrop")
                            .debug_selector(|| "menu-backdrop".into())
                            .occlude()
                            .w(size.width)
                            .h(size.height - top)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open = None;
                                    cx.notify();
                                }),
                            ),
                    ),
                )
                .with_priority(1),
            );
        }
        bar
    }
}

/// Convenience for building a [`MenuBar`] with an `is_enabled` closure.
pub fn menu_bar_with(
    theme: Theme,
    keymap: Vec<KeyBindingSpec>,
    is_enabled: impl Fn(&str) -> bool + 'static,
) -> MenuBar {
    MenuBar::new(vs_menus(), keymap, theme, Rc::new(is_enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_in_vs_order() {
        let titles: Vec<_> = vs_menus().iter().map(|m| m.title).collect();
        assert_eq!(titles, MENU_TITLES);
    }

    #[test]
    fn every_item_names_a_valid_command_id() {
        for m in vs_menus() {
            assert!(!m.entries.is_empty(), "{}", m.title);
            for e in &m.entries {
                if let MenuEntry::Item { command, args, .. } = e {
                    let segments: Vec<_> = command.split('.').collect();
                    assert!(segments.len() >= 2, "{command}");
                    assert!(
                        segments.iter().all(|s| s
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')),
                        "{command}"
                    );
                    assert!(args.is_object(), "{command}");
                }
            }
        }
    }

    #[test]
    fn view_menu_shows_keymap_shortcuts() {
        let keymap = crate::keymap::vs_keymap();
        let view = vs_menus().into_iter().find(|m| m.title == "View").unwrap();
        let shortcuts: Vec<_> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator => None,
            })
            .collect();
        assert!(shortcuts.contains(&("Workspace", Some("Ctrl+Alt+L"))));
        assert!(shortcuts.contains(&("Error List", Some("Ctrl+\\, Ctrl+E"))));
        assert!(shortcuts.contains(&("Command Palette", None)));
        let build = vs_menus().into_iter().find(|m| m.title == "Build").unwrap();
        let shortcuts: Vec<_> = build
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator => None,
            })
            .collect();
        assert!(shortcuts.contains(&("Build Solution", Some("Ctrl+Shift+B"))));
        assert!(shortcuts.contains(&("Build Project", Some("Shift+F6"))));
        assert!(shortcuts.contains(&("Rebuild Project", None)));
    }
}
