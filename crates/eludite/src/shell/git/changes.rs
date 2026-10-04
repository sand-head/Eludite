//! The Git Changes window (View > Git Changes, Ctrl+0, Ctrl+G): Visual Studio's. The branch with the outgoing and
//! incoming counts and Fetch, Pull, Push and Sync; the commit message box with Commit All (Commit Staged once
//! something is staged), Commit All and Push, Amend and Stash All; then Merge Changes (conflicted files), Staged
//! Changes and Changes, each file with its glyph and Stage, Unstage and Undo Changes; then the stashes with Apply, Pop
//! and Drop. A folder in no repository shows Create Git Repository. Every button dispatches its `eludite.git.*`
//! command ([`RunCommand`]), as the Git menu and agents run them; the window only draws what the status says.
//!
//! The list is virtualized (one uniform row height), so a thousand changed files cost what fits on screen.

use std::collections::HashSet;
use std::ops::Range;

use eludite_commands::git::StashOut;
use eludite_git::{ChangeKind, FileGlyph};
use eludite_ui::{RunCommand, Theme, check_box, icon_button, push_button, toggle_button};
use gpui::{
    App, Context, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div, px, rgb,
    uniform_list,
};
use serde_json::{Value, json};

use eludite_commands::git as cmds;

/// Row height of the list.
pub const ROW_HEIGHT: f32 = 22.;

/// Debug selectors (tests and the Xvfb driver).
pub const MESSAGE_BOX: &str = "git-changes-message";
pub const COMMIT: &str = "git-changes-commit";
pub const COMMIT_PUSH: &str = "git-changes-commit-push";
pub const AMEND: &str = "git-changes-amend";
pub const STASH_ALL: &str = "git-changes-stash";
pub const FETCH: &str = "git-changes-fetch";
pub const PULL: &str = "git-changes-pull";
pub const PUSH: &str = "git-changes-push";
pub const SYNC: &str = "git-changes-sync";
pub const BRANCH: &str = "git-changes-branch";
pub const CREATE: &str = "git-changes-create";
pub const HISTORY: &str = "git-changes-history";
pub const STAGE_ALL: &str = "git-changes-stage-all";
pub const UNSTAGE_ALL: &str = "git-changes-unstage-all";
pub const INFO: &str = "git-changes-info";
/// The warning line while `http.sslVerify` is false (brief 0045).
pub const SSL_WARNING: &str = "git-changes-ssl-warning";
/// Visual Studio's "Create a Pull Request" link, after a push and while the branch is ahead of its upstream (brief
/// 0046).
pub const PULL_REQUEST_LINK: &str = "git-changes-create-pull-request";
/// Its text.
pub const SSL_WARNING_TEXT: &str = "\u{26A0} http.sslVerify is false: the certificates of this repository's https remotes are not checked";

/// The groups of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Merge Changes: conflicted files.
    Merge,
    Staged,
    Changes,
    Stashes,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Merge => "Merge Changes",
            Group::Staged => "Staged Changes",
            Group::Changes => "Changes",
            Group::Stashes => "Stashes",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Group::Merge => "merge",
            Group::Staged => "staged",
            Group::Changes => "changes",
            Group::Stashes => "stashes",
        }
    }
}

/// Selector of the row of `path` in `group`.
pub fn file_selector(group: Group, path: &str) -> String {
    format!("git-file-{}-{path}", group.key())
}

/// Selector of a row's button (`stage`, `unstage`, `undo`) for `path`.
pub fn button_selector(action: &str, path: &str) -> String {
    format!("git-{action}-{path}")
}

/// Selector of stash `index`'s button (`apply`, `pop`, `drop`).
pub fn stash_selector(action: &str, index: u32) -> String {
    format!("git-stash-{action}-{index}")
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub glyph: FileGlyph,
    pub old_path: Option<String>,
}

/// What the window draws.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangesModel {
    /// `None` until the workspace is known; `Some(false)` in no repository.
    pub repository: Option<bool>,
    pub loading: bool,
    pub repo_name: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub operation: Option<String>,
    pub conflicted: Vec<FileRow>,
    pub staged: Vec<FileRow>,
    pub changes: Vec<FileRow>,
    pub stashes: Vec<StashOut>,
    /// A transfer in progress.
    pub progress: Option<String>,
    /// `http.sslVerify` is false: the warning line shows.
    pub ssl_verify_off: bool,
    pub generation: u64,
}

