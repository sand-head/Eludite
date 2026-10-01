//! [`DockController`]: the shared, thread-safe owner of the layout, and the
//! [`ViewTarget`] the `eludite.view.*` commands act on.
//!
//! Menus, keys, docking-guide drops and agents all change the layout by
//! invoking a command; the command handler runs here under a short mutex and
//! then wakes every subscriber (the GPUI view), which re-renders and schedules
//! a save. Nothing here does I/O.

use std::sync::{Arc, Mutex, MutexGuard};

use eludite_commands::CommandError;
use eludite_commands::view::{
    DockEdge, DockTarget, ToolWindowState, ViewOutput, ViewRequest, ViewTarget, WindowState,
};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use crate::model::{Bounds, DockLayout, DockSide, LayoutError, Place, ToolWindowRegistry};

impl From<DockEdge> for DockSide {
    fn from(e: DockEdge) -> Self {
        match e {
            DockEdge::Left => DockSide::Left,
            DockEdge::Right => DockSide::Right,
            DockEdge::Bottom => DockSide::Bottom,
        }
    }
}

impl From<DockSide> for DockEdge {
    fn from(s: DockSide) -> Self {
        match s {
            DockSide::Left => DockEdge::Left,
            DockSide::Right => DockEdge::Right,
            DockSide::Bottom => DockEdge::Bottom,
        }
    }
}

/// The layout plus transient view state, as of one revision.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub layout: DockLayout,
    pub registry: Arc<ToolWindowRegistry>,
    /// The tool window `eludite.view.*` acts on when no id is given (last shown
    /// or clicked). Not persisted.
    pub active_tool: Option<String>,
    /// The auto-hidden window currently slid out. Not persisted.
    pub flyout: Option<String>,
    /// Bumped on every change.
    pub revision: u64,
}

struct State {
    snapshot: Snapshot,
    subscribers: Vec<UnboundedSender<()>>,
}

/// Cheap to clone; all clones share one layout.
#[derive(Clone)]
pub struct DockController {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for DockController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockController")
            .field("revision", &self.lock().snapshot.revision)
            .finish()
    }
}

