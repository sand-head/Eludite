//! The Find in Files and Replace in Files dialog (brief 0042), as Visual Studio 2022 draws it: one modeless window
//! with the Find in Files and Replace in Files tabs. Find what (with the last queries), Replace with, Look in
//! (Entire Solution, Current Project, Current Document, All Open Documents, each project of the solution, a folder),
//! File types, Match case, Match whole word, Use regular expressions (with the notes on Rust's regex syntax), the
//! results window (Find Results 1 or 2), Append results and Keep modified files open after Replace All; Find All,
//! and in Replace in Files Replace Next, Skip File and Replace All.
//!
//! The dialog owns its fields; its buttons emit [`FindDialogEvent`]s the shell turns into `eludite.search.*` commands
//! with [`FindDialog::args`].

use eludite_commands::search::{FindArgs, ScopeKind, split_file_types};
use eludite_ui::Theme;
use eludite_ui::dialog::{dialog_panel, push_button, section_heading};
use eludite_ui::elements::{check_box, text_box};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px,
};

/// Element ids (and debug selectors).
pub const DIALOG: &str = "find-in-files-dialog";
pub const FIND_TAB: &str = "find-in-files-tab-find";
pub const REPLACE_TAB: &str = "find-in-files-tab-replace";
pub const QUERY_BOX: &str = "find-in-files-query";
pub const REPLACE_BOX: &str = "find-in-files-replacement";
pub const FILE_TYPES_BOX: &str = "find-in-files-file-types";
pub const FOLDER_BOX: &str = "find-in-files-folder";
pub const LOOK_IN: &str = "find-in-files-look-in";
pub const HISTORY: &str = "find-in-files-history";
pub const MATCH_CASE: &str = "find-in-files-match-case";
pub const WHOLE_WORD: &str = "find-in-files-whole-word";
pub const REGEX: &str = "find-in-files-regex";
pub const REGEX_HELP: &str = "find-in-files-regex-help";
pub const WINDOW_1: &str = "find-in-files-window-1";
pub const WINDOW_2: &str = "find-in-files-window-2";
pub const APPEND: &str = "find-in-files-append";
pub const KEEP_OPEN: &str = "find-in-files-keep-open";
pub const FIND_ALL: &str = "find-in-files-find-all";
pub const REPLACE_NEXT: &str = "find-in-files-replace-next";
pub const SKIP_FILE: &str = "find-in-files-skip-file";
pub const REPLACE_ALL: &str = "find-in-files-replace-all";
pub const CLOSE: &str = "find-in-files-close";

pub fn look_in_selector(i: usize) -> String {
    format!("find-in-files-look-in-{i}")
}

pub fn history_selector(i: usize) -> String {
    format!("find-in-files-history-{i}")
}

/// The differences between Rust's regex syntax and .NET's that the help link names.
pub const REGEX_NOTES: &str = "Regular expressions use Rust's syntax, which is .NET's for the common forms: \
classes (\\d \\w \\s [a-z]), quantifiers (* + ? {n,m} and lazy ones), groups ((...), (?:...), named (?<name>...)), \
anchors (^ $ \\b), alternation and flags ((?i)). Not supported: lookahead and lookbehind ((?=...) (?!...) (?<=...)), \
backreferences in the pattern (\\1), atomic groups and possessive quantifiers. A match never spans lines. In Replace \
with, $1, ${name} and $0 insert groups and $$ is a dollar sign; write ${1}0 for group 1 followed by 0.";

/// Which tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Find,
    Replace,
}

/// Look in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookIn {
    Solution,
    CurrentProject,
    CurrentDocument,
    OpenDocuments,
    Project(String),
    Folder,
}