impl ChangesModel {
    /// The model of `status` at `generation`.
    pub fn from_status(status: &eludite_git::Status, generation: u64) -> Self {
        let glyph = |k: ChangeKind| match k {
            ChangeKind::Added => FileGlyph::Added,
            ChangeKind::Deleted => FileGlyph::Deleted,
            ChangeKind::Renamed => FileGlyph::Renamed,
            ChangeKind::Modified | ChangeKind::Typechange => FileGlyph::Modified,
        };
        let row = |c: &eludite_git::Change| FileRow {
            path: c.path.clone(),
            glyph: glyph(c.kind),
            old_path: c.old_path.clone(),
        };
        let mut changes: Vec<FileRow> = status.unstaged.iter().map(row).collect();
        changes.extend(status.untracked.iter().map(|p| FileRow {
            path: p.clone(),
            glyph: FileGlyph::Untracked,
            old_path: None,
        }));
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            repository: Some(true),
            loading: false,
            repo_name: status
                .workdir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            branch: status.branch.clone(),
            detached: status.detached,
            upstream: status.upstream.clone(),
            ahead: status.ahead,
            behind: status.behind,
            operation: status.operation.map(|o| o.as_str().to_owned()),
            conflicted: status
                .conflicted
                .iter()
                .map(|p| FileRow {
                    path: p.clone(),
                    glyph: FileGlyph::Conflicted,
                    old_path: None,
                })
                .collect(),
            staged: status.staged.iter().map(row).collect(),
            changes,
            stashes: Vec::new(),
            progress: None,
            ssl_verify_off: status.ssl_verify_off,
            generation,
        }
    }
}

/// A row of the virtualized list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header(Group, usize),
    File(Group, FileRow),
    Stash(StashOut),
}

/// What the window tells the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesEvent {
    /// The message box changed (the draft is kept per repository).
    Draft(String),
    /// Commit All and Push: `eludite.git.commit` with these arguments, then `eludite.git.push`.
    CommitAndPush(Value),
}

pub struct GitChanges {
    theme: Theme,
    pub model: ChangesModel,
    rows: Vec<Row>,
    message: String,
    amend: bool,
    focus: FocusHandle,
    collapsed: HashSet<Group>,
    selected: Option<String>,
    /// The last command's message (`true`: an error), shown under the message box.
    info: Option<(String, bool)>,
    /// Show the "Create a Pull Request" link (brief 0046).
    pull_request_link: bool,
    scroll: UniformListScrollHandle,
    /// Where the message box, the Commit button and the file rows were drawn, while `--bounds-out` probes (the Xvfb
    /// run clicks them).
    probe: Option<eludite_ui::BoundsMap>,
}

impl EventEmitter<ChangesEvent> for GitChanges {}

