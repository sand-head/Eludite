//! The search: the parallel walk, the per-file search, the overlay, the sink, the cap and cancellation.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::Read as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch,
};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkState};

use crate::query::{Compiled, Query};
use crate::text::{Encoding, decode, fingerprint, strip_ending};

/// `search.excludes`'s default: build output, packages and repositories.
pub const DEFAULT_EXCLUDES: [&str; 5] = [
    "**/bin/**",
    "**/obj/**",
    "**/node_modules/**",
    "**/target/**",
    "**/.git/**",
];

/// What is searched, besides the scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filters {
    /// The setting `search.excludes`: globs (gitignore syntax) of files and folders to skip.
    pub excludes: Vec<String>,
    /// File types: globs a file must match (any of them); one starting with `!` excludes. Empty: every file.
    pub include: Vec<String>,
    /// More globs to skip.
    pub exclude: Vec<String>,
    /// Honor `.gitignore`, `.git/info/exclude`, the global excludes file and `.ignore` (`search.useGitignore`).
    pub use_gitignore: bool,
    /// Files larger than this many bytes are skipped (an open document is searched whatever its size).
    pub max_file_size: u64,
    /// Follow symbolic links (`search.followSymlinks`); off, they are skipped.
    pub follow_symlinks: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            excludes: DEFAULT_EXCLUDES.iter().map(|s| (*s).to_owned()).collect(),
            include: Vec::new(),
            exclude: Vec::new(),
            use_gitignore: true,
            max_file_size: 4 * 1024 * 1024,
            follow_symlinks: false,
        }
    }
}

/// Where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// These folders (with their subfolders) and files. A folder inside another one is searched once. A file named
    /// here is searched whatever the excludes and `.gitignore` say.
    Paths(Vec<PathBuf>),
    /// Only the overlay's documents (All Open Documents).
    OpenDocuments,
}

/// One search.
#[derive(Debug, Clone)]
pub struct Request {
    pub query: Query,
    pub scope: Scope,
    pub filters: Filters,
    /// Stop after this many matches ([`Summary::truncated`]).
    pub max_results: usize,
    /// Lines of context kept before and after each match.
    pub context_lines: usize,
    /// Walker threads; 0 is one per available core.
    pub threads: usize,
}

impl Request {
    pub fn new(query: Query, scope: Scope) -> Self {
        Self {
            query,
            scope,
            filters: Filters::default(),
            max_results: 1000,
            context_lines: 0,
            threads: 0,
        }
    }
}

/// The documents open in editors: their text, unsaved edits included, searched instead of their files.
pub trait Overlay: Send + Sync {
    /// The text of the open document at `path` (absolute, normalized as the scope's paths are).
    fn text(&self, path: &Path) -> Option<Arc<str>>;
    /// Every open document's path.
    fn paths(&self) -> Vec<PathBuf>;
}

impl Overlay for HashMap<PathBuf, Arc<str>> {
    fn text(&self, path: &Path) -> Option<Arc<str>> {
        self.get(path).cloned()
    }

    fn paths(&self) -> Vec<PathBuf> {
        let mut p: Vec<PathBuf> = self.keys().cloned().collect();
        p.sort();
        p
    }
}

/// A matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// 1-based.
    pub line: u64,
    /// Where the line starts in the searched text, in bytes.
    pub offset: u64,
    /// The line without its line ending (invalid UTF-8 replaced).
    pub text: String,
    /// The matches' byte ranges in `text`, in order.
    pub ranges: Vec<Range<usize>>,
    /// Context lines before and after, when asked.
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// A file's matches, handed over whole once the file is searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    /// Searched from the overlay (an open document's text).
    pub open: bool,
    /// The matching lines, in order.
    pub lines: Vec<LineMatch>,
    /// [`crate::fingerprint`] of what was searched: the file's bytes, or the overlay's text.
    pub fingerprint: u64,
    pub encoding: Encoding,
}

impl FileMatches {
    /// Matches in the file.
    pub fn count(&self) -> usize {
        self.lines.iter().map(|l| l.ranges.len()).sum()
    }
}

