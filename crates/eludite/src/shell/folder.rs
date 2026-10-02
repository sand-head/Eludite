//! File > Open Folder (brief 0019, `eludite.workspace.open_folder`): a folder, or the folder of a `Cargo.toml`.
//!
//! - **At once** (the command's handler, on the UI thread): the folder becomes the Workspace window's root, with a
//!   loading node for each part found: the .NET solution (at the root or one folder down, an `.slnx` before an
//!   `.sln`), handed to
//!   `eludite-host` exactly as File > Open > Project/Solution does, and the Cargo workspace.
//! - **Off the UI thread**: the folder's files are listed (`eludite-workspace`'s `folder`), and the Cargo workspace is
//!   read with `cargo metadata --format-version 1 --no-deps --offline` (no network). Each result replaces its loading
//!   node when it arrives; a result for a folder that is no longer open is dropped.
//! - **Language servers** start with their first document (`servers`), rooted at the Cargo workspace root.
//! - `eludite.workspace.tree` lists the projects of every part: `csproj`, `cargo` (with targets and dependencies)
//!   and `folder` for a folder with neither.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eludite_commands::CommandError;
use eludite_commands::workspace::{self, OpenFolderOutput, WorkspaceOutput};
use eludite_commands::workspace_tree::{WorkspaceProject, WorkspaceTarget, WorkspaceTreeOutput};
use eludite_lsp::ServerRegistration;
use eludite_lsp::host::{SolutionState, SolutionTree, TreeProjectKind};
use eludite_workspace::cargo::{CargoWorkspace, metadata_command};
use eludite_workspace::explorer::{Part, SolutionModel, WorkspaceParts};
use eludite_workspace::folder::{self as folder_model, FolderListing};
use gpui::{AppContext as _, Context, PathPromptOptions, Window};
use serde_json::json;

use super::Shell;
use super::documents::trace;

/// The Cargo part of an open folder.
#[derive(Debug, Clone)]
pub enum CargoState {
    Loading,
    Loaded(Box<CargoWorkspace>),
    Failed(String),
}

/// When the steps of opening a folder happened (the brief 0019 report).
#[derive(Debug, Default, Clone)]
// Read by the tests and the `--timings-out` harness.
#[allow(dead_code)]
pub struct FolderTimings {
    pub opened: Option<Instant>,
    pub listed: Option<Instant>,
    pub cargo: Option<Instant>,
}

/// The open folder.
#[derive(Debug, Clone)]
pub struct OpenFolder {
    pub root: PathBuf,
    pub solution: Option<PathBuf>,
    pub cargo_manifest: Option<PathBuf>,
    pub cargo: Option<CargoState>,
    pub listing: Option<FolderListing>,
    /// The host's tree for the solution, once it arrived.
    pub tree: Option<SolutionTree>,
    pub solution_failed: Option<String>,
    pub timings: FolderTimings,
    /// Which open this is: results for an older one are dropped.
    pub ticket: u64,
}

/// `cargo metadata` for `manifest`: the workspace, or why not. Blocking: background threads only.
pub fn read_cargo_metadata(manifest: &Path) -> Result<CargoWorkspace, String> {
    let (program, args) = metadata_command(manifest);
    let out = std::process::Command::new(&program)
        .args(&args)
        .current_dir(manifest.parent().unwrap_or(Path::new(".")))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("{program} metadata: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err
            .lines()
            .find(|l| l.starts_with("error"))
            .unwrap_or("cargo metadata failed")
            .to_owned());
    }
    CargoWorkspace::from_metadata(&String::from_utf8_lossy(&out.stdout))
}

impl Shell {
    /// The open folder, if any.
    // Read by the tests and the `--timings-out` harness.
    #[allow(dead_code)]
    pub fn folder(&self) -> Option<&OpenFolder> {
        self.folder.as_ref()
    }

    /// The folder relative paths resolve against: the open folder, else the solution's folder.
    pub fn workspace_root(&self) -> Option<PathBuf> {
        self.folder
            .as_ref()
            .map(|f| f.root.clone())
            .or_else(|| self.solution_dir())
    }

