//! The Git Repository window (Git > Manage Branches, Ctrl+0, Ctrl+R): Visual Studio's. Branches (local, remote),
//! tags, stashes and worktrees on the left; the history with its graph on the right (lanes computed by `eludite-git`,
//! `eludite.git.log` with `all`). A selected branch offers Checkout, Merge into Current Branch, Rebase Current Branch
//! onto and Delete; a selected commit Checkout, Cherry-Pick, New Branch from it and Reset (Keep Changes, Delete
//! Changes). New Branch takes a name. While a merge or rebase is stopped, Continue and Abort. Every button dispatches
//! its `eludite.git.*` command. The shell loads the data (off the UI thread) when the window asks for it.

use std::ops::Range;

use eludite_commands::git::{BranchesOutput, LogEntryOut, StashOut, WorktreeOut};
use eludite_ui::{RunCommand, Theme, push_button, text_box};
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, PathBuilder, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, canvas, div, point, px,
    rgb, uniform_list,
};
use serde_json::{Value, json};

use eludite_commands::git as cmds;

pub const ROW_HEIGHT: f32 = 22.;
/// Horizontal room per graph lane.
pub const LANE_WIDTH: f32 = 12.;

pub const NEW_BRANCH_BOX: &str = "git-repo-new-branch";
pub const NEW_BRANCH_CREATE: &str = "git-repo-new-branch-create";
pub const CHECKOUT: &str = "git-repo-checkout";
pub const MERGE: &str = "git-repo-merge";
pub const REBASE: &str = "git-repo-rebase";
pub const DELETE: &str = "git-repo-delete";
pub const CHERRY_PICK: &str = "git-repo-cherry-pick";
pub const RESET_KEEP: &str = "git-repo-reset-keep";
pub const RESET_DELETE: &str = "git-repo-reset-delete";
pub const CONTINUE: &str = "git-repo-continue";
pub const ABORT: &str = "git-repo-abort";

/// Selector of branch `name`'s row.
pub fn branch_selector(name: &str) -> String {
    format!("git-branch-{name}")
}

/// Selector of commit row `ix`.
pub fn commit_selector(ix: usize) -> String {
    format!("git-commit-{ix}")
}

/// What is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Branch { name: String, remote: bool },
    Commit(String),
}

/// What the window tells the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryEvent {
    /// Load the branches, log and worktrees (the window is shown and its data is stale).
    Load,
}

pub struct GitRepositoryWindow {
    theme: Theme,
    pub branches: Option<BranchesOutput>,
    pub log: Vec<LogEntryOut>,
    pub stashes: Vec<StashOut>,
    pub worktrees: Vec<WorktreeOut>,
    /// The operation stopped at a conflict (`merge`, `rebase`, `cherry_pick`).
    pub operation: Option<String>,
    pub stale: bool,
    pub loading: bool,
    pub selected: Option<Selection>,
    new_branch: String,
    new_branch_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    pub message: Option<String>,
}

impl EventEmitter<RepositoryEvent> for GitRepositoryWindow {}

