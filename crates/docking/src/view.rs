//! [`DockHost`]: the GPUI view that draws a [`DockController`]'s layout and
//! turns pointer input into `eludite.view.*` commands.
//!
//! - Drag a tool window's title bar or tab. While dragging, docking guides
//!   appear on the left, right and bottom of the document area; drop on one to
//!   dock there (`eludite.view.dock {side}`). Drop on another group to tab into
//!   it (`{tab_with}`). Drop anywhere else to float (`eludite.view.float`).
//! - Title bar buttons: Float, Auto Hide, Close. A floating window has Dock and
//!   Close; closing its OS window closes (hides) its tool windows, as in VS.
//! - Auto-hidden windows sit on edge strips; hovering or clicking one slides it
//!   out (`eludite.view.show`); clicking elsewhere slides it back in; Pin docks
//!   it again (`eludite.view.dock`).
//!
//! - Drag the splitter between a dock and the document area to resize the
//!   dock, or the one between two groups on an edge to resize them. The drag
//!   previews live; releasing commits one `eludite.view.resize`.
//!
//! Every one of those goes through the command bus. The view re-renders when
//! the controller reports a change, whoever made it (an agent included), and
//! hands the layout to the [`LayoutWriter`] thread to save.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use eludite_commands::{CommandError, CommandRegistry, view, workspace};
use eludite_ui::elements::TabStyle;
use eludite_ui::{RunCommand, SHELL_CONTEXT, Theme, icon_button, tab, vertical_label};
use futures::StreamExt as _;
use gpui::{
    AnyElement, App, Bounds as PxBounds, Context, DragMoveEvent, Entity, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowHandle, WindowOptions, canvas, div, point, prelude::*, px, size,
};
use serde_json::{Value, json};

use crate::controller::{DockController, Snapshot};
use crate::model::{
    DockLayout, DockSide, DocumentTab, Group, MIN_DOCK_SIZE, MIN_GROUP_SHARE, Place,
};
use crate::persist::LayoutWriter;

/// Draws a tool window's body. Brief 0008 bodies are empty titled panels;
/// later briefs supply real ones.
pub type ToolBody = Rc<dyn Fn(&str, &Theme) -> AnyElement>;
/// Draws the active document's body.
pub type DocumentBody = Rc<dyn Fn(&DocumentTab, &Theme) -> AnyElement>;

/// Drag payload for a splitter: a dock's edge (`group: None`) or the splitter
/// after group `group` on that edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DraggedSplitter {
    pub side: DockSide,
    pub group: Option<usize>,
}

/// What a splitter drag would commit, drawn while it lasts.
#[derive(Debug, Clone, PartialEq)]
enum ResizePreview {
    Dock(f32),
    Shares(Vec<f32>),
}

/// The splitter being dragged and its preview (set on mouse down, cleared
/// on release).
#[derive(Debug, Clone, PartialEq)]
struct Resizing {
    splitter: DraggedSplitter,
    /// Where the mouse went down; sizes change by the distance from here.
    start: gpui::Point<Pixels>,
    preview: Option<ResizePreview>,
}

/// A splitter drag shows no ghost.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Smallest the document area stays when a dock grows, logical pixels.
const MIN_CENTER: f32 = 200.;
const SPLITTER_PX: f32 = 4.;

/// Drag payload for a tool window tab or title bar.
#[derive(Clone, Debug)]
pub struct DraggedTool {
    pub id: String,
    pub title: SharedString,
}

struct DragGhost {
    title: SharedString,
    theme: Theme,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .bg(self.theme.accent)
            .text_color(self.theme.text_on_accent)
            .text_size(self.theme.typography.ui)
            .child(self.title.clone())
    }
}

/// Where a layout is saved.
#[derive(Clone, Debug)]
pub struct Persistence {
    pub path: PathBuf,
    pub writer: LayoutWriter,
}

/// Timing and element bounds for the harness (`eludite --bench-drag`). Off by
/// default; costs nothing when unset.
#[derive(Default, Debug)]
pub struct RenderProbe {
    /// Bounds of probed elements by debug selector (tabs, headers, guides,
    /// documents), from the last frame.
    pub bounds: HashMap<String, PxBounds<Pixels>>,
    /// Start of each `DockHost::render`.
    pub renders: Vec<Instant>,
    /// When GPUI ran a callback deferred from each render: after the frame
    /// was presented.
    pub presents: Vec<Instant>,
    /// Whether the docking guides were drawn in that render.
    pub guides_visible: Vec<bool>,
}

