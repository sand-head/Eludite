//! `EditorView`'s IntelliSense state, trigger logic and popups (brief 0013): the completion list, Quick Info and
//! Parameter Info. The pure parts are in [`crate::intellisense`]; this is the part that needs the view.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eludite_ui::markdown;
use eludite_ui::popup::{COMPLETION_ROWS, completion_row, popup_panel};
use fuzzy::StringMatchCandidate;
use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, SharedString, Styled, StyledText, Task, Window, anchored,
    deferred, div, point, px,
};
use text::{Anchor, Bias};

use crate::display::{from_display, to_display, visual_column};
use crate::intellisense::{
    AcceptedCompletion, CompletionItem, CompletionRequest, CompletionSnapshot, CompletionSource,
    CompletionTrigger, EditorEvent, HOVER_DELAY, HoverSnapshot, ListMatch, Rendered,
    SignatureHelpData, SignatureSnapshot, SignatureTrigger, candidates, enclosing_call,
    identifier_items, identifiers, is_identifier_char, rank, word_start_in,
};
use crate::syntax::{HighlightKind, SyntaxThread};
use crate::view::EditorView;

/// The open completion list.
pub(crate) struct CompletionMenu {
    /// The newest request for this list.
    id: u64,
    /// The request the shown items came from.
    items_id: u64,
    /// Start of the word being completed.
    anchor: Anchor,
    items: Arc<Vec<CompletionItem>>,
    candidates: Arc<Vec<StringMatchCandidate>>,
    source: Option<CompletionSource>,
    incomplete: bool,
    /// Waiting for an answer to `id`.
    loading: bool,
    matches: Vec<ListMatch>,
    /// The filter `matches` were computed for (`None`: not yet).
    filter: Option<String>,
    selected: usize,
    /// The user moved the selection with the keys.
    explicit: bool,
    /// Soft selection (opened by `(` or `<` with nothing typed): Enter does not commit.
    soft: bool,
    scroll_top: usize,
    filter_seq: u64,
    filter_task: Option<Task<()>>,
    /// The item a resolve was last asked for.
    resolving: Option<(u64, usize)>,
}

impl CompletionMenu {
    fn visible(&self) -> bool {
        !self.matches.is_empty()
    }
}

/// Quick Info.
pub(crate) struct HoverState {
    id: u64,
    offset: usize,
    range: Range<Anchor>,
    loading: bool,
    rendered: Option<Rendered>,
    /// Opened by resting the mouse (closes when it leaves the word), not by Ctrl+K, Ctrl+I.
    from_mouse: bool,
}

/// Parameter Info.
pub(crate) struct SignatureState {
    id: u64,
    /// The `(` of the call (or the caret when none was found).
    anchor: Anchor,
    loading: bool,
    data: Option<SignatureHelpData>,
    /// The overload shown (Up and Down cycle).
    shown: usize,
    /// The argument index from the text, until the server's answer to a retrigger arrives.
    local_parameter: Option<usize>,
}

/// Everything IntelliSense keeps in a view.
pub(crate) struct Popups {
    pub completion: Option<CompletionMenu>,
    pub hover: Option<HoverState>,
    pub signature: Option<SignatureState>,
    next_id: u64,
    pub triggers: Vec<char>,
    /// The fallback list's identifiers and the buffer version they were read from.
    identifiers: Option<(clock::Global, Arc<Vec<String>>)>,
    identifier_task: Option<Task<()>>,
    /// The word under the mouse whose hover delay is running.
    hover_timer: Option<(usize, Task<()>)>,
    /// The offset the last mouse hover was asked for.
    mouse_hover: Option<usize>,
}

impl Default for Popups {
    fn default() -> Self {
        Self {
            completion: None,
            hover: None,
            signature: None,
            next_id: 0,
            triggers: crate::intellisense::DEFAULT_COMPLETION_TRIGGERS.to_vec(),
            identifiers: None,
            identifier_task: None,
            hover_timer: None,
            mouse_hover: None,
        }
    }
}

impl Popups {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

impl EditorView {
    // ----- completion -----

    /// Open the completion list at the caret (or refresh the open one) and return the request to make. The list
    /// shows nothing until [`EditorView::set_completions`] answers with the returned id.
    pub fn open_completion(
        &mut self,
        trigger: Option<CompletionTrigger>,
        cx: &mut Context<Self>,
    ) -> CompletionRequest {
        let id = self.popups.next_id();
        let caret = self.editor.primary_selection().head;
        let word_start = match trigger {
            Some(CompletionTrigger::Character(_)) => caret,
            _ => self.word_start_before(caret),
        };
        let buffer = self.editor.buffer();
        let same = self
            .popups
            .completion
            .as_ref()
            .is_some_and(|m| buffer.offset_for_anchor(&m.anchor) == word_start);
        if same {
            let menu = self.popups.completion.as_mut().expect("checked");
            menu.id = id;
            menu.loading = true;
        } else {
            let soft = matches!(trigger, Some(CompletionTrigger::Character('(' | '<')));
            self.popups.completion = Some(CompletionMenu {
                id,
                items_id: 0,
                anchor: buffer.anchor_before(word_start),
                items: Arc::default(),
                candidates: Arc::default(),
                source: None,
                incomplete: false,
                loading: true,
                matches: Vec::new(),
                filter: None,
                selected: 0,
                explicit: false,
                soft,
                scroll_top: 0,
                filter_seq: 0,
                filter_task: None,
                resolving: None,
            });
        }
        cx.notify();
        CompletionRequest {
            id,
            offset: caret,
            word_start,
        }
    }

