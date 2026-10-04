//! [`TerminalView`]: our own GPUI view of a [`Terminal`] (ADR-0001: never Zed's `terminal_view`).
//!
//! - **Drawing.** Each frame the element takes a snapshot of the visible rows under the grid's lock, but only when
//!   the lock is free at once or the terminal changed since the last snapshot (then it waits at most one parse
//!   chunk); otherwise it redraws the last snapshot. Rows are shaped with the cell width forced, so the grid stays a
//!   grid, in the theme's 16 colors; the cursor, the selection, find matches and the bell's flash are quads.
//! - **Keys.** Every key goes to the terminal ([`crate::keys`]) except Visual Studio's clipboard keys (Ctrl+C with a
//!   selection, Ctrl+Shift+C, Ctrl+Insert copy; Ctrl+V, Ctrl+Shift+V, Shift+Insert paste), Ctrl+F (find in the
//!   scrollback), Shift+PageUp and Shift+PageDown (scroll), and Escape (back to the editor,
//!   [`TerminalViewEvent::FocusEditor`]). The shell's reserved chords are kept from the terminal by its key bindings
//!   (the shell binds them; [`bind_keys`] takes the rest away from the shell's keymap while a terminal has focus).
//! - **Mouse.** A drag selects (copied on release with `copy_on_select`); Ctrl+click opens a link
//!   ([`TerminalViewEvent::OpenLink`]); the wheel scrolls the scrollback, or sends arrows to a full-screen
//!   application (the alternate screen).

use std::ops::Range;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use eludite_ui::Theme;
use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, LayoutId, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Rgba,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Task, TextAlign, TextRun,
    UnderlineStyle, Window, div, fill, point, px, relative, size,
};

use crate::links::{self, Target};
use crate::pty::{self, Terminal};

/// The key context of a terminal view.
pub const KEY_CONTEXT: &str = "Terminal";

/// The view's settings (from `terminal.*`).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewSettings {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub copy_on_select: bool,
    /// `terminal.bell: visual`.
    pub visual_bell: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            font_family: default_font_family(),
            font_size: px(13.),
            copy_on_select: false,
            visual_bell: true,
        }
    }
}

/// Cascadia Mono on Windows (Visual Studio's terminal font), Menlo on macOS, else Noto Sans Mono, unless
/// `ELUDITE_EDITOR_FONT` says otherwise (the editor's rule).
pub fn default_font_family() -> SharedString {
    if let Ok(f) = std::env::var("ELUDITE_EDITOR_FONT") {
        return f.into();
    }
    if cfg!(target_os = "windows") {
        "Cascadia Mono".into()
    } else if cfg!(target_os = "macos") {
        "Menlo".into()
    } else {
        "Noto Sans Mono".into()
    }
}

/// What the view asks of its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalViewEvent {
    /// Ctrl+click on a link.
    OpenLink(Target),
    /// Escape: focus the editor.
    FocusEditor,
    /// The person typed or pasted into the terminal (an agent's wait ends).
    Typed,
    /// The exit line's Restart.
    Restart,
}

/// A cell as drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CellSnap {
    c: char,
    fg: Rgba,
    bg: Option<Rgba>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// The visible rows at one moment.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    rows: Vec<Vec<CellSnap>>,
    /// Row and column, when the cursor shows in the visible rows.
    cursor: Option<(usize, usize, CursorShape)>,
    mode: Option<TermMode>,
    history: usize,
    /// The grid line of the first visible row.
    top: i32,
    generation: u64,
}

/// A find match: the absolute line and the columns.
pub type FindMatch = (usize, Range<usize>);

/// A shaped row and its background runs.
type ShapedRow = (gpui::ShapedLine, Vec<(Range<usize>, Rgba)>);

/// A point of the scrollback and screen, counted from the oldest line kept (stable while the scrollback grows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsPoint {
    pub line: usize,
    pub column: usize,
}

/// Find in the scrollback.
#[derive(Debug, Default)]
struct Find {
    query: String,
    matches: Vec<(usize, Range<usize>)>,
    current: Option<usize>,
    focus: Option<FocusHandle>,
    task: Option<Task<()>>,
}

