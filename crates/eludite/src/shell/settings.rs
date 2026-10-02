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

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::settings::{SettingsOutput, SettingsRequest, SettingsTarget};
use gpui::Context;

use super::Shell;
use crate::settings::Settings;

/// `eludite.settings.get` and `set` on any thread, straight on the store; the shell applies the change when the
/// store signals it.
pub struct SettingsBus {
    pub settings: Settings,
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
            SettingsRequest::Options { .. } => Err(CommandError::Failed(
                "the Options dialog is not available here".into(),
            )),
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
