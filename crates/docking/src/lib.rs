//! Visual Studio-style docking (PLAN.md section 8 "Docking", section 12 `crates/docking`).
//!
//! `DockLayout` is pure data: tool windows ordered per dock side plus a document
//! area of tabs. `render` draws it with `niello-ui` elements. Floating,
//! auto-hide, drag guides and persistence come later.

mod model;
mod render;

pub use model::{
    DockArea, DockLayout, DockSide, DocumentArea, DocumentTab, Layout, Presentation, ToolWindow,
    ids,
};
pub use render::render_layout;
