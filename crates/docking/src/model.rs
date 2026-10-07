//! The docking layout as pure data: what is docked where, what floats, what is
//! auto-hidden or closed, and the document tabs. No GPUI types; everything here
//! is `Send` so command handlers on any thread can change it.
//!
//! # JSON schema and migration
//!
//! [`DockLayout`] serializes to JSON with a top-level `"version"`
//! ([`LAYOUT_SCHEMA_VERSION`]). Field names are stable: renaming or removing one
//! is a schema change. To change the schema:
//! 1. bump `LAYOUT_SCHEMA_VERSION`;
//! 2. add a step to [`migrate`] that rewrites a `serde_json::Value` of the old
//!    version into the new one (steps run in order, oldest first);
//! 3. add a test that loads a checked-in example of the old version.
//!
//! A file from a *newer* version than this build is rejected (the caller falls
//! back to the default layout and leaves the file alone). After loading, a
//! layout is [`normalize`](DockLayout::normalize)d against the registered tool
//! windows, so windows added by later builds appear (closed, at their default
//! side) and windows that no longer exist are dropped without a version bump.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current layout file schema version.
pub const LAYOUT_SCHEMA_VERSION: u32 = 5;

/// A dock edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

impl DockSide {
    pub const ALL: [DockSide; 3] = [DockSide::Left, DockSide::Right, DockSide::Bottom];

    pub fn name(self) -> &'static str {
        match self {
            DockSide::Left => "left",
            DockSide::Right => "right",
            DockSide::Bottom => "bottom",
        }
    }
}

/// Well-known tool window ids (Visual Studio's windows, snake_case).
pub mod ids {
    pub const WORKSPACE: &str = "workspace";
    pub const GIT_CHANGES: &str = "git_changes";
    pub const PROPERTIES: &str = "properties";
    pub const ERROR_LIST: &str = "error_list";
    pub const OUTPUT: &str = "output";
    pub const TOOLBOX: &str = "toolbox";
    /// Find All References (brief 0014): closed until a search shows it, then tabbed with the Error List.
    pub const FIND_ALL_REFERENCES: &str = "find_all_references";
    /// The Agents window (brief 0016): docked right, tabbed with Workspace.
    pub const AGENTS: &str = "agents";
    /// The debugger windows (brief 0018): closed until a debugging session shows them, then tabbed at the bottom.
    pub const LOCALS: &str = "locals";
    pub const WATCH: &str = "watch";
    pub const CALL_STACK: &str = "call_stack";
    pub const THREADS: &str = "threads";
    pub const BREAKPOINTS: &str = "breakpoints";
    pub const EXCEPTION_SETTINGS: &str = "exception_settings";
    /// Test > Test Explorer (brief 0035): closed until shown, docked left as Visual Studio docks it.
    pub const TEST_EXPLORER: &str = "test_explorer";
    /// Git > Manage Branches, Ctrl+0, Ctrl+R (brief 0040): the Git Repository window, closed until shown, docked at
    /// the bottom where its history has room.
    pub const GIT_REPOSITORY: &str = "git_repository";
    /// View > Terminal, Ctrl+` (brief 0041): the Terminal window, tabbed at the bottom with the Error List and Output.
    pub const TERMINAL: &str = "terminal";
    /// Find Results 1 and 2 (brief 0042): Visual Studio's two Find in Files results windows, closed until a search
    /// shows one, then tabbed at the bottom with the Error List.
    pub const FIND_RESULTS_1: &str = "find_results_1";
    pub const FIND_RESULTS_2: &str = "find_results_2";
    /// View > Other Windows > Pull Requests and Issues (brief 0046): the forge's lists, closed until shown, docked
    /// right beside Git Changes.
    pub const PULL_REQUESTS: &str = "pull_requests";
    pub const ISSUES: &str = "issues";
    /// The windows a debugging session shows, in tab order (Visual Studio's Debug layout). The program's output is
    /// the Output window's Debug source (brief 0020 retired the Debug Console window).
    pub const DEBUG_SESSION: [&str; 3] = [LOCALS, WATCH, CALL_STACK];
    /// The Web Browser window (brief 0032): View > Other Windows > Web Browser.
    pub const WEB_BROWSER: &str = "web_browser";
    /// The Manage NuGet Packages window (brief 0048), a document tab as Visual Studio's; its title names its scope.
    pub const NUGET: &str = "nuget";
    /// Windows that open as a document tab, as Visual Studio opens its Web Browser in the document well: `view.show`
    /// opens them there (id and title), and they close like documents.
    pub const DOCUMENT_WINDOWS: [(&str, &str); 3] = [
        (WEB_BROWSER, "Web Browser"),
        (NUGET, "NuGet - Solution"),
        (PROJECT_PROPERTIES, "Properties"),
    ];
    /// Project > Properties (brief 0049): `view.show` opens it as a document tab, which the shell turns into the
    /// selected project's property pages (`project_properties:<project file>`).
    pub const PROJECT_PROPERTIES: &str = "project_properties";
}

/// A kind of tool window the shell knows about. Later briefs register more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWindowDescriptor {
    pub id: String,
    pub title: String,
    /// Where it docks the first time it is shown.
    pub default_side: DockSide,
}

impl ToolWindowDescriptor {
    pub fn new(id: impl Into<String>, title: impl Into<String>, default_side: DockSide) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            default_side,
        }
    }
}

/// The registered tool windows, in registration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolWindowRegistry {
    windows: Vec<ToolWindowDescriptor>,
}

impl ToolWindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The Visual Studio windows of the default layout.
    pub fn vs_default() -> Self {
        let mut r = Self::new();
        for (id, title, side) in [
            (ids::WORKSPACE, "Workspace", DockSide::Right),
            (ids::GIT_CHANGES, "Git Changes", DockSide::Right),
            (ids::PROPERTIES, "Properties", DockSide::Right),
            (ids::ERROR_LIST, "Error List", DockSide::Bottom),
            (ids::OUTPUT, "Output", DockSide::Bottom),
            (ids::TOOLBOX, "Toolbox", DockSide::Left),
            (
                ids::FIND_ALL_REFERENCES,
                "Find All References",
                DockSide::Bottom,
            ),
            (ids::AGENTS, "Agents", DockSide::Right),
            (ids::LOCALS, "Locals", DockSide::Bottom),
            (ids::WATCH, "Watch 1", DockSide::Bottom),
            (ids::CALL_STACK, "Call Stack", DockSide::Bottom),
            (ids::THREADS, "Threads", DockSide::Bottom),
            (ids::BREAKPOINTS, "Breakpoints", DockSide::Bottom),
            (
                ids::EXCEPTION_SETTINGS,
                "Exception Settings",
                DockSide::Bottom,
            ),
            (ids::TEST_EXPLORER, "Test Explorer", DockSide::Left),
            (ids::GIT_REPOSITORY, "Git Repository", DockSide::Bottom),
            (ids::TERMINAL, "Terminal", DockSide::Bottom),
            (ids::FIND_RESULTS_1, "Find Results 1", DockSide::Bottom),
            (ids::FIND_RESULTS_2, "Find Results 2", DockSide::Bottom),
            (ids::PULL_REQUESTS, "Pull Requests", DockSide::Right),
            (ids::ISSUES, "Issues", DockSide::Right),
        ] {
            r.register(ToolWindowDescriptor::new(id, title, side));
        }
        r
    }

    /// Add a tool window, or replace the one with the same id.
    pub fn register(&mut self, descriptor: ToolWindowDescriptor) {
        match self.windows.iter_mut().find(|w| w.id == descriptor.id) {
            Some(w) => *w = descriptor,
            None => self.windows.push(descriptor),
        }
    }

    pub fn get(&self, id: &str) -> Option<&ToolWindowDescriptor> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// Display title, or the id itself for an unknown window.
    pub fn title<'a>(&'a self, id: &'a str) -> &'a str {
        self.get(id).map_or(id, |w| w.title.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolWindowDescriptor> {
        self.windows.iter()
    }
}

/// Tool windows shown one at a time behind a tab strip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: u32,
    pub tabs: Vec<String>,
    pub active: usize,
    /// The group's share of its dock's length relative to its neighbors
    /// (a flex weight; the shares on an edge are normalized when read).
    /// Absent means 1: every group the same size, as before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f32>,
}

