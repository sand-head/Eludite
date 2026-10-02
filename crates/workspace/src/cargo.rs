//! The Cargo workspace model (brief 0019), from `cargo metadata --format-version 1 --no-deps --offline`: the
//! workspace root, its member packages with their targets (bin, lib, proc-macro, example, test, bench, build script)
//! and declared dependencies. `--no-deps` lists the members only and resolves nothing, so reading it needs no network
//! and takes milliseconds; `--offline` makes sure. The shell runs the command off the UI thread
//! ([`metadata_command`]) and hands the output to [`CargoWorkspace::from_metadata`].

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// What a Cargo target builds (the first of `cargo metadata`'s `kind` list; the library kinds are one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetKind {
    Lib,
    ProcMacro,
    Bin,
    Example,
    Test,
    Bench,
    /// A build script (`build.rs`).
    CustomBuild,
}

impl TargetKind {
    fn parse(kinds: &[String]) -> Self {
        match kinds.first().map(String::as_str) {
            Some("bin") => TargetKind::Bin,
            Some("proc-macro") => TargetKind::ProcMacro,
            Some("example") => TargetKind::Example,
            Some("test") => TargetKind::Test,
            Some("bench") => TargetKind::Bench,
            Some("custom-build") => TargetKind::CustomBuild,
            // lib, rlib, dylib, cdylib, staticlib
            _ => TargetKind::Lib,
        }
    }

    /// The name `eludite.workspace.tree` and the Workspace window use (`bin`, `lib`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Lib => "lib",
            TargetKind::ProcMacro => "proc-macro",
            TargetKind::Bin => "bin",
            TargetKind::Example => "example",
            TargetKind::Test => "test",
            TargetKind::Bench => "bench",
            TargetKind::CustomBuild => "custom-build",
        }
    }

    /// Whether a package's label names this kind (`eludite (bin)`): what it builds, not its tests.
    pub fn is_product(self) -> bool {
        matches!(
            self,
            TargetKind::Lib | TargetKind::ProcMacro | TargetKind::Bin
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoTarget {
    pub name: String,
    pub kind: TargetKind,
    /// The target's root source file.
    pub src_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoDependency {
    pub name: String,
    /// The version requirement (`*` for a path dependency without one).
    pub req: String,
    pub kind: DependencyKind,
    /// A path dependency's folder.
    pub path: Option<PathBuf>,
}

/// One member package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoPackage {
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<CargoTarget>,
    pub dependencies: Vec<CargoDependency>,
}

impl CargoPackage {
    /// The package's folder (where its `Cargo.toml` is).
    pub fn dir(&self) -> &Path {
        self.manifest_path.parent().unwrap_or(Path::new(""))
    }

    /// The kinds it builds (`lib`, `bin`, `proc-macro`), each once, libraries first.
    pub fn product_kinds(&self) -> Vec<TargetKind> {
        let mut kinds: Vec<TargetKind> = self
            .targets
            .iter()
            .map(|t| t.kind)
            .filter(|k| k.is_product())
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }
}

/// A Cargo workspace (or a single package, which is a workspace of one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoWorkspace {
    pub root: PathBuf,
    /// `<root>/Cargo.toml`.
    pub manifest: PathBuf,
    pub target_directory: PathBuf,
    /// Members in `cargo metadata` order.
    pub members: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    workspace_members: Vec<String>,
    workspace_root: PathBuf,
    target_directory: PathBuf,
}

#[derive(Deserialize)]
struct RawPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: PathBuf,
    #[serde(default)]
    targets: Vec<RawTarget>,
    #[serde(default)]
    dependencies: Vec<RawDependency>,
}

#[derive(Deserialize)]
struct RawTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

#[derive(Deserialize)]
struct RawDependency {
    name: String,
    #[serde(default)]
    req: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
}

/// The program and arguments that print the metadata of the workspace containing `manifest` (a `Cargo.toml`).
pub fn metadata_command(manifest: &Path) -> (String, Vec<String>) {
    (
        "cargo".into(),
        vec![
            "metadata".into(),
            "--format-version".into(),
            "1".into(),
            "--no-deps".into(),
            "--offline".into(),
            "--manifest-path".into(),
            manifest.to_string_lossy().into_owned(),
        ],
    )
}

