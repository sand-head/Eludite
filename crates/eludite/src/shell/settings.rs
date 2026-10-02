//! The shell's side of settings (brief 0020): the `eludite.settings.*` target over the store
//! ([`crate::settings::Settings`]), and applying the effective settings to the parts of the shell they configure,
//! at start and whenever a file changes or a setting is set, without a restart:
//!
//! | Setting | Applies to |
//! |---|---|
//! | `build.beforeRun` | F5 and Ctrl+F5 (`debug`) |
//! | `build.onSave`, `build.showOutputOnStart`, `build.showErrorListOnFailure`, `build.cargoPath` | builds (`build`) |
//! | `debugger.netcoredbgPath` | the next debugging session's adapter search |
//! | `languageServers.rustAnalyzerPath` | the next rust-analyzer started |
//! | `agents.default`, `agents.claudeCodeAdapterPath`, `agents.custom` | the Agents window's registry, searched again |
//! | `keyboard.preset` | the key bindings (Visual Studio's is the only preset) |
//!
//! The environment variables that used to be the only switches (`ELUDITE_BUILD_ON_SAVE`, `ELUDITE_CARGO`,
//! `ELUDITE_NETCOREDBG`, `ELUDITE_RUST_ANALYZER`, `ELUDITE_CLAUDE_ACP`) still override the files: the store resolves
//! them, so nothing here reads the environment.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::settings::{
    OptionsOutput, SettingsGetOutput, SettingsOutput, SettingsRequest, SettingsTarget,
};
use futures::channel::mpsc::UnboundedSender;
use gpui::{AppContext as _, Context, Focusable as _, PathPromptOptions, Window};
use serde_json::{Value, json};

use super::Shell;
use super::options::{OptionsDialog, OptionsEvent};
use crate::settings::Settings;

type OptionsOutcome = Result<OptionsOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<OptionsOutcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the `eludite.tools.options` invocation it is about to make on the UI thread.
pub fn stage(outcome: OptionsOutcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// `eludite.tools.options` from another thread, for the UI thread to apply.
pub struct OptionsJob {
    pub section: Option<String>,
    pub reply: mpsc::SyncSender<OptionsOutcome>,
}

/// `eludite.settings.get` and `set` on any thread, straight on the store (the shell applies the change when the
/// store signals it); `eludite.tools.options` on the UI thread.
pub struct SettingsBus {
    pub settings: Settings,
    pub ui_thread: std::thread::ThreadId,
    pub jobs: UnboundedSender<OptionsJob>,
}

impl SettingsTarget for SettingsBus {
    fn apply(&self, request: SettingsRequest) -> Result<SettingsOutput, CommandError> {
        match request {
            SettingsRequest::Get { key } => Ok(SettingsOutput::Get(Box::new(
                self.settings.lock().get_output(key.as_deref()),
            ))),
            SettingsRequest::Set { key, value, scope } => self
                .settings
                .set(&key, value, scope)
                .map(SettingsOutput::Set),
            SettingsRequest::Options { section } => {
                if std::thread::current().id() == self.ui_thread {
                    return STAGED
                        .with(|s| s.borrow_mut().take())
                        .unwrap_or_else(|| {
                            Err(CommandError::Failed(
                                "eludite.tools.options runs on the UI thread through the shell"
                                    .into(),
                            ))
                        })
                        .map(SettingsOutput::Options);
                }
                let (reply, rx) = mpsc::sync_channel(1);
                self.jobs
                    .unbounded_send(OptionsJob { section, reply })
                    .map_err(|_| CommandError::Failed("the window is closed".into()))?;
                rx.recv_timeout(Duration::from_secs(30))
                    .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
                    .map(SettingsOutput::Options)
            }
        }
    }
}

/// What the settings configure, read from the store in one lock.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Applied {
    pub build_before_run: bool,
    pub build_on_save: bool,
    pub show_output_on_start: bool,
    pub show_error_list_on_failure: bool,
    pub cargo: Option<PathBuf>,
    pub netcoredbg: Option<PathBuf>,
    pub rust_analyzer: Option<PathBuf>,
    pub agents: super::agents::RegistryConfig,
    pub keyboard_preset: String,
}