/// What a search did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Files read (binary and too-large files are not counted).
    pub files_searched: u64,
    pub matching_files: u64,
    pub matching_lines: u64,
    /// Matches.
    pub total: u64,
    /// Stopped at [`Request::max_results`].
    pub truncated: bool,
    /// Stopped by its [`CancelToken`].
    pub canceled: bool,
    pub elapsed: Duration,
}

/// Stops a search: the walk ends and the file being searched stops at its next match.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why a search could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    Query(String),
    /// A glob of the filters does not parse.
    Glob(String),
    /// A path of the scope does not exist.
    Missing(PathBuf),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Query(m) => write!(f, "the query is not valid: {m}"),
            SearchError::Glob(m) => write!(f, "a glob is not valid: {m}"),
            SearchError::Missing(p) => write!(f, "{} does not exist", p.display()),
        }
    }
}

impl std::error::Error for SearchError {}

/// The counters every walker thread adds to.
#[derive(Default)]
struct Counters {
    searched: AtomicU64,
    files: AtomicU64,
    lines: AtomicU64,
    /// Matches handed over (or reserved by a file being searched).
    total: AtomicU64,
    /// The cap was reached.
    full: AtomicBool,
}

/// What every file's search shares.
struct Shared<'a> {
    compiled: &'a Compiled,
    overlay: &'a dyn Overlay,
    cancel: &'a CancelToken,
    sink: &'a (dyn Fn(FileMatches) + Send + Sync),
    counters: &'a Counters,
    cap: u64,
    max_file_size: u64,
    context: usize,
}

impl Shared<'_> {
    fn stopped(&self) -> bool {
        self.cancel.is_canceled() || self.counters.full.load(Ordering::Relaxed)
    }

    fn searcher(&self) -> Searcher {
        SearcherBuilder::new()
            .line_terminator(grep_matcher::LineTerminator::crlf())
            .line_number(true)
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .before_context(self.context)
            .after_context(self.context)
            .build()
    }

    /// Search one file (from the overlay when it is open there). `buf` is the thread's read buffer.
    fn file(&self, searcher: &mut Searcher, path: &Path, buf: &mut Vec<u8>) {
        if self.stopped() {
            return;
        }
        let overlay = self.overlay.text(path);
        let open = overlay.is_some();
        let bytes: &[u8] = match &overlay {
            Some(text) => text.as_bytes(),
            None => {
                buf.clear();
                let read = File::open(path).and_then(|mut f| {
                    let len = f.metadata()?.len();
                    if len > self.max_file_size {
                        return Err(std::io::ErrorKind::FileTooLarge.into());
                    }
                    buf.reserve(len as usize + 1);
                    f.read_to_end(buf)
                });
                if read.is_err() || buf.len() as u64 > self.max_file_size {
                    return;
                }
                buf
            }
        };
        searcher.set_binary_detection(if open {
            BinaryDetection::none()
        } else {
            BinaryDetection::quit(b'\x00')
        });
        let (text, encoding) = if open {
            (std::borrow::Cow::Borrowed(bytes), Encoding::Utf8)
        } else {
            decode(bytes)
        };
        let mut sink = Collect {
            shared: self,
            lines: Vec::new(),
            before: Vec::new(),
            binary: false,
            stop: false,
        };
        if searcher
            .search_slice(&self.compiled.grep, &text, &mut sink)
            .is_err()
        {
            return;
        }
        if sink.binary && !open {
            // Ripgrep's NUL rule: a binary file is skipped, matches and all.
            let n: usize = sink.lines.iter().map(|l| l.ranges.len()).sum();
            self.counters.total.fetch_sub(n as u64, Ordering::SeqCst);
            if sink.stop {
                self.counters.full.store(false, Ordering::SeqCst);
            }
            return;
        }
        self.counters.searched.fetch_add(1, Ordering::Relaxed);
        if sink.lines.is_empty() {
            return;
        }
        let lines = sink.lines;
        self.counters.files.fetch_add(1, Ordering::Relaxed);
        self.counters
            .lines
            .fetch_add(lines.len() as u64, Ordering::Relaxed);
        (self.sink)(FileMatches {
            path: path.to_path_buf(),
            open,
            lines,
            fingerprint: fingerprint(bytes),
            encoding,
        });
    }
}

