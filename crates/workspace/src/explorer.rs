//! Workspace's model, fed from the host's `eludite/solution/tree` answer (brief 0012).
//!
//! Visual Studio semantics, as far as this brief goes:
//! - the solution node reads `Solution 'Name' (N of M projects)`;
//! - a project node reads `Name (net10.0)`, with its target frameworks in parentheses; a project that did not
//!   evaluate reads `Name (load failed)` and has no children;
//! - folders mirror the file system under each project directory; a linked file appears at its `Link` path, any
//!   other file outside the project directory at the project root;
//! - nested files: `DependentUpon` first, then by name: `X.aspx.cs` under `X.aspx` (any `A.b.c` under a sibling
//!   `A.b`), `X.designer.cs` under `X` or `X.cs` or `X.resx`;
//! - folders before files, each sorted case-insensitively.
//!
//! Node ids are stable strings, so a view can keep its expanded set across tree refreshes.
//!
//! An opened folder (brief 0019, [`SolutionModel::compose`]) is a tree of its own: the folder at the root, with the
//! .NET solution and the Cargo workspace as siblings under it, then the folder's other files. The Cargo workspace
//! node lists its member packages (labelled with what they build: `eludite (bin)`, `eludite-lsp (lib)`), each with
//! a `Targets` folder (every bin, lib, example, test, bench and build script, opening its root source file) and the
//! package's files (`src/`, `tests/`, `Cargo.toml`), then the workspace's `Cargo.toml` and `Cargo.lock`.
//!
//! Nothing is listed twice (brief 0061): a project of the open solution and a package of the open Cargo workspace
//! appear under their solution or workspace only. The folder listing's files are compared with the projects' and
//! packages' folders by their canonical paths (a symlinked or `..` spelling of the root is the same folder), so a
//! project file found on disk does not come back at the workspace root or under a plain folder, and a folder that
//! held nothing else does not appear; a project the host lists twice is shown once.
//!
//! [`Row::file_type`] names a file row's type by its extension (case-insensitive), for the window's icons.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use eludite_protocol::host::{
    Generation, SolutionTree, TreeItemType, TreeProject, TreeProjectKind,
};

use crate::cargo::{CargoPackage, CargoWorkspace, TargetKind};
use crate::folder::FolderListing;

/// What a node stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Solution,
    Project {
        kind: TreeProjectKind,
        web: bool,
        /// Why the project did not evaluate.
        error: Option<String>,
    },
    Folder,
    /// Compile items of a .NET project; `content` for every file of a Cargo package or a folder.
    File {
        item_type: TreeItemType,
    },
    /// The opened folder (brief 0019).
    FolderRoot,
    CargoWorkspace,
    /// A member package and the kinds it builds.
    CargoPackage {
        kinds: Vec<TargetKind>,
    },
    /// A package's `Targets` folder.
    CargoTargets,
    /// One Cargo target; `path` is its root source file.
    CargoTarget {
        kind: TargetKind,
    },
    /// Visual Studio's Dependencies node of a .NET project (brief 0048); `path` is the project file.
    Dependencies,
    /// Its Frameworks, Packages or Projects folder; `path` is the project file.
    DependencyGroup {
        group: DependencyGroup,
    },
    /// A package: a top-level one of the project, or one a package brings in (`transitive`); `path` is the project
    /// file. `warning`: a known vulnerability (NuGet Audit) or a deprecation, the yellow glyph.
    Package {
        id: String,
        version: Option<String>,
        transitive: bool,
        warning: bool,
    },
    /// A shared framework (`Microsoft.NETCore.App`).
    Framework,
    /// A project reference; `path` is the referenced project file.
    ProjectReference,
}

/// The folders of a Dependencies node, in Visual Studio's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyGroup {
    Frameworks,
    Packages,
    Projects,
}

impl NodeKind {
    /// Whether the node is part of a project's Dependencies node.
    pub fn is_dependency(&self) -> bool {
        matches!(
            self,
            NodeKind::Dependencies
                | NodeKind::DependencyGroup { .. }
                | NodeKind::Package { .. }
                | NodeKind::Framework
                | NodeKind::ProjectReference
        )
    }
}

impl NodeKind {
    /// Whether opening the node opens the file at its `path`.
    pub fn opens_file(&self) -> bool {
        matches!(self, NodeKind::File { .. } | NodeKind::CargoTarget { .. })
    }
}

/// What kind of file a row stands for, by its name ([`file_type_of`]); the Workspace window draws an icon per type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    /// `.cs`.
    CSharp,
    /// `.rs`.
    Rust,
    /// `.json`, `.jsonc`.
    Json,
    /// `.md`, `.markdown`.
    Markdown,
    /// MSBuild and other XML: `.csproj`, `.vbproj`, `.fsproj`, `.props`, `.targets`, `.xml`, `.config`, `.resx`,
    /// `.nuspec`, `.xaml`, `.ruleset`.
    Xml,
    /// `.sln`, `.slnx`, `.slnf`.
    Solution,
    /// `.toml`.
    Toml,
    /// `.ts`, `.tsx`, `.mts`, `.cts`.
    TypeScript,
    /// `.js`, `.jsx`, `.mjs`, `.cjs`.
    JavaScript,
    /// `.html`, `.htm`.
    Html,
    /// `.css`, `.scss`, `.sass`, `.less`.
    Css,
    /// `.razor`.
    Razor,
    /// `.cshtml`, `.vbhtml`.
    Cshtml,
    /// Web Forms: `.aspx`, `.ascx`, `.master`, `.ashx`, `.asmx`, `.asax`.
    Aspx,
    /// `.png`, `.jpg`, `.jpeg`, `.gif`, `.svg`, `.ico`, `.bmp`, `.webp`.
    Image,
    /// `.txt`, `.log`.
    Text,
    /// `.sh`, `.bash`, `.zsh`, `.fish`, `.ps1`, `.psm1`, `.psd1`, `.cmd`, `.bat`.
    Shell,
    /// `.yml`, `.yaml`.
    Yaml,
    /// Lockfiles: `*.lock`, `package-lock.json`, `packages.lock.json`, `pnpm-lock.yaml`, `bun.lockb`.
    Lock,
    /// Anything else.
    Other,
}

