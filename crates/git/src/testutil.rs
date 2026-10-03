//! Temporary repositories for the unit tests, built with git2 and with the identity in the repository's config.

use std::path::Path;

use crate::{GlobalConfig, Repo};

pub struct TestRepo {
    pub dir: tempfile::TempDir,
    pub repo: Repo,
}

impl TestRepo {
    /// A repository on `main` with `Test <test@example.com>` in its config and no global config.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let r = git2::Repository::init(dir.path()).unwrap();
        r.set_head("refs/heads/main").unwrap();
        let mut c = r.config().unwrap();
        c.set_str("user.name", "Test").unwrap();
        c.set_str("user.email", "test@example.com").unwrap();
        let repo = Repo::open(dir.path())
            .unwrap()
            .with_global_config(GlobalConfig::Files(vec![]));
        Self { dir, repo }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write(&self, rel: &str, text: &str) {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }

    /// Stage everything and commit it.
    pub fn commit_all(&self, message: &str) -> git2::Oid {
        self.repo
            .commit(&crate::commit::CommitOptions {
                message: message.into(),
                all: true,
                ..Default::default()
            })
            .unwrap()
            .oid
    }
}