impl Group {
    pub fn active_id(&self) -> Option<&str> {
        self.tabs.get(self.active).map(String::as_str)
    }

    /// The flex weight (`share`, or 1 when absent).
    pub fn weight(&self) -> f32 {
        self.share.unwrap_or(1.)
    }

    fn contains(&self, id: &str) -> bool {
        self.tabs.iter().any(|t| t == id)
    }
}

/// One dock edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    /// Groups along the dock: top to bottom on the left and right, left to
    /// right at the bottom.
    pub groups: Vec<Group>,
    /// Width (left, right) or height (bottom), logical pixels.
    pub size: f32,
    /// Auto-hidden tool windows, shown as tabs on this edge's strip.
    pub auto_hidden: Vec<String>,
}

impl Dock {
    fn new(size: f32) -> Self {
        Self {
            groups: Vec::new(),
            size,
            auto_hidden: Vec::new(),
        }
    }

    /// Each group's share of the dock's length, in group order; they sum to 1.
    pub fn shares(&self) -> Vec<f32> {
        let total: f32 = self.groups.iter().map(Group::weight).sum();
        if total <= 0. {
            return vec![1. / self.groups.len().max(1) as f32; self.groups.len()];
        }
        self.groups.iter().map(|g| g.weight() / total).collect()
    }
}

/// The smallest a dock can be made, logical pixels (`eludite.view.resize`).
pub const MIN_DOCK_SIZE: f32 = 40.;
/// The smallest share of its dock a group keeps when a neighbor grows.
pub const MIN_GROUP_SHARE: f32 = 0.1;

/// Floating window bounds, logical pixels. The position is advisory: Wayland
/// compositors place top-level windows themselves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Bounds {
    pub const DEFAULT_FLOAT: Bounds = Bounds {
        x: 200.,
        y: 150.,
        width: 360.,
        height: 420.,
    };
}

/// A group in its own OS window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatingGroup {
    pub group: Group,
    pub bounds: Bounds,
    /// The dock "Dock" sends it back to.
    pub home: DockSide,
}

/// A closed tool window and the dock it reopens in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiddenWindow {
    pub id: String,
    pub side: DockSide,
}

/// A document tab. Pinned tabs come first; at most one preview tab exists and
/// it comes last (Visual Studio's preview tab, italic, replaced by the next
/// preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentTab {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub preview: bool,
}

impl DocumentTab {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            pinned: false,
            preview: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentArea {
    pub tabs: Vec<DocumentTab>,
    pub active: Option<String>,
}

impl DocumentArea {
    pub fn get(&self, id: &str) -> Option<&DocumentTab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn active_tab(&self) -> Option<&DocumentTab> {
        self.active.as_deref().and_then(|id| self.get(id))
    }

    /// Open (or activate) a normal tab. Opening the current preview tab's
    /// document promotes it.
    pub fn open(&mut self, id: &str, title: &str) {
        match self.tabs.iter_mut().find(|t| t.id == id) {
            Some(t) => t.preview = false,
            None => self.tabs.push(DocumentTab::new(id, title)),
        }
        self.active = Some(id.to_owned());
        self.reorder();
    }

    /// Open a document in the preview tab, replacing the previous preview.
    pub fn open_preview(&mut self, id: &str, title: &str) {
        if self.get(id).is_none() {
            self.tabs.retain(|t| !t.preview);
            let mut tab = DocumentTab::new(id, title);
            tab.preview = true;
            self.tabs.push(tab);
        }
        self.active = Some(id.to_owned());
        self.reorder();
    }

    /// Keep the preview tab open as a normal tab.
    pub fn promote(&mut self, id: &str) -> bool {
        match self.tabs.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.preview = false;
                self.reorder();
                true
            }
            None => false,
        }
    }

    /// Pin or unpin. Pinning a preview tab promotes it.
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        match self.tabs.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.pinned = pinned;
                if pinned {
                    t.preview = false;
                }
                self.reorder();
                true
            }
            None => false,
        }
    }

    pub fn activate(&mut self, id: &str) -> bool {
        if self.get(id).is_some() {
            self.active = Some(id.to_owned());
            true
        } else {
            false
        }
    }

    pub fn close(&mut self, id: &str) -> bool {
        let Some(ix) = self.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        self.tabs.remove(ix);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .tabs
                .get(ix.min(self.tabs.len().saturating_sub(1)))
                .map(|t| t.id.clone());
        }
        true
    }

    /// Pinned first, preview last, stable otherwise.
    fn reorder(&mut self) {
        self.tabs.sort_by_key(|t| {
            if t.pinned {
                0
            } else if t.preview {
                2
            } else {
                1
            }
        });
    }
}

/// Where a tool window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Docked { side: DockSide, group: u32 },
    Floating { group: u32 },
    AutoHidden { side: DockSide },
    Hidden { side: DockSide },
}

/// Why a layout operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("unknown tool window `{0}`")]
    UnknownWindow(String),
    #[error("`{0}` is floating; dock it before auto-hiding it")]
    CannotAutoHideFloating(String),
    #[error("`{0}` is closed; show it first")]
    Closed(String),
    #[error("`{0}` is not docked; only docked groups are resized")]
    NotDocked(String),
}

/// A tool window's full state, as the `eludite.view.*` commands report it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolWindowInfo {
    pub id: String,
    pub title: String,
    pub place: Place,
    /// Group tabs in order (docked and floating windows).
    pub group: Option<Vec<String>>,
    /// Visible tab of its group.
    pub active: bool,
}

/// The whole window layout: three docks around the document area, floating
/// groups, and closed windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    pub version: u32,
    pub left: Dock,
    pub right: Dock,
    pub bottom: Dock,
    pub floating: Vec<FloatingGroup>,
    pub hidden: Vec<HiddenWindow>,
    pub documents: DocumentArea,
    pub next_group_id: u32,
}

/// Alias matching PLAN.md's vocabulary (`Layout::default_vs()`).
pub type Layout = DockLayout;

impl DockLayout {
    pub fn empty() -> Self {
        Self {
            version: LAYOUT_SCHEMA_VERSION,
            left: Dock::new(240.),
            right: Dock::new(300.),
            bottom: Dock::new(200.),
            floating: Vec::new(),
            hidden: Vec::new(),
            documents: DocumentArea::default(),
            next_group_id: 1,
        }
    }

    /// The default (PLAN.md 8): Workspace with Git Changes and Agents tabbed on
    /// the right, Error List, Output and Terminal tabbed at the bottom, a Welcome
    /// document. Every other registered window starts closed, Properties and
    /// Toolbox among them until they have content; the View menu opens them.
    pub fn default_vs(registry: &ToolWindowRegistry) -> Self {
        let mut l = Self::empty();
        let group = |id: u32, tabs: &[&str]| Group {
            id,
            tabs: tabs.iter().map(|t| (*t).to_owned()).collect(),
            active: 0,
            share: None,
        };
        l.right.groups = vec![group(1, &[ids::WORKSPACE, ids::GIT_CHANGES, ids::AGENTS])];
        l.bottom.groups = vec![group(2, &[ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL])];
        l.next_group_id = 3;
        l.documents.open("welcome", "Welcome");
        l.normalize(registry);
        l
    }

    /// For the docking tests: the default with every dock in use, Properties under the Workspace group on the right
    /// and Toolbox auto-hidden on the left (Visual Studio's own default).
    #[cfg(test)]
    pub(crate) fn fixture(registry: &ToolWindowRegistry) -> Self {
        let mut l = Self::default_vs(registry);
        l.right.groups.push(Group {
            id: l.next_group_id,
            tabs: vec![ids::PROPERTIES.to_owned()],
            active: 0,
            share: None,
        });
        l.next_group_id += 1;
        l.left.auto_hidden.push(ids::TOOLBOX.into());
        l.normalize(registry);
        l
    }

