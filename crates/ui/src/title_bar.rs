//! The main window's title bar, drawn by Eludite (brief 0044, ADR-0010): Visual Studio 2022's layout, the menu bar in
//! the title bar beside the window's title, the caption buttons (Minimize, Maximize or Restore, Close) at the right,
//! and on Linux the window's frame (border, shadow, resize edges) when the compositor gives the window client-side
//! decorations.
//!
//! Public API: [`chrome`] decides what is drawn from the platform and the decorations the window got; [`TitleBar`]
//! draws the row; [`client_frame`] wraps the window's content on Linux. Where the platform draws the title bar (a
//! Linux compositor without client-side decorations) [`Chrome::custom`] is false and the caller keeps its plain menu
//! row.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, CursorStyle, Decorations, Div, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, PathBuilder, Pixels, Point, ResizeEdge, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Tiling, Window, WindowControlArea, canvas, div, point, px,
    rgb,
};

use crate::Theme;

/// The title bar's height.
pub const TITLE_BAR_HEIGHT: Pixels = px(32.);
/// A caption button's width (Windows' own).
pub const CAPTION_BUTTON_WIDTH: Pixels = px(46.);
/// The room macOS's window buttons take at the row's left.
pub const MAC_BUTTONS_INSET: Pixels = px(78.);
/// Where macOS draws its window buttons (`TitlebarOptions::traffic_light_position`): centered in the row.
pub const MAC_BUTTONS_POSITION: Point<Pixels> = point(px(12.), px(10.));
/// The invisible margin around a client-decorated Linux window: its shadow, and where it resizes.
pub const FRAME_INSET: Pixels = px(10.);

/// The platform whose conventions the title bar follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build runs on (Linux stands for every other Unix).
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// Who draws and handles the caption buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionButtons {
    /// None in the row: macOS draws its own; a full-screen window has none.
    None,
    /// Drawn by Eludite and pressed through the platform: Windows hit-tests them as the window's caption buttons, so
    /// hovering Maximize offers the snap layouts.
    Platform,
    /// Drawn and handled by Eludite (Linux).
    Drawn,
}

/// What the main window's chrome looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chrome {
    /// Eludite draws the title bar; false where the platform draws it.
    pub custom: bool,
    pub platform: Platform,
    /// Room left free at the row's left (macOS's buttons).
    pub left_inset: Pixels,
    pub buttons: CaptionButtons,
    /// `Some` when Eludite draws the window's border, shadow and resize edges (Linux), with the edges the compositor
    /// tiled (those have none).
    pub frame: Option<Tiling>,
}

/// The chrome for `platform` given the `decorations` the window got and whether it is full screen.
pub fn chrome(platform: Platform, decorations: Decorations, fullscreen: bool) -> Chrome {
    let buttons = |b| if fullscreen { CaptionButtons::None } else { b };
    match (platform, decorations) {
        (Platform::Mac, _) => Chrome {
            custom: true,
            platform,
            left_inset: if fullscreen {
                px(0.)
            } else {
                MAC_BUTTONS_INSET
            },
            buttons: CaptionButtons::None,
            frame: None,
        },
        (Platform::Windows, _) => Chrome {
            custom: true,
            platform,
            left_inset: px(0.),
            buttons: buttons(CaptionButtons::Platform),
            frame: None,
        },
        (Platform::Linux, Decorations::Client { tiling }) => Chrome {
            custom: true,
            platform,
            left_inset: px(0.),
            buttons: buttons(CaptionButtons::Drawn),
            frame: Some(if fullscreen { Tiling::tiled() } else { tiling }),
        },
        (Platform::Linux, Decorations::Server) => Chrome {
            custom: false,
            platform,
            left_inset: px(0.),
            buttons: CaptionButtons::None,
            frame: None,
        },
    }
}

