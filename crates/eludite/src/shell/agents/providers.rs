//! OpenAI-compatible servers in the Agents window (brief 0059): `eludite.agents.provider_set`, `provider_remove`
//! and `provider_models` over `agents.json`'s `providers` and the credential store, the keys read off the UI thread
//! for each launch, the Add server dialog and the servers' list on Tools > Options > Agents.
//!
//! - **Settings and keys.** A server is an entry in `agents.json`'s `providers` (`eludite_acp::ProviderSettings`); its
//!   key is never in the file: [`ProviderStore`] keeps it with `eludite_forge::credentials::Credentials` (the
//!   operating system's store, else the 0600 file with the person's consent, brief 0046's rule) under
//!   `provider:<name>`. The registry search reads the keys off the UI thread ([`read_keys`]); a launch adds its key
//!   to the agent's environment (`ELUDITE_OPENAI_API_KEY`) and nowhere else. No key is logged, audited (the bus
//!   redacts `apiKey`) or answered.
//! - **Models.** `provider_models` runs `eludite-openai-acp models` (the adapter's own listing code), off the UI
//!   thread, with the stored or the given key.
//! - **The dialog** ([`ProviderDialog`]): name, preset ([`PRESETS`]), base URL, the key (masked), Test (runs
//!   `provider_models`, shows "N models" or the message) and Save (runs `provider_set`). The name and the base URL
//!   are brief 0056's `TextInput` in single-line mode; the key box shows bullets and keeps the typed key in the dialog
//!   only (`TextInput` has no masked mode). Every action is a command on the bus, run off the UI thread by the shell.
//! - **Options** ([`ProvidersPage`]): the servers on the Agents page, each with Edit (the dialog) and Remove.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eludite_acp::settings::{
    AgentSettings, ProviderModel, ProviderSettings, provider_credential_key,
};
use eludite_acp::{AdapterSearch, OPENAI_ADAPTER_NOT_FOUND};
use eludite_commands::CommandError;
use eludite_commands::agents::{
    KeyChange, ProviderCommandOutput, ProviderModelsOutput, ProviderOutput, ProviderRequest,
    ProviderRow, ProviderSet, ProviderTarget,
};
use eludite_editor::{EditorStyle, TextInput, input_actions};
use eludite_forge::Family;
use eludite_forge::credentials::{Credential, Credentials, Secret, SignInMethod};
use eludite_ui::transcript::ToolStatus;
use eludite_ui::{Theme, check_box, dialog_panel, push_button, selector_option, text_box};
use futures::channel::mpsc::UnboundedSender;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use super::HostMsg;

/// One preset of the dialog: the base URL it fills in and whether the server wants a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    /// Empty for Custom.
    pub base_url: &'static str,
    pub key: KeyHint,
}

/// What the key box says for a preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHint {
    /// A local server that takes no key.
    None,
    /// A key if the server was started with one.
    Optional,
    Required,
}

impl KeyHint {
    pub fn placeholder(self) -> &'static str {
        match self {
            KeyHint::None => "No key needed",
            KeyHint::Optional => "Only if the server was started with one",
            KeyHint::Required => "Required",
        }
    }
}

/// The dialog's presets (brief 0059), in order; no xAI preset by the owner's decision (Custom takes its URL).
pub const PRESETS: [Preset; 11] = [
    Preset {
        name: "llama.cpp",
        base_url: "http://localhost:8080/v1",
        key: KeyHint::None,
    },
    Preset {
        name: "Ollama",
        base_url: "http://localhost:11434/v1",
        key: KeyHint::None,
    },
    Preset {
        name: "LM Studio",
        base_url: "http://localhost:1234/v1",
        key: KeyHint::None,
    },
    Preset {
        name: "vLLM",
        base_url: "http://localhost:8000/v1",
        key: KeyHint::Optional,
    },
    Preset {
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "Together",
        base_url: "https://api.together.xyz/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        key: KeyHint::Required,
    },
    Preset {
        name: "Custom",
        base_url: "",
        key: KeyHint::Optional,
    },
];

/// The preset whose base URL is `url` (one trailing slash ignored), else Custom.
pub fn preset_for(url: &str) -> usize {
    let url = url.strip_suffix('/').unwrap_or(url);
    PRESETS
        .iter()
        .position(|p| !p.base_url.is_empty() && p.base_url == url)
        .unwrap_or(PRESETS.len() - 1)
}

