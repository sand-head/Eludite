//! The Manage NuGet Packages window (brief 0048; Visual Studio's NuGet Package Manager, a document tab): the Browse,
//! Installed, Updates and Consolidate tabs, the search box (searched 300 ms after the last key), Include prerelease, the
//! Package source dropdown with All, the package list (virtualized, with each package's icon once the host fetched
//! it, the version and a yellow badge for a known vulnerability or a deprecation, and an error row for each source that
//! failed), and the detail pane (the versions dropdown, the description, the license and project links, the
//! vulnerability and deprecation lines, the projects with check boxes for the solution's scope, and Install, Update,
//! Uninstall or Consolidate).
//!
//! The window keeps what the commands answered (`eludite.nuget.*`'s outputs, the same an agent reads) and its own
//! state (tab, query, selection, check boxes). Every action is a [`WindowEvent`] the shell turns into a command run
//! off the UI thread; the answers come back through the shell's setters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;

use eludite_commands::nuget::{
    InstalledOutput, InstalledProject, SearchOutput, SearchRow, Tab, UpdateRow, UpdatesOutput,
    Vulnerability,
};
use eludite_ui::{Theme, check_box, menu_row, push_button, text_box, toggle_button};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, UniformListScrollHandle, Window, anchored, deferred, div, img, px, rgb, uniform_list,
};

/// Debug selectors.
pub const WINDOW: &str = "nuget-window";
pub const SEARCH_BOX: &str = "nuget-search";
pub const PRERELEASE: &str = "nuget-prerelease";
pub const SOURCE_BUTTON: &str = "nuget-source";
pub const REFRESH: &str = "nuget-refresh";
pub const SETTINGS: &str = "nuget-settings";
pub const DETAILS: &str = "nuget-details";
pub const VERSION_BUTTON: &str = "nuget-version";
pub const INSTALL: &str = "nuget-install";
pub const UPDATE: &str = "nuget-update";
pub const UNINSTALL: &str = "nuget-uninstall";
pub const CONSOLIDATE: &str = "nuget-consolidate";
pub const SELECT_ALL: &str = "nuget-select-all";
pub const UPDATE_SELECTED: &str = "nuget-update-selected";
pub const STATUS: &str = "nuget-status";

/// Row height: an icon of 32 px with two lines of text beside it.
pub const ROW_HEIGHT: f32 = 44.;

pub fn tab_selector(tab: Tab) -> String {
    format!("nuget-tab-{}", tab_name(tab))
}

/// Debug selector of visible row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("nuget-row-{ix}")
}

/// Debug selector of an Updates row's check box.
pub fn update_check_selector(id: &str) -> String {
    format!("nuget-update-check-{id}")
}

/// Debug selector of the detail pane's check box for a project.
pub fn project_check_selector(name: &str) -> String {
    format!("nuget-project-{name}")
}

pub fn source_item_selector(ix: usize) -> String {
    format!("nuget-source-item-{ix}")
}

pub fn version_item_selector(ix: usize) -> String {
    format!("nuget-version-item-{ix}")
}

fn tab_name(tab: Tab) -> &'static str {
    match tab {
        Tab::Browse => "browse",
        Tab::Installed => "installed",
        Tab::Updates => "updates",
        Tab::Consolidate => "consolidate",
    }
}

/// Whose packages the window manages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Solution,
    /// A project: its name and project file.
    Project {
        name: String,
        path: String,
    },
}

impl Scope {
    /// The tab's title, as Visual Studio names it.
    pub fn title(&self) -> String {
        match self {
            Scope::Solution => "NuGet - Solution".into(),
            Scope::Project { name, .. } => format!("NuGet: {name}"),
        }
    }
}

/// What the window asks the shell to run.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    /// The query changed: search after the debounce.
    QueryChanged,
    /// Search now (Enter, Refresh, the prerelease box, the source dropdown).
    Search,
    /// The tab's data again (Installed, Updates; Consolidate reads Installed).
    Refresh,
    Install {
        id: String,
        version: String,
        projects: Vec<String>,
    },
    Uninstall {
        id: String,
        projects: Vec<String>,
    },
    Update {
        id: String,
        version: String,
        projects: Vec<String>,
    },
    /// Updates' Update with packages checked: each to its newest.
    UpdateSelected {
        ids: Vec<String>,
    },
    Consolidate {
        id: String,
        version: String,
        projects: Vec<String>,
    },
    /// The host should fetch these icons (the rows that show).
    Icons(Vec<String>),
    /// The settings button: Tools > Options > NuGet Package Manager > Package Sources.
    PackageSources,
}

