//! What a lens's command does (brief 0052; host-rpc.md, "CodeLens" and "CodeLens from a generic server").
//!
//! Lens commands are client commands: the server names what it wants the client to do, and the client does it. The
//! shell knows these, whichever server sent them:
//!
//! - `eludite.editor.find_references` (Roslyn's `roslyn.client.peekReferences`, mapped by the host): the References
//!   popup for the symbol at a position.
//! - `eludite.test.run` and `eludite.test.debug` (Roslyn's `dotnet.test.run`, mapped by the host): run or debug the
//!   tests of a member, named as written.
//! - `rust-analyzer.runSingle` and `rust-analyzer.debugSingle` with a Cargo test runnable: run or debug a libtest
//!   test (or the tests of a module).
//! - `rust-analyzer.showReferences` and `editor.action.showReferences` (`[uri, position, locations]`): the References
//!   popup with the locations given, which are references or implementations by the title.
//!
//! [`classify`] turns a command into a [`LensCommand`]; a command it does not know is `None` and the lens is not shown.
//! An unresolved lens (no command yet) is a references lens for both Roslyn and rust-analyzer.

use eludite_protocol::lsp::{Command, Location, Position, Range};
use serde_json::Value;

pub const FIND_REFERENCES: &str = "eludite.editor.find_references";
pub const TEST_RUN: &str = "eludite.test.run";
pub const TEST_DEBUG: &str = "eludite.test.debug";
pub const RA_RUN_SINGLE: &str = "rust-analyzer.runSingle";
pub const RA_DEBUG_SINGLE: &str = "rust-analyzer.debugSingle";
pub const RA_SHOW_REFERENCES: &str = "rust-analyzer.showReferences";
pub const VSCODE_SHOW_REFERENCES: &str = "editor.action.showReferences";

/// The family of a lens: the settings `editor.codeLens.references` and `editor.codeLens.tests` choose by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LensKind {
    References,
    Implementations,
    RunTest,
    DebugTest,
}

impl LensKind {
    /// References and implementations (`editor.codeLens.references`).
    pub fn is_references(self) -> bool {
        matches!(self, LensKind::References | LensKind::Implementations)
    }

    /// Run and debug (`editor.codeLens.tests`).
    pub fn is_test(self) -> bool {
        matches!(self, LensKind::RunTest | LensKind::DebugTest)
    }
}

/// What activating the lens does.
#[derive(Debug, Clone, PartialEq)]
pub enum LensTarget {
    /// The References popup for the symbol at `position` in `uri`: the locations given (rust-analyzer, TypeScript),
    /// or asked for with `textDocument/references` (Roslyn's lens).
    References {
        uri: String,
        position: Position,
        locations: Option<Vec<Location>>,
    },
    /// The .NET tests of the lens's document whose method (or class) is `member`, through the Test Explorer.
    Test { member: String, range: Range },
    /// A Cargo test: libtest's `name` (an exact test, or a module whose tests all run), in `package` when the
    /// runnable names it.
    CargoTest {
        name: String,
        exact: bool,
        package: Option<String>,
    },
}

/// A lens command the shell knows.
#[derive(Debug, Clone, PartialEq)]
pub struct LensCommand {
    pub kind: LensKind,
    pub title: String,
    pub target: LensTarget,
}

/// The kind of an unresolved lens (no command yet): Roslyn's and rust-analyzer's are references lenses until resolved.
pub const UNRESOLVED_KIND: LensKind = LensKind::References;

/// What `command` does, or `None` for a command the shell does not know (the lens is not shown).
pub fn classify(command: &Command) -> Option<LensCommand> {
    let args = command.arguments.as_deref().unwrap_or_default();
    let title = command.title.clone();
    let (kind, target) = match command.command.as_str() {
        FIND_REFERENCES => {
            let a = args.first()?;
            (
                LensKind::References,
                LensTarget::References {
                    uri: a.get("uri")?.as_str()?.to_owned(),
                    position: serde_json::from_value(a.get("position")?.clone()).ok()?,
                    locations: None,
                },
            )
        }
        TEST_RUN | TEST_DEBUG => {
            let a = args.first()?;
            (
                if command.command == TEST_RUN {
                    LensKind::RunTest
                } else {
                    LensKind::DebugTest
                },
                LensTarget::Test {
                    member: a.get("member")?.as_str()?.to_owned(),
                    range: serde_json::from_value(a.get("range")?.clone()).ok()?,
                },
            )
        }
        RA_RUN_SINGLE | RA_DEBUG_SINGLE => {
            let (name, exact, package) = cargo_test(args.first()?)?;
            (
                if command.command == RA_RUN_SINGLE {
                    LensKind::RunTest
                } else {
                    LensKind::DebugTest
                },
                LensTarget::CargoTest {
                    name,
                    exact,
                    package,
                },
            )
        }
        RA_SHOW_REFERENCES | VSCODE_SHOW_REFERENCES => {
            let uri = args.first()?.as_str()?.to_owned();
            let position = serde_json::from_value(args.get(1)?.clone()).ok()?;
            let locations = args
                .get(2)
                .and_then(|l| serde_json::from_value::<Vec<Location>>(l.clone()).ok())
                .unwrap_or_default();
            let kind = if title.to_ascii_lowercase().contains("implementation") {
                LensKind::Implementations
            } else {
                LensKind::References
            };
            (
                kind,
                LensTarget::References {
                    uri,
                    position,
                    locations: Some(locations),
                },
            )
        }
        _ => return None,
    };
    Some(LensCommand {
        kind,
        title,
        target,
    })
}

