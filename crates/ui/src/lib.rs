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
//! - [`tree`]: tree rows for Workspace and other tree views (indent,
//!   disclosure triangle, label), drawn by the caller's list.
//! - [`popup`]: IntelliSense popups: completion rows with Visual Studio's kind
//!   icons ([`CompletionKind`]) and the tooltip frame.
//! - [`markdown`]: the Markdown subset tooltips render (paragraphs, code).
//! - [`dialog`]: the frame of a modal dialog, push buttons, and the light bulb menu's rows and glyphs.
//!
//! Written fresh against the Visual Studio model; Zed's `ui` and `theme` crates
//! are deliberately not used so Eludite cannot look like Zed by construction.

pub mod dialog;
pub mod elements;
pub mod keymap;
pub mod markdown;
pub mod menu;
pub mod popup;
pub mod status;
mod theme;
pub mod tree;

pub use dialog::{LightbulbKind, dialog_panel, menu_row, push_button, section_heading};
pub use elements::{highlighted_code, icon_button, tab, text_box, toggle_button};
pub use keymap::{
    EDITOR_COMMAND_KEYS, KeyBindingSpec, RunCommand, SHELL_CONTEXT, bind_keymap, shortcut_for,
    vs_keymap,
};
pub use menu::{MENU_TITLES, Menu, MenuBar, MenuEntry, menu_bar_with, vs_menus};
pub use popup::CompletionKind;
pub use status::{SlotAlign, StatusBar, slots};
pub use theme::{Theme, Typography};
pub use tree::{TREE_ROW_HEIGHT, TreeRowStyle, tree_row};