    pub fn dock(&self, side: DockSide) -> &Dock {
        match side {
            DockSide::Left => &self.left,
            DockSide::Right => &self.right,
            DockSide::Bottom => &self.bottom,
        }
    }

    pub fn dock_mut(&mut self, side: DockSide) -> &mut Dock {
        match side {
            DockSide::Left => &mut self.left,
            DockSide::Right => &mut self.right,
            DockSide::Bottom => &mut self.bottom,
        }
    }

    /// Set a dock's width (left, right) or height (bottom), clamped to
    /// [`MIN_DOCK_SIZE`]. Returns the size in effect.
    pub fn resize_dock(&mut self, side: DockSide, size: f32) -> f32 {
        let dock = self.dock_mut(side);
        dock.size = if size.is_finite() {
            size.max(MIN_DOCK_SIZE)
        } else {
            dock.size
        };
        dock.size
    }

    /// Give the group holding `id` `share` of its dock, scaling the other
    /// groups on that edge to fill the rest; every group keeps at least
    /// [`MIN_GROUP_SHARE`]. Returns the edge and the shares in effect.
    pub fn resize_group(
        &mut self,
        id: &str,
        share: f32,
    ) -> Result<(DockSide, Vec<f32>), LayoutError> {
        let Some(Place::Docked { side, .. }) = self.find(id) else {
            return Err(if self.find(id).is_none() {
                LayoutError::UnknownWindow(id.to_owned())
            } else {
                LayoutError::NotDocked(id.to_owned())
            });
        };
        let dock = self.dock_mut(side);
        let n = dock.groups.len();
        let ix = dock
            .groups
            .iter()
            .position(|g| g.contains(id))
            .expect("docked window has a group");
        if n > 1 && share.is_finite() {
            let others = (n - 1) as f32;
            let share = share.clamp(MIN_GROUP_SHARE, 1. - MIN_GROUP_SHARE * others);
            let old = dock.shares();
            let rest_old: f32 = 1. - old[ix];
            for (i, g) in dock.groups.iter_mut().enumerate() {
                let s = if i == ix {
                    share
                } else if rest_old > 0. {
                    (1. - share) * old[i] / rest_old
                } else {
                    (1. - share) / others
                };
                g.share = Some(s);
            }
        }
        Ok((side, self.dock(side).shares()))
    }

    pub fn find(&self, id: &str) -> Option<Place> {
        for side in DockSide::ALL {
            let dock = self.dock(side);
            if let Some(g) = dock.groups.iter().find(|g| g.contains(id)) {
                return Some(Place::Docked { side, group: g.id });
            }
            if dock.auto_hidden.iter().any(|t| t == id) {
                return Some(Place::AutoHidden { side });
            }
        }
        if let Some(f) = self.floating.iter().find(|f| f.group.contains(id)) {
            return Some(Place::Floating { group: f.group.id });
        }
        self.hidden
            .iter()
            .find(|h| h.id == id)
            .map(|h| Place::Hidden { side: h.side })
    }

    pub fn group(&self, gid: u32) -> Option<&Group> {
        DockSide::ALL
            .iter()
            .flat_map(|s| self.dock(*s).groups.iter())
            .chain(self.floating.iter().map(|f| &f.group))
            .find(|g| g.id == gid)
    }

    fn group_mut(&mut self, gid: u32) -> Option<&mut Group> {
        let DockLayout {
            left,
            right,
            bottom,
            floating,
            ..
        } = self;
        left.groups
            .iter_mut()
            .chain(right.groups.iter_mut())
            .chain(bottom.groups.iter_mut())
            .chain(floating.iter_mut().map(|f| &mut f.group))
            .find(|g| g.id == gid)
    }

    /// The group a docked or floating window is in.
    pub fn group_of(&self, id: &str) -> Option<&Group> {
        match self.find(id)? {
            Place::Docked { group, .. } | Place::Floating { group } => self.group(group),
            _ => None,
        }
    }

    /// The dock a window belongs to: where it is docked or auto-hidden, or
    /// where a floating or closed window returns.
    pub fn home_side(&self, id: &str) -> Option<DockSide> {
        Some(match self.find(id)? {
            Place::Docked { side, .. } | Place::AutoHidden { side } | Place::Hidden { side } => {
                side
            }
            Place::Floating { group } => self
                .floating
                .iter()
                .find(|f| f.group.id == group)
                .map(|f| f.home)?,
        })
    }

    /// Every tool window id in the layout, in display order.
    pub fn tool_window_ids(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for side in [DockSide::Left, DockSide::Right, DockSide::Bottom] {
            let d = self.dock(side);
            for g in &d.groups {
                out.extend(g.tabs.iter().map(String::as_str));
            }
            out.extend(d.auto_hidden.iter().map(String::as_str));
        }
        for f in &self.floating {
            out.extend(f.group.tabs.iter().map(String::as_str));
        }
        out.extend(self.hidden.iter().map(|h| h.id.as_str()));
        out
    }

    pub fn info(&self, id: &str, registry: &ToolWindowRegistry) -> Option<ToolWindowInfo> {
        let place = self.find(id)?;
        let group = self.group_of(id);
        Some(ToolWindowInfo {
            id: id.to_owned(),
            title: registry.title(id).to_owned(),
            place,
            group: group.map(|g| g.tabs.clone()),
            active: group.is_some_and(|g| g.active_id() == Some(id)),
        })
    }

    fn new_group(&mut self, id: &str) -> Group {
        let g = Group {
            id: self.next_group_id,
            tabs: vec![id.to_owned()],
            active: 0,
            share: None,
        };
        self.next_group_id += 1;
        g
    }

    fn place_new_group(&mut self, id: &str, side: DockSide) {
        let g = self.new_group(id);
        self.dock_mut(side).groups.push(g);
    }

    /// Detach a window from wherever it is; empty groups disappear. Returns its
    /// home side.
    fn detach(&mut self, id: &str) -> Option<DockSide> {
        let home = self.home_side(id)?;
        let fix = |g: &mut Group| {
            if let Some(ix) = g.tabs.iter().position(|t| t == id) {
                g.tabs.remove(ix);
                if g.active > ix || g.active >= g.tabs.len() {
                    g.active = g.active.saturating_sub(1);
                }
            }
        };
        for side in DockSide::ALL {
            let dock = self.dock_mut(side);
            dock.groups.iter_mut().for_each(fix);
            dock.groups.retain(|g| !g.tabs.is_empty());
            dock.auto_hidden.retain(|t| t != id);
        }
        self.floating.iter_mut().for_each(|f| fix(&mut f.group));
        self.floating.retain(|f| !f.group.tabs.is_empty());
        self.hidden.retain(|h| h.id != id);
        Some(home)
    }

