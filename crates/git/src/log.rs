//! The history with its graph (the Git Repository window): commits newest first in topological order, each with its
//! lane and the edges down to the next row, computed here and bounded at [`MAX_LANES`] lanes; a path filter; paging;
//! cancellation.

use std::collections::HashMap;

use git2::{Oid, Sort};

use crate::{Cancel, ErrorKind, GitError, Repo, Result, short, summary_of};

/// Lanes a row may use; lanes past it are dropped and the row says `overflow`.
pub const MAX_LANES: usize = 32;

/// What to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogOptions {
    /// Where to start (default HEAD).
    pub revision: Option<String>,
    /// Every branch, remote branch and tag.
    pub all: bool,
    /// Only commits that change this path (relative).
    pub path: Option<String>,
    pub max: usize,
    pub skip: usize,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            revision: None,
            all: false,
            path: None,
            max: 100,
            skip: 0,
        }
    }
}

/// One commit of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub oid: Oid,
    pub short: String,
    pub parents: Vec<Oid>,
    pub author: String,
    pub email: String,
    /// Seconds since the epoch, and the author's offset in minutes.
    pub time: i64,
    pub offset_minutes: i32,
    pub summary: String,
    /// `HEAD -> main`, `origin/main`, `tag: v1`.
    pub refs: Vec<String>,
    pub graph: GraphRow,
}

/// One row of the graph: the commit's lane and the edges from this row down to the next (`(from, to)` lanes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: usize,
    pub edges: Vec<(usize, usize)>,
    pub overflow: bool,
}

/// The lanes of `commits` (each `(id, parents)`, newest first, children before parents).
pub fn layout(commits: &[(Oid, Vec<Oid>)]) -> Vec<GraphRow> {
    struct Lane {
        expect: Oid,
        /// The lanes of the previous row that lead here.
        origins: Vec<usize>,
    }
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut rows: Vec<GraphRow> = Vec::with_capacity(commits.len());
    for (i, (id, parents)) in commits.iter().enumerate() {
        let lane = lanes
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| l.expect == *id))
            .or_else(|| lanes.iter().position(Option::is_none))
            .unwrap_or(lanes.len());
        // The previous row's edges down to this one.
        if i > 0 {
            let prev = &mut rows[i - 1];
            for (k, l) in lanes.iter().enumerate() {
                if let Some(l) = l {
                    let to = if l.expect == *id { lane } else { k };
                    for &o in &l.origins {
                        if !prev.edges.contains(&(o, to)) {
                            prev.edges.push((o, to));
                        }
                    }
                }
            }
        }
        // Lanes waiting for this commit end here; the others pass through.
        for (k, l) in lanes.iter_mut().enumerate() {
            match l {
                Some(x) if x.expect == *id => *l = None,
                Some(x) => x.origins = vec![k],
                None => {}
            }
        }
        if lane == lanes.len() {
            lanes.push(None);
        }
        let mut overflow = false;
        if let Some(first) = parents.first() {
            lanes[lane] = Some(Lane {
                expect: *first,
                origins: vec![lane],
            });
        }
        for p in parents.iter().skip(1) {
            if let Some(Some(l)) = lanes
                .iter_mut()
                .find(|l| l.as_ref().is_some_and(|l| l.expect == *p))
            {
                l.origins.push(lane);
                continue;
            }
            let slot = lanes
                .iter()
                .position(Option::is_none)
                .unwrap_or(lanes.len());
            if slot >= MAX_LANES {
                overflow = true;
                continue;
            }
            if slot == lanes.len() {
                lanes.push(None);
            }
            lanes[slot] = Some(Lane {
                expect: *p,
                origins: vec![lane],
            });
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        if lanes.len() > MAX_LANES {
            lanes.truncate(MAX_LANES);
            overflow = true;
        }
        rows.push(GraphRow {
            lane,
            edges: Vec::new(),
            overflow,
        });
    }
    // The last row's lanes continue below the page.
    if let Some(last) = rows.last_mut() {
        for (k, l) in lanes.iter().enumerate() {
            if let Some(l) = l {
                for &o in &l.origins {
                    if !last.edges.contains(&(o, k)) {
                        last.edges.push((o, k));
                    }
                }
            }
        }
    }
    for r in &mut rows {
        r.edges.sort_unstable();
    }
    rows
}

