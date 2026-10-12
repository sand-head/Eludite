//! Brief 0068 on the editor view: the actions and Visual Studio key bindings of the editing operations, their
//! listeners, Alt+drag box selection, right-click's context menu hook, Expand Selection's syntax tree (parsed on the
//! syntax thread, never on the UI thread), Scroll Line Up and Down, and the Tab hook other owners intercept.
//!
//! A child module of `view`, so it reaches the view's state; the view holds its state in one field
//! ([`EditingViewState`]).

use std::rc::Rc;

use gpui::{
    App, ClipboardItem, Context, Entity, Focusable, KeyBinding, MouseButton, MouseDownEvent,
    Pixels, Point, SharedString, Stateful, Subscription, Window, actions, div, prelude::*, px,
};

use super::EditorView;
use crate::editor::Editor;
use crate::input::TextInput;
use crate::syntax::SyntaxThread;

actions!(
    editor,
    [
        Outdent,
        CommentSelection,
        UncommentSelection,
        ToggleLineComment,
        ToggleBlockComment,
        Duplicate,
        MoveLinesUp,
        MoveLinesDown,
        DeleteLine,
        LineCut,
        OpenLineAbove,
        OpenLineBelow,
        MakeUppercase,
        MakeLowercase,
        ScrollLineUp,
        ScrollLineDown,
        TransposeLine,
        DeleteHorizontalWhiteSpace,
        CycleClipboardRing,
        ExpandSelection,
        ContractSelection,
        BoxSelectUp,
        BoxSelectDown,
        BoxSelectLeft,
        BoxSelectRight,
        InsertCaretsAtAllMatching,
        RemoveLastCaret,
        SkipOccurrence,
        QuickReplace,
        FindNextSelected,
        FindPreviousSelected,
        ToggleFindWholeWord,
        ToggleFindRegex,
        ToggleFindInSelection,
        ReplaceNext,
        ReplaceAll,
        FindBarPrevious,
        GoToLine,
    ]
);

/// The key context of the Quick Find bar (its text boxes are inside it).
pub const FIND_BAR_CONTEXT: &str = "EditorFindBar";

/// Visual Studio's keys for the editing operations (brief 0068), in the editor's context. Bound after the editor's
/// older bindings, so Shift+Alt+Up and Down are box selection (they were Add Caret Above and Below, which a box from
/// a caret is).
pub(crate) fn bindings() -> Vec<KeyBinding> {
    let c = Some(super::KEY_CONTEXT);
    let f = Some(FIND_BAR_CONTEXT);
    vec![
        KeyBinding::new("shift-tab", Outdent, c),
        KeyBinding::new("ctrl-k ctrl-c", CommentSelection, c),
        KeyBinding::new("ctrl-k ctrl-u", UncommentSelection, c),
        KeyBinding::new("ctrl-/", ToggleLineComment, c),
        KeyBinding::new("ctrl-shift-/", ToggleBlockComment, c),
        // The same keys where the layout reports Shift+/ as `?`.
        KeyBinding::new("ctrl-?", ToggleBlockComment, c),
        KeyBinding::new("ctrl-d", Duplicate, c),
        KeyBinding::new("alt-up", MoveLinesUp, c),
        KeyBinding::new("alt-down", MoveLinesDown, c),
        KeyBinding::new("ctrl-shift-l", DeleteLine, c),
        KeyBinding::new("ctrl-l", LineCut, c),
        KeyBinding::new("ctrl-enter", OpenLineAbove, c),
        KeyBinding::new("ctrl-shift-enter", OpenLineBelow, c),
        KeyBinding::new("ctrl-shift-u", MakeUppercase, c),
        KeyBinding::new("ctrl-u", MakeLowercase, c),
        KeyBinding::new("ctrl-up", ScrollLineUp, c),
        KeyBinding::new("ctrl-down", ScrollLineDown, c),
        KeyBinding::new("alt-shift-t", TransposeLine, c),
        KeyBinding::new("ctrl-k ctrl-\\", DeleteHorizontalWhiteSpace, c),
        KeyBinding::new("ctrl-shift-v", CycleClipboardRing, c),
        KeyBinding::new("alt-shift-=", ExpandSelection, c),
        KeyBinding::new("alt-+", ExpandSelection, c),
        KeyBinding::new("alt-shift--", ContractSelection, c),
        KeyBinding::new("alt-_", ContractSelection, c),
        KeyBinding::new("alt-shift-up", BoxSelectUp, c),
        KeyBinding::new("alt-shift-down", BoxSelectDown, c),
        KeyBinding::new("alt-shift-left", BoxSelectLeft, c),
        KeyBinding::new("alt-shift-right", BoxSelectRight, c),
        KeyBinding::new("alt-shift-;", InsertCaretsAtAllMatching, c),
        KeyBinding::new("alt-:", InsertCaretsAtAllMatching, c),
        KeyBinding::new("alt-shift-,", RemoveLastCaret, c),
        KeyBinding::new("alt-<", RemoveLastCaret, c),
        KeyBinding::new("ctrl-alt-shift-.", SkipOccurrence, c),
        KeyBinding::new("ctrl-alt->", SkipOccurrence, c),
        KeyBinding::new("secondary-h", QuickReplace, c),
        KeyBinding::new("ctrl-f3", FindNextSelected, c),
        KeyBinding::new("ctrl-shift-f3", FindPreviousSelected, c),
        KeyBinding::new("ctrl-g", GoToLine, c),
        // The Quick Find bar's own keys, while its boxes have the focus.
        KeyBinding::new("alt-c", super::ToggleFindCaseSensitive, f),
        KeyBinding::new("alt-w", ToggleFindWholeWord, f),
        KeyBinding::new("alt-e", ToggleFindRegex, f),
        KeyBinding::new("alt-i", ToggleFindInSelection, f),
        KeyBinding::new("alt-r", ReplaceNext, f),
        KeyBinding::new("alt-a", ReplaceAll, f),
        KeyBinding::new("f3", super::FindNext, f),
        KeyBinding::new("shift-f3", super::FindPrevious, f),
        KeyBinding::new(
            "shift-enter",
            FindBarPrevious,
            Some("EditorFindBar > Input"),
        ),
    ]
}

