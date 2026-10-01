//! Editor core (PLAN.md 4.1, section 12 `crates/editor`).
//!
//! `Buffer` is a stand-in: a `ropey::Rope` with a simple operation-based undo
//! stack. The Phase 0 vendoring audit of Zed's text crates (sum-tree, buffer,
//! anchors; PLAN.md D1) decides whether this is replaced or grown.

mod buffer;

pub use buffer::{Buffer, Edit};
