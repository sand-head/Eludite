//! Bookmarks (brief 0070): Visual Studio's Toggle Bookmark (Ctrl+K, Ctrl+K), Next and Previous Bookmark (Ctrl+K,
//! Ctrl+N and Ctrl+K, Ctrl+P), Clear Bookmarks (Ctrl+K, Ctrl+L) and the Bookmarks window (View > Bookmark Window,
//! Ctrl+K, Ctrl+W), as `eludite.bookmarks.*`.
//!
//! A bookmark of an open document is an anchor in its buffer, so it follows edits; the editor draws it in the
//! indicator margin (`eludite_editor`'s `bookmarks`). A closed document's bookmarks keep their line. They are kept
//! per workspace in `bookmarks.json` of its state folder (brief 0047), written when they change and when the workspace
//! closes.

use std::path::Path;

use eludite_commands::CommandError;
use eludite_commands::documents as cmd;
use eludite_editor::text::Anchor;
use eludite_ui::{RunCommand, Theme};
use gpui::{
    AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Shell;

/// The file in the workspace's state folder.
pub const FILE: &str = "bookmarks.json";

/// One bookmark.
#[derive(Debug, Clone)]
pub struct Bookmark {
    pub id: u64,
    pub name: String,
    pub path: String,
    /// 1-based; the anchor's line while the document is open.
    pub line: u32,
    pub enabled: bool,
    pub anchor: Option<Anchor>,
}

/// A bookmark as `bookmarks.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBookmark {
    pub name: String,
    pub path: String,
    pub line: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// The bookmarks half of the shell.
pub struct Bookmarks {
    pub list: Vec<Bookmark>,
    next_id: u64,
    /// The bookmark Next and Previous last went to.
    cursor: Option<u64>,
    pub window: Entity<BookmarksWindow>,
}

impl Bookmarks {
    pub fn new(theme: Theme, cx: &mut gpui::App) -> Self {
        Self {
            list: Vec::new(),
            next_id: 1,
            cursor: None,
            window: cx.new(|_| BookmarksWindow {
                rows: Vec::new(),
                theme,
                selected: None,
            }),
        }
    }

    fn add(&mut self, name: Option<String>, path: String, line: u32, enabled: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Bookmark {
            id,
            name: name.unwrap_or_else(|| format!("Bookmark{id}")),
            path,
            line,
            enabled,
            anchor: None,
        });
        id
    }
}

/// A row of the Bookmarks window.
#[derive(Debug, Clone, PartialEq)]
pub struct BookmarkRow {
    pub id: u64,
    pub name: String,
    pub file: String,
    pub path: String,
    pub line: u32,
    pub enabled: bool,
}

/// The Bookmarks window: its rows; a double-click goes to one, the box enables it.
pub struct BookmarksWindow {
    pub rows: Vec<BookmarkRow>,
    theme: Theme,
    selected: Option<u64>,
}

/// Debug selector of a bookmark's row.
pub fn row_selector(id: u64) -> String {
    format!("bookmark-row-{id}")
}

/// Debug selector of a bookmark's enable box.
pub fn check_selector(id: u64) -> String {
    format!("bookmark-check-{id}")
}

impl Render for BookmarksWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = div()
            .flex()
            .flex_row()
            .px_2()
            .py_1()
            .gap_2()
            .text_color(t.text_muted)
            .border_b_1()
            .border_color(t.border)
            .child(div().w(px(24.)))
            .child(div().w(px(160.)).child("Bookmark"))
            .child(div().flex_1().child("File Location"))
            .child(div().w(px(60.)).child("Line Number"));
        let rows = self.rows.iter().map(|r| {
            let id = r.id;
            let selected = self.selected == Some(id);
            let check = div()
                .id(SharedString::from(check_selector(id)))
                .debug_selector(move || check_selector(id))
                .w(px(24.))
                .cursor_pointer()
                .child(if r.enabled { "\u{2611}" } else { "\u{2610}" })
                .on_click(cx.listener(move |this, _, window, cx| {
                    let enabled = this
                        .rows
                        .iter()
                        .find(|r| r.id == id)
                        .is_some_and(|r| r.enabled);
                    window.dispatch_action(
                        Box::new(RunCommand::new(
                            cmd::BOOKMARK_UPDATE,
                            json!({ "id": id, "enabled": !enabled }),
                        )),
                        cx,
                    );
                }));
            let row = div()
                .id(SharedString::from(row_selector(id)))
                .debug_selector(move || row_selector(id))
                .flex()
                .flex_row()
                .px_2()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .child(check)
                .child(div().w(px(160.)).child(r.name.clone()))
                .child(div().flex_1().child(r.file.clone()))
                .child(div().w(px(60.)).child(r.line.to_string()))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    this.selected = Some(id);
                    if e.click_count() >= 2 {
                        window.dispatch_action(
                            Box::new(RunCommand::new(cmd::BOOKMARK_GO_TO, json!({ "id": id }))),
                            cx,
                        );
                    }
                    cx.notify();
                }));
            if selected { row.bg(t.menu_hover) } else { row }
        });
        div()
            .id("bookmarks-window")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(header)
            .children(rows)
    }
}

