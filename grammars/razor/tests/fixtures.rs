//! Every Razor fixture under `corpus/web/razor/` parses with no `ERROR` and no
//! `MISSING` node. The fixtures are real-shaped Blazor components, Razor Pages
//! and MVC views that exercise the constructs brief 0056 added or fixed.

use std::path::{Path, PathBuf};

fn fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fixtures(&path, out);
        } else if path
            .extension()
            .is_some_and(|e| e == "razor" || e == "cshtml")
        {
            out.push(path);
        }
    }
}

fn errors(node: tree_sitter::Node, text: &str, out: &mut Vec<String>) {
    if node.is_error() || node.is_missing() {
        let range = node.range();
        let end = range.end_byte.min(range.start_byte + 60);
        let snippet: String = text[range.start_byte..end]
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        out.push(format!(
            "    {}:{} {} `{snippet}`",
            range.start_point.row + 1,
            range.start_point.column + 1,
            if node.is_missing() {
                "MISSING"
            } else {
                "ERROR"
            }
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        errors(child, text, out);
    }
}

#[test]
fn every_fixture_parses_without_error_nodes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/web/razor");
    let mut files = Vec::new();
    fixtures(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no fixtures under {}", dir.display());

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_razor::LANGUAGE.into())
        .unwrap();

    let mut failures = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let tree = parser.parse(&text, None).unwrap();
        let mut found = Vec::new();
        errors(tree.root_node(), &text, &mut found);
        if !found.is_empty() {
            failures.push(format!(
                "  {}\n{}",
                file.strip_prefix(&dir).unwrap().display(),
                found.join("\n")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixtures have error nodes:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}
