//! Eludite's widget layer (PLAN.md D1, section 8, section 12 `crates/ui`).
//!
//! Public API:
//! - [`Theme`]: color tokens and typography; `vs_dark()` (default),
//!   `vs_light()`, `vs_blue()`.
//! - [`keymap`]: the key binding table ([`vs_keymap`]) and [`RunCommand`], the
//!   one GPUI action keys and menus dispatch; the shell turns it into a
//!   command-bus invocation.
//! - [`menu`]: Visual Studio's menus ([`vs_menus`]) and the [`MenuBar`] view.
//! - [`status`]: the [`StatusBar`] with named slots, and [`status_toggle`] (a two-state control beside them).
//! - [`elements`]: small stateless elements (tab strips, panels, buttons, text and check boxes, the selector bar).
//! - [`startup`]: the Startup Projects dialog (Project > Set Startup Projects..., brief 0028).
//! - [`tree`]: tree rows for Workspace and other tree views (indent,
//!   disclosure triangle, label), drawn by the caller's list.
//! - [`popup`]: IntelliSense popups: completion rows with Visual Studio's kind
//!   icons ([`CompletionKind`]) and the tooltip frame.
//! - [`markdown`]: the Markdown subset tooltips render (paragraphs, code).
//! - [`dialog`]: the frame of a modal dialog, push buttons, and the light bulb menu's rows and glyphs.
//! - [`diff`]: the line diff, its hunks and the inline diff rows of the pending-change review view.
//! - [`vertical_text`]: text rotated 90 degrees clockwise ([`vertical_label`]) for
//!   the auto-hide strips on the left and right edges.
//! - [`title_bar`]: the main window's title bar Eludite draws (ADR-0010): the menu bar, the title, the caption
//!   buttons, and the window's frame on Linux with client-side decorations.
//! - [`transcript`]: the Agents window's transcript rows: prompts, agent text, thinking, tool call cards with their
//!   status, plans and notices.
//!
//! Written fresh against the Visual Studio model; Zed's `ui` and `theme` crates
//! are deliberately not used so Eludite cannot look like Zed by construction.

pub mod dialog;
pub mod diff;
pub mod elements;
pub mod keymap;
pub mod markdown;
pub mod menu;
pub mod popup;
pub mod startup;
pub mod status;
mod theme;
pub mod title_bar;
pub mod transcript;
pub mod tree;
pub mod vertical_text;

pub use dialog::{LightbulbKind, dialog_panel, menu_row, push_button, section_heading};
pub use elements::{
    BoundsMap, bounds_canvas, check_box, highlighted_code, icon_button, selector_bar,
    selector_option, tab, text_box, toggle_button,
};
pub use keymap::{
    EDITOR_COMMAND_KEYS, KeyBindingSpec, RunCommand, SHELL_CONTEXT, bind_keymap, shortcut_for,
    vs_keymap,
};
pub use menu::{MENU_TITLES, Menu, MenuBar, MenuEntry, menu_bar_with, vs_menus};
pub use popup::CompletionKind;
pub use status::{SlotAlign, StatusBar, slots, status_toggle};
pub use theme::{Theme, Typography};
pub use title_bar::TitleBar;
pub use tree::{TREE_ROW_HEIGHT, TreeRowStyle, tree_row};
pub use vertical_text::{VerticalLabel, vertical_label};