/// The 16 ANSI colors for a theme: Visual Studio's dark terminal palette, or a light one on the Light theme.
pub fn palette(theme: &Theme) -> [Rgba; 16] {
    let dark = [
        0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC, 0x767676,
        0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
    ];
    let light = [
        0x000000, 0xCD3131, 0x00BC00, 0x949800, 0x0451A5, 0xBC05BC, 0x0598BC, 0x555555, 0x666666,
        0xCD3131, 0x14CE14, 0xB5BA00, 0x0451A5, 0xBC05BC, 0x0598BC, 0xA5A5A5,
    ];
    let p = if theme.name == "light" { light } else { dark };
    p.map(gpui::rgb)
}

fn rgb_of(c: Rgb) -> Rgba {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32)
}

/// xterm's 256-color cube and grays for indexes 16 to 255.
fn indexed(i: u8, p: &[Rgba; 16]) -> Rgba {
    match i {
        0..=15 => p[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            let (r, g, b) = (level(i / 36), level((i / 6) % 6), level(i % 6));
            gpui::rgb((r << 16) | (g << 8) | b)
        }
        _ => {
            let v = 8 + (i as u32 - 232) * 10;
            gpui::rgb((v << 16) | (v << 8) | v)
        }
    }
}

/// The view's colors.
#[derive(Debug, Clone, Copy)]
struct Colors {
    palette: [Rgba; 16],
    fg: Rgba,
    bg: Rgba,
    cursor: Rgba,
    selection: Rgba,
    find: Rgba,
}

impl Colors {
    fn of(theme: &Theme) -> Self {
        let mut selection = theme.accent;
        selection.a = 0.45;
        Self {
            palette: palette(theme),
            fg: theme.text,
            bg: theme.background,
            cursor: theme.text,
            selection,
            find: gpui::rgba(0xF2CC6080),
        }
    }

    fn color(
        &self,
        c: &Color,
        overrides: &alacritty_terminal::term::color::Colors,
        fg: bool,
    ) -> Rgba {
        match c {
            Color::Spec(rgb) => rgb_of(*rgb),
            Color::Indexed(i) => {
                overrides[*i as usize].map_or_else(|| indexed(*i, &self.palette), rgb_of)
            }
            Color::Named(n) => {
                let ix = *n as usize;
                if let Some(o) = overrides[ix] {
                    return rgb_of(o);
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    NamedColor::DimForeground => {
                        let mut f = self.fg;
                        f.a = 0.7;
                        f
                    }
                    _ if ix < 16 => self.palette[ix],
                    // The dim colors.
                    _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                        .contains(&ix) =>
                    {
                        let mut f = self.palette[ix - NamedColor::DimBlack as usize];
                        f.a = 0.7;
                        f
                    }
                    _ if fg => self.fg,
                    _ => self.bg,
                }
            }
        }
    }
}

/// Where the grid was last laid out (for the mouse).
#[derive(Debug, Clone, Copy, Default)]
struct Layout {
    origin: gpui::Point<Pixels>,
    cell: gpui::Size<Pixels>,
    cols: usize,
    rows: usize,
}

/// A terminal's view.
pub struct TerminalView {
    terminal: Terminal,
    theme: Theme,
    colors: Colors,
    settings: ViewSettings,
    focus: FocusHandle,
    snapshot: Snapshot,
    layout: Layout,
    /// Lines scrolled up into the scrollback (0: the bottom).
    scroll: usize,
    selection: Option<(AbsPoint, AbsPoint)>,
    selecting: bool,
    find: Option<Find>,
    bell_until: Option<Instant>,
    /// When the last key went to the terminal, and the terminal's generation then.
    pending_key: Option<(Instant, u64)>,
    /// Keystroke to a frame with new content, for the report's numbers.
    key_latencies: Vec<Duration>,
    /// The terminal element's prepaint and paint, per frame.
    frame_times: Vec<Duration>,
    _bell_task: Option<Task<()>>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Bind the keys a terminal takes from the shell's keymap: every binding of `shell_keys` whose keystrokes are not
/// in `reserved` is disabled while a terminal has focus, so the key reaches the terminal. Call after the shell's
/// keymap is bound (later bindings win).
pub fn bind_keys(cx: &mut App, shell_keys: &[&str], reserved: &[&str]) {
    cx.bind_keys(
        shell_keys
            .iter()
            .filter(|k| !reserved.contains(k))
            .map(|k| KeyBinding::new(k, gpui::NoAction, Some(KEY_CONTEXT))),
    );
}

impl TerminalView {
    pub fn new(
        terminal: Terminal,
        theme: Theme,
        settings: ViewSettings,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            terminal,
            colors: Colors::of(&theme),
            theme,
            settings,
            focus: cx.focus_handle(),
            snapshot: Snapshot::default(),
            layout: Layout::default(),
            scroll: 0,
            selection: None,
            selecting: false,
            find: None,
            bell_until: None,
            pending_key: None,
            key_latencies: Vec::new(),
            frame_times: Vec::new(),
            _bell_task: None,
        }
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn set_settings(&mut self, settings: ViewSettings, cx: &mut Context<Self>) {
        if self.settings != settings {
            self.settings = settings;
            cx.notify();
        }
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        self.colors = Colors::of(&theme);
        cx.notify();
    }

