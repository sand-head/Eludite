//! Docking layout model: pure data plus operations, serialized as JSON.
//!
//! Visual Studio's model, reduced to what brief 0001 asks for: three dock
//! sides holding tab groups of tool windows, floating groups (each in its own
//! OS window), and per-side auto-hidden windows.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Bottom];
}

/// Tool windows known to the spike: (id, title).
pub const TOOL_WINDOWS: [(&str, &str); 6] = [
    ("solution_explorer", "Solution Explorer"),
    ("properties", "Properties"),
    ("error_list", "Error List"),
    ("output", "Output"),
    ("toolbox", "Toolbox"),
    ("team_explorer", "Git Changes"),
];

pub fn title(id: &str) -> &str {
    TOOL_WINDOWS
        .iter()
        .find(|(i, _)| *i == id)
        .map_or(id, |(_, t)| t)
}

/// A set of tool windows shown one at a time behind a tab strip.
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    /// Groups stacked along the dock (top to bottom for left/right, left to right for bottom).
    pub groups: Vec<Group>,
    /// Width (left/right) or height (bottom) in logical pixels.
    pub size: f32,
    /// Auto-hidden tool windows, shown as tabs on this edge.
    pub auto_hidden: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    pub group: Group,
    /// x, y, width, height in logical pixels. The position is advisory: Wayland
    /// does not let clients place top-level windows.
    pub bounds: [f32; 4],
    /// Where "Dock" sends the group back.
    pub home: Side,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub version: u32,
    pub left: Dock,
    pub right: Dock,
    pub bottom: Dock,
    pub floating: Vec<Floating>,
    pub next_group_id: u32,
}

/// Where a tool window currently lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Docked { side: Side, group: u32 },
    Floating { group: u32 },
    AutoHidden { side: Side },
}

impl Layout {
    pub fn empty() -> Self {
        let dock = |size| Dock {
            groups: Vec::new(),
            size,
            auto_hidden: Vec::new(),
        };
        Self {
            version: LAYOUT_VERSION,
            left: dock(240.),
            right: dock(300.),
            bottom: dock(220.),
            floating: Vec::new(),
            next_group_id: 1,
        }
    }

    /// VS default: Solution Explorer over Properties on the right, Error List
    /// and Output tabbed at the bottom, Toolbox auto-hidden on the left.
    pub fn default_vs() -> Self {
        let mut l = Self::empty();
        l.dock_to("solution_explorer", Side::Right);
        l.dock_to("properties", Side::Right);
        l.dock_to("error_list", Side::Bottom);
        let g = l.right.groups[0].id;
        l.tab_into("team_explorer", g);
        l.activate("solution_explorer");
        let g = l.bottom.groups[0].id;
        l.tab_into("output", g);
        l.activate("error_list");
        l.left.auto_hidden.push("toolbox".into());
        l
    }