fn failed(message: impl Into<String>) -> CommandError {
    CommandError::Failed(message.into())
}

impl Shell {
    /// The bookmark's line now.
    fn bookmark_line(&self, b: &Bookmark, cx: &gpui::App) -> u32 {
        match (&b.anchor, self.documents.get(&b.path)) {
            (Some(a), Some(doc)) => {
                let buffer = doc.view.read(cx).editor().buffer();
                buffer.offset_to_point(buffer.offset_for_anchor(a)).row + 1
            }
            _ => b.line,
        }
    }

    fn bookmark_json(&self, b: &Bookmark, cx: &gpui::App) -> Value {
        json!({
            "id": b.id,
            "name": b.name,
            "path": b.path,
            "line": self.bookmark_line(b, cx),
            "enabled": b.enabled,
        })
    }

    /// Draw a document's bookmarks in its margin and refresh the window's rows.
    fn bookmarks_sync(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let paths: Vec<String> = match path {
            Some(p) => vec![p.to_owned()],
            None => self.documents.keys().cloned().collect(),
        };
        for p in paths {
            let Some(view) = self.documents.get(&p).map(|d| d.view.clone()) else {
                continue;
            };
            let rows: Vec<(u32, bool)> = self
                .bookmarks
                .list
                .iter()
                .filter(|b| b.path == p)
                .map(|b| (self.bookmark_line(b, cx).saturating_sub(1), b.enabled))
                .collect();
            view.update(cx, |v, cx| v.set_bookmark_rows(rows, cx));
        }
        let rows: Vec<BookmarkRow> = self
            .bookmarks
            .list
            .iter()
            .map(|b| BookmarkRow {
                id: b.id,
                name: b.name.clone(),
                file: Path::new(&b.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| b.path.clone()),
                path: b.path.clone(),
                line: self.bookmark_line(b, cx),
                enabled: b.enabled,
            })
            .collect();
        self.bookmarks.window.update(cx, |w, cx| {
            w.rows = rows;
            cx.notify();
        });
    }

    /// `eludite.bookmarks.toggle`.
    pub(super) fn bookmark_toggle(
        &mut self,
        path: Option<&str>,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let id = self.document_id(path)?;
        let doc = &self.documents[&id];
        let (anchor, line) = {
            let e = doc.view.read(cx).editor();
            let b = e.buffer();
            let row = match line {
                Some(l) => l.saturating_sub(1).min(b.line_count().saturating_sub(1)),
                None => b.offset_to_point(e.primary_selection().head).row,
            };
            let start = b.point_to_offset(eludite_editor::text::Point::new(row, 0));
            (b.anchor_after(start), row + 1)
        };
        let existing = self
            .bookmarks
            .list
            .iter()
            .position(|b| b.path == id && self.bookmark_line(b, cx) == line);
        let out = match existing {
            Some(ix) => {
                let b = self.bookmarks.list.remove(ix);
                json!({ "path": id, "line": line, "added": false, "id": b.id })
            }
            None => {
                let bid = self.bookmarks.add(None, id.clone(), line, true);
                if let Some(b) = self.bookmarks.list.last_mut() {
                    b.anchor = Some(anchor);
                }
                json!({ "path": id, "line": line, "added": true, "id": bid })
            }
        };
        self.bookmarks_sync(Some(&id), cx);
        self.bookmarks_schedule_save(cx);
        Ok(out)
    }