    fn known(&self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            Some(_) => Ok(()),
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
        }
    }

    /// Dock as a new group at the end of `side` (a side docking guide).
    pub fn dock_to(&mut self, id: &str, side: DockSide) -> Result<(), LayoutError> {
        self.known(id)?;
        self.detach(id);
        self.place_new_group(id, side);
        Ok(())
    }

    /// Add `id` as the active tab of the group holding `target` (dropping on a
    /// group). A closed or auto-hidden target cannot take tabs.
    pub fn tab_into(&mut self, id: &str, target: &str) -> Result<(), LayoutError> {
        self.known(id)?;
        let gid = match self.find(target) {
            Some(Place::Docked { group, .. } | Place::Floating { group }) => group,
            Some(Place::AutoHidden { .. } | Place::Hidden { .. }) => {
                return Err(LayoutError::Closed(target.to_owned()));
            }
            None => return Err(LayoutError::UnknownWindow(target.to_owned())),
        };
        if id == target {
            return Ok(());
        }
        self.detach(id);
        // `target` is still in `gid`: only `id` was removed.
        let g = self.group_mut(gid).expect("target group survives");
        g.tabs.push(id.to_owned());
        g.active = g.tabs.len() - 1;
        Ok(())
    }

    /// Float `id` in a new OS window. Returns the floating group's id.
    pub fn float(&mut self, id: &str, bounds: Bounds) -> Result<u32, LayoutError> {
        self.known(id)?;
        let home = self.detach(id).expect("known");
        let group = self.new_group(id);
        let gid = group.id;
        self.floating.push(FloatingGroup {
            group,
            bounds,
            home,
        });
        Ok(gid)
    }

    /// Return a window to its dock: a floating group goes back whole to its
    /// home dock, an auto-hidden window is pinned, a closed one reopens. A
    /// docked window stays put.
    pub fn dock_home(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::Docked { .. }) => Ok(()),
            Some(Place::Floating { group }) => {
                let ix = self
                    .floating
                    .iter()
                    .position(|f| f.group.id == group)
                    .expect("found");
                let f = self.floating.remove(ix);
                self.dock_mut(f.home).groups.push(f.group);
                Ok(())
            }
            Some(Place::AutoHidden { .. }) => self.pin(id),
            Some(Place::Hidden { .. }) => self.show(id),
        }
    }

    /// Auto-hide a docked window to its dock's strip. Floating windows cannot
    /// be auto-hidden (as in VS); a closed window must be shown first.
    pub fn auto_hide(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::AutoHidden { .. }) => Ok(()),
            Some(Place::Floating { .. }) => Err(LayoutError::CannotAutoHideFloating(id.into())),
            Some(Place::Hidden { .. }) => Err(LayoutError::Closed(id.into())),
            Some(Place::Docked { side, .. }) => {
                self.detach(id);
                self.dock_mut(side).auto_hidden.push(id.to_owned());
                Ok(())
            }
        }
    }

    /// Pin an auto-hidden window back into its dock: into the dock's first
    /// group, or a new one. A window that is not auto-hidden is left alone.
    pub fn pin(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::AutoHidden { side }) => {
                match self.dock(side).groups.first().and_then(|g| g.active_id()) {
                    Some(target) => {
                        let target = target.to_owned();
                        self.tab_into(id, &target)?;
                    }
                    None => {
                        self.detach(id);
                        self.place_new_group(id, side);
                    }
                }
                Ok(())
            }
            Some(_) => Ok(()),
        }
    }

    /// Close a window. It remembers its dock and reopens there.
    pub fn hide(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::Hidden { .. }) => Ok(()),
            Some(_) => {
                let side = self.detach(id).expect("known");
                self.hidden.push(HiddenWindow {
                    id: id.to_owned(),
                    side,
                });
                Ok(())
            }
        }
    }

    /// Make a window visible and active: a closed one reopens as a tab of the
    /// first group of its dock (or a new group); a tabbed one becomes the
    /// active tab. An auto-hidden window stays on its strip (the view slides it
    /// out).
    pub fn show(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::Hidden { side }) => {
                self.hidden.retain(|h| h.id != id);
                match self.dock(side).groups.first().and_then(|g| g.active_id()) {
                    Some(target) => {
                        let target = target.to_owned();
                        // Put it back in the layout first so tab_into knows it.
                        self.place_new_group(id, side);
                        self.tab_into(id, &target)?;
                    }
                    None => self.place_new_group(id, side),
                }
                Ok(())
            }
            Some(_) => self.activate(id),
        }
    }

    /// Make `id` the visible tab of its group. No-op for auto-hidden windows.
    pub fn activate(&mut self, id: &str) -> Result<(), LayoutError> {
        match self.find(id) {
            None => Err(LayoutError::UnknownWindow(id.to_owned())),
            Some(Place::Docked { group, .. } | Place::Floating { group }) => {
                let g = self.group_mut(group).expect("found");
                g.active = g.tabs.iter().position(|t| t == id).expect("found");
                Ok(())
            }
            Some(Place::AutoHidden { .. }) => Ok(()),
            Some(Place::Hidden { .. }) => Err(LayoutError::Closed(id.to_owned())),
        }
    }

    /// Repair a loaded layout against the registered windows: drop unknown and
    /// duplicate ids and empty groups, clamp active tabs, add registered windows
    /// that are missing as closed at their default side, and keep group ids
    /// unique.
    pub fn normalize(&mut self, registry: &ToolWindowRegistry) {
        self.version = LAYOUT_SCHEMA_VERSION;
        let mut seen: Vec<String> = Vec::new();
        let mut keep = |id: &String| {
            if registry.contains(id) && !seen.contains(id) {
                seen.push(id.clone());
                true
            } else {
                false
            }
        };
        let mut gids: Vec<u32> = Vec::new();
        let mut fix_group = |g: &mut Group, keep: &mut dyn FnMut(&String) -> bool| {
            let active = g.active_id().map(str::to_owned);
            g.tabs.retain(|t| keep(t));
            g.active = active
                .and_then(|a| g.tabs.iter().position(|t| *t == a))
                .unwrap_or(0);
            gids.push(g.id);
        };
        for side in DockSide::ALL {
            let dock = self.dock_mut(side);
            for g in &mut dock.groups {
                fix_group(g, &mut keep);
            }
            dock.groups.retain(|g| !g.tabs.is_empty());
            dock.auto_hidden.retain(|t| keep(t));
            if !dock.size.is_finite() || dock.size < MIN_DOCK_SIZE {
                dock.size = 200.;
            }
            for g in &mut dock.groups {
                if g.share.is_some_and(|s| !s.is_finite() || s <= 0.) {
                    g.share = None;
                }
            }
        }
        for f in &mut self.floating {
            fix_group(&mut f.group, &mut keep);
        }
        self.floating.retain(|f| !f.group.tabs.is_empty());
        self.hidden.retain(|h| keep(&h.id));
        for w in registry.iter() {
            if !seen.contains(&w.id) {
                seen.push(w.id.clone());
                self.hidden.push(HiddenWindow {
                    id: w.id.clone(),
                    side: w.default_side,
                });
            }
        }
        // Unique group ids, and next_group_id past all of them.
        gids.sort_unstable();
        let duplicated = gids.windows(2).any(|w| w[0] == w[1]);
        if duplicated {
            let mut next = 1;
            for side in DockSide::ALL {
                for g in &mut self.dock_mut(side).groups {
                    g.id = next;
                    next += 1;
                }
            }
            for f in &mut self.floating {
                f.group.id = next;
                next += 1;
            }
            self.next_group_id = next;
        } else {
            let max = gids.last().copied().unwrap_or(0);
            self.next_group_id = self.next_group_id.max(max + 1);
        }
        let docs = &mut self.documents;
        if docs
            .active
            .as_deref()
            .is_some_and(|a| docs.get(a).is_none())
        {
            docs.active = docs.tabs.first().map(|t| t.id.clone());
        }
        docs.reorder();
    }

    /// Every registered window appears exactly once, groups are non-empty, and
    /// active indexes are in range.
    pub fn is_consistent(&self, registry: &ToolWindowRegistry) -> bool {
        let groups_ok = DockSide::ALL
            .iter()
            .flat_map(|s| self.dock(*s).groups.iter())
            .chain(self.floating.iter().map(|f| &f.group))
            .all(|g| !g.tabs.is_empty() && g.active < g.tabs.len());
        let mut seen = self.tool_window_ids();
        seen.sort_unstable();
        let mut all: Vec<&str> = registry.iter().map(|w| w.id.as_str()).collect();
        all.sort_unstable();
        groups_ok && seen == all
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("layout serializes")
    }

    /// Parse a layout file, migrating older versions. Does not normalize.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let value = migrate(value)?;
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// Bring a layout document of any supported version up to
/// [`LAYOUT_SCHEMA_VERSION`], one step per version, oldest first:
/// - 1 to 2: the Solution Explorer window became Workspace.
/// - 2 to 3 (brief 0020): the Debug Console window is retired; the program's output
///   is the Output window's Debug source. Where the Debug Console was placed and
///   Output was not, Output takes its place; otherwise the Debug Console is removed.
/// - 3 to 4 (brief 0041): the Terminal window joins the bottom group of Output (or of the Error List) after it, as
///   the default layout has it; a layout with neither in a bottom group gets it closed at the bottom.
/// - 4 to 5 (brief 0042): Find Results 1 and Find Results 2 join the closed windows, at the bottom (a search shows
///   them there), unless placed already.
pub fn migrate(mut value: Value) -> Result<Value, String> {
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or("layout has no numeric `version`")?;
    if version == 0 || version > u64::from(LAYOUT_SCHEMA_VERSION) {
        return Err(format!(
            "unsupported layout version {version} (this build reads up to {LAYOUT_SCHEMA_VERSION})"
        ));
    }
    // Migration steps go here, oldest first.
    if version < 2 {
        // Version 2 renamed the Solution Explorer window to Workspace (PLAN.md section 8).
        rename_id(&mut value, "solution_explorer", "workspace");
    }
    if version < 3 {
        retire_id(&mut value, RETIRED_DEBUG_CONSOLE, ids::OUTPUT);
    }
    if version < 4 {
        add_terminal(&mut value);
    }
    if version < 5 {
        for id in [ids::FIND_RESULTS_1, ids::FIND_RESULTS_2] {
            add_closed(&mut value, id, DockSide::Bottom);
        }
    }
    value["version"] = Value::from(LAYOUT_SCHEMA_VERSION);
    Ok(value)
}

