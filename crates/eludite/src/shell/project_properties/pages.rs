//! The project property pages as a document tab (brief 0049): Visual Studio's page list on the left (Application,
//! Build, Package, Debug, Code Analysis, then Resources, Settings and Signing with their "not yet" note) and, on the
//! right, the page's form generated from the host's catalog: a text box for text and lists, a check box for a bool,
//! one button per choice for an enum, a text box with Browse... for a path. Pages with per-configuration properties
//! show the Configuration and Platform lists at the top ("All Configurations" writes the unconditioned value, after a
//! confirmation when the property differs per configuration). Each value says where it comes from; an inherited one
//! (Directory.Build.props) is read-only behind a banner whose Override lets it be written in the project.
//!
//! Edits are dirty until Save (Ctrl+S, `eludite.editor.save` on the tab), which the shell sends as one
//! `eludite/project/setProperty` call. The Debug page is the launch profiles editor: its changes run
//! `eludite.project.set_launch_profile` at once, as Visual Studio's launch profiles dialog applies them.

use std::collections::BTreeSet;

use eludite_commands::project::properties::ProfileAction;
use eludite_lsp::host::{
    LaunchProfile, LaunchProfilesResult, ProjectPropertiesResult, ProjectProperty, PropertySource,
    PropertyType,
};
use eludite_ui::{BoundsMap, Theme, check_box, push_button, text_box, toggle_button};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, PromptLevel, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use serde_json::{Map, Value, json};

/// `el` recording its bounds under `key` while `--bounds-out` probes (the Xvfb run clicks them).
pub fn probed<E: ParentElement + Styled + IntoElement>(
    map: Option<&BoundsMap>,
    el: E,
    key: String,
) -> gpui::AnyElement {
    match map {
        None => el.into_any_element(),
        Some(_) => el
            .relative()
            .children(eludite_ui::bounds_canvas(map, key))
            .into_any_element(),
    }
}

/// Debug selectors.
pub fn page_selector(id: &str) -> String {
    format!("pp-page-{id}")
}

/// A property's editor: its text box or check box (enums: [`choice_selector`]).
pub fn property_selector(name: &str) -> String {
    format!("pp-prop-{name}")
}

pub fn choice_selector(name: &str, ix: usize) -> String {
    format!("pp-prop-{name}-{ix}")
}

pub fn browse_selector(name: &str) -> String {
    format!("pp-browse-{name}")
}

pub fn override_selector(name: &str) -> String {
    format!("pp-override-{name}")
}

/// The Configuration list's entry `ix` (the project's configurations, then All Configurations).
pub fn configuration_selector(ix: usize) -> String {
    format!("pp-configuration-{ix}")
}

pub fn platform_selector(ix: usize) -> String {
    format!("pp-platform-{ix}")
}

pub const SAVE: &str = "pp-save";
pub const PROFILE_NEW: &str = "pp-profile-new";
pub const PROFILE_DELETE: &str = "pp-profile-delete";
pub const PROFILE_RENAME: &str = "pp-profile-rename";
pub const ENV_ADD: &str = "pp-env-add";

/// The Debug page's profile `ix` of the list.
pub fn profile_selector(ix: usize) -> String {
    format!("pp-profile-{ix}")
}

/// The Debug page's editor of member `member` (`command_line_args`, `launch_browser`, ...).
pub fn profile_field_selector(member: &str) -> String {
    format!("pp-profile-field-{member}")
}

pub fn env_remove_selector(ix: usize) -> String {
    format!("pp-env-remove-{ix}")
}

/// The label "All Configurations" in the Configuration list.
pub const ALL_CONFIGURATIONS: &str = "All Configurations";

/// One unsaved change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyEdit {
    pub name: String,
    /// None: the element is removed (back to the default).
    pub value: Option<String>,
    pub configuration: Option<String>,
    pub platform: Option<String>,
    /// All Configurations, confirmed: the conditioned values go.
    pub all_configurations: bool,
    pub override_inherited: bool,
}