    /// The terminal changed (its I/O thread said so): draw the next frame.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// The bell rang: flash the frame for 150 ms (with `terminal.bell: visual`).
    pub fn bell(&mut self, cx: &mut Context<Self>) {
        if !self.settings.visual_bell {
            return;
        }
        self.bell_until = Some(Instant::now() + Duration::from_millis(150));
        cx.notify();
        self._bell_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(160))
                .await;
            let _ = this.update(cx, |v, cx| {
                v.bell_until = None;
                cx.notify();
            });
        }));
    }

    /// Whether the bell's flash shows.
    pub fn flashing(&self) -> bool {
        self.bell_until.is_some_and(|t| Instant::now() < t)
    }

    /// Keystroke-to-new-content latencies measured so far.
    pub fn key_latencies(&self) -> &[Duration] {
        &self.key_latencies
    }

    /// The terminal element's time per frame (prepaint and paint).
    pub fn frame_times(&self) -> &[Duration] {
        &self.frame_times
    }

    /// The text of the last drawn snapshot (the visible rows), trailing spaces trimmed.
    pub fn visible_text(&self) -> String {
        let mut lines: Vec<String> = self
            .snapshot
            .rows
            .iter()
            .map(|r| {
                r.iter()
                    .map(|c| c.c)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// The selection, as absolute points (start, end).
    pub fn selection(&self) -> Option<(AbsPoint, AbsPoint)> {
        self.selection
    }

    pub fn set_selection(&mut self, sel: Option<(AbsPoint, AbsPoint)>, cx: &mut Context<Self>) {
        self.selection = sel;
        cx.notify();
    }

    /// The selection's text, as it is copied.
    pub fn selection_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let text = self.terminal.with_term_blocking(|t| {
            let history = t.grid().history_size() as i32;
            let to_grid = |p: AbsPoint| {
                let line =
                    (p.line as i32 - history).clamp(-history, t.grid().screen_lines() as i32 - 1);
                GridPoint::new(Line(line), Column(p.column.min(t.grid().columns() - 1)))
            };
            pty::text_between(t, to_grid(a), to_grid(b))
        });
        (!text.is_empty()).then_some(text)
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }

    /// Scroll `lines` up (positive) or down into the scrollback.
    pub fn scroll_by(&mut self, lines: i32, cx: &mut Context<Self>) {
        let max = self.snapshot.history as i32;
        self.scroll = (self.scroll as i32 + lines).clamp(0, max) as usize;
        cx.notify();
    }

    /// Copy the selection; false when there is none.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selection_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    /// Paste the clipboard's text.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            self.terminal.paste(&text);
            self.typed(cx);
        }
    }

    /// Send `bytes` as the person's input.
    pub fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.pending_key = Some((Instant::now(), self.terminal.generation()));
        self.terminal.write(bytes);
        self.typed(cx);
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        self.scroll = 0;
        if self.selection.is_some() && !self.selecting {
            self.selection = None;
        }
        cx.emit(TerminalViewEvent::Typed);
        cx.notify();
    }

    /// Handle a key: the clipboard keys, find, scrolling, Escape; everything else goes to the terminal.
    pub fn key_down(
        &mut self,
        ks: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = ks.modifiers;
        let key = ks.key.as_str();
        let plain = |mods: Modifiers| mods == m;
        match key {
            "c" if plain(Modifiers::control_shift()) => {
                self.copy(cx);
                return true;
            }
            "v" if plain(Modifiers::control_shift()) || plain(Modifiers::control()) => {
                self.paste(cx);
                return true;
            }
            "insert" if plain(Modifiers::control()) => {
                self.copy(cx);
                return true;
            }
            "insert" if plain(Modifiers::shift()) => {
                self.paste(cx);
                return true;
            }
            // Visual Studio's choice: Ctrl+C copies a selection, else interrupts.
            "c" if plain(Modifiers::control()) && self.selection.is_some() => {
                self.copy(cx);
                self.selection = None;
                cx.notify();
                return true;
            }
            "f" if plain(Modifiers::control()) => {
                self.open_find(window, cx);
                return true;
            }
            "pageup" if plain(Modifiers::shift()) => {
                let page = self.layout.rows.max(2) as i32 - 1;
                self.scroll_by(page, cx);
                return true;
            }
            "pagedown" if plain(Modifiers::shift()) => {
                let page = self.layout.rows.max(2) as i32 - 1;
                self.scroll_by(-page, cx);
                return true;
            }
            "escape" if m == Modifiers::none() => {
                cx.emit(TerminalViewEvent::FocusEditor);
                return true;
            }
            _ => {}
        }
        let app_cursor = self
            .snapshot
            .mode
            .is_some_and(|mode| mode.contains(TermMode::APP_CURSOR));
        match crate::keys::to_bytes(ks, app_cursor) {
            Some(bytes) => {
                self.input(bytes, cx);
                true
            }
            None => false,
        }
    }

    // ----- Find -----

    pub fn find_query(&self) -> Option<&str> {
        self.find.as_ref().map(|f| f.query.as_str())
    }

    /// The matches (absolute line, columns) and the current one.
    pub fn find_matches(&self) -> Option<(&[FindMatch], Option<usize>)> {
        self.find
            .as_ref()
            .map(|f| (f.matches.as_slice(), f.current))
    }

    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let query = self.find.take().map(|f| f.query).unwrap_or_default();
        self.find = Some(Find {
            query,
            focus: Some(focus),
            ..Default::default()
        });
        cx.notify();
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Search the scrollback and screen for `query` (case-insensitive), off the UI thread.
    pub fn set_find_query(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.query = query.clone();
        if query.is_empty() {
            find.matches.clear();
            find.current = None;
            cx.notify();
            return;
        }
        let terminal = self.terminal.clone();
        let work = cx.background_spawn(async move { find_in(&terminal, &query) });
        find.task = Some(cx.spawn(async move |this, cx| {
            let found = work.await;
            let _ = this.update(cx, |v, cx| {
                if let Some(f) = v.find.as_mut() {
                    f.current = found.len().checked_sub(1);
                    f.matches = found;
                }
                v.reveal_current(cx);
            });
        }));
        cx.notify();
    }

    /// The next (or previous) match.
    pub fn find_next(&mut self, back: bool, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else {
            return;
        };
        let n = f.matches.len();
        if n == 0 {
            return;
        }
        f.current = Some(match (f.current, back) {
            (None, _) => n - 1,
            (Some(i), true) => (i + n - 1) % n,
            (Some(i), false) => (i + 1) % n,
        });
        self.reveal_current(cx);
    }

    /// Scroll so the current match shows.
    fn reveal_current(&mut self, cx: &mut Context<Self>) {
        let Some((line, _)) = self
            .find
            .as_ref()
            .and_then(|f| f.current.and_then(|i| f.matches.get(i)).cloned())
        else {
            cx.notify();
            return;
        };
        let history = self.snapshot.history;
        let rows = self.layout.rows.max(1);
        // The view's top line (absolute) is history - scroll.
        let top = history.saturating_sub(self.scroll);
        if line < top || line >= top + rows {
            let new_top = line.saturating_sub(rows / 2).min(history);
            self.scroll = history - new_top;
        }
        cx.notify();
    }

    fn find_key(&mut self, ks: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.find.as_ref().map(|f| f.query.clone()) else {
            return;
        };
        match ks.key.as_str() {
            "escape" => self.close_find(window, cx),
            "enter" => self.find_next(ks.modifiers.shift, cx),
            "backspace" => {
                let mut q = query;
                q.pop();
                self.set_find_query(q, cx);
            }
            _ => {
                if ks.modifiers.control || ks.modifiers.alt || ks.modifiers.platform {
                    return;
                }
                if let Some(t) = &ks.key_char {
                    self.set_find_query(format!("{query}{t}"), cx);
                } else if ks.key.chars().count() == 1 {
                    let t = if ks.modifiers.shift {
                        ks.key.to_uppercase()
                    } else {
                        ks.key.clone()
                    };
                    self.set_find_query(format!("{query}{t}"), cx);
                }
            }
        }
    }

    // ----- Mouse -----

    /// The absolute point under a window position.
    fn point_at(&self, pos: gpui::Point<Pixels>) -> Option<AbsPoint> {
        let l = self.layout;
        if l.cell.width <= px(0.) || l.cell.height <= px(0.) {
            return None;
        }
        let x = ((pos.x - l.origin.x) / l.cell.width).floor().max(0.) as usize;
        let y = ((pos.y - l.origin.y) / l.cell.height).floor().max(0.) as usize;
        let row = y.min(l.rows.saturating_sub(1));
        let top = self.snapshot.history.saturating_sub(self.scroll);
        Some(AbsPoint {
            line: top + row,
            column: x.min(l.cols.saturating_sub(1)),
        })
    }

    /// The text of visible row `row` of the snapshot.
    fn row_string(&self, row: usize) -> Option<String> {
        self.snapshot
            .rows
            .get(row)
            .map(|r| r.iter().map(|c| c.c).collect())
    }

    /// The link under a window position.
    pub fn link_at(&self, pos: gpui::Point<Pixels>) -> Option<Target> {
        let p = self.point_at(pos)?;
        let top = self.snapshot.history.saturating_sub(self.scroll);
        let row = p.line.checked_sub(top)?;
        let text = self.row_string(row)?;
        links::link_at(&text, p.column).map(|l| l.target)
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if e.button != MouseButton::Left {
            return;
        }
        if e.modifiers.control || e.modifiers.platform {
            if let Some(target) = self.link_at(e.position) {
                cx.emit(TerminalViewEvent::OpenLink(target));
            }
            return;
        }
        let Some(p) = self.point_at(e.position) else {
            return;
        };
        if e.click_count >= 2 {
            self.select_word(p);
        } else {
            self.selection = Some((p, p));
        }
        self.selecting = true;
        cx.notify();
    }

    fn select_word(&mut self, p: AbsPoint) {
        let top = self.snapshot.history.saturating_sub(self.scroll);
        let Some(text) = p.line.checked_sub(top).and_then(|r| self.row_string(r)) else {
            return;
        };
        let chars: Vec<char> = text.chars().collect();
        let word = |c: char| !c.is_whitespace() && !"()[]{}<>\"'`|,;".contains(c);
        if !chars.get(p.column).is_some_and(|&c| word(c)) {
            self.selection = Some((p, p));
            return;
        }
        let mut a = p.column;
        while a > 0 && word(chars[a - 1]) {
            a -= 1;
        }
        let mut b = p.column;
        while b + 1 < chars.len() && word(chars[b + 1]) {
            b += 1;
        }
        self.selection = Some((
            AbsPoint {
                line: p.line,
                column: a,
            },
            AbsPoint {
                line: p.line,
                column: b,
            },
        ));
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let (Some(p), Some((a, _))) = (self.point_at(e.position), self.selection) {
            self.selection = Some((a, p));
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        // A click without a drag selects nothing.
        if let Some((a, b)) = self.selection
            && a == b
        {
            self.selection = None;
        }
        if self.settings.copy_on_select && self.selection.is_some() {
            self.copy(cx);
        }
        cx.notify();
    }

    fn scroll_wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = match e.delta {
            gpui::ScrollDelta::Lines(d) => d.y.round() as i32 * 3,
            gpui::ScrollDelta::Pixels(d) => {
                let h = self.layout.cell.height.max(px(1.));
                (d.y / h).round() as i32
            }
        };
        if lines == 0 {
            return;
        }
        let mode = self.snapshot.mode.unwrap_or_default();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // A full-screen application scrolls itself: arrows.
            let app = mode.contains(TermMode::APP_CURSOR);
            let one: &[u8] = match (lines > 0, app) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.terminal
                .write(one.repeat(lines.unsigned_abs() as usize));
            return;
        }
        self.scroll_by(lines, cx);
    }

    /// Take a snapshot of the visible rows when the terminal changed (or nothing was drawn yet).
    fn refresh(&mut self, cols: usize, rows: usize) {
        if cols >= 2 && rows >= 1 {
            self.terminal.resize(cols as u16, rows as u16);
        }
        let changed = self.terminal.take_dirty();
        if !changed && !self.snapshot.rows.is_empty() && self.snapshot_matches_scroll() {
            return;
        }
        let colors = self.colors;
        let scroll = self.scroll;
        let previous_history = self.snapshot.history;
        // Read before the snapshot: the snapshot is at least this new.
        let generation = self.terminal.generation();
        let take = |t: &pty::Emulator| snapshot(t, &colors, scroll);
        let mut snap = match self.terminal.try_with_term(take) {
            Some(s) => s,
            // The I/O thread is parsing: wait for its chunk only when there is something new to show.
            None if changed || self.snapshot.rows.is_empty() => {
                self.terminal.with_term_blocking(take)
            }
            None => {
                // Draw again next frame with what is new by then.
                self.terminal.mark_dirty();
                return;
            }
        };
        snap.generation = generation;
        // Keep the view where it was while output scrolls under a scrolled-up view.
        if self.scroll > 0 && snap.history > previous_history {
            self.scroll = (self.scroll + snap.history - previous_history).min(snap.history);
        }
        self.snapshot = snap;
        if let Some((at, generation)) = self.pending_key
            && self.snapshot.generation > generation
        {
            self.pending_key = None;
            self.key_latencies.push(at.elapsed());
        }
    }

    fn snapshot_matches_scroll(&self) -> bool {
        self.snapshot.top == -(self.scroll as i32)
    }
}