/// The sink of one file's search.
struct Collect<'a, 'b> {
    shared: &'a Shared<'b>,
    lines: Vec<LineMatch>,
    /// Context lines waiting for the next match.
    before: Vec<String>,
    binary: bool,
    /// This file reached the cap.
    stop: bool,
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(strip_ending(bytes)).into_owned()
}

impl Sink for Collect<'_, '_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if self.stop || self.shared.stopped() {
            return Ok(false);
        }
        let text = lossy(m.bytes());
        let mut ranges = self.shared.compiled.ranges(text.as_bytes());
        if ranges.is_empty() {
            // ripgrep's matcher saw a match in the line ending (`\r`), which is not part of the line.
            self.before.clear();
            return Ok(true);
        }
        let counters = self.shared.counters;
        let n = ranges.len() as u64;
        let before = counters.total.fetch_add(n, Ordering::SeqCst);
        if before >= self.shared.cap {
            counters.total.fetch_sub(n, Ordering::SeqCst);
            counters.full.store(true, Ordering::SeqCst);
            self.stop = true;
            return Ok(false);
        }
        if before + n >= self.shared.cap {
            let keep = (self.shared.cap - before) as usize;
            counters.total.fetch_sub(n - keep as u64, Ordering::SeqCst);
            ranges.truncate(keep);
            counters.full.store(true, Ordering::SeqCst);
            self.stop = true;
        }
        self.lines.push(LineMatch {
            line: m.line_number().unwrap_or(0),
            offset: m.absolute_byte_offset(),
            text,
            ranges,
            before: std::mem::take(&mut self.before),
            after: Vec::new(),
        });
        Ok(!self.stop)
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        let text = lossy(c.bytes());
        match c.kind() {
            SinkContextKind::Before => self.before.push(text),
            SinkContextKind::After => {
                if let Some(last) = self.lines.last_mut() {
                    last.after.push(text);
                }
            }
            SinkContextKind::Other => {}
        }
        Ok(true)
    }

    fn context_break(&mut self, _: &Searcher) -> Result<bool, Self::Error> {
        self.before.clear();
        Ok(true)
    }

    fn binary_data(&mut self, _: &Searcher, _: u64) -> Result<bool, Self::Error> {
        self.binary = true;
        Ok(false)
    }
}

/// The folders and files of a scope, a folder inside another one dropped.
fn roots(paths: &[PathBuf]) -> Result<Vec<PathBuf>, SearchError> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut sorted = paths.to_vec();
    sorted.sort();
    sorted.dedup();
    for p in sorted {
        if !p.exists() {
            return Err(SearchError::Missing(p));
        }
        if out.iter().any(|r| p.starts_with(r)) {
            continue;
        }
        out.push(p);
    }
    Ok(out)
}

/// The excludes of `filters` as overrides rooted at `root`: every glob ignores, and a folder glob (`x/**`) also
/// prunes the folder itself.
fn excludes(root: &Path, filters: &Filters) -> Result<Override, SearchError> {
    let mut b = OverrideBuilder::new(root);
    let negated = filters
        .include
        .iter()
        .filter_map(|g| g.strip_prefix('!'))
        .map(str::to_owned);
    for g in filters
        .excludes
        .iter()
        .chain(&filters.exclude)
        .cloned()
        .chain(negated)
    {
        let g = g.trim();
        if g.is_empty() {
            continue;
        }
        b.add(&format!("!{g}"))
            .map_err(|e| SearchError::Glob(e.to_string()))?;
        if let Some(dir) = g.strip_suffix("/**")
            && !dir.is_empty()
        {
            b.add(&format!("!{dir}"))
                .map_err(|e| SearchError::Glob(e.to_string()))?;
        }
    }
    b.build().map_err(|e| SearchError::Glob(e.to_string()))
}

/// The include globs (File types without the `!` ones) rooted at `root`; `None` when there are none.
fn includes(root: &Path, filters: &Filters) -> Result<Option<Gitignore>, SearchError> {
    let globs: Vec<&str> = filters
        .include
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty() && !g.starts_with('!'))
        .collect();
    if globs.is_empty() {
        return Ok(None);
    }
    let mut b = GitignoreBuilder::new(root);
    for g in globs {
        b.add_line(None, g)
            .map_err(|e| SearchError::Glob(e.to_string()))?;
    }
    b.build()
        .map(Some)
        .map_err(|e| SearchError::Glob(e.to_string()))
}

