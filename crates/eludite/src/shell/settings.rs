//! The shell's side of settings (brief 0020): the `eludite.settings.*` target over the store
//! ([`crate::settings::Settings`]), and applying the effective settings to the parts of the shell they configure,
//! at start and whenever a file changes or a setting is set, without a restart:
//!
//! | Setting | Applies to |
//! |---|---|
//! | `build.beforeRun` | F5 and Ctrl+F5 (`debug`) |
//! | `build.onSave`, `build.showOutputOnStart`, `build.showErrorListOnFailure`, `build.cargoPath` | builds (`build`) |
//! | `debugger.netcoredbgPath` | the next debugging session's adapter search |
//! | `debugger.monoPrefix`, `debugger.monoAdapterPath` | the next session's Mono and `eludite-dbg-mono` searches |
//! | `debugger.lldbDapPath`, `debugger.rustFormatters` | the next native (Cargo) session's lldb-dap and Rust formatters |
//! | `debugger.allowAgentsByDefault` | whether each new debugging session lets agents drive it (brief 0027) |
//! | `languageServers.rustAnalyzerPath` | the next rust-analyzer started |
//! | `agents.default`, `agents.claudeCodeAdapterPath`, `agents.custom` | the Agents window's registry, searched again |
//! | `keyboard.preset` | the key bindings (Visual Studio's is the only preset) |
//! | `browser.chromePath`, `browser.headless`, `browser.viewport` | the browser's next launch (`browser`, brief 0023) |
//! | `browser.enginePath`, `browser.allowNoSandbox` | the embedded engine's search and whether its next launch may drop the sandbox (brief 0039); the opt-in is read from the person's state for the workspace only, and the Web Browser window and the Output window warn while the workspace's file carries it (brief 0047) |
//! | `browser.useBuiltIn`, `debugger.launchBrowser` | where (and whether) the next start opens a web project's page (brief 0037) |
//! | `debugger.attachBrowser`, `debugger.nodePath`, `debugger.jsDebugPath` | whether the next start debugs its page, and the next browser session's Node.js and vscode-js-debug (brief 0038) |
//! | `test.parallel`, `test.runSettings`, `test.vstestConsolePath` | the next test discovery and run (`test_runs`, brief 0035) |
//!
//! The environment variables that used to be the only switches (`ELUDITE_BUILD_ON_SAVE`, `ELUDITE_CARGO`,
//! `ELUDITE_NETCOREDBG`, `ELUDITE_RUST_ANALYZER`, `ELUDITE_CLAUDE_ACP`, `ELUDITE_CHROME`; brief 0022's `ELUDITE_MONO_PREFIX`
//! and `ELUDITE_DBG_MONO`; brief 0029's `ELUDITE_LLDB_DAP`) still override the files: the store resolves them, so nothing here reads the environment.

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
    pub mono_prefix: Option<PathBuf>,
    pub mono_adapter: Option<PathBuf>,
    pub lldb_dap: Option<PathBuf>,
    pub rust_formatters: bool,
    pub agents_drive: bool,
    pub rust_analyzer: Option<PathBuf>,
    pub agents: super::agents::RegistryConfig,
    pub keyboard_preset: String,
    pub browser: super::browser::BrowserSettings,
    /// `browser.useBuiltIn` and `debugger.launchBrowser` (brief 0037).
    pub use_built_in: bool,
    pub launch_browser: bool,
    /// `debugger.attachBrowser`, `debugger.nodePath`, `debugger.jsDebugPath` (brief 0038).
    pub attach_browser: bool,
    pub node: Option<PathBuf>,
    pub js_debug: Option<PathBuf>,
    /// Brief 0050: the registrations' own settings (`languageServers.eslint`, `languageServers.typescriptPath`) by
    /// key, `languageServers.nodePath`, `editor.formatter`, `editor.formatOnSave.<id>` by registration id and
    /// `editor.emmet`.
    pub server_settings: std::collections::BTreeMap<String, String>,
    pub language_node: Option<PathBuf>,
    pub formatter: String,
    pub format_on_save: std::collections::BTreeMap<String, bool>,
    pub emmet: bool,
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
                mono_prefix: s.path("debugger.monoPrefix"),
                mono_adapter: s.path("debugger.monoAdapterPath"),
                lldb_dap: s.path("debugger.lldbDapPath"),
                rust_formatters: s.bool("debugger.rustFormatters"),
                agents_drive: s.bool("debugger.allowAgentsByDefault"),
                rust_analyzer: s.path("languageServers.rustAnalyzerPath"),
                agents: super::agents::RegistryConfig::from_store(&s),
                keyboard_preset: s.string("keyboard.preset"),
                browser: super::browser::BrowserSettings {
                    chrome_path: s.path("browser.chromePath"),
                    headless: s.bool("browser.headless"),
                    viewport: eludite_browser::engine::parse_viewport(
                        &s.string("browser.viewport"),
                    )
                    .unwrap_or(eludite_browser::EngineConfig::VIEWPORT),
                    engine: eludite_browser::EngineChoice::from_setting(
                        &s.string("browser.engine"),
                    ),
                    home_page: Some(s.string("browser.homePage"))
                        .filter(|h| !h.trim().is_empty())
                        .unwrap_or_else(|| "about:blank".into()),
                    show_devtools_tab: s.bool("browser.showDevToolsTab"),
                    engine_path: s.path("browser.enginePath"),
                    allow_no_sandbox: s.bool("browser.allowNoSandbox"),
                    opt_in_ignored: s
                        .ignored_keys()
                        .iter()
                        .any(|k| k == "browser.allowNoSandbox"),
                },
                use_built_in: s.bool("browser.useBuiltIn"),
                launch_browser: s.bool("debugger.launchBrowser"),
                attach_browser: s.bool("debugger.attachBrowser"),
                node: s.path("debugger.nodePath"),
                js_debug: s.path("debugger.jsDebugPath"),
                // Whatever the registrations name (servers.json), so no language has a code path here.
                server_settings: self
                    .launches
                    .registry
                    .servers
                    .iter()
                    .flat_map(|r| {
                        r.activation
                            .iter()
                            .map(|a| a.setting.clone())
                            .chain(r.modules.iter().filter_map(|m| m.setting.clone()))
                    })
                    .filter(|k| s.schema().get(k).is_some())
                    .map(|k| {
                        let v = s.string(&k);
                        (k, v)
                    })
                    .collect(),
                language_node: s.path("languageServers.nodePath"),
                formatter: s.string("editor.formatter"),
                format_on_save: self
                    .launches
                    .registry
                    .servers
                    .iter()
                    .map(|r| format!("editor.formatOnSave.{}", r.id))
                    .filter(|k| s.schema().get(k).is_some())
                    .map(|k| {
                        let on = s.bool(&k);
                        (k["editor.formatOnSave.".len()..].to_owned(), on)
                    })
                    .collect(),
                emmet: s.bool("editor.emmet"),
            }
        };
        self.nuget_apply_settings();
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
        self.debug.set_mono_prefix(applied.mono_prefix.clone());
        self.debug
            .set_mono_adapter_path(applied.mono_adapter.clone());
        self.debug.set_lldb_dap_path(applied.lldb_dap.clone());
        self.debug.set_rust_formatters(applied.rust_formatters);
        self.debug.set_agents_default(applied.agents_drive);
        self.debug
            .set_launch_browser(applied.use_built_in, applied.launch_browser);
        self.debug.set_attach_browser(applied.attach_browser);
        self.debug.set_node_path(applied.node.clone());
        self.debug.set_js_debug_path(applied.js_debug.clone());
        self.launches.rust_analyzer = applied.rust_analyzer.clone();
        self.launches.settings = applied.server_settings.clone();
        self.launches.node = applied.language_node.clone();
        self.launches.formatter = applied.formatter.clone();
        self.launches.format_on_save = applied.format_on_save.clone();
        if self.launches.emmet != applied.emmet {
            self.launches.emmet = applied.emmet;
            for doc in self.documents.values() {
                doc.view.update(cx, |v, _| v.set_emmet(applied.emmet));
            }
        }
        self.browser.set_settings(applied.browser.clone());
        // Brief 0047: the workspace's file carries the per-person opt-in, which is ignored: say so once in the
        // Output window each time it appears, and in the Web Browser window while it stays.
        let ignored = applied.browser.opt_in_ignored;
        let was_ignored = self
            .applied_settings
            .as_ref()
            .is_some_and(|a| a.browser.opt_in_ignored);
        if ignored && !was_ignored {
            eprintln!("eludite: {}", super::browser_window::OPT_IN_IGNORED);
            self.output.update(cx, |o, cx| {
                o.append(
                    eludite_commands::build::OutputSource::Browser,
                    &format!("{}\n", super::browser_window::OPT_IN_IGNORED),
                    cx,
                )
            });
        }
        self.browser_window
            .update(cx, |w, cx| w.set_opt_in_ignored(ignored, cx));
        self.apply_test_settings();
        self.git_apply_settings(cx);
        self.terminal_apply_settings(cx);
        self.search_apply_settings();
        self.forge_apply_settings(cx);
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
        // The browser's profile is the workspace's: a running browser closes with the workspace (brief 0023).
        self.browser.set_workspace(dir.as_deref());
        // The workspace's Git repository, looked for off the UI thread (brief 0040).
        self.git_workspace_changed(dir.as_deref());
        // New terminals start in the workspace's folder (brief 0041).
        self.terminal_workspace_changed(dir.as_deref());
        // The forge cache is the workspace's; nothing is fetched (brief 0046).
        self.forge_workspace_changed(dir.as_deref());
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
                // Brief 0048: NuGet Package Manager > Package Sources, the NuGet module's page.
                let page = self.nuget_options_page(cx);
                d.update(cx, |d, cx| {
                    d.add_page(
                        eludite_commands::settings::PACKAGE_SOURCES_PAGE.into(),
                        page,
                        cx,
                    )
                });
                self.options = Some(d.clone());
                d
            }
        };
        if let Some(s) = &section {
            let ix = dialog.read(cx).position(s);
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

    /// `eludite.settings.set` from the UI, in the user file (Visual Studio's Options are per user), in the
    /// workspace's for a setting that belongs to the workspace (`x-eludite-scope`, brief 0039), or in the person's
    /// state for the workspace for a per-person one (`user-workspace`, brief 0047's `browser.allowNoSandbox`).
    fn set_setting(
        &mut self,
        key: &str,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self
            .settings
            .lock()
            .schema()
            .get(key)
            .map(|spec| spec.scope)
            .unwrap_or_default();
        self.run(
            eludite_commands::settings::SET,
            json!({ "key": key, "value": value, "scope": scope }),
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
