//! Brief 0001 spike: a throwaway GPUI prototype of Niello's shell.
//!
//! - `layout`: VS-style docking model with JSON persistence.
//! - `shell`: the window's root view (docks, guides, floating windows, auto-hide).
//! - `text_view` and `buffer`: a 100k-line editable text view.
//! - `bench`: frame-time, keystroke and startup measurements.
//!
//! Not production code: see docs/briefs/0001-report.md for what was learned.

pub mod bench;
pub mod buffer;
pub mod layout;
pub mod shell;
pub mod text_view;

#[cfg(test)]
mod tests;
