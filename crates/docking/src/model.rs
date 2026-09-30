/// Where a tool window is docked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

impl DockSide {
    pub const ALL: [DockSide; 3] = [DockSide::Left, DockSide::Right, DockSide::Bottom];
}

/// Well-known tool window ids.
pub mod ids {
    pub const SOLUTION_EXPLORER: &str = "solution_explorer";
    pub const PROPERTIES: &str = "properties";
    pub const ERROR_LIST: &str = "error_list";
    pub const OUTPUT: &str = "output";
    pub const TOOLBOX: &str = "toolbox";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWindow {
    pub id: String,
    pub title: String,
}

impl ToolWindow {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// How a dock shows several tool windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// One above the other (VS's right dock: Solution Explorer over Properties).
    Stacked,
    /// One at a time behind a tab strip (VS's bottom dock: Error List | Output).
    Tabbed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DockArea {
    pub windows: Vec<ToolWindow>,
    pub presentation: Presentation,
    /// Index into `windows` shown when `Tabbed`.
    pub active: usize,
    /// Width (left/right) or height (bottom) in logical pixels.
    pub size: f32,
}

impl DockArea {
    pub fn new(presentation: Presentation, size: f32) -> Self {
        Self {
            windows: Vec::new(),
            presentation,
            active: 0,
            size,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn active_window(&self) -> Option<&ToolWindow> {
        self.windows.get(self.active)
    }

    fn clamp_active(&mut self) {
        if self.active >= self.windows.len() {
            self.active = self.windows.len().saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTab {
    pub id: String,
    pub title: String,
}

impl DocumentTab {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentArea {
    pub tabs: Vec<DocumentTab>,
    pub active: Option<usize>,
}

impl DocumentArea {
    pub fn open(&mut self, tab: DocumentTab) {
        let ix = match self.tabs.iter().position(|t| t.id == tab.id) {
            Some(ix) => ix,
            None => {
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        self.active = Some(ix);
    }

    pub fn active_tab(&self) -> Option<&DocumentTab> {
        self.active.and_then(|ix| self.tabs.get(ix))
    }
}

/// The whole window layout: three docks around a document area.
#[derive(Debug, Clone, PartialEq)]
pub struct DockLayout {
    pub left: DockArea,
    pub right: DockArea,
    pub bottom: DockArea,
    pub documents: DocumentArea,
}

/// Alias matching PLAN.md's vocabulary (`Layout::default_vs()`).
pub type Layout = DockLayout;

impl DockLayout {
    pub fn empty() -> Self {
        Self {
            left: DockArea::new(Presentation::Stacked, 240.),
            right: DockArea::new(Presentation::Stacked, 300.),
            bottom: DockArea::new(Presentation::Tabbed, 200.),
            documents: DocumentArea::default(),
        }
    }

    /// Visual Studio's default: Solution Explorer and Properties stacked on the
    /// right, Error List and Output tabbed at the bottom, a Welcome document.
    /// (Toolbox collapsed left arrives with auto-hide.)
    pub fn default_vs() -> Self {
        let mut layout = Self::empty();
        layout.add(
            DockSide::Right,
            ToolWindow::new(ids::SOLUTION_EXPLORER, "Solution Explorer"),
        );
        layout.add(
            DockSide::Right,
            ToolWindow::new(ids::PROPERTIES, "Properties"),
        );
        layout.add(
            DockSide::Bottom,
            ToolWindow::new(ids::ERROR_LIST, "Error List"),
        );
        layout.add(DockSide::Bottom, ToolWindow::new(ids::OUTPUT, "Output"));
        layout
            .documents
            .open(DocumentTab::new("welcome", "Welcome"));
        layout
    }

    pub fn dock(&self, side: DockSide) -> &DockArea {
        match side {
            DockSide::Left => &self.left,
            DockSide::Right => &self.right,
            DockSide::Bottom => &self.bottom,
        }
    }

    pub fn dock_mut(&mut self, side: DockSide) -> &mut DockArea {
        match side {
            DockSide::Left => &mut self.left,
            DockSide::Right => &mut self.right,
            DockSide::Bottom => &mut self.bottom,
        }
    }

    /// Append a tool window to `side`. A window id appears at most once in the
    /// layout, so an existing window with the same id is moved.
    pub fn add(&mut self, side: DockSide, window: ToolWindow) {
        self.remove(&window.id);
        self.dock_mut(side).windows.push(window);
    }

    pub fn remove(&mut self, id: &str) -> Option<(DockSide, ToolWindow)> {
        let (side, ix) = self.position(id)?;
        let dock = self.dock_mut(side);
        let window = dock.windows.remove(ix);
        dock.clamp_active();
        Some((side, window))
    }

    pub fn find(&self, id: &str) -> Option<(DockSide, &ToolWindow)> {
        let (side, ix) = self.position(id)?;
        Some((side, &self.dock(side).windows[ix]))
    }

    /// Move a tool window to the end of another dock. Returns false if `id` is unknown.
    pub fn move_to(&mut self, id: &str, side: DockSide) -> bool {
        match self.remove(id) {
            Some((_, window)) => {
                self.dock_mut(side).windows.push(window);
                true
            }
            None => false,
        }
    }

    /// Make `id` the visible tab of its dock. Returns false if `id` is unknown.
    pub fn activate(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some((side, ix)) => {
                self.dock_mut(side).active = ix;
                true
            }
            None => false,
        }
    }

    fn position(&self, id: &str) -> Option<(DockSide, usize)> {
        DockSide::ALL.into_iter().find_map(|side| {
            self.dock(side)
                .windows
                .iter()
                .position(|w| w.id == id)
                .map(|ix| (side, ix))
        })
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        Self::default_vs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(dock: &DockArea) -> Vec<&str> {
        dock.windows.iter().map(|w| w.title.as_str()).collect()
    }

    #[test]
    fn default_vs_layout() {
        let l = Layout::default_vs();
        assert!(l.left.is_empty());
        assert_eq!(titles(&l.right), ["Solution Explorer", "Properties"]);
        assert_eq!(l.right.presentation, Presentation::Stacked);
        assert_eq!(titles(&l.bottom), ["Error List", "Output"]);
        assert_eq!(l.bottom.presentation, Presentation::Tabbed);
        assert_eq!(l.bottom.active_window().unwrap().id, ids::ERROR_LIST);
        assert_eq!(l.documents.active_tab().unwrap().title, "Welcome");
    }

    #[test]
    fn find_move_remove() {
        let mut l = DockLayout::default_vs();
        assert_eq!(l.find(ids::OUTPUT).unwrap().0, DockSide::Bottom);
        assert!(l.move_to(ids::OUTPUT, DockSide::Left));
        assert_eq!(l.find(ids::OUTPUT).unwrap().0, DockSide::Left);
        assert_eq!(titles(&l.bottom), ["Error List"]);
        assert!(!l.move_to("nope", DockSide::Left));

        let (side, w) = l.remove(ids::PROPERTIES).unwrap();
        assert_eq!((side, w.title.as_str()), (DockSide::Right, "Properties"));
        assert!(l.find(ids::PROPERTIES).is_none());
        assert!(l.remove(ids::PROPERTIES).is_none());
    }

    #[test]
    fn add_existing_id_moves_instead_of_duplicating() {
        let mut l = DockLayout::default_vs();
        l.add(
            DockSide::Left,
            ToolWindow::new(ids::SOLUTION_EXPLORER, "Solution Explorer"),
        );
        assert_eq!(titles(&l.right), ["Properties"]);
        assert_eq!(titles(&l.left), ["Solution Explorer"]);
    }

    #[test]
    fn active_tab_tracks_removal() {
        let mut l = DockLayout::default_vs();
        assert!(l.activate(ids::OUTPUT));
        assert_eq!(l.bottom.active, 1);
        l.remove(ids::OUTPUT);
        assert_eq!(l.bottom.active, 0);
        l.remove(ids::ERROR_LIST);
        assert!(l.bottom.active_window().is_none());
    }

    #[test]
    fn documents_dedupe_by_id() {
        let mut docs = DocumentArea::default();
        docs.open(DocumentTab::new("a", "A"));
        docs.open(DocumentTab::new("b", "B"));
        docs.open(DocumentTab::new("a", "A"));
        assert_eq!(docs.tabs.len(), 2);
        assert_eq!(docs.active, Some(0));
    }
}