    /// A project's name as the Workspace window shows it: the Cargo package for a `Cargo.toml` (brief 0019), else
    /// the project file's name without extension.
    pub(super) fn project_display(&self, project: &str) -> String {
        let path = Path::new(project);
        if path.file_name().is_some_and(|n| n == "Cargo.toml")
            && let Some(ws) = self.cargo_workspace()
            && let Some(p) = ws.members.iter().find(|p| {
                super::documents::normalize_path(&p.manifest_path)
                    == super::documents::normalize_path(path)
            })
        {
            return p.name.clone();
        }
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The root a generic server for `path` gets from the open folder: the Cargo workspace root (or, while
    /// `cargo metadata` runs, the folder holding the `Cargo.toml`) when the registration's root marker is the
    /// manifest the folder opened and `path` is inside it.
    pub(super) fn folder_root_for(&self, reg: &ServerRegistration, path: &Path) -> Option<PathBuf> {
        let f = self.folder.as_ref()?;
        let manifest = f.cargo_manifest.as_ref()?;
        let marker = manifest.file_name()?.to_string_lossy().into_owned();
        if !reg.root_markers.contains(&marker) {
            return None;
        }
        let root = match &f.cargo {
            Some(CargoState::Loaded(ws)) => ws.root.clone(),
            _ => manifest.parent()?.to_path_buf(),
        };
        path.starts_with(&root).then_some(root)
    }

    pub(super) fn prompt_open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            if let Some(path) = chosen {
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.run(
                        workspace::WORKSPACE_OPEN_FOLDER,
                        json!({ "path": path.to_string_lossy() }),
                        window,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// `eludite.workspace.open_folder`.
    pub(super) fn open_folder(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let p = std::path::absolute(Path::new(path))
            .map(|p| super::documents::normalize_path(&p))
            .map_err(|e| CommandError::InvalidInput(format!("{path}: {e}")))?;
        let (root, manifest) = if p.is_dir() {
            let manifest = folder_model::find_cargo_manifest(&p);
            (p, manifest)
        } else if p.is_file() && p.file_name().is_some_and(|n| n == "Cargo.toml") {
            let root = p.parent().map(Path::to_path_buf).unwrap_or_default();
            (root, Some(p))
        } else {
            return Err(CommandError::InvalidInput(format!(
                "{}: expected a folder or a Cargo.toml",
                p.display()
            )));
        };
        let solution = folder_model::find_solution(&root);
        trace(format_args!(
            "open folder {} (solution {:?}, Cargo {:?})",
            root.display(),
            solution,
            manifest
        ));
        // Another folder: its language servers go (they were rooted in the old one).
        if self.folder.as_ref().is_some_and(|f| f.root != root) {
            for rx in self.shutdown_generic() {
                drop(rx);
            }
            self.generic.clear();
        }
        let ticket = self.folder.as_ref().map_or(1, |f| f.ticket + 1);
        let now = Instant::now();
        self.folder = Some(OpenFolder {
            root: root.clone(),
            solution: solution.clone(),
            cargo_manifest: manifest.clone(),
            cargo: manifest.as_ref().map(|_| CargoState::Loading),
            listing: None,
            tree: None,
            solution_failed: None,
            timings: FolderTimings {
                opened: Some(now),
                ..Default::default()
            },
            ticket,
        });
        self.update_settings_dir();
        self.timings = super::Timings {
            open: Some(now),
            ..Default::default()
        };
        match &solution {
            Some(sln) if self.solution.as_ref() != Some(sln) => {
                self.session.open(sln.clone());
            }
            Some(_) => {}
            None => {
                if self.session.close().is_some() {
                    trace(format_args!("closed the solution for a folder without one"));
                }
            }
        }
        window.set_window_title(&format!(
            "{} - Eludite",
            root.file_name()
                .map_or_else(|| root.to_string_lossy(), |n| n.to_string_lossy())
        ));
        self.recompose(cx);

        // The folder's files and the Cargo workspace, off the UI thread.
        let list_root = root.clone();
        let listing = cx.background_spawn(async move {
            folder_model::list_folder(&list_root, folder_model::MAX_FILES)
        });
        cx.spawn_in(window, async move |this, cx| {
            let listing = listing.await;
            let _ = this.update(cx, |shell, cx| {
                if let Some(f) = shell.folder.as_mut().filter(|f| f.ticket == ticket) {
                    trace(format_args!(
                        "folder listed: {} files{}",
                        listing.files.len(),
                        if listing.truncated {
                            " (truncated)"
                        } else {
                            ""
                        }
                    ));
                    f.timings.listed = Some(Instant::now());
                    f.listing = Some(listing);
                    shell.recompose(cx);
                }
            });
        })
        .detach();
        if let Some(manifest) = manifest.clone() {
            let read = cx.background_spawn(async move { read_cargo_metadata(&manifest) });
            cx.spawn_in(window, async move |this, cx| {
                let result = read.await;
                let _ = this.update(cx, |shell, cx| {
                    if let Some(f) = shell.folder.as_mut().filter(|f| f.ticket == ticket) {
                        f.timings.cargo = Some(Instant::now());
                        f.cargo = Some(match result {
                            Ok(ws) => {
                                trace(format_args!(
                                    "cargo metadata: {} members at {}",
                                    ws.members.len(),
                                    ws.root.display()
                                ));
                                CargoState::Loaded(Box::new(ws))
                            }
                            Err(e) => {
                                eprintln!("eludite: cargo metadata: {e}");
                                CargoState::Failed(e)
                            }
                        });
                        shell.recompose(cx);
                    }
                });
            })
            .detach();
        }
        Ok(WorkspaceOutput::OpenFolder(OpenFolderOutput {
            root: root.to_string_lossy().into_owned(),
            state: "loading".into(),
            solution: solution.map(|p| p.to_string_lossy().into_owned()),
            cargo_manifest: manifest.map(|p| p.to_string_lossy().into_owned()),
        }))
    }

    /// The open folder's Workspace tree and `eludite.workspace.tree`, from what has loaded so far.
    pub(super) fn recompose(&mut self, cx: &mut Context<Self>) {
        let Some(f) = &self.folder else {
            return;
        };
        let failed = f.solution_failed.clone();
        let model = {
            let solution = f.solution.as_deref().map(|p| {
                let part = match (&f.tree, &failed) {
                    (Some(t), _) => Part::Loaded(t),
                    (None, Some(why)) => Part::Failed(why.as_str()),
                    (None, None) => Part::Loading,
                };
                (p, part)
            });
            let cargo = f.cargo_manifest.as_deref().map(|m| {
                let part = match &f.cargo {
                    Some(CargoState::Loaded(ws)) => Part::Loaded(ws.as_ref()),
                    Some(CargoState::Failed(why)) => Part::Failed(why.as_str()),
                    _ => Part::Loading,
                };
                (m, part)
            });
            SolutionModel::compose(&WorkspaceParts {
                root: &f.root,
                solution,
                cargo,
                listing: f.listing.as_ref(),
            })
        };
        let active = self
            .controller
            .active_document()
            .filter(|id| self.documents.contains_key(id));
        self.explorer.update(cx, |e, cx| {
            e.set_model(model, cx);
            if let Some(id) = active {
                e.reveal(Path::new(&id), cx);
            }
        });
        self.publish_workspace_tree();
        self.update_error_list(cx);
        cx.notify();
    }

    /// What `eludite.workspace.tree` returns from now on: the open folder's parts, or the open solution's projects.
    pub(super) fn publish_workspace_tree(&mut self) {
        let mut out = WorkspaceTreeOutput::default();
        let tree = self
            .folder
            .as_ref()
            .and_then(|f| f.tree.clone())
            .or_else(|| self.last_tree.clone());
        let mut loading = false;
        let mut failed = false;
        if let Some(t) = &tree {
            for p in &t.projects {
                out.projects.push(WorkspaceProject {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    kind: "csproj".into(),
                    msbuild: Some(
                        match p.kind {
                            TreeProjectKind::Sdk => "sdk",
                            TreeProjectKind::Legacy => "legacy",
                        }
                        .into(),
                    ),
                    target_frameworks: Some(p.target_frameworks.clone()),
                    version: None,
                    targets: None,
                    dependencies: None,
                    files: p.files.iter().map(|f| f.path.clone()).collect(),
                    error: p.error.clone(),
                    startup: None,
                });
            }
        }
        match &self.folder {
            Some(f) => {
                out.root = Some(f.root.to_string_lossy().into_owned());
                if f.solution.is_some() && f.tree.is_none() {
                    if f.solution_failed.is_some() {
                        failed = true;
                    } else {
                        loading = true;
                    }
                }
                match &f.cargo {
                    Some(CargoState::Loaded(ws)) => {
                        for p in &ws.members {
                            let dir = p.dir();
                            out.projects.push(WorkspaceProject {
                                name: p.name.clone(),
                                path: p.manifest_path.to_string_lossy().into_owned(),
                                kind: "cargo".into(),
                                msbuild: None,
                                target_frameworks: None,
                                version: Some(p.version.clone()),
                                targets: Some(
                                    p.targets
                                        .iter()
                                        .map(|t| WorkspaceTarget {
                                            name: t.name.clone(),
                                            kind: t.kind.as_str().into(),
                                            path: t.src_path.to_string_lossy().into_owned(),
                                        })
                                        .collect(),
                                ),
                                dependencies: Some(
                                    p.dependencies.iter().map(|d| d.name.clone()).collect(),
                                ),
                                files: f
                                    .listing
                                    .as_ref()
                                    .map(|l| {
                                        l.files
                                            .iter()
                                            .filter(|x| {
                                                ws.package_of(x).is_some_and(|q| q.dir() == dir)
                                            })
                                            .map(|x| x.to_string_lossy().into_owned())
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                error: None,
                                startup: None,
                            });
                        }
                    }
                    Some(CargoState::Loading) => loading = true,
                    Some(CargoState::Failed(why)) => {
                        failed = true;
                        if let Some(m) = &f.cargo_manifest {
                            out.projects.push(WorkspaceProject {
                                name: f
                                    .root
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "Cargo".into()),
                                path: m.to_string_lossy().into_owned(),
                                kind: "cargo".into(),
                                msbuild: None,
                                target_frameworks: None,
                                version: None,
                                targets: None,
                                dependencies: None,
                                files: Vec::new(),
                                error: Some(why.clone()),
                                startup: None,
                            });
                        }
                    }
                    None => {}
                }
                if f.solution.is_none() && f.cargo.is_none() {
                    out.projects.push(WorkspaceProject {
                        name: f
                            .root
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        path: f.root.to_string_lossy().into_owned(),
                        kind: "folder".into(),
                        msbuild: None,
                        target_frameworks: None,
                        version: None,
                        targets: None,
                        dependencies: None,
                        files: f
                            .listing
                            .as_ref()
                            .map(|l| {
                                l.files
                                    .iter()
                                    .map(|x| x.to_string_lossy().into_owned())
                                    .collect()
                            })
                            .unwrap_or_default(),
                        error: None,
                        startup: None,
                    });
                }
                if f.listing.is_none() {
                    loading = true;
                }
                out.state = if failed {
                    "failed"
                } else if loading {
                    "loading"
                } else {
                    "loaded"
                }
                .into();
            }
            None => {
                out.root = self
                    .solution_dir()
                    .map(|d| d.to_string_lossy().into_owned());
                out.state = match (&self.solution, &tree, self.solution_state) {
                    (None, _, _) => "none",
                    (Some(_), _, Some(SolutionState::Failed)) => "failed",
                    (Some(_), Some(_), _) => "loaded",
                    (Some(_), None, _) => "loading",
                }
                .into();
            }
        }
        // The startup project (brief 0020).
        if let Some(startup) = self
            .startup_project()
            .map(|p| super::documents::normalize_path(&p))
        {
            for p in out.projects.iter_mut().filter(|p| p.kind == "csproj") {
                if super::documents::normalize_path(Path::new(&p.path)) == startup {
                    p.startup = Some(true);
                }
            }
        }
        *self
            .workspace_tree
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = out;
    }
}