/// A handler that takes Tab first (a snippet session, brief 0069): true when it handled the key.
pub type TabHandler = Rc<dyn Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) -> bool>;

/// What right-click shows: the owner's items for the caret offset after the click (the shell's code-window menu).
pub type ContextMenuHandler = Rc<dyn Fn(usize, &App) -> Vec<ContextMenuEntry>>;

/// One row of the editor's context menu. Choosing an item dispatches [`eludite_ui::RunCommand`] with its command.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextMenuEntry {
    Item {
        label: SharedString,
        /// As Visual Studio shows it (`Ctrl+K, Ctrl+C`).
        shortcut: Option<SharedString>,
        command: SharedString,
        args: serde_json::Value,
        enabled: bool,
    },
    Separator,
}

/// The open context menu: where, and its entries.
pub(crate) struct OpenMenu {
    pub at: Point<Pixels>,
    pub entries: Vec<ContextMenuEntry>,
}

/// The Go To Line dialog's box.
pub(crate) struct GoToLineDialog {
    pub input: Entity<TextInput>,
    pub error: bool,
    _subscription: Subscription,
}

/// The view's brief 0068 state.
#[derive(Default)]
pub(crate) struct EditingViewState {
    pub tab_handler: Option<TabHandler>,
    pub context_menu: Option<ContextMenuHandler>,
    /// The syntax tree for Expand Selection: the buffer version it was parsed from.
    pub tree: Option<(clock::Global, tree_sitter::Tree)>,
    /// An Expand Selection waiting for its tree.
    pub tree_task: Option<gpui::Task<()>>,
    /// The Quick Find bar's boxes and their subscriptions.
    pub find_input: Option<Entity<TextInput>>,
    pub replace_input: Option<Entity<TextInput>>,
    pub replace_mode: bool,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    /// Replace All's count, shown until the query changes.
    pub replaced: Option<usize>,
    pub menu: Option<OpenMenu>,
    pub go_to_line: Option<GoToLineDialog>,
    pub subscriptions: Vec<Subscription>,
}

impl EditorView {
    /// Let `handler` take Tab before the editor's own Tab (after the completion list and Emmet).
    pub fn set_tab_handler(&mut self, handler: Option<TabHandler>) {
        self.editing.tab_handler = handler;
    }

    /// Show the owner's context menu on right-click in the text (the shell's code-window menu).
    pub fn set_context_menu_handler(&mut self, handler: Option<ContextMenuHandler>) {
        self.editing.context_menu = handler;
    }

