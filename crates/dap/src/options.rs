//! Debugging options the person sets in Tools > Options > Debugging > General (brief 0077) that the launch and attach
//! argument builders read: Enable Just My Code (`debugger.justMyCode`, on by default), passed as `justMyCode`. The
//! shell sets them from its settings; they apply to the next launch or attach.

use std::sync::atomic::{AtomicBool, Ordering};

static JUST_MY_CODE: AtomicBool = AtomicBool::new(true);

/// Whether the next launch or attach asks for Just My Code.
pub fn just_my_code() -> bool {
    JUST_MY_CODE.load(Ordering::Relaxed)
}

pub fn set_just_my_code(on: bool) {
    JUST_MY_CODE.store(on, Ordering::Relaxed);
}