impl GitChanges {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            model: ChangesModel::default(),
            rows: Vec::new(),
            message: String::new(),
            amend: false,
            focus: cx.focus_handle(),
            collapsed: HashSet::new(),
            selected: None,
            info: None,
            pull_request_link: false,
            scroll: UniformListScrollHandle::new(),
            probe: None,
        }
    }

    pub fn set_probe(&mut self, probe: Option<eludite_ui::BoundsMap>) {
        self.probe = probe;
    }

    pub fn set_model(&mut self, model: ChangesModel, cx: &mut Context<Self>) {
        let stashes = std::mem::take(&mut self.model.stashes);
        let progress = self.model.progress.take();
        self.model = model;
        if self.model.stashes.is_empty() {
            self.model.stashes = stashes;
        }
        if self.model.progress.is_none() {
            self.model.progress = progress;
        }
        self.refresh_rows();
        cx.notify();
    }

    pub fn set_stashes(&mut self, stashes: Vec<StashOut>, cx: &mut Context<Self>) {
        self.model.stashes = stashes;
        self.refresh_rows();
        cx.notify();
    }

    pub fn set_progress(&mut self, progress: Option<String>, cx: &mut Context<Self>) {
        self.model.progress = progress;
        cx.notify();
    }

    /// The commit message box's text.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Replace the message box's text (a restored draft, or cleared after a commit).
    pub fn set_message(&mut self, text: String, cx: &mut Context<Self>) {
        self.message = text;
        cx.notify();
    }

    pub fn set_amend(&mut self, on: bool, cx: &mut Context<Self>) {
        self.amend = on;
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn info(&self) -> Option<&(String, bool)> {
        self.info.as_ref()
    }

    pub fn set_info(&mut self, info: Option<(String, bool)>, cx: &mut Context<Self>) {
        self.info = info;
        cx.notify();
    }

    /// Show or hide the "Create a Pull Request" link (brief 0046).
    pub fn set_pull_request_link(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.pull_request_link != show {
            self.pull_request_link = show;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pull_request_link(&self) -> bool {
        self.pull_request_link
    }

    /// The paths listed in `group`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn group(&self, group: Group) -> Vec<String> {
        let files = match group {
            Group::Merge => &self.model.conflicted,
            Group::Staged => &self.model.staged,
            Group::Changes => &self.model.changes,
            Group::Stashes => {
                return self
                    .model
                    .stashes
                    .iter()
                    .map(|s| s.message.clone())
                    .collect();
            }
        };
        files.iter().map(|f| f.path.clone()).collect()
    }

    fn refresh_rows(&mut self) {
        let mut rows = Vec::new();
        for (group, files) in [
            (Group::Merge, &self.model.conflicted),
            (Group::Staged, &self.model.staged),
            (Group::Changes, &self.model.changes),
        ] {
            if files.is_empty() && group != Group::Changes {
                continue;
            }
            rows.push(Row::Header(group, files.len()));
            if !self.collapsed.contains(&group) {
                rows.extend(files.iter().map(|f| Row::File(group, f.clone())));
            }
        }
        if !self.model.stashes.is_empty() {
            rows.push(Row::Header(Group::Stashes, self.model.stashes.len()));
            if !self.collapsed.contains(&Group::Stashes) {
                rows.extend(self.model.stashes.iter().cloned().map(Row::Stash));
            }
        }
        self.rows = rows;
    }

    fn run(command: &str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    /// The Commit button's command: Commit All when nothing is staged (Visual Studio's), else Commit Staged.
    fn commit_args(&self) -> Value {
        let mut args = json!({ "message": self.message.trim_end() });
        if self.amend {
            args["amend"] = json!(true);
        } else if self.model.staged.is_empty() {
            args["all"] = json!(true);
        }
        args
    }

    fn commit(&mut self, and_push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.message.trim().is_empty() && !self.amend {
            self.info = Some(("Enter a commit message".into(), true));
            cx.notify();
            return;
        }
        let args = self.commit_args();
        if and_push {
            // Commit All and Push: the shell runs eludite.git.commit, then eludite.git.push once it is made.
            cx.emit(ChangesEvent::CommitAndPush(args));
        } else {
            Self::run(cmds::COMMIT, args, window, cx);
        }
    }

    fn message_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control && k.key == "enter" {
            cx.stop_propagation();
            self.commit(false, window, cx);
            return;
        }
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "backspace" => {
                self.message.pop();
            }
            "enter" => self.message.push('\n'),
            "space" => self.message.push(' '),
            "escape" => return,
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
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        self.message.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.emit(ChangesEvent::Draft(self.message.clone()));
        cx.notify();
    }

    fn toggle(&mut self, group: Group, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&group) {
            self.collapsed.insert(group);
        }
        self.refresh_rows();
        cx.notify();
    }

    fn header(&self, group: Group, count: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let collapsed = self.collapsed.contains(&group);
        let action = match group {
            Group::Staged => Some((UNSTAGE_ALL, "\u{2212}", "Unstage All", cmds::UNSTAGE)),
            Group::Changes if count > 0 => Some((STAGE_ALL, "+", "Stage All", cmds::STAGE)),
            _ => None,
        };
        let sel = format!("git-changes-group-{}", group.key());
        div()
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel.clone())
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .px_1()
            .gap_1()
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(group, cx)))
            .child(if collapsed { "\u{25B7}" } else { "\u{25E2}" })
            .child(format!("{} ({count})", group.title()))
            .child(div().flex_1())
            .children(action.map(|(sel, glyph, _tip, command)| {
                icon_button(sel, glyph, &t)
                    .debug_selector(move || sel.into())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        Self::run(command, json!({ "all": true }), window, cx)
                    })
            }))
            .into_any_element()
    }

    fn file_row(&self, group: Group, f: &FileRow, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let key = file_selector(group, &f.path);
        let selected = self.selected.as_deref() == Some(key.as_str());
        let (name, folder) = match f.path.rsplit_once('/') {
            Some((dir, name)) => (name.to_owned(), dir.to_owned()),
            None => (f.path.clone(), String::new()),
        };
        let path = f.path.clone();
        let mut buttons: Vec<gpui::AnyElement> = Vec::new();
        let button =
            |action: &'static str, glyph: &'static str, command: &'static str, args: Value| {
                let sel = button_selector(action, &path);
                icon_button(SharedString::from(sel.clone()), glyph, &t)
                    .debug_selector(move || sel.clone())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        Self::run(command, args.clone(), window, cx)
                    })
                    .into_any_element()
            };
        match group {
            Group::Staged => buttons.push(button(
                "unstage",
                "\u{2212}",
                cmds::UNSTAGE,
                json!({ "paths": [path.clone()] }),
            )),
            Group::Changes | Group::Merge => {
                if group == Group::Changes {
                    buttons.push(button(
                        "undo",
                        "\u{21B6}",
                        cmds::DISCARD,
                        json!({ "paths": [path.clone()] }),
                    ));
                }
                buttons.push(button(
                    "stage",
                    "+",
                    cmds::STAGE,
                    json!({ "paths": [path.clone()] }),
                ))
            }
            Group::Stashes => {}
        }
        let staged = group == Group::Staged;
        let open = path.clone();
        let probed = eludite_ui::bounds_canvas(self.probe.as_ref(), key.clone());
        let label = match &f.old_path {
            Some(old) => format!("{name} \u{2190} {old}"),
            None => name,
        };
        div()
            .id(SharedString::from(key.clone()))
            .debug_selector({
                let key = key.clone();
                move || key
            })
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .pl(px(18.))
            .pr_1()
            .gap_1()
            .whitespace_nowrap()
            .overflow_hidden()
            .relative()
            .children(probed)
            .when_selected(selected, &t)
            .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                this.selected = Some(key.clone());
                cx.notify();
                if e.click_count() >= 2 {
                    // Double-click: Compare with Unmodified.
                    let mut args = json!({ "path": open });
                    if staged {
                        args["staged"] = json!(true);
                    }
                    Self::run(cmds::DIFF, args, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, |_: &MouseDownEvent, _, _| {})
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .text_color(rgb(f.glyph.color()))
                    .child(f.glyph.glyph()),
            )
            .child(div().flex_none().child(label))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_color(t.text_muted)
                    .text_size(t.typography.small)
                    .child(folder),
            )
            .children(buttons)
            .into_any_element()
    }

    fn stash_row(&self, s: &StashOut) -> gpui::AnyElement {
        let t = self.theme;
        let index = s.index;
        let button = |action: &'static str, label: &'static str| {
            let sel = stash_selector(action, index);
            icon_button(SharedString::from(sel.clone()), label, &t)
                .debug_selector(move || sel.clone())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    Self::run(
                        cmds::STASH,
                        json!({ "action": action, "index": index }),
                        window,
                        cx,
                    )
                })
        };
        let sel = format!("git-stash-{index}");
        div()
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel.clone())
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .pl(px(18.))
            .pr_1()
            .gap_1()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(div().flex_1().overflow_hidden().child(s.message.clone()))
            .child(button("apply", "Apply"))
            .child(button("pop", "Pop"))
            .child(button("drop", "Drop"))
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?.clone();
                Some(match row {
                    Row::Header(g, n) => self.header(g, n, cx),
                    Row::File(g, f) => self.file_row(g, &f, cx),
                    Row::Stash(s) => self.stash_row(&s),
                })
            })
            .collect()
    }

    fn toolbar(&self) -> impl IntoElement + use<> {
        let t = self.theme;
        let m = &self.model;
        let branch = match (&m.branch, m.detached) {
            (Some(b), _) => b.clone(),
            (None, true) => "(detached HEAD)".into(),
            (None, false) => String::new(),
        };
        let counts = if m.upstream.is_some() {
            format!("{} outgoing / {} incoming", m.ahead, m.behind)
        } else if m.branch.is_some() {
            "not published".to_owned()
        } else {
            String::new()
        };
        let tool = |sel: &'static str, label: &'static str, command: &'static str| {
            toggle_button(sel, label, false, &t)
                .on_click(move |_, window, cx| Self::run(command, json!({}), window, cx))
        };
        div()
            .flex()
            .flex_row()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .id(BRANCH)
                    .debug_selector(|| BRANCH.into())
                    .px_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .on_click(|_, window, cx| {
                        Self::run(
                            "eludite.view.show",
                            json!({ "id": "git_repository" }),
                            window,
                            cx,
                        )
                    })
                    .child(format!("\u{2387} {branch}")),
            )
            .child(
                div()
                    .text_color(t.text_muted)
                    .text_size(t.typography.small)
                    .child(counts),
            )
            .child(div().flex_1())
            .child(tool(FETCH, "Fetch", cmds::FETCH))
            .child(tool(PULL, "Pull", cmds::PULL))
            .child(tool(PUSH, "Push", cmds::PUSH))
            .child(tool(SYNC, "Sync", cmds::SYNC))
    }
}