    /// Tab's single dispatch point: an owner's handler, Emmet, then indentation.
    pub(super) fn dispatch_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.editing.tab_handler.clone()
            && !self.read_only
            && handler(self, window, cx)
        {
            return;
        }
        if !self.read_only && self.expand_emmet(cx) {
            return;
        }
        self.edit(cx, Editor::tab);
    }

    /// Run an editing operation on the editor as the keys and commands do (nothing in a read-only document).
    pub fn run_editing(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        self.edit(cx, f);
    }

    /// Move the selection as the keys and commands do (works in a read-only document).
    pub fn run_selection(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        self.movement(cx, f);
    }

    /// Edit.LineCut and Edit.Cut: the text goes to the clipboard.
    fn line_cut(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let text = self.editor.line_cut();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.changed(cx);
        self.after_other_change(cx);
    }

    fn paste_ring(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(text) = self.editor.paste_ring() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.changed(cx);
            self.after_other_change(cx);
        }
    }

    /// Scroll one line; the caret moves only when it would leave the view.
    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        let y = self.scroll.y + self.style.line_height * lines as f32;
        self.set_scroll_y(y, cx);
        let visible = self.visible_rows();
        let row = self.editor.primary_head().row;
        if visible.end > visible.start + 1 {
            // The last visible row may be partly cut off.
            let last = visible.end.saturating_sub(2).max(visible.start);
            if row < visible.start {
                self.editor
                    .move_vertical((visible.start - row) as i64, false);
            } else if row > last {
                self.editor.move_vertical(-((row - last) as i64), false);
            }
        }
        cx.notify();
    }

    /// Expand Selection: by the current syntax tree, parsed on the syntax thread when there is none for this
    /// version (the step applies when it arrives, unless the text or selections changed meanwhile).
    pub fn expand_selection(&mut self, cx: &mut Context<Self>) {
        let Some(language) = self.syntax.language.clone() else {
            self.movement(cx, |e| {
                e.expand_selection(None);
            });
            return;
        };
        let version = self.editor.buffer().version();
        if let Some((v, tree)) = &self.editing.tree
            && *v == version
        {
            let tree = tree.clone();
            self.movement(cx, |e| {
                e.expand_selection(Some(&tree));
            });
            return;
        }
        let snapshot = self.editor.buffer().snapshot().clone();
        let selections = self.editor.selections();
        let parse = SyntaxThread::global().run(move || {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(language.grammar()).ok()?;
            parser.parse(snapshot.text(), None)
        });
        self.editing.tree_task = Some(cx.spawn(async move |this, cx| {
            let Some(Some(tree)) = parse.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.editing.tree_task = None;
                if this.editor.buffer().version() != version {
                    return;
                }
                this.editing.tree = Some((version, tree.clone()));
                if this.editor.selections() == selections {
                    this.movement(cx, |e| {
                        e.expand_selection(Some(&tree));
                    });
                }
            })
            .ok();
        }));
    }

    /// True while Expand Selection waits for its tree (tests wait on it).
    pub fn is_expanding_selection(&self) -> bool {
        self.editing.tree_task.is_some()
    }

    /// Right-click: a click outside the selections moves the caret there first, then the owner's menu opens.
    pub(super) fn on_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(offset) = self.offset_for_position(event.position) else {
            return;
        };
        let inside = self
            .editor
            .selections()
            .iter()
            .any(|s| !s.is_empty() && s.range().contains(&offset));
        if !inside {
            self.editor
                .click(offset, crate::editor::ClickKind::Single, false, false);
            self.moved(cx);
            self.after_other_change(cx);
        }
        let caret = self.editor.primary_selection().head;
        if let Some(handler) = self.editing.context_menu.clone() {
            let entries = handler(caret, cx);
            self.open_context_menu(event.position, entries, cx);
        }
    }

    /// Show `entries` as the context menu at `at` (window coordinates).
    pub fn open_context_menu(
        &mut self,
        at: Point<Pixels>,
        entries: Vec<ContextMenuEntry>,
        cx: &mut Context<Self>,
    ) {
        self.editing.menu = (!entries.is_empty()).then_some(OpenMenu { at, entries });
        cx.notify();
    }

    /// The context menu's entries while it is open.
    pub fn context_menu(&self) -> Option<&[ContextMenuEntry]> {
        self.editing.menu.as_ref().map(|m| m.entries.as_slice())
    }

    pub fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.editing.menu.take().is_some() {
            cx.notify();
        }
    }

    /// Choose context menu item `index`: the menu closes and the item's command runs.
    pub fn choose_context_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.editing.menu.take() else {
            return;
        };
        cx.notify();
        if let Some(ContextMenuEntry::Item {
            command,
            args,
            enabled: true,
            ..
        }) = menu.entries.get(index)
        {
            window.dispatch_action(
                Box::new(eludite_ui::RunCommand::new(command.clone(), args.clone())),
                cx,
            );
        }
    }

    /// Edit.GoTo (Ctrl+G): Visual Studio's Go To Line dialog over the editor, "Line number (1 - N):", with the caret's
    /// line selected; Enter goes (clamped), Escape cancels.
    pub fn open_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.editor.primary_head().row + 1;
        let style = self.style.clone();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(false, cx);
            input.set_style(style, cx);
            input.set_text(&line.to_string(), cx);
            input.select_all(cx);
            input
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, event, window, cx| match event {
                crate::input::TextInputEvent::Submit => {
                    let text = input.read(cx).text();
                    this.submit_go_to_line(&text, window, cx);
                }
                crate::input::TextInputEvent::Escape => this.close_go_to_line(window, cx),
                crate::input::TextInputEvent::Changed => {
                    if let Some(d) = this.editing.go_to_line.as_mut() {
                        d.error = false;
                    }
                }
                _ => {}
            },
        );
        let focus = input.focus_handle(cx);
        self.editing.go_to_line = Some(GoToLineDialog {
            input,
            error: false,
            _subscription: sub,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The dialog is open.
    pub fn is_go_to_line_open(&self) -> bool {
        self.editing.go_to_line.is_some()
    }

    /// OK: go to the line typed (a number; out of range is clamped). False, and the dialog stays, for a non-number.
    pub fn submit_go_to_line(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Ok(line) = text.trim().parse::<u32>() else {
            if let Some(d) = self.editing.go_to_line.as_mut() {
                d.error = true;
            }
            cx.notify();
            return false;
        };
        self.movement(cx, |e| e.go_to_line(line));
        self.close_go_to_line(window, cx);
        true
    }

    pub fn close_go_to_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.go_to_line = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The context menu and the Go To Line dialog, drawn over the text.
    pub(super) fn render_editing_overlays(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let theme = self.style.theme;
        let mut out = Vec::new();
        if let Some(menu) = &self.editing.menu {
            let origin = self
                .layout
                .as_ref()
                .map_or(menu.at, |l| menu.at - l.bounds.origin);
            let mut rows: Vec<gpui::AnyElement> = Vec::new();
            for (i, entry) in menu.entries.iter().enumerate() {
                match entry {
                    ContextMenuEntry::Separator => rows.push(
                        div()
                            .h(px(1.))
                            .mx_1()
                            .my_1()
                            .bg(theme.border)
                            .into_any_element(),
                    ),
                    ContextMenuEntry::Item {
                        label,
                        shortcut,
                        enabled,
                        ..
                    } => {
                        let row = div()
                            .id(SharedString::from(format!("editor-context-item-{i}")))
                            .flex()
                            .flex_row()
                            .justify_between()
                            .gap_4()
                            .px_2()
                            .py(px(2.))
                            .min_w(px(240.))
                            .when(!*enabled, |d| d.text_color(theme.text_disabled))
                            .when(*enabled, |d| d.hover(|d| d.bg(theme.menu_hover)))
                            .child(label.clone())
                            .children(shortcut.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.choose_context_item(i, window, cx)
                                }),
                            );
                        rows.push(row.into_any_element());
                    }
                }
            }
            out.push(
                div()
                    .id("editor-context-menu")
                    .absolute()
                    .left(origin.x)
                    .top(origin.y)
                    .py_1()
                    .bg(theme.menu_background)
                    .border_1()
                    .border_color(theme.popup_border)
                    .text_color(theme.menu_text)
                    .text_size(theme.typography.ui)
                    .cursor_default()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_context_menu(cx)))
                    .children(rows)
                    .into_any_element(),
            );
        }
        if let Some(dialog) = &self.editing.go_to_line {
            let n = self.editor.buffer().line_count();
            let input = dialog.input.clone();
            out.push(
                div()
                    .id("go-to-line")
                    .absolute()
                    .top(px(40.))
                    .left(px(80.))
                    .w(px(300.))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(theme.panel)
                    .border_1()
                    .border_color(theme.accent)
                    .text_color(theme.text)
                    .text_size(theme.typography.ui)
                    .cursor_default()
                    .child(SharedString::from("Go To Line"))
                    .child(SharedString::from(format!("Line number (1 - {n}):")))
                    .child(
                        div()
                            .border_1()
                            .border_color(if dialog.error {
                                theme.warning
                            } else {
                                theme.border
                            })
                            .child(input.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("go-to-line-ok")
                                    .px_2()
                                    .border_1()
                                    .border_color(theme.border)
                                    .child("OK")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            let text = input.read(cx).text();
                                            this.submit_go_to_line(&text, window, cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("go-to-line-cancel")
                                    .px_2()
                                    .border_1()
                                    .border_color(theme.border)
                                    .child("Cancel")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.close_go_to_line(window, cx)
                                        }),
                                    ),
                            ),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// Alt+drag: the (row, visual column) under `position`, columns past a line's end included.
    pub(super) fn box_point(&self, position: Point<Pixels>) -> Option<(u32, u32)> {
        let l = self.layout.as_ref()?;
        let buffer = self.editor.buffer();
        let y = position.y - l.bounds.top() + l.scroll.y;
        let row = l
            .vertical
            .hit(f32::from(y))
            .row
            .min(buffer.line_count() - 1);
        let x = (position.x - l.text_left + l.scroll.x).max(px(0.));
        let col = (x / l.char_width).round() as u32;
        Some((row, col))
    }

    /// The listeners of brief 0068's actions.
    pub(super) fn bind_editing(
        &self,
        el: Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        macro_rules! edits {
            ($el:expr, $( $action:ident => |$e:ident| $body:expr; )*) => {
                $el$(
                    .on_action(cx.listener(|this: &mut EditorView, _: &$action, _: &mut Window, cx: &mut Context<EditorView>| {
                        this.edit(cx, |$e: &mut Editor| { $body; });
                    }))
                )*
            };
        }
        macro_rules! moves {
            ($el:expr, $( $action:ident => |$e:ident| $body:expr; )*) => {
                $el$(
                    .on_action(cx.listener(|this: &mut EditorView, _: &$action, _: &mut Window, cx: &mut Context<EditorView>| {
                        this.movement(cx, |$e: &mut Editor| { $body; });
                    }))
                )*
            };
        }
        let el = edits!(el,
            Outdent => |e| e.outdent();
            CommentSelection => |e| e.comment_selection();
            UncommentSelection => |e| e.uncomment_selection();
            ToggleLineComment => |e| e.toggle_line_comment();
            ToggleBlockComment => |e| e.toggle_block_comment();
            Duplicate => |e| e.duplicate();
            MoveLinesUp => |e| e.move_lines(true);
            MoveLinesDown => |e| e.move_lines(false);
            DeleteLine => |e| e.delete_line();
            OpenLineAbove => |e| e.open_line(true);
            OpenLineBelow => |e| e.open_line(false);
            MakeUppercase => |e| e.change_case(true);
            MakeLowercase => |e| e.change_case(false);
            TransposeLine => |e| e.transpose_line();
            DeleteHorizontalWhiteSpace => |e| e.delete_horizontal_white_space();
        );
        let el = moves!(el,
            BoxSelectUp => |e| e.extend_box(-1, 0);
            BoxSelectDown => |e| e.extend_box(1, 0);
            BoxSelectLeft => |e| e.extend_box(0, -1);
            BoxSelectRight => |e| e.extend_box(0, 1);
            InsertCaretsAtAllMatching => |e| e.select_all_occurrences();
            RemoveLastCaret => |e| e.remove_last_caret();
            SkipOccurrence => |e| e.skip_occurrence();
            ContractSelection => |e| e.contract_selection();
        );
        el.on_action(cx.listener(|this, _: &LineCut, _, cx| this.line_cut(cx)))
            .on_action(cx.listener(|this, _: &CycleClipboardRing, _, cx| this.paste_ring(cx)))
            .on_action(cx.listener(|this, _: &ScrollLineUp, _, cx| this.scroll_lines(-1, cx)))
            .on_action(cx.listener(|this, _: &ScrollLineDown, _, cx| this.scroll_lines(1, cx)))
            .on_action(cx.listener(|this, _: &ExpandSelection, _, cx| this.expand_selection(cx)))
            .on_action(
                cx.listener(|this, _: &GoToLine, window, cx| this.open_go_to_line(window, cx)),
            )
            .on_action(cx.listener(|this, _: &QuickReplace, window, cx| {
                this.open_find_bar(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindNextSelected, window, cx| {
                this.find_selected(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FindPreviousSelected, window, cx| {
                this.find_selected(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleFindWholeWord, _, cx| {
                this.toggle_find_option(|o| o.whole_word = !o.whole_word, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleFindRegex, _, cx| {
                this.toggle_find_option(|o| o.regex = !o.regex, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleFindInSelection, _, cx| {
                this.toggle_find_in_selection(cx)
            }))
            .on_action(cx.listener(|this, _: &ReplaceNext, _, cx| this.replace_next(cx)))
            .on_action(cx.listener(|this, _: &ReplaceAll, _, cx| {
                this.replace_all(cx);
            }))
            .on_action(cx.listener(|this, _: &FindBarPrevious, _, cx| {
                this.movement(cx, |e| {
                    e.find_previous();
                })
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
    }
}