impl DockController {
    pub fn new(mut layout: DockLayout, registry: ToolWindowRegistry) -> Self {
        layout.normalize(&registry);
        Self {
            state: Arc::new(Mutex::new(State {
                snapshot: Snapshot {
                    layout,
                    registry: Arc::new(registry),
                    active_tool: None,
                    flyout: None,
                    revision: 0,
                },
                subscribers: Vec::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic mid-update leaves a layout that still parses; keep going.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A copy of the current state.
    pub fn snapshot(&self) -> Snapshot {
        self.lock().snapshot.clone()
    }

    pub fn layout(&self) -> DockLayout {
        self.lock().snapshot.layout.clone()
    }

    pub fn revision(&self) -> u64 {
        self.lock().snapshot.revision
    }

    /// A stream that yields once per change (changes may coalesce).
    pub fn subscribe(&self) -> UnboundedReceiver<()> {
        let (tx, rx) = unbounded();
        self.lock().subscribers.push(tx);
        rx
    }

    fn changed(state: &mut State) {
        state.snapshot.revision += 1;
        state.subscribers.retain(|tx| tx.unbounded_send(()).is_ok());
    }

    /// Slide the auto-hide fly-out back in (clicking elsewhere). Transient view
    /// state, not a command.
    pub fn close_flyout(&self) -> bool {
        let mut s = self.lock();
        if s.snapshot.flyout.take().is_some() {
            Self::changed(&mut s);
            true
        } else {
            false
        }
    }

    /// Replace the whole layout (for example with a named layout).
    pub fn replace_layout(&self, mut layout: DockLayout) {
        let mut s = self.lock();
        layout.normalize(&s.snapshot.registry);
        s.snapshot.layout = layout;
        s.snapshot.flyout = None;
        Self::changed(&mut s);
    }

    fn state_of(snap: &Snapshot, id: &str) -> Option<ToolWindowState> {
        if snap.registry.contains(id) {
            let info = snap.layout.info(id, &snap.registry)?;
            let (state, side) = match info.place {
                Place::Docked { side, .. } => (WindowState::Docked, Some(side)),
                Place::AutoHidden { side } => (WindowState::AutoHidden, Some(side)),
                Place::Hidden { side } => (WindowState::Hidden, Some(side)),
                Place::Floating { .. } => (WindowState::Floating, snap.layout.home_side(id)),
            };
            let auto_hidden = state == WindowState::AutoHidden;
            Some(ToolWindowState {
                id: info.id,
                title: info.title,
                state,
                side: side.map(Into::into),
                group: info.group,
                active: info.active || (auto_hidden && snap.flyout.as_deref() == Some(id)),
                flyout_open: auto_hidden.then(|| snap.flyout.as_deref() == Some(id)),
            })
        } else {
            let tab = snap.layout.documents.get(id)?;
            Some(ToolWindowState {
                id: tab.id.clone(),
                title: tab.title.clone(),
                state: WindowState::Document,
                side: None,
                group: None,
                active: snap.layout.documents.active.as_deref() == Some(id),
                flyout_open: None,
            })
        }
    }

    /// Every registered tool window's state, in registration order.
    pub fn all_states(&self) -> Vec<ToolWindowState> {
        let s = self.lock();
        let snap = &s.snapshot;
        snap.registry
            .iter()
            .filter_map(|w| Self::state_of(snap, &w.id))
            .collect()
    }

    fn resolve(snap: &Snapshot, id: Option<String>) -> Result<String, CommandError> {
        id.or_else(|| snap.active_tool.clone()).ok_or_else(|| {
            CommandError::InvalidInput("no `id` given and no active tool window".into())
        })
    }

    fn run(snap: &mut Snapshot, request: ViewRequest) -> Result<Option<String>, CommandError> {
        let reg = snap.registry.clone();
        let check = |id: &str| {
            if reg.contains(id) {
                Ok(())
            } else {
                Err(CommandError::InvalidInput(format!(
                    "unknown tool window `{id}`"
                )))
            }
        };
        let id = match request {
            ViewRequest::Show { id } => {
                if !reg.contains(&id) && snap.layout.documents.activate(&id) {
                    return Ok(Some(id));
                }
                check(&id)?;
                snap.layout.show(&id).map_err(layout_err)?;
                let auto_hidden = matches!(snap.layout.find(&id), Some(Place::AutoHidden { .. }));
                snap.flyout = auto_hidden.then(|| id.clone());
                snap.active_tool = Some(id.clone());
                id
            }
            ViewRequest::Hide { id } => {
                let id = Self::resolve(snap, id)?;
                check(&id)?;
                snap.layout.hide(&id).map_err(layout_err)?;
                id
            }
            ViewRequest::Float { id, bounds } => {
                let id = Self::resolve(snap, id)?;
                check(&id)?;
                let b = bounds.map_or(Bounds::DEFAULT_FLOAT, |b| Bounds {
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                });
                snap.layout.float(&id, b).map_err(layout_err)?;
                snap.active_tool = Some(id.clone());
                id
            }
            ViewRequest::AutoHide { id } => {
                let id = Self::resolve(snap, id)?;
                check(&id)?;
                snap.layout.auto_hide(&id).map_err(layout_err)?;
                id
            }
            ViewRequest::Dock { id, target } => {
                let id = Self::resolve(snap, id)?;
                check(&id)?;
                match target {
                    DockTarget::Home => snap.layout.dock_home(&id),
                    DockTarget::Side(e) => snap.layout.dock_to(&id, e.into()),
                    DockTarget::TabWith(t) => {
                        check(&t)?;
                        snap.layout.tab_into(&id, &t)
                    }
                }
                .map_err(layout_err)?;
                snap.active_tool = Some(id.clone());
                id
            }
            ViewRequest::ResetLayout => {
                snap.layout = DockLayout::default_vs(&reg);
                snap.active_tool = None;
                snap.flyout = None;
                return Ok(None);
            }
        };
        // A fly-out only stays open while its window is still auto-hidden.
        if let Some(f) = snap.flyout.clone()
            && !matches!(snap.layout.find(&f), Some(Place::AutoHidden { .. }))
        {
            snap.flyout = None;
        }
        if snap
            .active_tool
            .as_deref()
            .is_some_and(|a| matches!(snap.layout.find(a), Some(Place::Hidden { .. })))
        {
            snap.active_tool = None;
        }
        Ok(Some(id))
    }
}

fn layout_err(e: LayoutError) -> CommandError {
    match e {
        LayoutError::UnknownWindow(_) => CommandError::InvalidInput(e.to_string()),
        _ => CommandError::Failed(e.to_string()),
    }
}

impl ViewTarget for DockController {
    fn apply(&self, request: ViewRequest) -> Result<ViewOutput, CommandError> {
        let mut s = self.lock();
        let mut snap = s.snapshot.clone();
        let id = Self::run(&mut snap, request)?;
        let out = match id {
            Some(id) => ViewOutput::Window(
                Self::state_of(&snap, &id)
                    .ok_or_else(|| CommandError::Failed(format!("`{id}` vanished")))?,
            ),
            None => ViewOutput::Reset(
                snap.registry
                    .iter()
                    .filter_map(|w| Self::state_of(&snap, &w.id))
                    .collect(),
            ),
        };
        debug_assert!(snap.layout.is_consistent(&snap.registry));
        s.snapshot = Snapshot {
            revision: s.snapshot.revision,
            ..snap
        };
        Self::changed(&mut s);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_commands::{CommandRegistry, view};
    use serde_json::json;

    fn setup() -> (DockController, CommandRegistry) {
        let reg = ToolWindowRegistry::vs_default();
        let c = DockController::new(DockLayout::default_vs(&reg), reg);
        let mut r = CommandRegistry::new();
        view::register(&mut r, Arc::new(c.clone())).unwrap();
        (c, r)
    }

    #[test]
    fn commands_change_the_layout() {
        let (c, r) = setup();
        let mut rx = c.subscribe();
        let out = r
            .invoke(view::DOCK, json!({"id": "output", "side": "left"}))
            .unwrap();
        assert_eq!(out["state"], "docked");
        assert_eq!(out["side"], "left");
        assert_eq!(out["group"], json!(["output"]));
        assert!(rx.try_recv().is_ok(), "subscriber woken");
        assert_eq!(c.revision(), 1);

        // No id: acts on the active tool window (Output, just docked).
        let out = r.invoke(view::FLOAT, json!({})).unwrap();
        assert_eq!(out["state"], "floating");
        assert_eq!(out["side"], "left", "home dock");
        let out = r.invoke(view::DOCK, json!({})).unwrap();
        assert_eq!(out["state"], "docked");

        let out = r
            .invoke(view::AUTO_HIDE, json!({"id": "properties"}))
            .unwrap();
        assert_eq!(out["state"], "auto_hidden");
        assert_eq!(out["flyout_open"], false);
        let out = r.invoke(view::SHOW, json!({"id": "properties"})).unwrap();
        assert_eq!(out["flyout_open"], true);
        assert_eq!(c.snapshot().flyout.as_deref(), Some("properties"));
        assert!(c.close_flyout());
        assert!(!c.close_flyout());

        let out = r.invoke(view::HIDE, json!({"id": "toolbox"})).unwrap();
        assert_eq!(out["state"], "hidden");
        let out = r.invoke(view::SHOW, json!({"id": "toolbox"})).unwrap();
        assert_eq!(out["state"], "docked");
        assert_eq!(out["side"], "left");

        let out = r
            .invoke(
                view::DOCK,
                json!({"id": "error_list", "tab_with": "toolbox"}),
            )
            .unwrap();
        assert_eq!(out["group"], json!(["output", "toolbox", "error_list"]));

        let out = r.invoke(view::RESET_LAYOUT, json!({})).unwrap();
        assert_eq!(out["tool_windows"].as_array().unwrap().len(), 6);
        assert_eq!(
            c.layout(),
            DockLayout::default_vs(&ToolWindowRegistry::vs_default())
        );
        assert!(c.snapshot().active_tool.is_none());
    }

    #[test]
    fn commands_from_another_thread_wake_subscribers() {
        let (c, r) = setup();
        let mut rx = c.subscribe();
        let r = Arc::new(r);
        let r2 = r.clone();
        std::thread::spawn(move || {
            r2.invoke(view::DOCK, json!({"id": "toolbox", "side": "bottom"}))
                .unwrap();
        })
        .join()
        .unwrap();
        assert!(rx.try_recv().is_ok());
        assert_eq!(c.layout().bottom.groups.last().unwrap().tabs, ["toolbox"]);
    }

    #[test]
    fn show_activates_documents() {
        let (c, r) = setup();
        let out = r.invoke(view::SHOW, json!({"id": "welcome"})).unwrap();
        assert_eq!(out["state"], "document");
        assert_eq!(out["active"], true);
        assert_eq!(c.layout().documents.active.as_deref(), Some("welcome"));
    }

    #[test]
    fn errors_leave_layout_untouched() {
        let (c, r) = setup();
        let before = c.snapshot();
        assert!(matches!(
            r.invoke(view::SHOW, json!({"id": "nope"})),
            Err(CommandError::InvalidInput(_))
        ));
        assert!(matches!(
            r.invoke(view::HIDE, json!({})),
            Err(CommandError::InvalidInput(_))
        ));
        r.invoke(view::FLOAT, json!({"id": "output"})).unwrap();
        assert!(matches!(
            r.invoke(view::AUTO_HIDE, json!({"id": "output"})),
            Err(CommandError::Failed(_))
        ));
        assert!(matches!(
            r.invoke(view::DOCK, json!({"id": "output", "tab_with": "toolbox"})),
            Err(CommandError::Failed(_))
        ));
        assert_eq!(
            c.revision(),
            before.revision + 1,
            "only the float changed it"
        );
    }
}
