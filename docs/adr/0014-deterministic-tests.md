# ADR-0014: Tests fail on behavior only: budgets report on CI, hang bounds scale, required tools cannot skip

Status: Proposed, 2026-10-09
Plan reference: PLAN.md section 2 (the performance budgets), CLAUDE.md "Performance budgets" and "Definition of done"

## Context

In the week to 2026-10-09, CI failed on 47 runs for reasons other than billing. Apart from compile and lint
errors, almost none of those failures were defects in the code under test. They came from four patterns:

- **Timing asserts on shared runners.** Each crate had copied its own `assert_budget` (12 Rust copies, one in C#),
  and about 25 more timing and memory asserts in Rust and 12 in .NET bypassed every copy: a click p95 under 150 ms
  against real Chrome, a type-name lookup under 50 ms under Mono, a warm configuration switch under 500 ms. The
  hosted runners missed each one in turn, and each miss became its own one-off "print this budget under CI" commit.
- **Hang bounds that were really budgets.** Waits on real processes (netcoredbg, Mono, CEF, cargo) gave up after 10 s
  whether or not anything was stuck, and "it did not wait" checks compared against a number only just under the wait.
- **Sleeps as synchronization**, and fakes that announced an event before finishing it (the fake host counted a
  test discovery before sending its tests, and a test that waited on the count ended the run between two containers).
- **Skips that pass.** A test whose tool is missing prints "skipped" and passes, so a broken fetch on CI would turn
  every real-adapter test green.

## Decision

- **One policy, one place per ecosystem.** `crates/test-support` (`eludite-test-support`, a dev-dependency only) for
  Rust and `dotnet/tests/Shared/` (`Budget`, `Poll`, linked into every test project) for .NET. Crates re-export or call
  them and never copy them. The MIT agents under `agents/` stay standalone workspaces and keep their own copies.
- **A budget is a performance claim, not a correctness one.** A test checks behavior with ordinary asserts and hands
  only the number to `assert_budget`, `assert_memory_budget` or `assert_at_least` (`Budget.Assert` in C#). These
  assert on a quiet developer machine and report, never assert, on CI (`CI`, `GITHUB_ACTIONS` or `TF_BUILD`) and when
  the load average exceeds the core count. `ELUDITE_BUDGETS=assert|report` forces either. The budgets in CLAUDE.md
  keep their meaning: they hold on a reference machine, and a regression is caught there and by the benchmarks.
- **A hang bound is generous and scales.** Waits that only catch a hang go through `hang_bound` / `wait_until`
  (`Budget.Hang` / `Poll.UntilAsync`): three times longer on CI, or `ELUDITE_TEST_TIMEOUT_SCALE` times. A "did not
  wait" check leaves a wide gap: it asserts far under the wait it would otherwise sit out (half or less), never just
  under it. A stall that a test must finish inside is made long enough on CI that finishing inside it is certain, and
  the proof is the stalled side's silence, not a clock.
- **No fixed sleep before a positive assert.** Wait for the condition, or for a barrier that proves it (a ping after a
  notification on the same ordered stream; a file the worker renames into place when it ends). Fakes signal an event
  only once everything it implies has been sent. Sleeps that prove something did *not* happen are allowed.
- **Required tools cannot skip.** A test that needs an external tool calls `skip(tool, why)`, which prints why, or
  panics when `ELUDITE_REQUIRE` names the tool (or is `all`). CI sets it for each tool it installs on that OS.
- **CI bounds every job and step** (`timeout-minutes`), keeps backtraces (`RUST_BACKTRACE=1`), and the .NET job
  writes TRX reports, names tests running past two minutes, bounds the whole run, and uploads the results.

## Alternatives considered

- **Retrying failed tests** (nextest `retries`, MTP's Retry extension). It turns a flaky failure into a flaky pass
  and hides the defect; MTP's Retry extension is also not under an open-source license. Rejected while the root
  causes can be fixed.
- **cargo-nextest** for process-per-test isolation. Worth it later, but several suites serialize on in-process
  mutexes (`ONE_CHROME`, the netfx fixture) and would need nextest test groups first. Deferred.
- **Dropping the budget tests.** They are the only check of PLAN.md's budgets on a real machine. Kept, gated.

## Revisit when

- A self-hosted reference runner exists: run the budget tests there with `ELUDITE_BUDGETS=assert`.
- The in-process serialization is replaced by nextest test groups: adopt nextest, still without retries.