/// The keys of the saved servers by name, read off the UI thread. Its `Debug` shows only the names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProviderKeys(HashMap<String, String>);

impl ProviderKeys {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

impl std::fmt::Debug for ProviderKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// The stored key of each provider (blocks on the credential store: off the UI thread).
pub fn read_keys(credentials: &Credentials, providers: &[ProviderSettings]) -> ProviderKeys {
    ProviderKeys(
        providers
            .iter()
            .filter_map(|p| {
                let (c, _) = credentials.get(&p.credential_key())?;
                Some((p.name.clone(), c.token.expose().to_owned()))
            })
            .filter(|(_, k)| !k.is_empty())
            .collect(),
    )
}

/// `agents.json` at `file` (none: no file, the empty settings) and its error.
pub fn load_settings(file: Option<&Path>) -> (AgentSettings, Option<String>) {
    match file.map(AgentSettings::load) {
        Some(Ok(s)) => (s, None),
        Some(Err(e)) => (AgentSettings::default(), Some(e)),
        None => (AgentSettings::default(), None),
    }
}

/// The rows the Options page and `agents-provider.output.json` list.
pub fn rows(settings: &AgentSettings, keys: &ProviderKeys) -> Vec<ProviderRow> {
    settings
        .providers
        .iter()
        .map(|p| ProviderRow {
            name: p.name.clone(),
            base_url: p.base_url.clone(),
            default_model: p.default_model.clone(),
            tools: p.tools.clone(),
            has_key: keys.has(&p.name),
        })
        .collect()
}

/// A provider's key as a credential: no forge family, a token.
fn key_credential(key: &str) -> Credential {
    Credential {
        family: Family::None,
        token: Secret::new(key),
        refresh: None,
        method: SignInMethod::Token,
        account: None,
        did: None,
        pds: None,
        basic: false,
    }
}

/// The shell's [`ProviderTarget`]: `agents.json` and the credential store, on the invoking thread (the window runs
/// these commands off the UI thread: the store may wait on the system's keyring, `provider_models` on the network
/// for up to 30 s). A change asks the UI to search the registry again ([`HostMsg::Providers`]).
pub struct ProviderStore {
    /// `agents.json`; `None`: there is no config directory.
    pub file: Option<PathBuf>,
    pub credentials: Arc<Credentials>,
    /// Where `eludite-openai-acp` is looked for.
    pub search: AdapterSearch,
    pub notify: UnboundedSender<HostMsg>,
    /// One change to the file at a time.
    pub lock: Mutex<()>,
}

fn failed(e: impl std::fmt::Display) -> CommandError {
    CommandError::Failed(e.to_string())
}

impl ProviderStore {
    fn file(&self) -> Result<&Path, CommandError> {
        self.file
            .as_deref()
            .ok_or_else(|| failed("Eludite has no config directory to keep agents.json in"))
    }

    /// The settings, refusing to overwrite a file that does not parse.
    fn settings(&self) -> Result<AgentSettings, CommandError> {
        let (s, e) = load_settings(Some(self.file()?));
        match e {
            Some(e) => Err(failed(format!(
                "{e}; fix the file before changing its servers"
            ))),
            None => Ok(s),
        }
    }

    fn output(&self, name: &str, action: &str, settings: &AgentSettings) -> ProviderOutput {
        let keys = read_keys(&self.credentials, &settings.providers);
        ProviderOutput {
            name: name.to_owned(),
            action: action.into(),
            has_key: keys.has(name),
            providers: rows(settings, &keys),
        }
    }

    fn set(&self, s: ProviderSet) -> Result<ProviderCommandOutput, CommandError> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = self.settings()?;
        let key = provider_credential_key(&s.name);
        match &s.api_key {
            KeyChange::Keep => {}
            KeyChange::Delete => self
                .credentials
                .delete(&key)
                .map_err(|e| failed(e.message))?,
            KeyChange::Set(k) => {
                self.credentials
                    .set(&key, &key_credential(k.expose()), s.allow_file_store)
                    .map_err(|e| failed(e.message))?;
            }
        }
        settings.set_provider(ProviderSettings {
            name: s.name.clone(),
            base_url: s.base_url,
            default_model: s.default_model,
            headers: s.headers,
            models: s
                .models
                .into_iter()
                .map(|m| ProviderModel {
                    id: m.id,
                    name: m.name,
                    context_window: m.context_window,
                })
                .collect(),
            tools: s.tools,
        });
        settings.save(self.file()?).map_err(failed)?;
        let _ = self.notify.unbounded_send(HostMsg::Providers);
        Ok(ProviderCommandOutput::Provider(
            self.output(&s.name, "saved", &settings),
        ))
    }