/// A visible row of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A source that failed (`name`, why).
    SourceError(String, String),
    Package {
        id: String,
        /// The version the row shows (Browse: the newest; Installed: installed; Updates: the newest).
        version: String,
        /// Second line: the description, or the installed versions.
        detail: String,
        icon: Option<String>,
        warning: bool,
    },
}

impl Row {
    pub fn id(&self) -> Option<&str> {
        match self {
            Row::Package { id, .. } => Some(id),
            Row::SourceError(..) => None,
        }
    }
}

/// One package's place in one project (the Installed answer).
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub project: String,
    pub path: String,
    pub version: Option<String>,
    pub vulnerabilities: Vec<Vulnerability>,
    pub deprecated: bool,
}

pub struct NuGetWindow {
    theme: Theme,
    scope: Scope,
    /// The solution's projects (name, path), from the tree.
    projects: Vec<(String, String)>,
    tab: Tab,
    query: String,
    prerelease: bool,
    /// `None`: All.
    source: Option<String>,
    sources: Vec<String>,
    source_menu: bool,
    browse: Option<SearchOutput>,
    installed: Option<InstalledOutput>,
    updates: Option<UpdatesOutput>,
    checked_updates: BTreeSet<String>,
    rows: Vec<Row>,
    selected: Option<String>,
    version: Option<String>,
    version_menu: bool,
    checked_projects: BTreeSet<String>,
    /// Fetched icons by address (the host's cached file); `None` while fetching or when it failed.
    icons: HashMap<String, Option<PathBuf>>,
    /// What runs (a search, an install), shown in the status line.
    busy: Option<String>,
    message: Option<String>,
    search_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Where the search box, the tabs, the rows, the buttons and the project check boxes were drawn, while
    /// `--bounds-out` probes (the Xvfb run clicks them).
    probe: Option<eludite_ui::BoundsMap>,
    /// Renders of the list rows (tests read it: the list draws only what shows).
    rows_drawn: usize,
}

impl EventEmitter<WindowEvent> for NuGetWindow {}