/// The Debug Console's id in layouts of version 2 and older.
pub const RETIRED_DEBUG_CONSOLE: &str = "debug_console";

/// Version 4: put the Terminal window after Output (or the Error List) in its bottom group, unless placed already.
fn add_terminal(value: &mut Value) {
    if placed(value, ids::TERMINAL) {
        return;
    }
    let Some(groups) = value["bottom"]["groups"].as_array_mut() else {
        return;
    };
    for neighbor in [ids::OUTPUT, ids::ERROR_LIST] {
        for g in groups.iter_mut() {
            let Some(tabs) = g["tabs"].as_array_mut() else {
                continue;
            };
            if let Some(ix) = tabs.iter().position(|t| t == neighbor) {
                tabs.insert(ix + 1, Value::from(ids::TERMINAL));
                // The active tab stays the same window.
                if let Some(active) = g["active"].as_u64()
                    && active > ix as u64
                {
                    g["active"] = Value::from(active + 1);
                }
                if let Some(hidden) = value["hidden"].as_array_mut() {
                    hidden.retain(|h| h["id"] != ids::TERMINAL);
                }
                return;
            }
        }
    }
}

/// Version 5: tool window `id` joins the closed windows on `side`, unless it is placed or closed already.
fn add_closed(value: &mut Value, id: &str, side: DockSide) {
    if placed(value, id) {
        return;
    }
    let Some(hidden) = value["hidden"].as_array_mut() else {
        return;
    };
    if hidden.iter().any(|h| h["id"] == id) {
        return;
    }
    hidden.push(serde_json::json!({"id": id, "side": side.name()}));
}

/// Whether tool window `id` is placed (docked, auto-hidden or floating) in a layout document.
fn placed(value: &Value, id: &str) -> bool {
    let has = |v: &Value| v.as_array().is_some_and(|a| a.iter().any(|t| t == id));
    let docks = ["left", "right", "bottom"].iter().any(|side| {
        let dock = &value[*side];
        has(&dock["auto_hidden"])
            || dock["groups"]
                .as_array()
                .is_some_and(|gs| gs.iter().any(|g| has(&g["tabs"])))
    });
    docks
        || value["floating"]
            .as_array()
            .is_some_and(|fs| fs.iter().any(|f| has(&f["group"]["tabs"])))
}

/// Retire tool window `old`: when `successor` is not placed, it takes `old`'s place (a rename); otherwise `old` is
/// removed from every tab list, auto-hide strip and the closed list, keeping each group's active tab.
fn retire_id(value: &mut Value, old: &str, successor: &str) {
    if !placed(value, successor) {
        if let Some(hidden) = value["hidden"].as_array_mut() {
            // A closed successor reopens where the retired window was.
            hidden.retain(|h| h["id"] != successor);
        }
        rename_id(value, old, successor);
        return;
    }
    let fix_group = |g: &mut Value| {
        let Some(tabs) = g["tabs"].as_array_mut() else {
            return;
        };
        let Some(ix) = tabs.iter().position(|t| t == old) else {
            return;
        };
        tabs.remove(ix);
        if let Some(active) = g["active"].as_u64() {
            let ix = ix as u64;
            if active > ix || (active == ix && active > 0) {
                g["active"] = Value::from(active - 1);
            }
        }
    };
    for side in ["left", "right", "bottom"] {
        let dock = &mut value[side];
        if let Some(gs) = dock["groups"].as_array_mut() {
            gs.iter_mut().for_each(fix_group);
        }
        if let Some(a) = dock["auto_hidden"].as_array_mut() {
            a.retain(|t| t != old);
        }
    }
    if let Some(fs) = value["floating"].as_array_mut() {
        for f in fs.iter_mut() {
            fix_group(&mut f["group"]);
        }
    }
    if let Some(hidden) = value["hidden"].as_array_mut() {
        hidden.retain(|h| h["id"] != old);
    }
}