/// A caption button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    /// The element id and debug selector: `caption-minimize`, `caption-maximize`, `caption-restore`, `caption-close`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Minimize => "caption-minimize",
            Self::Maximize => "caption-maximize",
            Self::Restore => "caption-restore",
            Self::Close => "caption-close",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }

    /// The glyph's strokes, 10 pixels square around `c`, as line strips.
    fn strokes(self, c: Point<Pixels>) -> Vec<Vec<Point<Pixels>>> {
        let at = |x: f32, y: f32| point(c.x + px(x), c.y + px(y));
        match self {
            Self::Minimize => vec![vec![at(-5., 0.5), at(5., 0.5)]],
            Self::Maximize => vec![vec![
                at(-4.5, -4.5),
                at(4.5, -4.5),
                at(4.5, 4.5),
                at(-4.5, 4.5),
                at(-4.5, -4.5),
            ]],
            Self::Restore => vec![
                vec![
                    at(-4.5, -2.5),
                    at(2.5, -2.5),
                    at(2.5, 4.5),
                    at(-4.5, 4.5),
                    at(-4.5, -2.5),
                ],
                vec![
                    at(-2.5, -2.5),
                    at(-2.5, -4.5),
                    at(4.5, -4.5),
                    at(4.5, 2.5),
                    at(2.5, 2.5),
                ],
            ],
            Self::Close => vec![
                vec![at(-5., -5.), at(5., 5.)],
                vec![at(5., -5.), at(-5., 5.)],
            ],
        }
    }
}

/// What the row holds besides the caption buttons.
pub struct TitleBarContent {
    /// The window's title, on the part of the row that drags the window.
    pub title: SharedString,
    /// The menu bar, at the left (after macOS's buttons).
    pub menu: AnyElement,
    /// The build toolbar, before the caption buttons.
    pub tools: AnyElement,
}

/// What Eludite's own Close button does (Linux).
type OnClose = Rc<dyn Fn(&mut Window, &mut App)>;

/// Windows' red for a hovered Close button.
const CLOSE_HOVER: u32 = 0xE81123;

/// The title bar row. Keeps one bit of state: a press on the empty part of the row that moves the window once the
/// mouse moves (so a double-click still maximizes).
#[derive(Debug, Default)]
pub struct TitleBar {
    armed: Rc<Cell<bool>>,
}

impl TitleBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// The row: the menu bar, the title on the part of the row that drags the window, the build toolbar, and the
    /// caption buttons. `on_close` runs when Eludite's own Close button is pressed (Linux); elsewhere the platform
    /// closes the window.
    pub fn render(
        &self,
        chrome: Chrome,
        theme: &Theme,
        content: TitleBarContent,
        window: &Window,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let TitleBarContent { title, menu, tools } = content;
        let active = window.is_window_active();
        let mut drag = div()
            .id("title-bar-drag")
            .debug_selector(|| "title-bar-drag".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .debug_selector(|| "title-bar-title".into())
                    .min_w_0()
                    .px_2()
                    .py(px(2.))
                    .rounded_sm()
                    .bg(theme.menu_hover)
                    .text_color(if active {
                        theme.menu_text
                    } else {
                        theme.text_muted
                    })
                    .text_size(theme.typography.ui)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            );
        // Windows moves, snaps and maximizes the window itself over the drag area; elsewhere the row asks for it.
        if chrome.platform != Platform::Windows {
            let platform = chrome.platform;
            let (down, up, moved) = (self.armed.clone(), self.armed.clone(), self.armed.clone());
            drag = drag
                .on_mouse_down(MouseButton::Left, move |e, window, _| {
                    down.set(e.click_count < 2);
                    if e.click_count == 2 {
                        match platform {
                            Platform::Mac => window.titlebar_double_click(),
                            _ => window.zoom_window(),
                        }
                    }
                })
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(false))
                .on_mouse_move(move |e, window, _| {
                    if moved.get() && e.pressed_button == Some(MouseButton::Left) {
                        moved.set(false);
                        window.start_window_move();
                    }
                });
            if platform == Platform::Linux && window.window_controls().window_menu {
                drag = drag.on_mouse_down(MouseButton::Right, |e, window, _| {
                    window.show_window_menu(e.position)
                });
            }
        }
        let on_close: OnClose = Rc::new(on_close);
        let maximized = window.is_maximized();
        let captions = match chrome.buttons {
            CaptionButtons::None => Vec::new(),
            _ => vec![
                Caption::Minimize,
                if maximized {
                    Caption::Restore
                } else {
                    Caption::Maximize
                },
                Caption::Close,
            ],
        };
        div()
            .id("title-bar")
            .debug_selector(|| "title-bar".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(TITLE_BAR_HEIGHT)
            .pl(chrome.left_inset)
            .bg(theme.menu_background)
            .child(div().flex_none().child(menu))
            .child(drag)
            .child(div().flex_none().child(tools))
            .children(captions.into_iter().map(|caption| {
                caption_button(caption, chrome.buttons, theme, active, on_close.clone())
            }))
    }
}