impl GitRepositoryWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            branches: None,
            log: Vec::new(),
            stashes: Vec::new(),
            worktrees: Vec::new(),
            operation: None,
            stale: true,
            loading: false,
            selected: None,
            new_branch: String::new(),
            new_branch_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            message: None,
        }
    }

    /// The data changed (HEAD, the refs or the stashes moved): load again the next time the window is drawn.
    pub fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        cx.notify();
    }

    pub fn set_data(
        &mut self,
        branches: Option<BranchesOutput>,
        log: Vec<LogEntryOut>,
        stashes: Vec<StashOut>,
        worktrees: Vec<WorktreeOut>,
        cx: &mut Context<Self>,
    ) {
        self.branches = branches;
        self.log = log;
        self.stashes = stashes;
        self.worktrees = worktrees;
        self.loading = false;
        cx.notify();
    }

    /// Focus the New Branch box (Git > New Branch...).
    pub fn focus_new_branch(&self, window: &mut Window, cx: &mut App) {
        self.new_branch_focus.focus(window, cx);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new_branch_text(&self) -> &str {
        &self.new_branch
    }

    fn run(command: &str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    fn create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_branch.trim().to_owned();
        if name.is_empty() {
            return;
        }
        let mut args = json!({ "name": name, "create": true });
        if let Some(Selection::Commit(c)) = &self.selected {
            args["start_point"] = json!(c);
        } else if let Some(Selection::Branch { name, .. }) = &self.selected {
            args["start_point"] = json!(name);
        }
        self.new_branch.clear();
        Self::run(cmds::CHECKOUT, args, window, cx);
        cx.notify();
    }

    fn new_branch_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "backspace" => {
                self.new_branch.pop();
            }
            "enter" => return self.create_branch(window, cx),
            "escape" => self.new_branch.clear(),
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(|c| c.is_control() || c == ' ') => {
                        self.new_branch.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn select(&mut self, s: Selection, cx: &mut Context<Self>) {
        self.selected = Some(s);
        cx.notify();
    }

    fn branch_rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let mut out = Vec::new();
        let heading = |text: &str| {
            div()
                .px_1()
                .pt_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_muted)
                .child(text.to_owned())
                .into_any_element()
        };
        let Some(b) = &self.branches else {
            out.push(
                div()
                    .px_1()
                    .text_color(t.text_muted)
                    .child(if self.loading { "Loading\u{2026}" } else { "" })
                    .into_any_element(),
            );
            return out;
        };
        for (title, remote) in [("Branches", false), ("Remotes", true)] {
            let list: Vec<_> = b.branches.iter().filter(|x| x.remote == remote).collect();
            if list.is_empty() && remote {
                continue;
            }
            out.push(heading(title));
            for br in list {
                let sel = branch_selector(&br.name);
                let selected = matches!(&self.selected, Some(Selection::Branch { name, .. }) if *name == br.name);
                let s = Selection::Branch {
                    name: br.name.clone(),
                    remote,
                };
                let counts = match (br.ahead, br.behind) {
                    (Some(a), Some(b)) if a + b > 0 => format!(" \u{2191}{a} \u{2193}{b}"),
                    _ => String::new(),
                };
                let row = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel.clone())
                    .flex()
                    .flex_row()
                    .h(px(ROW_HEIGHT))
                    .items_center()
                    .pl(px(12.))
                    .gap_1()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when_bold(br.head)
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        this.select(s.clone(), cx);
                        if e.click_count() >= 2 {
                            // A double-click checks the branch out, as Visual Studio does.
                            let Selection::Branch { name, .. } = &s else {
                                return;
                            };
                            Self::run(cmds::CHECKOUT, json!({ "name": name }), window, cx);
                        }
                    }))
                    .child(br.name.clone())
                    .child(div().text_color(t.text_muted).child(counts));
                out.push(if selected {
                    row.bg(t.accent)
                        .text_color(t.text_on_accent)
                        .into_any_element()
                } else {
                    row.hover(|s| s.bg(t.menu_hover)).into_any_element()
                });
            }
        }
        if !b.tags.is_empty() {
            out.push(heading("Tags"));
            for tag in &b.tags {
                out.push(div().pl(px(12.)).child(tag.name.clone()).into_any_element());
            }
        }
        if !self.stashes.is_empty() {
            out.push(heading("Stashes"));
            for s in &self.stashes {
                out.push(
                    div()
                        .pl(px(12.))
                        .whitespace_nowrap()
                        .child(format!("stash@{{{}}}: {}", s.index, s.message))
                        .into_any_element(),
                );
            }
        }
        if self.worktrees.len() > 1 {
            out.push(heading("Worktrees"));
            for w in &self.worktrees {
                let sel = format!("git-worktree-{}", w.name);
                out.push(
                    div()
                        .id(SharedString::from(sel.clone()))
                        .debug_selector(move || sel.clone())
                        .pl(px(12.))
                        .whitespace_nowrap()
                        .child(format!(
                            "{} [{}] {}",
                            w.name,
                            w.branch.clone().unwrap_or_else(|| "detached".into()),
                            w.path
                        ))
                        .into_any_element(),
                );
            }
        }
        out
    }

    fn commit_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        range
            .filter_map(|ix| {
                let e = self.log.get(ix)?.clone();
                let above: Vec<[u32; 2]> = ix
                    .checked_sub(1)
                    .and_then(|p| self.log.get(p))
                    .map(|p| p.graph.edges.clone())
                    .unwrap_or_default();
                let width = e
                    .graph
                    .edges
                    .iter()
                    .chain(&above)
                    .flat_map(|[a, b]| [*a, *b])
                    .chain([e.graph.lane])
                    .max()
                    .unwrap_or(0)
                    + 1;
                let sel = commit_selector(ix);
                let selected =
                    matches!(&self.selected, Some(Selection::Commit(c)) if *c == e.commit);
                let commit = e.commit.clone();
                let graph = graph_cell(e.graph.lane, above, e.graph.edges.clone(), width);
                let refs = (!e.refs.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .px_1()
                        .border_1()
                        .border_color(t.accent)
                        .text_size(t.typography.small)
                        .child(e.refs.join(", "))
                });
                let row = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel.clone())
                    .flex()
                    .flex_row()
                    .h(px(ROW_HEIGHT))
                    .items_center()
                    .gap_2()
                    .pr_1()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(Selection::Commit(commit.clone()), cx)
                    }))
                    .child(graph)
                    .children(refs)
                    .child(div().flex_1().overflow_hidden().child(e.summary.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_color(t.text_muted)
                            .child(e.author.clone()),
                    )
                    .child(
                        div().flex_none().text_color(t.text_muted).child(
                            e.date
                                .clone()
                                .unwrap_or_default()
                                .chars()
                                .take(16)
                                .collect::<String>()
                                .replace('T', " "),
                        ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(t.text_muted)
                            .child(e.short.clone()),
                    );
                Some(if selected {
                    row.bg(t.accent)
                        .text_color(t.text_on_accent)
                        .into_any_element()
                } else {
                    row.hover(|s| s.bg(t.menu_hover)).into_any_element()
                })
            })
            .collect()
    }

    fn actions(&self) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let button =
            |sel: &'static str, label: &'static str, command: &'static str, args: Value| {
                push_button(sel, label, false, true, &t)
                    .on_click(move |_, window, cx| Self::run(command, args.clone(), window, cx))
                    .into_any_element()
            };
        let mut out = Vec::new();
        match self.operation.as_deref() {
            Some("rebase") => {
                out.push(button(
                    CONTINUE,
                    "Continue Rebase",
                    cmds::REBASE,
                    json!({ "action": "continue" }),
                ));
                out.push(button(
                    ABORT,
                    "Abort Rebase",
                    cmds::REBASE,
                    json!({ "action": "abort" }),
                ));
            }
            Some("merge" | "cherry_pick") => {
                out.push(button(
                    ABORT,
                    "Abort Merge",
                    cmds::MERGE,
                    json!({ "abort": true }),
                ));
            }
            _ => {}
        }
        match &self.selected {
            Some(Selection::Branch { name, remote }) => {
                out.push(button(
                    CHECKOUT,
                    "Checkout",
                    cmds::CHECKOUT,
                    json!({ "name": name }),
                ));
                out.push(button(
                    MERGE,
                    "Merge into Current",
                    cmds::MERGE,
                    json!({ "branch": name }),
                ));
                out.push(button(
                    REBASE,
                    "Rebase Current onto",
                    cmds::REBASE,
                    json!({ "onto": name }),
                ));
                if !remote {
                    out.push(button(
                        DELETE,
                        "Delete",
                        cmds::BRANCHES,
                        json!({ "action": "delete", "name": name }),
                    ));
                }
            }
            Some(Selection::Commit(c)) => {
                out.push(button(
                    CHECKOUT,
                    "Checkout",
                    cmds::CHECKOUT,
                    json!({ "name": c }),
                ));
                out.push(button(
                    CHERRY_PICK,
                    "Cherry-Pick",
                    cmds::CHERRY_PICK,
                    json!({ "commit": c }),
                ));
                out.push(button(
                    RESET_KEEP,
                    "Reset (Keep Changes)",
                    cmds::RESET,
                    json!({ "revision": c, "mode": "mixed" }),
                ));
                out.push(button(
                    RESET_DELETE,
                    "Reset (Delete Changes)",
                    cmds::RESET,
                    json!({ "revision": c, "mode": "hard" }),
                ));
            }
            None => {}
        }
        out
    }
}