    fn remove(&self, name: String) -> Result<ProviderCommandOutput, CommandError> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = self.settings()?;
        settings
            .remove_provider(&name)
            .ok_or_else(|| failed(format!("no server named `{name}`")))?;
        settings.save(self.file()?).map_err(failed)?;
        self.credentials
            .delete(&provider_credential_key(&name))
            .map_err(|e| failed(e.message))?;
        let _ = self.notify.unbounded_send(HostMsg::Providers);
        Ok(ProviderCommandOutput::Provider(
            self.output(&name, "removed", &settings),
        ))
    }

    fn models(
        &self,
        name: Option<String>,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<ProviderCommandOutput, CommandError> {
        let mut provider = match &name {
            Some(n) => {
                let (settings, _) = load_settings(self.file.as_deref());
                settings
                    .providers
                    .into_iter()
                    .find(|p| p.name == *n)
                    .ok_or_else(|| failed(format!("no server named `{n}`")))?
            }
            None => ProviderSettings::default(),
        };
        if let Some(u) = base_url {
            provider.base_url = u;
        }
        let key = match (api_key, &name) {
            (Some(k), _) => Some(k),
            (None, Some(n)) => self
                .credentials
                .get(&provider_credential_key(n))
                .map(|(c, _)| c.token.expose().to_owned()),
            (None, None) => None,
        };
        let adapter = eludite_acp::find_native_openai_adapter(&self.search)
            .ok_or_else(|| failed(OPENAI_ADAPTER_NOT_FOUND))?;
        let launch = eludite_acp::with_provider_key(
            eludite_acp::provider_models_launch(&adapter, &provider),
            key.as_deref(),
        );
        let out = eludite_acp::run_provider_models(&launch).map_err(failed)?;
        let out: ProviderModelsOutput = serde_json::from_value(out)
            .map_err(|e| failed(format!("the adapter's model list: {e}")))?;
        Ok(ProviderCommandOutput::Models(out))
    }
}

impl ProviderTarget for ProviderStore {
    fn apply(&self, request: ProviderRequest) -> Result<ProviderCommandOutput, CommandError> {
        match request {
            ProviderRequest::Set(s) => self.set(s),
            ProviderRequest::Remove { name } => self.remove(name),
            ProviderRequest::Models {
                name,
                base_url,
                api_key,
            } => self.models(name, base_url, api_key.map(|k| k.expose().to_owned())),
        }
    }
}

/// What a `provider_models` answer says in the dialog: "12 models", "1 model", or the message.
pub fn test_summary(out: &Value) -> (String, bool) {
    let n = out["models"].as_array().map_or(0, Vec::len);
    match (out["listing"].as_str(), out["message"].as_str()) {
        (Some("server" | "catalog"), _) if n > 0 => {
            (format!("{n} model{}", if n == 1 { "" } else { "s" }), false)
        }
        (_, Some(m)) => (m.to_owned(), true),
        _ => ("The server listed no models".into(), true),
    }
}

// ---- The dialog ----

/// Debug selectors.
pub const DIALOG: &str = "agents-provider-dialog";
pub const NAME_BOX: &str = "agents-provider-name";
pub const URL_BOX: &str = "agents-provider-url";
pub const KEY_BOX: &str = "agents-provider-key";
pub const FILE_CONSENT: &str = "agents-provider-file";
pub const TEST: &str = "agents-provider-test";
pub const SAVE: &str = "agents-provider-save";
pub const CANCEL: &str = "agents-provider-cancel";
pub const MESSAGE: &str = "agents-provider-message";

pub fn preset_selector(ix: usize) -> String {
    format!("agents-provider-preset-{ix}")
}

const WIDTH: f32 = 560.;

/// What the dialog asks the shell to run (every one a command on the bus, off the UI thread).
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderDialogEvent {
    /// `eludite.agents.provider_models` with these arguments.
    Test(Value),
    /// `eludite.agents.provider_set` with these arguments.
    Save(Value),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Url,
    Key,
}

