//! Reload on external change (brief 0070): a file an agent, a git checkout or another editor changes under an open
//! document.
//!
//! - **Watching.** Every [`POLL`] and whenever the window becomes active, the open documents' files are stat-ed off
//!   the UI thread (modification time and length; no `notify`, as brief 0040's git watcher). A changed stamp is
//!   compared with the one the document last read or wrote.
//! - **A clean document** reloads in place (`files.autoReload`, on): the file is read and decoded off the UI thread,
//!   then the editor applies one edit replacing the text that differs, so caret, selections and scroll stay where
//!   they were. Off, it asks as a dirty one does.
//! - **A dirty document** asks Visual Studio's "This file has been modified outside of the source editor. Do you want
//!   to reload it?" (Yes, Yes to All, No, No to All). No remembers the stamp: the next save overwrites that version.
//! - **A deleted file** marks its tab `[deleted]`; the buffer is kept, and saving writes the file again.
//! - **Autosave** (`files.autoSave`): `afterDelay` saves a document `files.autoSaveDelay` ms after its last edit,
//!   `onFocusChange` saves every dirty document when the window loses focus.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eludite_commands::CommandError;
use eludite_editor::Buffer;
use gpui::{AppContext as _, Context, PromptLevel, Task, Window};

use super::Shell;

/// How often the open documents' files are stat-ed.
pub const POLL: Duration = Duration::from_secs(1);

/// A file's modification time and length: what says it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl DiskStamp {
    /// The stamp of the file at `path`, or `None` when it does not exist.
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        m.is_file().then(|| DiskStamp {
            modified: m.modified().ok(),
            len: m.len(),
        })
    }
}

/// The question's answers, as Visual Studio words them.
pub const ANSWERS: [&str; 4] = ["Yes", "Yes to All", "No", "No to All"];

/// The reload half of the shell.
#[derive(Default)]
pub struct ReloadState {
    /// Documents being read for a reload.
    pub reloading: HashSet<String>,
    /// Documents waiting for the question, in the order their changes were seen.
    pub questions: Vec<String>,
    /// The question is on screen.
    pub asking: bool,
    /// A stat pass is in flight.
    polling: bool,
    /// Pending autosaves by document.
    autosave: std::collections::HashMap<String, Task<()>>,
    /// How many reloads were applied (tests and the report).
    pub reloads: u64,
}

impl Shell {
    /// Brief 0070: start watching (call once from `new`).
    pub(super) fn reload_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this
                    .update_in(cx, |shell, window, cx| shell.reload_poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(poll);
        // Stat on focus, and the onFocusChange autosave on losing it.
        let activation = cx.observe_window_activation(window, |shell, window, cx| {
            if window.is_window_active() {
                shell.reload_poll(window, cx);
            } else if shell.settings.lock().string("files.autoSave") == "onFocusChange" {
                shell.autosave_all(cx);
            }
        });
        activation.detach();
    }

    /// The open files to stat: (id, path).
    fn watch_list(&self) -> Vec<(String, PathBuf)> {
        self.documents
            .iter()
            .filter(|(id, d)| !d.untitled && !d.read_only && !self.reload.reloading.contains(*id))
            .map(|(id, d)| (id.clone(), d.path.clone()))
            .collect()
    }

