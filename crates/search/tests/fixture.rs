//! The search against a fixture tree on disk: excluded and ignored folders, a binary file, a 10 MB file, UTF-16 with
//! a byte order mark, CRLF line endings, the open-documents overlay, regular expression groups in replacements, whole
//! word, File types, the cap and cancellation mid-walk.

#![allow(clippy::single_range_in_vec_init)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_search::replace::{apply, file_edits, replacements};
use eludite_search::{
    CancelToken, Encoding, FileMatches, Overlay, Query, Request, Scope, Summary, fingerprint,
    search,
};

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

/// The fixture: a small solution with build output, a .gitignore, a binary, a large file and odd encodings.
fn fixture() -> Tree {
    let t = Tree {
        dir: tempfile::tempdir().unwrap(),
    };
    t.write(".gitignore", "generated/\n*.log\n");
    t.write(
        "src/App/Program.cs",
        "class Program\n{\n    static void Main() { var order = new Order(); }\n}\n",
    );
    t.write(
        "src/App/Order.cs",
        "class Order\n{\n    public int Id;\n    public Orders Orders;\n}\n",
    );
    t.write("src/App/Order.Designer.cs", "partial class Order { }\n");
    t.write(
        "src/App/bin/Debug/App.cs",
        "class Order { } // build output\n",
    );
    t.write("src/App/obj/App.AssemblyInfo.cs", "// Order\n");
    t.write("node_modules/lib/index.js", "const Order = 1;\n");
    t.write("generated/Order.g.cs", "class Order { }\n");
    t.write("build.log", "Order built\n");
    t.write("web/site.css", ".order { color: red; }\n");
    // A NUL byte early on: binary.
    let mut bin = b"Order\0\x01\x02".to_vec();
    bin.extend(std::iter::repeat_n(b'x', 1000));
    t.write("assets/logo.bin", bin);
    // CRLF line endings.
    t.write(
        "src/App/Crlf.cs",
        "// first\r\nvar total = Order.Count;\r\n// last\r\n",
    );
    // UTF-16 little-endian with its byte order mark.
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(
            "// é\nclass Wide { Order o; }\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        )
        .collect();
    t.write("src/App/Wide.cs", utf16);
    // 10 MB, with a match near the end.
    let mut big = "filler line without the word\n".repeat(360_000);
    big.push_str("the last Order\n");
    assert!(big.len() > 10_000_000);
    t.write("data/big.txt", big);
    t
}

struct Found {
    files: Vec<FileMatches>,
    summary: Summary,
}

impl Found {
    /// Paths relative to the tree, sorted.
    fn paths(&self, t: &Tree) -> Vec<String> {
        let mut p: Vec<String> = self
            .files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(t.dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        p.sort();
        p
    }

    fn file(&self, t: &Tree, rel: &str) -> &FileMatches {
        let p = t.path(rel);
        self.files
            .iter()
            .find(|f| f.path == p)
            .unwrap_or_else(|| panic!("no matches in {rel}"))
    }
}

fn run(request: &Request, overlay: &dyn Overlay, cancel: &CancelToken) -> Found {
    let files = Mutex::new(Vec::new());
    let summary = search(request, overlay, cancel, &|f| files.lock().unwrap().push(f)).unwrap();
    let files = files.into_inner().unwrap();
    Found { files, summary }
}

fn find(t: &Tree, query: Query, f: impl FnOnce(&mut Request)) -> Found {
    let mut r = Request::new(query, Scope::Paths(vec![t.dir.path().to_path_buf()]));
    f(&mut r);
    run(&r, &HashMap::new(), &CancelToken::new())
}

#[test]
fn excluded_ignored_binary_and_large_files_are_skipped() {
    let t = fixture();
    let found = find(&t, Query::new("Order"), |_| {});
    assert_eq!(
        found.paths(&t),
        [
            "src/App/Crlf.cs",
            "src/App/Order.Designer.cs",
            "src/App/Order.cs",
            "src/App/Program.cs",
            "src/App/Wide.cs",
            "web/site.css",
        ],
        "bin, obj and node_modules (search.excludes), generated/ and *.log (.gitignore), the binary and the 10 MB \
         file (search.maxFileSize) are skipped"
    );
    let s = &found.summary;
    assert_eq!(s.matching_files, 6);
    // Case-insensitive by default: `.order` in the CSS, both `Orders` on one line count twice.
    assert_eq!(s.total, 9);
    assert_eq!(s.matching_lines, 7);
    assert!(!s.truncated && !s.canceled);
    // .gitignore, the six matching files, and nothing excluded: the binary and the large file are not counted.
    assert_eq!(s.files_searched, 7);
    let order = found.file(&t, "src/App/Order.cs");
    assert_eq!(
        order
            .lines
            .iter()
            .map(|l| (l.line, l.ranges.clone()))
            .collect::<Vec<_>>(),
        [(1, vec![6..11]), (4, vec![11..16, 18..23])]
    );
    assert_eq!(order.lines[1].text, "    public Orders Orders;");
    assert_eq!(
        order.fingerprint,
        fingerprint(&std::fs::read(&order.path).unwrap())
    );

    // Off: .gitignore no longer applies; a larger limit finds the 10 MB file.
    let found = find(&t, Query::new("Order"), |r| {
        r.filters.use_gitignore = false;
        r.filters.max_file_size = 16 * 1024 * 1024;
    });
    let paths = found.paths(&t);
    for p in ["generated/Order.g.cs", "build.log", "data/big.txt"] {
        assert!(paths.contains(&p.to_owned()), "{p} in {paths:?}");
    }
    assert!(
        !paths.contains(&"assets/logo.bin".to_owned()),
        "still binary"
    );
    assert!(
        !paths.iter().any(|p| p.contains("bin/Debug")),
        "still excluded"
    );
    let big = found.file(&t, "data/big.txt");
    assert_eq!(big.lines[0].line, 360_001);
    // No excludes at all: build output is searched.
    let found = find(&t, Query::new("Order"), |r| r.filters.excludes.clear());
    assert!(
        found
            .paths(&t)
            .contains(&"src/App/bin/Debug/App.cs".to_owned())
    );
}

#[test]
fn crlf_and_utf16_files_report_lines_without_their_endings() {
    let t = fixture();
    let found = find(
        &t,
        Query {
            case_sensitive: true,
            ..Query::new("Order")
        },
        |_| {},
    );
    let crlf = found.file(&t, "src/App/Crlf.cs");
    assert_eq!(crlf.lines.len(), 1);
    assert_eq!(crlf.lines[0].line, 2);
    assert_eq!(crlf.lines[0].text, "var total = Order.Count;");
    assert_eq!(crlf.encoding, Encoding::Utf8);
    // `$` matches before `\r\n`.
    let end = find(
        &t,
        Query {
            regex: true,
            ..Query::new(r"Count;$")
        },
        |_| {},
    );
    assert_eq!(end.file(&t, "src/App/Crlf.cs").lines[0].ranges, [18..24]);
    let wide = found.file(&t, "src/App/Wide.cs");
    assert_eq!(wide.encoding, Encoding::Utf16Le);
    assert_eq!(wide.lines[0].line, 2);
    assert_eq!(wide.lines[0].text, "class Wide { Order o; }");
    // The UTF-16 file's first line, decoded.
    let accent = find(&t, Query::new("é"), |_| {});
    assert_eq!(accent.file(&t, "src/App/Wide.cs").lines[0].text, "// é");
}

#[test]
fn context_lines_come_with_their_match() {
    let t = fixture();
    let found = find(
        &t,
        Query {
            case_sensitive: true,
            ..Query::new("total")
        },
        |r| r.context_lines = 1,
    );
    let l = &found.file(&t, "src/App/Crlf.cs").lines[0];
    assert_eq!(l.before, ["// first"]);
    assert_eq!(l.after, ["// last"]);
}

#[test]
fn file_types_include_and_exclude_globs() {
    let t = fixture();
    let found = find(&t, Query::new("Order"), |r| {
        r.filters.include = vec!["*.cs".into(), "!*.Designer.cs".into()];
    });
    assert_eq!(
        found.paths(&t),
        [
            "src/App/Crlf.cs",
            "src/App/Order.cs",
            "src/App/Program.cs",
            "src/App/Wide.cs"
        ]
    );
    // An include glob never brings back what .gitignore or the excludes skip.
    assert!(!found.paths(&t).iter().any(|p| p.contains("generated")));
    let found = find(&t, Query::new("Order"), |r| {
        r.filters.exclude = vec!["web/**".into()];
        r.filters.include = vec!["src/**".into(), "web/*".into()];
    });
    assert!(found.paths(&t).iter().all(|p| p.starts_with("src/")));
    // A file named by the scope is searched whatever the filters say.
    let mut r = Request::new(
        Query::new("Order"),
        Scope::Paths(vec![t.path("generated/Order.g.cs")]),
    );
    r.filters.include = vec!["*.txt".into()];
    let found = run(&r, &HashMap::new(), &CancelToken::new());
    assert_eq!(found.paths(&t), ["generated/Order.g.cs"]);
    // A bad glob is an error before anything is searched.
    let mut r = Request::new(Query::new("x"), Scope::Paths(vec![t.dir.path().into()]));
    r.filters.include = vec!["a{".into()];
    assert!(search(&r, &HashMap::new(), &CancelToken::new(), &|_| {}).is_err());
    let missing = Request::new(Query::new("x"), Scope::Paths(vec![t.path("nope")]));
    assert!(search(&missing, &HashMap::new(), &CancelToken::new(), &|_| {}).is_err());
}

#[test]
fn whole_word_matches_identifiers_only() {
    let t = fixture();
    let found = find(
        &t,
        Query {
            whole_word: true,
            case_sensitive: true,
            ..Query::new("Order")
        },
        |_| {},
    );
    // `Orders` and `.order` are not the word; `new Order()` and `class Order` are.
    let order = found.file(&t, "src/App/Order.cs");
    assert_eq!(order.lines.len(), 1);
    assert_eq!(order.lines[0].line, 1);
    assert!(!found.paths(&t).contains(&"web/site.css".to_owned()));
    let program = found.file(&t, "src/App/Program.cs");
    assert_eq!(program.lines[0].ranges, [41..46]);
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_followed_only_when_asked() {
    let t = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("Linked.cs"), "class Order { }\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), t.path("src/linked")).unwrap();
    let off = find(&t, Query::new("Order"), |_| {});
    assert!(!off.paths(&t).iter().any(|p| p.contains("linked")));
    let on = find(&t, Query::new("Order"), |r| {
        r.filters.follow_symlinks = true
    });
    assert!(on.paths(&t).contains(&"src/linked/Linked.cs".to_owned()));
}

/// Open documents: a map of path to text, with a counter of lookups (and an optional delay per lookup).
struct Docs {
    texts: HashMap<PathBuf, Arc<str>>,
    lookups: AtomicUsize,
    delay: Option<Duration>,
    cancel_after: Option<(usize, CancelToken)>,
}

impl Docs {
    fn new(texts: &[(PathBuf, &str)]) -> Self {
        Self {
            texts: texts
                .iter()
                .map(|(p, t)| (p.clone(), Arc::from(*t)))
                .collect(),
            lookups: AtomicUsize::new(0),
            delay: None,
            cancel_after: None,
        }
    }
}

impl Overlay for Docs {
    fn text(&self, path: &Path) -> Option<Arc<str>> {
        let n = self.lookups.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        if let Some((after, token)) = &self.cancel_after
            && n == *after
        {
            token.cancel();
        }
        self.texts.get(path).cloned()
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.texts.paths()
    }
}

#[test]
fn the_overlay_beats_the_disk_and_counts_once() {
    let t = fixture();
    let program = t.path("src/App/Program.cs");
    // Unsaved: the open document no longer has `Order` but has `Invoice`, twice.
    let unsaved = "class Program\n{\n    Invoice a; Invoice b;\n}\n";
    let docs = Docs::new(&[(program.clone(), unsaved)]);
    let r = Request::new(
        Query::new("Invoice"),
        Scope::Paths(vec![t.dir.path().into()]),
    );
    let found = run(&r, &docs, &CancelToken::new());
    assert_eq!(found.paths(&t), ["src/App/Program.cs"]);
    let f = found.file(&t, "src/App/Program.cs");
    assert!(f.open);
    assert_eq!(f.lines.len(), 1, "the dirty text counts once");
    assert_eq!(f.lines[0].ranges, [4..11, 15..22]);
    assert_eq!(f.fingerprint, fingerprint(unsaved.as_bytes()));
    // What is only on disk is not found in the open document.
    let r = Request::new(
        Query::new("new Order"),
        Scope::Paths(vec![t.dir.path().into()]),
    );
    assert!(run(&r, &docs, &CancelToken::new()).files.is_empty());
    // All Open Documents: the overlay alone, with the File types.
    let order = t.path("src/App/Order.cs");
    let docs = Docs::new(&[(program.clone(), unsaved), (order.clone(), "Invoice\n")]);
    let mut r = Request::new(Query::new("Invoice"), Scope::OpenDocuments);
    let found = run(&r, &docs, &CancelToken::new());
    assert_eq!(found.files.len(), 2);
    r.filters.include = vec!["Order.*".into()];
    let found = run(&r, &docs, &CancelToken::new());
    assert_eq!(found.paths(&t), ["src/App/Order.cs"]);
    r.filters.include = vec!["*.cs".into(), "!Program.cs".into()];
    assert_eq!(
        run(&r, &docs, &CancelToken::new()).paths(&t),
        ["src/App/Order.cs"]
    );
}

#[test]
fn replacements_expand_groups_and_apply_to_the_searched_text() {
    let t = fixture();
    let text = "var a = items.Count();\r\nif (xs.Count() > 1 && 𝄞.Count()) { }\r\n";
    let p = t.write("src/App/Counts.cs", text);
    let mut r = Request::new(
        Query {
            regex: true,
            case_sensitive: true,
            ..Query::new(r"(\w+|𝄞)\.Count\(\)")
        },
        Scope::Paths(vec![p.clone()]),
    );
    r.max_results = 100;
    let found = run(&r, &HashMap::new(), &CancelToken::new());
    let file = &found.files[0];
    let compiled = r.query.compile().unwrap();
    let edits = replacements(&compiled, file, "$1.Length");
    assert_eq!(edits.len(), 3);
    assert_eq!(
        (edits[0].line, edits[0].start_utf16, edits[0].end_utf16),
        (1, 8, 21)
    );
    assert_eq!(edits[0].text, "items.Length");
    // UTF-16 columns count the astral character as two units.
    assert_eq!(
        (edits[2].line, edits[2].start_utf16, edits[2].end_utf16),
        (2, 22, 32)
    );
    assert_eq!(edits[2].text, "𝄞.Length");
    let (path, ranges) = file_edits(&compiled, file, "$1.Length");
    assert_eq!(path, p);
    assert_eq!(
        apply(text, &ranges),
        "var a = items.Length;\r\nif (xs.Length > 1 && 𝄞.Length) { }\r\n"
    );
    // Literal replacement text is not expanded.
    let lit = Request::new(Query::new("items"), Scope::Paths(vec![p.clone()]));
    let found = run(&lit, &HashMap::new(), &CancelToken::new());
    let c = lit.query.compile().unwrap();
    let (_, ranges) = file_edits(&c, &found.files[0], "$1");
    assert!(apply(text, &ranges).starts_with("var a = $1.Count()"));
}

#[test]
fn the_cap_stops_the_search_and_says_truncated() {
    let t = fixture();
    for i in 0..50 {
        t.write(&format!("many/f{i:02}.txt"), "hit hit\nhit\n");
    }
    let mut r = Request::new(Query::new("hit"), Scope::Paths(vec![t.path("many")]));
    r.max_results = 25;
    let found = run(&r, &HashMap::new(), &CancelToken::new());
    let handed: usize = found.files.iter().map(FileMatches::count).sum();
    assert_eq!(handed, 25);
    assert_eq!(found.summary.total, 25);
    assert!(found.summary.truncated);
    assert!(
        found.files.len() <= 9,
        "stopped early: {}",
        found.files.len()
    );
    // Exactly at the number of matches there is nothing more.
    r.max_results = 150;
    let all = run(&r, &HashMap::new(), &CancelToken::new());
    assert_eq!(all.summary.total, 150);
    assert!(all.summary.truncated, "150 matches reach a cap of 150");
    r.max_results = 151;
    assert!(
        !run(&r, &HashMap::new(), &CancelToken::new())
            .summary
            .truncated
    );
}

#[test]
fn cancellation_stops_the_walk_within_one_file() {
    let t = fixture();
    for i in 0..400 {
        t.write(&format!("slow/f{i:03}.txt"), "needle\n");
    }
    let cancel = CancelToken::new();
    let docs = Docs {
        delay: Some(Duration::from_millis(2)),
        cancel_after: Some((20, cancel.clone())),
        ..Docs::new(&[])
    };
    let mut r = Request::new(Query::new("needle"), Scope::Paths(vec![t.path("slow")]));
    r.threads = 4;
    let started = Instant::now();
    let found = run(&r, &docs, &cancel);
    assert!(found.summary.canceled);
    // Each walker thread finishes at most the file it was in when the token fired, and one it had already passed the
    // token check for (25 after a cancel at 20 was seen on a loaded runner); never the hundreds left.
    let looked = docs.lookups.load(Ordering::SeqCst);
    assert!(
        looked <= 20 + 2 * 4,
        "{looked} files looked up after the cancel at 20"
    );
    assert!(found.summary.files_searched <= 28);
    assert!(started.elapsed() < Duration::from_secs(2));
    // A token canceled before the start searches nothing.
    let pre = CancelToken::new();
    pre.cancel();
    let none = run(&r, &HashMap::new(), &pre);
    assert_eq!(none.summary.files_searched, 0);
    assert!(none.summary.canceled);
}