/// What the page asks the shell to do.
#[derive(Debug, Clone, PartialEq)]
pub enum PagesEvent {
    /// Show the values of this configuration and platform (None: the active selection's mapping).
    Load {
        configuration: Option<String>,
        platform: Option<String>,
    },
    /// The page became dirty or clean (the tab's `*`).
    Dirty(bool),
    /// Pick a file for property `key` (a catalog name, or `profile:<member>` on the Debug page).
    Browse { key: String },
    /// Save the dirty edits (Ctrl+S).
    Save,
    /// The Debug page: `eludite.project.set_launch_profile`.
    Profile {
        action: ProfileAction,
        profile: String,
        new_name: Option<String>,
        values: Option<Map<String, Value>>,
    },
}

pub struct PropertyPages {
    theme: Theme,
    pub project: String,
    pub name: String,
    pub tab: String,
    pub result: Option<ProjectPropertiesResult>,
    pub loading: bool,
    pub error: Option<String>,
    pub page: String,
    /// The Configuration list: None is All Configurations.
    pub configuration: Option<String>,
    pub platform: Option<String>,
    pub dirty: Vec<DirtyEdit>,
    /// Inherited properties whose banner's Override was clicked.
    pub overrides: BTreeSet<String>,
    /// The text being edited: its key and text so far.
    editing: Option<(String, String)>,
    /// The last save's or edit's outcome, shown under the header.
    pub message: Option<String>,
    pub launch: Option<LaunchProfilesResult>,
    /// The profile shown on the Debug page.
    pub profile: Option<String>,
    /// Where the controls are drawn, while `--bounds-out` probes.
    pub probe: Option<BoundsMap>,
    focus: FocusHandle,
}

impl EventEmitter<PagesEvent> for PropertyPages {}

impl Focusable for PropertyPages {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Short names of the pages, for the left list.
const PAGE_ORDER: [(&str, &str); 8] = [
    ("application", "Application"),
    ("build", "Build"),
    ("package", "Package"),
    ("debug", "Debug"),
    ("codeAnalysis", "Code Analysis"),
    ("resources", "Resources"),
    ("settings", "Settings"),
    ("signing", "Signing"),
];

impl PropertyPages {
    pub fn new(theme: Theme, project: String, tab: String, cx: &mut Context<Self>) -> Self {
        let name = std::path::Path::new(&project)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.clone());
        Self {
            theme,
            project,
            name,
            tab,
            result: None,
            loading: true,
            error: None,
            page: "application".into(),
            configuration: None,
            platform: None,
            dirty: Vec::new(),
            overrides: BTreeSet::new(),
            editing: None,
            message: None,
            launch: None,
            profile: None,
            probe: None,
            focus: cx.focus_handle(),
        }
    }