impl Repo {
    /// The labels of every ref, by the commit they point at.
    pub fn ref_labels(&self) -> Result<HashMap<Oid, Vec<String>>> {
        let repo = self.repository()?;
        let mut out: HashMap<Oid, Vec<String>> = HashMap::new();
        let head = repo.head().ok();
        let head_branch = head
            .as_ref()
            .filter(|h| h.is_branch())
            .and_then(|h| h.shorthand().map(str::to_owned));
        if let Some(h) = &head
            && !h.is_branch()
            && let Some(t) = h.target()
        {
            out.entry(t).or_default().push("HEAD".into());
        }
        for r in repo.references()?.flatten() {
            let Some(name) = r.name().map(str::to_owned) else {
                continue;
            };
            let Ok(commit) = r.peel_to_commit() else {
                continue;
            };
            let label = if let Some(b) = name.strip_prefix("refs/heads/") {
                if head_branch.as_deref() == Some(b) {
                    format!("HEAD -> {b}")
                } else {
                    b.to_owned()
                }
            } else if let Some(b) = name.strip_prefix("refs/remotes/") {
                if b.ends_with("/HEAD") {
                    continue;
                }
                b.to_owned()
            } else if let Some(t) = name.strip_prefix("refs/tags/") {
                format!("tag: {t}")
            } else {
                continue;
            };
            out.entry(commit.id()).or_default().push(label);
        }
        for v in out.values_mut() {
            v.sort_by_key(|l| (!l.starts_with("HEAD"), l.starts_with("tag: "), l.clone()));
        }
        Ok(out)
    }

    /// The history, with whether more commits follow.
    pub fn log(&self, opts: &LogOptions, cancel: &Cancel) -> Result<(Vec<LogEntry>, bool)> {
        cancel.check()?;
        let repo = self.repository()?;
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
        if opts.all {
            for glob in ["refs/heads", "refs/remotes", "refs/tags"] {
                walk.push_glob(glob)?;
            }
            if repo.head().is_ok() {
                walk.push_head()?;
            }
        } else {
            let rev = opts.revision.as_deref().unwrap_or("HEAD");
            let oid = match repo.revparse_single(rev) {
                Ok(o) => o.peel_to_commit()?.id(),
                Err(_) if rev == "HEAD" && repo.head().is_err() => return Ok((Vec::new(), false)),
                Err(e) => {
                    return Err(GitError::new(
                        ErrorKind::NotFound,
                        format!("unknown revision `{rev}`: {}", e.message()),
                    ));
                }
            };
            walk.push(oid)?;
        }
        let rel = match &opts.path {
            Some(p) => Some(self.relative(p)?),
            None => None,
        };
        let labels = self.ref_labels()?;
        let mut picked: Vec<git2::Commit<'_>> = Vec::new();
        let mut skipped = 0;
        let mut more = false;
        for (n, oid) in walk.enumerate() {
            if n % 64 == 0 {
                cancel.check()?;
            }
            let commit = repo.find_commit(oid?)?;
            if let Some(rel) = &rel
                && !touches(&commit, rel)?
            {
                continue;
            }
            if skipped < opts.skip {
                skipped += 1;
                continue;
            }
            if picked.len() == opts.max {
                more = true;
                break;
            }
            picked.push(commit);
        }
        let shape: Vec<(Oid, Vec<Oid>)> = picked
            .iter()
            .map(|c| (c.id(), c.parent_ids().collect()))
            .collect();
        let rows = layout(&shape);
        let entries = picked
            .iter()
            .zip(rows)
            .map(|(c, graph)| {
                let a = c.author();
                LogEntry {
                    oid: c.id(),
                    short: short(c.id()),
                    parents: c.parent_ids().collect(),
                    author: a.name().unwrap_or_default().to_owned(),
                    email: a.email().unwrap_or_default().to_owned(),
                    time: a.when().seconds(),
                    offset_minutes: a.when().offset_minutes(),
                    summary: summary_of(c.message().unwrap_or_default()),
                    refs: labels.get(&c.id()).cloned().unwrap_or_default(),
                    graph,
                }
            })
            .collect();
        Ok((entries, more))
    }
}

/// Whether `commit` changes `rel` against its parents (against every parent, for a merge, as `git log -- path`).
fn touches(commit: &git2::Commit<'_>, rel: &str) -> Result<bool> {
    let entry = |c: &git2::Commit<'_>| -> Result<Option<Oid>> {
        Ok(c.tree()?
            .get_path(std::path::Path::new(rel))
            .ok()
            .map(|e| e.id()))
    };
    let mine = entry(commit)?;
    if commit.parent_count() == 0 {
        return Ok(mine.is_some());
    }
    for p in commit.parents() {
        if entry(&p)? == mine {
            return Ok(false);
        }
    }
    Ok(true)
}

