//! The root view: VS-style docking around a document area, plus floating
//! tool windows in their own OS windows.
//!
//! Interactions (all mutate `Layout`, then save it to JSON if a path is set):
//! - Drag a tool window's title bar or tab. While dragging, docking guides
//!   appear on the left, right and bottom edges of the document area; drop on
//!   one to dock there. Drop on another tool window group to tab into it.
//!   Drop anywhere else to float.
//! - "Float" / "Auto Hide" buttons on each group header; "Dock" on a floating
//!   window (closing a floating window also docks it back).
//! - Auto-hidden windows sit on the edge strips; click one to slide it out,
//!   "Pin" to dock it again.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FocusHandle, Focusable, IntoElement, MouseButton,
    ParentElement, Render, Rgba, SharedString, Styled, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions, div, point, prelude::*, px, rgb, size,
};

use crate::layout::{Group, Layout, Side, title};
use crate::text_view::TextView;

pub struct Colors;
impl Colors {
    pub const BG: u32 = 0x1E1E1E;
    pub const PANEL: u32 = 0x252526;
    pub const HEADER: u32 = 0x2D2D30;
    pub const ACCENT: u32 = 0x007ACC;
    pub const BORDER: u32 = 0x3F3F46;
    pub const TEXT: u32 = 0xD4D4D4;
    pub const MUTED: u32 = 0x9D9D9D;
    pub const GUIDE: u32 = 0x1C97EA;
}

fn c(hex: u32) -> Rgba {
    rgb(hex)
}

type ShellUpdate = Box<dyn FnOnce(&mut Shell, &mut Context<Shell>)>;

/// Drag payload for a tool window.
#[derive(Clone, Debug)]
pub struct DraggedTool {
    pub id: String,
}

struct DragGhost(SharedString);
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .bg(c(Colors::ACCENT))
            .text_color(gpui::white())
            .text_size(px(12.))
            .child(self.0.clone())
    }
}

pub struct Shell {
    pub layout: Layout,
    pub layout_path: Option<PathBuf>,
    pub text_view: Entity<TextView>,
    pub documents: Vec<SharedString>,
    pub active_document: usize,
    /// Auto-hidden window currently slid out.
    pub flyout: Option<String>,
    floating_windows: HashMap<u32, WindowHandle<FloatingView>>,
    focus: FocusHandle,
}