    /// The host's values for the configuration shown (they replace the previous ones; dirty edits stay).
    pub fn set_result(&mut self, result: ProjectPropertiesResult, cx: &mut Context<Self>) {
        if self.configuration.is_some() || self.result.is_none() {
            self.configuration = Some(result.configuration.clone());
        }
        self.platform = Some(result.platform.clone());
        self.result = Some(result);
        self.loading = false;
        self.error = None;
        cx.notify();
    }

    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(error);
        cx.notify();
    }

    pub fn set_launch(&mut self, launch: LaunchProfilesResult, cx: &mut Context<Self>) {
        let keep = self
            .profile
            .as_ref()
            .is_some_and(|p| launch.profiles.iter().any(|x| &x.name == p));
        if !keep {
            self.profile = launch.profiles.first().map(|p| p.name.clone());
        }
        self.launch = Some(launch);
        cx.notify();
    }

    pub fn select_profile(&mut self, name: &str, cx: &mut Context<Self>) {
        self.profile = Some(name.to_owned());
        cx.notify();
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Saved: the edits are no longer dirty.
    pub fn saved(&mut self, message: String, cx: &mut Context<Self>) {
        self.dirty.clear();
        self.message = Some(message);
        cx.emit(PagesEvent::Dirty(false));
        cx.notify();
    }

    pub fn set_message(&mut self, message: String, cx: &mut Context<Self>) {
        self.message = Some(message);
        cx.notify();
    }

    pub fn show_page(&mut self, page: &str, cx: &mut Context<Self>) {
        self.page = page.to_owned();
        self.editing = None;
        if page == "debug" && self.launch.is_none() {
            // The shell loads the profiles when the Debug page is asked for.
            cx.emit(PagesEvent::Load {
                configuration: self.configuration.clone(),
                platform: self.platform.clone(),
            });
        }
        cx.notify();
    }

    fn property(&self, name: &str) -> Option<&ProjectProperty> {
        self.result
            .as_ref()?
            .properties
            .iter()
            .find(|p| p.name == name)
    }

    /// The dirty edit that applies to `name` in the configuration shown.
    fn dirty_for(&self, name: &str, per_configuration: bool) -> Option<&DirtyEdit> {
        self.dirty.iter().rev().find(|d| {
            d.name == name
                && (!per_configuration
                    || d.all_configurations
                    || (d.configuration == self.configuration && d.platform == self.platform))
        })
    }

    /// What the page shows for `name`: the unsaved edit, else the host's value.
    pub fn shown_value(&self, name: &str) -> Option<String> {
        let p = self.property(name)?;
        Some(match self.dirty_for(name, p.per_configuration) {
            Some(d) => d.value.clone().unwrap_or_default(),
            None => p.value.clone(),
        })
    }

    /// The note under a value: where it comes from.
    pub fn source_note(&self, name: &str) -> Option<String> {
        let p = self.property(name)?;
        if self.dirty_for(name, p.per_configuration).is_some() {
            return Some("Changed (not saved)".into());
        }
        let file = |f: &str| {
            std::path::Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.to_owned())
        };
        Some(match p.source {
            PropertySource::Project => format!(
                "From the project file{}",
                p.defined_in
                    .as_ref()
                    .map(|d| format!(", line {}", d.line))
                    .unwrap_or_default()
            ),
            PropertySource::Conditioned => format!(
                "From the project file, under {}",
                p.defined_in
                    .as_ref()
                    .and_then(|d| d.condition.clone())
                    .unwrap_or_default()
                    .trim()
            ),
            PropertySource::Inherited => format!(
                "Inherited from {}",
                p.inherited_from.as_deref().map(file).unwrap_or_default()
            ),
            PropertySource::Default => "Default".into(),
        })
    }

    /// Record an edit of `name` to `value` in the configuration shown (All Configurations: confirmed first when the
    /// property differs per configuration).
    pub fn edit(
        &mut self,
        name: &str,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self.property(name).cloned() else {
            return;
        };
        let all = p.per_configuration && self.configuration.is_none();
        if all && p.conditions.as_ref().is_some_and(|c| !c.is_empty()) {
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Set {} for all configurations?", p.label),
                Some(
                    "The value differs per configuration. All Configurations writes one value and removes the \
                     per-configuration ones.",
                ),
                &["Set for All Configurations", "Cancel"],
                cx,
            );
            let name = name.to_owned();
            cx.spawn_in(window, async move |this, cx| {
                if answer.await == Ok(0) {
                    let _ = this.update(cx, |this, cx| this.push_edit(&name, value, true, cx));
                }
            })
            .detach();
            return;
        }
        self.push_edit(name, value, all, cx);
    }

    fn push_edit(&mut self, name: &str, value: Option<String>, all: bool, cx: &mut Context<Self>) {
        let Some(p) = self.property(name).cloned() else {
            return;
        };
        let (configuration, platform) = if p.per_configuration && !all {
            (self.configuration.clone(), self.platform.clone())
        } else {
            (None, None)
        };
        let was_dirty = self.is_dirty();
        self.dirty.retain(|d| {
            !(d.name == name && d.configuration == configuration && d.platform == platform)
        });
        if p.value != value.clone().unwrap_or_default() || p.per_configuration && all {
            self.dirty.push(DirtyEdit {
                name: name.to_owned(),
                value,
                configuration,
                platform,
                all_configurations: all && p.per_configuration,
                override_inherited: self.overrides.contains(name),
            });
        }
        if was_dirty != self.is_dirty() {
            cx.emit(PagesEvent::Dirty(self.is_dirty()));
        }
        cx.notify();
    }

    /// The Configuration list's choice (None: All Configurations).
    pub fn choose_configuration(&mut self, configuration: Option<String>, cx: &mut Context<Self>) {
        self.configuration = configuration.clone();
        if let Some(c) = configuration {
            self.loading = true;
            cx.emit(PagesEvent::Load {
                configuration: Some(c),
                platform: self.platform.clone(),
            });
        }
        cx.notify();
    }

    pub fn choose_platform(&mut self, platform: String, cx: &mut Context<Self>) {
        self.platform = Some(platform.clone());
        self.loading = true;
        cx.emit(PagesEvent::Load {
            configuration: self.configuration.clone(),
            platform: Some(platform),
        });
        cx.notify();
    }

    pub fn override_inherited(&mut self, name: &str, cx: &mut Context<Self>) {
        self.overrides.insert(name.to_owned());
        for d in self.dirty.iter_mut().filter(|d| d.name == name) {
            d.override_inherited = true;
        }
        cx.notify();
    }

    fn start_editing(
        &mut self,
        key: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing = Some((key, text));
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Commit the text being edited (Enter).
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, text)) = self.editing.take() else {
            return;
        };
        if let Some(member) = key.strip_prefix("profile:") {
            self.commit_profile(member, text, cx);
        } else {
            let value = (!text.is_empty()).then_some(text);
            self.edit(&key, value, window, cx);
        }
        cx.notify();
    }

    fn selected_profile(&self) -> Option<&LaunchProfile> {
        let launch = self.launch.as_ref()?;
        let name = self.profile.as_ref()?;
        launch.profiles.iter().find(|p| &p.name == name)
    }

    fn commit_profile(&mut self, member: &str, text: String, cx: &mut Context<Self>) {
        let Some(profile) = self.profile.clone() else {
            return;
        };
        match member {
            "rename" => {
                if !text.is_empty() && text != profile {
                    cx.emit(PagesEvent::Profile {
                        action: ProfileAction::Rename,
                        profile,
                        new_name: Some(text),
                        values: None,
                    });
                }
            }
            "env_add" => {
                let Some((name, value)) = text.split_once('=') else {
                    self.message = Some("Type NAME=value to add an environment variable.".into());
                    return;
                };
                let mut table = self.env_table();
                table.retain(|(n, _)| n != name.trim());
                table.push((name.trim().to_owned(), value.to_owned()));
                self.emit_env(profile, table, cx);
            }
            m => {
                let mut values = Map::new();
                values.insert(
                    m.to_owned(),
                    if text.is_empty() {
                        Value::Null
                    } else {
                        Value::String(text)
                    },
                );
                cx.emit(PagesEvent::Profile {
                    action: ProfileAction::Set,
                    profile,
                    new_name: None,
                    values: Some(values),
                });
            }
        }
    }

    fn env_table(&self) -> Vec<(String, String)> {
        self.selected_profile()
            .map(|p| {
                p.environment_variables
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn emit_env(&mut self, profile: String, table: Vec<(String, String)>, cx: &mut Context<Self>) {
        let rows: Vec<Value> = table
            .into_iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect();
        let mut values = Map::new();
        values.insert("environment_variables".into(), Value::Array(rows));
        cx.emit(PagesEvent::Profile {
            action: ProfileAction::Set,
            profile,
            new_name: None,
            values: Some(values),
        });
    }

    /// Type into the text being edited (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some((_, t)) = self.editing.as_mut() {
            t.push_str(text);
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match (k.key.as_str(), self.editing.as_mut()) {
            ("escape", Some(_)) => self.editing = None,
            ("enter", Some(_)) => self.commit(window, cx),
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

    fn text_editor(
        &self,
        key: String,
        shown: String,
        enabled: bool,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let editing = self
            .editing
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, text)| text.clone());
        let text = editing.clone().unwrap_or_else(|| shown.clone());
        let selector = if let Some(m) = key.strip_prefix("profile:") {
            profile_field_selector(m)
        } else {
            property_selector(&key)
        };
        let el = text_box(
            selector.clone(),
            &text,
            "",
            editing.is_some() && focused,
            &t,
        )
        .w(px(380.));
        if !enabled {
            return el.text_color(t.text_disabled).into_any_element();
        }
        let el = el.on_click(cx.listener(move |this, _, window, cx| {
            this.start_editing(key.clone(), shown.clone(), window, cx)
        }));
        probed(self.probe.as_ref(), el, selector)
    }

    fn property_row(
        &self,
        p: &ProjectProperty,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let name = p.name.clone();
        let inherited = p.source == PropertySource::Inherited
            && !self.overrides.contains(&p.name)
            && self.dirty_for(&p.name, p.per_configuration).is_none();
        let enabled = !p.read_only && !inherited && !self.loading;
        let shown = self.shown_value(&p.name).unwrap_or_default();
        let editor = match p.kind {
            PropertyType::Bool => {
                let on_value = p.true_value.clone().unwrap_or_else(|| "true".into());
                let off_value = p.false_value.clone().unwrap_or_else(|| "false".into());
                let on = shown.eq_ignore_ascii_case(&on_value);
                let el = check_box(property_selector(&name), p.label.clone(), on, &t);
                if enabled {
                    let key = property_selector(&name);
                    let el = el.on_click(cx.listener(move |this, _, window, cx| {
                        let v = if on {
                            off_value.clone()
                        } else {
                            on_value.clone()
                        };
                        this.edit(&name, Some(v), window, cx)
                    }));
                    probed(self.probe.as_ref(), el, key)
                } else {
                    el.text_color(t.text_disabled).into_any_element()
                }
            }
            PropertyType::Enum => {
                let mut values: Vec<(String, String)> = p
                    .values
                    .iter()
                    .flatten()
                    .map(|v| (v.value.clone(), v.label.clone()))
                    .collect();
                if !shown.is_empty() && !values.iter().any(|(v, _)| v.eq_ignore_ascii_case(&shown))
                {
                    values.push((shown.clone(), shown.clone()));
                }
                let probe = self.probe.clone();
                let choices = values.into_iter().enumerate().map(|(ix, (v, label))| {
                    let selected = v.eq_ignore_ascii_case(&shown);
                    let el = toggle_button(choice_selector(&name, ix), label, selected, &t);
                    let name = name.clone();
                    let key = choice_selector(&name, ix);
                    let el = if enabled {
                        el.on_click(cx.listener(move |this, _, window, cx| {
                            this.edit(&name, Some(v.clone()), window, cx)
                        }))
                    } else {
                        el
                    };
                    probed(probe.as_ref(), el, key)
                });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(p.label.clone())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap_1()
                            .children(choices),
                    )
                    .into_any_element()
            }
            _ => {
                let editor = self.text_editor(name.clone(), shown, enabled, focused, cx);
                let browse = (p.kind == PropertyType::Path && enabled).then(|| {
                    let key = name.clone();
                    push_button(browse_selector(&name), "Browse...", false, true, &t).on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(PagesEvent::Browse { key: key.clone() })
                        }),
                    )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(p.label.clone())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(editor)
                            .children(browse),
                    )
                    .into_any_element()
            }
        };
        let banner = inherited.then(|| {
            let name = p.name.clone();
            // The sentence shrinks and wraps and the button keeps its size, so a narrow pane (or wider platform fonts)
            // never pushes Override out of reach.
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .bg(t.panel_header)
                .text_size(t.typography.small)
                .child(div().flex_1().min_w_0().child(format!(
                    "This value is inherited from {}. Override it to set it in the project file.",
                    p.inherited_from.as_deref().unwrap_or("an imported file")
                )))
                .child(div().flex_shrink_0().child(
                    push_button(override_selector(&name), "Override", false, true, &t).on_click(
                        cx.listener(move |this, _, _, cx| this.override_inherited(&name, cx)),
                    ),
                ))
        });
        let differs = (p.per_configuration
            && self.configuration.is_none()
            && p.conditions.as_ref().is_some_and(|c| !c.is_empty()))
        .then(|| {
            div()
                .text_size(t.typography.small)
                .text_color(t.accent)
                .child("Differs per configuration.")
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .py_1()
            .child(editor)
            .children(banner)
            .children(differs)
            .child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(p.description.clone().unwrap_or_default()),
            )
            .children(self.source_note(&p.name).map(|n| {
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(n)
            }))
            .children(p.read_only_reason.clone().map(|r| {
                div()
                    .text_size(t.typography.small)
                    .text_color(t.accent)
                    .child(r)
            }))
            .into_any_element()
    }

    fn configuration_bar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.theme;
        let r = self.result.as_ref()?;
        let mut configs: Vec<Option<String>> = r.configurations.iter().cloned().map(Some).collect();
        configs.push(None);
        let config_buttons = configs.into_iter().enumerate().map(|(ix, c)| {
            let label = c.clone().unwrap_or_else(|| ALL_CONFIGURATIONS.into());
            let on = c == self.configuration;
            let el = toggle_button(configuration_selector(ix), label, on, &t).on_click(
                cx.listener(move |this, _, _, cx| this.choose_configuration(c.clone(), cx)),
            );
            probed(self.probe.as_ref(), el, configuration_selector(ix))
        });
        let platform_buttons = r.platforms.iter().cloned().enumerate().map(|(ix, p)| {
            let on = self.platform.as_deref() == Some(p.as_str());
            let el = toggle_button(platform_selector(ix), p.clone(), on, &t)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_platform(p.clone(), cx)));
            probed(self.probe.as_ref(), el, platform_selector(ix))
        });
        Some(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1()
                .pb_2()
                .border_b_1()
                .border_color(t.border)
                .child("Configuration:")
                .children(config_buttons)
                .child(div().w(px(16.)))
                .child("Platform:")
                .children(platform_buttons)
                .into_any_element(),
        )
    }

    fn debug_page(&self, focused: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let Some(launch) = &self.launch else {
            return div()
                .child("Loading the launch profiles\u{2026}")
                .into_any_element();
        };
        let list = launch
            .profiles
            .iter()
            .enumerate()
            .map(|(ix, p)| {
                let name = p.name.clone();
                let el = toggle_button(
                    profile_selector(ix),
                    p.name.clone(),
                    self.profile.as_ref() == Some(&p.name),
                    &t,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.select_profile(&name, cx)));
                probed(self.probe.as_ref(), el, profile_selector(ix))
            })
            .collect::<Vec<_>>();
        let next_name = (1..)
            .map(|n| format!("Profile {n}"))
            .find(|n| !launch.profiles.iter().any(|p| &p.name == n))
            .unwrap_or_default();
        let buttons =
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    push_button(PROFILE_NEW, "New", false, true, &t).on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.emit(PagesEvent::Profile {
                                action: ProfileAction::Create,
                                profile: next_name.clone(),
                                new_name: None,
                                values: None,
                            })
                        },
                    )),
                )
                .child(
                    push_button(PROFILE_DELETE, "Delete", false, self.profile.is_some(), &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(p) = this.profile.clone() {
                                cx.emit(PagesEvent::Profile {
                                    action: ProfileAction::Delete,
                                    profile: p,
                                    new_name: None,
                                    values: None,
                                })
                            }
                        })),
                )
                .child(
                    push_button(PROFILE_RENAME, "Rename", false, self.profile.is_some(), &t)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(p) = this.profile.clone() {
                                this.start_editing("profile:rename".into(), p, window, cx)
                            }
                        })),
                );
        let mut form: Vec<gpui::AnyElement> = Vec::new();
        if let Some(p) = self.selected_profile() {
            let read_only = p.read_only;
            if self
                .editing
                .as_ref()
                .is_some_and(|(k, _)| k == "profile:rename")
            {
                let rename_editor =
                    self.text_editor("profile:rename".into(), p.name.clone(), true, focused, cx);
                form.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child("New name")
                        .child(rename_editor)
                        .into_any_element(),
                );
            }
            form.push(
                div()
                    .text_color(t.text_muted)
                    .child(format!(
                        "Kind: {}{}",
                        p.command_name,
                        if read_only {
                            " (IIS Express profiles are read-only off Windows)"
                        } else {
                            ""
                        }
                    ))
                    .into_any_element(),
            );
            let mut fields = Vec::new();
            if p.command_name == "Executable" {
                fields.push(("Executable", "executable_path", p.executable_path.clone()));
            }
            fields.push((
                "Command line arguments",
                "command_line_args",
                p.command_line_args.clone(),
            ));
            fields.push((
                "Working directory",
                "working_directory",
                p.working_directory.clone(),
            ));
            fields.push(("App URL", "application_url", p.application_url.clone()));
            fields.push(("Launch URL", "launch_url", p.launch_url.clone()));
            for (label, member, value) in fields {
                let editor = self.text_editor(
                    format!("profile:{member}"),
                    value.unwrap_or_default(),
                    !read_only,
                    focused,
                    cx,
                );
                form.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(label.to_owned())
                        .child(editor)
                        .into_any_element(),
                );
            }
            for (label, member, value) in [
                ("Launch browser", "launch_browser", p.launch_browser),
                (
                    "Show dotnet run messages",
                    "dotnet_run_messages",
                    p.dotnet_run_messages,
                ),
                (
                    "Enable Hot Reload",
                    "hot_reload_enabled",
                    p.hot_reload_enabled,
                ),
            ] {
                let on = value.unwrap_or(false);
                let profile = p.name.clone();
                let el = check_box(profile_field_selector(member), label, on, &t);
                form.push(if read_only {
                    el.into_any_element()
                } else {
                    let key = profile_field_selector(member);
                    let el = el.on_click(cx.listener(move |_, _, _, cx| {
                        let mut values = Map::new();
                        values.insert(member.to_owned(), Value::Bool(!on));
                        cx.emit(PagesEvent::Profile {
                            action: ProfileAction::Set,
                            profile: profile.clone(),
                            new_name: None,
                            values: Some(values),
                        })
                    }));
                    probed(self.probe.as_ref(), el, key)
                });
            }
            let rows: Vec<_> = p
                .environment_variables
                .iter()
                .enumerate()
                .map(|(ix, e)| {
                    let profile = p.name.clone();
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(200.)).child(e.name.clone()))
                        .child(div().w(px(240.)).child(e.value.clone()))
                        .children((!read_only).then(|| {
                            push_button(env_remove_selector(ix), "\u{2715}", false, true, &t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let mut table = this.env_table();
                                    if ix < table.len() {
                                        table.remove(ix);
                                    }
                                    this.emit_env(profile.clone(), table, cx);
                                }))
                        }))
                })
                .collect();
            let add = (!read_only).then(|| {
                self.text_editor("profile:env_add".into(), String::new(), true, focused, cx)
            });
            form.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("Environment variables")
                    .children(rows)
                    .children(add.map(|add| {
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(add)
                            .child(
                                div()
                                    .id(ENV_ADD)
                                    .text_size(t.typography.small)
                                    .text_color(t.text_muted)
                                    .child("NAME=value, then Enter"),
                            )
                    }))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(t.text_muted)
                    .child(format!("Launch profiles ({})", launch.file)),
            )
            .child(div().flex().flex_row().flex_wrap().gap_1().children(list))
            .child(buttons)
            .children(form)
            .into_any_element()
    }
}

