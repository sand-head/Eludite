//! Blame (Annotate): each line of a file as committed at HEAD with the commit that last changed it.

use std::collections::HashMap;
use std::path::Path;

use crate::{Repo, Result, short, summary_of};

/// A commit blame refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameCommit {
    pub oid: git2::Oid,
    pub short: String,
    pub author: String,
    pub time: i64,
    pub offset_minutes: i32,
    pub summary: String,
}

/// A file's blame: per line (1-based, in order) the index of its commit in `commits`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blame {
    pub path: String,
    pub lines: Vec<(u32, usize)>,
    pub commits: Vec<BlameCommit>,
    pub truncated: bool,
}

impl Repo {
    /// Blame `path` at HEAD, at most `max_lines` lines.
    pub fn blame(&self, path: &str, max_lines: usize) -> Result<Blame> {
        let rel = self.relative(path)?;
        let repo = self.repository()?;
        let blame = repo.blame_file(Path::new(&rel), None)?;
        let mut out = Blame {
            path: rel,
            ..Default::default()
        };
        let mut seen: HashMap<git2::Oid, usize> = HashMap::new();
        'hunks: for hunk in blame.iter() {
            let oid = hunk.final_commit_id();
            let ix = match seen.get(&oid) {
                Some(i) => *i,
                None => {
                    let commit = repo.find_commit(oid)?;
                    let a = hunk.final_signature();
                    out.commits.push(BlameCommit {
                        oid,
                        short: short(oid),
                        author: a.name().unwrap_or_default().to_owned(),
                        time: a.when().seconds(),
                        offset_minutes: a.when().offset_minutes(),
                        summary: summary_of(commit.message().unwrap_or_default()),
                    });
                    seen.insert(oid, out.commits.len() - 1);
                    out.commits.len() - 1
                }
            };
            let start = hunk.final_start_line() as u32;
            for k in 0..hunk.lines_in_hunk() as u32 {
                if out.lines.len() == max_lines {
                    out.truncated = true;
                    break 'hunks;
                }
                out.lines.push((start + k, ix));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::TestRepo;

    #[test]
    fn each_line_names_its_commit() {
        let t = TestRepo::new();
        t.write("a.cs", "one\ntwo\nthree\n");
        let first = t.commit_all("first");
        t.write("a.cs", "one\nTWO\nthree\nfour\n");
        let second = t.commit_all("second");
        let b = t.repo.blame("a.cs", 100).unwrap();
        let by_line: Vec<(u32, git2::Oid)> = b
            .lines
            .iter()
            .map(|(l, c)| (*l, b.commits[*c].oid))
            .collect();
        assert_eq!(by_line, [(1, first), (2, second), (3, first), (4, second)]);
        assert_eq!(b.commits.len(), 2);
        assert_eq!(b.commits[0].author, "Test");
        let cut = t.repo.blame("a.cs", 2).unwrap();
        assert_eq!((cut.lines.len(), cut.truncated), (2, true));
        assert!(t.repo.blame("nope.cs", 10).is_err());
    }
}