pub type Probe = Rc<RefCell<RenderProbe>>;

pub struct DockHost {
    controller: DockController,
    commands: Arc<CommandRegistry>,
    theme: Theme,
    snap: Snapshot,
    tool_body: ToolBody,
    document_body: DocumentBody,
    floating_windows: HashMap<u32, WindowHandle<FloatingView>>,
    persistence: Option<Persistence>,
    saved: Option<DockLayout>,
    probe: Option<Probe>,
    last_error: Option<String>,
    resizing: Option<Resizing>,
    _watch: Task<()>,
}

impl DockHost {
    pub fn new(
        controller: DockController,
        commands: Arc<CommandRegistry>,
        theme: Theme,
        tool_body: ToolBody,
        document_body: DocumentBody,
        persistence: Option<Persistence>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut changes = controller.subscribe();
        let watch = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            while changes.next().await.is_some() {
                // Coalesce a burst of changes into one refresh.
                while changes.try_recv().is_ok() {}
                if this.update(cx, |host, cx| host.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let snap = controller.snapshot();
        Self {
            saved: Some(snap.layout.clone()),
            snap,
            controller,
            commands,
            theme,
            tool_body,
            document_body,
            floating_windows: HashMap::new(),
            persistence,
            probe: None,
            last_error: None,
            resizing: None,
            _watch: watch,
        }
    }

    pub fn controller(&self) -> &DockController {
        &self.controller
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snap
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn set_probe(&mut self, probe: Option<Probe>) {
        self.probe = probe;
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn floating_window_count(&self) -> usize {
        self.floating_windows.len()
    }

    /// Floating group ids and their OS windows.
    pub fn floating_windows(&self) -> Vec<(u32, gpui::AnyWindowHandle)> {
        let mut v: Vec<_> = self
            .floating_windows
            .iter()
            .map(|(gid, h)| (*gid, (*h).into()))
            .collect();
        v.sort_by_key(|(gid, _)| *gid);
        v
    }

    pub fn persistence(&self) -> Option<&Persistence> {
        self.persistence.as_ref()
    }

    /// Invoke a command on the bus. Errors are kept for the status bar and
    /// logged; the layout is unchanged.
    pub fn invoke(&mut self, command: &str, args: Value) -> Result<Value, CommandError> {
        let result = self.commands.invoke(command, args);
        match &result {
            Ok(_) => self.last_error = None,
            Err(e) => {
                eprintln!("eludite: {command}: {e}");
                self.last_error = Some(e.to_string());
            }
        }
        result
    }

    /// Pull the controller's state after a change: re-render, save if the
    /// persisted layout changed, and reconcile floating OS windows.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let snap = self.controller.snapshot();
        if snap.revision == self.snap.revision {
            return;
        }
        self.snap = snap;
        if self.saved.as_ref() != Some(&self.snap.layout) {
            if let Some(p) = &self.persistence {
                p.writer.save(p.path.clone(), &self.snap.layout);
            }
            self.saved = Some(self.snap.layout.clone());
        }
        let this = cx.entity().downgrade();
        // Opening a window renders it, and its first render reads this entity,
        // so it must happen outside this update.
        cx.defer(move |cx| {
            if let Some(this) = this.upgrade() {
                DockHost::sync_floating_windows(&this, cx);
            }
        });
        cx.notify();
    }

    /// Write the current layout now (app exit). Resolves when written.
    pub fn flush(&self) -> Option<futures::channel::oneshot::Receiver<()>> {
        let p = self.persistence.as_ref()?;
        p.writer.save(p.path.clone(), &self.controller.layout());
        Some(p.writer.flush())
    }

    /// Open an OS window per floating group and close windows whose group is
    /// gone. Call outside any `DockHost` update (for example after opening the
    /// main window, to restore floating windows from a saved layout).
    pub fn sync_floating_windows(host: &Entity<DockHost>, cx: &mut App) {
        let (missing, stale) = {
            let h = host.read(cx);
            let missing: Vec<(u32, crate::model::Bounds, String)> = h
                .snap
                .layout
                .floating
                .iter()
                .filter(|f| !h.floating_windows.contains_key(&f.group.id))
                .map(|f| {
                    let title = f
                        .group
                        .active_id()
                        .map(|id| h.snap.registry.title(id).to_owned())
                        .unwrap_or_default();
                    (f.group.id, f.bounds, title)
                })
                .collect();
            let stale: Vec<u32> = h
                .floating_windows
                .keys()
                .copied()
                .filter(|gid| !h.snap.layout.floating.iter().any(|f| f.group.id == *gid))
                .collect();
            (missing, stale)
        };
        for gid in stale {
            if let Some(handle) = host.update(cx, |h, _| h.floating_windows.remove(&gid)) {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }
        for (gid, b, title) in missing {
            let weak = host.downgrade();
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(PxBounds::new(
                    point(px(b.x), px(b.y)),
                    size(px(b.width), px(b.height)),
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                app_id: Some("eludite".into()),
                ..Default::default()
            };
            let opened = cx.open_window(options, |window, cx| {
                let weak2 = weak.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    // Closing a floating tool window closes its tool windows (VS).
                    if let Some(host) = weak2.upgrade() {
                        host.update(cx, |h, _| {
                            h.floating_windows.remove(&gid);
                            let tabs = h
                                .snap
                                .layout
                                .group(gid)
                                .map(|g| g.tabs.clone())
                                .unwrap_or_default();
                            for id in tabs {
                                let _ = h.invoke(view::HIDE, json!({ "id": id }));
                            }
                        });
                    }
                    true
                });
                cx.new(|cx| FloatingView::new(weak, gid, cx))
            });
            match opened {
                Ok(handle) => host.update(cx, |h, _| {
                    h.floating_windows.insert(gid, handle);
                }),
                Err(e) => eprintln!("eludite: failed to open a floating window: {e:#}"),
            }
        }
    }

    fn probe_bounds(&self, key: String) -> Option<AnyElement> {
        probe_canvas(&self.probe, key)
    }

    fn render_documents(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let docs = &self.snap.layout.documents;
        let tabs = docs.tabs.iter().map(|d| {
            let id = d.id.clone();
            let style = TabStyle {
                active: docs.active.as_deref() == Some(d.id.as_str()),
                preview: d.preview,
                pinned: d.pinned,
            };
            let sel = format!("doc-tab-{}", d.id);
            let dirty = self.snap.dirty.contains(&d.id);
            let title = if dirty {
                format!("{}*", d.title)
            } else {
                d.title.clone()
            };
            let close_id = d.id.clone();
            // Closing goes through `eludite.file.close` (the shell asks about unsaved changes first).
            let close = icon_button(
                SharedString::from(format!("doc-close-{}", d.id)),
                "\u{00D7}",
                &t,
            )
            .debug_selector({
                let s = format!("doc-close-{}", d.id);
                move || s
            })
            .ml_1()
            .on_click(cx.listener(move |_, _, window, cx| {
                cx.stop_propagation();
                window.dispatch_action(
                    Box::new(RunCommand::new(
                        workspace::FILE_CLOSE,
                        json!({ "path": close_id }),
                    )),
                    cx,
                );
            }));
            // The file's source control glyph, after the title (brief 0040).
            let badge = self.snap.badges.get(&d.id).map(|(glyph, color)| {
                div()
                    .flex_none()
                    .pr_1()
                    .text_color(gpui::rgb(*color))
                    .child(glyph.clone())
            });
            let sel_badge = format!("doc-badge-{}", d.id);
            tab(&t, title, style)
                .children(badge.map(|b| b.debug_selector(move || sel_badge)))
                .child(close)
                .id(SharedString::from(sel.clone()))
                .debug_selector({
                    let s = sel.clone();
                    move || s
                })
                .relative()
                .children(self.probe_bounds(sel))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, _| {
                    let _ = this.invoke(view::SHOW, json!({ "id": id }));
                }))
        });
        let body = match docs.active_tab() {
            Some(d) => (self.document_body)(d, &t),
            None => div().into_any_element(),
        };
        div()
            .id("documents")
            .debug_selector(|| "documents".into())
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .bg(t.background)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.controller.close_flyout();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .h(t.typography.tab_height)
                    .bg(t.chrome)
                    .border_b_2()
                    .border_color(t.accent)
                    .children(tabs),
            )
            .child(
                // The body fills the area; a document view (the editor) brings its own margins.
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_color(t.text)
                    .text_size(t.typography.body)
                    .child(body),
            )
            .children(self.probe_bounds("documents".into()))
    }