/// The type of the file named `name` (a file name or a path), by its extension, case-insensitively; lockfiles first.
pub fn file_type_of(name: &str) -> FileType {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "package-lock.json"
            | "packages.lock.json"
            | "pnpm-lock.yaml"
            | "bun.lockb"
            | "npm-shrinkwrap.json"
    ) {
        return FileType::Lock;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return FileType::Other;
    };
    match ext {
        "lock" => FileType::Lock,
        "cs" => FileType::CSharp,
        "rs" => FileType::Rust,
        "json" | "jsonc" => FileType::Json,
        "md" | "markdown" => FileType::Markdown,
        "csproj" | "vbproj" | "fsproj" | "props" | "targets" | "xml" | "config" | "resx"
        | "nuspec" | "xaml" | "ruleset" => FileType::Xml,
        "sln" | "slnx" | "slnf" => FileType::Solution,
        "toml" => FileType::Toml,
        "ts" | "tsx" | "mts" | "cts" => FileType::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => FileType::JavaScript,
        "html" | "htm" => FileType::Html,
        "css" | "scss" | "sass" | "less" => FileType::Css,
        "razor" => FileType::Razor,
        "cshtml" | "vbhtml" => FileType::Cshtml,
        "aspx" | "ascx" | "master" | "ashx" | "asmx" | "asax" => FileType::Aspx,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "bmp" | "webp" => FileType::Image,
        "txt" | "log" => FileType::Text,
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "psd1" | "cmd" | "bat" => FileType::Shell,
        "yml" | "yaml" => FileType::Yaml,
        _ => FileType::Other,
    }
}

impl Row {
    /// The type of the file a file row stands for (by its path's name, else its label); `None` for other rows.
    pub fn file_type(&self) -> Option<FileType> {
        if !matches!(self.kind, NodeKind::File { .. }) {
            return None;
        }
        let name = self
            .path
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.label.clone());
        Some(file_type_of(&name))
    }
}