impl Shell {
    pub fn new(
        layout: Layout,
        layout_path: Option<PathBuf>,
        text_view: Entity<TextView>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            layout,
            layout_path,
            text_view,
            documents: vec!["Generated100k.cs".into(), "Welcome".into()],
            active_document: 0,
            flyout: None,
            floating_windows: HashMap::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Apply a layout change, persist it, and reconcile floating OS windows.
    pub fn mutate(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Layout)) {
        f(&mut self.layout);
        debug_assert!(self.layout.is_consistent());
        if let Some(path) = &self.layout_path
            && let Err(e) = self.layout.save(path)
        {
            eprintln!("failed to save layout to {}: {e}", path.display());
        }
        if self.flyout.as_ref().is_some_and(|id| {
            !matches!(
                self.layout.find(id),
                Some(crate::layout::Place::AutoHidden { .. })
            )
        }) {
            self.flyout = None;
        }
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Some(this) = this.upgrade() {
                Shell::sync_floating_windows(&this, cx);
            }
        });
        cx.notify();
    }

    /// Open an OS window per floating group, close windows whose group is gone.
    /// Runs outside any `Shell` update, because a new window renders (and so
    /// reads the shell) while it is being opened.
    pub fn sync_floating_windows(shell: &Entity<Shell>, cx: &mut App) {
        let (live, stale) = {
            let s = shell.read(cx);
            let live: Vec<(u32, [f32; 4], String)> = s
                .layout
                .floating
                .iter()
                .filter(|f| !s.floating_windows.contains_key(&f.group.id))
                .map(|f| {
                    let t = f
                        .group
                        .active_id()
                        .map(title)
                        .unwrap_or("Tool Window")
                        .to_owned();
                    (f.group.id, f.bounds, t)
                })
                .collect();
            let stale: Vec<u32> = s
                .floating_windows
                .keys()
                .copied()
                .filter(|gid| !s.layout.floating.iter().any(|f| f.group.id == *gid))
                .collect();
            (live, stale)
        };
        for gid in stale {
            let h = shell.update(cx, |s, _| s.floating_windows.remove(&gid));
            if let Some(h) = h {
                let _ = h.update(cx, |_, window, _| window.remove_window());
            }
        }
        for (gid, b, title_text) in live {
            let weak = shell.downgrade();
            let opts = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(b[0]), px(b[1])),
                    size(px(b[2]), px(b[3])),
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title_text.into()),
                    ..Default::default()
                }),
                app_id: Some("niello-spike".into()),
                ..Default::default()
            };
            let opened = cx.open_window(opts, |window, cx| {
                let weak2 = weak.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    // Closing a floating tool window docks it back home.
                    if let Some(s) = weak2.upgrade() {
                        s.update(cx, |s, cx| {
                            s.floating_windows.remove(&gid);
                            s.mutate(cx, |l| l.dock_floating(gid));
                        });
                    }
                    true
                });
                cx.new(|cx| FloatingView::new(weak, gid, cx))
            });
            match opened {
                Ok(h) => shell.update(cx, |s, _| {
                    s.floating_windows.insert(gid, h);
                }),
                Err(e) => eprintln!("failed to open floating window: {e:#}"),
            }
        }
    }

    pub fn floating_window_count(&self) -> usize {
        self.floating_windows.len()
    }

    fn drop_float(&mut self, id: &str, window: &Window, cx: &mut Context<Self>) {
        let m = window.mouse_position();
        let wb = window.bounds();
        let (x, y) = (
            f32::from(wb.origin.x + m.x) - 40.,
            f32::from(wb.origin.y + m.y) - 20.,
        );
        let id = id.to_owned();
        self.mutate(cx, |l| {
            l.float(&id, [x, y, 360., 420.]);
        });
    }

    fn render_group(
        &self,
        g: &Group,
        floating: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        render_group(g, floating, cx.entity().downgrade())
    }

    fn render_dock(&self, side: Side, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dock = self.layout.dock(side);
        if dock.groups.is_empty() {
            return None;
        }
        let mut d = div().flex().flex_none().bg(c(Colors::PANEL));
        d = match side {
            Side::Bottom => d.flex_row().h(px(dock.size)).border_t_1(),
            _ => d.flex_col().w(px(dock.size)),
        }
        .border_color(c(Colors::BORDER));
        for g in &dock.groups {
            d = d.child(self.render_group(g, false, cx).flex_1());
        }
        Some(d.into_any_element())
    }

    fn render_strip(&self, side: Side, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ids = &self.layout.dock(side).auto_hidden;
        if ids.is_empty() {
            return None;
        }
        let mut strip = div()
            .flex()
            .flex_none()
            .gap_1()
            .bg(c(Colors::HEADER))
            .text_size(px(11.))
            .text_color(c(Colors::MUTED));
        strip = match side {
            Side::Bottom => strip.flex_row().h(px(22.)).px_2(),
            _ => strip.flex_col().w(px(84.)).py_2(),
        };
        for id in ids {
            let id2 = id.clone();
            let open = self.flyout.as_deref() == Some(id.as_str());
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("strip-{id}")))
                    .debug_selector(|| format!("strip-{id}"))
                    .px_1()
                    .py_0p5()
                    .border_b_2()
                    .border_color(c(if open { Colors::ACCENT } else { Colors::BORDER }))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c(Colors::TEXT)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.flyout = if this.flyout.as_deref() == Some(id2.as_str()) {
                            None
                        } else {
                            Some(id2.clone())
                        };
                        cx.notify();
                    }))
                    .child(title(id).to_owned()),
            );
        }
        Some(strip.into_any_element())
    }

    fn render_flyout(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.flyout.clone()?;
        let side = match self.layout.find(&id)? {
            crate::layout::Place::AutoHidden { side } => side,
            _ => return None,
        };
        let size_px = px(self.layout.dock(side).size);
        let id2 = id.clone();
        let panel = div()
            .id("flyout")
            .absolute()
            .flex()
            .flex_col()
            .bg(c(Colors::PANEL))
            .border_1()
            .border_color(c(Colors::ACCENT))
            .shadow_lg()
            .occlude()
            .child(
                header(title(&id).to_owned(), true).child(
                    button("flyout-pin", "Pin")
                        .debug_selector(|| "flyout-pin".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = id2.clone();
                            this.mutate(cx, |l| {
                                l.pin(&id);
                            });
                        })),
                ),
            )
            .child(div().flex_1().p_2().child(tool_body(&id)));
        let panel = match side {
            Side::Left => panel.left_0().top_0().bottom_0().w(size_px),
            Side::Right => panel.right_0().top_0().bottom_0().w(size_px),
            Side::Bottom => panel.left_0().right_0().bottom_0().h(size_px),
        };
        Some(panel.into_any_element())
    }

    fn render_documents(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(24.))
            .bg(c(Colors::PANEL))
            .border_b_1()
            .border_color(c(Colors::BORDER))
            .text_size(px(12.))
            .children(self.documents.iter().enumerate().map(|(ix, t)| {
                let tab = div()
                    .id(("doc-tab", ix))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_r_1()
                    .border_color(c(Colors::BORDER))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_document = ix;
                        cx.notify();
                    }))
                    .child(t.clone());
                if ix == self.active_document {
                    tab.bg(c(Colors::ACCENT)).text_color(gpui::white())
                } else {
                    tab.text_color(c(Colors::MUTED))
                }
            }));
        let body: AnyElement = if self.active_document == 0 {
            self.text_view.clone().into_any_element()
        } else {
            div()
                .p_4()
                .child("Niello spike 0001. Drag tool window title bars or tabs to dock, tab or float them.")
                .into_any_element()
        };
        div()
            .id("documents")
            .debug_selector(|| "documents".into())
            .flex()
            .flex_col()
            .flex_1()
            .relative()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.flyout.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(tabs)
            .child(div().flex_1().overflow_hidden().child(body))
    }

    /// Docking guides shown during a tool window drag.
    fn render_guides(&self, cx: &mut Context<Self>) -> AnyElement {
        let guide = |side: Side, label: &'static str| {
            let el = div()
                .id(SharedString::from(format!("guide-{label}")))
                .debug_selector(|| format!("guide-{label}"))
                .absolute()
                .flex()
                .items_center()
                .justify_center()
                .bg(c(Colors::GUIDE).alpha(0.25))
                .border_2()
                .border_color(c(Colors::GUIDE))
                .text_color(gpui::white())
                .text_size(px(12.))
                .drag_over::<DraggedTool>(|s, _, _, _| s.bg(c(Colors::GUIDE).alpha(0.6)))
                .on_drop(cx.listener(move |this, d: &DraggedTool, _, cx| {
                    let id = d.id.clone();
                    this.mutate(cx, |l| l.dock_to(&id, side));
                }))
                .child(label);
            match side {
                Side::Left => el.left_2().top(px(80.)).bottom(px(80.)).w(px(64.)),
                Side::Right => el.right_2().top(px(80.)).bottom(px(80.)).w(px(64.)),
                Side::Bottom => el.bottom_2().left(px(120.)).right(px(120.)).h(px(48.)),
            }
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(guide(Side::Left, "Dock Left"))
            .child(guide(Side::Right, "Dock Right"))
            .child(guide(Side::Bottom, "Dock Bottom"))
            .into_any_element()
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = cx.has_active_drag();
        let mut center = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(self.render_documents(cx));
        if dragging {
            center = center.child(self.render_guides(cx));
        }
        center = center.children(self.render_flyout(cx));

        let mut column = div().flex().flex_col().flex_1().min_w_0().child(center);
        column = column.children(self.render_dock(Side::Bottom, cx));
        column = column.children(self.render_strip(Side::Bottom, cx));

        let row = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .children(self.render_strip(Side::Left, cx))
            .children(self.render_dock(Side::Left, cx))
            .child(column)
            .children(self.render_dock(Side::Right, cx))
            .children(self.render_strip(Side::Right, cx));

        let (lf, crlf, cr) = self.text_view.read(cx).buffer.ending_counts();
        let status = format!(
            "Ready   |   {} lines   LF {lf}  CRLF {crlf}  CR {cr}",
            self.text_view.read(cx).buffer.len()
        );
        div()
            .id("shell")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(c(Colors::BG))
            .text_color(c(Colors::TEXT))
            .text_size(px(12.))
            // Fallback drop target: dropping a tool window anywhere that is not
            // a guide or a group floats it (VS behavior).
            .on_drop(cx.listener(|this, d: &DraggedTool, window, cx| {
                let id = d.id.clone();
                this.drop_float(&id, window, cx);
            }))
            .child(row)
            .child(
                div()
                    .flex_none()
                    .h(px(22.))
                    .px_2()
                    .flex()
                    .items_center()
                    .bg(c(Colors::ACCENT))
                    .text_color(gpui::white())
                    .text_size(px(11.))
                    .child(status),
            )
    }
}