/// RFC 3339 for seconds since the epoch at an offset in minutes (`2026-10-03T14:05:09+02:00`).
pub fn rfc3339(seconds: i64, offset_minutes: i32) -> String {
    let local = seconds + i64::from(offset_minutes) * 60;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let off = offset_minutes.unsigned_abs();
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        off / 60,
        off % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes(&[n; 20]).unwrap()
    }

    #[test]
    fn lanes_of_a_merge_heavy_history() {
        // 6 merges 5 and 4; 5 and 4 come from 3; 3 merges 2 and 1; 2 and 1 come from 0.
        //   6
        //   |\
        //   5 |
        //   | 4
        //   |/
        //   3
        //   |\
        //   2 |
        //   | 1
        //   |/
        //   0
        let h = vec![
            (oid(6), vec![oid(5), oid(4)]),
            (oid(5), vec![oid(3)]),
            (oid(4), vec![oid(3)]),
            (oid(3), vec![oid(2), oid(1)]),
            (oid(2), vec![oid(0)]),
            (oid(1), vec![oid(0)]),
            (oid(0), vec![]),
        ];
        let rows = layout(&h);
        let lanes: Vec<usize> = rows.iter().map(|r| r.lane).collect();
        assert_eq!(lanes, [0, 0, 1, 0, 0, 1, 0]);
        assert_eq!(rows[0].edges, [(0, 0), (0, 1)], "the merge opens a lane");
        assert_eq!(rows[1].edges, [(0, 0), (1, 1)], "lane 1 passes 5");
        assert_eq!(rows[2].edges, [(0, 0), (1, 0)], "4 joins 3");
        assert_eq!(rows[3].edges, [(0, 0), (0, 1)]);
        assert_eq!(rows[5].edges, [(0, 0), (1, 0)]);
        assert!(rows[6].edges.is_empty(), "the root ends every lane");
        assert!(rows.iter().all(|r| !r.overflow));
    }

    #[test]
    fn lanes_are_bounded() {
        // An octopus of 40 parents, each a root.
        let mut h = vec![(oid(200), (0..40).map(oid).collect::<Vec<_>>())];
        h.extend((0..40).map(|n| (oid(n), vec![])));
        let rows = layout(&h);
        assert!(rows[0].overflow);
        assert!(rows.iter().all(|r| r.lane < MAX_LANES + 1));
        assert!(
            rows.iter()
                .flat_map(|r| &r.edges)
                .all(|&(a, b)| a < MAX_LANES && b < MAX_LANES)
        );
    }

    #[test]
    fn log_with_refs_paths_paging_and_cancel() {
        let t = TestRepo::new();
        t.write("a.cs", "1\n");
        t.commit_all("one");
        t.write("b.cs", "1\n");
        t.commit_all("two");
        t.write("a.cs", "2\n");
        let three = t.commit_all("three");
        let r = t.repo.repository().unwrap();
        r.tag_lightweight("v1", &r.find_object(three, None).unwrap(), false)
            .unwrap();
        let (entries, more) = t.repo.log(&LogOptions::default(), &Cancel::new()).unwrap();
        assert!(!more);
        let summaries: Vec<&str> = entries.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(summaries, ["three", "two", "one"]);
        assert_eq!(entries[0].refs, ["HEAD -> main", "tag: v1"]);
        assert_eq!(entries[0].author, "Test");
        assert_eq!(entries[0].short.len(), 7);
        let only_a = LogOptions {
            path: Some("a.cs".into()),
            ..Default::default()
        };
        let (a, _) = t.repo.log(&only_a, &Cancel::new()).unwrap();
        assert_eq!(
            a.iter().map(|e| e.summary.as_str()).collect::<Vec<_>>(),
            ["three", "one"]
        );
        let page = LogOptions {
            max: 1,
            skip: 1,
            ..Default::default()
        };
        let (p, more) = t.repo.log(&page, &Cancel::new()).unwrap();
        assert_eq!((p[0].summary.as_str(), more), ("two", true));
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(
            t.repo
                .log(&LogOptions::default(), &cancel)
                .unwrap_err()
                .kind,
            ErrorKind::Canceled
        );
        assert_eq!(rfc3339(0, 0), "1970-01-01T00:00:00+00:00");
        assert_eq!(rfc3339(1_791_036_309, 120), "2026-10-03T16:05:09+02:00");
        assert_eq!(rfc3339(1_791_036_309, -300), "2026-10-03T09:05:09-05:00");
    }
}