/// The visible rows of `t` scrolled `scroll` lines up.
fn snapshot(t: &pty::Emulator, colors: &Colors, scroll: usize) -> Snapshot {
    let grid = t.grid();
    let history = grid.history_size();
    let scroll = scroll.min(history);
    let rows = grid.screen_lines();
    let cols = grid.columns();
    let top = -(scroll as i32);
    let overrides = t.colors();
    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let line = Line(top + r as i32);
        let row = &grid[line];
        let mut cells = Vec::with_capacity(cols);
        for c in 0..cols {
            let cell = &row[Column(c)];
            let mut fg = colors.color(&cell.fg, overrides, true);
            let mut bg = colors.color(&cell.bg, overrides, false);
            let flags = cell.flags;
            if flags.contains(Flags::BOLD)
                && let Color::Named(n) = cell.fg
                && (n as usize) < 8
            {
                // Bold in one of the eight colors is its bright twin (xterm).
                fg = colors.palette[n as usize + 8];
            }
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg.a *= 0.7;
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let ch = if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                ' '
            } else {
                cell.c
            };
            cells.push(CellSnap {
                c: if ch == '\0' || ch == '\t' { ' ' } else { ch },
                fg,
                bg: (bg != colors.bg).then_some(bg),
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strike: flags.contains(Flags::STRIKEOUT),
            });
        }
        out.push(cells);
    }
    let content = t.renderable_content();
    let cp = content.cursor.point;
    let cursor_row = cp.line.0 - top;
    let cursor = (content.cursor.shape != CursorShape::Hidden
        && (0..rows as i32).contains(&cursor_row))
    .then_some((cursor_row as usize, cp.column.0, content.cursor.shape));
    Snapshot {
        rows: out,
        cursor,
        mode: Some(*t.mode()),
        history,
        top,
        generation: 0,
    }
}