fn caption_button(
    caption: Caption,
    buttons: CaptionButtons,
    theme: &Theme,
    active: bool,
    on_close: OnClose,
) -> Stateful<Div> {
    let id = caption.id();
    let close = caption == Caption::Close;
    let hover = if close {
        rgb(CLOSE_HOVER)
    } else {
        theme.menu_hover
    };
    let color: Hsla = if active {
        theme.menu_text
    } else {
        theme.text_muted
    }
    .into();
    let button = div()
        .id(id)
        .debug_selector(move || id.into())
        .relative()
        .flex_none()
        .w(CAPTION_BUTTON_WIDTH)
        .h_full()
        .hover(move |s| s.bg(hover))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    // White on Close's red, as Windows draws it.
                    let color = if close && bounds.contains(&window.mouse_position()) {
                        gpui::white()
                    } else {
                        color
                    };
                    for strip in caption.strokes(bounds.center()) {
                        let mut path = PathBuilder::stroke(px(1.));
                        path.move_to(strip[0]);
                        for p in &strip[1..] {
                            path.line_to(*p);
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        );
    match buttons {
        CaptionButtons::Platform => button.window_control_area(caption.area()),
        _ => button
            // The press is the button's, not the row's.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| match caption {
                Caption::Minimize => window.minimize_window(),
                Caption::Maximize | Caption::Restore => window.zoom_window(),
                Caption::Close => on_close(window, cx),
            }),
    }
}

/// The resize edges a frame with `tiling` offers: every edge and corner not against a tiled side.
pub fn resize_edges(tiling: Tiling) -> Vec<ResizeEdge> {
    use ResizeEdge::*;
    [
        (Top, !tiling.top),
        (Bottom, !tiling.bottom),
        (Left, !tiling.left),
        (Right, !tiling.right),
        (TopLeft, !tiling.top && !tiling.left),
        (TopRight, !tiling.top && !tiling.right),
        (BottomLeft, !tiling.bottom && !tiling.left),
        (BottomRight, !tiling.bottom && !tiling.right),
    ]
    .into_iter()
    .filter_map(|(edge, free)| free.then_some(edge))
    .collect()
}

fn edge_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// The handle for `edge`: a strip of the margin along it (corners a square of one and a half margins).
fn edge_handle(edge: ResizeEdge) -> Div {
    let corner = FRAME_INSET * 1.5;
    let h = div().absolute().cursor(edge_cursor(edge));
    let h = match edge {
        ResizeEdge::Top => h.top_0().left(corner).right(corner).h(FRAME_INSET),
        ResizeEdge::Bottom => h.bottom_0().left(corner).right(corner).h(FRAME_INSET),
        ResizeEdge::Left => h.left_0().top(corner).bottom(corner).w(FRAME_INSET),
        ResizeEdge::Right => h.right_0().top(corner).bottom(corner).w(FRAME_INSET),
        ResizeEdge::TopLeft => h.top_0().left_0().size(corner),
        ResizeEdge::TopRight => h.top_0().right_0().size(corner),
        ResizeEdge::BottomLeft => h.bottom_0().left_0().size(corner),
        ResizeEdge::BottomRight => h.bottom_0().right_0().size(corner),
    };
    h.on_mouse_down(MouseButton::Left, move |_, window, cx| {
        cx.stop_propagation();
        window.start_window_resize(edge);
    })
}

