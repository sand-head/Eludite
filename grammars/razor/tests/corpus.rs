//! Replays the grammar's test corpus (`test/corpus/*.txt`, the format
//! `tree-sitter test` reads) through the compiled parser, so the grammar is
//! proven by `cargo test` on every platform with no Node and no tree-sitter CLI.
//!
//! A case is a name between `===` lines, the input, a `---` line and the
//! expected tree. A header line ending in `:error` expects the tree to carry
//! `ERROR` or `MISSING` nodes; `:skip` skips the case.

use std::fs;
use std::path::Path;

struct Case {
    file: String,
    name: String,
    input: String,
    expected: String,
    expects_error: bool,
    skip: bool,
}

fn separator(line: &str, ch: char) -> bool {
    line.len() >= 3 && line.chars().all(|c| c == ch)
}

fn parse_cases(file: &Path) -> Vec<Case> {
    let text = fs::read_to_string(file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut cases = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !separator(lines[i], '=') {
            i += 1;
            continue;
        }
        // The header: the name, and then any attribute lines (`:error`, `:skip`).
        let mut header = Vec::new();
        i += 1;
        while i < lines.len() && !separator(lines[i], '=') {
            header.push(lines[i]);
            i += 1;
        }
        i += 1;
        let name = header.first().copied().unwrap_or("").trim().to_string();
        let attrs: Vec<&str> = header.iter().skip(1).map(|l| l.trim()).collect();
        let mut input = Vec::new();
        while i < lines.len() && !separator(lines[i], '-') {
            input.push(lines[i]);
            i += 1;
        }
        i += 1;
        let mut expected = Vec::new();
        while i < lines.len() && !separator(lines[i], '=') {
            expected.push(lines[i]);
            i += 1;
        }
        cases.push(Case {
            file: file.file_name().unwrap().to_string_lossy().into_owned(),
            name,
            input: input.join("\n"),
            expected: expected.join("\n"),
            expects_error: attrs.contains(&":error"),
            skip: attrs.contains(&":skip"),
        });
    }
    cases
}

/// Collapse whitespace and drop field labels (`name: (identifier)`), which
/// `to_sexp` prints and `tree-sitter test`'s expected trees leave out, so the
/// two compare.
fn normalize(sexp: &str) -> String {
    let mut out = String::with_capacity(sexp.len());
    for token in sexp.split_whitespace() {
        if token.ends_with(':') && !token.starts_with('(') {
            continue;
        }
        if !out.is_empty() && !out.ends_with('(') && !token.starts_with(')') {
            out.push(' ');
        }
        out.push_str(token);
    }
    out
}

#[test]
fn every_corpus_case_parses_to_its_expected_tree() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no corpus files under {}", dir.display());

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_razor::LANGUAGE.into())
        .unwrap();

    let mut total = 0;
    let mut failures = Vec::new();
    for file in &files {
        for case in parse_cases(file) {
            if case.skip {
                continue;
            }
            total += 1;
            let tree = parser.parse(&case.input, None).unwrap();
            let actual = normalize(&tree.root_node().to_sexp());
            let expected = normalize(&case.expected);
            if actual != expected {
                failures.push(format!(
                    "{} > {}\n  expected: {expected}\n  actual:   {actual}",
                    case.file, case.name
                ));
            } else if !case.expects_error && tree.root_node().has_error() {
                failures.push(format!(
                    "{} > {}: the tree has an error the expected tree does not show",
                    case.file, case.name
                ));
            }
        }
    }
    assert!(total > 0, "no corpus cases");
    assert!(
        failures.is_empty(),
        "{} of {total} corpus cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
