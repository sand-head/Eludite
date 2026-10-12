//! Documents reopened on restart (brief 0070; Visual Studio's `.suo` role): per workspace, the open documents in tab
//! order with their carets and pins, and the active one, in `documents.json` of the workspace's state folder
//! (`<config>/eludite/workspaces/<name>-<hash>/`, brief 0047).
//!
//! - **Writing**: after a tab opens, closes, moves or is activated (debounced, written off the UI thread), and
//!   synchronously when Eludite quits or the workspace closes.
//! - **Restoring**: when a workspace opens (`files.restoreDocuments`, on), the file is read off the UI thread; the
//!   active document loads at once, the others become tabs whose files are read when first shown, so 20 restored
//!   documents cost one read before their tabs are drawn. Files that no longer exist are skipped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{AppContext as _, Context, Task, Window};
use serde::{Deserialize, Serialize};

use super::Shell;

/// The file in the workspace's state folder.
pub const FILE: &str = "documents.json";
/// Quiet time after a tab change before the file is written.
pub const WRITE_DEBOUNCE: Duration = Duration::from_millis(500);

/// One document of `documents.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDocument {
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based.
    pub column: u32,
    #[serde(default)]
    pub pinned: bool,
}

/// `documents.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSession {
    pub version: u32,
    pub documents: Vec<StoredDocument>,
    #[serde(default)]
    pub active: Option<String>,
}

impl StoredSession {
    pub const VERSION: u32 = 1;

    /// Read a session file; anything unreadable is an empty session.
    pub fn read(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<StoredSession>(&b).ok())
            .filter(|s| s.version == Self::VERSION)
            .unwrap_or_default()
    }

    /// Write it, creating the folder (mode 0700 on Unix, as brief 0047's state folders).
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

/// The session half of the shell.
#[derive(Default)]
pub struct SessionState {
    /// The workspace whose documents are tracked.
    pub root: Option<PathBuf>,
    /// Restored tabs whose files are read when first shown: their carets.
    pub lazy: HashMap<String, (u32, u32)>,
    write: Option<Task<()>>,
    /// The last active document seen, to load a lazy tab when it becomes active.
    last_active: Option<String>,
    /// Restores applied (tests).
    pub restored: u64,
}

impl Shell {
    /// The workspace state folder of `root`, when state persists.
    pub(super) fn workspace_state_dir(&self, root: &Path) -> Option<PathBuf> {
        let state_root = self.docs.state_root.as_ref()?;
        Some(crate::settings::workspace_state_dir(state_root, root))
    }

    /// Watch the workspace and the tabs (call once, from `documents_install`).
    pub(super) fn session_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changes = self.controller.subscribe();
        let task = cx.spawn_in(window, async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.session_layout_changed(window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(task);
        let this = cx.entity();
        cx.observe_in(&this, window, |shell, _, window, cx| {
            shell.session_watch_workspace(window, cx)
        })
        .detach();
    }

    /// The shell changed: when the workspace did, write the old one's documents and restore the new one's.
    fn session_watch_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.workspace_root();
        if root == self.doc_session.root {
            return;
        }
        if self.doc_session.root.is_some() {
            self.session_save_now(cx);
        }
        self.doc_session.root = root.clone();
        self.doc_session.lazy.clear();
        let Some(root) = root else {
            return;
        };
        self.bookmarks_load(&root, window, cx);
        if self.settings.lock().bool("files.restoreDocuments") {
            self.session_restore(&root, window, cx);
        }
    }