/// `path` with `.` and `..` resolved by name (no file system access).
pub fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` as the file system resolves it (symlinks followed), or [`lexical`] when it does not exist.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical(path))
}

/// One part of an opened folder, as it loads.
#[derive(Debug)]
pub enum Part<'a, T> {
    Loading,
    Loaded(&'a T),
    Failed(&'a str),
}

/// What [`SolutionModel::compose`] builds an opened folder's tree from.
#[derive(Debug)]
pub struct WorkspaceParts<'a> {
    pub root: &'a Path,
    /// The .NET solution at the root, and the host's tree for it.
    pub solution: Option<(&'a Path, Part<'a, SolutionTree>)>,
    /// The root's `Cargo.toml`, and the workspace read from `cargo metadata`.
    pub cargo: Option<(&'a Path, Part<'a, CargoWorkspace>)>,
    /// The folder's files, once listed.
    pub listing: Option<&'a FolderListing>,
}

/// One node of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Stable id: the solution or project file path, `project|relative/folder/` for folders, `project|relative/file`
    /// for files.
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    /// The file or project file on disk (`None` for folders).
    pub path: Option<PathBuf>,
    pub children: Vec<Node>,
}

/// A row of the flattened, visible tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub kind: NodeKind,
    pub path: Option<PathBuf>,
    pub has_children: bool,
    pub expanded: bool,
}

/// Workspace's tree for one solution generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolutionModel {
    pub generation: Generation,
    pub path: PathBuf,
    pub root: Node,
    /// File path to the names of the projects that list it.
    projects_by_file: HashMap<PathBuf, Vec<String>>,
}

impl SolutionModel {
    /// The tree for `tree`; `None` when no solution is open.
    pub fn from_tree(tree: &SolutionTree) -> Option<Self> {
        let path = PathBuf::from(tree.path.as_deref()?);
        // A project the host lists twice (two spellings of one path) is one node.
        let mut seen = HashSet::new();
        let projects: Vec<&TreeProject> = tree
            .projects
            .iter()
            .filter(|p| seen.insert(canonical(Path::new(&p.path))))
            .collect();
        let loaded = projects.iter().filter(|p| p.error.is_none()).count();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut projects_by_file: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for p in &tree.projects {
            for f in &p.files {
                projects_by_file
                    .entry(PathBuf::from(&f.path))
                    .or_default()
                    .push(p.name.clone());
            }
        }
        let mut children: Vec<Node> = projects.iter().map(|p| project_node(p)).collect();
        sort_nodes(&mut children);
        let root = Node {
            id: path.to_string_lossy().into_owned(),
            label: format!(
                "Solution '{name}' ({loaded} of {} project{})",
                projects.len(),
                if projects.len() == 1 { "" } else { "s" }
            ),
            kind: NodeKind::Solution,
            path: Some(path.clone()),
            children,
        };
        Some(Self {
            generation: tree.generation,
            path,
            root,
            projects_by_file,
        })
    }

    /// An opened folder's tree: the folder at the root; the solution and the Cargo workspace as siblings under it,
    /// each as far as it has loaded; then the folder's files that belong to neither.
    pub fn compose(parts: &WorkspaceParts<'_>) -> Self {
        let root_path = parts.root.to_path_buf();
        let mut projects_by_file: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut children = Vec::new();
        // Folders whose files their own node lists (projects, packages), and files that are nodes already.
        let mut owned_dirs: Vec<PathBuf> = Vec::new();
        let mut owned_files: HashSet<PathBuf> = HashSet::new();
        let mut generation = 0;
        // Each listed file with its canonical spelling: the root resolved once, the rest by name (brief 0061).
        let canonical_root = canonical(parts.root);
        let canonical_files: Vec<(PathBuf, &PathBuf)> = parts
            .listing
            .map(|l| {
                l.files
                    .iter()
                    .map(|f| {
                        let c = match f
                            .strip_prefix(&l.root)
                            .or_else(|_| f.strip_prefix(parts.root))
                        {
                            Ok(rel) => lexical(&canonical_root.join(rel)),
                            Err(_) => lexical(f),
                        };
                        (c, f)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let listed = |dir: &Path| -> Vec<PathBuf> {
            let dir = canonical(dir);
            canonical_files
                .iter()
                .filter(|(c, _)| c.starts_with(&dir))
                .map(|(_, f)| (*f).clone())
                .collect()
        };

        if let Some((path, part)) = &parts.solution {
            owned_files.insert(path.to_path_buf());
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let placeholder = |suffix: &str| Node {
                id: path.to_string_lossy().into_owned(),
                label: format!("Solution '{name}' ({suffix})"),
                kind: NodeKind::Solution,
                path: Some(path.to_path_buf()),
                children: Vec::new(),
            };
            match part {
                Part::Loaded(tree) => match Self::from_tree(tree) {
                    Some(model) => {
                        generation = model.generation;
                        for (file, names) in model.projects_by_file {
                            projects_by_file.entry(file).or_default().extend(names);
                        }
                        for p in &tree.projects {
                            owned_files.insert(PathBuf::from(&p.path));
                            if let Some(dir) = Path::new(&p.path).parent() {
                                owned_dirs.push(dir.to_path_buf());
                            }
                        }
                        children.push(model.root);
                    }
                    None => children.push(placeholder("closed")),
                },
                Part::Loading => children.push(placeholder("loading\u{2026}")),
                Part::Failed(_) => children.push(placeholder("load failed")),
            }
        }

        if let Some((manifest, part)) = &parts.cargo {
            owned_files.insert(manifest.to_path_buf());
            let lock = manifest.with_file_name("Cargo.lock");
            owned_files.insert(lock.clone());
            let id = format!("cargo|{}", manifest.to_string_lossy());
            let name = manifest
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Cargo".into());
            let mut node = Node {
                id: id.clone(),
                label: String::new(),
                kind: NodeKind::CargoWorkspace,
                path: Some(manifest.to_path_buf()),
                children: Vec::new(),
            };
            match part {
                Part::Loaded(ws) => {
                    node.label = format!(
                        "Cargo workspace '{}' ({} member{})",
                        ws.name(),
                        ws.members.len(),
                        if ws.members.len() == 1 { "" } else { "s" }
                    );
                    let mut members: Vec<&CargoPackage> = ws.members.iter().collect();
                    members.sort_by_key(|p| p.name.to_lowercase());
                    for p in members {
                        let dir = p.dir().to_path_buf();
                        // A package folder nested in another (the root package of a workspace) keeps its own files.
                        let nested: Vec<&Path> = ws
                            .members
                            .iter()
                            .map(CargoPackage::dir)
                            .filter(|d| *d != dir && d.starts_with(&dir))
                            .collect();
                        let mut files: Vec<PathBuf> = listed(&dir)
                            .into_iter()
                            .filter(|f| !nested.iter().any(|n| f.starts_with(n)))
                            .collect();
                        if parts.listing.is_none() {
                            files.push(p.manifest_path.clone());
                        }
                        for f in &files {
                            projects_by_file
                                .entry(f.clone())
                                .or_default()
                                .push(p.name.clone());
                        }
                        // The package's folder as the listing spells it, for its folders' names.
                        let shown = match parts.listing {
                            Some(_) => canonical(&dir)
                                .strip_prefix(&canonical_root)
                                .map(|rel| parts.root.join(rel))
                                .unwrap_or_else(|_| dir.clone()),
                            None => dir.clone(),
                        };
                        node.children.push(package_node(p, &shown, &files));
                        owned_dirs.push(dir);
                    }
                    node.children.push(file_leaf(&id, "Cargo.toml", manifest));
                    if parts.listing.is_none_or(|l| l.files.contains(&lock)) {
                        node.children.push(file_leaf(&id, "Cargo.lock", &lock));
                    }
                }
                Part::Loading => node.label = format!("Cargo workspace '{name}' (loading\u{2026})"),
                Part::Failed(_) => node.label = format!("Cargo workspace '{name}' (load failed)"),
            }
            children.push(node);
        }

        // The folder's other files: not a node already and not in a project's or package's folder, compared by
        // their canonical paths, so nothing is listed twice.
        if parts.listing.is_some() {
            let owned_files: HashSet<PathBuf> = owned_files.iter().map(|f| canonical(f)).collect();
            let owned_dirs: Vec<PathBuf> = owned_dirs.iter().map(|d| canonical(d)).collect();
            let rest: Vec<PathBuf> = canonical_files
                .iter()
                .filter(|(c, _)| {
                    !owned_files.contains(c) && !owned_dirs.iter().any(|d| c.starts_with(d))
                })
                .map(|(_, f)| (*f).clone())
                .collect();
            let prefix = root_path.to_string_lossy().into_owned();
            children.extend(files_tree(&prefix, parts.root, &rest));
        }

        let root = Node {
            id: format!("folder|{}", root_path.to_string_lossy()),
            label: root_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root_path.to_string_lossy().into_owned()),
            kind: NodeKind::FolderRoot,
            path: Some(root_path.clone()),
            children,
        };
        Self {
            generation,
            path: root_path,
            root,
            projects_by_file,
        }
    }

    /// The first project (in solution order) that lists `file`.
    pub fn project_of(&self, file: &Path) -> Option<&str> {
        self.projects_by_file
            .get(file)
            .and_then(|v| v.first())
            .map(String::as_str)
    }

    /// Ids of the solution and project nodes: what Visual Studio expands when a solution first opens. For an opened
    /// folder, the folder and its solution and Cargo workspace nodes.
    pub fn default_expanded(&self) -> HashSet<String> {
        let mut out = HashSet::from([self.root.id.clone()]);
        if self.root.kind == NodeKind::FolderRoot {
            out.extend(
                self.root
                    .children
                    .iter()
                    .filter(|c| matches!(c.kind, NodeKind::Solution | NodeKind::CargoWorkspace))
                    .map(|c| c.id.clone()),
            );
        }
        out
    }

    /// The node with `id`.
    pub fn find(&self, id: &str) -> Option<&Node> {
        fn walk<'a>(n: &'a Node, id: &str) -> Option<&'a Node> {
            if n.id == id {
                return Some(n);
            }
            n.children.iter().find_map(|c| walk(c, id))
        }
        walk(&self.root, id)
    }

    /// Ids of the nodes from the root down to the file node for `path` (to reveal it), or `None`.
    pub fn ancestors_of_file(&self, path: &Path) -> Option<Vec<String>> {
        fn walk(n: &Node, path: &Path, trail: &mut Vec<String>) -> bool {
            if n.path.as_deref() == Some(path) && matches!(n.kind, NodeKind::File { .. }) {
                return true;
            }
            trail.push(n.id.clone());
            if n.children.iter().any(|c| walk(c, path, trail)) {
                return true;
            }
            trail.pop();
            false
        }
        let mut trail = Vec::new();
        walk(&self.root, path, &mut trail).then_some(trail)
    }

    /// The rows a view shows when the nodes in `expanded` are expanded.
    pub fn visible_rows(&self, expanded: &HashSet<String>) -> Vec<Row> {
        fn push(n: &Node, depth: usize, expanded: &HashSet<String>, out: &mut Vec<Row>) {
            let open = expanded.contains(&n.id);
            out.push(Row {
                id: n.id.clone(),
                label: n.label.clone(),
                depth,
                kind: n.kind.clone(),
                path: n.path.clone(),
                has_children: !n.children.is_empty(),
                expanded: open && !n.children.is_empty(),
            });
            if open {
                for c in &n.children {
                    push(c, depth + 1, expanded, out);
                }
            }
        }
        let mut out = Vec::new();
        push(&self.root, 0, expanded, &mut out);
        out
    }
}

fn project_node(p: &TreeProject) -> Node {
    let id = p.path.clone();
    let label = match (&p.error, p.target_frameworks.is_empty()) {
        (Some(_), _) => format!("{} (load failed)", p.name),
        (None, true) => p.name.clone(),
        (None, false) => format!("{} ({})", p.name, p.target_frameworks.join(", ")),
    };
    let dir = Path::new(&p.path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    // Display path of every file, relative to the project, with `/` separators.
    struct Entry {
        display: String,
        path: PathBuf,
        item_type: TreeItemType,
        dependent_upon: Option<PathBuf>,
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen = HashSet::new();
    for f in &p.files {
        let path = PathBuf::from(&f.path);
        if !seen.insert(path.clone()) {
            continue;
        }
        let display = match (path.strip_prefix(&dir), &f.link) {
            (Ok(rel), _) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            (Err(_), Some(link)) => link.trim_start_matches('/').to_owned(),
            (Err(_), None) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        entries.push(Entry {
            display,
            path,
            item_type: f.item_type,
            dependent_upon: f.dependent_upon.as_deref().map(PathBuf::from),
        });
    }

    // Nesting: index of the parent entry for each entry.
    let by_path: HashMap<&Path, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.path.as_path(), i))
        .collect();
    let by_display: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.display.to_lowercase(), i))
        .collect();
    let mut parent: Vec<Option<usize>> = vec![None; entries.len()];
    for (i, e) in entries.iter().enumerate() {
        let explicit = e
            .dependent_upon
            .as_deref()
            .and_then(|d| by_path.get(d).copied());
        parent[i] = explicit
            .or_else(|| nest_by_name(&e.display, &by_display))
            .filter(|&j| j != i);
    }
    // Break cycles: a file whose parent chain comes back to it stays at its folder.
    for i in 0..entries.len() {
        let mut j = parent[i];
        let mut steps = 0;
        while let Some(k) = j {
            if k == i || steps > entries.len() {
                parent[i] = None;
                break;
            }
            j = parent[k];
            steps += 1;
        }
    }

    // Folders.
    #[derive(Default)]
    struct Folder {
        folders: BTreeMap<String, Folder>,
        files: Vec<usize>,
    }
    let mut root = Folder::default();
    for (i, e) in entries.iter().enumerate() {
        if parent[i].is_some() {
            continue;
        }
        let mut f = &mut root;
        let parts: Vec<&str> = e.display.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            f = f.folders.entry((*part).to_owned()).or_default();
        }
        f.files.push(i);
    }
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children_of.entry(*p).or_default().push(i);
        }
    }
    fn file_node(
        i: usize,
        entries: &[Entry],
        children_of: &HashMap<usize, Vec<usize>>,
        project: &str,
    ) -> Node {
        let e = &entries[i];
        let mut children: Vec<Node> = children_of
            .get(&i)
            .map(|c| {
                c.iter()
                    .map(|&j| file_node(j, entries, children_of, project))
                    .collect()
            })
            .unwrap_or_default();
        sort_nodes(&mut children);
        Node {
            id: format!("{project}|{}", e.display),
            label: e
                .display
                .rsplit('/')
                .next()
                .unwrap_or(&e.display)
                .to_owned(),
            kind: NodeKind::File {
                item_type: e.item_type,
            },
            path: Some(e.path.clone()),
            children,
        }
    }
    fn folder_nodes(
        f: &Folder,
        prefix: &str,
        entries: &[Entry],
        children_of: &HashMap<usize, Vec<usize>>,
        project: &str,
    ) -> Vec<Node> {
        let mut out: Vec<Node> = f
            .folders
            .iter()
            .map(|(name, sub)| {
                let rel = format!("{prefix}{name}/");
                Node {
                    id: format!("{project}|{rel}"),
                    label: name.clone(),
                    kind: NodeKind::Folder,
                    path: None,
                    children: folder_nodes(sub, &rel, entries, children_of, project),
                }
            })
            .collect();
        out.extend(
            f.files
                .iter()
                .map(|&i| file_node(i, entries, children_of, project)),
        );
        sort_nodes(&mut out);
        out
    }
    let children = if p.error.is_some() {
        Vec::new()
    } else {
        let mut children = folder_nodes(&root, "", &entries, &children_of, &id);
        if let Some(deps) = &p.dependencies {
            children.insert(0, dependencies_node(&id, &p.path, deps));
        }
        children
    };
    Node {
        id: id.clone(),
        label,
        kind: NodeKind::Project {
            kind: p.kind,
            web: p.web,
            error: p.error.clone(),
        },
        path: Some(PathBuf::from(&p.path)),
        children,
    }
}

/// Visual Studio's Dependencies node (brief 0048): Frameworks, Packages (each package with the packages it brings in
/// under it) and Projects, each folder only when it has something. Node ids are `<project>|deps`, then
/// `<project>|deps|frameworks`, `...|packages|<id>`, `...|packages|<id>|<child id>`, `...|projects|<path>`.
fn dependencies_node(
    id: &str,
    project: &str,
    deps: &eludite_protocol::host::TreeDependencies,
) -> Node {
    let path = Some(PathBuf::from(project));
    let base = format!("{id}|deps");
    let label_of = |name: &str, version: Option<&str>| match version {
        Some(v) if !v.is_empty() => format!("{name} ({v})"),
        _ => name.to_owned(),
    };
    let mut groups = Vec::new();
    if !deps.frameworks.is_empty() {
        groups.push(Node {
            id: format!("{base}|frameworks"),
            label: "Frameworks".into(),
            kind: NodeKind::DependencyGroup {
                group: DependencyGroup::Frameworks,
            },
            path: path.clone(),
            children: deps
                .frameworks
                .iter()
                .map(|f| Node {
                    id: format!("{base}|frameworks|{}", f.name),
                    label: f.name.clone(),
                    kind: NodeKind::Framework,
                    path: path.clone(),
                    children: Vec::new(),
                })
                .collect(),
        });
    }
    if !deps.packages.is_empty() {
        let mut packages: Vec<&eludite_protocol::host::TreePackage> =
            deps.packages.iter().collect();
        packages.sort_by_key(|p| p.id.to_lowercase());
        groups.push(Node {
            id: format!("{base}|packages"),
            label: "Packages".into(),
            kind: NodeKind::DependencyGroup {
                group: DependencyGroup::Packages,
            },
            path: path.clone(),
            children: packages
                .into_iter()
                .map(|p| {
                    let pid = format!("{base}|packages|{}", p.id);
                    let version = p.version.clone().or_else(|| p.requested.clone());
                    Node {
                        id: pid.clone(),
                        label: label_of(&p.id, version.as_deref()),
                        kind: NodeKind::Package {
                            id: p.id.clone(),
                            version,
                            transitive: false,
                            warning: p.deprecated
                                || p.vulnerabilities.as_ref().is_some_and(|v| !v.is_empty()),
                        },
                        path: path.clone(),
                        children: p
                            .transitive
                            .iter()
                            .flatten()
                            .map(|c| Node {
                                id: format!("{pid}|{}", c.id),
                                label: label_of(&c.id, c.version.as_deref()),
                                kind: NodeKind::Package {
                                    id: c.id.clone(),
                                    version: c.version.clone(),
                                    transitive: true,
                                    warning: false,
                                },
                                path: path.clone(),
                                children: Vec::new(),
                            })
                            .collect(),
                    }
                })
                .collect(),
        });
    }
    if !deps.projects.is_empty() {
        groups.push(Node {
            id: format!("{base}|projects"),
            label: "Projects".into(),
            kind: NodeKind::DependencyGroup {
                group: DependencyGroup::Projects,
            },
            path: path.clone(),
            children: deps
                .projects
                .iter()
                .map(|r| Node {
                    id: format!("{base}|projects|{}", r.path),
                    label: r.name.clone(),
                    kind: NodeKind::ProjectReference,
                    path: Some(PathBuf::from(&r.path)),
                    children: Vec::new(),
                })
                .collect(),
        });
    }
    Node {
        id: base,
        label: "Dependencies".into(),
        kind: NodeKind::Dependencies,
        path,
        children: groups,
    }
}

/// A member package: `name (lib, bin)`, its `Targets` and its files.
fn package_node(p: &CargoPackage, dir: &Path, files: &[PathBuf]) -> Node {
    let id = p.manifest_path.to_string_lossy().into_owned();
    let kinds = p.product_kinds();
    let label = if kinds.is_empty() {
        p.name.clone()
    } else {
        format!(
            "{} ({})",
            p.name,
            kinds
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut targets: Vec<Node> = p
        .targets
        .iter()
        .map(|t| Node {
            id: format!("{id}|#target:{}:{}", t.kind.as_str(), t.name),
            label: format!("{} ({})", t.name, t.kind.as_str()),
            kind: NodeKind::CargoTarget { kind: t.kind },
            path: Some(t.src_path.clone()),
            children: Vec::new(),
        })
        .collect();
    targets.sort_by(|a, b| {
        let k = |n: &Node| match &n.kind {
            NodeKind::CargoTarget { kind } => *kind,
            _ => TargetKind::Lib,
        };
        k(a).cmp(&k(b)).then_with(|| a.label.cmp(&b.label))
    });
    let mut children = vec![Node {
        id: format!("{id}|#targets"),
        label: "Targets".into(),
        kind: NodeKind::CargoTargets,
        path: None,
        children: targets,
    }];
    children.extend(files_tree(&id, dir, files));
    Node {
        id,
        label,
        kind: NodeKind::CargoPackage { kinds },
        path: Some(p.manifest_path.clone()),
        children,
    }
}

/// A file node with a fixed label.
fn file_leaf(prefix: &str, label: &str, path: &Path) -> Node {
    Node {
        id: format!("{prefix}|{label}"),
        label: label.to_owned(),
        kind: NodeKind::File {
            item_type: TreeItemType::Content,
        },
        path: Some(path.to_path_buf()),
        children: Vec::new(),
    }
}

/// Folders mirroring the file system under `dir` for plain `files` (no nesting), sorted as Visual Studio does.
/// Ids are `prefix|relative/path` (folders end with `/`).
pub fn files_tree(prefix: &str, dir: &Path, files: &[PathBuf]) -> Vec<Node> {
    #[derive(Default)]
    struct Folder {
        folders: BTreeMap<String, Folder>,
        files: Vec<(String, PathBuf)>,
    }
    let mut root = Folder::default();
    for f in files {
        let Ok(rel) = f.strip_prefix(dir) else {
            continue;
        };
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some((name, folders)) = parts.split_last() else {
            continue;
        };
        let mut at = &mut root;
        for part in folders {
            at = at.folders.entry(part.clone()).or_default();
        }
        at.files.push((name.clone(), f.clone()));
    }
    fn nodes(f: &Folder, rel: &str, prefix: &str) -> Vec<Node> {
        let mut out: Vec<Node> = f
            .folders
            .iter()
            .map(|(name, sub)| {
                let rel = format!("{rel}{name}/");
                Node {
                    id: format!("{prefix}|{rel}"),
                    label: name.clone(),
                    kind: NodeKind::Folder,
                    path: None,
                    children: nodes(sub, &rel, prefix),
                }
            })
            .collect();
        out.extend(f.files.iter().map(|(name, path)| Node {
            id: format!("{prefix}|{rel}{name}"),
            label: name.clone(),
            kind: NodeKind::File {
                item_type: TreeItemType::Content,
            },
            path: Some(path.clone()),
            children: Vec::new(),
        }));
        sort_nodes(&mut out);
        out
    }
    nodes(&root, "", prefix)
}

/// The entry `display` nests under by name, if any.
fn nest_by_name(display: &str, by_display: &HashMap<String, usize>) -> Option<usize> {
    let lower = display.to_lowercase();
    let (dir, name) = match lower.rsplit_once('/') {
        Some((d, n)) => (format!("{d}/"), n.to_owned()),
        None => (String::new(), lower.clone()),
    };
    let sibling = |n: &str| by_display.get(&format!("{dir}{n}")).copied();
    if let Some(base) = name.strip_suffix(".designer.cs") {
        // Form1.Designer.cs under Form1.cs or Form1.resx; Default.aspx.designer.cs under Default.aspx.
        return sibling(base)
            .or_else(|| sibling(&format!("{base}.cs")))
            .or_else(|| sibling(&format!("{base}.resx")));
    }
    // Default.aspx.cs under Default.aspx: any name with two extensions under the sibling without the last one.
    let (stem, _) = name.rsplit_once('.')?;
    stem.contains('.').then(|| sibling(stem)).flatten()
}

/// Folders before files, folders and files each by label, case-insensitively (Visual Studio order). The
/// solution's projects are sorted by name.
fn sort_nodes(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        let rank = |n: &Node| match n.kind {
            NodeKind::CargoTargets => 0,
            NodeKind::Folder => 1,
            _ => 2,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_protocol::host::TreeFile;

    #[test]
    fn a_project_with_dependencies_shows_visual_studios_dependencies_node_first() {
        use eludite_protocol::host::{
            NuGetVulnerability, TreeDependencies, TreeFramework, TreePackage, TreeProjectReference,
            TreeTransitive,
        };
        let tree = SolutionTree {
            generation: 3,
            path: Some("/c/Corpus.slnx".into()),
            projects: vec![TreeProject {
                name: "App".into(),
                path: "/c/App/App.csproj".into(),
                kind: TreeProjectKind::Sdk,
                web: false,
                target_frameworks: vec!["net10.0".into()],
                files: vec![file("/c/App/Program.cs", TreeItemType::Compile)],
                error: None,
                dependencies: Some(TreeDependencies {
                    restored: true,
                    packages: vec![
                        TreePackage {
                            id: "Zeta".into(),
                            requested: Some("2.0.0".into()),
                            ..TreePackage::default()
                        },
                        TreePackage {
                            id: "Eludite.Corpus.Greeter".into(),
                            version: Some("1.0.0".into()),
                            transitive: Some(vec![TreeTransitive {
                                id: "Eludite.Corpus.Logging".into(),
                                version: Some("[1.0.0, )".into()),
                            }]),
                            vulnerabilities: Some(vec![NuGetVulnerability {
                                severity: "high".into(),
                                advisory_url: "https://github.com/advisories/GHSA-x".into(),
                            }]),
                            ..TreePackage::default()
                        },
                    ],
                    projects: vec![TreeProjectReference {
                        name: "Shared".into(),
                        path: "/c/Shared/Shared.csproj".into(),
                    }],
                    frameworks: vec![TreeFramework {
                        name: "Microsoft.NETCore.App".into(),
                        target_framework: Some("net10.0".into()),
                    }],
                }),
            }],
        };
        let model = SolutionModel::from_tree(&tree).unwrap();
        let mut expanded = model.default_expanded();
        for id in [
            "/c/App/App.csproj",
            "/c/App/App.csproj|deps",
            "/c/App/App.csproj|deps|packages",
            "/c/App/App.csproj|deps|packages|Eludite.Corpus.Greeter",
            "/c/App/App.csproj|deps|frameworks",
            "/c/App/App.csproj|deps|projects",
        ] {
            expanded.insert(id.into());
        }
        let rows: Vec<String> = model
            .visible_rows(&expanded)
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
            .collect();
        assert_eq!(
            rows,
            [
                "Solution 'Corpus' (1 of 1 project)",
                "  App (net10.0)",
                "    Dependencies",
                "      Frameworks",
                "        Microsoft.NETCore.App",
                "      Packages",
                "        Eludite.Corpus.Greeter (1.0.0)",
                "          Eludite.Corpus.Logging ([1.0.0, ))",
                "        Zeta (2.0.0)",
                "      Projects",
                "        Shared",
                "    Program.cs",
            ]
        );
        let greeter = model
            .find("/c/App/App.csproj|deps|packages|Eludite.Corpus.Greeter")
            .unwrap();
        assert!(
            matches!(&greeter.kind, NodeKind::Package { warning: true, transitive: false, version: Some(v), .. } if v == "1.0.0")
        );
        assert!(greeter.kind.is_dependency() && !greeter.kind.opens_file());
        assert_eq!(
            greeter.path.as_deref(),
            Some(Path::new("/c/App/App.csproj"))
        );
        let shared = model
            .find("/c/App/App.csproj|deps|projects|/c/Shared/Shared.csproj")
            .unwrap();
        assert_eq!(
            shared.path.as_deref(),
            Some(Path::new("/c/Shared/Shared.csproj"))
        );
        // The file is still found under its project.
        assert!(
            model
                .ancestors_of_file(Path::new("/c/App/Program.cs"))
                .is_some()
        );
    }

    fn file(path: &str, item_type: TreeItemType) -> TreeFile {
        TreeFile {
            path: path.into(),
            item_type,
            dependent_upon: None,
            link: None,
        }
    }

    fn tree() -> SolutionTree {
        use TreeItemType::*;
        SolutionTree {
            generation: 4,
            path: Some("/src/Shop.sln".into()),
            projects: vec![
                TreeProject {
                    name: "Shop.Web".into(),
                    path: "/src/Web/Shop.Web.csproj".into(),
                    kind: TreeProjectKind::Legacy,
                    web: true,
                    target_frameworks: vec!["net48".into()],
                    files: vec![
                        file("/src/Web/Default.aspx", Content),
                        TreeFile {
                            dependent_upon: Some("/src/Web/Default.aspx".into()),
                            ..file("/src/Web/Default.aspx.cs", Compile)
                        },
                        TreeFile {
                            dependent_upon: Some("/src/Web/Default.aspx".into()),
                            ..file("/src/Web/Default.aspx.designer.cs", Compile)
                        },
                        file("/src/Web/Account/Login.aspx", Content),
                        file("/src/Web/Account/Login.aspx.cs", Compile),
                        file("/src/Web/Account/Login.aspx.designer.cs", Compile),
                        file("/src/Web/Global.asax.cs", Compile),
                        TreeFile {
                            link: Some("Properties/SharedInfo.cs".into()),
                            ..file("/src/Shared/SharedInfo.cs", Compile)
                        },
                    ],
                    error: None,
                    dependencies: None,
                },
                TreeProject {
                    name: "Core".into(),
                    path: "/src/Core/Core.csproj".into(),
                    kind: TreeProjectKind::Sdk,
                    web: false,
                    target_frameworks: vec!["net8.0".into(), "net10.0".into()],
                    files: vec![
                        file("/src/Core/Widget.cs", Compile),
                        file("/src/Core/Forms/Main.cs", Compile),
                        file("/src/Core/Forms/Main.Designer.cs", Compile),
                        file("/src/Core/Forms/Main.resx", Content),
                        file("/src/Core/a.cs", Compile),
                    ],
                    error: None,
                    dependencies: None,
                },
                TreeProject {
                    name: "Broken".into(),
                    path: "/src/Broken/Broken.csproj".into(),
                    kind: TreeProjectKind::Sdk,
                    web: false,
                    target_frameworks: vec![],
                    files: vec![],
                    error: Some("bad xml".into()),
                    dependencies: None,
                },
            ],
        }
    }

    fn labels(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
            .collect()
    }

    fn expand_all(m: &SolutionModel) -> HashSet<String> {
        fn walk(n: &Node, out: &mut HashSet<String>) {
            out.insert(n.id.clone());
            n.children.iter().for_each(|c| walk(c, out));
        }
        let mut out = HashSet::new();
        walk(&m.root, &mut out);
        out
    }

    #[test]
    fn vs_tree_with_folders_nesting_and_frameworks() {
        let m = SolutionModel::from_tree(&tree()).unwrap();
        assert_eq!(m.generation, 4);
        let rows = m.visible_rows(&expand_all(&m));
        assert_eq!(
            labels(&rows),
            [
                "Solution 'Shop' (2 of 3 projects)",
                "  Broken (load failed)",
                "  Core (net8.0, net10.0)",
                "    Forms",
                "      Main.cs",
                "        Main.Designer.cs",
                "      Main.resx",
                "    a.cs",
                "    Widget.cs",
                "  Shop.Web (net48)",
                "    Account",
                "      Login.aspx",
                "        Login.aspx.cs",
                "        Login.aspx.designer.cs",
                "    Properties",
                "      SharedInfo.cs",
                "    Default.aspx",
                "      Default.aspx.cs",
                "      Default.aspx.designer.cs",
                "    Global.asax.cs",
            ]
        );
        let login = rows.iter().find(|r| r.label == "Login.aspx").unwrap();
        assert!(login.has_children && login.expanded);
        assert_eq!(
            login.path.as_deref(),
            Some(Path::new("/src/Web/Account/Login.aspx"))
        );
        assert_eq!(
            rows.iter().find(|r| r.label == "Account").unwrap().kind,
            NodeKind::Folder
        );
    }

    #[test]
    fn collapsed_by_default_and_expand_one_level() {
        let m = SolutionModel::from_tree(&tree()).unwrap();
        let mut expanded = m.default_expanded();
        let rows = m.visible_rows(&expanded);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].expanded);
        assert!(!rows[2].expanded && rows[2].has_children);
        assert!(!rows[1].has_children, "a failed project has no children");
        expanded.insert("/src/Core/Core.csproj".into());
        let rows = m.visible_rows(&expanded);
        assert_eq!(
            labels(&rows)[2..6],
            [
                "  Core (net8.0, net10.0)",
                "    Forms",
                "    a.cs",
                "    Widget.cs"
            ]
        );
    }

    #[test]
    fn project_lookup_and_reveal() {
        let m = SolutionModel::from_tree(&tree()).unwrap();
        assert_eq!(
            m.project_of(Path::new("/src/Web/Account/Login.aspx.cs")),
            Some("Shop.Web")
        );
        assert_eq!(m.project_of(Path::new("/elsewhere.cs")), None);
        let trail = m
            .ancestors_of_file(Path::new("/src/Web/Account/Login.aspx.designer.cs"))
            .unwrap();
        assert_eq!(
            trail,
            [
                "/src/Shop.sln",
                "/src/Web/Shop.Web.csproj",
                "/src/Web/Shop.Web.csproj|Account/",
                "/src/Web/Shop.Web.csproj|Account/Login.aspx",
            ]
        );
        assert!(m.find("/src/Web/Shop.Web.csproj|Account/").is_some());
        assert!(m.ancestors_of_file(Path::new("/nope.cs")).is_none());
    }

    #[test]
    fn no_solution_is_no_model() {
        assert!(
            SolutionModel::from_tree(&SolutionTree {
                generation: 0,
                path: None,
                projects: vec![]
            })
            .is_none()
        );
    }

    /// This repository as File > Open Folder shows it: the recorded `cargo metadata`, a listing of a few of its
    /// files, and a one-project solution beside the Cargo workspace.
    fn mixed_repo() -> (SolutionTree, CargoWorkspace, FolderListing) {
        let cargo =
            CargoWorkspace::from_metadata(include_str!("testdata/cargo-metadata.json")).unwrap();
        let solution = SolutionTree {
            generation: 1,
            path: Some("/w/eludite/dotnet/Eludite.slnx".into()),
            projects: vec![TreeProject {
                name: "Eludite.Host".into(),
                path: "/w/eludite/dotnet/src/Eludite.Host/Eludite.Host.csproj".into(),
                kind: TreeProjectKind::Sdk,
                web: false,
                target_frameworks: vec!["net10.0".into()],
                files: vec![file(
                    "/w/eludite/dotnet/src/Eludite.Host/Program.cs",
                    TreeItemType::Compile,
                )],
                error: None,
                dependencies: None,
            }],
        };
        let files = [
            "Cargo.lock",
            "Cargo.toml",
            "README.md",
            "crates/editor/Cargo.toml",
            "crates/editor/src/buffer.rs",
            "crates/editor/src/syntax/mod.rs",
            "crates/eludite/Cargo.toml",
            "crates/eludite/src/main.rs",
            "crates/eludite/tests/relay.rs",
            "docs/PLAN.md",
            "dotnet/Eludite.slnx",
            "dotnet/src/Eludite.Host/Eludite.Host.csproj",
            "dotnet/src/Eludite.Host/Program.cs",
        ];
        let listing = FolderListing {
            root: "/w/eludite".into(),
            files: files
                .iter()
                .map(|f| Path::new("/w/eludite").join(f))
                .collect(),
            truncated: false,
        };
        (solution, cargo, listing)
    }

    #[test]
    fn an_opened_folder_shows_the_solution_and_the_cargo_workspace_as_siblings() {
        let (solution, cargo, listing) = mixed_repo();
        let sln = Path::new("/w/eludite/dotnet/Eludite.slnx");
        let manifest = Path::new("/w/eludite/Cargo.toml");
        let m = SolutionModel::compose(&WorkspaceParts {
            root: Path::new("/w/eludite"),
            solution: Some((sln, Part::Loaded(&solution))),
            cargo: Some((manifest, Part::Loaded(&cargo))),
            listing: Some(&listing),
        });
        assert_eq!(m.root.kind, NodeKind::FolderRoot);
        assert_eq!(m.root.label, "eludite");
        assert_eq!(m.generation, 1);
        let top: Vec<&str> = m.root.children.iter().map(|c| c.label.as_str()).collect();
        // The solution, the Cargo workspace, then the folder's other files (folders first).
        assert_eq!(
            top,
            [
                "Solution 'Eludite' (1 of 1 project)",
                "Cargo workspace 'eludite' (16 members)",
                "docs",
                "README.md"
            ]
        );
        let cargo_node = &m.root.children[1];
        let labels: Vec<&str> = cargo_node
            .children
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(labels.first(), Some(&"eludite (bin)"));
        assert!(labels.contains(&"eludite-lsp (lib)"));
        assert_eq!(&labels[labels.len() - 2..], ["Cargo.toml", "Cargo.lock"]);
        let shell = &cargo_node.children[0];
        let shell_children: Vec<&str> = shell.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(shell_children, ["Targets", "src", "tests", "Cargo.toml"]);
        let targets: Vec<&str> = shell.children[0]
            .children
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(targets, ["eludite (bin)", "relay (test)"]);
        assert!(shell.children[0].children[1].kind.opens_file());
        assert_eq!(
            shell.children[0].children[1].path.as_deref(),
            Some(Path::new("/w/eludite/crates/eludite/tests/relay.rs"))
        );

        let buffer = Path::new("/w/eludite/crates/editor/src/buffer.rs");
        assert_eq!(m.project_of(buffer), Some("eludite-editor"));
        assert_eq!(
            m.project_of(Path::new("/w/eludite/dotnet/src/Eludite.Host/Program.cs")),
            Some("Eludite.Host")
        );
        // Reveal walks folder, Cargo workspace, package, src.
        let trail = m.ancestors_of_file(buffer).unwrap();
        assert_eq!(trail.len(), 4);
        let expanded = m.default_expanded();
        assert!(expanded.contains(&m.root.id));
        assert!(expanded.contains(&cargo_node.id));
        assert!(expanded.contains(&m.root.children[0].id));
        let rows = m.visible_rows(&expanded);
        assert_eq!(rows[0].label, "eludite");
        assert!(
            rows.iter()
                .any(|r| r.label == "eludite-editor (lib)" && r.depth == 2)
        );
    }

    #[test]
    fn parts_show_while_they_load_or_fail() {
        let sln = Path::new("/w/App.slnx");
        let manifest = Path::new("/w/Cargo.toml");
        let m = SolutionModel::compose(&WorkspaceParts {
            root: Path::new("/w"),
            solution: Some((sln, Part::Loading)),
            cargo: Some((manifest, Part::Failed("cargo: not found"))),
            listing: None,
        });
        let top: Vec<&str> = m.root.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            top,
            [
                "Solution 'App' (loading\u{2026})",
                "Cargo workspace 'w' (load failed)"
            ]
        );
        // A plain folder: its files only.
        let listing = FolderListing {
            root: "/notes".into(),
            files: vec!["/notes/a/b.md".into(), "/notes/c.txt".into()],
            truncated: false,
        };
        let m = SolutionModel::compose(&WorkspaceParts {
            root: Path::new("/notes"),
            solution: None,
            cargo: None,
            listing: Some(&listing),
        });
        let rows = m.visible_rows(&m.default_expanded());
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["notes", "a", "c.txt"]);
    }
    #[test]
    fn file_types_by_extension_case_insensitively() {
        let cases = [
            ("Program.cs", FileType::CSharp),
            ("MAIN.RS", FileType::Rust),
            ("appsettings.json", FileType::Json),
            ("README.md", FileType::Markdown),
            ("Eludite.Host.csproj", FileType::Xml),
            ("Directory.Build.props", FileType::Xml),
            ("Sdk.targets", FileType::Xml),
            ("web.config", FileType::Xml),
            ("Strings.resx", FileType::Xml),
            ("Eludite.slnx", FileType::Solution),
            ("Legacy.SLN", FileType::Solution),
            ("Cargo.toml", FileType::Toml),
            ("app.ts", FileType::TypeScript),
            ("view.tsx", FileType::TypeScript),
            ("site.js", FileType::JavaScript),
            ("index.html", FileType::Html),
            ("site.css", FileType::Css),
            ("Counter.razor", FileType::Razor),
            ("Index.cshtml", FileType::Cshtml),
            ("Default.aspx", FileType::Aspx),
            ("logo.PNG", FileType::Image),
            ("icon.svg", FileType::Image),
            ("notes.txt", FileType::Text),
            ("build.sh", FileType::Shell),
            ("fetch.ps1", FileType::Shell),
            ("ci.yml", FileType::Yaml),
            ("Cargo.lock", FileType::Lock),
            ("packages.lock.json", FileType::Lock),
            ("package-lock.json", FileType::Lock),
            ("LICENSE", FileType::Other),
            ("a.unknown", FileType::Other),
            ("/w/src/Dir.cs/Thing.json", FileType::Json),
        ];
        for (name, want) in cases {
            assert_eq!(file_type_of(name), want, "{name}");
        }
        let row = |kind: NodeKind, path: Option<&str>, label: &str| Row {
            id: label.into(),
            label: label.into(),
            depth: 0,
            kind,
            path: path.map(PathBuf::from),
            has_children: false,
            expanded: false,
        };
        let content = NodeKind::File {
            item_type: TreeItemType::Content,
        };
        assert_eq!(
            row(content.clone(), Some("/w/Default.ASPX"), "Default.ASPX").file_type(),
            Some(FileType::Aspx)
        );
        assert_eq!(
            row(content, None, "notes.md").file_type(),
            Some(FileType::Markdown)
        );
        assert_eq!(row(NodeKind::Folder, None, "src.cs").file_type(), None);
    }

    /// A workspace folder opened through a symlink (or a `..` spelling): the host and `cargo metadata` report the
    /// real paths, the listing the opened ones. Each project and package is listed once, under its solution or
    /// workspace, and a folder that held only projects does not come back at the root.
    #[test]
    fn nothing_is_listed_twice_whatever_the_spelling_of_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("real")).unwrap();
        let real = canonical(&tmp.path().join("real"));
        let write = |rel: &str| {
            let p = real.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        };
        for f in [
            "Eludite.slnx",
            "src/Eludite.Host/Eludite.Host.csproj",
            "src/Eludite.Host/Program.cs",
            "src/Eludite.Host/Rpc/HostRpcTarget.cs",
            "tests/Eludite.Host.Tests/Eludite.Host.Tests.csproj",
            "tests/Eludite.Host.Tests/HostTests.cs",
            "Directory.Build.props",
        ] {
            write(f);
        }
        // The opened spelling: `<tmp>/real/../real`, and a symlink where the platform has them.
        let mut spellings = vec![tmp.path().join("real").join("..").join("real")];
        #[cfg(unix)]
        {
            let alias = tmp.path().join("alias");
            std::os::unix::fs::symlink(&real, &alias).unwrap();
            spellings.push(alias);
        }
        let project = |name: &str, dir: &str| TreeProject {
            name: name.into(),
            path: real
                .join(dir)
                .join(format!("{name}.csproj"))
                .to_string_lossy()
                .into_owned(),
            kind: TreeProjectKind::Sdk,
            web: false,
            target_frameworks: vec!["net10.0".into()],
            files: vec![],
            error: None,
            dependencies: None,
        };
        let host = project("Eludite.Host", "src/Eludite.Host");
        // The host lists one project twice, once through a `..` spelling.
        let mut again = host.clone();
        again.path = real
            .join("src/Eludite.Host/../Eludite.Host/Eludite.Host.csproj")
            .to_string_lossy()
            .into_owned();
        let tree = SolutionTree {
            generation: 2,
            path: Some(real.join("Eludite.slnx").to_string_lossy().into_owned()),
            projects: vec![
                host,
                project("Eludite.Host.Tests", "tests/Eludite.Host.Tests"),
                again,
            ],
        };
        for root in spellings {
            let listing = crate::folder::list_folder(&root, 1000);
            let sln = root.join("Eludite.slnx");
            let m = SolutionModel::compose(&WorkspaceParts {
                root: &root,
                solution: Some((&sln, Part::Loaded(&tree))),
                cargo: None,
                listing: Some(&listing),
            });
            let rows = m.visible_rows(&expand_all(&m));
            let labels = labels(&rows);
            assert_eq!(
                labels,
                [
                    root.file_name().unwrap().to_string_lossy().into_owned(),
                    "  Solution 'Eludite' (2 of 2 projects)".into(),
                    "    Eludite.Host (net10.0)".into(),
                    "    Eludite.Host.Tests (net10.0)".into(),
                    "  Directory.Build.props".into(),
                ],
                "{root:?}"
            );
            // A csproj is never a plain file row, and no folder holds only projects.
            assert!(
                !rows.iter().any(|r| r.label.ends_with(".csproj")
                    || r.label == "src"
                    || r.label == "tests"),
                "{labels:?}"
            );
        }
    }
}