/// Replace every string equal to `from` anywhere in `value` with `to`. Tool window ids
/// appear as strings in tab lists, auto-hide strips, active fields and floating windows.
fn rename_id(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::String(s) if s == from => *s = to.to_owned(),
        Value::Array(items) => items.iter_mut().for_each(|v| rename_id(v, from, to)),
        Value::Object(map) => map.values_mut().for_each(|v| rename_id(v, from, to)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> ToolWindowRegistry {
        ToolWindowRegistry::vs_default()
    }

    fn docked_side(l: &DockLayout, id: &str) -> Option<DockSide> {
        match l.find(id)? {
            Place::Docked { side, .. } => Some(side),
            _ => None,
        }
    }

    #[test]
    fn version_1_layouts_rename_solution_explorer_to_workspace() {
        // Take the real current shape, mark it version 1 and give it the old id.
        let r = reg();
        let mut v: Value = serde_json::from_str(&DockLayout::fixture(&r).to_json()).unwrap();
        v["version"] = Value::from(1);
        rename_id(&mut v, ids::WORKSPACE, "solution_explorer");
        assert!(
            serde_json::to_string(&v)
                .unwrap()
                .contains("solution_explorer")
        );
        let migrated = migrate(v).unwrap();
        assert_eq!(migrated["version"], LAYOUT_SCHEMA_VERSION);
        let text = serde_json::to_string(&migrated).unwrap();
        assert!(!text.contains("solution_explorer"));
        let mut layout = DockLayout::from_json(&text).unwrap();
        layout.normalize(&r);
        assert!(layout.find(ids::WORKSPACE).is_some());
        assert!(layout.find("solution_explorer").is_none());
    }

    /// A version 2 layout as brief 0018 saved it after a debugging session: the Debug Console tabbed with Call Stack
    /// and Breakpoints in a second bottom group, active there.
    const V2_DEBUG_LAYOUT: &str = r#"{
      "version": 2,
      "left": {"groups": [], "size": 240.0, "auto_hidden": ["toolbox"]},
      "right": {"groups": [{"id": 1, "tabs": ["workspace", "git_changes", "agents"], "active": 0},
                           {"id": 2, "tabs": ["properties"], "active": 0}],
                "size": 300.0, "auto_hidden": []},
      "bottom": {"groups": [{"id": 3, "tabs": ["error_list", "output", "locals", "watch"], "active": 2},
                            {"id": 4, "tabs": ["call_stack", "breakpoints", "debug_console"], "active": 2}],
                 "size": 200.0, "auto_hidden": []},
      "floating": [],
      "hidden": [{"id": "find_all_references", "side": "bottom"}, {"id": "threads", "side": "bottom"},
                 {"id": "exception_settings", "side": "bottom"}],
      "documents": {"tabs": [], "active": null},
      "next_group_id": 5
    }"#;

    #[test]
    fn resizing_clamps_and_keeps_shares_summing_to_one() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        assert_eq!(l.resize_dock(DockSide::Right, 500.), 500.);
        assert_eq!(l.right.size, 500.);
        assert_eq!(l.resize_dock(DockSide::Right, 10.), MIN_DOCK_SIZE);
        let size = l.resize_dock(DockSide::Bottom, f32::NAN);
        assert_eq!(size, l.bottom.size, "NaN leaves the size alone");

        assert_eq!(l.right.shares(), [0.5, 0.5]);
        let (side, shares) = l.resize_group(ids::PROPERTIES, 0.25).unwrap();
        assert_eq!(side, DockSide::Right);
        assert_eq!(shares, [0.75, 0.25]);
        let (_, shares) = l.resize_group(ids::WORKSPACE, 0.99).unwrap();
        assert!(
            (shares[0] - (1. - MIN_GROUP_SHARE)).abs() < 1e-5,
            "{shares:?}"
        );
        assert!((shares[1] - MIN_GROUP_SHARE).abs() < 1e-5, "{shares:?}");
        // A lone group on an edge is always the whole edge.
        let (_, shares) = l.resize_group(ids::OUTPUT, 0.3).unwrap();
        assert_eq!(shares, [1.]);
        assert_eq!(
            l.resize_group(ids::TOOLBOX, 0.5),
            Err(LayoutError::NotDocked(ids::TOOLBOX.into()))
        );
        assert_eq!(
            l.resize_group("nope", 0.5),
            Err(LayoutError::UnknownWindow("nope".into()))
        );
        // Shares round-trip through JSON; absent ones stay absent.
        let text = l.to_json();
        let back = DockLayout::from_json(&text).unwrap();
        assert!((back.right.shares()[1] - MIN_GROUP_SHARE).abs() < 1e-5);
        assert!(back.bottom.groups[0].share.is_none());
        assert!(!text.contains("\"share\":null"));
        // normalize drops a share a hand-edited file broke.
        let mut broken = back.clone();
        broken.right.groups[0].share = Some(-1.);
        broken.normalize(&r);
        assert!(broken.right.groups[0].share.is_none());
    }

    #[test]
    fn version_2_layouts_retire_the_debug_console() {
        let r = reg();
        assert!(!r.contains(RETIRED_DEBUG_CONSOLE));
        let v: Value = serde_json::from_str(V2_DEBUG_LAYOUT).unwrap();
        let migrated = migrate(v).unwrap();
        assert_eq!(migrated["version"], LAYOUT_SCHEMA_VERSION);
        let text = serde_json::to_string(&migrated).unwrap();
        assert!(!text.contains(RETIRED_DEBUG_CONSOLE), "{text}");
        // Output was placed already: the Debug Console is removed and its group's active tab moves to its neighbor.
        assert_eq!(
            migrated["bottom"]["groups"][1],
            serde_json::json!({"id": 4, "tabs": ["call_stack", "breakpoints"], "active": 1})
        );
        // Version 4 puts the Terminal after Output.
        assert_eq!(
            migrated["bottom"]["groups"][0]["tabs"],
            serde_json::json!(["error_list", "output", "terminal", "locals", "watch"])
        );
        let mut layout = DockLayout::from_json(&text).unwrap();
        layout.normalize(&r);
        assert!(layout.is_consistent(&r));
        assert_eq!(layout.bottom.groups[0].active_id(), Some(ids::LOCALS));
        assert_eq!(layout.bottom.groups[1].active_id(), Some(ids::BREAKPOINTS));

        // A layout where Output was closed: Output takes the Debug Console's place, and is no longer closed.
        let mut v: Value = serde_json::from_str(V2_DEBUG_LAYOUT).unwrap();
        v["bottom"]["groups"][0]["tabs"] = serde_json::json!(["error_list", "locals", "watch"]);
        v["bottom"]["groups"][0]["active"] = Value::from(0);
        v["hidden"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"id": "output", "side": "bottom"}));
        let migrated = migrate(v).unwrap();
        assert_eq!(
            migrated["bottom"]["groups"][1]["tabs"],
            serde_json::json!(["call_stack", "breakpoints", "output", "terminal"])
        );
        let mut layout = DockLayout::from_json(&serde_json::to_string(&migrated).unwrap()).unwrap();
        layout.normalize(&r);
        assert!(layout.is_consistent(&r));
        assert_eq!(layout.bottom.groups[1].active_id(), Some(ids::OUTPUT));
        assert!(layout.hidden.iter().all(|h| h.id != ids::OUTPUT));

        // Auto-hidden and floating Debug Consoles go too; a layout from a newer build is still refused.
        let mut v: Value = serde_json::from_str(V2_DEBUG_LAYOUT).unwrap();
        v["left"]["auto_hidden"] = serde_json::json!(["toolbox", "debug_console"]);
        v["bottom"]["groups"][1]["tabs"] = serde_json::json!(["call_stack", "breakpoints"]);
        v["bottom"]["groups"][1]["active"] = Value::from(0);
        let migrated = migrate(v).unwrap();
        assert_eq!(
            migrated["left"]["auto_hidden"],
            serde_json::json!(["toolbox"])
        );
        assert!(migrate(serde_json::json!({"version": LAYOUT_SCHEMA_VERSION + 1})).is_err());
    }

    #[test]
    fn default_vs_layout() {
        let r = reg();
        let l = DockLayout::default_vs(&r);
        assert!(l.is_consistent(&r));
        assert!(l.left.groups.is_empty());
        // Properties and Toolbox start closed until they have content.
        assert!(l.left.auto_hidden.is_empty());
        assert_eq!(l.right.groups.len(), 1);
        assert_eq!(
            l.right.groups[0].tabs,
            [ids::WORKSPACE, ids::GIT_CHANGES, ids::AGENTS]
        );
        assert_eq!(l.right.groups[0].active_id(), Some(ids::WORKSPACE));
        assert_eq!(
            l.bottom.groups[0].tabs,
            [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL]
        );
        assert_eq!(l.bottom.groups[0].active_id(), Some(ids::ERROR_LIST));
        // Find All References starts closed and opens beside the Error List (brief 0014), as do the debugger
        // windows (brief 0018).
        let hidden: Vec<&str> = l.hidden.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            hidden,
            [
                ids::PROPERTIES,
                ids::TOOLBOX,
                ids::FIND_ALL_REFERENCES,
                ids::LOCALS,
                ids::WATCH,
                ids::CALL_STACK,
                ids::THREADS,
                ids::BREAKPOINTS,
                ids::EXCEPTION_SETTINGS,
                ids::TEST_EXPLORER,
                ids::GIT_REPOSITORY,
                ids::FIND_RESULTS_1,
                ids::FIND_RESULTS_2,
                ids::PULL_REQUESTS,
                ids::ISSUES
            ]
        );
        // The Test Explorer (brief 0035) and the Toolbox open docked left, Properties and the Pull Requests and
        // Issues windows (brief 0046) right; the others at the bottom.
        assert!(l.hidden.iter().all(|h| h.side == DockSide::Bottom
            || (h.side == DockSide::Left
                && [ids::TEST_EXPLORER, ids::TOOLBOX].contains(&h.id.as_str()))
            || (h.side == DockSide::Right
                && [ids::PROPERTIES, ids::PULL_REQUESTS, ids::ISSUES].contains(&h.id.as_str()))));
        assert!(l.floating.is_empty());
        // View > Properties Window opens it on the right.
        let mut props = l.clone();
        props.show(ids::PROPERTIES).unwrap();
        assert!(
            props
                .right
                .groups
                .iter()
                .any(|g| g.tabs.iter().any(|t| t == ids::PROPERTIES))
        );
        let mut shown = l.clone();
        shown.show(ids::FIND_ALL_REFERENCES).unwrap();
        assert_eq!(
            shown.bottom.groups[0].tabs,
            [
                ids::ERROR_LIST,
                ids::OUTPUT,
                ids::TERMINAL,
                ids::FIND_ALL_REFERENCES
            ]
        );
        assert_eq!(l.documents.active_tab().unwrap().title, "Welcome");
    }

    #[test]
    fn dock_to_each_side() {
        let r = reg();
        for side in DockSide::ALL {
            let mut l = DockLayout::fixture(&r);
            l.dock_to(ids::PROPERTIES, side).unwrap();
            assert_eq!(docked_side(&l, ids::PROPERTIES), Some(side));
            assert_eq!(
                l.dock(side).groups.last().unwrap().tabs,
                [ids::PROPERTIES],
                "{side:?}"
            );
            assert!(l.is_consistent(&r));
        }
        let mut l = DockLayout::fixture(&r);
        assert_eq!(
            l.dock_to("nope", DockSide::Left),
            Err(LayoutError::UnknownWindow("nope".into()))
        );
    }

    #[test]
    fn tab_and_untab() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        l.tab_into(ids::PROPERTIES, ids::OUTPUT).unwrap();
        assert_eq!(l.right.groups.len(), 1, "Properties' group disappears");
        let g = l.group_of(ids::OUTPUT).unwrap();
        assert_eq!(
            g.tabs,
            [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL, ids::PROPERTIES]
        );
        assert_eq!(g.active_id(), Some(ids::PROPERTIES));
        // Untab: drag it out to a side.
        l.dock_to(ids::PROPERTIES, DockSide::Left).unwrap();
        assert_eq!(
            l.group_of(ids::OUTPUT).unwrap().tabs,
            [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL]
        );
        // The active tab moves to its neighbor (the Terminal, after Output).
        assert_eq!(l.group_of(ids::OUTPUT).unwrap().active, 2);
        // Dropping on itself does nothing.
        l.tab_into(ids::PROPERTIES, ids::PROPERTIES).unwrap();
        assert_eq!(docked_side(&l, ids::PROPERTIES), Some(DockSide::Left));
        // Cannot tab into a closed or auto-hidden window's (non-)group.
        assert_eq!(
            l.tab_into(ids::OUTPUT, ids::TOOLBOX),
            Err(LayoutError::Closed(ids::TOOLBOX.into()))
        );
        assert!(l.is_consistent(&r));
    }

    #[test]
    fn float_and_redock() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        // The Error List and Output alone at the bottom (the Terminal closed).
        l.hide(ids::TERMINAL).unwrap();
        let gid = l.float(ids::OUTPUT, Bounds::DEFAULT_FLOAT).unwrap();
        assert_eq!(l.find(ids::OUTPUT), Some(Place::Floating { group: gid }));
        assert_eq!(l.floating[0].home, DockSide::Bottom);
        assert_eq!(l.bottom.groups[0].tabs, [ids::ERROR_LIST]);
        // Tab Error List into the floating group, then re-dock the whole group.
        l.tab_into(ids::ERROR_LIST, ids::OUTPUT).unwrap();
        assert!(l.bottom.groups.is_empty());
        assert_eq!(
            l.auto_hide(ids::OUTPUT),
            Err(LayoutError::CannotAutoHideFloating(ids::OUTPUT.into()))
        );
        l.dock_home(ids::ERROR_LIST).unwrap();
        assert!(l.floating.is_empty());
        assert_eq!(l.bottom.groups[0].tabs, [ids::OUTPUT, ids::ERROR_LIST]);
        assert!(l.is_consistent(&r));
    }

    #[test]
    fn auto_hide_and_pin() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        l.auto_hide(ids::WORKSPACE).unwrap();
        assert_eq!(
            l.find(ids::WORKSPACE),
            Some(Place::AutoHidden {
                side: DockSide::Right
            })
        );
        assert_eq!(l.right.groups[0].tabs, [ids::GIT_CHANGES, ids::AGENTS]);
        l.pin(ids::WORKSPACE).unwrap();
        assert_eq!(docked_side(&l, ids::WORKSPACE), Some(DockSide::Right));
        assert_eq!(
            l.right.groups[0].tabs,
            [ids::GIT_CHANGES, ids::AGENTS, ids::WORKSPACE]
        );
        // Toolbox: left dock has no groups, so pinning makes one.
        l.dock_home(ids::TOOLBOX).unwrap();
        assert_eq!(l.left.groups[0].tabs, [ids::TOOLBOX]);
        assert!(l.is_consistent(&r));
    }

    #[test]
    fn hide_and_show_return_home() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        l.hide(ids::OUTPUT).unwrap();
        assert_eq!(
            l.find(ids::OUTPUT),
            Some(Place::Hidden {
                side: DockSide::Bottom
            })
        );
        assert_eq!(
            l.activate(ids::OUTPUT),
            Err(LayoutError::Closed(ids::OUTPUT.into()))
        );
        l.show(ids::OUTPUT).unwrap();
        let g = l.group_of(ids::OUTPUT).unwrap();
        assert_eq!(g.tabs, [ids::ERROR_LIST, ids::TERMINAL, ids::OUTPUT]);
        assert_eq!(g.active_id(), Some(ids::OUTPUT));
        // Hiding a floating window remembers its home dock.
        l.float(ids::PROPERTIES, Bounds::DEFAULT_FLOAT).unwrap();
        l.hide(ids::PROPERTIES).unwrap();
        assert!(l.floating.is_empty());
        l.show(ids::PROPERTIES).unwrap();
        assert_eq!(docked_side(&l, ids::PROPERTIES), Some(DockSide::Right));
        // Hiding the only window of the left dock, then showing it.
        l.dock_home(ids::TOOLBOX).unwrap();
        l.hide(ids::TOOLBOX).unwrap();
        assert!(l.left.groups.is_empty());
        l.show(ids::TOOLBOX).unwrap();
        assert_eq!(l.left.groups[0].tabs, [ids::TOOLBOX]);
        assert!(l.is_consistent(&r));
    }

    #[test]
    fn info_reports_state() {
        let r = reg();
        let l = DockLayout::fixture(&r);
        let i = l.info(ids::OUTPUT, &r).unwrap();
        assert_eq!(i.title, "Output");
        assert!(matches!(
            i.place,
            Place::Docked {
                side: DockSide::Bottom,
                ..
            }
        ));
        assert_eq!(
            i.group.unwrap(),
            [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL]
        );
        assert!(!i.active);
        let t = l.info(ids::TOOLBOX, &r).unwrap();
        assert_eq!(
            t.place,
            Place::AutoHidden {
                side: DockSide::Left
            }
        );
        assert!(t.group.is_none());
        assert!(l.info("nope", &r).is_none());
    }

    #[test]
    fn documents_pinned_and_preview() {
        let mut d = DocumentArea::default();
        d.open("a", "A");
        d.open("b", "B");
        d.open_preview("p1", "P1");
        assert_eq!(d.active.as_deref(), Some("p1"));
        d.open_preview("p2", "P2");
        let ids: Vec<_> = d.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "p2"], "a new preview replaces the old one");
        assert!(d.get("p2").unwrap().preview);
        d.set_pinned("b", true);
        let ids: Vec<_> = d.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "p2"], "pinned tabs come first");
        d.open("p2", "P2");
        assert!(
            !d.get("p2").unwrap().preview,
            "opening the preview promotes it"
        );
        d.open_preview("p3", "P3");
        assert_eq!(d.tabs.len(), 4);
        assert!(d.close("p3"));
        assert_eq!(d.active.as_deref(), Some("p2"));
        assert!(d.activate("a") && !d.activate("zz"));
    }

    #[test]
    fn json_roundtrip_and_versions() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        l.float(
            ids::OUTPUT,
            Bounds {
                x: 1.,
                y: 2.,
                width: 3.,
                height: 4.,
            },
        )
        .unwrap();
        l.auto_hide(ids::PROPERTIES).unwrap();
        l.hide(ids::GIT_CHANGES).unwrap();
        l.documents.open_preview("readme", "README.md");
        l.documents.set_pinned("welcome", true);
        let json = l.to_json();
        let back = DockLayout::from_json(&json).unwrap();
        assert_eq!(back, l);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["version"], LAYOUT_SCHEMA_VERSION);
        assert_eq!(v["left"]["auto_hidden"][0], ids::TOOLBOX);

        assert!(
            DockLayout::from_json("{\"version\": 99}")
                .unwrap_err()
                .contains("99")
        );
        assert!(DockLayout::from_json("{}").is_err());
        assert!(DockLayout::from_json("not json").is_err());
    }

    #[test]
    fn normalize_repairs_and_adds_new_windows() {
        let mut r = reg();
        let mut l = DockLayout::fixture(&r);
        // A layout written by a build that had a window this one does not,
        // duplicated an id, and lacked a window registered since.
        l.right.groups[0].tabs.push("class_view".into());
        l.bottom.auto_hidden.push(ids::OUTPUT.into());
        l.right.groups[1].active = 7;
        r.register(ToolWindowDescriptor::new(
            "test_explorer",
            "Test Explorer",
            DockSide::Left,
        ));
        l.normalize(&r);
        assert!(l.is_consistent(&r));
        assert!(l.find("class_view").is_none());
        assert!(l.bottom.auto_hidden.is_empty());
        assert_eq!(
            l.find("test_explorer"),
            Some(Place::Hidden {
                side: DockSide::Left
            })
        );
        assert_eq!(l.right.groups[1].active, 0);
        l.show("test_explorer").unwrap();
        assert_eq!(docked_side(&l, "test_explorer"), Some(DockSide::Left));
    }

    #[test]
    fn normalize_fixes_duplicate_group_ids() {
        let r = reg();
        let mut l = DockLayout::fixture(&r);
        let gid = l.right.groups[0].id;
        l.bottom.groups[0].id = gid;
        l.next_group_id = 1;
        l.normalize(&r);
        let mut ids: Vec<u32> = DockSide::ALL
            .iter()
            .flat_map(|s| l.dock(*s).groups.iter().map(|g| g.id))
            .collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(l.next_group_id > *ids.last().unwrap());
    }

    /// A version 3 layout as brief 0040 saved it: the Error List and Output at the bottom with Git Repository beside
    /// them, Output active.
    const V3_LAYOUT: &str = r#"{
      "version": 3,
      "left": {"groups": [], "size": 240.0, "auto_hidden": ["toolbox"]},
      "right": {"groups": [{"id": 1, "tabs": ["workspace", "git_changes", "agents"], "active": 0},
                           {"id": 2, "tabs": ["properties"], "active": 0}],
                "size": 300.0, "auto_hidden": []},
      "bottom": {"groups": [{"id": 3, "tabs": ["error_list", "output", "git_repository"], "active": 1}],
                 "size": 200.0, "auto_hidden": []},
      "floating": [],
      "hidden": [{"id": "find_all_references", "side": "bottom"}, {"id": "test_explorer", "side": "left"}],
      "documents": {"tabs": [], "active": null},
      "next_group_id": 4
    }"#;

    #[test]
    fn version_3_layouts_get_the_terminal_in_the_bottom_group() {
        let r = reg();
        let migrated = migrate(serde_json::from_str(V3_LAYOUT).unwrap()).unwrap();
        assert_eq!(migrated["version"], LAYOUT_SCHEMA_VERSION);
        assert_eq!(
            migrated["bottom"]["groups"][0]["tabs"],
            serde_json::json!(["error_list", "output", "terminal", "git_repository"])
        );
        let mut layout = DockLayout::from_json(&serde_json::to_string(&migrated).unwrap()).unwrap();
        layout.normalize(&r);
        assert!(layout.is_consistent(&r));
        assert_eq!(docked_side(&layout, ids::TERMINAL), Some(DockSide::Bottom));
        assert_eq!(
            layout.bottom.groups[0].active_id(),
            Some(ids::OUTPUT),
            "the active tab stays"
        );
        assert!(layout.hidden.iter().all(|h| h.id != ids::TERMINAL));

        // Output closed: after the Error List. Neither docked at the bottom: closed, at the bottom.
        let mut v: Value = serde_json::from_str(V3_LAYOUT).unwrap();
        v["bottom"]["groups"][0]["tabs"] = serde_json::json!(["error_list"]);
        v["bottom"]["groups"][0]["active"] = Value::from(0);
        let migrated = migrate(v).unwrap();
        assert_eq!(
            migrated["bottom"]["groups"][0]["tabs"],
            serde_json::json!(["error_list", "terminal"])
        );
        let mut v: Value = serde_json::from_str(V3_LAYOUT).unwrap();
        v["bottom"]["groups"] = serde_json::json!([]);
        let mut layout =
            DockLayout::from_json(&serde_json::to_string(&migrate(v).unwrap()).unwrap()).unwrap();
        layout.normalize(&r);
        assert!(layout.is_consistent(&r));
        assert!(
            layout
                .hidden
                .iter()
                .any(|h| h.id == ids::TERMINAL && h.side == DockSide::Bottom)
        );
        // The default layout has it beside Output.
        let d = DockLayout::fixture(&r);
        assert_eq!(
            d.bottom.groups[0].tabs,
            [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL]
        );
    }

    /// A version 4 layout as brief 0041 saved it: the Terminal beside Output, Find All References shown.
    const V4_LAYOUT: &str = r#"{
      "version": 4,
      "left": {"groups": [], "size": 240.0, "auto_hidden": ["toolbox"]},
      "right": {"groups": [{"id": 1, "tabs": ["workspace", "git_changes", "agents"], "active": 0},
                           {"id": 2, "tabs": ["properties"], "active": 0}],
                "size": 300.0, "auto_hidden": []},
      "bottom": {"groups": [{"id": 3, "tabs": ["error_list", "output", "terminal", "find_all_references"], "active": 2}],
                 "size": 200.0, "auto_hidden": []},
      "floating": [],
      "hidden": [{"id": "test_explorer", "side": "left"}, {"id": "git_repository", "side": "bottom"}],
      "documents": {"tabs": [], "active": null},
      "next_group_id": 4
    }"#;

    #[test]
    fn version_4_layouts_get_the_find_results_windows_closed_at_the_bottom() {
        let r = reg();
        let migrated = migrate(serde_json::from_str(V4_LAYOUT).unwrap()).unwrap();
        assert_eq!(migrated["version"], LAYOUT_SCHEMA_VERSION);
        assert_eq!(
            migrated["hidden"],
            serde_json::json!([
                {"id": "test_explorer", "side": "left"},
                {"id": "git_repository", "side": "bottom"},
                {"id": "find_results_1", "side": "bottom"},
                {"id": "find_results_2", "side": "bottom"}
            ])
        );
        // Nothing else moved.
        assert_eq!(
            migrated["bottom"]["groups"][0]["tabs"],
            serde_json::json!(["error_list", "output", "terminal", "find_all_references"])
        );
        let mut layout = DockLayout::from_json(&serde_json::to_string(&migrated).unwrap()).unwrap();
        layout.normalize(&r);
        assert!(layout.is_consistent(&r));
        assert_eq!(layout.bottom.groups[0].active_id(), Some(ids::TERMINAL));
        // A search shows Find Results 1 as a tab of the bottom group.
        layout.show(ids::FIND_RESULTS_1).unwrap();
        assert_eq!(
            docked_side(&layout, ids::FIND_RESULTS_1),
            Some(DockSide::Bottom)
        );
        assert!(
            layout.bottom.groups[0]
                .tabs
                .iter()
                .any(|t| t == ids::FIND_RESULTS_1)
        );
        // A layout that placed one already keeps it where it is (and the other still joins the closed ones).
        let mut v: Value = serde_json::from_str(V4_LAYOUT).unwrap();
        v["left"]["groups"] =
            serde_json::json!([{"id": 5, "tabs": ["find_results_2"], "active": 0}]);
        v["next_group_id"] = Value::from(6);
        let migrated = migrate(v).unwrap();
        assert_eq!(
            migrated["left"]["groups"][0]["tabs"],
            serde_json::json!(["find_results_2"])
        );
        let hidden = migrated["hidden"].as_array().unwrap();
        assert!(hidden.iter().any(|h| h["id"] == "find_results_1"));
        assert!(!hidden.iter().any(|h| h["id"] == "find_results_2"));
        // Version 3 layouts get the Terminal and the two windows.
        let v3 = migrate(serde_json::from_str(V3_LAYOUT).unwrap()).unwrap();
        let hidden = v3["hidden"].as_array().unwrap();
        assert!(hidden.iter().any(|h| h["id"] == "find_results_2"));
    }
}