/// Whether a file under `root` passes the include globs.
fn included(inc: &Option<Gitignore>, root: &Path, path: &Path) -> bool {
    match inc {
        None => true,
        Some(g) if path.starts_with(root) && path != root => {
            g.matched_path_or_any_parents(path, false).is_ignore()
        }
        Some(g) => path
            .file_name()
            .is_some_and(|n| g.matched(Path::new(n), false).is_ignore()),
    }
}

fn threads(n: usize) -> usize {
    if n > 0 {
        return n;
    }
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// Search `request`, handing each matching file to `sink` (from the walker's threads, as each file is done). Open
/// documents come from `overlay`. Returns once the walk ends, the cap is reached or `cancel` fires.
pub fn search(
    request: &Request,
    overlay: &dyn Overlay,
    cancel: &CancelToken,
    sink: &(dyn Fn(FileMatches) + Send + Sync),
) -> Result<Summary, SearchError> {
    let started = Instant::now();
    let compiled = request
        .query
        .compile()
        .map_err(|e| SearchError::Query(e.0))?;
    let counters = Counters::default();
    let shared = Shared {
        compiled: &compiled,
        overlay,
        cancel,
        sink,
        counters: &counters,
        cap: request.max_results.max(1) as u64,
        max_file_size: request.filters.max_file_size,
        context: request.context_lines,
    };
    match &request.scope {
        Scope::OpenDocuments => {
            let filters = &request.filters;
            let empty = Path::new("");
            let inc = includes(empty, filters)?;
            let mut exc_builder = GitignoreBuilder::new(empty);
            for g in filters
                .exclude
                .iter()
                .map(String::as_str)
                .chain(filters.include.iter().filter_map(|g| g.strip_prefix('!')))
            {
                exc_builder
                    .add_line(None, g.trim())
                    .map_err(|e| SearchError::Glob(e.to_string()))?;
            }
            let exc = exc_builder
                .build()
                .map_err(|e| SearchError::Glob(e.to_string()))?;
            let mut searcher = shared.searcher();
            let mut buf = Vec::new();
            for path in overlay.paths() {
                if shared.stopped() {
                    break;
                }
                let name = Path::new(path.file_name().unwrap_or_default());
                if !included(&inc, empty, &path) || exc.matched(name, false).is_ignore() {
                    continue;
                }
                shared.file(&mut searcher, &path, &mut buf);
            }
        }
        Scope::Paths(paths) => {
            for root in roots(paths)? {
                if shared.stopped() {
                    break;
                }
                walk(&shared, &root, request)?;
            }
        }
    }
    Ok(Summary {
        files_searched: counters.searched.load(Ordering::SeqCst),
        matching_files: counters.files.load(Ordering::SeqCst),
        matching_lines: counters.lines.load(Ordering::SeqCst),
        total: counters.total.load(Ordering::SeqCst),
        truncated: counters.full.load(Ordering::SeqCst),
        canceled: cancel.is_canceled(),
        elapsed: started.elapsed(),
    })
}

/// Walk one folder (or file) in parallel.
fn walk(shared: &Shared<'_>, root: &Path, request: &Request) -> Result<(), SearchError> {
    let filters = &request.filters;
    let git = filters.use_gitignore;
    let mut b = WalkBuilder::new(root);
    b.threads(threads(request.threads))
        .hidden(false)
        .parents(git)
        .ignore(git)
        .git_ignore(git)
        .git_global(git)
        .git_exclude(git)
        .require_git(false)
        .follow_links(filters.follow_symlinks)
        .overrides(excludes(root, filters)?);
    let inc = includes(root, filters)?;
    b.build_parallel().run(|| {
        let mut searcher = shared.searcher();
        let mut buf = Vec::new();
        let inc = &inc;
        Box::new(move |entry| {
            if shared.stopped() {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path();
            if entry.depth() > 0 && !included(inc, root, path) {
                return WalkState::Continue;
            }
            shared.file(&mut searcher, path, &mut buf);
            if shared.stopped() {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        })
    });
    Ok(())
}
