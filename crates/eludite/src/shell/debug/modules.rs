//! The Modules window (brief 0077; Debug > Windows > Modules, Ctrl+Alt+U): the modules the debug adapter reported
//! through DAP `module` events, per session, as Visual Studio lists them (Name, Path, Optimized, User Code, Symbol
//! Status, Version), and Visual Studio's "Loaded '...'. Symbols loaded." line for each new one.

use eludite_commands::debug_inspect::{MAX_MODULES, ModuleRow, ModulesOutput};
use eludite_dap::types::ModuleEvent;
use eludite_ui::Theme;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div, px, uniform_list,
};

use super::windows::ROW_HEIGHT;

/// One session's modules, in load order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Modules {
    rows: Vec<ModuleRow>,
}

impl Modules {
    /// Apply a `module` event; the Output line for a new module.
    pub fn apply(&mut self, e: &ModuleEvent) -> Option<String> {
        let m = &e.module;
        let row = ModuleRow {
            id: m.id.clone(),
            name: m.name.clone(),
            path: m.path.clone(),
            optimized: m.is_optimized,
            user_code: m.is_user_code,
            symbol_status: m.symbol_status.clone(),
            version: m.version.clone(),
        };
        let at = self
            .rows
            .iter()
            .position(|r| r.id == row.id && !row.id.is_empty());
        match (e.reason.as_str(), at) {
            ("removed", Some(i)) => {
                self.rows.remove(i);
                None
            }
            ("removed", None) => None,
            (_, Some(i)) => {
                self.rows[i] = row;
                None
            }
            (reason, None) => {
                let line = (reason == "new").then(|| loaded_line(&row));
                self.rows.push(row);
                line
            }
        }
    }

    pub fn rows(&self) -> &[ModuleRow] {
        &self.rows
    }

    pub fn output(&self) -> ModulesOutput {
        ModulesOutput {
            modules: self.rows.iter().take(MAX_MODULES).cloned().collect(),
            total: self.rows.len(),
        }
    }
}

/// Visual Studio's Output line for a loaded module.
pub fn loaded_line(row: &ModuleRow) -> String {
    let what = row.path.as_deref().unwrap_or(&row.name);
    let symbols = match row.symbol_status.as_deref() {
        Some(s) if s.to_lowercase().contains("skipped") || s.to_lowercase().contains("not") => {
            "Skipped loading symbols."
        }
        Some(s) if s.to_lowercase().contains("loaded") => "Symbols loaded.",
        _ => "",
    };
    if symbols.is_empty() {
        format!("Loaded '{what}'.")
    } else {
        format!("Loaded '{what}'. {symbols}")
    }
}

pub struct ModulesWindow {
    theme: Theme,
    rows: Vec<ModuleRow>,
}

impl ModulesWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
        }
    }

    pub fn set_rows(&mut self, rows: &[ModuleRow], cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows.to_vec();
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[ModuleRow] {
        &self.rows
    }
}

fn yes_no(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "",
    }
}

const COLUMNS: [(&str, Option<f32>); 6] = [
    ("Name", Some(180.)),
    ("Path", None),
    ("Optimized", Some(80.)),
    ("User Code", Some(80.)),
    ("Symbol Status", Some(160.)),
    ("Version", Some(100.)),
];

fn cell(text: String, width: Option<f32>) -> gpui::Div {
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
}

impl Render for ModulesWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .text_size(t.typography.small)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text_muted)
            .children(COLUMNS.iter().map(|(n, w)| cell((*n).to_owned(), *w)));
        let list = uniform_list(
            "debug-modules-rows",
            self.rows.len(),
            cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                let t = this.theme;
                range
                    .filter_map(|ix| {
                        let r = this.rows.get(ix)?;
                        let values = [
                            r.name.clone(),
                            r.path.clone().unwrap_or_default(),
                            yes_no(r.optimized).to_owned(),
                            yes_no(r.user_code).to_owned(),
                            r.symbol_status.clone().unwrap_or_default(),
                            r.version.clone().unwrap_or_default(),
                        ];
                        let sel = format!("debug-modules-row-{ix}");
                        Some(
                            div()
                                .id(SharedString::from(sel.clone()))
                                .debug_selector(move || sel)
                                .w_full()
                                .flex()
                                .flex_row()
                                .flex_none()
                                .h(px(ROW_HEIGHT))
                                .items_center()
                                .text_size(t.typography.ui)
                                .text_color(t.text)
                                .hover(|s| s.bg(t.menu_hover))
                                .children(
                                    values
                                        .into_iter()
                                        .zip(COLUMNS)
                                        .map(|(v, (_, w))| cell(v, w)),
                                ),
                        )
                    })
                    .collect()
            }),
        )
        .flex_1();
        div()
            .id("debug-modules")
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .children((self.rows.is_empty()).then(|| {
                div()
                    .p_2()
                    .text_size(t.typography.ui)
                    .text_color(t.text_muted)
                    .child("Modules are listed while debugging.")
            }))
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use eludite_dap::types::Module;

    use super::*;

    fn event(reason: &str, id: &str, status: Option<&str>) -> ModuleEvent {
        ModuleEvent {
            reason: reason.into(),
            module: Module {
                id: id.into(),
                name: format!("{id}.dll"),
                path: Some(format!("/app/{id}.dll")),
                symbol_status: status.map(str::to_owned),
                ..Module::default()
            },
        }
    }

    #[test]
    fn module_events_add_change_and_remove_rows() {
        let mut m = Modules::default();
        assert_eq!(
            m.apply(&event("new", "App", Some("Symbols loaded.")))
                .as_deref(),
            Some("Loaded '/app/App.dll'. Symbols loaded.")
        );
        assert_eq!(
            m.apply(&event(
                "new",
                "System.Runtime",
                Some("Skipped loading symbols.")
            ))
            .as_deref(),
            Some("Loaded '/app/System.Runtime.dll'. Skipped loading symbols.")
        );
        assert_eq!(m.apply(&event("changed", "App", None)), None);
        assert_eq!(m.rows().len(), 2);
        assert_eq!(m.rows()[0].symbol_status, None);
        m.apply(&event("removed", "App", None));
        assert_eq!(m.output().total, 1);
        assert_eq!(m.output().modules[0].name, "System.Runtime.dll");
    }
}