/// The window's frame on Linux with client-side decorations: a margin of [`FRAME_INSET`] (shadow and resize edges) on
/// each side the compositor did not tile, and a one-pixel border, the accent color while the window is active (as
/// Visual Studio's). Sets the window's client inset; call it on every frame the window is client-decorated.
pub fn client_frame(
    content: impl IntoElement,
    tiling: Tiling,
    theme: &Theme,
    window: &mut Window,
) -> Div {
    window.set_client_inset(FRAME_INSET);
    let border = if window.is_window_active() {
        theme.accent
    } else {
        theme.border
    };
    let mut inner = div()
        .debug_selector(|| "window-frame".into())
        .size_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .border_color(border)
        .child(content);
    let mut outer = div().size_full().relative();
    if !tiling.top {
        outer = outer.pt(FRAME_INSET);
        inner = inner.border_t_1();
    }
    if !tiling.bottom {
        outer = outer.pb(FRAME_INSET);
        inner = inner.border_b_1();
    }
    if !tiling.left {
        outer = outer.pl(FRAME_INSET);
        inner = inner.border_l_1();
    }
    if !tiling.right {
        outer = outer.pr(FRAME_INSET);
        inner = inner.border_r_1();
    }
    if !tiling.is_tiled() {
        inner = inner.shadow(vec![
            gpui::BoxShadow::new(px(0.), px(0.), gpui::black().opacity(0.4))
                .blur_radius(FRAME_INSET / 2.),
        ]);
    }
    let handles = if window.is_resizable() {
        resize_edges(tiling)
    } else {
        Vec::new()
    };
    outer
        .child(inner)
        .children(handles.into_iter().map(edge_handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_per_platform() {
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };
        let mac = chrome(Platform::Mac, Decorations::Server, false);
        assert!(mac.custom);
        assert_eq!(mac.left_inset, MAC_BUTTONS_INSET);
        assert_eq!(mac.buttons, CaptionButtons::None);
        assert_eq!(
            chrome(Platform::Mac, Decorations::Server, true).left_inset,
            px(0.)
        );

        let win = chrome(Platform::Windows, Decorations::Server, false);
        assert!(win.custom);
        assert_eq!(win.buttons, CaptionButtons::Platform);
        assert_eq!(win.frame, None);
        assert_eq!(
            chrome(Platform::Windows, Decorations::Server, true).buttons,
            CaptionButtons::None
        );

        let linux = chrome(Platform::Linux, client, false);
        assert!(linux.custom);
        assert_eq!(linux.buttons, CaptionButtons::Drawn);
        assert_eq!(linux.frame, Some(Tiling::default()));
        let full = chrome(Platform::Linux, client, true);
        assert_eq!(full.buttons, CaptionButtons::None);
        assert_eq!(full.frame, Some(Tiling::tiled()));

        // A compositor that keeps server-side decorations draws the title bar itself.
        let server = chrome(Platform::Linux, Decorations::Server, false);
        assert!(!server.custom);
        assert_eq!(server.buttons, CaptionButtons::None);
        assert_eq!(server.frame, None);
    }

    #[test]
    fn resize_edges_skip_tiled_sides() {
        assert_eq!(resize_edges(Tiling::default()).len(), 8);
        assert!(resize_edges(Tiling::tiled()).is_empty());
        let left_half = Tiling {
            top: true,
            bottom: true,
            left: true,
            right: false,
        };
        assert_eq!(resize_edges(left_half), [ResizeEdge::Right]);
    }

    #[test]
    fn glyphs_stay_in_ten_pixels() {
        let c = point(px(23.), px(16.));
        for caption in [
            Caption::Minimize,
            Caption::Maximize,
            Caption::Restore,
            Caption::Close,
        ] {
            for p in caption.strokes(c).into_iter().flatten() {
                assert!(
                    (p.x - c.x).abs() <= px(5.) && (p.y - c.y).abs() <= px(5.),
                    "{caption:?}"
                );
            }
        }
    }
}