impl Shell {
    /// How long each change took from being seen (a set, or the poll that found the file changed) to being applied.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings_timings(&self) -> &[Duration] {
        &self.settings_applied
    }

    /// Apply the effective settings. `seen` is when the change was noticed.
    pub(super) fn apply_settings(&mut self, seen: Option<Instant>, cx: &mut Context<Self>) {
        let applied = {
            let s = self.settings.lock();
            Applied {
                build_before_run: s.bool("build.beforeRun"),
                build_on_save: s.bool("build.onSave"),
                show_output_on_start: s.bool("build.showOutputOnStart"),
                show_error_list_on_failure: s.bool("build.showErrorListOnFailure"),
                cargo: s.path("build.cargoPath"),
                netcoredbg: s.path("debugger.netcoredbgPath"),
                rust_analyzer: s.path("languageServers.rustAnalyzerPath"),
                agents: super::agents::RegistryConfig::from_store(&s),
                keyboard_preset: s.string("keyboard.preset"),
            }
        };
        let b = &mut self.builds;
        b.build_on_save = applied.build_on_save;
        b.build_before_run = applied.build_before_run;
        b.show_output_on_start = applied.show_output_on_start;
        b.show_error_list_on_failure = applied.show_error_list_on_failure;
        b.cargo_program = applied
            .cargo
            .clone()
            .map_or_else(|| "cargo".into(), PathBuf::into_os_string);
        self.debug.set_adapter_path(applied.netcoredbg.clone());
        self.launches.rust_analyzer = applied.rust_analyzer.clone();
        let agents_changed = self
            .applied_settings
            .as_ref()
            .is_none_or(|a| a.agents != applied.agents);
        if agents_changed {
            self.agents.set_registry_config(applied.agents.clone(), cx);
        }
        self.refresh_options(cx);
        if let Some(seen) = seen {
            let took = seen.elapsed();
            super::documents::trace(format_args!(
                "settings applied {:.2} ms after the change was seen",
                took.as_secs_f64() * 1e3
            ));
            if self.settings_applied.len() < 4096 {
                self.settings_applied.push(took);
            }
        }
        self.applied_settings = Some(applied);
        cx.notify();
    }

    /// The open solution or folder changed: its `.eludite/settings.json` becomes the second layer (read by the
    /// settings thread, which signals the change).
    pub(super) fn update_settings_dir(&mut self) {
        let dir = self.workspace_root();
        self.settings.set_solution_dir(dir.as_deref());
    }

    /// What the last [`Shell::apply_settings`] applied.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn applied_settings(&self) -> Option<&Applied> {
        self.applied_settings.as_ref()
    }
}

impl Shell {
    /// `eludite.tools.options`: open the Options dialog (or show `section` in the open one).
    pub(super) fn open_options(
        &mut self,
        section: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> OptionsOutcome {
        let dialog = match &self.options {
            Some(d) => d.clone(),
            None => {
                let schema = self.settings.lock().schema().clone();
                let theme = self.theme;
                let probe = self.ui_bounds.clone();
                let d = cx.new(|cx| {
                    let mut d = OptionsDialog::new(theme, schema, section.as_deref(), cx);
                    d.set_probe(probe);
                    d
                });
                cx.subscribe_in(&d, window, Self::on_options_event).detach();
                self.options = Some(d.clone());
                d
            }
        };
        if let Some(s) = &section {
            let ix = self
                .settings
                .lock()
                .schema()
                .sections
                .iter()
                .position(|x| x == s);
            if let Some(ix) = ix {
                dialog.update(cx, |d, cx| d.show_section(ix, cx));
            }
        }
        self.refresh_options(cx);
        dialog.focus_handle(cx).focus(window, cx);
        cx.notify();
        Ok(OptionsOutput {
            section: dialog.read(cx).section().to_owned(),
        })
    }

    /// Give the open dialog the current values.
    fn refresh_options(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.options.clone() {
            let out: SettingsGetOutput = self.settings.lock().get_output(None);
            d.update(cx, |d, cx| d.set_values(out, cx));
        }
    }

    pub(super) fn close_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.options.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// The Options dialog's changes run `eludite.settings.set` through the bus, as an agent's would.
    fn on_options_event(
        &mut self,
        _: &gpui::Entity<OptionsDialog>,
        event: &OptionsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            OptionsEvent::Set { key, value } => {
                self.set_setting(key, value.clone(), window, cx);
            }
            OptionsEvent::Browse { key } => {
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Select".into()),
                });
                let key = key.clone();
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = paths.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        let _ = this.update_in(cx, |shell, window, cx| {
                            let value = Value::String(path.to_string_lossy().into_owned());
                            shell.set_setting(&key, value, window, cx)
                        });
                    }
                })
                .detach();
            }
            OptionsEvent::Close => self.close_options(window, cx),
        }
    }

    /// `eludite.settings.set` from the UI, in the user file (Visual Studio's Options are per user).
    fn set_setting(
        &mut self,
        key: &str,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            eludite_commands::settings::SET,
            json!({ "key": key, "value": value }),
            window,
            cx,
        );
        // The store signals the change; refresh now so the dialog never shows the old value for a frame.
        self.refresh_options(cx);
    }

    /// The Options dialog, while it is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn options_dialog(&self) -> Option<&gpui::Entity<OptionsDialog>> {
        self.options.as_ref()
    }
}