pub struct ProviderDialog {
    theme: Theme,
    /// The server being edited (its saved name), `None` when adding one.
    pub editing: Option<String>,
    /// Whether the edited server has a stored key (an empty key box keeps it).
    pub has_key: bool,
    pub preset: usize,
    pub name: Entity<TextInput>,
    pub url: Entity<TextInput>,
    key: String,
    field: Field,
    pub allow_file: bool,
    pub offer_file: bool,
    /// A Test or Save is running.
    pub busy: bool,
    /// The last answer: (text, failed).
    pub message: Option<(String, bool)>,
    /// Where its controls were drawn, while `--bounds-out` probes.
    pub probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
}

impl EventEmitter<ProviderDialogEvent> for ProviderDialog {}

impl Focusable for ProviderDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn single_line(
    theme: Theme,
    placeholder: &str,
    cx: &mut Context<ProviderDialog>,
) -> Entity<TextInput> {
    let placeholder = placeholder.to_owned();
    cx.new(|cx| {
        let mut input = TextInput::new(false, cx);
        input.set_placeholder(placeholder, cx);
        input.set_style(
            EditorStyle {
                theme,
                ..EditorStyle::default()
            },
            cx,
        );
        input
    })
}

impl ProviderDialog {
    /// The dialog for a new server (llama.cpp's preset filled in), or for `editing` (its saved values; the key box
    /// empty, which keeps the stored key).
    pub fn new(theme: Theme, editing: Option<ProviderRow>, cx: &mut Context<Self>) -> Self {
        let name = single_line(theme, "A name for the server", cx);
        let url = single_line(theme, "http://localhost:8080/v1", cx);
        let mut this = Self {
            theme,
            editing: None,
            has_key: false,
            preset: 0,
            name,
            url,
            key: String::new(),
            field: Field::Name,
            allow_file: false,
            offer_file: false,
            busy: false,
            message: None,
            probe: None,
            focus: cx.focus_handle(),
        };
        match editing {
            Some(row) => {
                this.preset = preset_for(&row.base_url);
                this.has_key = row.has_key;
                this.name.update(cx, |i, cx| i.set_text(&row.name, cx));
                this.url.update(cx, |i, cx| i.set_text(&row.base_url, cx));
                this.editing = Some(row.name);
            }
            None => this.set_preset(0, cx),
        }
        this
    }

    /// Pick preset `ix`: its base URL, and its name when the name box is empty or still another preset's name.
    pub fn set_preset(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(p) = PRESETS.get(ix) else { return };
        self.preset = ix;
        let name = self.name.read(cx).text();
        if name.is_empty() || PRESETS.iter().any(|q| q.name == name) {
            let n = if p.base_url.is_empty() { "" } else { p.name };
            self.name.update(cx, |i, cx| i.set_text(n, cx));
        }
        if !p.base_url.is_empty() || self.url.read(cx).text().is_empty() {
            self.url.update(cx, |i, cx| i.set_text(p.base_url, cx));
        }
        self.message = None;
        cx.notify();
    }