    /// `eludite.bookmarks.next` and `previous`: the next enabled bookmark in the window's order, wrapping; from the
    /// caret's position when none was gone to yet.
    pub(super) fn bookmark_step(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let enabled: Vec<(u64, String, u32)> = self
            .bookmarks
            .list
            .iter()
            .filter(|b| b.enabled)
            .map(|b| (b.id, b.path.clone(), self.bookmark_line(b, cx)))
            .collect();
        if enabled.is_empty() {
            return Ok(json!({}));
        }
        let n = enabled.len();
        let at = self
            .bookmarks
            .cursor
            .and_then(|c| enabled.iter().position(|(id, _, _)| *id == c));
        let ix = match at {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => {
                // From the caret: the first bookmark after it in the active document, else the first one.
                let here = self.caret_entry(cx);
                let pos = here.as_ref().and_then(|h| {
                    if forward {
                        enabled
                            .iter()
                            .position(|(_, p, l)| *p == h.path && *l > h.line)
                    } else {
                        enabled
                            .iter()
                            .rposition(|(_, p, l)| *p == h.path && *l < h.line)
                    }
                });
                pos.unwrap_or(if forward { 0 } else { n - 1 })
            }
        };
        let (id, path, line) = enabled[ix].clone();
        self.bookmarks.cursor = Some(id);
        self.bookmark_open(&path, line, window, cx)?;
        Ok(json!({ "path": path, "line": line, "column": 1 }))
    }

    fn bookmark_open(
        &mut self,
        path: &str,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        if let Some(here) = self.caret_entry(cx) {
            self.navigation.history.push(here);
        }
        self.open_file(path, Some((line, 1)), window, cx)
            .map(|_| ())
    }

    /// `eludite.bookmarks.clear`.
    pub(super) fn bookmark_clear(
        &mut self,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let filter = path.map(|p| {
            if self.documents.contains_key(p) {
                p.to_owned()
            } else {
                self.resolve_file(p).to_string_lossy().into_owned()
            }
        });
        let before = self.bookmarks.list.len();
        self.bookmarks
            .list
            .retain(|b| filter.as_ref().is_some_and(|f| *f != b.path));
        let removed = before - self.bookmarks.list.len();
        self.bookmarks.cursor = None;
        self.bookmarks_sync(None, cx);
        self.bookmarks_schedule_save(cx);
        Ok(json!({ "removed": removed }))
    }

    /// `eludite.bookmarks.list`.
    pub(super) fn bookmark_list(&mut self, cx: &mut Context<Self>) -> Value {
        let rows: Vec<Value> = self
            .bookmarks
            .list
            .iter()
            .map(|b| self.bookmark_json(b, cx))
            .collect();
        json!({ "bookmarks": rows })
    }

    /// `eludite.bookmarks.update`.
    pub(super) fn bookmark_update(
        &mut self,
        id: u64,
        name: Option<String>,
        enabled: Option<bool>,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let b = self
            .bookmarks
            .list
            .iter_mut()
            .find(|b| b.id == id)
            .ok_or_else(|| failed(format!("no bookmark {id}")))?;
        if let Some(n) = name {
            b.name = n;
        }
        if let Some(e) = enabled {
            b.enabled = e;
        }
        let path = b.path.clone();
        self.bookmarks_sync(Some(&path), cx);
        self.bookmarks_schedule_save(cx);
        let b = self
            .bookmarks
            .list
            .iter()
            .find(|b| b.id == id)
            .expect("found above");
        Ok(self.bookmark_json(b, cx))
    }

    /// `eludite.bookmarks.go_to`.
    pub(super) fn bookmark_go_to(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let (path, line) = {
            let b = self
                .bookmarks
                .list
                .iter()
                .find(|b| b.id == id)
                .ok_or_else(|| failed(format!("no bookmark {id}")))?;
            (b.path.clone(), self.bookmark_line(b, cx))
        };
        self.bookmarks.cursor = Some(id);
        self.bookmark_open(&path, line, window, cx)?;
        Ok(json!({ "path": path, "line": line }))
    }