impl CargoWorkspace {
    /// The model from `cargo metadata --format-version 1` output.
    pub fn from_metadata(json: &str) -> Result<Self, String> {
        let raw: RawMetadata =
            serde_json::from_str(json).map_err(|e| format!("unexpected cargo metadata: {e}"))?;
        let members = raw
            .packages
            .into_iter()
            .filter(|p| raw.workspace_members.contains(&p.id))
            .map(|p| CargoPackage {
                name: p.name,
                version: p.version,
                manifest_path: p.manifest_path,
                targets: p
                    .targets
                    .into_iter()
                    .map(|t| CargoTarget {
                        kind: TargetKind::parse(&t.kind),
                        name: t.name,
                        src_path: t.src_path,
                    })
                    .collect(),
                dependencies: p
                    .dependencies
                    .into_iter()
                    .map(|d| CargoDependency {
                        name: d.name,
                        req: d.req,
                        kind: match d.kind.as_deref() {
                            Some("dev") => DependencyKind::Dev,
                            Some("build") => DependencyKind::Build,
                            _ => DependencyKind::Normal,
                        },
                        path: d.path,
                    })
                    .collect(),
            })
            .collect();
        Ok(Self {
            manifest: raw.workspace_root.join("Cargo.toml"),
            root: raw.workspace_root,
            target_directory: raw.target_directory,
            members,
        })
    }

    /// The member whose folder is the deepest one containing `file`.
    pub fn package_of(&self, file: &Path) -> Option<&CargoPackage> {
        self.members
            .iter()
            .filter(|p| file.starts_with(p.dir()))
            .max_by_key(|p| p.dir().components().count())
    }

    /// The member named `name`.
    pub fn package(&self, name: &str) -> Option<&CargoPackage> {
        self.members.iter().find(|p| p.name == name)
    }

    /// The root folder's name, which the Workspace window shows for it.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Cargo".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo metadata --format-version 1 --no-deps --offline` of this repository, recorded at brief 0019 with the
    /// checkout's root replaced by `/w/eludite`.
    const RECORDED: &str = include_str!("testdata/cargo-metadata.json");

    #[test]
    fn members_targets_and_dependencies_from_recorded_metadata() {
        let ws = CargoWorkspace::from_metadata(RECORDED).unwrap();
        assert_eq!(ws.root, Path::new("/w/eludite"));
        assert_eq!(ws.manifest, Path::new("/w/eludite/Cargo.toml"));
        assert_eq!(ws.target_directory, Path::new("/w/eludite/target"));
        assert_eq!(ws.name(), "eludite");
        assert_eq!(ws.members.len(), 16);

        let shell = ws.package("eludite").unwrap();
        assert_eq!(shell.version, "0.1.0");
        assert_eq!(shell.dir(), Path::new("/w/eludite/crates/eludite"));
        assert_eq!(shell.product_kinds(), [TargetKind::Bin]);
        let relay = shell.targets.iter().find(|t| t.name == "relay").unwrap();
        assert_eq!(relay.kind, TargetKind::Test);
        assert_eq!(
            relay.src_path,
            Path::new("/w/eludite/crates/eludite/tests/relay.rs")
        );
        let clock = shell
            .dependencies
            .iter()
            .find(|d| d.name == "clock")
            .unwrap();
        assert_eq!(clock.kind, DependencyKind::Normal);
        assert_eq!(
            clock.path.as_deref(),
            Some(Path::new("/w/eludite/vendor/clock"))
        );
        assert!(
            shell
                .dependencies
                .iter()
                .any(|d| d.name == "tempfile" && d.kind == DependencyKind::Dev)
        );

        let lsp = ws.package("eludite-lsp").unwrap();
        assert_eq!(lsp.product_kinds(), [TargetKind::Lib]);
        assert!(
            lsp.targets
                .iter()
                .any(|t| t.name == "fake_host" && t.kind == TargetKind::Test)
        );
    }

    #[test]
    fn files_belong_to_the_deepest_member() {
        let ws = CargoWorkspace::from_metadata(RECORDED).unwrap();
        let of = |p: &str| ws.package_of(Path::new(p)).map(|p| p.name.as_str());
        assert_eq!(
            of("/w/eludite/crates/editor/src/buffer.rs"),
            Some("eludite-editor")
        );
        assert_eq!(
            of("/w/eludite/protocol/rust/src/lsp.rs"),
            Some("eludite-protocol")
        );
        assert_eq!(of("/w/eludite/docs/PLAN.md"), None);
    }

    #[test]
    fn target_kinds() {
        let k =
            |s: &[&str]| TargetKind::parse(&s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(k(&["cdylib", "rlib"]), TargetKind::Lib);
        assert_eq!(k(&["proc-macro"]), TargetKind::ProcMacro);
        assert_eq!(k(&["custom-build"]), TargetKind::CustomBuild);
        assert_eq!(TargetKind::Example.as_str(), "example");
        assert!(!TargetKind::Bench.is_product());
    }

    #[test]
    fn bad_metadata_is_an_error() {
        assert!(CargoWorkspace::from_metadata("{}").is_err());
        assert!(CargoWorkspace::from_metadata("error: could not find `Cargo.toml`").is_err());
    }

    #[test]
    fn the_metadata_command_needs_no_network() {
        let (program, args) = metadata_command(Path::new("/w/Cargo.toml"));
        assert_eq!(program, "cargo");
        assert!(args.contains(&"--no-deps".to_owned()));
        assert!(args.contains(&"--offline".to_owned()));
        assert_eq!(args.last().unwrap(), "/w/Cargo.toml");
    }
}