fn header(text: String, active: bool) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap_1()
        .h(px(22.))
        .px_2()
        .bg(c(if active {
            Colors::ACCENT
        } else {
            Colors::HEADER
        }))
        .text_color(gpui::white())
        .child(div().flex_1().child(text))
}

fn button(id: impl Into<SharedString>, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .px_1()
        .text_size(px(11.))
        .cursor_pointer()
        .hover(|s| s.bg(c(Colors::BORDER)))
        .child(label)
}

/// Placeholder content per tool window.
pub fn tool_body(id: &str) -> AnyElement {
    let lines: &[&str] = match id {
        "solution_explorer" => &[
            "Solution 'Generated' (1 of 1 project)",
            "  Generated (net10.0)",
            "    Dependencies",
            "    Generated100k.cs",
            "    Program.cs",
        ],
        "properties" => &[
            "Build Action    Compile",
            "Copy to Output  Do not copy",
            "File Name       Generated100k.cs",
        ],
        "error_list" => &["0 Errors   0 Warnings   0 Messages"],
        "output" => &[
            "Show output from: Build",
            "Build started...",
            "========== Build: 0 succeeded ==========",
        ],
        "toolbox" => &["General", "  There are no usable controls in this group."],
        "team_explorer" => &["Git Changes", "  main  ↑0 ↓0", "  No changes"],
        _ => &[""],
    };
    div()
        .flex()
        .flex_col()
        .text_color(c(Colors::TEXT))
        .children(lines.iter().map(|l| div().child(*l)))
        .into_any_element()
}