    /// Docking guides, drawn over the document area during a drag.
    fn render_guides(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let guide = |side: DockSide, label: &'static str| {
            let sel = format!("guide-{}", side.name());
            let el = div()
                .id(SharedString::from(sel.clone()))
                .debug_selector({
                    let sel = sel.clone();
                    move || sel
                })
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.guide.alpha(0.25))
                .border_2()
                .border_color(t.guide)
                .text_color(t.text_on_accent)
                .text_size(t.typography.ui)
                .drag_over::<DraggedTool>(move |s, _, _, _| s.bg(t.guide.alpha(0.6)))
                .on_drop(cx.listener(move |this, d: &DraggedTool, _, _| {
                    let _ = this.invoke(view::DOCK, json!({ "id": d.id, "side": side.name() }));
                }))
                .child(label)
                .children(self.probe_bounds(sel));
            match side {
                DockSide::Left => el.left_2().top(px(64.)).bottom(px(64.)).w(px(64.)),
                DockSide::Right => el.right_2().top(px(64.)).bottom(px(64.)).w(px(64.)),
                DockSide::Bottom => el.bottom_2().left(px(96.)).right(px(96.)).h(px(44.)),
            }
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(guide(DockSide::Left, "Dock Left"))
            .child(guide(DockSide::Right, "Dock Right"))
            .child(guide(DockSide::Bottom, "Dock Bottom"))
            .into_any_element()
    }

    fn render_dock(&self, side: DockSide, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dock = self.snap.layout.dock(side);
        if dock.groups.is_empty() {
            return None;
        }
        let t = self.theme;
        let host = cx.entity().downgrade();
        let (size, shares) = self.dock_preview(side);
        let base = div().flex().flex_none().bg(t.chrome).p(px(2.));
        let base = match side {
            DockSide::Bottom => base.flex_row().h(px(size)),
            _ => base.flex_col().w(px(size)),
        };
        let last = dock.groups.len() - 1;
        let groups = dock.groups.iter().enumerate().flat_map(|(ix, g)| {
            let group = render_group(
                &self.snap,
                &t,
                g,
                false,
                &host,
                &self.tool_body,
                &self.probe,
            )
            .flex_basis(px(0.))
            .flex_grow(shares[ix].max(0.01))
            .flex_shrink(1.);
            let group = match side {
                DockSide::Bottom => group.min_w(px(MIN_DOCK_SIZE)),
                _ => group.min_h(px(MIN_DOCK_SIZE)),
            };
            let splitter = (ix < last).then(|| {
                self.splitter(
                    DraggedSplitter {
                        side,
                        group: Some(ix),
                    },
                    cx,
                )
            });
            std::iter::once(group.into_any_element()).chain(splitter)
        });
        Some(
            base.children(groups)
                .on_drag_move(cx.listener(
                    move |this, e: &DragMoveEvent<DraggedSplitter>, window, cx| {
                        let d = *e.drag(cx);
                        if d.side == side {
                            this.preview_resize(d, e.bounds, e.event.position, window);
                            cx.notify();
                        }
                    },
                ))
                .into_any_element(),
        )
    }

    /// A dock's size and its groups' shares, with the drag preview applied.
    fn dock_preview(&self, side: DockSide) -> (f32, Vec<f32>) {
        let dock = self.snap.layout.dock(side);
        let mut size = dock.size;
        let mut shares = dock.shares();
        if let Some(r) = &self.resizing
            && r.splitter.side == side
        {
            match &r.preview {
                Some(ResizePreview::Dock(s)) => size = *s,
                Some(ResizePreview::Shares(s)) => shares = s.clone(),
                None => {}
            }
        }
        (size, shares)
    }

    /// The splitter element for `d`, 4 px thick across the dock's axis.
    fn splitter(&self, d: DraggedSplitter, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let sel = match d.group {
            None => format!("splitter-{}", d.side.name()),
            Some(ix) => format!("splitter-{}-{ix}", d.side.name()),
        };
        let horizontal = matches!(d.side, DockSide::Bottom) ^ d.group.is_some();
        let el = div()
            .id(SharedString::from(sel.clone()))
            .debug_selector({
                let s = sel.clone();
                move || s
            })
            .relative()
            .flex_none()
            .bg(t.chrome)
            .hover(|s| s.bg(t.guide))
            .children(probe_canvas(&self.probe, sel));
        // A horizontal bar (between things stacked vertically) resizes rows.
        let el = if horizontal {
            el.h(px(SPLITTER_PX)).w_full().cursor_row_resize()
        } else {
            el.w(px(SPLITTER_PX)).h_full().cursor_col_resize()
        };
        el.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                this.resizing = Some(Resizing {
                    splitter: d,
                    start: e.position,
                    preview: None,
                });
                cx.notify();
            }),
        )
        .on_drag(d, |_, _, _, cx| cx.new(|_| NoGhost))
        .into_any_element()
    }

    /// Update the preview of splitter `d` from the mouse at `pos`, given the
    /// dock's `bounds`.
    fn preview_resize(
        &mut self,
        d: DraggedSplitter,
        bounds: PxBounds<Pixels>,
        pos: gpui::Point<Pixels>,
        window: &Window,
    ) {
        let Some(r) = self.resizing.as_mut() else {
            return;
        };
        if r.splitter != d {
            return;
        }
        let moved = pos - r.start;
        let dock = self.snap.layout.dock(d.side);
        let preview = match d.group {
            None => {
                let viewport = window.viewport_size();
                let (delta, limit) = match d.side {
                    DockSide::Left => (moved.x, viewport.width),
                    DockSide::Right => (-moved.x, viewport.width),
                    DockSide::Bottom => (-moved.y, viewport.height),
                };
                let max = (f32::from(limit) - MIN_CENTER).max(MIN_DOCK_SIZE);
                ResizePreview::Dock((dock.size + f32::from(delta)).clamp(MIN_DOCK_SIZE, max))
            }
            Some(ix) => {
                let (delta, extent) = match d.side {
                    DockSide::Bottom => (moved.x, bounds.size.width),
                    _ => (moved.y, bounds.size.height),
                };
                let fraction = f32::from(delta) / f32::from(extent).max(1.);
                let mut shares = dock.shares();
                if ix + 1 < shares.len() {
                    let pair = shares[ix] + shares[ix + 1];
                    let first = (shares[ix] + fraction).clamp(
                        MIN_GROUP_SHARE,
                        (pair - MIN_GROUP_SHARE).max(MIN_GROUP_SHARE),
                    );
                    shares[ix] = first;
                    shares[ix + 1] = pair - first;
                }
                ResizePreview::Shares(shares)
            }
        };
        r.preview = Some(preview);
    }

    /// Commit the splitter drag's preview through the bus and end the drag.
    fn commit_resize(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.resizing.take() else {
            return;
        };
        cx.notify();
        let side = r.splitter.side.name();
        match (r.splitter.group, r.preview) {
            (None, Some(ResizePreview::Dock(size))) => {
                let _ = self.invoke(view::RESIZE, json!({ "side": side, "size": size }));
            }
            (Some(ix), Some(ResizePreview::Shares(shares))) => {
                let dock = self.snap.layout.dock(r.splitter.side);
                if let (Some(g), Some(share)) = (dock.groups.get(ix), shares.get(ix))
                    && let Some(id) = g.active_id()
                {
                    let _ = self.invoke(view::RESIZE, json!({ "id": id, "share": share }));
                }
            }
            _ => {}
        }
    }

    /// The splitter between a dock with groups and the document area.
    fn edge_splitter(&self, side: DockSide, cx: &mut Context<Self>) -> Option<AnyElement> {
        (!self.snap.layout.dock(side).groups.is_empty())
            .then(|| self.splitter(DraggedSplitter { side, group: None }, cx))
    }

    fn render_strip(&self, side: DockSide, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ids = &self.snap.layout.dock(side).auto_hidden;
        if ids.is_empty() {
            return None;
        }
        let t = self.theme;
        let strip = div()
            .flex()
            .flex_none()
            .gap_2()
            .bg(t.chrome)
            .text_size(t.typography.small)
            .text_color(t.chrome_text);
        let strip = match side {
            DockSide::Bottom => strip.flex_row().h(px(22.)).px_2(),
            _ => strip.flex_col().w(px(22.)).py_2(),
        };
        Some(
            strip
                .children(ids.iter().map(|id| {
                    let title = self.snap.registry.title(id).to_owned();
                    let open = self.snap.flyout.as_deref() == Some(id.as_str());
                    let label: AnyElement = match side {
                        // VS's vertical tabs: the title rotated 90 degrees
                        // clockwise, reading top to bottom, on both edges.
                        DockSide::Left | DockSide::Right => div()
                            .flex()
                            .justify_center()
                            .py_1()
                            .child(vertical_label(title.clone()))
                            .into_any_element(),
                        DockSide::Bottom => div().child(title.clone()).into_any_element(),
                    };
                    let (hover_id, click_id) = (id.clone(), id.clone());
                    let sel = format!("strip-{id}");
                    let el = div()
                        .id(SharedString::from(sel.clone()))
                        .debug_selector({
                            let s = sel.clone();
                            move || s
                        })
                        .relative()
                        .children(self.probe_bounds(sel))
                        .p_0p5()
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.text))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, _| {
                            if *hovered && this.snap.flyout.as_deref() != Some(hover_id.as_str()) {
                                let _ = this.invoke(view::SHOW, json!({ "id": hover_id }));
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, _| {
                            // Hovering usually opened it already; a click keeps
                            // it open (VS), clicking elsewhere closes it.
                            if this.snap.flyout.as_deref() != Some(click_id.as_str()) {
                                let _ = this.invoke(view::SHOW, json!({ "id": click_id }));
                            }
                        }))
                        .child(label);
                    let el = match side {
                        DockSide::Bottom => el.border_t_2(),
                        DockSide::Left => el.border_l_2(),
                        DockSide::Right => el.border_r_2(),
                    };
                    el.border_color(if open { t.accent } else { t.border })
                }))
                .into_any_element(),
        )
    }

    fn render_flyout(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.snap.flyout.clone()?;
        let Some(Place::AutoHidden { side }) = self.snap.layout.find(&id) else {
            return None;
        };
        let t = self.theme;
        let size_px = px(self.snap.layout.dock(side).size);
        let title = self.snap.registry.title(&id).to_owned();
        let (pin_id, close_id) = (id.clone(), id.clone());
        let header = header_bar(&t, title, true)
            .child(
                icon_button("flyout-pin", "Pin", &t)
                    .debug_selector(|| "flyout-pin".into())
                    .relative()
                    .children(self.probe_bounds("flyout-pin".into()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let _ = this.invoke(view::DOCK, json!({ "id": pin_id }));
                    })),
            )
            .child(
                icon_button("flyout-close", "\u{00D7}", &t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        let _ = this.invoke(view::HIDE, json!({ "id": close_id }));
                    },
                )),
            );
        let panel = div()
            .id("flyout")
            .debug_selector(|| "flyout".into())
            .absolute()
            .flex()
            .flex_col()
            .bg(t.panel)
            .border_1()
            .border_color(t.accent)
            .shadow_lg()
            .occlude()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .p_2()
                    .text_color(t.text)
                    .child((self.tool_body)(&id, &t)),
            );
        let panel = match side {
            DockSide::Left => panel.left_0().top_0().bottom_0().w(size_px),
            DockSide::Right => panel.right_0().top_0().bottom_0().w(size_px),
            DockSide::Bottom => panel.left_0().right_0().bottom_0().h(size_px),
        };
        Some(panel.into_any_element())
    }
}

