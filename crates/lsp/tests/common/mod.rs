//! Helpers shared by the client tests.

use std::time::Duration;

/// `measured` under `limit`, asserted on a developer machine only: under CI (`CI` set) the hosted runners are shared
/// VMs, not a reference machine, so the number is printed instead.
pub fn assert_budget(what: &str, measured: Duration, limit: Duration) {
    if std::env::var_os("CI").is_some() {
        eprintln!(
            "timing: {what} {:.2} ms not asserted against {:.0} ms: a CI run, not a reference machine",
            measured.as_secs_f64() * 1e3,
            limit.as_secs_f64() * 1e3
        );
    } else {
        assert!(
            measured < limit,
            "{what}: {measured:?} is not under {limit:?}"
        );
    }
}