impl LookIn {
    pub fn label(&self) -> String {
        match self {
            LookIn::Solution => "Entire Solution".into(),
            LookIn::CurrentProject => "Current Project".into(),
            LookIn::CurrentDocument => "Current Document".into(),
            LookIn::OpenDocuments => "All Open Documents".into(),
            LookIn::Project(name) => format!("Project: {name}"),
            LookIn::Folder => "Folder...".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Query,
    Replacement,
    FileTypes,
    Folder,
}

/// What the dialog asks of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindDialogEvent {
    FindAll,
    ReplaceNext,
    SkipFile,
    ReplaceAll,
    Close,
}

pub struct FindDialog {
    theme: Theme,
    focus: FocusHandle,
    pub mode: Mode,
    pub query: String,
    pub replacement: String,
    pub file_types: String,
    pub folder: String,
    pub look_in: LookIn,
    pub projects: Vec<String>,
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub window: u8,
    pub append: bool,
    pub keep_open: bool,
    pub history: Vec<String>,
    field: Field,
    /// The field's whole text is selected (as when the dialog opens): typing replaces it.
    selected: bool,
    look_in_open: bool,
    history_open: bool,
    help_open: bool,
}

impl EventEmitter<FindDialogEvent> for FindDialog {}

impl Focusable for FindDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FindDialog {
    pub fn new(theme: Theme, mode: Mode, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            focus: cx.focus_handle(),
            mode,
            query: String::new(),
            replacement: String::new(),
            file_types: String::new(),
            folder: String::new(),
            look_in: LookIn::Solution,
            projects: Vec::new(),
            match_case: false,
            whole_word: false,
            regex: false,
            window: 1,
            append: false,
            keep_open: true,
            history: Vec::new(),
            field: Field::Query,
            selected: true,
            look_in_open: false,
            history_open: false,
            help_open: false,
        }
    }

    /// Show the dialog in `mode` with `query` (the editor's selection) selected in Find what.
    pub fn show(&mut self, mode: Mode, query: Option<String>, cx: &mut Context<Self>) {
        self.mode = mode;
        if let Some(q) = query.filter(|q| !q.is_empty()) {
            self.query = q;
        }
        self.field = Field::Query;
        self.selected = true;
        cx.notify();
    }

    /// The find half of the command's arguments.
    pub fn args(&self) -> FindArgs {
        let (scope, project, path) = match &self.look_in {
            LookIn::Solution => (ScopeKind::Solution, None, None),
            LookIn::CurrentProject => (ScopeKind::Project, None, None),
            LookIn::CurrentDocument => (ScopeKind::Document, None, None),
            LookIn::OpenDocuments => (ScopeKind::OpenDocuments, None, None),
            LookIn::Project(name) => (ScopeKind::Project, Some(name.clone()), None),
            LookIn::Folder => (
                ScopeKind::Folder,
                None,
                Some(self.folder.trim().to_owned()).filter(|p| !p.is_empty()),
            ),
        };
        FindArgs {
            query: Some(self.query.clone()).filter(|q| !q.is_empty()),
            regex: self.regex,
            case_sensitive: self.match_case,
            whole_word: self.whole_word,
            scope,
            project,
            path,
            include: split_file_types(&self.file_types),
            results_window: Some(self.window),
            append: self.append,
            ..FindArgs::default()
        }
    }

    pub fn set_projects(&mut self, projects: Vec<String>, cx: &mut Context<Self>) {
        self.projects = projects;
        cx.notify();
    }

    pub fn set_history(&mut self, history: Vec<String>, cx: &mut Context<Self>) {
        self.history = history;
        cx.notify();
    }

    /// Look in's choices, in order.
    pub fn look_in_choices(&self) -> Vec<LookIn> {
        let mut v = vec![
            LookIn::Solution,
            LookIn::CurrentProject,
            LookIn::CurrentDocument,
            LookIn::OpenDocuments,
        ];
        v.extend(self.projects.iter().cloned().map(LookIn::Project));
        v.push(LookIn::Folder);
        v
    }

    fn fields(&self) -> Vec<Field> {
        let mut f = vec![Field::Query];
        if self.mode == Mode::Replace {
            f.push(Field::Replacement);
        }
        if self.look_in == LookIn::Folder {
            f.push(Field::Folder);
        }
        f.push(Field::FileTypes);
        f
    }