impl Render for DockHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = cx.has_active_drag() && self.resizing.is_none();
        if let Some(probe) = &self.probe {
            let mut p = probe.borrow_mut();
            p.renders.push(Instant::now());
            p.guides_visible.push(dragging);
            let probe = probe.clone();
            // GPUI runs effects deferred from a render after the frame is
            // presented (brief 0001 report, section 3.1).
            cx.defer(move |_| probe.borrow_mut().presents.push(Instant::now()));
        }
        let t = self.theme;

        let mut center = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(self.render_documents(cx));
        center = center.children(self.edge_splitter(DockSide::Bottom, cx));
        center = center.children(self.render_dock(DockSide::Bottom, cx));
        if dragging {
            center = center.child(self.render_guides(cx));
        }
        center = center.children(self.render_flyout(cx));

        let row = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .children(self.render_strip(DockSide::Left, cx))
            .children(self.render_dock(DockSide::Left, cx))
            .children(self.edge_splitter(DockSide::Left, cx))
            .child(center)
            .children(self.edge_splitter(DockSide::Right, cx))
            .children(self.render_dock(DockSide::Right, cx))
            .children(self.render_strip(DockSide::Right, cx));

        let _ = window;
        div()
            .id("dock-host")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .bg(t.chrome)
            .text_color(t.text)
            .text_size(t.typography.ui)
            // Dropping a tool window where there is no guide and no group
            // floats it (VS behavior).
            .on_drop(cx.listener(|this, d: &DraggedTool, window, _| {
                let m = window.mouse_position();
                let wb = window.bounds();
                let x = f32::from(wb.origin.x + m.x) - 40.;
                let y = f32::from(wb.origin.y + m.y) - 12.;
                let _ = this.invoke(
                    view::FLOAT,
                    json!({ "id": d.id, "bounds": {"x": x, "y": y, "width": 360, "height": 420} }),
                );
            }))
            .on_drop(cx.listener(|this, _: &DraggedSplitter, _, cx| this.commit_resize(cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.commit_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.commit_resize(cx)),
            )
            .child(row)
            .children(self.render_strip(DockSide::Bottom, cx))
    }
}

