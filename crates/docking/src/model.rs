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
pub const LAYOUT_SCHEMA_VERSION: u32 = 2;

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
    pub const DEBUG_CONSOLE: &str = "debug_console";
    /// The windows a debugging session shows, in tab order (Visual Studio's Debug layout).
    pub const DEBUG_SESSION: [&str; 4] = [LOCALS, WATCH, CALL_STACK, DEBUG_CONSOLE];
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
            (ids::DEBUG_CONSOLE, "Debug Console", DockSide::Bottom),
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
}

impl Group {
    pub fn active_id(&self) -> Option<&str> {
        self.tabs.get(self.active).map(String::as_str)
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
}

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

    /// Visual Studio's default (PLAN.md 8): Workspace with Git Changes
    /// tabbed on the right above Properties, Error List and Output tabbed at the
    /// bottom, Toolbox auto-hidden on the left, a Welcome document. Any other
    /// registered window starts closed.
    pub fn default_vs(registry: &ToolWindowRegistry) -> Self {
        let mut l = Self::empty();
        let group = |id: u32, tabs: &[&str]| Group {
            id,
            tabs: tabs.iter().map(|t| (*t).to_owned()).collect(),
            active: 0,
        };
        l.right.groups = vec![
            group(1, &[ids::WORKSPACE, ids::GIT_CHANGES, ids::AGENTS]),
            group(2, &[ids::PROPERTIES]),
        ];
        l.bottom.groups = vec![group(3, &[ids::ERROR_LIST, ids::OUTPUT])];
        l.left.auto_hidden.push(ids::TOOLBOX.into());
        l.next_group_id = 4;
        l.documents.open("welcome", "Welcome");
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
            if !dock.size.is_finite() || dock.size < 40. {
                dock.size = 200.;
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
/// [`LAYOUT_SCHEMA_VERSION`]. Version 1 is the first; there are no steps yet.
/// A step looks like `if version == 1 { /* rewrite fields */ version = 2; }`.
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
    value["version"] = Value::from(LAYOUT_SCHEMA_VERSION);
    Ok(value)
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
        let mut v: Value = serde_json::from_str(&DockLayout::default_vs(&r).to_json()).unwrap();
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

    #[test]
    fn default_vs_layout() {
        let r = reg();
        let l = DockLayout::default_vs(&r);
        assert!(l.is_consistent(&r));
        assert!(l.left.groups.is_empty());
        assert_eq!(l.left.auto_hidden, [ids::TOOLBOX]);
        assert_eq!(l.right.groups.len(), 2);
        assert_eq!(
            l.right.groups[0].tabs,
            [ids::WORKSPACE, ids::GIT_CHANGES, ids::AGENTS]
        );
        assert_eq!(l.right.groups[0].active_id(), Some(ids::WORKSPACE));
        assert_eq!(l.right.groups[1].tabs, [ids::PROPERTIES]);
        assert_eq!(l.bottom.groups[0].tabs, [ids::ERROR_LIST, ids::OUTPUT]);
        assert_eq!(l.bottom.groups[0].active_id(), Some(ids::ERROR_LIST));
        // Find All References starts closed and opens beside the Error List (brief 0014), as do the debugger
        // windows (brief 0018).
        let hidden: Vec<&str> = l.hidden.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            hidden,
            [
                ids::FIND_ALL_REFERENCES,
                ids::LOCALS,
                ids::WATCH,
                ids::CALL_STACK,
                ids::THREADS,
                ids::BREAKPOINTS,
                ids::EXCEPTION_SETTINGS,
                ids::DEBUG_CONSOLE
            ]
        );
        assert!(l.hidden.iter().all(|h| h.side == DockSide::Bottom));
        assert!(l.floating.is_empty());
        let mut shown = l.clone();
        shown.show(ids::FIND_ALL_REFERENCES).unwrap();
        assert_eq!(
            shown.bottom.groups[0].tabs,
            [ids::ERROR_LIST, ids::OUTPUT, ids::FIND_ALL_REFERENCES]
        );
        assert_eq!(l.documents.active_tab().unwrap().title, "Welcome");
    }

    #[test]
    fn dock_to_each_side() {
        let r = reg();
        for side in DockSide::ALL {
            let mut l = DockLayout::default_vs(&r);
            l.dock_to(ids::PROPERTIES, side).unwrap();
            assert_eq!(docked_side(&l, ids::PROPERTIES), Some(side));
            assert_eq!(
                l.dock(side).groups.last().unwrap().tabs,
                [ids::PROPERTIES],
                "{side:?}"
            );
            assert!(l.is_consistent(&r));
        }
        let mut l = DockLayout::default_vs(&r);
        assert_eq!(
            l.dock_to("nope", DockSide::Left),
            Err(LayoutError::UnknownWindow("nope".into()))
        );
    }

    #[test]
    fn tab_and_untab() {
        let r = reg();
        let mut l = DockLayout::default_vs(&r);
        l.tab_into(ids::PROPERTIES, ids::OUTPUT).unwrap();
        assert_eq!(l.right.groups.len(), 1, "Properties' group disappears");
        let g = l.group_of(ids::OUTPUT).unwrap();
        assert_eq!(g.tabs, [ids::ERROR_LIST, ids::OUTPUT, ids::PROPERTIES]);
        assert_eq!(g.active_id(), Some(ids::PROPERTIES));
        // Untab: drag it out to a side.
        l.dock_to(ids::PROPERTIES, DockSide::Left).unwrap();
        assert_eq!(
            l.group_of(ids::OUTPUT).unwrap().tabs,
            [ids::ERROR_LIST, ids::OUTPUT]
        );
        assert_eq!(l.group_of(ids::OUTPUT).unwrap().active, 1);
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
        let mut l = DockLayout::default_vs(&r);
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
        let mut l = DockLayout::default_vs(&r);
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
        let mut l = DockLayout::default_vs(&r);
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
        assert_eq!(g.tabs, [ids::ERROR_LIST, ids::OUTPUT]);
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
        let l = DockLayout::default_vs(&r);
        let i = l.info(ids::OUTPUT, &r).unwrap();
        assert_eq!(i.title, "Output");
        assert!(matches!(
            i.place,
            Place::Docked {
                side: DockSide::Bottom,
                ..
            }
        ));
        assert_eq!(i.group.unwrap(), [ids::ERROR_LIST, ids::OUTPUT]);
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
        let mut l = DockLayout::default_vs(&r);
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
        let mut l = DockLayout::default_vs(&r);
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
        let mut l = DockLayout::default_vs(&r);
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
}