    /// Answer completion request `id`. Returns false (and changes nothing) when `id` is not the newest request of the
    /// open list. Language-server items replace fallback items; fallback items never replace a server's, and an empty
    /// server answer keeps the fallback list.
    pub fn set_completions(
        &mut self,
        id: u64,
        items: Vec<CompletionItem>,
        source: CompletionSource,
        incomplete: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.popups.completion.as_mut() else {
            return false;
        };
        if menu.id != id {
            return false;
        }
        menu.loading = false;
        let keep_server = source == CompletionSource::Syntax
            && menu.source == Some(CompletionSource::LanguageServer)
            && !menu.items.is_empty();
        let keep_fallback = source == CompletionSource::LanguageServer
            && items.is_empty()
            && menu.source == Some(CompletionSource::Syntax);
        if keep_server || keep_fallback {
            let empty = menu.matches.is_empty();
            if empty {
                self.close_completion(cx);
            }
            cx.notify();
            return true;
        }
        menu.candidates = Arc::new(candidates(&items));
        menu.items = Arc::new(items);
        menu.items_id = id;
        menu.source = Some(source);
        menu.incomplete = incomplete;
        menu.filter = None;
        menu.matches.clear();
        menu.resolving = None;
        self.refilter(cx);
        cx.notify();
        true
    }