/// Selected rows are drawn on the accent color.
trait Selected {
    fn when_selected(self, selected: bool, t: &Theme) -> Self;
}

impl Selected for gpui::Stateful<gpui::Div> {
    fn when_selected(self, selected: bool, t: &Theme) -> Self {
        if selected {
            self.bg(t.accent).text_color(t.text_on_accent)
        } else {
            self.hover(|s| s.bg(t.menu_hover)).text_color(t.text)
        }
    }
}

impl Render for GitChanges {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let root = div()
            .id("git-changes")
            .debug_selector(|| "git-changes".into())
            .size_full()
            .flex()
            .flex_col()
            .text_size(t.typography.ui)
            .text_color(t.text);
        match self.model.repository {
            Some(true) => {}
            Some(false) | None => {
                let text = if self.model.loading {
                    "Looking for a Git repository\u{2026}"
                } else if self.model.repository.is_none() {
                    "Open a workspace to see its Git changes."
                } else {
                    "The workspace is not in a Git repository."
                };
                return root
                    .p_2()
                    .gap_2()
                    .child(div().text_color(t.text_muted).child(text))
                    .children((self.model.repository == Some(false)).then(|| {
                        push_button(CREATE, "Create Git Repository...", true, true, &t)
                            .on_click(|_, window, cx| Self::run(cmds::INIT, json!({}), window, cx))
                    }))
                    .into_any_element();
            }
        }
        let focused = self.focus.is_focused(window);
        let lines: Vec<String> = if self.message.is_empty() && !focused {
            vec!["Enter a message <Required>".into()]
        } else {
            let mut l: Vec<String> = self.message.split('\n').map(str::to_owned).collect();
            if focused && let Some(last) = l.last_mut() {
                last.push('\u{2502}');
            }
            l
        };
        let message_box = div()
            .id(MESSAGE_BOX)
            .debug_selector(|| MESSAGE_BOX.into())
            .relative()
            .children(eludite_ui::bounds_canvas(self.probe.as_ref(), MESSAGE_BOX))
            .track_focus(&self.focus)
            .key_context("GitCommitMessage")
            .on_key_down(cx.listener(Self::message_key))
            .on_click(cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .flex()
            .flex_col()
            .min_h(px(40.))
            .max_h(px(96.))
            .overflow_y_scroll()
            .p_1()
            .bg(t.background)
            .border_1()
            .border_color(if focused { t.accent } else { t.border })
            .cursor_text()
            .text_color(if self.message.is_empty() {
                t.text_muted
            } else {
                t.text
            })
            .children(lines.into_iter().map(|l| div().min_h(px(16.)).child(l)));
        let staged = !self.model.staged.is_empty();
        let commit_label = match (self.amend, staged) {
            (true, _) => "Amend",
            (false, true) => "Commit Staged",
            (false, false) => "Commit All",
        };
        let push_label = if staged {
            "Commit Staged and Push"
        } else {
            "Commit All and Push"
        };
        let amend = self.amend;
        let buttons = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(
                push_button(COMMIT, commit_label, true, true, &t)
                    .relative()
                    .children(eludite_ui::bounds_canvas(self.probe.as_ref(), COMMIT))
                    .on_click(cx.listener(|this, _, window, cx| this.commit(false, window, cx))),
            )
            .child(
                push_button(COMMIT_PUSH, push_label, false, !amend, &t).on_click(cx.listener(
                    move |this, _, window, cx| {
                        if !amend {
                            this.commit(true, window, cx)
                        }
                    },
                )),
            )
            .child(
                push_button(STASH_ALL, "Stash All", false, true, &t).on_click(cx.listener(
                    |this, _, window, cx| {
                        let mut args = json!({ "action": "push", "include_untracked": true });
                        if !this.message.trim().is_empty() {
                            args["message"] = json!(this.message.trim());
                        }
                        Self::run(cmds::STASH, args, window, cx)
                    },
                )),
            )
            .child(
                check_box(AMEND, "Amend", amend, &t).on_click(cx.listener(|this, _, _, cx| {
                    this.amend = !this.amend;
                    cx.notify();
                })),
            );
        let info = self.info.clone().map(|(text, error)| {
            div()
                .id(INFO)
                .debug_selector(|| INFO.into())
                .px_1()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(t.typography.small)
                .text_color(if error { rgb(0xF1_4C_4C) } else { t.text_muted })
                .child(text)
        });
        let operation = self.model.operation.clone().map(|op| {
            div()
                .px_1()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(t.typography.small)
                .text_color(rgb(0xD7_BA_7D))
                .child(match op.as_str() {
                    "rebase" => "Rebasing: resolve, stage, then Continue Rebase".to_owned(),
                    "merge" => "Merging: resolve, stage, then commit".to_owned(),
                    other => format!("{}: resolve, stage, then commit", other.replace('_', "-")),
                })
        });
        let ssl_warning = self.model.ssl_verify_off.then(|| {
            div()
                .id(SSL_WARNING)
                .debug_selector(|| SSL_WARNING.into())
                .px_1()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(t.typography.small)
                .text_color(rgb(0xD7_BA_7D))
                .child(SSL_WARNING_TEXT)
        });
        let progress = self.model.progress.clone().map(|p| {
            div()
                .px_1()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_size(t.typography.small)
                .text_color(t.text_muted)
                .child(p)
        });
        let pull_request_link = self.pull_request_link.then(|| {
            div()
                .id(PULL_REQUEST_LINK)
                .debug_selector(|| PULL_REQUEST_LINK.into())
                .px_1()
                .text_size(t.typography.small)
                .text_color(t.accent)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child("Create a Pull Request")
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(
                        Box::new(RunCommand::new(
                            "eludite.forge.pull_create".to_owned(),
                            json!({}),
                        )),
                        cx,
                    );
                }))
        });
        let count = self.rows.len();
        root.child(self.toolbar())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap_1()
                    .p_1()
                    .child(message_box)
                    .child(buttons)
                    .children(info)
                    .children(pull_request_link)
                    .children(operation)
                    .children(ssl_warning)
                    .children(progress),
            )
            .child(
                uniform_list(
                    "git-changes-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(
                div()
                    .id(HISTORY)
                    .debug_selector(|| HISTORY.into())
                    .flex_none()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(t.border)
                    .text_color(t.accent)
                    .cursor_pointer()
                    .on_click(|_, window, cx| {
                        Self::run(
                            "eludite.view.show",
                            json!({ "id": "git_repository" }),
                            window,
                            cx,
                        )
                    })
                    .child("View all commits (Git Repository)"),
            )
            .into_any_element()
    }
}