    fn fields(&self, cx: &App) -> Result<(String, String), String> {
        let name = self.name.read(cx).text().trim().to_owned();
        let url = self.url.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return Err("Enter a name for the server.".into());
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("Enter the base URL, such as http://localhost:8080/v1.".into());
        }
        Ok((name, url))
    }

    /// `provider_models`' arguments: the typed URL and key; the saved name too when editing with the key box empty
    /// (its stored key is used).
    pub fn test_args(&self, cx: &App) -> Result<Value, String> {
        let (_, url) = self.fields(cx)?;
        let mut args = json!({"baseUrl": url});
        if !self.key.is_empty() {
            args["apiKey"] = json!(self.key);
        } else if let Some(saved) = &self.editing {
            args["name"] = json!(saved);
        }
        Ok(args)
    }

    /// `provider_set`'s arguments; an empty key box keeps the stored key.
    pub fn save_args(&self, cx: &App) -> Result<Value, String> {
        let (name, url) = self.fields(cx)?;
        // The key is kept under the name: a renamed server needs it typed again (the dialog never reads it back).
        if self.has_key
            && self.key.is_empty()
            && self.editing.as_ref().is_some_and(|old| *old != name)
        {
            return Err("Enter the key again to rename the server.".into());
        }
        let mut args = json!({"name": name, "baseUrl": url});
        if !self.key.is_empty() {
            args["apiKey"] = json!(self.key);
        }
        if self.allow_file {
            args["allowFileStore"] = json!(true);
        }
        Ok(args)
    }

    pub fn test(&mut self, cx: &mut Context<Self>) {
        match self.test_args(cx) {
            Ok(args) => {
                self.busy = true;
                self.message = Some(("Listing the server's models\u{2026}".into(), false));
                cx.emit(ProviderDialogEvent::Test(args));
            }
            Err(e) => self.message = Some((e, true)),
        }
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        match self.save_args(cx) {
            Ok(args) => {
                self.busy = true;
                self.message = Some(("Saving\u{2026}".into(), false));
                cx.emit(ProviderDialogEvent::Save(args));
            }
            Err(e) => self.message = Some((e, true)),
        }
        cx.notify();
    }

    /// The answer of a Test.
    pub fn tested(&mut self, result: Result<Value, String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.message = Some(match result {
            Ok(out) => test_summary(&out),
            Err(e) => (e, true),
        });
        cx.notify();
    }

    /// A Save that failed (one that succeeded closes the dialog): the credential store's refusal offers the file.
    pub fn save_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.busy = false;
        if error.contains("store_unavailable") {
            self.offer_file = true;
        }
        self.message = Some((error, true));
        cx.notify();
    }

    /// The key box's keys (the name and URL boxes are `TextInput`s; their Enter, Escape and Tab arrive as actions).
    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.field != Field::Key {
            return;
        }
        let k = &e.keystroke;
        if (k.modifiers.control || k.modifiers.platform) && k.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.key.push_str(text.trim());
            }
        } else {
            use super::super::forge::widgets::{Edit, edit};
            match edit(&mut self.key, e, false) {
                Edit::Changed => {}
                Edit::Enter => self.save(cx),
                Edit::Escape => cx.emit(ProviderDialogEvent::Cancel),
                Edit::Tab => {
                    let next = if k.modifiers.shift {
                        Field::Url
                    } else {
                        Field::Name
                    };
                    self.focus_field(next, window, cx);
                }
                Edit::Ignored => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Tab in the name or URL box: the next box (Shift: the previous one).
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let next = match (self.field, back) {
            (Field::Name, false) | (Field::Key, true) => Field::Url,
            (Field::Url, false) | (Field::Name, true) => Field::Key,
            (Field::Url, true) | (Field::Key, false) => Field::Name,
        };
        self.focus_field(next, window, cx);
    }

    fn focus_field(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        self.field = field;
        match field {
            Field::Name => window.focus(&self.name.focus_handle(cx), cx),
            Field::Url => window.focus(&self.url.focus_handle(cx), cx),
            Field::Key => window.focus(&self.focus, cx),
        }
        cx.notify();
    }

    /// Focus the name box (when the dialog opens).
    pub fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_field(Field::Name, window, cx);
    }

    fn probed(&self, id: &str) -> Option<gpui::AnyElement> {
        eludite_ui::bounds_canvas(self.probe.as_ref(), id.to_owned())
            .map(IntoElement::into_any_element)
    }
}