trait Bold {
    fn when_bold(self, bold: bool) -> Self;
}

impl Bold for gpui::Stateful<gpui::Div> {
    fn when_bold(self, bold: bool) -> Self {
        if bold {
            self.font_weight(FontWeight::BOLD)
        } else {
            self
        }
    }
}

/// The colors of the graph's lanes, by lane.
const LANE_COLORS: [u32; 6] = [
    0x56_9C_D6, 0xC5_86_C0, 0x4E_C9_B0, 0xDC_DC_AA, 0xCE_91_78, 0x9C_DC_FE,
];

fn lane_x(bounds: &Bounds<Pixels>, lane: u32) -> Pixels {
    bounds.left() + px(6. + LANE_WIDTH * lane as f32)
}

/// One row's piece of the graph: the edges from the row above down to this row's center, this row's edges down to the
/// next row, and the commit's node.
fn graph_cell(
    lane: u32,
    above: Vec<[u32; 2]>,
    below: Vec<[u32; 2]>,
    width: u32,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let top = bounds.top();
            let mid = bounds.center().y;
            let bottom = bounds.bottom();
            let line = |window: &mut Window,
                        from: gpui::Point<Pixels>,
                        to: gpui::Point<Pixels>,
                        lane: u32| {
                let mut p = PathBuilder::stroke(px(1.5));
                p.move_to(from);
                p.line_to(to);
                if let Ok(path) = p.build() {
                    window.paint_path(path, rgb(LANE_COLORS[lane as usize % LANE_COLORS.len()]));
                }
            };
            for [a, b] in &above {
                // The lower half of the row above's edge: from the boundary (halfway across) to this row's center.
                let half = (lane_x(&bounds, *a) + lane_x(&bounds, *b)) / 2.;
                line(
                    window,
                    point(half, top),
                    point(lane_x(&bounds, *b), mid),
                    *b,
                );
            }
            for [a, b] in &below {
                let half = (lane_x(&bounds, *a) + lane_x(&bounds, *b)) / 2.;
                line(
                    window,
                    point(lane_x(&bounds, *a), mid),
                    point(half, bottom),
                    *b,
                );
            }
            let c = point(lane_x(&bounds, lane), mid);
            let r = px(3.5);
            window.paint_quad(gpui::fill(
                Bounds::new(point(c.x - r, c.y - r), gpui::size(r * 2., r * 2.)),
                rgb(LANE_COLORS[lane as usize % LANE_COLORS.len()]),
            ));
        },
    )
    .flex_none()
    .w(px(6. + LANE_WIDTH * width as f32))
    .h_full()
}