    /// A document opened: anchor its bookmarks in its buffer and draw them.
    pub(super) fn bookmarks_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let view = doc.view.clone();
        {
            let b = view.read(cx).editor().buffer();
            let last = b.line_count().saturating_sub(1);
            for bm in self.bookmarks.list.iter_mut().filter(|bm| bm.path == id) {
                let row = bm.line.saturating_sub(1).min(last);
                let start = b.point_to_offset(eludite_editor::text::Point::new(row, 0));
                bm.anchor = Some(b.anchor_after(start));
            }
        }
        self.bookmarks_sync(Some(id), cx);
    }

    /// A document is closing: its bookmarks keep the lines they are on now.
    pub(super) fn bookmarks_document_closing(&mut self, id: &str, cx: &mut Context<Self>) {
        let lines: Vec<(u64, u32)> = self
            .bookmarks
            .list
            .iter()
            .filter(|b| b.path == id)
            .map(|b| (b.id, self.bookmark_line(b, cx)))
            .collect();
        for (bid, line) in lines {
            if let Some(b) = self.bookmarks.list.iter_mut().find(|b| b.id == bid) {
                b.line = line;
                b.anchor = None;
            }
        }
        self.bookmarks_schedule_save(cx);
    }

    /// A document moved: its bookmarks follow it.
    pub(super) fn bookmarks_document_renamed(
        &mut self,
        old: &str,
        new: &str,
        cx: &mut Context<Self>,
    ) {
        for b in self.bookmarks.list.iter_mut().filter(|b| b.path == old) {
            b.path = new.to_owned();
        }
        self.bookmarks_sync(Some(new), cx);
        self.bookmarks_schedule_save(cx);
    }

    fn bookmarks_file(&self) -> Option<std::path::PathBuf> {
        let root = self.doc_session.root.as_ref()?;
        Some(self.workspace_state_dir(root)?.join(FILE))
    }

    fn bookmarks_stored(&self, cx: &gpui::App) -> Vec<StoredBookmark> {
        self.bookmarks
            .list
            .iter()
            .map(|b| StoredBookmark {
                name: b.name.clone(),
                path: b.path.clone(),
                line: self.bookmark_line(b, cx),
                enabled: b.enabled,
            })
            .collect()
    }

    /// Write `bookmarks.json` off the UI thread.
    fn bookmarks_schedule_save(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.bookmarks_file() else {
            return;
        };
        let stored = self.bookmarks_stored(cx);
        cx.background_spawn(async move {
            if let Err(e) = write_json(&file, &stored) {
                eprintln!("eludite: {}: {e}", file.display());
            }
        })
        .detach();
    }

    /// Write `bookmarks.json` now (quitting, closing the workspace).
    pub(super) fn bookmarks_save_now(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.bookmarks_file() else {
            return;
        };
        if let Err(e) = write_json(&file, &self.bookmarks_stored(cx)) {
            eprintln!("eludite: {}: {e}", file.display());
        }
    }

    /// A workspace opened: read its bookmarks off the UI thread.
    pub(super) fn bookmarks_load(
        &mut self,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bookmarks.list.clear();
        self.bookmarks.cursor = None;
        self.bookmarks_sync(None, cx);
        let Some(dir) = self.workspace_state_dir(root) else {
            return;
        };
        let root = root.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let stored: Vec<StoredBookmark> = cx
                .background_spawn(async move {
                    std::fs::read(dir.join(FILE))
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                        .unwrap_or_default()
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.doc_session.root.as_deref() != Some(root.as_path()) {
                    return;
                }
                for s in stored {
                    shell
                        .bookmarks
                        .add(Some(s.name), s.path, s.line.max(1), s.enabled);
                }
                let open: Vec<String> = shell.documents.keys().cloned().collect();
                for id in open {
                    shell.bookmarks_document_opened(&id, cx);
                }
                shell.bookmarks_sync(None, cx);
            });
        })
        .detach();
    }
}

fn write_json(file: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_bookmarks_default_to_enabled() {
        let s: Vec<StoredBookmark> =
            serde_json::from_str(r#"[{"name": "B", "path": "/a.cs", "line": 2}]"#).unwrap();
        assert!(s[0].enabled);
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x").join(FILE);
        write_json(&f, &s).unwrap();
        let back: Vec<StoredBookmark> =
            serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