impl Render for ProviderDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let preset = PRESETS[self.preset.min(PRESETS.len() - 1)];
        let title = match &self.editing {
            Some(n) => format!("Edit server: {n}"),
            None => "Add an OpenAI-compatible server".to_owned(),
        };
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().w(px(90.)).flex_none().child(label))
        };
        let input_box = |id: &'static str, input: &Entity<TextInput>, focused: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .relative()
                .children(self.probed(id))
                .w(px(400.))
                .px_1()
                .py(px(2.))
                .border_1()
                .border_color(if focused { t.accent } else { t.border })
                .bg(t.background)
                .text_color(t.text)
                .child(input.clone())
        };
        let name_focused = self.name.focus_handle(cx).is_focused(window);
        let url_focused = self.url.focus_handle(cx).is_focused(window);
        let key_focused = self.focus.is_focused(window) && self.field == Field::Key;
        let mut presets = div().flex().flex_row().flex_wrap().gap_1().px_3().pt_3();
        for (ix, p) in PRESETS.iter().enumerate() {
            let sel = preset_selector(ix);
            presets = presets.child(
                selector_option(sel.clone(), p.name, ix == self.preset, &t)
                    .relative()
                    .children(self.probed(&sel))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_preset(ix, cx))),
            );
        }
        let key_hint = if self.editing.is_some() && self.has_key {
            "A key is stored; leave empty to keep it"
        } else {
            preset.key.placeholder()
        };
        let bullets = "\u{2022}".repeat(self.key.chars().count());
        let mut panel = dialog_panel(&t, title)
            .id(DIALOG)
            .debug_selector(|| DIALOG.into())
            .track_focus(&self.focus)
            .key_context("AgentsProviderDialog")
            .capture_key_down(cx.listener(Self::key_down))
            // The name and URL boxes' Enter, Escape and Tab.
            .capture_action(cx.listener(|this, _: &input_actions::Submit, _, cx| {
                this.save(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|_, _: &input_actions::Escape, _, cx| {
                cx.emit(ProviderDialogEvent::Cancel);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Tab, window, cx| {
                this.tab(false, window, cx);
                cx.stop_propagation();
            }))
            .occlude()
            .w(px(WIDTH))
            .child(presets)
            .child(
                row("Name:").child(input_box(NAME_BOX, &self.name, name_focused).on_click(
                    cx.listener(|this, _, window, cx| this.focus_field(Field::Name, window, cx)),
                )),
            )
            .child(
                row("Base URL:").child(input_box(URL_BOX, &self.url, url_focused).on_click(
                    cx.listener(|this, _, window, cx| this.focus_field(Field::Url, window, cx)),
                )),
            )
            .child(
                row("API key:").child(
                    text_box(KEY_BOX, &bullets, key_hint, key_focused, &t)
                        .relative()
                        .children(self.probed(KEY_BOX))
                        .w(px(400.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.focus_field(Field::Key, window, cx)
                        })),
                ),
            );
        if self.offer_file {
            panel = panel.child(
                div().px_3().py_1().child(
                    check_box(
                        FILE_CONSENT,
                        "The credential store is unavailable: keep the key in a file only I can read",
                        self.allow_file,
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.allow_file = !this.allow_file;
                        cx.notify();
                    })),
                ),
            );
        }
        let busy = self.busy;
        panel = panel
            .child(
                div()
                    .px_3()
                    .pt_1()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(
                        "The key is kept in the operating system's credential store, never in agents.json or a log. \
                         The server appears in the Agents window's agent list.",
                    ),
            )
            .children(self.message.clone().map(|(m, failed)| {
                div()
                    .id(MESSAGE)
                    .debug_selector(|| MESSAGE.into())
                    .px_3()
                    .py_1()
                    .text_color(if failed { ToolStatus::Failed.color(&t) } else { t.text })
                    .child(m)
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_3()
                    .child(
                        push_button(TEST, "Test", false, !busy, &t)
                            .relative()
                            .children(self.probed(TEST))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !busy {
                                    this.test(cx)
                                }
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        push_button(SAVE, "Save", true, !busy, &t)
                            .relative()
                            .children(self.probed(SAVE))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !busy {
                                    this.save(cx)
                                }
                            })),
                    )
                    .child(
                        push_button(CANCEL, "Cancel", false, true, &t)
                            .relative()
                            .children(self.probed(CANCEL))
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ProviderDialogEvent::Cancel)),
                            ),
                    ),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(WIDTH)) / 2.).max(px(0.)),
            (viewport.height / 6.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(6)
    }
}

// ---- Tools > Options > Agents ----

pub fn edit_selector(ix: usize) -> String {
    format!("options-provider-edit-{ix}")
}

pub fn remove_selector(ix: usize) -> String {
    format!("options-provider-remove-{ix}")
}

pub const ADD_SERVER: &str = "options-provider-add";

#[derive(Debug, Clone, PartialEq)]
pub enum ProvidersPageEvent {
    /// Open the dialog for server `name`, or for a new one.
    Edit(Option<String>),
    /// `eludite.agents.provider_remove`.
    Remove(String),
}

/// The servers on Tools > Options > Agents, each with Edit and Remove.
pub struct ProvidersPage {
    theme: Theme,
    pub rows: Vec<ProviderRow>,
    /// The last Remove's failure.
    pub message: Option<String>,
}

impl EventEmitter<ProvidersPageEvent> for ProvidersPage {}