/// A tab group: header (title, buttons), body, and a bottom tab strip when it has several tabs.
pub fn render_group(
    g: &Group,
    floating: bool,
    shell: WeakEntity<Shell>,
) -> gpui::Stateful<gpui::Div> {
    let active = g.active_id().unwrap_or_default().to_owned();
    let gid = g.id;
    let with_shell = move |shell: &WeakEntity<Shell>, cx: &mut App, f: ShellUpdate| {
        if let Some(s) = shell.upgrade() {
            s.update(cx, |s, cx| f(s, cx));
        }
    };

    let mut head = header(title(&active).to_owned(), false)
        .id(SharedString::from(format!("group-head-{gid}")))
        .debug_selector(|| format!("head-{active}"))
        .cursor_grab();
    if !floating {
        head = head.on_drag(DraggedTool { id: active.clone() }, |d, _, _, cx| {
            cx.new(|_| DragGhost(title(&d.id).to_owned().into()))
        });
    }
    let mut head = head;
    if floating {
        let s = shell.clone();
        head = head.child(
            button(format!("dock-{gid}"), "Dock").on_click(move |_, _, cx| {
                with_shell(
                    &s,
                    cx,
                    Box::new(move |sh, cx| sh.mutate(cx, |l| l.dock_floating(gid))),
                );
            }),
        );
    } else {
        let (s1, s2) = (shell.clone(), shell.clone());
        let (a1, a2) = (active.clone(), active.clone());
        head = head
            .child(
                button(format!("float-{gid}"), "Float")
                    .debug_selector(|| format!("float-{a1}"))
                    .on_click(move |_, window, cx| {
                        let b = window.bounds();
                        let (x, y) = (f32::from(b.origin.x) + 200., f32::from(b.origin.y) + 120.);
                        let a = a1.clone();
                        with_shell(
                            &s1,
                            cx,
                            Box::new(move |sh, cx| {
                                sh.mutate(cx, |l| {
                                    l.float(&a, [x, y, 360., 420.]);
                                })
                            }),
                        );
                    }),
            )
            .child(
                button(format!("hide-{gid}"), "Auto Hide")
                    .debug_selector(|| format!("hide-{a2}"))
                    .on_click(move |_, _, cx| {
                        let a = a2.clone();
                        with_shell(
                            &s2,
                            cx,
                            Box::new(move |sh, cx| {
                                sh.mutate(cx, |l| {
                                    l.auto_hide(&a);
                                })
                            }),
                        );
                    }),
            );
    }

    let tabs = (g.tabs.len() > 1).then(|| {
        div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(22.))
            .border_t_1()
            .border_color(c(Colors::BORDER))
            .children(g.tabs.iter().enumerate().map(|(ix, id)| {
                let s = shell.clone();
                let id2 = id.clone();
                let mut tab = div()
                    .id(SharedString::from(format!("tab-{gid}-{id}")))
                    .debug_selector(|| format!("tab-{id}"))
                    .flex()
                    .items_center()
                    .px_2()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let id = id2.clone();
                        with_shell(
                            &s,
                            cx,
                            Box::new(move |sh, cx| {
                                sh.mutate(cx, |l| {
                                    l.activate(&id);
                                })
                            }),
                        );
                    })
                    .child(title(id).to_owned());
                if !floating {
                    tab = tab.on_drag(DraggedTool { id: id.clone() }, |d, _, _, cx| {
                        cx.new(|_| DragGhost(title(&d.id).to_owned().into()))
                    });
                }
                if ix == g.active {
                    tab.bg(c(Colors::BG)).text_color(c(Colors::ACCENT))
                } else {
                    tab.text_color(c(Colors::MUTED))
                }
            }))
    });

    let s = shell.clone();
    div()
        .id(SharedString::from(format!("group-{gid}")))
        .debug_selector(|| format!("group-{active}"))
        .flex()
        .flex_col()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .bg(c(Colors::PANEL))
        .border_1()
        .border_color(c(Colors::BORDER))
        .drag_over::<DraggedTool>(|st, _, _, _| {
            st.border_color(c(Colors::GUIDE)).bg(c(Colors::HEADER))
        })
        .on_drop(move |d: &DraggedTool, _, cx| {
            let id = d.id.clone();
            with_shell(
                &s,
                cx,
                Box::new(move |sh, cx| {
                    sh.mutate(cx, |l| {
                        l.tab_into(&id, gid);
                    })
                }),
            );
        })
        .child(head)
        .child(
            div()
                .flex_1()
                .p_2()
                .overflow_hidden()
                .child(tool_body(&active)),
        )
        .children(tabs)
}

/// Root view of a floating tool window's OS window.
pub struct FloatingView {
    shell: WeakEntity<Shell>,
    gid: u32,
    _observe: Option<gpui::Subscription>,
}

impl FloatingView {
    fn new(shell: WeakEntity<Shell>, gid: u32, cx: &mut Context<Self>) -> Self {
        let sub = shell
            .upgrade()
            .map(|s| cx.observe(&s, |_, _, cx| cx.notify()));
        Self {
            shell,
            gid,
            _observe: sub,
        }
    }
}

impl Render for FloatingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let group = self
            .shell
            .upgrade()
            .and_then(|s| s.read(cx).layout.group(self.gid).cloned());
        let body = match group {
            Some(g) => render_group(&g, true, self.shell.clone())
                .size_full()
                .into_any_element(),
            None => div().child("(docked)").into_any_element(),
        };
        div()
            .size_full()
            .bg(c(Colors::BG))
            .text_color(c(Colors::TEXT))
            .text_size(px(12.))
            .child(body)
    }
}