/// Find `query` (case-insensitive) in `terminal`'s scrollback and screen: (absolute line, column range).
pub fn find_in(terminal: &Terminal, query: &str) -> Vec<(usize, Range<usize>)> {
    let (lines, _) = terminal.all_lines();
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.to_lowercase().chars().collect();
        if chars.len() < q.len() {
            continue;
        }
        let mut c = 0;
        while c + q.len() <= chars.len() {
            if chars[c..c + q.len()] == q[..] {
                out.push((i, c..c + q.len()));
                c += q.len();
            } else {
                c += 1;
            }
        }
    }
    out
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let exited = self.terminal.exited();
        let flashing = self.flashing();
        let find = self.find.as_ref().map(|f| {
            let focused = f.focus.as_ref().is_some_and(|h| h.is_focused(window));
            let count = match (f.current, f.matches.len()) {
                (_, 0) if !f.query.is_empty() => "No results".to_owned(),
                (Some(i), n) => format!("{} of {n}", i + 1),
                _ => String::new(),
            };
            let mut bar = div()
                .id("terminal-find")
                .debug_selector(|| "terminal-find".into())
                .absolute()
                .top_1()
                .right_4()
                .flex()
                .items_center()
                .gap_1()
                .p_1()
                .bg(t.popup_background)
                .border_1()
                .border_color(t.popup_border)
                .text_size(t.typography.ui)
                .child(eludite_ui::text_box(
                    "terminal-find-box",
                    &f.query,
                    "Find",
                    focused,
                    &t,
                ))
                .child(div().text_color(t.text_muted).child(count))
                .child(
                    eludite_ui::icon_button("terminal-find-prev", "\u{2191}", &t)
                        .on_click(cx.listener(|v, _, _, cx| v.find_next(true, cx))),
                )
                .child(
                    eludite_ui::icon_button("terminal-find-next", "\u{2193}", &t)
                        .on_click(cx.listener(|v, _, _, cx| v.find_next(false, cx))),
                )
                .child(
                    eludite_ui::icon_button("terminal-find-close", "\u{2715}", &t)
                        .on_click(cx.listener(|v, _, window, cx| v.close_find(window, cx))),
                );
            if let Some(h) = &f.focus {
                bar = bar.track_focus(h).on_key_down(cx.listener(
                    |v, e: &KeyDownEvent, window, cx| {
                        v.find_key(&e.keystroke, window, cx);
                        cx.stop_propagation();
                    },
                ));
            }
            bar
        });
        let exit_line = exited.map(|code| {
            let text = match code {
                Some(c) => format!("[Process exited with code {c}]"),
                None => "[Process exited]".to_owned(),
            };
            div()
                .id("terminal-exit")
                .debug_selector(|| "terminal-exit".into())
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(24.))
                .bg(t.panel)
                .border_t_1()
                .border_color(t.border)
                .text_size(t.typography.ui)
                .text_color(t.text_muted)
                .child(text)
                .child(
                    eludite_ui::push_button("terminal-restart", "Restart", true, true, &t)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(TerminalViewEvent::Restart))),
                )
        });
        let view = cx.entity();
        div()
            .id("terminal-view")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(t.background)
            .border_2()
            .border_color(if flashing {
                Hsla::from(t.accent)
            } else {
                gpui::transparent_black()
            })
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, window, cx| {
                if v.key_down(&e.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(GridElement { view }),
            )
            .children(exit_line)
            .children(find)
    }
}

