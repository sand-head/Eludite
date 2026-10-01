# bench/

Performance suite enforcing the budgets in PLAN.md section 9. Runs in CI on every PR that touches `crates/`; a regression over 5 percent blocks merge.

Planned contents:
- Reference solutions of 10, 100 and 400 projects (generated, checked in as scripts that produce them).
- Shell benchmarks: cold start, time to editable text, keystroke to pixel, scroll throughput, resident memory.
- Host benchmarks: solution load, time to first completion, symbol index build.

Empty until Phase 0 spike 1 establishes the harness.
