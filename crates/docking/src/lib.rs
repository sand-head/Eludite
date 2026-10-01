//! Visual Studio-style docking (PLAN.md section 8 "Docking", 4.12, section 12
//! `crates/docking`; brief 0008).
//!
//! Public API:
//! - [`model`]: [`DockLayout`], pure data: three docks of tab groups,
//!   floating groups, auto-hidden and closed tool windows, and document tabs
//!   with pinned and preview states. Versioned JSON with a migration hook.
//!   [`ToolWindowRegistry`] lists the tool windows a build knows; later briefs
//!   register theirs.
//! - [`controller`]: [`DockController`], the shared owner of the layout and the
//!   `eludite_commands::view::ViewTarget` that the `eludite.view.*` commands act
//!   on. Every layout change goes through those commands. Document tabs are
//!   opened, closed and marked dirty by the shell's `eludite.file.*` commands
//!   through [`DockController::open_document`] and its siblings; a tab's close
//!   button dispatches `eludite.file.close`.
//! - [`persist`]: [`LayoutStore`] (per-solution, default and named layout
//!   files under the user config directory) and [`LayoutWriter`] (debounced
//!   saving on a background thread).
//! - [`view`]: [`DockHost`], the GPUI view: drag with docking guides, tabs,
//!   floating OS windows, auto-hide strips and fly-outs.
//!
//! Known limit (brief 0001 report, section 4): GPUI drag and drop stays inside
//! one window, so a floating window re-docks through its Dock button or the
//! `eludite.view.dock` command, not by dragging it back onto the guides.

pub mod controller;
pub mod model;
pub mod persist;
pub mod view;

pub use controller::{DockController, Snapshot};
pub use model::{
    Bounds, Dock, DockLayout, DockSide, DocumentArea, DocumentTab, FloatingGroup, Group,
    HiddenWindow, LAYOUT_SCHEMA_VERSION, Layout, LayoutError, Place, ToolWindowDescriptor,
    ToolWindowInfo, ToolWindowRegistry, ids,
};
pub use persist::{LayoutSource, LayoutStore, LayoutWriter, eludite_config_dir};
pub use view::{
    DockHost, DocumentBody, DraggedTool, FloatingView, Persistence, Probe, RenderProbe, ToolBody,
};

#[cfg(test)]
mod tests;
