//! Solution Explorer's model, fed from the host's `eludite/solution/tree` answer (brief 0012).
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use eludite_protocol::host::{
    Generation, SolutionTree, TreeItemType, TreeProject, TreeProjectKind,
};

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
    File {
        item_type: TreeItemType,
    },
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

/// Solution Explorer's tree for one solution generation.
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
        let loaded = tree.projects.iter().filter(|p| p.error.is_none()).count();
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
        let mut children: Vec<Node> = tree.projects.iter().map(project_node).collect();
        sort_nodes(&mut children);
        let root = Node {
            id: path.to_string_lossy().into_owned(),
            label: format!(
                "Solution '{name}' ({loaded} of {} project{})",
                tree.projects.len(),
                if tree.projects.len() == 1 { "" } else { "s" }
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

    /// The first project (in solution order) that lists `file`.
    pub fn project_of(&self, file: &Path) -> Option<&str> {
        self.projects_by_file
            .get(file)
            .and_then(|v| v.first())
            .map(String::as_str)
    }

    /// Ids of the solution and project nodes: what Visual Studio expands when a solution first opens.
    pub fn default_expanded(&self) -> HashSet<String> {
        HashSet::from([self.root.id.clone()])
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
        folder_nodes(&root, "", &entries, &children_of, &id)
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
            NodeKind::Folder => 0,
            _ => 1,
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
                },
                TreeProject {
                    name: "Broken".into(),
                    path: "/src/Broken/Broken.csproj".into(),
                    kind: TreeProjectKind::Sdk,
                    web: false,
                    target_frameworks: vec![],
                    files: vec![],
                    error: Some("bad xml".into()),
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
}