impl Render for GitRepositoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        if self.stale && !self.loading {
            self.stale = false;
            self.loading = true;
            cx.defer_in(window, |_, _, cx| cx.emit(RepositoryEvent::Load));
        }
        let focused = self.new_branch_focus.is_focused(window);
        let new_branch = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                text_box(
                    NEW_BRANCH_BOX,
                    &self.new_branch,
                    "New branch name",
                    focused,
                    &t,
                )
                .track_focus(&self.new_branch_focus)
                .key_context("GitNewBranch")
                .on_key_down(cx.listener(Self::new_branch_key))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_branch_focus.focus(window, cx);
                    cx.notify();
                })),
            )
            .child(
                push_button(NEW_BRANCH_CREATE, "New Branch", false, true, &t)
                    .on_click(cx.listener(|this, _, window, cx| this.create_branch(window, cx))),
            );
        let count = self.log.len();
        div()
            .id("git-repository")
            .debug_selector(|| "git-repository".into())
            .size_full()
            .flex()
            .flex_col()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(new_branch)
                    .children(self.actions())
                    .children(self.message.clone().map(|m| {
                        div()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(m)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("git-repository-branches")
                            .flex()
                            .flex_col()
                            .flex_none()
                            .w(px(220.))
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(t.border)
                            .children(self.branch_rows(cx)),
                    )
                    .child(
                        uniform_list(
                            "git-repository-log",
                            count,
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                this.commit_rows(range, cx)
                            }),
                        )
                        .track_scroll(&self.scroll)
                        .flex_1()
                        .min_h_0(),
                    ),
            )
    }
}