    /// Fill completion request `id` from the identifiers of the buffer's syntax tree (the fallback while no language
    /// server can answer). Uses the identifiers read last, at once, and reads them again off the UI thread when the
    /// buffer changed since.
    pub fn complete_from_syntax(&mut self, id: u64, cx: &mut Context<Self>) {
        let version = self.editor.buffer().version();
        let fresh = self
            .popups
            .identifiers
            .as_ref()
            .is_some_and(|(v, _)| *v == version);
        if let Some((_, names)) = self.popups.identifiers.clone() {
            self.set_syntax_items(id, &names, cx);
        }
        if fresh || self.popups.identifier_task.is_some() {
            return;
        }
        let Some(language) = self.syntax_language() else {
            if self.popups.identifiers.is_none() {
                self.set_completions(id, Vec::new(), CompletionSource::Syntax, false, cx);
            }
            return;
        };
        let text = self.editor.buffer().text();
        let job = SyntaxThread::global().run(move || {
            let names = identifiers(&language, &text);
            drop(text);
            crate::syntax::alloc::release_free_memory();
            names
        });
        self.popups.identifier_task = Some(cx.spawn(async move |this, cx| {
            let names = job.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.popups.identifier_task = None;
                let names = Arc::new(names);
                this.popups.identifiers = Some((version, names.clone()));
                // Fill the open list if it is still waiting for syntax items (or shows older ones).
                if let Some(menu) = &this.popups.completion
                    && menu.source != Some(CompletionSource::LanguageServer)
                {
                    let id = menu.id;
                    this.set_syntax_items(id, &names, cx);
                }
            });
        }));
    }

    /// Read the fallback identifiers now, off the UI thread, so the first fallback list shows at once.
    pub fn prefetch_identifiers(&mut self, cx: &mut Context<Self>) {
        if self.popups.identifiers.is_none() && self.popups.identifier_task.is_none() {
            // An id no list has: nothing is shown, the cache is filled.
            self.complete_from_syntax(0, cx);
        }
    }

    fn set_syntax_items(&mut self, id: u64, names: &[String], cx: &mut Context<Self>) {
        let typing = self.typed_word();
        let items = identifier_items(names, &typing);
        let menu_id = self.popups.completion.as_ref().map(|m| m.id);
        if menu_id == Some(id) {
            self.set_completions(id, items, CompletionSource::Syntax, false, cx);
        }
    }

    /// Item `index` of list `id` was resolved: show its documentation (Markdown) and detail.
    pub fn set_completion_resolved(
        &mut self,
        id: u64,
        index: usize,
        detail: Option<String>,
        documentation: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.popups.completion.as_mut() else {
            return false;
        };
        if menu.items_id != id || index >= menu.items.len() {
            return false;
        }
        let items = Arc::make_mut(&mut menu.items);
        let item = &mut items[index];
        if detail.is_some() {
            item.detail = detail;
        }
        if documentation.is_some() {
            item.documentation = documentation;
        }
        item.resolved = true;
        cx.notify();
        true
    }

    /// The item of list `id` at `index`, as the owner gave it.
    pub fn completion_item(&self, id: u64, index: usize) -> Option<&CompletionItem> {
        let menu = self.popups.completion.as_ref()?;
        (menu.items_id == id)
            .then(|| menu.items.get(index))
            .flatten()
    }

    /// The completion list as shown, `None` when closed.
    pub fn completion(&self) -> Option<CompletionSnapshot> {
        let menu = self.popups.completion.as_ref()?;
        Some(CompletionSnapshot {
            id: menu.id,
            loading: menu.loading,
            visible: menu.visible(),
            source: menu.source,
            filter: self.typed_word(),
            items: menu
                .matches
                .iter()
                .map(|m| {
                    let i = &menu.items[m.item];
                    (i.label.clone(), i.kind, i.detail.clone())
                })
                .collect(),
            selected: menu.visible().then_some(menu.selected),
            word_start: self.editor.buffer().offset_for_anchor(&menu.anchor),
        })
    }

    /// Commit the selected item, or the shown item labeled `label`. Returns what was inserted, `None` when nothing was
    /// (no list, or no such item); the list closes either way when an item was committed.
    pub fn accept_completion(
        &mut self,
        label: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<AcceptedCompletion> {
        let menu = self.popups.completion.as_ref()?;
        let index = match label {
            Some(l) => menu
                .matches
                .iter()
                .map(|m| m.item)
                .find(|&i| menu.items[i].label == l)
                .or_else(|| menu.items.iter().position(|i| i.label == l))?,
            None => menu.matches.get(menu.selected)?.item,
        };
        let item = menu.items[index].clone();
        let word_start = self.editor.buffer().offset_for_anchor(&menu.anchor);
        self.editor.collapse_to_primary();
        let caret = self.editor.primary_selection().head;
        let buffer = self.editor.buffer();
        let (range, text) = match &item.edit {
            Some(edit) => {
                let start = buffer.offset_for_anchor(&edit.range.start);
                let mut end = buffer.offset_for_anchor(&edit.range.end).max(start);
                // Text typed after the request extends the edit to the caret (same line).
                let same_line =
                    buffer.offset_to_point(end).row == buffer.offset_to_point(caret).row;
                if caret > end && same_line {
                    end = caret;
                }
                (start..end, edit.new_text.clone())
            }
            None => (
                word_start.min(caret)..caret,
                item.insert_text
                    .clone()
                    .unwrap_or_else(|| item.label.clone()),
            ),
        };
        self.editor.set_selections(
            vec![crate::SelectionRange {
                tail: range.start,
                head: range.end,
            }],
            0,
        );
        self.editor.insert(&text);
        self.popups.completion = None;
        cx.emit(EditorEvent::CompletionClosed);
        self.changed(cx);
        self.signature_follow_caret(cx);
        Some(AcceptedCompletion {
            label: item.label,
            text,
        })
    }

    /// Close the completion list (Escape, or the owner's decision).
    pub fn close_completion(&mut self, cx: &mut Context<Self>) {
        if self.popups.completion.take().is_some() {
            cx.emit(EditorEvent::CompletionClosed);
            cx.notify();
        }
    }

    /// Characters that open the completion list when typed (default [`crate::intellisense::DEFAULT_COMPLETION_TRIGGERS`]).
    pub fn set_completion_triggers(&mut self, triggers: &[char]) {
        self.popups.triggers = triggers.to_vec();
    }

    fn word_start_before(&self, offset: usize) -> usize {
        let buffer = self.editor.buffer();
        let point = buffer.offset_to_point(offset);
        let line_start = buffer.point_to_offset(text::Point::new(point.row, 0));
        let before = buffer.text_for_range(line_start..offset);
        line_start + word_start_in(&before, before.len())
    }

    /// The text from the start of the open list's word to the caret.
    fn typed_word(&self) -> String {
        let Some(menu) = &self.popups.completion else {
            return String::new();
        };
        let buffer = self.editor.buffer();
        let start = buffer.offset_for_anchor(&menu.anchor);
        let caret = self.editor.primary_selection().head;
        if caret < start || caret - start > 256 {
            return String::new();
        }
        buffer.text_for_range(start..caret)
    }

    /// Filter the open list by the text typed since its word start. Returns false when the list closed because the
    /// caret left the word.
    fn refilter(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &self.popups.completion else {
            return false;
        };
        let buffer = self.editor.buffer();
        let start = buffer.offset_for_anchor(&menu.anchor);
        let caret = self.editor.primary_selection().head;
        let query = if caret >= start && caret - start <= 256 {
            buffer.text_for_range(start..caret)
        } else {
            String::from("\n")
        };
        if !query.chars().all(is_identifier_char) {
            self.close_completion(cx);
            return false;
        }
        let menu = self.popups.completion.as_mut().expect("checked");
        if !query.is_empty() {
            menu.soft = false;
        }
        if menu.filter.as_deref() == Some(query.as_str()) {
            return true;
        }
        menu.filter_seq += 1;
        let seq = menu.filter_seq;
        if query.is_empty() || menu.items.is_empty() {
            let matches = rank(&menu.items, &menu.candidates, "", Vec::new());
            self.apply_matches(seq, query, matches, cx);
            return true;
        }
        let items = menu.items.clone();
        let candidates = menu.candidates.clone();
        let executor = cx.background_executor().clone();
        menu.filter_task = Some(cx.spawn(async move |this, cx| {
            let cancel = AtomicBool::new(false);
            let found = fuzzy::match_strings(
                candidates.as_slice(),
                &query,
                false,
                true,
                candidates.len(),
                &cancel,
                executor,
            )
            .await;
            let matches = rank(&items, &candidates, &query, found);
            let _ = this.update(cx, |this, cx| this.apply_matches(seq, query, matches, cx));
        }));
        true
    }

    fn apply_matches(
        &mut self,
        seq: u64,
        query: String,
        matches: Vec<ListMatch>,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.popups.completion.as_mut() else {
            return;
        };
        if menu.filter_seq != seq {
            return;
        }
        let previous = menu
            .explicit
            .then(|| menu.matches.get(menu.selected).map(|m| m.item))
            .flatten();
        menu.filter_task = None;
        menu.filter = Some(query);
        menu.matches = matches;
        menu.selected = previous
            .and_then(|p| menu.matches.iter().position(|m| m.item == p))
            .unwrap_or(0);
        if !menu.explicit {
            menu.scroll_top = 0;
        }
        Self::scroll_to_selected(menu);
        if menu.matches.is_empty() && !menu.loading && menu.source.is_some() {
            self.close_completion(cx);
            return;
        }
        self.request_resolve(cx);
        cx.notify();
    }

    fn scroll_to_selected(menu: &mut CompletionMenu) {
        if menu.selected < menu.scroll_top {
            menu.scroll_top = menu.selected;
        } else if menu.selected >= menu.scroll_top + COMPLETION_ROWS {
            menu.scroll_top = menu.selected + 1 - COMPLETION_ROWS;
        }
    }

    /// Ask for the selected item's documentation once.
    fn request_resolve(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.popups.completion.as_mut() else {
            return;
        };
        let Some(m) = menu.matches.get(menu.selected) else {
            return;
        };
        let key = (menu.items_id, m.item);
        if menu.items[m.item].resolved || menu.resolving == Some(key) {
            return;
        }
        menu.resolving = Some(key);
        cx.emit(EditorEvent::ResolveCompletion {
            id: key.0,
            index: key.1,
        });
    }

    /// Move the selection by `delta` rows (no wrap).
    pub(crate) fn move_completion_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(menu) = self.popups.completion.as_mut() else {
            return;
        };
        if menu.matches.is_empty() {
            return;
        }
        let last = menu.matches.len() as isize - 1;
        menu.selected = (menu.selected as isize + delta).clamp(0, last) as usize;
        menu.explicit = true;
        menu.soft = false;
        Self::scroll_to_selected(menu);
        self.request_resolve(cx);
        cx.notify();
    }

    pub(crate) fn completion_visible(&self) -> bool {
        self.popups
            .completion
            .as_ref()
            .is_some_and(CompletionMenu::visible)
    }

    pub(crate) fn completion_hard_selected(&self) -> bool {
        self.popups
            .completion
            .as_ref()
            .is_some_and(|m| m.visible() && !m.soft)
    }

    /// After typing `text`: filter or close the open list, and decide whether to ask for completion or Parameter Info.
    pub(crate) fn after_typing(&mut self, text: &str, cx: &mut Context<Self>) {
        self.close_hover(cx);
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            // A paste or composed text.
            self.close_completion(cx);
            self.signature_follow_caret(cx);
            return;
        };
        if self.editor.selections().len() > 1 {
            self.close_completion(cx);
            return;
        }
        let caret = self.editor.primary_selection().head;
        let typed_at = caret - c.len_utf8();
        if is_identifier_char(c) {
            if let Some(menu) = &self.popups.completion {
                let refresh = menu.loading || menu.incomplete;
                if self.refilter(cx) && refresh {
                    cx.emit(EditorEvent::CompletionTriggered(CompletionTrigger::Refresh));
                }
            } else if !c.is_ascii_digit()
                && self.word_start_before(caret) == typed_at
                && !self.in_comment_or_string(typed_at)
            {
                cx.emit(EditorEvent::CompletionTriggered(CompletionTrigger::Typing(
                    c,
                )));
            }
        } else {
            self.close_completion(cx);
            if self.popups.triggers.contains(&c) && !self.in_comment_or_string(typed_at) {
                cx.emit(EditorEvent::CompletionTriggered(
                    CompletionTrigger::Character(c),
                ));
            }
        }
        match c {
            '(' | ',' if !self.in_comment_or_string(typed_at) => {
                if c == ',' {
                    self.signature_follow_caret(cx);
                }
                cx.emit(EditorEvent::SignatureHelpTriggered(
                    SignatureTrigger::Character(c),
                ));
            }
            ')' => self.close_signature_help(cx),
            _ => self.signature_follow_caret(cx),
        }
    }

    /// After Backspace: filter the open list again, or close it when the caret left the word.
    pub(crate) fn after_backspace(&mut self, cx: &mut Context<Self>) {
        self.close_hover(cx);
        if let Some(menu) = &self.popups.completion {
            let refresh = menu.loading || menu.incomplete;
            if self.refilter(cx) && refresh {
                cx.emit(EditorEvent::CompletionTriggered(CompletionTrigger::Refresh));
            }
        }
        self.signature_follow_caret(cx);
    }

    /// After any other edit or a caret move by the keys or the mouse.
    pub(crate) fn after_other_change(&mut self, cx: &mut Context<Self>) {
        self.close_completion(cx);
        self.close_hover(cx);
        self.signature_follow_caret(cx);
    }

    /// True when `offset` is inside a comment or a string literal by the current highlights.
    fn in_comment_or_string(&self, offset: usize) -> bool {
        let buffer = self.editor.buffer();
        let kind = |o: usize| self.highlights().kind_at(buffer.offset_to_point(o));
        let before = (offset > 0).then(|| kind(offset - 1)).flatten();
        let after = (offset + 1 < buffer.len())
            .then(|| kind(offset + 1))
            .flatten();
        let comment = |k: Option<HighlightKind>| {
            matches!(k, Some(HighlightKind::Comment | HighlightKind::DocComment))
        };
        let string = |k: Option<HighlightKind>| matches!(k, Some(HighlightKind::String));
        comment(before) || (string(before) && string(after)) || self.after_line_comment(offset)
    }

    /// True when `//` starts a comment before `offset` on its line (outside a `"` string). Covers text typed into a
    /// comment before the highlights caught up with it (C-family and Rust line comments).
    fn after_line_comment(&self, offset: usize) -> bool {
        let buffer = self.editor.buffer();
        let point = buffer.offset_to_point(offset);
        let line = buffer.line(point.row);
        let prefix = &line[..(point.column as usize).min(line.len())];
        let mut in_string = false;
        let mut prev = '\0';
        for c in prefix.chars() {
            match c {
                '"' if prev != '\\' => in_string = !in_string,
                '/' if prev == '/' && !in_string => return true,
                _ => {}
            }
            prev = c;
        }
        false
    }

    // ----- Parameter Info -----

    /// Open Parameter Info for the call around the caret (or refresh it) and return the request id to answer with
    /// [`EditorView::set_signature_help`].
    pub fn open_signature_help(&mut self, cx: &mut Context<Self>) -> u64 {
        let id = self.popups.next_id();
        let (paren, commas) = self.call_at_caret().unzip();
        let caret = self.editor.primary_selection().head;
        let anchor = self.editor.buffer().anchor_before(paren.unwrap_or(caret));
        match self.popups.signature.as_mut() {
            Some(s) => {
                s.id = id;
                s.loading = true;
                if paren.is_some() {
                    s.anchor = anchor;
                    s.local_parameter = commas;
                }
            }
            None => {
                self.popups.signature = Some(SignatureState {
                    id,
                    anchor,
                    loading: true,
                    data: None,
                    shown: 0,
                    local_parameter: commas,
                });
            }
        }
        cx.notify();
        id
    }

    /// Answer Parameter Info request `id`; `None` (or no signatures) closes it.
    pub fn set_signature_help(
        &mut self,
        id: u64,
        data: Option<SignatureHelpData>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.popups.signature.as_mut() else {
            return false;
        };
        if state.id != id {
            return false;
        }
        match data.filter(|d| !d.signatures.is_empty()) {
            None => self.close_signature_help(cx),
            Some(data) => {
                let same_overloads = state
                    .data
                    .as_ref()
                    .is_some_and(|d| d.signatures.len() == data.signatures.len());
                if !same_overloads {
                    state.shown = data.active_signature.min(data.signatures.len() - 1);
                }
                state.data = Some(data);
                state.loading = false;
                state.local_parameter = None;
                cx.notify();
            }
        }
        true
    }

    /// Parameter Info as shown, `None` when closed.
    pub fn signature_help(&self) -> Option<SignatureSnapshot> {
        let s = self.popups.signature.as_ref()?;
        Some(SignatureSnapshot {
            id: s.id,
            loading: s.loading,
            visible: s.data.is_some(),
            data: s.data.clone(),
            active_signature: s.shown,
            active_parameter: self.active_parameter(s),
        })
    }

    pub fn close_signature_help(&mut self, cx: &mut Context<Self>) {
        if self.popups.signature.take().is_some() {
            cx.emit(EditorEvent::SignatureHelpClosed);
            cx.notify();
        }
    }

    fn active_parameter(&self, s: &SignatureState) -> Option<usize> {
        let data = s.data.as_ref()?;
        let sig = data.signatures.get(s.shown)?;
        s.local_parameter
            .or(sig.active_parameter)
            .or(data.active_parameter)
    }

    /// The `(` of the call around the caret and the argument index.
    fn call_at_caret(&self) -> Option<(usize, usize)> {
        let caret = self.editor.primary_selection().head;
        let start = caret.saturating_sub(8192);
        let buffer = self.editor.buffer();
        let start = buffer.clip_offset(start, Bias::Right);
        let before = buffer.text_for_range(start..caret);
        enclosing_call(&before).map(|(p, commas)| (start + p, commas))
    }

    /// Keep open Parameter Info on the call around the caret: update the argument index from the text at once, ask
    /// the owner to refresh, and close when the caret left the call.
    pub(crate) fn signature_follow_caret(&mut self, cx: &mut Context<Self>) {
        let Some(state) = &self.popups.signature else {
            return;
        };
        let anchor = self.editor.buffer().offset_for_anchor(&state.anchor);
        match self.call_at_caret() {
            Some((paren, commas)) if paren == anchor => {
                let state = self.popups.signature.as_mut().expect("checked");
                if state.local_parameter != Some(commas) {
                    state.local_parameter = Some(commas);
                    cx.notify();
                }
                cx.emit(EditorEvent::SignatureHelpTriggered(
                    SignatureTrigger::Retrigger,
                ));
            }
            _ => self.close_signature_help(cx),
        }
    }

    pub(crate) fn signatures_cyclable(&self) -> bool {
        self.popups
            .signature
            .as_ref()
            .and_then(|s| s.data.as_ref())
            .is_some_and(|d| d.signatures.len() > 1)
    }

    pub(crate) fn cycle_signature(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(s) = self.popups.signature.as_mut()
            && let Some(d) = &s.data
        {
            let n = d.signatures.len() as isize;
            s.shown = ((s.shown as isize + delta).rem_euclid(n)) as usize;
            cx.notify();
        }
    }

    // ----- Quick Info -----

    /// Open Quick Info for `offset` (or refresh it) and return the request id to answer with
    /// [`EditorView::set_hover`].
    pub fn open_hover(&mut self, offset: usize, cx: &mut Context<Self>) -> u64 {
        let id = self.popups.next_id();
        let from_mouse = self.popups.mouse_hover.take() == Some(offset);
        let range = self.word_around(offset).unwrap_or(offset..offset);
        let buffer = self.editor.buffer();
        self.popups.hover = Some(HoverState {
            id,
            offset,
            range: buffer.anchor_before(range.start)..buffer.anchor_after(range.end),
            loading: true,
            rendered: None,
            from_mouse,
        });
        cx.notify();
        id
    }

    /// Answer Quick Info request `id` with Markdown; `None` or blank closes it. `range` (offsets) is the text it
    /// describes, which keeps a mouse hover open while the mouse stays on it.
    pub fn set_hover(
        &mut self,
        id: u64,
        markdown_text: Option<&str>,
        range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(hover) = self.popups.hover.as_mut() else {
            return false;
        };
        if hover.id != id {
            return false;
        }
        let rendered = markdown_text
            .map(Rendered::new)
            .filter(|r| !r.plain.trim().is_empty());
        match rendered {
            None => self.close_hover(cx),
            Some(r) => {
                if let Some(range) = range {
                    let buffer = self.editor.buffer();
                    let len = buffer.len();
                    hover.range = buffer.anchor_before(range.start.min(len))
                        ..buffer.anchor_after(range.end.min(len));
                }
                hover.rendered = Some(r);
                hover.loading = false;
                cx.notify();
            }
        }
        true
    }

    /// Quick Info as shown, `None` when closed.
    pub fn hover(&self) -> Option<HoverSnapshot> {
        let h = self.popups.hover.as_ref()?;
        Some(HoverSnapshot {
            id: h.id,
            loading: h.loading,
            visible: h.rendered.is_some(),
            offset: h.offset,
            text: h.rendered.as_ref().map(|r| r.plain.clone()),
        })
    }

    pub fn close_hover(&mut self, cx: &mut Context<Self>) {
        self.popups.hover_timer = None;
        if self.popups.hover.take().is_some() {
            cx.emit(EditorEvent::HoverClosed);
            cx.notify();
        }
    }

    /// The identifier containing `offset` (or ending at it).
    fn word_around(&self, offset: usize) -> Option<Range<usize>> {
        let buffer = self.editor.buffer();
        let point = buffer.offset_to_point(offset);
        let line = buffer.line(point.row);
        let line_start = offset - point.column as usize;
        let col = point.column as usize;
        let start = line[..col]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_identifier_char(*c))
            .last()
            .map_or(col, |(i, _)| i);
        let end = line[col..]
            .char_indices()
            .find(|(_, c)| !is_identifier_char(*c))
            .map_or(line.len(), |(i, _)| col + i);
        (start < end).then(|| line_start + start..line_start + end)
    }

    /// The identifier under a window position, if the mouse is over one of its characters.
    fn word_under(&self, position: Point<Pixels>) -> Option<Range<usize>> {
        let l = self.layout.as_ref()?;
        if !l.bounds.contains(&position) || position.x < l.text_left {
            return None;
        }
        let y = position.y - l.bounds.top() + l.scroll.y;
        let row = (y / l.line_height).floor().max(0.) as u32;
        let (_, text, shaped) = l.rows.iter().find(|(r, _, _)| *r == row)?;
        let x = position.x - l.text_left + l.scroll.x;
        let col = from_display(text, shaped.index_for_x(x)?);
        let c = text[col..].chars().next()?;
        if !is_identifier_char(c) {
            return None;
        }
        let offset = self
            .editor
            .buffer()
            .point_to_offset(text::Point::new(row, col as u32));
        self.word_around(offset)
    }

    /// The mouse moved over the text: start the hover delay over a new word, close a mouse hover it left.
    pub(crate) fn hover_mouse_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let word = self.word_under(position);
        if let Some(h) = &self.popups.hover
            && h.from_mouse
        {
            let buffer = self.editor.buffer();
            let range =
                buffer.offset_for_anchor(&h.range.start)..buffer.offset_for_anchor(&h.range.end);
            let inside = word
                .as_ref()
                .is_some_and(|w| w.start < range.end && range.start < w.end);
            if inside {
                return;
            }
            self.close_hover(cx);
        }
        let Some(word) = word else {
            self.popups.hover_timer = None;
            return;
        };
        if self
            .popups
            .hover_timer
            .as_ref()
            .is_some_and(|(start, _)| *start == word.start)
            || self
                .popups
                .hover
                .as_ref()
                .is_some_and(|h| h.offset >= word.start && h.offset <= word.end)
        {
            return;
        }
        let offset = word.start;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.popups.hover_timer = None;
                this.popups.mouse_hover = Some(offset);
                cx.emit(EditorEvent::HoverTriggered { offset });
            });
        });
        self.popups.hover_timer = Some((offset, task));
    }

    /// The mouse left the editor.
    pub(crate) fn hover_mouse_left(&mut self, cx: &mut Context<Self>) {
        self.popups.hover_timer = None;
        if self.popups.hover.as_ref().is_some_and(|h| h.from_mouse) {
            self.close_hover(cx);
        }
    }

    // ----- drawing -----

    /// Window position of the top-left corner of the character at `offset` at the current scroll position, if its row
    /// is in view.
    fn popup_origin(&self, offset: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let buffer = self.editor.buffer();
        let p = buffer.offset_to_point(offset);
        let y = l.bounds.top() + l.line_height * p.row as f32 - self.scroll_position().y;
        if y + l.line_height < l.bounds.top() || y > l.bounds.bottom() {
            return None;
        }
        let x_in_text = match l.rows.iter().find(|(r, _, _)| *r == p.row) {
            Some((_, text, shaped)) => shaped.x_for_index(to_display(text, p.column as usize)),
            None => l.char_width * visual_column(&buffer.line(p.row), p.column as usize) as f32,
        };
        Some(point(l.text_left - self.scroll_position().x + x_in_text, y))
    }

    pub(crate) fn render_popups(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let Some(lh) = self.layout.as_ref().map(|l| l.line_height) else {
            return out;
        };
        let ui_font = window.text_style().font();
        let mono = gpui::font(self.style().font_family.clone());
        let theme = self.style().theme;
        let buffer = self.editor.buffer();
        if let Some(sig) = &self.popups.signature
            && let Some(data) = &sig.data
            && let Some(origin) = self.popup_origin(buffer.offset_for_anchor(&sig.anchor))
        {
            let shown = sig.shown.min(data.signatures.len() - 1);
            let info = &data.signatures[shown];
            let active = self.active_parameter(sig);
            let highlights: Vec<(Range<usize>, HighlightStyle)> = active
                .and_then(|a| info.parameters.get(a))
                .filter(|r| r.end <= info.label.len())
                .map(|r| {
                    (
                        r.clone(),
                        HighlightStyle {
                            font_weight: Some(FontWeight::BOLD),
                            color: Some(theme.guide.into()),
                            ..Default::default()
                        },
                    )
                })
                .into_iter()
                .collect();
            let mut panel = popup_panel(&theme)
                .id("signature-help")
                .debug_selector(|| "signature-help".into())
                .occlude()
                .font(ui_font.clone())
                .max_w(px(640.))
                .px(px(6.))
                .py(px(3.))
                .flex()
                .flex_col()
                .gap(px(2.));
            let mut head = div().flex().gap(px(6.));
            if data.signatures.len() > 1 {
                head = head.child(
                    div()
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child(format!(
                            "\u{25B2}{} of {}\u{25BC}",
                            shown + 1,
                            data.signatures.len()
                        )),
                );
            }
            head = head.child(
                StyledText::new(SharedString::from(info.label.clone())).with_highlights(highlights),
            );
            panel = panel.child(head);
            if let Some(doc) = &info.documentation {
                let rendered = Rendered::new(doc);
                if !rendered.plain.trim().is_empty() {
                    panel =
                        panel.child(div().text_color(theme.text_muted).child(markdown::render(
                            &rendered.blocks,
                            &theme,
                            &ui_font,
                            &mono,
                        )));
                }
            }
            out.push(
                deferred(
                    anchored()
                        .anchor(gpui::Anchor::BottomLeft)
                        .position(point(origin.x - px(8.), origin.y - px(2.)))
                        .snap_to_window_with_margin(px(4.))
                        .child(panel),
                )
                .with_priority(1)
                .into_any_element(),
            );
        }
        if let Some(hover) = &self.popups.hover
            && let Some(rendered) = &hover.rendered
            && let Some(origin) = self.popup_origin(buffer.offset_for_anchor(&hover.range.start))
        {
            let panel = popup_panel(&theme)
                .id("quick-info")
                .debug_selector(|| "quick-info".into())
                .occlude()
                .font(ui_font.clone())
                .max_w(px(560.))
                .px(px(8.))
                .py(px(6.))
                .child(markdown::render(&rendered.blocks, &theme, &ui_font, &mono));
            out.push(
                deferred(
                    anchored()
                        .position(point(origin.x, origin.y + lh + px(2.)))
                        .snap_to_window_with_margin(px(4.))
                        .child(panel),
                )
                .with_priority(2)
                .into_any_element(),
            );
        }
        if let Some(menu) = &self.popups.completion
            && menu.visible()
            && let Some(origin) = self.popup_origin(buffer.offset_for_anchor(&menu.anchor))
        {
            out.push(
                deferred(
                    anchored()
                        .position(point(origin.x - px(24.), origin.y + lh))
                        .snap_to_window_with_margin(px(4.))
                        .child(self.completion_panel(menu, &ui_font, &mono, cx)),
                )
                .with_priority(3)
                .into_any_element(),
            );
        }
        out
    }

    fn completion_panel(
        &self,
        menu: &CompletionMenu,
        ui_font: &gpui::Font,
        mono: &gpui::Font,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.style().theme;
        let end = (menu.scroll_top + COMPLETION_ROWS).min(menu.matches.len());
        let longest = menu.matches[menu.scroll_top..end]
            .iter()
            .map(|m| menu.items[m.item].label.chars().count())
            .max()
            .unwrap_or(0);
        let width = px((longest as f32 * 7.5 + 48.).clamp(220., 480.));
        let mut list = popup_panel(&theme)
            .font(ui_font.clone())
            .w(width)
            .flex()
            .flex_col();
        for (row, m) in menu.matches[menu.scroll_top..end].iter().enumerate() {
            let index = menu.scroll_top + row;
            let item = &menu.items[m.item];
            let selected = index == menu.selected;
            let el = completion_row(
                &theme,
                SharedString::from(format!("completion-row-{index}")),
                item.kind,
                item.label.clone(),
                &m.highlights,
                selected && !menu.soft,
            );
            let el = if selected && menu.soft {
                el.border_1().border_color(theme.accent)
            } else {
                el
            };
            list = list.child(el.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    let delta = index as isize
                        - this.popups.completion.as_ref().map_or(0, |m| m.selected) as isize;
                    this.move_completion_selection(delta, cx);
                }),
            ));
        }
        if menu.source == Some(CompletionSource::Syntax) {
            list = list.child(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .border_t_1()
                    .border_color(theme.popup_border)
                    .text_size(theme.typography.small)
                    .text_color(theme.text_muted)
                    .child("Identifiers in this file \u{2014} the language server is loading"),
            );
        }
        let mut row = div()
            .id("completion-list")
            .debug_selector(|| "completion-list".into())
            .occlude()
            .flex()
            .items_start()
            .gap(px(2.))
            .child(list);
        let selected = menu.matches.get(menu.selected).map(|m| &menu.items[m.item]);
        if let Some(item) = selected
            && !menu.soft
            && (item.detail.is_some() || item.documentation.is_some())
        {
            let mut doc = popup_panel(&theme)
                .font(ui_font.clone())
                .max_w(px(420.))
                .px(px(8.))
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(4.));
            if let Some(detail) = &item.detail {
                doc = doc.child(
                    div()
                        .font(mono.clone())
                        .child(SharedString::from(detail.clone())),
                );
            }
            if let Some(text) = &item.documentation {
                let rendered = Rendered::new(text);
                doc = doc.child(markdown::render(&rendered.blocks, &theme, ui_font, mono));
            }
            row = row.child(doc);
        }
        row.into_any_element()
    }
}
