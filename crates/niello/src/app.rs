//! Process wiring: load the layout, build the command bus, open the window.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Bounds, Focusable as _, TitlebarOptions, WindowBounds, WindowOptions, px,
    size,
};
use niello_commands::{builtins, view};
use niello_docking::{
    DockController, DockHost, DockLayout, LayoutSource, LayoutStore, LayoutWriter, Persistence,
    ToolWindowRegistry,
};
use niello_ui::{Theme, bind_keymap, vs_keymap};

use crate::args::Args;
use crate::bench;
use crate::shell::Shell;

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
            eprintln!("niello: layout from {source:?}");
        }

        let controller = DockController::new(layout, ToolWindowRegistry::vs_default());
        let mut commands = builtins::default_registry();
        view::register(&mut commands, Arc::new(controller.clone()))
            .expect("view commands register once");
        let commands = Arc::new(commands);
        let persistence = store.map(|s| Persistence {
            path: s.path_for(args.solution.as_deref()),
            writer: LayoutWriter::spawn(LayoutWriter::DEFAULT_DEBOUNCE, None),
        });

        bind_keymap(cx, &vs_keymap());
        let title = match &args.solution {
            Some(s) => format!(
                "{} - Niello",
                s.file_stem()
                    .map_or("Solution".into(), |n| n.to_string_lossy())
            ),
            None => "Niello".to_owned(),
        };
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            app_id: Some("niello".into()),
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
                let shell = cx.new(|cx| Shell::new(commands, controller, theme, persistence, cx));
                shell.focus_handle(cx).focus(window, cx);
                shell
            })
            .expect("failed to open the Niello window");
        let shell = window.entity(cx).expect("shell entity");
        let dock = shell.read(cx).dock().clone();
        // Closing the main window quits, floating tool windows included.
        let _ = window.update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                true
            });
        });
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
