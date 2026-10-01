//! Niello application entry point (PLAN.md D1, section 8, section 12 `crates/niello`).
//!
//! Opens one GPUI window showing the static Visual Studio-style shell. The
//! layout lives in `niello-docking`, the widgets in `niello-ui`.

mod shell;

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

fn main() {
    let commands = niello_commands::builtins::default_registry();

    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Niello".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| shell::Shell::new(commands)))
            .expect("failed to open the Niello window");
        cx.activate(true);
    });
}
