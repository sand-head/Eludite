//! The speed budget (brief 0042): a literal search over a synthetic tree of 50,000 files (500 MB) finishes within
//! 1.5 times `rg`'s time on the same machine. The tree goes in a temporary folder and is removed at the end.
//!
//! Prints one line per measurement and a summary line `search-bench: ...`; with `ELUDITE_SEARCH_BENCH_OUT` set, also
//! writes the numbers there as JSON. The ratio is asserted when `rg` is on PATH, the build is optimized or the ratio
//! holds anyway, and the machine is not loaded beyond its cores (the other budgets' rule); otherwise it is printed.
//! `ELUDITE_SEARCH_BENCH_FILES` makes the tree smaller (a quick run).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eludite_search::{CancelToken, Query, Request, Scope, search};

const FILE_BYTES: usize = 10_000;
const PER_DIR: usize = 500;
const NEEDLE: &str = "QuuxFrobnicator";

/// One file's text: code-like lines, with the needle in one file of a hundred.
fn file_text(i: usize) -> String {
    let mut s = String::with_capacity(FILE_BYTES + 100);
    let mut n = i.wrapping_mul(2_654_435_761);
    let mut line = 0;
    while s.len() < FILE_BYTES {
        n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        if i % 100 == 7 && line == 40 {
            s.push_str("    // TODO: replace the QuuxFrobnicator with the new pipeline\n");
        } else {
            s.push_str(&format!(
                "    let value_{line} = compute(alpha_{}, beta_{}, {}); // {:x}\n",
                n % 97,
                n % 89,
                n % 10_007,
                n >> 40
            ));
        }
        line += 1;
    }
    s
}

