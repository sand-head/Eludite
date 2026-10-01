//! Niello's widget layer: theme tokens, typography and small GPUI elements
//! (PLAN.md D1, section 8, section 12 `crates/ui`).
//!
//! Written fresh against the Visual Studio model; Zed's `ui` and `theme` crates
//! are deliberately not used so Niello cannot look like Zed by construction.

mod elements;
mod theme;

pub use elements::{menu_bar, panel, status_bar, tab_strip};
pub use theme::{Theme, Typography};