    /// Read `root`'s session off the UI thread and reopen its documents.
    pub(super) fn session_restore(
        &mut self,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.workspace_state_dir(root) else {
            return;
        };
        let root = root.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let stored = cx
                .background_spawn(async move {
                    let mut s = StoredSession::read(&dir.join(FILE));
                    s.documents.retain(|d| Path::new(&d.path).is_file());
                    s
                })
                .await;
            let _ = this.update_in(cx, |shell, window, cx| {
                if shell.doc_session.root.as_deref() != Some(root.as_path()) {
                    return; // Another workspace opened meanwhile.
                }
                shell.session_apply(stored, window, cx);
            });
        })
        .detach();
    }

    /// Reopen a stored session's documents: tabs at once, the active one's file read now, the rest when shown.
    pub(super) fn session_apply(
        &mut self,
        stored: StoredSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stored.documents.is_empty() {
            return;
        }
        let previously = self.controller.active_document();
        for d in &stored.documents {
            if self.documents.contains_key(&d.path) || self.loading.contains_key(&d.path) {
                continue;
            }
            let title = super::navigation::document_title(Path::new(&d.path));
            self.controller.open_document(&d.path, &title);
            if d.pinned {
                self.controller.set_document_pinned(&d.path, true);
            }
            self.doc_session
                .lazy
                .insert(d.path.clone(), (d.line, d.column));
        }
        // A document the person opened meanwhile stays active; else the stored one.
        let active = previously
            .filter(|p| self.documents.contains_key(p) || self.loading.contains_key(p))
            .or(stored.active)
            .filter(|a| self.controller.layout().documents.get(a).is_some())
            .or_else(|| stored.documents.first().map(|d| d.path.clone()));
        if let Some(active) = active {
            self.controller.activate_document(&active);
            self.session_load_lazy(&active, window, cx);
        }
        self.doc_session.restored += 1;
        cx.notify();
    }

    /// Load a restored tab's file if it has not been read yet.
    fn session_load_lazy(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(caret) = self.doc_session.lazy.remove(id) else {
            return;
        };
        if self.documents.contains_key(id) || self.loading.contains_key(id) {
            return;
        }
        if let Err(e) = self.open_file(id, Some(caret), window, cx) {
            self.status
                .set(eludite_ui::slots::STATE, format!("Cannot reopen {id}: {e}"));
            self.controller.close_document(id);
        }
    }

    /// The tabs changed: load a restored tab that became active, and write the session soon.
    pub(super) fn session_layout_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.controller.layout();
        self.doc_session
            .lazy
            .retain(|id, _| layout.documents.get(id).is_some());
        let active = layout.documents.active.clone();
        if active != self.doc_session.last_active {
            self.doc_session.last_active = active.clone();
            if let Some(a) = &active {
                self.session_load_lazy(a, window, cx);
            }
            self.update_document_status(cx);
        }
        self.session_schedule_write(cx);
    }

    /// What `documents.json` holds now.
    pub(super) fn session_snapshot(&self, cx: &gpui::App) -> StoredSession {
        let layout = self.controller.layout();
        let documents =
            layout
                .documents
                .tabs
                .iter()
                .filter_map(|t| {
                    if let Some(doc) = self.documents.get(&t.id) {
                        if doc.untitled || doc.read_only {
                            return None;
                        }
                        let head = doc.view.read(cx).editor().primary_selection().head;
                        let (line, column) = super::intellisense::line_column(&doc.view, head, cx);
                        return Some(StoredDocument {
                            path: t.id.clone(),
                            line,
                            column,
                            pinned: t.pinned,
                        });
                    }
                    let (line, column) =
                        self.doc_session.lazy.get(&t.id).copied().or_else(|| {
                            self.loading.get(&t.id).map(|(_, c)| c.unwrap_or((1, 1)))
                        })?;
                    Some(StoredDocument {
                        path: t.id.clone(),
                        line,
                        column,
                        pinned: t.pinned,
                    })
                })
                .collect::<Vec<_>>();
        let active = layout
            .documents
            .active
            .clone()
            .filter(|a| documents.iter().any(|d| d.path == *a));
        StoredSession {
            version: StoredSession::VERSION,
            documents,
            active,
        }
    }

    fn session_file(&self) -> Option<PathBuf> {
        let root = self.doc_session.root.as_ref()?;
        Some(self.workspace_state_dir(root)?.join(FILE))
    }

    fn session_schedule_write(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self.session_file() else {
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WRITE_DEBOUNCE).await;
            let Ok(snapshot) = this.update(cx, |shell, cx| shell.session_snapshot(cx)) else {
                return;
            };
            cx.background_spawn(async move {
                if let Err(e) = snapshot.write(&file) {
                    eprintln!("eludite: {}: {e}", file.display());
                }
            })
            .await;
        });
        self.doc_session.write = Some(task);
    }

    /// Write the session now (quitting, closing the workspace).
    pub(super) fn session_save_now(&mut self, cx: &mut Context<Self>) {
        self.doc_session.write = None;
        let Some(file) = self.session_file() else {
            return;
        };
        if let Err(e) = self.session_snapshot(cx).write(&file) {
            eprintln!("eludite: {}: {e}", file.display());
        }
        self.bookmarks_save_now(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_and_unreadable_files_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ws").join(FILE);
        assert_eq!(StoredSession::read(&file), StoredSession::default());
        let s = StoredSession {
            version: StoredSession::VERSION,
            documents: vec![StoredDocument {
                path: "/a/b.cs".into(),
                line: 3,
                column: 5,
                pinned: true,
            }],
            active: Some("/a/b.cs".into()),
        };
        s.write(&file).unwrap();
        assert_eq!(StoredSession::read(&file), s);
        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(StoredSession::read(&file), StoredSession::default());
        std::fs::write(&file, r#"{"version": 99, "documents": []}"#).unwrap();
        assert_eq!(StoredSession::read(&file), StoredSession::default());
    }
}