impl Render for PropertyPages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let pages = self
            .result
            .as_ref()
            .map(|r| {
                r.pages
                    .iter()
                    .map(|p| {
                        (
                            p.id.clone(),
                            p.title.clone(),
                            p.state.clone(),
                            p.note.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                PAGE_ORDER
                    .iter()
                    .map(|(id, title)| ((*id).to_owned(), (*title).to_owned(), String::new(), None))
                    .collect()
            });
        let list = pages
            .iter()
            .map(|(id, title, _, _)| {
                let sel = page_selector(id);
                let selected = *id == self.page;
                let id = id.clone();
                let probe = eludite_ui::bounds_canvas(self.probe.as_ref(), sel.clone());
                let item = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .relative()
                    .children(probe)
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child(title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.show_page(&id, cx)));
                if selected {
                    item.bg(t.accent)
                        .text_color(t.text_on_accent)
                        .into_any_element()
                } else {
                    item.hover(|s| s.bg(t.menu_hover)).into_any_element()
                }
            })
            .collect::<Vec<_>>();
        let page = pages.iter().find(|(id, ..)| *id == self.page).cloned();
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        if let Some(e) = &self.error {
            body.push(
                div()
                    .text_color(t.accent)
                    .child(e.clone())
                    .into_any_element(),
            );
        } else if let Some((_, _, state, note)) = &page {
            match state.as_str() {
                "notYet" => body.push(
                    div()
                        .text_color(t.text_muted)
                        .child(note.clone().unwrap_or_else(|| "Not yet.".into()))
                        .into_any_element(),
                ),
                "launchProfiles" => body.push(self.debug_page(focused, cx)),
                _ => {
                    let props: Vec<ProjectProperty> = self
                        .result
                        .as_ref()
                        .map(|r| {
                            r.properties
                                .iter()
                                .filter(|p| p.page == self.page)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    if props.iter().any(|p| p.per_configuration)
                        && let Some(bar) = self.configuration_bar(cx)
                    {
                        body.push(bar);
                    }
                    let mut section: Option<String> = None;
                    for p in &props {
                        if p.section != section {
                            section = p.section.clone();
                            body.push(
                                eludite_ui::section_heading(
                                    section.clone().unwrap_or_default(),
                                    &t,
                                )
                                .into_any_element(),
                            );
                        }
                        body.push(self.property_row(p, focused, cx));
                    }
                }
            }
        } else if self.loading {
            body.push(div().child("Loading\u{2026}").into_any_element());
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pb_2()
            .child(
                div()
                    .text_size(px(18.))
                    .child(format!("{} \u{2014} Properties", self.name)),
            )
            .child(div().flex_1())
            .children(
                self.loading
                    .then(|| div().text_color(t.text_muted).child("Evaluating\u{2026}")),
            )
            .child(probed(
                self.probe.as_ref(),
                push_button(SAVE, "Save (Ctrl+S)", false, self.is_dirty(), &t)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PagesEvent::Save))),
                SAVE.into(),
            ));
        div()
            .id(SharedString::from(format!("pp-{}", self.tab)))
            .track_focus(&self.focus)
            .key_context("PropertyPages")
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_row()
            .bg(t.background)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(
                div()
                    .id("pp-pages")
                    .flex()
                    .flex_col()
                    .w(px(180.))
                    .py_2()
                    .border_r_1()
                    .border_color(t.border)
                    .children(list),
            )
            .child(
                div()
                    .id("pp-form")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .p_3()
                    .overflow_y_scroll()
                    .child(header)
                    .children(self.message.clone().map(|m| {
                        div()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(m)
                    }))
                    .children(body),
            )
    }
}