    fn text_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Query => &mut self.query,
            Field::Replacement => &mut self.replacement,
            Field::FileTypes => &mut self.file_types,
            Field::Folder => &mut self.folder,
        }
    }

    fn focus_field(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        self.field = field;
        self.selected = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if m.alt && !m.control {
            // Visual Studio's access keys: the options, and Replace in Files' buttons.
            let replace = self.mode == Mode::Replace && !self.query.is_empty();
            match k.key.as_str() {
                "c" => self.match_case = !self.match_case,
                "w" => self.whole_word = !self.whole_word,
                "e" => self.regex = !self.regex,
                // Replace All, Replace Next and Skip File in Replace in Files.
                "a" if replace => cx.emit(FindDialogEvent::ReplaceAll),
                "r" if replace => cx.emit(FindDialogEvent::ReplaceNext),
                "s" if replace => cx.emit(FindDialogEvent::SkipFile),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if m.control || m.platform {
            match k.key.as_str() {
                "a" => self.selected = true,
                "v" => {
                    let pasted = cx
                        .read_from_clipboard()
                        .and_then(|c| c.text())
                        .map(|t| t.lines().next().unwrap_or_default().to_owned())
                        .unwrap_or_default();
                    let field = self.field;
                    let selected = self.selected;
                    let text = self.text_mut(field);
                    if selected {
                        text.clear();
                    }
                    text.push_str(&pasted);
                    self.selected = false;
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        match k.key.as_str() {
            "escape" => {
                if self.look_in_open || self.history_open {
                    self.look_in_open = false;
                    self.history_open = false;
                } else {
                    cx.emit(FindDialogEvent::Close);
                }
            }
            "enter" => cx.emit(FindDialogEvent::FindAll),
            "tab" => {
                let fields = self.fields();
                let at = fields.iter().position(|f| *f == self.field).unwrap_or(0);
                let next = if m.shift {
                    (at + fields.len() - 1) % fields.len()
                } else {
                    (at + 1) % fields.len()
                };
                self.field = fields[next];
                self.selected = true;
            }
            "backspace" => {
                let field = self.field;
                let selected = self.selected;
                let text = self.text_mut(field);
                if selected {
                    text.clear();
                } else {
                    text.pop();
                }
                self.selected = false;
            }
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if m.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        let field = self.field;
                        let selected = self.selected;
                        let text = self.text_mut(field);
                        if selected {
                            text.clear();
                        }
                        text.push_str(&c);
                        self.selected = false;
                    }
                    _ => return,
                }
            }
        }
        let _ = window;
        cx.stop_propagation();
        cx.notify();
    }

    fn field_box(
        &self,
        field: Field,
        id: &'static str,
        placeholder: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        let focused = self.focus.is_focused(window) && self.field == field;
        let text = match field {
            Field::Query => &self.query,
            Field::Replacement => &self.replacement,
            Field::FileTypes => &self.file_types,
            Field::Folder => &self.folder,
        };
        let mut el = text_box(id, text, placeholder, focused, &t).w(px(330.));
        if focused && self.selected && !text.is_empty() {
            let mut bg = t.accent;
            bg.a = 0.35;
            el = el.bg(bg);
        }
        el.on_click(cx.listener(move |this, _, window, cx| this.focus_field(field, window, cx)))
    }
}

fn label(text: &'static str, t: &Theme) -> gpui::Div {
    div()
        .w(px(110.))
        .flex_none()
        .text_size(t.typography.ui)
        .child(text)
}

impl Render for FindDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let tab = |id: &'static str,
                   text: &'static str,
                   active: bool,
                   mode: Mode,
                   cx: &mut Context<Self>| {
            eludite_ui::tab(
                &t,
                text,
                eludite_ui::elements::TabStyle {
                    active,
                    ..Default::default()
                },
            )
            .id(id)
            .debug_selector(move || id.to_owned())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.mode = mode;
                cx.notify();
            }))
        };
        let tabs = div()
            .flex()
            .flex_row()
            .h(t.typography.tab_height)
            .border_b_1()
            .border_color(t.border)
            .child(tab(
                FIND_TAB,
                "Find in Files",
                self.mode == Mode::Find,
                Mode::Find,
                cx,
            ))
            .child(tab(
                REPLACE_TAB,
                "Replace in Files",
                self.mode == Mode::Replace,
                Mode::Replace,
                cx,
            ));
        let row = || div().flex().flex_row().items_center().gap_2().px_3().py_1();
        let history_button = div()
            .id(HISTORY)
            .debug_selector(|| HISTORY.to_owned())
            .px_1()
            .cursor_pointer()
            .hover(|s| s.bg(t.menu_hover))
            .child("\u{25BE}")
            .on_click(cx.listener(|this, _, _, cx| {
                this.history_open = !this.history_open;
                cx.notify();
            }));
        let mut body = div().flex().flex_col().py_2().child(
            row()
                .child(label("Find what:", &t))
                .child(self.field_box(Field::Query, QUERY_BOX, "", window, cx))
                .child(history_button),
        );
        if self.history_open && !self.history.is_empty() {
            let mut list = div()
                .id("find-in-files-history-list")
                .flex()
                .flex_col()
                .ml(px(122.))
                .w(px(330.))
                .max_h(px(200.))
                .overflow_y_scroll()
                .bg(t.background)
                .border_1()
                .border_color(t.border);
            for (i, q) in self.history.iter().enumerate() {
                let q2 = q.clone();
                list = list.child(
                    eludite_ui::menu_row(
                        history_selector(i),
                        q.clone(),
                        0,
                        false,
                        false,
                        false,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query = q2.clone();
                        this.history_open = false;
                        this.selected = true;
                        cx.notify();
                    })),
                );
            }
            body = body.child(list);
        }
        if self.mode == Mode::Replace {
            body = body.child(
                row()
                    .child(label("Replace with:", &t))
                    .child(self.field_box(Field::Replacement, REPLACE_BOX, "", window, cx)),
            );
        }
        let look_in = div()
            .id(LOOK_IN)
            .debug_selector(|| LOOK_IN.to_owned())
            .flex()
            .items_center()
            .justify_between()
            .w(px(330.))
            .h(px(20.))
            .px_1()
            .bg(t.background)
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .child(self.look_in.label())
            .child("\u{25BE}")
            .on_click(cx.listener(|this, _, _, cx| {
                this.look_in_open = !this.look_in_open;
                cx.notify();
            }));
        body = body.child(row().child(label("Look in:", &t)).child(look_in));
        if self.look_in_open {
            let mut list = div()
                .id("find-in-files-look-in-list")
                .flex()
                .flex_col()
                .ml(px(122.))
                .w(px(330.))
                .max_h(px(220.))
                .overflow_y_scroll()
                .bg(t.background)
                .border_1()
                .border_color(t.border);
            for (i, choice) in self.look_in_choices().into_iter().enumerate() {
                let selected = choice == self.look_in;
                let c = choice.clone();
                list = list.child(
                    eludite_ui::menu_row(
                        look_in_selector(i),
                        choice.label(),
                        0,
                        selected,
                        false,
                        false,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.look_in = c.clone();
                        this.look_in_open = false;
                        if this.look_in == LookIn::Folder {
                            this.focus_field(Field::Folder, window, cx);
                        }
                        cx.notify();
                    })),
                );
            }
            body = body.child(list);
        }
        if self.look_in == LookIn::Folder {
            body = body.child(row().child(label("Folder:", &t)).child(self.field_box(
                Field::Folder,
                FOLDER_BOX,
                "a folder, absolute or in the workspace",
                window,
                cx,
            )));
        }
        body = body.child(row().child(label("File types:", &t)).child(self.field_box(
            Field::FileTypes,
            FILE_TYPES_BOX,
            "*.* (all files); e.g. *.cs;*.cshtml;!*.g.cs",
            window,
            cx,
        )));
        let toggle = |id: &'static str,
                      text: &'static str,
                      on: bool,
                      f: fn(&mut Self),
                      cx: &mut Context<Self>| {
            check_box(id, text, on, &t).on_click(cx.listener(move |this, _, _, cx| {
                f(this);
                cx.notify();
            }))
        };
        body = body.child(section_heading("Find options", &t)).child(
            div()
                .flex()
                .flex_col()
                .px_3()
                .child(toggle(
                    MATCH_CASE,
                    "Match case",
                    self.match_case,
                    |d| d.match_case = !d.match_case,
                    cx,
                ))
                .child(toggle(
                    WHOLE_WORD,
                    "Match whole word",
                    self.whole_word,
                    |d| d.whole_word = !d.whole_word,
                    cx,
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(toggle(
                            REGEX,
                            "Use regular expressions",
                            self.regex,
                            |d| d.regex = !d.regex,
                            cx,
                        ))
                        .child(
                            div()
                                .id(REGEX_HELP)
                                .debug_selector(|| REGEX_HELP.to_owned())
                                .text_color(t.accent)
                                .cursor_pointer()
                                .child("(Rust syntax: differences from .NET)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.help_open = !this.help_open;
                                    cx.notify();
                                })),
                        ),
                ),
        );
        if self.help_open {
            body = body.child(
                div()
                    .mx_3()
                    .p_2()
                    .max_w(px(460.))
                    .bg(t.background)
                    .border_1()
                    .border_color(t.border)
                    .text_size(t.typography.small)
                    .child(REGEX_NOTES),
            );
        }
        let window_choice =
            |id: &'static str, text: &'static str, n: u8, cx: &mut Context<Self>| {
                check_box(id, text, self.window == n, &t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.window = n;
                        cx.notify();
                    },
                ))
            };
        let mut results = div()
            .flex()
            .flex_col()
            .px_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(window_choice(WINDOW_1, "Find Results 1", 1, cx))
                    .child(window_choice(WINDOW_2, "Find Results 2", 2, cx)),
            )
            .child(toggle(
                APPEND,
                "Append results",
                self.append,
                |d| d.append = !d.append,
                cx,
            ));
        if self.mode == Mode::Replace {
            results = results.child(toggle(
                KEEP_OPEN,
                "Keep modified files open after Replace All",
                self.keep_open,
                |d| d.keep_open = !d.keep_open,
                cx,
            ));
        }
        body = body
            .child(section_heading("Result options", &t))
            .child(results);
        let can = !self.query.is_empty();
        let button = |id: &'static str,
                      text: &'static str,
                      default: bool,
                      e: FindDialogEvent,
                      cx: &mut Context<Self>| {
            let b = push_button(id, text, default, can, &t);
            if can {
                b.on_click(cx.listener(move |_, _, _, cx| cx.emit(e)))
            } else {
                b
            }
        };
        let mut buttons = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .p_3()
            .child(button(
                FIND_ALL,
                "Find All",
                true,
                FindDialogEvent::FindAll,
                cx,
            ));
        if self.mode == Mode::Replace {
            buttons = buttons
                .child(button(
                    REPLACE_NEXT,
                    "Replace Next",
                    false,
                    FindDialogEvent::ReplaceNext,
                    cx,
                ))
                .child(button(
                    SKIP_FILE,
                    "Skip File",
                    false,
                    FindDialogEvent::SkipFile,
                    cx,
                ))
                .child(button(
                    REPLACE_ALL,
                    "Replace All",
                    false,
                    FindDialogEvent::ReplaceAll,
                    cx,
                ));
        }
        buttons = buttons.child(
            push_button(CLOSE, "Close", false, true, &t)
                .on_click(cx.listener(|_, _, _, cx| cx.emit(FindDialogEvent::Close))),
        );
        let title = match self.mode {
            Mode::Find => "Find in Files",
            Mode::Replace => "Replace in Files",
        };
        let panel = dialog_panel(&t, title)
            .id(DIALOG)
            .debug_selector(|| DIALOG.into())
            .track_focus(&self.focus)
            .key_context("FindInFilesDialog")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(500.))
            .child(tabs)
            .child(body)
            .child(buttons);
        let viewport = window.viewport_size();
        let at = point((viewport.width - px(520.)).max(px(0.)), px(60.));
        deferred(anchored().position(at).child(panel)).with_priority(4)
    }
}
