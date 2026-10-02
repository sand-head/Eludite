//! The Agents window (brief 0016): hosting ACP agents in the shell, Eludite's MCP endpoint inside this process, the
//! permission policy and the agents' edits held as pending changes.

// Wired into the shell by the Agents window (the next step of brief 0016).
#[cfg_attr(not(test), allow(dead_code))]
pub mod endpoint;