/// A rust-analyzer runnable's libtest test: `(name, exact, package)`, or `None` when it is not a Cargo test
/// (`cargo run`, a doctest, a bench).
fn cargo_test(runnable: &Value) -> Option<(String, bool, Option<String>)> {
    if runnable.get("kind").and_then(Value::as_str) != Some("cargo") {
        return None;
    }
    let args = runnable.get("args")?;
    let strings = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let cargo = strings("cargoArgs");
    if cargo.first().map(String::as_str) != Some("test") || cargo.iter().any(|a| a == "--doc") {
        return None;
    }
    let package = cargo
        .iter()
        .position(|a| a == "--package" || a == "-p")
        .and_then(|i| cargo.get(i + 1))
        .cloned();
    let exec = strings("executableArgs");
    let name = exec.first().filter(|n| !n.starts_with('-'))?.clone();
    let exact = exec.iter().any(|a| a == "--exact");
    Some((name, exact, package))
}

/// The count a references title shows (`"3 references"`, `"1 reference"`, `"99+ references"`, `"2 implementations"`):
/// `(count, more)` where `more` is Roslyn's `+`; `None` when it has no count (`"- references"`).
pub fn title_count(title: &str) -> Option<(u32, bool)> {
    let first = title.split_whitespace().next()?;
    let (digits, more) = match first.strip_suffix('+') {
        Some(d) => (d, true),
        None => (first, false),
    };
    digits.parse().ok().map(|n| (n, more))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cmd(title: &str, command: &str, arguments: Value) -> Command {
        Command {
            title: title.into(),
            command: command.into(),
            arguments: arguments.as_array().cloned(),
        }
    }

    #[test]
    fn mapped_roslyn_commands() {
        let pos = json!({"line": 2, "character": 13});
        let refs = classify(&cmd(
            "3 references",
            FIND_REFERENCES,
            json!([{"uri": "file:///a.cs", "path": "/a.cs", "position": pos}]),
        ))
        .unwrap();
        assert_eq!(refs.kind, LensKind::References);
        assert_eq!(
            refs.target,
            LensTarget::References {
                uri: "file:///a.cs".into(),
                position: Position {
                    line: 2,
                    character: 13
                },
                locations: None
            }
        );
        let range = json!({"start": pos, "end": {"line": 2, "character": 28}});
        let debug = classify(&cmd(
            "Debug Test",
            TEST_DEBUG,
            json!([{"uri": "file:///a.cs", "path": "/a.cs", "range": range, "member": "Adds"}]),
        ))
        .unwrap();
        assert_eq!(debug.kind, LensKind::DebugTest);
        assert!(matches!(debug.target, LensTarget::Test { ref member, .. } if member == "Adds"));
        // Roslyn's own ids never reach the shell unmapped; a malformed argument is no lens.
        assert!(classify(&cmd("Run Test", "dotnet.test.run", json!([{}]))).is_none());
        assert!(classify(&cmd("Run Test", TEST_RUN, json!([]))).is_none());
    }

    #[test]
    fn rust_analyzer_runnables_and_references() {
        let runnable = |cargo: Value, exec: Value| {
            json!([{"label": "test tests::adds", "kind": "cargo",
                    "args": {"workspaceRoot": "/w", "cargoArgs": cargo, "executableArgs": exec}}])
        };
        let run = classify(&cmd(
            "\u{25b6}\u{fe0e} Run Test",
            RA_RUN_SINGLE,
            runnable(
                json!(["test", "--package", "corpus-tests", "--lib"]),
                json!(["tests::adds", "--exact", "--nocapture"]),
            ),
        ))
        .unwrap();
        assert_eq!(run.kind, LensKind::RunTest);
        assert_eq!(
            run.target,
            LensTarget::CargoTest {
                name: "tests::adds".into(),
                exact: true,
                package: Some("corpus-tests".into())
            }
        );
        let module = classify(&cmd(
            "Debug",
            RA_DEBUG_SINGLE,
            runnable(json!(["test", "--lib"]), json!(["tests", "--nocapture"])),
        ))
        .unwrap();
        assert_eq!(module.kind, LensKind::DebugTest);
        assert!(matches!(module.target, LensTarget::CargoTest { exact: false, .. }));
        // `cargo run` and doctests are not tests.
        assert!(
            classify(&cmd(
                "Run",
                RA_RUN_SINGLE,
                runnable(json!(["run", "--bin", "x"]), json!([]))
            ))
            .is_none()
        );
        assert!(
            classify(&cmd(
                "Run Doctest",
                RA_RUN_SINGLE,
                runnable(json!(["test", "--doc"]), json!(["f", "--exact"]))
            ))
            .is_none()
        );
        let loc = json!({"uri": "file:///w/b.rs", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}});
        let imps = classify(&cmd(
            "2 implementations",
            RA_SHOW_REFERENCES,
            json!(["file:///w/a.rs", {"line": 0, "character": 7}, [loc, loc]]),
        ))
        .unwrap();
        assert_eq!(imps.kind, LensKind::Implementations);
        assert!(
            matches!(imps.target, LensTarget::References { locations: Some(ref l), .. } if l.len() == 2)
        );
        assert!(classify(&cmd("x", "rust-analyzer.gotoLocation", json!([]))).is_none());
    }

    #[test]
    fn counts_in_titles() {
        assert_eq!(title_count("3 references"), Some((3, false)));
        assert_eq!(title_count("1 reference"), Some((1, false)));
        assert_eq!(title_count("99+ references"), Some((99, true)));
        assert_eq!(title_count("2 implementations"), Some((2, false)));
        assert_eq!(title_count("- references"), None);
        assert!(LensKind::Implementations.is_references() && LensKind::DebugTest.is_test());
    }
}