impl NuGetWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            scope: Scope::Solution,
            projects: Vec::new(),
            tab: Tab::Browse,
            query: String::new(),
            prerelease: false,
            source: None,
            sources: Vec::new(),
            source_menu: false,
            browse: None,
            installed: None,
            updates: None,
            checked_updates: BTreeSet::new(),
            rows: Vec::new(),
            selected: None,
            version: None,
            version_menu: false,
            checked_projects: BTreeSet::new(),
            icons: HashMap::new(),
            busy: None,
            message: None,
            search_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            probe: None,
            rows_drawn: 0,
        }
    }

    pub fn set_probe(&mut self, probe: Option<eludite_ui::BoundsMap>) {
        self.probe = probe;
    }

    fn probed(&self, key: impl Into<String>) -> Option<gpui::AnyElement> {
        eludite_ui::bounds_canvas(self.probe.as_ref(), key)
    }

    // ------------------------------------------------------------------ what the shell sets and reads

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn prerelease(&self) -> bool {
        self.prerelease
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// Browse's last answer.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn browse_output(&self) -> Option<&SearchOutput> {
        self.browse.as_ref()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn status(&self) -> Option<String> {
        self.busy
            .clone()
            .map(|b| format!("{b}\u{2026}"))
            .or_else(|| self.message.clone())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows_drawn(&self) -> usize {
        self.rows_drawn
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn checked_projects(&self) -> &BTreeSet<String> {
        &self.checked_projects
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn icon(&self, url: &str) -> Option<&PathBuf> {
        self.icons.get(url).and_then(Option::as_ref)
    }

    /// The projects the actions apply to: the project of the scope, or the checked ones.
    pub fn target_projects(&self) -> Vec<String> {
        match &self.scope {
            Scope::Project { path, .. } => vec![path.clone()],
            Scope::Solution => self.checked_projects.iter().cloned().collect(),
        }
    }

    /// Scope, tab and query (the context menu's Manage NuGet Packages, an agent's eludite.nuget.manage).
    pub fn open(
        &mut self,
        scope: Scope,
        projects: Vec<(String, String)>,
        tab: Option<Tab>,
        query: Option<String>,
        prerelease: bool,
        cx: &mut Context<Self>,
    ) {
        if scope != self.scope {
            self.installed = None;
            self.updates = None;
            self.selected = None;
            self.checked_projects.clear();
        }
        self.scope = scope;
        self.projects = projects;
        if let Some(t) = tab {
            self.tab = t;
        }
        if let Some(q) = query {
            self.query = q;
        }
        if self.browse.is_none() {
            self.prerelease = prerelease;
        }
        self.rebuild();
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.source_menu = false;
        self.version_menu = false;
        self.rebuild();
        cx.notify();
    }

    pub fn set_sources(&mut self, sources: Vec<String>, cx: &mut Context<Self>) {
        if self.source.as_ref().is_some_and(|s| !sources.contains(s)) {
            self.source = None;
        }
        self.sources = sources;
        cx.notify();
    }

    pub fn set_busy(&mut self, busy: Option<String>, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }

    pub fn set_message(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.message = message;
        cx.notify();
    }

    pub fn set_browse(&mut self, out: SearchOutput, cx: &mut Context<Self>) {
        self.browse = Some(out);
        self.rebuild();
        cx.notify();
    }

    pub fn set_installed(&mut self, out: InstalledOutput, cx: &mut Context<Self>) {
        self.installed = Some(out);
        self.rebuild();
        cx.notify();
    }

    pub fn set_updates(&mut self, out: UpdatesOutput, cx: &mut Context<Self>) {
        self.checked_updates
            .retain(|id| out.updates.iter().any(|u| &u.id == id));
        self.updates = Some(out);
        self.rebuild();
        cx.notify();
    }

    pub fn set_icon(&mut self, url: String, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.icons.insert(url, path);
        cx.notify();
    }

    /// Icons the visible rows need that nobody asked for yet; marked as asked.
    pub fn take_icon_requests(&mut self, range: Range<usize>) -> Vec<String> {
        let wanted: Vec<String> = self.rows
            [range.start.min(self.rows.len())..range.end.min(self.rows.len())]
            .iter()
            .filter_map(|r| match r {
                Row::Package { icon: Some(u), .. } => Some(u.clone()),
                _ => None,
            })
            .filter(|u| !self.icons.contains_key(u))
            .collect();
        for u in &wanted {
            self.icons.insert(u.clone(), None);
        }
        wanted
    }

    pub fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.version = None;
        self.version_menu = false;
        self.selected = id;
        if let (Scope::Solution, Some(id)) = (&self.scope, self.selected.clone()) {
            // The projects that have it start checked for Uninstall and Update; none for Install.
            self.checked_projects = self
                .placements(&id)
                .into_iter()
                .filter(|p| p.version.is_some())
                .map(|p| p.path)
                .collect();
        }
        cx.notify();
    }

    pub fn set_version(&mut self, version: String, cx: &mut Context<Self>) {
        self.version = Some(version);
        self.version_menu = false;
        cx.notify();
    }

    pub fn toggle_project(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.checked_projects.remove(path) {
            self.checked_projects.insert(path.to_owned());
        }
        cx.notify();
    }

    pub fn toggle_update(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.checked_updates.remove(id) {
            self.checked_updates.insert(id.to_owned());
        }
        cx.notify();
    }

    /// Updates' "Select all packages".
    pub fn select_all_updates(&mut self, cx: &mut Context<Self>) {
        let all: BTreeSet<String> = self
            .updates
            .iter()
            .flat_map(|u| &u.updates)
            .filter(|u| self.in_scope(&u.project))
            .map(|u| u.id.clone())
            .collect();
        self.checked_updates = if self.checked_updates == all {
            BTreeSet::new()
        } else {
            all
        };
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn checked_updates(&self) -> &BTreeSet<String> {
        &self.checked_updates
    }

    // ------------------------------------------------------------------ the model

    fn in_scope(&self, project: &str) -> bool {
        match &self.scope {
            Scope::Solution => true,
            Scope::Project { path, .. } => path == project,
        }
    }

    fn scoped_projects(&self) -> Vec<&InstalledProject> {
        self.installed
            .iter()
            .flat_map(|i| &i.projects)
            .filter(|p| self.in_scope(&p.path))
            .collect()
    }

    /// Where package `id` is installed, per project of the scope (projects without it too, with no version).
    pub fn placements(&self, id: &str) -> Vec<Placement> {
        let installed: Vec<&InstalledProject> = self.scoped_projects();
        let mut out = Vec::new();
        let names: Vec<(String, String)> = match &self.scope {
            Scope::Project { name, path } => vec![(name.clone(), path.clone())],
            Scope::Solution if !self.projects.is_empty() => self.projects.clone(),
            Scope::Solution => installed
                .iter()
                .map(|p| (p.name.clone(), p.path.clone()))
                .collect(),
        };
        for (name, path) in names {
            let pkg = installed.iter().find(|p| p.path == path).and_then(|p| {
                p.packages
                    .iter()
                    .find(|x| !x.transitive && x.id.eq_ignore_ascii_case(id))
            });
            out.push(Placement {
                project: name,
                path,
                version: pkg.and_then(|p| p.version.clone().or_else(|| p.requested.clone())),
                vulnerabilities: pkg.map(|p| p.vulnerabilities.clone()).unwrap_or_default(),
                deprecated: pkg.is_some_and(|p| p.deprecated),
            });
        }
        out
    }

    /// Packages installed at different versions across the solution's projects (Consolidate), with the versions.
    pub fn consolidate_rows(&self) -> Vec<(String, Vec<String>)> {
        if self.scope != Scope::Solution {
            return Vec::new();
        }
        let mut by_id: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
        for p in self.scoped_projects() {
            for pk in p.packages.iter().filter(|x| !x.transitive) {
                if let Some(v) = pk.version.clone().or_else(|| pk.requested.clone()) {
                    by_id
                        .entry(pk.id.to_lowercase())
                        .or_insert_with(|| (pk.id.clone(), BTreeSet::new()))
                        .1
                        .insert(v);
                }
            }
        }
        by_id
            .into_values()
            .filter(|(_, v)| v.len() > 1)
            .map(|(id, v)| {
                let mut versions: Vec<String> = v.into_iter().collect();
                versions.sort_by_key(|x| std::cmp::Reverse(super::version_key(x)));
                (id, versions)
            })
            .collect()
    }

    fn update_rows(&self) -> Vec<&UpdateRow> {
        self.updates
            .iter()
            .flat_map(|u| &u.updates)
            .filter(|u| self.in_scope(&u.project))
            .collect()
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        match self.tab {
            Tab::Browse => {
                if let Some(b) = &self.browse {
                    for s in b.sources.iter().filter(|s| s.error.is_some()) {
                        rows.push(Row::SourceError(
                            s.name.clone(),
                            s.error.clone().unwrap_or_default(),
                        ));
                    }
                    for r in &b.results {
                        rows.push(Row::Package {
                            id: r.id.clone(),
                            version: r.version.clone(),
                            detail: r.description.clone().unwrap_or_default(),
                            icon: r.icon_url.clone(),
                            warning: !r.vulnerabilities.is_empty() || r.deprecated,
                        });
                    }
                }
            }
            Tab::Installed => {
                let mut by_id: BTreeMap<String, (String, BTreeSet<String>, bool)> = BTreeMap::new();
                for p in self.scoped_projects() {
                    for pk in p.packages.iter().filter(|x| !x.transitive) {
                        let e = by_id
                            .entry(pk.id.to_lowercase())
                            .or_insert_with(|| (pk.id.clone(), BTreeSet::new(), false));
                        if let Some(v) = pk.version.clone().or_else(|| pk.requested.clone()) {
                            e.1.insert(v);
                        }
                        e.2 |= !pk.vulnerabilities.is_empty() || pk.deprecated;
                    }
                }
                for (id, versions, warning) in by_id.into_values() {
                    let versions: Vec<String> = versions.into_iter().collect();
                    rows.push(Row::Package {
                        id,
                        version: versions.last().cloned().unwrap_or_default(),
                        detail: if versions.len() > 1 {
                            format!("Installed: {}", versions.join(", "))
                        } else {
                            String::new()
                        },
                        icon: None,
                        warning,
                    });
                }
            }
            Tab::Updates => {
                let mut by_id: BTreeMap<String, (String, String, BTreeSet<String>, bool)> =
                    BTreeMap::new();
                for u in self.update_rows() {
                    let e = by_id.entry(u.id.to_lowercase()).or_insert_with(|| {
                        (u.id.clone(), u.latest.clone(), BTreeSet::new(), false)
                    });
                    e.2.insert(u.installed.clone());
                    e.3 |= u.vulnerable;
                }
                for (id, latest, installed, warning) in by_id.into_values() {
                    rows.push(Row::Package {
                        id,
                        version: latest,
                        detail: format!(
                            "Installed: {}",
                            installed.into_iter().collect::<Vec<_>>().join(", ")
                        ),
                        icon: None,
                        warning,
                    });
                }
            }
            Tab::Consolidate => {
                for (id, versions) in self.consolidate_rows() {
                    rows.push(Row::Package {
                        version: versions[0].clone(),
                        detail: format!("Installed: {}", versions.join(", ")),
                        id,
                        icon: None,
                        warning: false,
                    });
                }
            }
        }
        self.rows = rows;
        if self
            .selected
            .as_ref()
            .is_some_and(|s| !self.rows.iter().any(|r| r.id() == Some(s.as_str())))
            && self.tab != Tab::Browse
        {
            self.selected = None;
        }
    }

    /// The search answer's row for the selected package, if the Browse tab found it.
    fn browse_row(&self, id: &str) -> Option<&SearchRow> {
        self.browse
            .as_ref()?
            .results
            .iter()
            .find(|r| r.id.eq_ignore_ascii_case(id))
    }

    /// The versions the dropdown offers for `id`, newest first.
    pub fn versions(&self, id: &str) -> Vec<String> {
        if let Some(r) = self.browse_row(id).filter(|r| !r.versions.is_empty()) {
            return r.versions.clone();
        }
        match self.tab {
            Tab::Consolidate => self
                .consolidate_rows()
                .into_iter()
                .find(|(x, _)| x.eq_ignore_ascii_case(id))
                .map(|(_, v)| v)
                .unwrap_or_default(),
            _ => {
                let mut v: Vec<String> = self
                    .update_rows()
                    .into_iter()
                    .filter(|u| u.id.eq_ignore_ascii_case(id))
                    .map(|u| u.latest.clone())
                    .collect();
                v.extend(self.placements(id).into_iter().filter_map(|p| p.version));
                v.sort_by_key(|x| std::cmp::Reverse(super::version_key(x)));
                v.dedup();
                v
            }
        }
    }

    /// The version the detail pane acts with: the one chosen, else the first offered.
    pub fn chosen_version(&self) -> Option<String> {
        let id = self.selected.as_ref()?;
        self.version
            .clone()
            .or_else(|| self.versions(id).into_iter().next())
    }

    /// The detail pane's lines (tests read them as the person sees them).
    pub fn details(&self) -> Vec<String> {
        let Some(id) = &self.selected else {
            return Vec::new();
        };
        let mut lines = vec![id.clone()];
        if let Some(v) = self.chosen_version() {
            lines.push(format!("Version: {v}"));
        }
        let placements = self.placements(id);
        let installed: Vec<String> = placements
            .iter()
            .filter_map(|p| p.version.as_ref().map(|v| format!("{} {v}", p.project)))
            .collect();
        if !installed.is_empty() {
            lines.push(format!("Installed: {}", installed.join(", ")));
        }
        if let Some(r) = self.browse_row(id) {
            if let Some(d) = &r.description {
                lines.push(d.clone());
            }
            if let Some(a) = &r.authors {
                lines.push(format!("Author(s): {a}"));
            }
            if let Some(l) = &r.license {
                lines.push(format!("License: {l}"));
            }
            if let Some(u) = &r.project_url {
                lines.push(format!("Project URL: {u}"));
            }
            if let Some(n) = r.downloads {
                lines.push(format!("Downloads: {n}"));
            }
            for v in &r.vulnerabilities {
                lines.push(vulnerability_line(v));
            }
            if r.deprecated {
                lines.push("\u{26A0} This package version is deprecated.".into());
            }
        }
        let mut seen = BTreeSet::new();
        for p in &placements {
            for v in &p.vulnerabilities {
                if seen.insert(v.advisory_url.clone()) {
                    lines.push(format!(
                        "{} ({} {})",
                        vulnerability_line(v),
                        p.project,
                        p.version.as_deref().unwrap_or_default()
                    ));
                }
            }
            if p.deprecated {
                lines.push(format!(
                    "\u{26A0} {} {} is deprecated.",
                    id,
                    p.version.as_deref().unwrap_or_default()
                ));
            }
        }
        lines
    }

    /// Which buttons the detail pane offers for the selection: (install, update, uninstall, consolidate).
    pub fn actions(&self) -> (bool, bool, bool, bool) {
        let Some(id) = &self.selected else {
            return (false, false, false, false);
        };
        let version = self.chosen_version();
        let placements = self.placements(id);
        let targets: Vec<&Placement> = match &self.scope {
            Scope::Project { .. } => placements.iter().collect(),
            Scope::Solution => placements
                .iter()
                .filter(|p| self.checked_projects.contains(&p.path))
                .collect(),
        };
        let any_target = !targets.is_empty() && version.is_some();
        let has = targets.iter().any(|p| p.version.is_some());
        let differs = targets
            .iter()
            .any(|p| p.version.is_some() && p.version != version);
        let missing = targets.iter().any(|p| p.version.is_none());
        match self.tab {
            Tab::Consolidate => (false, false, false, any_target),
            Tab::Installed => (
                false,
                any_target && differs,
                !targets.is_empty() && has,
                false,
            ),
            Tab::Updates => (false, any_target && differs, false, false),
            Tab::Browse => (
                any_target && missing,
                any_target && has && differs,
                false,
                false,
            ),
        }
    }

    // ------------------------------------------------------------------ input

    fn search_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "enter" => {
                cx.stop_propagation();
                cx.emit(WindowEvent::Search);
                return;
            }
            "backspace" => {
                self.query.pop();
            }
            "escape" => self.query.clear(),
            "space" => self.query.push(' '),
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        self.query.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
        if self.tab == Tab::Browse {
            cx.emit(WindowEvent::QueryChanged);
        }
    }

    fn act(&mut self, which: &'static str, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let version = self.chosen_version().unwrap_or_default();
        let projects = self.target_projects();
        let event = match which {
            INSTALL => {
                let missing: Vec<String> = self
                    .placements(&id)
                    .into_iter()
                    .filter(|p| p.version.is_none() && projects.contains(&p.path))
                    .map(|p| p.path)
                    .collect();
                WindowEvent::Install {
                    id,
                    version,
                    projects: missing,
                }
            }
            UPDATE => WindowEvent::Update {
                id,
                version,
                projects,
            },
            UNINSTALL => WindowEvent::Uninstall { id, projects },
            _ => WindowEvent::Consolidate {
                id,
                version,
                projects,
            },
        };
        cx.emit(event);
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let icons_wanted = self.take_icon_requests(range.clone());
        if !icons_wanted.is_empty() {
            cx.emit(WindowEvent::Icons(icons_wanted));
        }
        self.rows_drawn += range.len();
        let updates = self.tab == Tab::Updates;
        // Few elements per row: in a debug build every builder call moves the element, and the list redraws its
        // rows each frame (brief 0048's frame budget).
        let probe = self.probe.clone();
        range
            .filter_map(|ix| {
                let row = self.rows.get(ix)?;
                let sel = row_selector(ix);
                let mut el = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(t.typography.ui);
                if probe.is_some() {
                    el = el
                        .relative()
                        .children(eludite_ui::bounds_canvas(probe.as_ref(), row_selector(ix)));
                }
                Some(match row {
                    Row::SourceError(name, why) => el
                        .text_color(rgb(0xF1_4C_4C))
                        .child(format!("\u{2716} {name}: {why}"))
                        .into_any_element(),
                    Row::Package {
                        id,
                        version,
                        detail,
                        icon,
                        warning,
                    } => {
                        let icon_el = match icon
                            .as_ref()
                            .and_then(|u| self.icons.get(u))
                            .cloned()
                            .flatten()
                        {
                            Some(path) => img(path).size(px(32.)).into_any_element(),
                            None => div()
                                .size(px(32.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(t.chrome)
                                .text_color(t.text_muted)
                                .child("\u{25C8}")
                                .into_any_element(),
                        };
                        if updates {
                            let check_id = id.clone();
                            el = el.child(
                                check_box(
                                    update_check_selector(id),
                                    "",
                                    self.checked_updates.contains(id),
                                    &t,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.toggle_update(&check_id, cx),
                                )),
                            );
                        }
                        el = if self.selected.as_deref() == Some(id.as_str()) {
                            el.bg(t.menu_hover)
                        } else {
                            el.hover(|s| s.bg(t.menu_hover))
                        };
                        // A vulnerable or deprecated package: the warning glyph, in yellow, before its version.
                        let (version_text, version_color) = if *warning {
                            (
                                SharedString::from(format!("\u{26A0} {version}")),
                                rgb(0xFF_CC_00),
                            )
                        } else {
                            (SharedString::from(version.clone()), t.text_muted)
                        };
                        let select_id = id.clone();
                        el.cursor_pointer()
                            .child(icon_el)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(
                                        div().font_weight(FontWeight::SEMIBOLD).child(id.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(t.typography.small)
                                            .text_color(t.text_muted)
                                            .child(detail.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(version_color)
                                    .child(version_text),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(Some(select_id.clone()), cx)
                            }))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn details_pane(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let mut pane = div()
            .id(DETAILS)
            .debug_selector(|| DETAILS.into())
            .w(px(340.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_l_1()
            .border_color(t.border)
            .overflow_y_scroll()
            .text_size(t.typography.ui);
        let Some(id) = self.selected.clone() else {
            return pane.child(
                div()
                    .text_color(t.text_muted)
                    .child("Select a package to see its details."),
            );
        };
        let versions = self.versions(&id);
        let chosen = self.chosen_version().unwrap_or_default();
        let (install, update, uninstall, consolidate) = self.actions();
        let probe = self.probe.clone();
        let button =
            |sel: &'static str, label: &'static str, enabled: bool, cx: &mut Context<Self>| {
                let b = push_button(sel, label, false, enabled, &t)
                    .relative()
                    .children(eludite_ui::bounds_canvas(probe.as_ref(), sel));
                if enabled {
                    b.on_click(cx.listener(move |this, _, _, cx| this.act(sel, cx)))
                } else {
                    b
                }
            };
        pane = pane
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(id.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child("Version:")
                    .child(
                        toggle_button(VERSION_BUTTON, format!("{chosen} \u{25BE}"), false, &t)
                            .min_w(px(120.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.version_menu = !this.version_menu;
                                cx.notify();
                            })),
                    ),
            );
        if self.version_menu {
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(t.border)
                    .children(versions.into_iter().enumerate().map(|(ix, v)| {
                        let pick = v.clone();
                        menu_row(
                            version_item_selector(ix),
                            v.clone(),
                            0,
                            v == chosen,
                            false,
                            false,
                            &t,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_version(pick.clone(), cx)),
                        )
                    })),
            );
        }
        pane = pane.child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .child(button(INSTALL, "Install", install, cx))
                .child(button(UPDATE, "Update", update, cx))
                .child(button(UNINSTALL, "Uninstall", uninstall, cx))
                .children(
                    (self.tab == Tab::Consolidate)
                        .then(|| button(CONSOLIDATE, "Install", consolidate, cx)),
                ),
        );
        if self.scope == Scope::Solution {
            pane = pane.child(
                div()
                    .pt_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Projects"),
            );
            for p in self.placements(&id) {
                let path = p.path.clone();
                let label = match &p.version {
                    Some(v) => format!("{}  {v}", p.project),
                    None => p.project.clone(),
                };
                pane = pane.child(
                    check_box(
                        project_check_selector(&p.project),
                        label,
                        self.checked_projects.contains(&p.path),
                        &t,
                    )
                    .relative()
                    .children(self.probed(project_check_selector(&p.project)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(&path, cx))),
                );
            }
        }
        for (ix, line) in self.details().into_iter().skip(1).enumerate() {
            let warning = line.starts_with('\u{26A0}');
            let row = div().id(("nuget-detail", ix)).text_color(if warning {
                rgb(0xFF_CC_00)
            } else {
                t.text
            });
            // The license, the project's page and an advisory open in the system's browser.
            pane = pane.child(match detail_link(&line) {
                Some(url) => {
                    let sel = detail_link_selector(ix);
                    row.debug_selector(move || sel)
                        .cursor_pointer()
                        .underline()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(line)
                }
                None => row.child(line),
            });
        }
        pane
    }
}

/// Debug selector of the detail pane's line `ix` (after the package's name) when it is a link.
pub fn detail_link_selector(ix: usize) -> String {
    format!("nuget-detail-link-{ix}")
}

/// Where a detail pane line links to: the license (an SPDX expression on licenses.nuget.org, as Visual Studio links
/// it, or the package's license address), the project's page, or a vulnerability's advisory.
pub fn detail_link(line: &str) -> Option<String> {
    let web =
        |u: &str| (u.starts_with("https://") || u.starts_with("http://")).then(|| u.to_owned());
    if let Some(l) = line.strip_prefix("License: ") {
        return web(l).or_else(|| Some(format!("https://licenses.nuget.org/{l}")));
    }
    if let Some(u) = line.strip_prefix("Project URL: ") {
        return web(u);
    }
    line.starts_with('\u{26A0}')
        .then(|| line.split_once(": "))
        .flatten()
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(web)
}

/// A vulnerability as the detail pane shows it.
pub fn vulnerability_line(v: &Vulnerability) -> String {
    let severity = match v.severity.as_str() {
        "low" => "Low",
        "moderate" => "Moderate",
        "high" => "High",
        _ => "Critical",
    };
    format!(
        "\u{26A0} {severity} severity vulnerability: {}",
        v.advisory_url
    )
}

impl Focusable for NuGetWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.search_focus.clone()
    }
}

impl Render for NuGetWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let tab_label = |tab: Tab| -> String {
            match tab {
                Tab::Browse => "Browse".into(),
                Tab::Installed => "Installed".into(),
                Tab::Updates => match self.updates.as_ref() {
                    Some(_) => format!("Updates {}", self.update_rows().len()),
                    None => "Updates".into(),
                },
                Tab::Consolidate => match self.installed.as_ref() {
                    Some(_) if self.scope == Scope::Solution => {
                        format!("Consolidate {}", self.consolidate_rows().len())
                    }
                    _ => "Consolidate".into(),
                },
            }
        };
        let tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .children(
                [Tab::Browse, Tab::Installed, Tab::Updates, Tab::Consolidate].map(|tab| {
                    toggle_button(tab_selector(tab), tab_label(tab), self.tab == tab, &t)
                        .relative()
                        .children(self.probed(tab_selector(tab)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_tab(tab, cx);
                            cx.emit(if tab == Tab::Browse {
                                WindowEvent::Search
                            } else {
                                WindowEvent::Refresh
                            });
                        }))
                }),
            )
            .child(div().flex_1())
            .child(div().text_size(px(15.)).child(match &self.scope {
                Scope::Solution => "Manage Packages for Solution".to_owned(),
                Scope::Project { name, .. } => format!("NuGet Package Manager: {name}"),
            }));
        let focused = self.search_focus.is_focused(window);
        let search = text_box(SEARCH_BOX, &self.query, "Search (Ctrl+L)", focused, &t)
            .relative()
            .children(self.probed(SEARCH_BOX))
            .w(px(260.))
            .track_focus(&self.search_focus)
            .key_context("NuGetSearch")
            .on_key_down(cx.listener(Self::search_key))
            .on_click(cx.listener(|this, _, window, cx| {
                this.search_focus.focus(window, cx);
                cx.notify();
            }));
        let source_label = self.source.clone().unwrap_or_else(|| "All".into());
        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(search)
            .child(
                toggle_button(REFRESH, "\u{21BB}", false, &t).on_click(cx.listener(
                    |this, _, _, cx| {
                        cx.emit(if this.tab == Tab::Browse {
                            WindowEvent::Search
                        } else {
                            WindowEvent::Refresh
                        })
                    },
                )),
            )
            .child(
                check_box(PRERELEASE, "Include prerelease", self.prerelease, &t).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.prerelease = !this.prerelease;
                        cx.notify();
                        cx.emit(if this.tab == Tab::Browse {
                            WindowEvent::Search
                        } else {
                            WindowEvent::Refresh
                        });
                    }),
                ),
            )
            .child(div().flex_1())
            .child("Package source:")
            .child(
                toggle_button(SOURCE_BUTTON, format!("{source_label} \u{25BE}"), false, &t)
                    .min_w(px(110.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source_menu = !this.source_menu;
                        cx.notify();
                    })),
            )
            .child(
                toggle_button(SETTINGS, "\u{2699}", false, &t)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(WindowEvent::PackageSources))),
            );
        let source_menu = self.source_menu.then(|| {
            let mut names: Vec<Option<String>> = vec![None];
            names.extend(self.sources.iter().cloned().map(Some));
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("nuget-source-menu")
                        .occlude()
                        .py_1()
                        .text_size(t.typography.ui)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.source_menu = false;
                            cx.notify();
                        }))
                        .children(names.into_iter().enumerate().map(|(ix, name)| {
                            let label = name.clone().unwrap_or_else(|| "All".into());
                            let selected = name == self.source;
                            menu_row(
                                source_item_selector(ix),
                                label,
                                0,
                                selected,
                                false,
                                false,
                                &t,
                            )
                            .min_w(px(160.))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.source = name.clone();
                                    this.source_menu = false;
                                    cx.notify();
                                    cx.emit(WindowEvent::Search);
                                },
                            ))
                        })),
                ),
            )
            .with_priority(3)
        });
        let updates_bar = (self.tab == Tab::Updates).then(|| {
            let any = !self.checked_updates.is_empty();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .child(
                    check_box(
                        SELECT_ALL,
                        "Select all packages",
                        any && self.checked_updates.len() == self.rows.len(),
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.select_all_updates(cx))),
                )
                .child({
                    let b = push_button(UPDATE_SELECTED, "Update", false, any, &t);
                    if any {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            let ids = this.checked_updates.iter().cloned().collect();
                            cx.emit(WindowEvent::UpdateSelected { ids });
                        }))
                    } else {
                        b
                    }
                })
        });
        let status = self.status();
        let empty = match self.tab {
            Tab::Consolidate if self.scope != Scope::Solution => {
                Some("Consolidate is in Manage NuGet Packages for Solution.")
            }
            _ if self.rows.is_empty() && self.busy.is_none() => Some(match self.tab {
                Tab::Browse => "No packages found.",
                Tab::Installed => "No packages are installed.",
                Tab::Updates => "No updates are available.",
                Tab::Consolidate => "No packages need consolidation.",
            }),
            _ => None,
        };
        let count = self.rows.len();
        div()
            .id(WINDOW)
            .debug_selector(|| WINDOW.into())
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(tabs)
                    .child(toolbar),
            )
            .children(updates_bar)
            .children(status.map(|s| {
                div()
                    .id(STATUS)
                    .debug_selector(|| STATUS.into())
                    .flex_none()
                    .px_2()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(s)
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .children(empty.map(|e| div().p_2().text_color(t.text_muted).child(e)))
                            .child(
                                uniform_list(
                                    "nuget-rows",
                                    count,
                                    cx.processor(|this, range: Range<usize>, _, cx| {
                                        this.render_rows(range, cx)
                                    }),
                                )
                                .track_scroll(&self.scroll)
                                .flex_1()
                                .min_h_0(),
                            ),
                    )
                    .child(self.details_pane(cx)),
            )
            .children(source_menu)
    }
}