/// When probing, an invisible canvas filling its (relative) parent that
/// records the parent's bounds under `key`.
fn probe_canvas(probe: &Option<Probe>, key: String) -> Option<AnyElement> {
    let probe = probe.clone()?;
    Some(
        canvas(
            move |bounds, _, _| {
                probe.borrow_mut().bounds.insert(key, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element(),
    )
}

fn header_bar(t: &Theme, title: String, active: bool) -> gpui::Div {
    let (bg, fg) = if active {
        (t.panel_header_active, t.panel_header_active_text)
    } else {
        (t.panel_header, t.panel_header_text)
    };
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap_1()
        .h(t.typography.panel_header_height)
        .px_2()
        .bg(bg)
        .text_color(fg)
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(title),
        )
}

fn ghost(d: &DraggedTool, theme: Theme, cx: &mut App) -> Entity<DragGhost> {
    let title = d.title.clone();
    cx.new(|_| DragGhost { title, theme })
}

/// A tool window group: title bar with buttons, the active window's body, and
/// a tab strip at the bottom when it has several tabs (VS puts tool window
/// tabs at the bottom).
fn render_group(
    snap: &Snapshot,
    t: &Theme,
    g: &Group,
    floating: bool,
    host: &WeakEntity<DockHost>,
    tool_body: &ToolBody,
    probe: &Option<Probe>,
) -> gpui::Stateful<gpui::Div> {
    let t = *t;
    let active = g.active_id().unwrap_or_default().to_owned();
    let title = snap.registry.title(&active).to_owned();
    let is_active_tool = snap.active_tool.as_deref() == Some(active.as_str());
    let invoke = |host: &WeakEntity<DockHost>, cx: &mut App, cmd: &'static str, args: Value| {
        if let Some(h) = host.upgrade() {
            h.update(cx, |h, _| {
                let _ = h.invoke(cmd, args);
            });
        }
    };
    // Floating windows record bounds relative to their own OS window.
    let prefix = if floating { "floating/" } else { "" };
    let probe_el = |key: String| probe_canvas(probe, format!("{prefix}{key}"));

    let button = |name: &str, label: &'static str, cmd: &'static str| {
        let h = host.clone();
        let id = active.clone();
        let sel = format!("{name}-{active}");
        icon_button(SharedString::from(sel.clone()), label, &t)
            .debug_selector({
                let s = sel.clone();
                move || s
            })
            .relative()
            .children(probe_el(sel))
            .on_click(move |_, _, cx| {
                // The title bar under the button activates on click; do not.
                cx.stop_propagation();
                invoke(&h, cx, cmd, json!({ "id": id }))
            })
    };

    let head_sel = format!("head-{active}");
    let mut head = header_bar(&t, title.clone(), is_active_tool)
        .id(SharedString::from(head_sel.clone()))
        .debug_selector({
            let s = head_sel.clone();
            move || s
        })
        .relative()
        .cursor_grab()
        .on_click({
            let h = host.clone();
            let id = active.clone();
            move |_, _, cx| invoke(&h, cx, view::SHOW, json!({ "id": id }))
        });
    if !floating {
        head = head.on_drag(
            DraggedTool {
                id: active.clone(),
                title: title.clone().into(),
            },
            move |d, _, _, cx| ghost(d, t, cx),
        );
        head = head
            .child(button("float", "Float", view::FLOAT))
            .child(button("hide", "Auto Hide", view::AUTO_HIDE));
    } else {
        head = head.child(button("dock", "Dock", view::DOCK));
    }
    head = head
        .child(button("close", "\u{00D7}", view::HIDE))
        .children(probe_el(head_sel));

    let tabs = (g.tabs.len() > 1).then(|| {
        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(22.))
            .bg(t.chrome)
            .children(g.tabs.iter().enumerate().map(|(ix, id)| {
                let sel = format!("tab-{id}");
                let h = host.clone();
                let click_id = id.clone();
                let tab_title = snap.registry.title(id).to_owned();
                let mut el = tab(
                    &t,
                    tab_title.clone(),
                    TabStyle {
                        active: ix == g.active,
                        ..Default::default()
                    },
                )
                .id(SharedString::from(sel.clone()))
                .debug_selector({
                    let s = sel.clone();
                    move || s
                })
                .relative()
                .cursor_pointer()
                .on_click(move |_, _, cx| invoke(&h, cx, view::SHOW, json!({ "id": click_id })));
                if !floating {
                    el = el.on_drag(
                        DraggedTool {
                            id: id.clone(),
                            title: tab_title.into(),
                        },
                        move |d, _, _, cx| ghost(d, t, cx),
                    );
                }
                el.children(probe_el(sel))
            }))
    });

    let drop_host = host.clone();
    let target = active.clone();
    let group_probe = probe_el(format!("group-{active}"));
    div()
        .id(SharedString::from(format!("group-{}", g.id)))
        .debug_selector(move || format!("group-{active}"))
        .relative()
        .flex()
        .flex_col()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .drag_over::<DraggedTool>(move |s, _, _, _| s.border_color(t.guide).bg(t.menu_hover))
        .on_drop(move |d: &DraggedTool, _, cx| {
            invoke(
                &drop_host,
                cx,
                view::DOCK,
                json!({ "id": d.id, "tab_with": target }),
            )
        })
        .child(head)
        .child(
            div()
                .flex_1()
                .p_2()
                .overflow_hidden()
                .text_color(t.text)
                .child(tool_body(g.active_id().unwrap_or_default(), &t)),
        )
        .children(tabs)
        .children(group_probe)
}

