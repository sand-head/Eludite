//! Eludite's widget layer (PLAN.md D1, section 8, section 12 `crates/ui`).
//!
//! Public API:
//! - [`Theme`]: color tokens and typography; `vs_dark()` (default),
//!   `vs_light()`, `vs_blue()`.
//! - [`keymap`]: the key binding table ([`vs_keymap`]) and [`RunCommand`], the
//!   one GPUI action keys and menus dispatch; the shell turns it into a
//!   command-bus invocation.
//! - [`menu`]: Visual Studio's menus ([`vs_menus`]) and the [`MenuBar`] view.
//! - [`status`]: the [`StatusBar`] with named slots.
//! - [`elements`]: small stateless elements (tab strips, panels, buttons).
//! - [`tree`]: tree rows for Solution Explorer and other tree views (indent,
//!   disclosure triangle, label), drawn by the caller's list.
//!
//! Written fresh against the Visual Studio model; Zed's `ui` and `theme` crates
//! are deliberately not used so Eludite cannot look like Zed by construction.

pub mod elements;
pub mod keymap;
pub mod menu;
pub mod status;
mod theme;
pub mod tree;

pub use elements::{icon_button, tab};
pub use keymap::{
    EDITOR_COMMAND_KEYS, KeyBindingSpec, RunCommand, SHELL_CONTEXT, bind_keymap, shortcut_for,
    vs_keymap,
};
pub use menu::{MENU_TITLES, Menu, MenuBar, MenuEntry, menu_bar_with, vs_menus};
pub use status::{SlotAlign, StatusBar, slots};
pub use theme::{Theme, Typography};
pub use tree::{TREE_ROW_HEIGHT, TreeRowStyle, tree_row};