impl ProvidersPage {
    pub fn new(theme: Theme, rows: Vec<ProviderRow>) -> Self {
        Self {
            theme,
            rows,
            message: None,
        }
    }

    pub fn set_rows(&mut self, rows: Vec<ProviderRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }
}

impl Render for ProvidersPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().flex().flex_col().gap_1();
        if self.rows.is_empty() {
            list = list.child(div().text_color(t.text_muted).child(
                "No servers yet. Add a llama.cpp, Ollama or other OpenAI-compatible server.",
            ));
        }
        for (ix, r) in self.rows.iter().enumerate() {
            let (edit, remove) = (r.name.clone(), r.name.clone());
            list =
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().overflow_hidden().child(
                            SharedString::from(format!(
                                "{}  {}{}",
                                r.name,
                                r.base_url,
                                if r.has_key { "  (key stored)" } else { "" }
                            )),
                        ))
                        .child(
                            push_button(edit_selector(ix), "Edit", false, true, &t).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(ProvidersPageEvent::Edit(Some(edit.clone())))
                                }),
                            ),
                        )
                        .child(
                            push_button(remove_selector(ix), "Remove", false, true, &t).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(ProvidersPageEvent::Remove(remove.clone()))
                                }),
                            ),
                        ),
                );
        }
        div()
            .id("options-providers")
            .debug_selector(|| "options-providers".into())
            .flex()
            .flex_col()
            .gap_1()
            .py_1()
            .w_full()
            .child("OpenAI-compatible servers")
            .child(list)
            .child(
                div().flex().child(
                    push_button(ADD_SERVER, "Add server\u{2026}", false, true, &t).on_click(
                        cx.listener(|_, _, _, cx| cx.emit(ProvidersPageEvent::Edit(None))),
                    ),
                ),
            )
            .when_some(self.message.clone(), |d, m| {
                d.child(div().text_color(ToolStatus::Failed.color(&t)).child(m))
            })
            .child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(
                        "Each server is an agent in the Agents window, run through eludite-openai-acp with Eludite's \
                         tools. Kept in agents.json's providers; keys in the credential store.",
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_the_briefs_without_xai() {
        let names: Vec<_> = PRESETS.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "llama.cpp",
                "Ollama",
                "LM Studio",
                "vLLM",
                "OpenAI",
                "OpenRouter",
                "Groq",
                "Together",
                "DeepSeek",
                "Mistral",
                "Custom"
            ]
        );
        assert!(
            !PRESETS
                .iter()
                .any(|p| p.name.contains("xAI") || p.base_url.contains("x.ai"))
        );
        for p in &PRESETS[..3] {
            assert_eq!(p.key, KeyHint::None, "{}", p.name);
        }
        assert_eq!(PRESETS[0].base_url, "http://localhost:8080/v1");
        assert_eq!(PRESETS[1].base_url, "http://localhost:11434/v1");
        assert_eq!(PRESETS[2].base_url, "http://localhost:1234/v1");
        assert_eq!(PRESETS[3].base_url, "http://localhost:8000/v1");
        assert_eq!(PRESETS[6].base_url, "https://api.groq.com/openai/v1");
        assert_eq!(preset_for("https://openrouter.ai/api/v1/"), 5);
        assert_eq!(preset_for("https://api.x.ai/v1"), PRESETS.len() - 1);
    }

    #[test]
    fn a_test_answer_reads_as_a_count_or_the_message() {
        let models = json!({"models": [{"id": "a"}, {"id": "b"}], "listing": "server"});
        assert_eq!(test_summary(&models), ("2 models".into(), false));
        let one = json!({"models": [{"id": "a"}], "listing": "catalog"});
        assert_eq!(test_summary(&one), ("1 model".into(), false));
        let none =
            json!({"models": [], "listing": "none", "message": "the key was rejected (401)"});
        assert_eq!(
            test_summary(&none),
            ("the key was rejected (401)".into(), true)
        );
        assert!(test_summary(&json!({"models": [], "listing": "server"})).1);
    }

    #[test]
    fn keys_never_show_in_debug() {
        let keys = ProviderKeys(HashMap::from([("a".to_owned(), "sk-secret".to_owned())]));
        assert!(!format!("{keys:?}").contains("sk-secret"));
        assert_eq!(keys.get("a"), Some("sk-secret"));
    }
}