    /// Stat every open file off the UI thread, then act on what changed.
    pub(super) fn reload_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reload.polling {
            return;
        }
        let list = self.watch_list();
        if list.is_empty() {
            return;
        }
        self.reload.polling = true;
        cx.spawn_in(window, async move |this, cx| {
            let stamps = cx
                .background_spawn(async move {
                    list.into_iter()
                        .map(|(id, path)| {
                            let stamp = DiskStamp::of(&path);
                            (id, stamp)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.reload.polling = false;
                shell.disk_stamps(stamps, window, cx);
            });
        })
        .detach();
    }

    /// What the stat pass found.
    fn disk_stamps(
        &mut self,
        stamps: Vec<(String, Option<DiskStamp>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let auto = self.settings.lock().bool("files.autoReload");
        let mut ask = Vec::new();
        for (id, stamp) in stamps {
            let Some(doc) = self.documents.get_mut(&id) else {
                continue;
            };
            if self.reload.reloading.contains(&id) || doc.disk == stamp {
                continue;
            }
            let Some(stamp) = stamp else {
                if !doc.deleted {
                    doc.deleted = true;
                    self.controller.set_document_deleted(&id, true);
                }
                continue;
            };
            if std::mem::take(&mut doc.deleted) {
                self.controller.set_document_deleted(&id, false);
            }
            if doc.declined == Some(stamp) {
                continue;
            }
            if !doc.dirty && auto {
                self.start_reload(&id, None, window, cx);
            } else if !self.reload.questions.contains(&id) {
                ask.push(id);
            }
        }
        self.reload.questions.extend(ask);
        self.ask_reload(window, cx);
        cx.notify();
    }

    /// Read the file off the UI thread and apply it; `reply` hears the outcome.
    pub(super) fn start_reload(
        &mut self,
        id: &str,
        reply: Option<Box<dyn FnOnce(Result<serde_json::Value, CommandError>)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.get(id) else {
            if let Some(r) = reply {
                r(Err(CommandError::Failed(format!("{id} is not open"))));
            }
            return;
        };
        let path = doc.path.clone();
        let fallback = self.encoding_fallback();
        self.reload.reloading.insert(id.to_owned());
        let id = id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let (stamp, loaded) = cx
                .background_spawn(async move {
                    let stamp = DiskStamp::of(&path);
                    let loaded = stamp.map(|_| Buffer::load_with(&path, fallback));
                    (stamp, loaded)
                })
                .await;
            let _ = this.update_in(cx, |shell, _, cx| {
                shell.reload.reloading.remove(&id);
                let out = match loaded {
                    Some(Ok(buffer)) => {
                        let reloaded = shell.apply_reload(&id, buffer, stamp, cx);
                        shell.reload.reloads += 1;
                        let dirty = shell.documents.get(&id).is_some_and(|d| d.dirty);
                        Ok(serde_json::json!({
                            "path": id, "reloaded": reloaded, "dirty": dirty, "deleted": false
                        }))
                    }
                    Some(Err(e)) => Err(CommandError::Failed(format!("{id}: {e}"))),
                    None => {
                        if let Some(doc) = shell.documents.get_mut(&id)
                            && !doc.deleted
                        {
                            doc.deleted = true;
                            shell.controller.set_document_deleted(&id, true);
                        }
                        let dirty = shell.documents.get(&id).is_some_and(|d| d.dirty);
                        Ok(serde_json::json!({
                            "path": id, "reloaded": false, "dirty": dirty, "deleted": true
                        }))
                    }
                };
                if let Some(r) = reply {
                    r(out);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show the question for the next document waiting for it.
    fn ask_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reload.asking {
            return;
        }
        self.reload
            .questions
            .retain(|id| self.documents.contains_key(id));
        let Some(id) = self.reload.questions.first().cloned() else {
            return;
        };
        self.reload.asking = true;
        let detail = format!(
            "{id}\n\nThis file has been modified outside of the source editor. Do you want to reload it?"
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "File Modification Detected",
            Some(&detail),
            &ANSWERS,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.reload.asking = false;
                let all = matches!(answer, Some(1) | Some(3));
                let reload = matches!(answer, Some(0) | Some(1));
                let ids: Vec<String> = if all {
                    std::mem::take(&mut shell.reload.questions)
                } else {
                    shell
                        .reload
                        .questions
                        .first()
                        .cloned()
                        .into_iter()
                        .collect()
                };
                shell.reload.questions.retain(|q| !ids.contains(q));
                for id in ids {
                    if reload {
                        shell.start_reload(&id, None, window, cx);
                    } else if let Some(doc) = shell.documents.get_mut(&id) {
                        // No: keep the buffer; the next save overwrites this version without asking.
                        doc.declined = DiskStamp::of(&doc.path);
                    }
                }
                shell.ask_reload(window, cx);
            });
        })
        .detach();
    }

    /// `eludite.file.reload`.
    pub(super) fn reload_command(
        &mut self,
        path: Option<&str>,
        discard: bool,
        reply: Box<dyn FnOnce(Result<serde_json::Value, CommandError>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = match self.document_id(path) {
            Ok(id) => id,
            Err(e) => return reply(Err(e)),
        };
        let doc = &self.documents[&id];
        if doc.untitled {
            return reply(Err(CommandError::Failed(format!(
                "{id} has no file to reload"
            ))));
        }
        if doc.dirty && !discard {
            return reply(Err(CommandError::Failed(format!(
                "{id} has unsaved changes; pass \"discard\": true to drop them"
            ))));
        }
        self.reload.questions.retain(|q| q != &id);
        self.start_reload(&id, Some(reply), window, cx);
    }

    /// Brief 0070's autosave after an edit (`files.autoSave: afterDelay`).
    pub(super) fn autosave_after_edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let (mode, delay) = {
            let s = self.settings.lock();
            (
                s.string("files.autoSave"),
                s.effective("files.autoSaveDelay")
                    .0
                    .as_u64()
                    .unwrap_or(1000),
            )
        };
        if mode != "afterDelay" {
            return;
        }
        let task_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.reload.autosave.remove(&task_id);
                if shell
                    .documents
                    .get(&task_id)
                    .is_some_and(|d| d.dirty && !d.untitled && !d.deleted)
                {
                    let _ = shell.save(Some(&task_id), cx);
                }
            });
        });
        self.reload.autosave.insert(id.to_owned(), task);
    }

    /// Save every dirty text document that has a file (autosave on focus change).
    fn autosave_all(&mut self, cx: &mut Context<Self>) {
        let ids: BTreeSet<String> = self
            .documents
            .iter()
            .filter(|(_, d)| d.dirty && !d.untitled && !d.deleted && !d.read_only)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.save(Some(&id), cx);
        }
    }
}

/// The edit that turns `old` into `new`: the range of `old` between their common prefix and suffix, and its
/// replacement. `None` when they are equal. Offsets are on character boundaries.
pub fn minimal_edit(old: &str, new: &str) -> Option<(std::ops::Range<usize>, String)> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some((
        prefix..old.len() - suffix,
        new[prefix..new.len() - suffix].to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_edits_replace_only_what_differs() {
        assert_eq!(minimal_edit("abc", "abc"), None);
        assert_eq!(minimal_edit("abc", "abXc"), Some((2..2, "X".into())));
        assert_eq!(
            minimal_edit("a\nb\nc\n", "a\nc\n"),
            Some((2..4, String::new()))
        );
        assert_eq!(minimal_edit("", "x"), Some((0..0, "x".into())));
        assert_eq!(minimal_edit("aaa", "aa"), Some((2..3, String::new())));
        // Multi-byte characters stay whole.
        let (range, text) = minimal_edit("é", "è").unwrap();
        assert_eq!((range, text.as_str()), (0..2, "è"));
    }

    #[test]
    fn stamps_see_a_change_and_a_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cs");
        assert_eq!(DiskStamp::of(&p), None);
        std::fs::write(&p, "a").unwrap();
        let a = DiskStamp::of(&p).unwrap();
        std::fs::write(&p, "ab").unwrap();
        assert_ne!(DiskStamp::of(&p), Some(a), "the length changed");
        assert_eq!(DiskStamp::of(dir.path()), None, "a folder is not a file");
    }
}