/// Root view of a floating tool window's OS window.
pub struct FloatingView {
    host: WeakEntity<DockHost>,
    gid: u32,
    _observe: Option<gpui::Subscription>,
}

impl FloatingView {
    fn new(host: WeakEntity<DockHost>, gid: u32, cx: &mut Context<Self>) -> Self {
        let sub = host
            .upgrade()
            .map(|h| cx.observe(&h, |_, _, cx| cx.notify()));
        Self {
            host,
            gid,
            _observe: sub,
        }
    }

    pub fn group_id(&self) -> u32 {
        self.gid
    }
}

impl Render for FloatingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(host) = self.host.upgrade() else {
            return div().into_any_element();
        };
        let h = host.read(cx);
        let t = h.theme;
        let body = match h.snap.layout.group(self.gid) {
            Some(g) => render_group(&h.snap, &t, g, true, &self.host, &h.tool_body, &h.probe)
                .size_full()
                .into_any_element(),
            None => div().into_any_element(),
        };
        let weak = self.host.clone();
        div()
            .key_context(SHELL_CONTEXT)
            .on_action(move |a: &RunCommand, _, cx| {
                if let Some(h) = weak.upgrade() {
                    h.update(cx, |h, _| {
                        let _ = h.invoke(&a.command, a.args.clone());
                    });
                }
            })
            .size_full()
            .bg(t.panel)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(body)
            .into_any_element()
    }
}