    pub fn dock(&self, side: Side) -> &Dock {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
            Side::Bottom => &self.bottom,
        }
    }

    pub fn dock_mut(&mut self, side: Side) -> &mut Dock {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
            Side::Bottom => &mut self.bottom,
        }
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

    pub fn find(&self, id: &str) -> Option<Place> {
        for side in Side::ALL {
            let dock = self.dock(side);
            if let Some(g) = dock.groups.iter().find(|g| g.tabs.iter().any(|t| t == id)) {
                return Some(Place::Docked { side, group: g.id });
            }
            if dock.auto_hidden.iter().any(|t| t == id) {
                return Some(Place::AutoHidden { side });
            }
        }
        self.floating
            .iter()
            .find(|f| f.group.tabs.iter().any(|t| t == id))
            .map(|f| Place::Floating { group: f.group.id })
    }

    pub fn group(&self, gid: u32) -> Option<&Group> {
        Side::ALL
            .iter()
            .flat_map(|s| self.dock(*s).groups.iter())
            .chain(self.floating.iter().map(|f| &f.group))
            .find(|g| g.id == gid)
    }

    fn group_mut(&mut self, gid: u32) -> Option<&mut Group> {
        let Layout {
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

    /// Side a tool window would return to (its dock, auto-hide edge, or floating home).
    pub fn home_side(&self, id: &str) -> Side {
        match self.find(id) {
            Some(Place::Docked { side, .. } | Place::AutoHidden { side }) => side,
            Some(Place::Floating { group }) => self
                .floating
                .iter()
                .find(|f| f.group.id == group)
                .map_or(Side::Right, |f| f.home),
            None => Side::Right,
        }
    }

    /// Detach a tool window from wherever it is. Empty groups disappear.
    pub fn remove(&mut self, id: &str) -> bool {
        let mut found = false;
        let fix = |g: &mut Group, found: &mut bool| {
            if let Some(ix) = g.tabs.iter().position(|t| t == id) {
                g.tabs.remove(ix);
                if g.active > ix || g.active >= g.tabs.len() {
                    g.active = g.active.saturating_sub(1);
                }
                *found = true;
            }
        };
        for side in Side::ALL {
            let dock = self.dock_mut(side);
            for g in &mut dock.groups {
                fix(g, &mut found);
            }
            dock.groups.retain(|g| !g.tabs.is_empty());
            let before = dock.auto_hidden.len();
            dock.auto_hidden.retain(|t| t != id);
            found |= before != dock.auto_hidden.len();
        }
        for f in &mut self.floating {
            fix(&mut f.group, &mut found);
        }
        self.floating.retain(|f| !f.group.tabs.is_empty());
        found
    }

    /// Dock a tool window as a new group at the end of `side` (VS guide: dock left/right/bottom).
    pub fn dock_to(&mut self, id: &str, side: Side) {
        self.remove(id);
        let g = self.new_group(id);
        self.dock_mut(side).groups.push(g);
    }

    /// Add a tool window as a tab of an existing group (VS guide: center, "tab").
    /// Returns false if the group does not exist.
    pub fn tab_into(&mut self, id: &str, gid: u32) -> bool {
        if self.group(gid).is_none() {
            return false;
        }
        if self
            .group(gid)
            .is_some_and(|g| g.tabs.len() == 1 && g.tabs[0] == id)
        {
            return true; // dropped onto itself
        }
        self.remove(id);
        // The target may have vanished only if it was the sole window's group; checked above.
        let g = self.group_mut(gid).expect("group checked above");
        g.tabs.push(id.to_owned());
        g.active = g.tabs.len() - 1;
        true
    }

    /// Float a tool window in its own OS window.
    pub fn float(&mut self, id: &str, bounds: [f32; 4]) -> u32 {
        let home = self.home_side(id);
        self.remove(id);
        let group = self.new_group(id);
        let gid = group.id;
        self.floating.push(Floating {
            group,
            bounds,
            home,
        });
        gid
    }

    /// Move a whole floating group back to its home dock.
    pub fn dock_floating(&mut self, gid: u32) {
        if let Some(ix) = self.floating.iter().position(|f| f.group.id == gid) {
            let f = self.floating.remove(ix);
            self.dock_mut(f.home).groups.push(f.group);
        }
    }

    /// Auto-hide a docked tool window to its dock edge. Floating windows cannot
    /// be auto-hidden (same as VS). Returns false if not docked.
    pub fn auto_hide(&mut self, id: &str) -> bool {
        match self.find(id) {
            Some(Place::Docked { side, .. }) => {
                self.remove(id);
                self.dock_mut(side).auto_hidden.push(id.to_owned());
                true
            }
            _ => false,
        }
    }

    /// Pin an auto-hidden window back into its dock: into the dock's first group
    /// if there is one, else as a new group. Returns false if not auto-hidden.
    pub fn pin(&mut self, id: &str) -> bool {
        match self.find(id) {
            Some(Place::AutoHidden { side }) => {
                self.remove(id);
                match self.dock(side).groups.first().map(|g| g.id) {
                    Some(gid) => {
                        self.tab_into(id, gid);
                    }
                    None => self.dock_to(id, side),
                }
                true
            }
            _ => false,
        }
    }

    pub fn activate(&mut self, id: &str) -> bool {
        let gid = match self.find(id) {
            Some(Place::Docked { group, .. } | Place::Floating { group }) => group,
            _ => return false,
        };
        let g = self.group_mut(gid).expect("found above");
        g.active = g.tabs.iter().position(|t| t == id).expect("found above");
        true
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("layout serializes")
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let l: Self = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if l.version != LAYOUT_VERSION {
            return Err(format!("unsupported layout version {}", l.version));
        }
        Ok(l)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> Option<Self> {
        let s = std::fs::read_to_string(path).ok()?;
        match Self::from_json(&s) {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("ignoring layout {}: {e}", path.display());
                None
            }
        }
    }

    /// Every known tool window appears exactly once.
    pub fn is_consistent(&self) -> bool {
        let mut seen: Vec<&str> = Vec::new();
        for side in Side::ALL {
            let d = self.dock(side);
            for g in &d.groups {
                seen.extend(g.tabs.iter().map(String::as_str));
                if g.tabs.is_empty() || g.active >= g.tabs.len() {
                    return false;
                }
            }
            seen.extend(d.auto_hidden.iter().map(String::as_str));
        }
        for f in &self.floating {
            seen.extend(f.group.tabs.iter().map(String::as_str));
        }
        seen.sort_unstable();
        let mut all: Vec<&str> = TOOL_WINDOWS.iter().map(|(i, _)| *i).collect();
        all.sort_unstable();
        seen == all
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::default_vs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout() {
        let l = Layout::default_vs();
        assert!(l.is_consistent());
        assert_eq!(l.right.groups.len(), 2);
        assert_eq!(
            l.right.groups[0].tabs,
            ["solution_explorer", "team_explorer"]
        );
        assert_eq!(l.right.groups[0].active_id(), Some("solution_explorer"));
        assert_eq!(l.bottom.groups[0].tabs, ["error_list", "output"]);
        assert_eq!(l.left.auto_hidden, ["toolbox"]);
    }

    #[test]
    fn dock_tab_float_autohide_pin() {
        let mut l = Layout::default_vs();
        l.dock_to("output", Side::Left);
        assert!(matches!(
            l.find("output"),
            Some(Place::Docked {
                side: Side::Left,
                ..
            })
        ));
        assert_eq!(l.bottom.groups[0].tabs, ["error_list"]);

        let gid = l.right.groups[1].id; // Properties
        assert!(l.tab_into("error_list", gid));
        assert!(l.bottom.groups.is_empty());
        assert_eq!(l.group(gid).unwrap().tabs, ["properties", "error_list"]);
        assert_eq!(l.group(gid).unwrap().active, 1);

        let fid = l.float("properties", [100., 100., 300., 400.]);
        assert_eq!(l.floating[0].home, Side::Right);
        assert_eq!(l.group(gid).unwrap().tabs, ["error_list"]);
        assert!(
            !l.auto_hide("properties"),
            "floating windows cannot auto-hide"
        );

        assert!(l.auto_hide("solution_explorer"));
        assert_eq!(l.right.auto_hidden, ["solution_explorer"]);
        assert!(l.pin("solution_explorer"));
        assert!(matches!(
            l.find("solution_explorer"),
            Some(Place::Docked {
                side: Side::Right,
                ..
            })
        ));

        l.dock_floating(fid);
        assert!(l.floating.is_empty());
        assert!(l.is_consistent());
    }

    #[test]
    fn json_roundtrip() {
        let mut l = Layout::default_vs();
        l.float("output", [1., 2., 3., 4.]);
        l.auto_hide("properties");
        let back = Layout::from_json(&l.to_json()).unwrap();
        assert_eq!(back, l);
        assert!(Layout::from_json("{\"version\": 99}").is_err());
    }
}
