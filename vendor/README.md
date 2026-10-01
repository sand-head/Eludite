# vendor/

Pinned copies of third-party crates we modify, per PLAN.md D1 and section 13 risk 5.

Rules:
- Each vendored crate lives in its own directory with a `WHY.md` naming the upstream repository, the pinned commit, the license, and every local change.
- Only low-level, non-visual crates are candidates (text buffer, rope, fuzzy matching). Nothing that decides what a user sees.
- Re-sync happens on a schedule via a script that will live here, never ad hoc.

Empty until the Phase 0 vendoring audit (docs/briefs/0001) reports.
