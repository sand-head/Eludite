//! Generates the parser from `src/grammar.json` with `tree-sitter-generate`
//! (the library behind the tree-sitter CLI, at the version `PIN` names) and
//! compiles it with the external scanner (`src/scanner.c`) into the crate.
//! The generated C, its node types and its headers are never checked in
//! (ADR-0012); `grammar.json` is written by `generate.sh`, never by hand.
//!
//! Generation takes about 15 s, so its output is kept in Eludite's cache folder
//! under a key of everything it depends on (`grammars/razor/<key>/`), and a
//! second checkout or a fresh `target/` copies it from there instead.

use std::fs;
use std::path::{Path, PathBuf};

/// What a complete generation writes, relative to its output folder.
const GENERATED: [&str; 5] = [
    "parser.c",
    "node-types.json",
    "tree_sitter/alloc.h",
    "tree_sitter/array.h",
    "tree_sitter/parser.h",
];

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let src_dir = crate_dir.join("src");
    let grammar_json = src_dir.join("grammar.json");
    let scanner = src_dir.join("scanner.c");
    // The parser records the version in tree-sitter.json; PIN names the generator's version.
    let version_file = crate_dir.join("tree-sitter.json");
    let pin_file = crate_dir.join("PIN");
    for path in [&grammar_json, &scanner, &version_file, &pin_file] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-env-changed=ELUDITE_CACHE_DIR");

    let generated = out_dir.join("src");
    let key = cache_key(&[&pin_file, &grammar_json, &version_file]);
    let entry = cache_dir().map(|dir| dir.join("grammars").join("razor").join(key));
    match entry.as_deref().filter(|entry| is_complete(entry)) {
        Some(entry) => copy_generated(entry, &generated),
        None => {
            generate(&crate_dir, &grammar_json, &generated);
            if let Some(entry) = &entry {
                store(&generated, entry);
            }
        }
    }

    let mut c_config = cc::Build::new();
    c_config.std("c11").include(&generated);

    #[cfg(target_env = "msvc")]
    c_config.flag("-utf-8");

    c_config.file(generated.join("parser.c")).file(&scanner);
    c_config.compile("tree-sitter-razor");
}

/// Writes `parser.c`, `node-types.json` and `tree_sitter/*.h` under `out`,
/// exactly as `tree-sitter generate` at the pinned version does from the same
/// `grammar.json` (ABI 15, the CLI's default state merging).
fn generate(crate_dir: &Path, grammar_json: &Path, out: &Path) {
    let mut diagnostics = Vec::new();
    tree_sitter_generate::generate_parser_in_directory(
        crate_dir,
        Some(out),
        Some(grammar_json),
        tree_sitter_generate::ABI_VERSION_MAX,
        None,
        None,
        true,
        tree_sitter_generate::OptLevel::default(),
        &mut diagnostics,
    )
    .unwrap_or_else(|error| {
        panic!(
            "generating the Razor parser from {}: {error}",
            grammar_json.display()
        )
    });
    for diagnostic in diagnostics {
        println!("cargo:warning=tree-sitter-razor: {diagnostic}");
    }
}

/// Eludite's cache folder for fetched and generated tools: `ELUDITE_CACHE_DIR`,
/// else `$XDG_CACHE_HOME/eludite`, else `~/.cache/eludite` (`%USERPROFILE%` on
/// Windows), the folder of `tools/*/fetch.sh` and CI's cache steps. None when
/// the environment names no home, in which case every build generates.
fn cache_dir() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(dir) = env("ELUDITE_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = env("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| {
        env("HOME")
            .or_else(|| env("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".cache"))
    })?;
    Some(base.join("eludite"))
}

/// A name for the generation's inputs: FNV-1a over each file's content and
/// the ABI, so a changed grammar, version or generator pin is a new entry.
fn cache_key(inputs: &[&Path]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(format!("abi {}\n", tree_sitter_generate::ABI_VERSION_MAX).as_bytes());
    for path in inputs {
        let content =
            fs::read(path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        feed(&content);
        feed(b"\n");
    }
    format!("{hash:016x}")
}

fn is_complete(entry: &Path) -> bool {
    GENERATED.iter().all(|file| entry.join(file).is_file())
}

fn copy_generated(from: &Path, to: &Path) {
    for file in GENERATED {
        let target = to.join(file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("creating {}: {error}", parent.display()));
        }
        fs::copy(from.join(file), &target).unwrap_or_else(|error| {
            panic!(
                "copying {} to {}: {error}",
                from.join(file).display(),
                target.display()
            )
        });
    }
}

/// Stores a generation under `entry`, written whole under a temporary name and
/// renamed into place, so a build that reads it sees all of it or nothing. A
/// cache that cannot be written (read-only, another user's) is not an error:
/// the build has its parser already.
fn store(generated: &Path, entry: &Path) {
    let Some(parent) = entry.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let temp = parent.join(format!(".{}.{}", std::process::id(), key_name(entry)));
    let _ = fs::remove_dir_all(&temp);
    let copied = GENERATED.iter().all(|file| {
        let target = temp.join(file);
        target
            .parent()
            .is_some_and(|dir| fs::create_dir_all(dir).is_ok())
            && fs::copy(generated.join(file), target).is_ok()
    });
    if !copied || fs::rename(&temp, entry).is_err() {
        // Another build stored the same entry first, or the folder is not writable.
        let _ = fs::remove_dir_all(&temp);
    }
}

fn key_name(entry: &Path) -> String {
    entry
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