/// Draws a [`TerminalView`]'s grid.
struct GridElement {
    view: Entity<TerminalView>,
}

impl IntoElement for GridElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What prepaint prepared for paint.
struct Prepared {
    rows: Vec<ShapedRow>,
    cell: gpui::Size<Pixels>,
    cursor: Option<(usize, usize, CursorShape)>,
    selection: Vec<(usize, Range<usize>)>,
    matches: Vec<(usize, Range<usize>, bool)>,
    colors: Colors,
    focused: bool,
    started: Instant,
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepared;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepared {
        let started = Instant::now();
        let (family, font_size) = {
            let v = self.view.read(cx);
            (v.settings.font_family.clone(), v.settings.font_size)
        };
        let font = gpui::font(family);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let line_height = (font_size * 1.25).round();
        let cols = ((bounds.size.width / cell_width).floor() as usize).max(2);
        let rows = ((bounds.size.height / line_height).floor() as usize).max(1);
        let focused = self.view.read(cx).focus.is_focused(window);
        self.view.update(cx, |v, _| {
            v.layout = Layout {
                origin: bounds.origin,
                cell: size(cell_width, line_height),
                cols,
                rows,
            };
            v.refresh(cols, rows);
        });
        let v = self.view.read(cx);
        let colors = v.colors;
        let top_abs = v.snapshot.history.saturating_sub(v.scroll);
        let mut shaped = Vec::with_capacity(v.snapshot.rows.len());
        for row in &v.snapshot.rows {
            let mut text = String::with_capacity(row.len());
            let mut runs: Vec<TextRun> = Vec::new();
            let mut backgrounds: Vec<(Range<usize>, Rgba)> = Vec::new();
            for (i, c) in row.iter().enumerate() {
                let start = text.len();
                text.push(c.c);
                let len = text.len() - start;
                let run_font = Font {
                    weight: if c.bold {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    style: if c.italic {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                    ..font.clone()
                };
                let color: Hsla = c.fg.into();
                let underline = c.underline.then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                });
                let strike = c.strike.then(|| gpui::StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color),
                });
                match runs.last_mut() {
                    Some(r)
                        if r.font == run_font
                            && r.color == color
                            && r.underline == underline
                            && r.strikethrough == strike =>
                    {
                        r.len += len
                    }
                    _ => runs.push(TextRun {
                        len,
                        font: run_font,
                        color,
                        background_color: None,
                        underline,
                        strikethrough: strike,
                    }),
                }
                if let Some(bg) = c.bg {
                    match backgrounds.last_mut() {
                        Some((r, b)) if *b == bg && r.end == i => r.end = i + 1,
                        _ => backgrounds.push((i..i + 1, bg)),
                    }
                }
            }
            let line = text_system.shape_line(text.into(), font_size, &runs, Some(cell_width));
            shaped.push((line, backgrounds));
        }
        // The selection and the find matches in visible rows.
        let rows_n = v.snapshot.rows.len();
        let visible = |abs: usize| abs.checked_sub(top_abs).filter(|r| *r < rows_n);
        let mut selection = Vec::new();
        if let Some((a, b)) = v.selection {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            for abs in a.line..=b.line {
                if let Some(r) = visible(abs) {
                    let start = if abs == a.line { a.column } else { 0 };
                    let end = if abs == b.line { b.column + 1 } else { cols };
                    selection.push((r, start..end));
                }
            }
        }
        let mut matches = Vec::new();
        if let Some(f) = &v.find {
            for (i, (abs, range)) in f.matches.iter().enumerate() {
                if let Some(r) = visible(*abs) {
                    matches.push((r, range.clone(), f.current == Some(i)));
                }
            }
        }
        Prepared {
            rows: shaped,
            cell: size(cell_width, line_height),
            cursor: v.snapshot.cursor,
            selection,
            matches,
            colors,
            focused,
            started,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepared,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cell = state.cell;
        let at = |row: usize, col: usize| {
            point(
                bounds.origin.x + cell.width * col as f32,
                bounds.origin.y + cell.height * row as f32,
            )
        };
        window.paint_quad(fill(bounds, Hsla::from(state.colors.bg)));
        for (row, (_, backgrounds)) in state.rows.iter().enumerate() {
            for (range, color) in backgrounds {
                window.paint_quad(fill(
                    Bounds::new(
                        at(row, range.start),
                        size(cell.width * range.len() as f32, cell.height),
                    ),
                    Hsla::from(*color),
                ));
            }
        }
        for (row, range, current) in &state.matches {
            let mut c = state.colors.find;
            if *current {
                c.a = 0.9;
            }
            window.paint_quad(fill(
                Bounds::new(
                    at(*row, range.start),
                    size(cell.width * range.len() as f32, cell.height),
                ),
                Hsla::from(c),
            ));
        }
        for (row, range) in &state.selection {
            window.paint_quad(fill(
                Bounds::new(
                    at(*row, range.start),
                    size(
                        cell.width * range.end.saturating_sub(range.start) as f32,
                        cell.height,
                    ),
                ),
                Hsla::from(state.colors.selection),
            ));
        }
        for (row, (line, _)) in state.rows.iter().enumerate() {
            let _ = line.paint(at(row, 0), cell.height, TextAlign::Left, None, window, cx);
        }
        if let Some((row, col, shape)) = state.cursor {
            let origin = at(row, col);
            let color = Hsla::from(state.colors.cursor);
            let quad = match (shape, state.focused) {
                (CursorShape::Beam, _) => {
                    fill(Bounds::new(origin, size(px(2.), cell.height)), color)
                }
                (CursorShape::Underline, _) => fill(
                    Bounds::new(
                        point(origin.x, origin.y + cell.height - px(2.)),
                        size(cell.width, px(2.)),
                    ),
                    color,
                ),
                (_, true) => fill(Bounds::new(origin, cell), color.opacity(0.6)),
                (_, false) => {
                    gpui::outline(Bounds::new(origin, cell), color, gpui::BorderStyle::Solid)
                }
            };
            window.paint_quad(quad);
        }
        let elapsed = state.started.elapsed();
        self.view.update(cx, |v, _| {
            if v.frame_times.len() >= 100_000 {
                v.frame_times.drain(..50_000);
            }
            v.frame_times.push(elapsed);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_colors_follow_xterm() {
        let p = palette(&Theme::vs_dark());
        assert_eq!(indexed(1, &p), p[1]);
        assert_eq!(indexed(16, &p), gpui::rgb(0x000000));
        assert_eq!(indexed(231, &p), gpui::rgb(0xFFFFFF));
        assert_eq!(indexed(196, &p), gpui::rgb(0xFF0000));
        assert_eq!(indexed(232, &p), gpui::rgb(0x080808));
        assert_eq!(indexed(255, &p), gpui::rgb(0xEEEEEE));
        assert_ne!(palette(&Theme::vs_light())[7], p[7]);
    }
}
