//! Process wiring: load the layout, build the command bus, open the window.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::{builtins, view, workspace};
use eludite_docking::{
    DockController, DockHost, DockLayout, LayoutSource, LayoutStore, LayoutWriter, Persistence,
    ToolWindowRegistry,
};
use eludite_ui::{EDITOR_COMMAND_KEYS, RunCommand, Theme, bind_keymap, vs_keymap};
use gpui::{
    App, AppContext as _, Bounds, Focusable as _, KeyBinding, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use serde_json::json;

use crate::args::Args;
use crate::bench;
use crate::shell::Shell;
use crate::shell::session::HostLaunch;

/// The editor's key bindings, except the keys whose actions are commands (`eludite.editor.undo`, `redo`, `find`,
/// and IntelliSense's `complete`, `signature_help`, `hover` and `accept_completion`): those keys dispatch
/// `RunCommand` in the editor's context, so they reach the command bus first.
pub fn bind_editor_keys(cx: &mut App) {
    let commands = vs_keymap();
    cx.bind_keys(eludite_editor::key_bindings().into_iter().filter(|b| {
        !matches!(
            b.action().name(),
            "editor::Undo"
                | "editor::Redo"
                | "editor::Find"
                | "editor::ShowCompletions"
                | "editor::ShowSignatureHelp"
                | "editor::ShowHover"
                | "editor::AcceptCompletion"
        )
    }));
    cx.bind_keys(EDITOR_COMMAND_KEYS.iter().filter_map(|keys| {
        let command = match *keys {
            "ctrl-shift-z" => "eludite.editor.redo",
            other => commands.iter().find(|k| k.keystrokes == other)?.command,
        };
        Some(KeyBinding::new(
            keys,
            RunCommand::new(command, json!({})),
            Some(eludite_editor::KEY_CONTEXT),
        ))
    }));
    // Committing a completion item: Tab, and Enter on a selected (not soft-selected) item, while the list shows.
    let accept = || RunCommand::new(workspace::EDITOR_ACCEPT_COMPLETION, json!({}));
    cx.bind_keys([
        KeyBinding::new("tab", accept(), Some(eludite_editor::COMPLETION_CONTEXT)),
        KeyBinding::new(
            "enter",
            accept(),
            Some(eludite_editor::COMPLETION_SELECTED_CONTEXT),
        ),
    ]);
}

/// What the loader thread hands back.
struct Loaded {
    layout: DockLayout,
    source: LayoutSource,
    store: Option<LayoutStore>,
}

fn load(args: &Args, tools: &ToolWindowRegistry) -> Loaded {
    let store = (!args.no_persist && !args.benching())
        .then(LayoutStore::user)
        .flatten();
    let (layout, source) = match (&store, args.reset_layout) {
        (Some(s), false) => s.load(args.solution.as_deref(), tools),
        _ => (DockLayout::default_vs(tools), LayoutSource::BuiltIn),
    };
    Loaded {
        layout,
        source,
        store,
    }
}

pub fn run(args: Args, t_main: Instant) {
    // Read the saved layout on a worker thread while GPUI starts up; the main
    // thread only joins it before opening the window.
    let loader = {
        let args = args.clone();
        std::thread::spawn(move || load(&args, &ToolWindowRegistry::vs_default()))
    };
    let theme = args
        .theme
        .as_deref()
        .and_then(Theme::by_name)
        .unwrap_or_default();

    gpui_platform::application().run(move |cx: &mut App| {
        let t_join = Instant::now();
        let Loaded {
            layout,
            source,
            store,
        } = loader.join().expect("layout loader thread");
        let join_wait = t_join.elapsed();
        if !args.benching() {
            eprintln!("eludite: layout from {source:?}");
        }

        let controller = DockController::new(layout, ToolWindowRegistry::vs_default());
        let mut commands = builtins::default_registry();
        view::register(&mut commands, Arc::new(controller.clone()))
            .expect("view commands register once");
        // The host starts on the first solution open, never at startup.
        let mut services = crate::shell::register_workspace(&mut commands, HostLaunch::locate());
        services.agents.preferred = args.agent.clone();
        services.agents.transcript_out = args.transcript_out.clone();
        if let Some(fake) = &args.bench_agent_stream {
            services.agents.registry = Some(vec![eludite_acp::RegisteredAgent {
                descriptor: eludite_acp::AgentDescriptor {
                    name: "Fake streamer".into(),
                    command: fake.to_string_lossy().into_owned(),
                    args: ["--scenario", "stream", "--chunks", "2000", "--rate", "200"]
                        .map(str::to_owned)
                        .to_vec(),
                    env: Vec::new(),
                    env_remove: Vec::new(),
                },
                source: eludite_acp::AgentSource::Settings,
            }]);
        }
        let session = services.session.clone();
        let commands = Arc::new(commands);
        let persistence = store.map(|s| Persistence {
            path: s.path_for(args.solution.as_deref()),
            writer: LayoutWriter::spawn(LayoutWriter::DEFAULT_DEBOUNCE, None),
        });

        bind_keymap(cx, &vs_keymap());
        bind_editor_keys(cx);
        let title = match &args.solution {
            Some(s) => format!(
                "{} - Eludite",
                s.file_stem()
                    .map_or("Solution".into(), |n| n.to_string_lossy())
            ),
            None => "Eludite".to_owned(),
        };
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            app_id: Some("eludite".into()),
            // The harness must not be throttled if the compositor withholds focus.
            inactive_frame_interval: if args.benching() {
                None
            } else {
                WindowOptions::default().inactive_frame_interval
            },
            ..Default::default()
        };
        let window = cx
            .open_window(options, |window, cx| {
                let shell = cx.new(|cx| {
                    Shell::new(
                        commands,
                        controller,
                        theme,
                        persistence,
                        services,
                        window,
                        cx,
                    )
                });
                shell.focus_handle(cx).focus(window, cx);
                shell
            })
            .expect("failed to open the Eludite window");
        let shell = window.entity(cx).expect("shell entity");
        let dock = shell.read(cx).dock().clone();
        // Closing the main window quits, floating tool windows included.
        let _ = window.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                true
            });
        });
        // Stop eludite-host on exit, off the UI thread; GPUI waits for quit handlers up to its shutdown timeout.
        cx.on_app_quit(move |_| {
            let done = session.shutdown();
            let (tx, rx) = futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = done.recv_timeout(Duration::from_secs(5));
                let _ = tx.send(());
            });
            async move {
                let _ = rx.await;
            }
        })
        .detach();
        // Save on exit: flush the writer thread; GPUI waits for quit handlers
        // (up to its shutdown timeout) without blocking a frame.
        dock.update(cx, |_, cx| {
            cx.on_app_quit(|dock: &mut DockHost, _| {
                let flushed = dock.flush();
                async move {
                    if let Some(rx) = flushed {
                        let _ = rx.await;
                    }
                }
            })
            .detach();
        });
        DockHost::sync_floating_windows(&dock, cx);
        cx.activate(true);

        if args.bench_start {
            bench::start(&shell, t_main, join_wait, cx);
        }
        if let Some(frames) = args.bench_drag {
            bench::drag(&shell, window.into(), frames, cx);
        }
        if let Some(n) = args.bench_agent_ready {
            bench::agent_ready(&shell, n, t_main, cx);
        }
        if args.bench_agent_stream.is_some() {
            bench::agent_stream(&shell, cx);
        }
        if let Some(n) = args.bench_diff {
            bench::diff(&shell, n, cx);
        }
        if let Some(secs) = args.bench_output {
            bench::output_stream(&shell, secs, cx);
        }
        if let Some(path) = args.bounds_out.clone() {
            bench::bounds_out(&shell, path, cx);
        }
        // Stop the generic language servers (rust-analyzer) on exit too, off the UI thread.
        shell.update(cx, |_, cx| {
            cx.on_app_quit(|shell: &mut Shell, _| {
                let done = shell.shutdown_generic();
                let (tx, rx) = futures::channel::oneshot::channel();
                std::thread::spawn(move || {
                    for d in done {
                        let _ = d.recv_timeout(Duration::from_secs(5));
                    }
                    let _ = tx.send(());
                });
                async move {
                    let _ = rx.await;
                }
            })
            .detach();
        });
        if args.solution.is_some() || args.folder.is_some() || args.open_file.is_some() {
            // Open the solution once the window is up, as File > Open > Project/Solution would, then the file.
            let solution = args.solution.clone();
            let folder = args.folder.clone();
            let open_file = args.open_file.clone();
            let timings_out = args.timings_out.clone();
            let bench_type = args.bench_type;
            let bench_complete = args.bench_complete;
            let bench_navigate = args.bench_navigate;
            let bench_refactor = args.bench_refactor;
            let bench_build = args.bench_build;
            let bench_debug = args.bench_debug;
            let _ = window.update(cx, |shell, window, cx| {
                if let Some(folder) = &folder {
                    shell.run(
                        workspace::WORKSPACE_OPEN_FOLDER,
                        json!({ "path": folder.to_string_lossy() }),
                        window,
                        cx,
                    );
                }
                if let Some(solution) = &solution {
                    shell.run(
                        workspace::SOLUTION_OPEN,
                        json!({ "path": solution.to_string_lossy() }),
                        window,
                        cx,
                    );
                }
                if let Some(file) = &open_file {
                    shell.run(
                        workspace::FILE_OPEN,
                        json!({ "path": file.to_string_lossy() }),
                        window,
                        cx,
                    );
                }
                if let Some(path) = timings_out {
                    bench::timings_out(cx.entity(), path, window, cx);
                }
                if let (Some(count), Some(file)) = (bench_type, open_file.clone()) {
                    let with_host = solution.is_some() || folder.is_some();
                    bench::type_keys(cx.entity(), file, count, with_host, window, cx);
                }
                if let (Some(count), Some(file)) = (bench_complete, open_file.clone()) {
                    bench::complete(cx.entity(), file, count, window, cx);
                }
                if let (Some(count), Some(file)) = (bench_navigate, open_file.clone()) {
                    bench::navigate(cx.entity(), file, count, window, cx);
                }
                if let (Some(count), Some(file)) = (bench_refactor, open_file.clone()) {
                    bench::refactor(cx.entity(), file, count, window, cx);
                }
                if let Some(count) = bench_build {
                    bench::build_keys(cx.entity(), count, window, cx);
                }
                if let (Some(sessions), Some(file)) = (bench_debug, open_file) {
                    bench::debug(cx.entity(), file, sessions, window, cx);
                }
            });
        }
        if let Some(after) = args.exit_after_ms {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(after))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}