fn build_tree(root: &Path, files: usize) -> u64 {
    let mut bytes = 0u64;
    for i in 0..files {
        let dir = root.join(format!("d{:03}", i / PER_DIR));
        if i % PER_DIR == 0 {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let text = file_text(i);
        bytes += text.len() as u64;
        std::fs::write(dir.join(format!("f{i:05}.rs")), text).unwrap();
    }
    bytes
}

fn ours(root: &Path) -> (Duration, u64) {
    let request = Request::new(Query::new(NEEDLE), Scope::Paths(vec![root.to_path_buf()]));
    let found = AtomicU64::new(0);
    let started = Instant::now();
    let summary = search(&request, &HashMap::new(), &CancelToken::new(), &|f| {
        found.fetch_add(f.count() as u64, Ordering::Relaxed);
    })
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(summary.total, found.load(Ordering::Relaxed));
    (elapsed, summary.total)
}

/// `rg` with the same semantics: case-insensitive literal, line numbers, the default ignore rules.
fn rg(root: &Path) -> Option<(Duration, u64)> {
    let started = Instant::now();
    let out = Command::new("rg")
        .args(["--fixed-strings", "--ignore-case", "--line-number", NEEDLE])
        .arg(root)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let elapsed = started.elapsed();
    let lines = out.stdout.iter().filter(|&&b| b == b'\n').count() as u64;
    Some((elapsed, lines))
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[test]
fn a_literal_search_of_fifty_thousand_files_keeps_up_with_rg() {
    let files: usize = std::env::var("ELUDITE_SEARCH_BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tree");
    let made = Instant::now();
    let bytes = build_tree(&root, files);
    println!(
        "search-bench: tree of {files} files, {:.0} MB, written in {:.1} s",
        bytes as f64 / 1e6,
        made.elapsed().as_secs_f64()
    );
    let expected = (0..files).filter(|i| i % 100 == 7).count() as u64;
    let has_rg = rg(&root).is_some();
    // Warm the page cache, then alternate.
    let _ = ours(&root);
    let mut mine = Vec::new();
    let mut theirs = Vec::new();
    for _ in 0..5 {
        let (t, n) = ours(&root);
        assert_eq!(n, expected);
        mine.push(t);
        if has_rg && let Some((t, n)) = rg(&root) {
            assert_eq!(n, expected, "rg found the same lines");
            theirs.push(t);
        }
    }
    let m = median(mine.clone());
    let load = load_average();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let optimized = !cfg!(debug_assertions);
    let line = match (has_rg, theirs.is_empty()) {
        (true, false) => {
            let r = median(theirs.clone());
            let ratio = m.as_secs_f64() / r.as_secs_f64();
            println!(
                "search-bench: eludite-search median {:.1} ms (runs {:?}), rg median {:.1} ms (runs {:?}), ratio {ratio:.2}, \
                 load {load:?} on {cores} cores, optimized build {optimized}",
                m.as_secs_f64() * 1e3,
                mine.iter().map(|d| d.as_millis()).collect::<Vec<_>>(),
                r.as_secs_f64() * 1e3,
                theirs.iter().map(|d| d.as_millis()).collect::<Vec<_>>(),
            );
            let quiet = load.is_some_and(|l| l < cores as f64);
            if quiet && files >= 50_000 && (optimized || ratio <= 1.5) {
                assert!(
                    ratio <= 1.5,
                    "eludite-search took {ratio:.2} times rg's time (budget 1.5)"
                );
            }
            serde_like(
                files,
                bytes,
                m,
                Some(r),
                Some(ratio),
                load,
                cores,
                optimized,
            )
        }
        _ => {
            println!(
                "search-bench: eludite-search median {:.1} ms; rg is not on PATH, so no ratio; load {load:?} on {cores} cores",
                m.as_secs_f64() * 1e3
            );
            serde_like(files, bytes, m, None, None, load, cores, optimized)
        }
    };
    if let Ok(out) = std::env::var("ELUDITE_SEARCH_BENCH_OUT") {
        let mut f = std::fs::File::create(out).unwrap();
        writeln!(f, "{line}").unwrap();
    }
    // The tree is removed with `dir`.
    drop(dir);
}

/// The numbers as a JSON object (no serde in this crate's dependencies).
#[allow(clippy::too_many_arguments)]
fn serde_like(
    files: usize,
    bytes: u64,
    ours: Duration,
    rg: Option<Duration>,
    ratio: Option<f64>,
    load: Option<f64>,
    cores: usize,
    optimized: bool,
) -> String {
    let opt = |v: Option<f64>| v.map_or("null".to_owned(), |v| format!("{v:.3}"));
    format!(
        "{{\"files\": {files}, \"bytes\": {bytes}, \"eludite_ms\": {:.1}, \"rg_ms\": {}, \"ratio\": {}, \"load\": {}, \
         \"cores\": {cores}, \"optimized\": {optimized}}}",
        ours.as_secs_f64() * 1e3,
        opt(rg.map(|d| d.as_secs_f64() * 1e3)),
        opt(ratio),
        opt(load),
    )
}

/// The first result's budget (brief 0042): on this repository, the first matching file reaches the sink within 50 ms
/// of the search's start (`target/` and the other build folders excluded, `.gitignore` honored).
#[test]
fn the_first_result_on_this_repository_comes_quickly() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = std::fs::canonicalize(repo).unwrap();
    let request = Request::new(Query::new("fn main"), Scope::Paths(vec![repo]));
    let mut firsts = Vec::new();
    let mut totals = Vec::new();
    for _ in 0..5 {
        let first: std::sync::Mutex<Option<Duration>> = std::sync::Mutex::new(None);
        let started = Instant::now();
        let summary = search(&request, &HashMap::new(), &CancelToken::new(), &|_| {
            first
                .lock()
                .unwrap()
                .get_or_insert_with(|| started.elapsed());
        })
        .unwrap();
        totals.push(started.elapsed());
        firsts.push(first.into_inner().unwrap().expect("a match"));
        assert!(summary.matching_files > 5);
    }
    let first = median(firsts.clone());
    let total = median(totals.clone());
    let load = load_average();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!(
        "search-bench: first result on this repository median {:.1} ms (runs {:?}), whole search median {:.1} ms, \
         load {load:?} on {cores} cores",
        first.as_secs_f64() * 1e3,
        firsts.iter().map(|d| d.as_micros()).collect::<Vec<_>>(),
        total.as_secs_f64() * 1e3
    );
    if load.is_some_and(|l| l < cores as f64) {
        assert!(
            first < Duration::from_millis(50),
            "first result after {first:?}"
        );
    }
}
