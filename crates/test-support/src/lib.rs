//! Test helpers shared by every Eludite crate (a dev-dependency only; nothing here ships).
//!
//! Three rules keep the suite deterministic on any machine, from a developer's workstation to the cheapest hosted CI
//! runner:
//!
//! 1. **A timing budget is a performance claim, not a correctness one.** Tests check behavior with ordinary asserts and
//!    hand only the measured time (or bytes, or rate) to [`assert_budget`], [`assert_memory_budget`] or
//!    [`assert_at_least`]. Those assert on a developer machine and report, never assert, on CI ([`ci`]) and on a
//!    machine whose load average exceeds its cores ([`machine_busy`]). `ELUDITE_BUDGETS=assert` or `report` forces
//!    either way.
//! 2. **A bound that only catches a hang is generous and scales.** A wait for a process, an adapter, a build or another
//!    thread goes through [`hang_bound`] (or [`wait_until`], which uses it): on CI the bound is
//!    [`timeout_scale`] times longer (3 unless `ELUDITE_TEST_TIMEOUT_SCALE` says otherwise). Such a bound fails a test
//!    only when something is really stuck. Never sleep a fixed time and then assert that something happened: wait for
//!    it.
//! 3. **A skip is visible, and CI can forbid it.** A test that needs an external tool (netcoredbg, Chrome, CEF, Mono,
//!    lldb-dap, js-debug, …) and does not find it calls [`skip`], which prints why, or panics when `ELUDITE_REQUIRE`
//!    names that tool (or is `all`): CI sets it for every tool it installs, so a broken install fails loudly instead of
//!    passing silently.
//!
//! Public API: [`ci`], [`machine_busy`], [`budgets_enforced`], [`assert_budget`], [`assert_memory_budget`],
//! [`assert_at_least`], [`timeout_scale`], [`hang_bound`], [`wait_until`], [`required`], [`skip`].

use std::fmt::Display;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A CI run: `CI` (GitHub Actions and most others), `GITHUB_ACTIONS` or `TF_BUILD` (Azure Pipelines) set non-empty.
pub fn ci() -> bool {
    static CI: OnceLock<bool> = OnceLock::new();
    *CI.get_or_init(|| ["CI", "GITHUB_ACTIONS", "TF_BUILD"].iter().any(|v| set(v)))
}

/// The 1-minute load average above the core count (Linux; elsewhere never): other builds or agents share the machine,
/// and scheduling inflates every measured time.
pub fn machine_busy() -> bool {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    load_average().is_some_and(|l| l > cores)
}

/// Whether budgets assert (true) or only report (false) right now. See the crate docs.
pub fn budgets_enforced() -> bool {
    match std::env::var("ELUDITE_BUDGETS").as_deref() {
        Ok("assert") => true,
        Ok("report") => false,
        _ => !ci() && !machine_busy(),
    }
}

/// `measured` under `limit`: asserted when [`budgets_enforced`], otherwise printed with the reason.
#[track_caller]
pub fn assert_budget(what: &str, measured: Duration, limit: Duration) {
    let (ms, limit_ms) = (measured.as_secs_f64() * 1e3, limit.as_secs_f64() * 1e3);
    check(
        what,
        measured < limit,
        format_args!("{ms:.2} ms (budget {limit_ms:.0} ms)"),
    );
}

/// `bytes` under `limit` bytes (a memory budget), with [`assert_budget`]'s rule.
#[track_caller]
pub fn assert_memory_budget(what: &str, bytes: u64, limit: u64) {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    check(
        what,
        bytes < limit,
        format_args!("{:.1} MB (budget {:.1} MB)", mb(bytes), mb(limit)),
    );
}

/// `measured` at least `floor` (a rate: frames, stops or hits in a fixed time), with [`assert_budget`]'s rule.
#[track_caller]
pub fn assert_at_least(what: &str, measured: f64, floor: f64) {
    check(
        what,
        measured >= floor,
        format_args!("{measured} (floor {floor})"),
    );
}

/// How much longer [`hang_bound`] is than its base: `ELUDITE_TEST_TIMEOUT_SCALE` (an integer of at least 1), else 3 on
/// CI and 1 elsewhere.
pub fn timeout_scale() -> u32 {
    static SCALE: OnceLock<u32> = OnceLock::new();
    *SCALE.get_or_init(|| {
        std::env::var("ELUDITE_TEST_TIMEOUT_SCALE")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&s| s >= 1)
            .unwrap_or(if ci() { 3 } else { 1 })
    })
}

/// A bound that only turns a hang into a failure: `base` times [`timeout_scale`].
pub fn hang_bound(base: Duration) -> Duration {
    base * timeout_scale()
}

/// Polls `condition` every few milliseconds until it holds; panics naming `what` after [`hang_bound`]`(base)`.
#[track_caller]
pub fn wait_until(what: &str, base: Duration, mut condition: impl FnMut() -> bool) {
    let bound = hang_bound(base);
    let start = Instant::now();
    while !condition() {
        if start.elapsed() > bound {
            panic!("timed out after {bound:?} waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether `ELUDITE_REQUIRE` (a comma-separated list, or `all`) names `tool`.
pub fn required(tool: &str) -> bool {
    std::env::var("ELUDITE_REQUIRE").is_ok_and(|list| {
        list.split(',')
            .map(str::trim)
            .any(|t| t.eq_ignore_ascii_case("all") || t.eq_ignore_ascii_case(tool))
    })
}

/// A test skipping because `tool` is missing: prints `why`, or panics when [`required`]`(tool)`. The caller returns
/// after it.
#[track_caller]
pub fn skip(tool: &str, why: impl Display) {
    if required(tool) {
        panic!("{tool} is required here (ELUDITE_REQUIRE) but missing: {why}");
    }
    eprintln!("skipped: {tool} is missing: {why}");
}

#[track_caller]
fn check(what: &str, within: bool, numbers: std::fmt::Arguments<'_>) {
    if budgets_enforced() {
        assert!(within, "{what}: {numbers}");
        eprintln!("timing: {what} {numbers}");
    } else {
        let why = if ci() {
            "a CI run, not a reference machine".to_owned()
        } else if let Some(l) = load_average() {
            format!("load average {l:.1}")
        } else {
            "ELUDITE_BUDGETS=report".to_owned()
        };
        eprintln!(
            "timing: {what} {numbers}, {}: {why}",
            if within {
                "within"
            } else {
                "OVER, not asserted"
            }
        );
    }
}

fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hang_bound_never_shrinks() {
        let base = Duration::from_secs(10);
        assert!(hang_bound(base) >= base);
        assert_eq!(hang_bound(base), base * timeout_scale());
    }

    #[test]
    fn wait_until_returns_once_the_condition_holds() {
        let mut n = 0;
        wait_until("three polls", Duration::from_secs(5), || {
            n += 1;
            n >= 3
        });
        assert_eq!(n, 3);
    }

    #[test]
    #[should_panic(expected = "timed out")]
    fn wait_until_fails_a_condition_that_never_holds() {
        // ELUDITE_TEST_TIMEOUT_SCALE may lengthen this, but never past a fraction of a second times the scale.
        wait_until("never", Duration::from_millis(20), || false);
    }

    #[test]
    fn a_budget_within_its_limit_passes_everywhere() {
        assert_budget("nothing", Duration::ZERO, Duration::from_secs(1));
        assert_memory_budget("nothing", 0, 1);
        assert_at_least("something", 1.0, 1.0);
    }
}
