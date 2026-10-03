//! A file's two texts for Compare with Unmodified and the editor's change margin: the old side from the index,
//! HEAD or a revision, the new side from the working tree (or the index). The line diff itself is the caller's
//! (`eludite_ui::diff::diff_lines`, the editor's own), so both views compare lines the same way.

use std::path::Path;

use crate::{ErrorKind, GitError, Repo, Result};

/// What the old side is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Against {
    Index,
    Head,
    Revision(String),
}

impl Against {
    /// `index`, `head`, or a revision.
    pub fn parse(s: &str) -> Self {
        match s {
            "index" => Against::Index,
            "head" | "HEAD" => Against::Head,
            other => Against::Revision(other.to_owned()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Against::Index => "index".into(),
            Against::Head => "HEAD".into(),
            Against::Revision(r) => r.clone(),
        }
    }
}

/// The two texts of a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffTexts {
    pub path: String,
    pub old: String,
    pub new: String,
    /// Either side is binary (its text is left empty).
    pub binary: bool,
    pub old_label: String,
    pub new_label: String,
}

/// Bytes looked at for a NUL, as git decides a file is binary.
const BINARY_PROBE: usize = 8000;

fn text_of(bytes: &[u8]) -> Option<String> {
    let probe = &bytes[..bytes.len().min(BINARY_PROBE)];
    if probe.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

impl Repo {
    /// The index's content of `rel` (stage 0; for a conflicted file, "ours"), or `None` when it is not in the index.
    pub fn index_blob(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let repo = self.repository()?;
        let index = repo.index()?;
        let entry = index
            .get_path(Path::new(rel), 0)
            .or_else(|| index.get_path(Path::new(rel), 2));
        match entry {
            Some(e) => Ok(Some(repo.find_blob(e.id)?.content().to_vec())),
            None => Ok(None),
        }
    }

    /// The content of `rel` at `rev`, or `None` when the revision has no such file (or HEAD is unborn).
    pub fn revision_blob(&self, rev: &str, rel: &str) -> Result<Option<Vec<u8>>> {
        let repo = self.repository()?;
        let tree = match repo.revparse_single(rev) {
            Ok(o) => o.peel_to_tree()?,
            Err(e) if rev == "HEAD" && e.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(e) => {
                return Err(GitError::new(
                    ErrorKind::NotFound,
                    format!("unknown revision `{rev}`: {}", e.message()),
                ));
            }
        };
        match tree.get_path(Path::new(rel)) {
            Ok(entry) => Ok(Some(repo.find_blob(entry.id())?.content().to_vec())),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The index's text of `rel` for the editor's change margin: `None` when the file is not in the index or is
    /// binary.
    pub fn index_text(&self, rel: &str) -> Result<Option<String>> {
        Ok(self.index_blob(rel)?.and_then(|b| text_of(&b)))
    }

    /// The old and new texts of `path` (relative or absolute): the working tree's (the index's when `staged`)
    /// against `against` (`staged` with [`Against::Index`] compares the index with HEAD).
    pub fn diff_texts(&self, path: &str, against: &Against, staged: bool) -> Result<DiffTexts> {
        let rel = self.relative(path)?;
        let against = if staged && *against == Against::Index {
            &Against::Head
        } else {
            against
        };
        let old = match against {
            Against::Index => self.index_blob(&rel)?,
            Against::Head => self.revision_blob("HEAD", &rel)?,
            Against::Revision(r) => self.revision_blob(r, &rel)?,
        };
        let new = if staged {
            self.index_blob(&rel)?
        } else {
            match std::fs::read(self.absolute(&rel)) {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            }
        };
        if old.is_none() && new.is_none() {
            return Err(GitError::new(
                ErrorKind::NotFound,
                format!("{rel} is not in the repository"),
            ));
        }
        let old_text = old.as_deref().map(text_of);
        let new_text = new.as_deref().map(text_of);
        let binary = matches!(old_text, Some(None)) || matches!(new_text, Some(None));
        Ok(DiffTexts {
            path: rel,
            old: if binary {
                String::new()
            } else {
                old_text.flatten().unwrap_or_default()
            },
            new: if binary {
                String::new()
            } else {
                new_text.flatten().unwrap_or_default()
            },
            binary,
            old_label: against.label(),
            new_label: if staged { "index" } else { "working tree" }.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn texts_against_index_head_and_revisions() {
        let t = TestRepo::new();
        t.write("a.cs", "one\n");
        t.commit_all("one");
        t.write("a.cs", "two\n");
        t.commit_all("two");
        t.write("a.cs", "three\n");
        t.repo.stage(Some(&["a.cs".into()])).unwrap();
        t.write("a.cs", "four\n");
        let d = t.repo.diff_texts("a.cs", &Against::Index, false).unwrap();
        assert_eq!((d.old.as_str(), d.new.as_str()), ("three\n", "four\n"));
        assert_eq!(
            (d.old_label.as_str(), d.new_label.as_str()),
            ("index", "working tree")
        );
        let d = t.repo.diff_texts("a.cs", &Against::Head, false).unwrap();
        assert_eq!(d.old, "two\n");
        let d = t.repo.diff_texts("a.cs", &Against::Index, true).unwrap();
        assert_eq!(
            (d.old.as_str(), d.new.as_str(), d.old_label.as_str()),
            ("two\n", "three\n", "HEAD")
        );
        let d = t
            .repo
            .diff_texts("a.cs", &Against::parse("HEAD~1"), false)
            .unwrap();
        assert_eq!(d.old, "one\n");
        assert!(
            t.repo
                .diff_texts("a.cs", &Against::parse("nope"), false)
                .is_err()
        );
        // A new file has an empty old side; a binary file has no texts.
        t.write("n.cs", "n\n");
        let d = t.repo.diff_texts("n.cs", &Against::Index, false).unwrap();
        assert_eq!((d.old.as_str(), d.new.as_str()), ("", "n\n"));
        t.write("b.bin", "a\0b");
        assert!(
            t.repo
                .diff_texts("b.bin", &Against::Index, false)
                .unwrap()
                .binary
        );
        assert_eq!(
            t.repo.index_text("a.cs").unwrap().as_deref(),
            Some("three\n")
        );
        assert_eq!(t.repo.index_text("n.cs").unwrap(), None);
        assert!(
            t.repo
                .diff_texts("none.cs", &Against::Index, false)
                .is_err()
        );
    }
}
