# Briefs

A brief is the unit of delegated work. Work is issued as briefs so that one agent, in one worktree, can finish a task without reading the rest of the repo, and so the human can review a diff and a benchmark delta instead of a transcript (PLAN.md section 11).

## Format

Every brief is `docs/briefs/NNNN-short-slug.md` and has these sections, in this order:

1. **Goal.** One paragraph. What exists when the brief is done that does not exist now.
2. **Files in scope.** The paths the agent owns. The agent edits nothing outside this list.
3. **Contract.** The interfaces, schemas, protocols and versions the work must satisfy. Link to the schema in `protocol/` or the ADR.
4. **Proving test.** The test, command or procedure that demonstrates the goal, and how to run it.
5. **Budget.** The performance and resource limits the work must not regress, from PLAN.md section 9, plus any brief-specific limits.
6. **Exit criterion.** Measurable conditions, each checkable as pass or fail. Include the deliverables to hand back.
7. **Out of scope.** An explicit list of things the agent must not do, even if they look useful.

Optional header lines: Status (open, in progress, done), Plan reference, Depends on.

## Rules

- One brief per git worktree. Name the branch after the brief, for example `brief/0002-eludite-host-roslyn`.
- Briefs are written so a fresh agent with no other context can execute them. Name commands, versions, paths and thresholds.
- A brief that cannot be stated this precisely is not ready to be delegated. It goes back to design, and the result is an ADR or a sharper brief.
- Exit criteria are measurable. "Works well" is not one. "p95 under 50 ms over 1000 runs on the reference machine" is.
- Spike briefs produce throwaway code plus a written report. Production code lands through a later brief.
- If the agent finds the brief cannot be satisfied as written, it stops and reports. It does not widen scope.
- When a brief is done, mark its status and link the PR. Do not delete briefs.
- Follow CLAUDE.md for the definition of done, except where a spike brief says throwaway code is exempt from tests it would not need.

## Index

| Brief | Title | Phase | Result |
|---|---|---|---|
| [0001](0001-gpui-shell-and-docking-spike.md) | GPUI shell and docking prototype, Zed vendoring audit | 0 | [report](0001-report.md): Linux GO (provisional), Windows GO (provisional; cold start 360 ms over budget), macOS pending |
| [0002](0002-eludite-host-roslyn-spike.md) | `eludite-host` with Roslyn, time-to-IntelliSense | 0 | [report](0002-report.md): Linux and Windows done, D2 stands; Windows loads the solution about 2x slower |
| [0003](0003-legacy-project-load-spike.md) | Legacy project load, WebForms code-behind IntelliSense | 0 | [report](0003-report.md): Linux done with user-space Mono, 27/29 load, completion passes; Windows done, 0 getItem differences, 23/29 (web targets missing), completion passes |
| [0004](0004-icordebug-dap-spike.md) | ICorDebug proof over TCP DAP | 0 | [report](0004-report.md): done on Windows; attach 34 to 41 ms, hit to stopped about 1 ms, 12 MB; Rust plus `windows` is workable; second-machine run owed |
| [0005](0005-acp-claude-code-spike.md) | Claude Code via ACP in a GPUI panel, one MCP tool | 0 | [report](0005-report.md): Linux pass end to end; adapter is @agentclientprotocol/claude-agent-acp |
| [0006](0006-native-claude-acp-adapter.md) | Native Rust ACP adapter for Claude Code (no Node) | 0/1 | [report](0006-report.md): done on Linux; no Node, 0005 panel unchanged, 2.3 MB binary; Windows: finds `claude.exe` with no Node |
| [0007](0007-host-lsp-bridge.md) | Production LSP bridge between the shell and eludite-host | 1 | [report](0007-report.md): done on Linux; lifecycle under eludite/host/*, warming fixes referenced-project completion |
| [0008](0008-docking-production.md) | Docking and tool windows in the production shell | 1 | [report](0008-report.md): done on Linux; cold start ~100 ms, 19/19 real-input steps |
| [0009](0009-editor-core.md) | Vendor Zed's text crates and build the editor core | 1 | [report](0009-report.md): done on Linux; speed budgets pass, memory on a 100k-line file 249 MB vs 150 MB budget (follow-up) |
| [0010](0010-rename-to-eludite.md) | Rename the project to Eludite | 1 | [report](0010-report.md): done on Linux; full Linux suite green |
| [0011](0011-editor-memory.md) | Bring editor memory under budget | 1 | [report](0011-report.md): done on Linux; 100k-line C# 98 MB idle (was 241), growth 8 MB; mimalloc for tree-sitter only |
| [0012](0012-open-solution.md) | Open a solution end to end | 1 | [report](0012-report.md): done on Linux; editable in 16 ms, tree 0.5 s, first diagnostics 2.4 s on Eludite.slnx |
| [0013](0013-completion-and-hover.md) | Completion, hover and signature help in the editor | 1 | [report](0013-report.md): done on Linux; popup visible 12 ms p50, 29 ms p95 |
| [0014](0014-navigation.md) | Go to definition, Find All References and Error List filtering | 1 | [report](0014-report.md): done on Linux; F12 4 ms; references 508 ms p95 (Roslyn's fixed 500 ms batching) |
| [0015](0015-rename-and-code-actions.md) | Rename, code actions and the workspace-edit applier | 1 | [report](0015-report.md): done on Linux; light bulb 121 ms p95, rename preview 170 ms p95 |
| [0016](0016-agents-window.md) | The Agents window in the production shell | 1 | [report](0016-report.md): done on Linux; Claude fixed a real error through a pending-change review; ready 0.65 s |
| [0017](0017-build.md) | Build with Output and Error List | 1 | [report](0017-report.md): done on Linux; first Output line 18 ms, Error List rows 2 ms after finish |
| [0018](0018-debug.md) | Run and debug with netcoredbg | 1 | [report](0018-report.md): done on Linux; F5 to first stop 180 ms warm, step 12 ms p95 |
| [0019](0019-rust-workspace.md) | Rust through the generic paths, so Eludite can build Eludite | 1 | [report](0019-report.md): done on Linux; Eludite edits and builds Eludite; rust-analyzer ready 7 s warm |
| [0020](0020-integration-pass.md) | Integration pass after build and debug | 1 | [report](0020-report.md): done on Linux; F5 builds first, settings store and Options dialog, Debug output source |
| 0021 | Rotated auto-hide strip titles (no brief file; merge 89569e5) | 1 | done on Linux |
| [0022](0022-mono-debug-adapter.md) | The Mono soft-debugger adapter: .NET Framework debugging on Linux and macOS (proposal 0001 E1) | 2 | [report](0022-report.md): done on Linux; `eludite-dbg-mono` (C# on Mono.Debugging.Soft under Mono), adapter chosen by target framework and platform |
| [0023](0023-browser-automation-read.md) | Browser automation over CDP against an external Chrome: tabs, navigation, reading the page (proposal 0002 A, first half) | 2 | [report](0023-report.md): done on Linux; screenshot 52 to 67 ms p95, read_page on 5,000 items 241 to 299 ms p95 |
| [0024](0024-browser-automation-act.md) | Browser automation over CDP: acting on the page, the browser policy, transcript thumbnails and the fake-agent proof (proposal 0002 A, second half) | 2 | [report](0024-report.md): done on Linux; ADR-0009 escalation; input click 112 to 125 ms p95 (wait_ms 100), fake-agent proof 0.8 to 1.0 s |
| [0025](0025-debug-inspection-depth.md) | Inspection depth for the agent debugging suite (proposal 0001 A) | 2 | [report](0025-report.md): done on Linux; snapshot 3 to 9 ms p95 (fake), summary 7.6 KB at the corpus stop, wait wakes in 2.5 to 11 ms; netcoredbg not run |
| [0026](0026-debug-run-control.md) | Run control for the agent debugging suite: tracepoints, run_until, trace, function breakpoints, exception types, set_variable (proposal 0001 B) | 2 | [report](0026-report.md): done on Linux; emulated tracepoint 6.3 to 7.0 ms per hit (eludite-dbg-mono, emulation forced), run_until +1 to 5 ms over continue; netcoredbg not run. Merged after 0029: user function breakpoints and the Rust panics row go in one `setFunctionBreakpoints` list; lldb-dap's `setVariable` `result` read as `value` |
| [0027](0027-debug-attach-and-policy.md) | Attach, restart and the debug policy for the agent debugging suite (proposal 0001 C) | 2 | [report](0027-report.md): done on Linux; attach to a waiting Mono TestApp to the first stop 0.27 to 0.32 s, `processes` 5 to 9 ms p95, an interrupted wait answers in 2 ms; the guide [docs/agents/debugging.md](../agents/debugging.md) is the MCP resource `eludite://guides/debugging`; netcoredbg not run |
| [0028](0028-debug-multi-session.md) | Multiple debugging sessions: `session` on every command, compound launch, session selectors (proposal 0001 D) | 2 | [report](0028-report.md): done on Linux; a two-project compound to both running 1.16 to 1.23 times one launch, frame p99 7.4 to 8.0 ms with two sessions stopping by turns (one session: 7.1 to 8.1 ms), two `eludite-dbg-mono` sessions to their breakpoints 0.31 to 0.34 s; netcoredbg not run |
| [0029](0029-rust-debug-adapter.md) | Debug Rust with lldb-dap: Cargo launch configuration, Rust formatters, Set as Startup Project on packages (proposal 0001 E2) | 2 | [report](0029-report.md): done on Linux; lldb-dap 18.1.3, launch to first stop 0.57 to 0.78 s, `next` 7 to 8 ms p95, a page of a 10,000-element `Vec` 10 to 18 ms p95; Rust formatters need two LLDB 18 compatibility lines |
| [0030](0030-debug-proving-scenario.md) | The agent debugging proving scenario: a seeded-bug corpus, the scripted fake agent on every platform, the recorded Claude Code run (proposal 0001 F) | 2/4 | [report](0030-report.md): done on Linux under eludite-dbg-mono; the scripted agent reaches each faulting statement in 7 debug calls (1.0 to 1.4 s); Claude Code named all nine correctly but stopped on the statement within eight calls in 3 of 9 runs (it confirms after or at the symptom); [recorded run](0030-run/); netcoredbg not run |
| [0031](0031-cef-offscreen-spike.md) | Spike: CEF offscreen rendering into GPUI, out of process (proposal 0002 S) | 2 | [report](0031-report.md): done on Linux (Xvfb, software rendering); `eludite-chromium` renders into a memfd ring the shell draws with `img`; engine 60 fps, upload 1.15 ms + img paint 1.30 ms p50 per 1600x1000 frame, frame p99 108.8 ms under lavapipe (110.0 ms without uploads); no GO by the rule, fallback: partial uploads from dirty rectangles; owner's GPU run decides |
| [0032](0032-web-browser-window.md) | The Web Browser window: tabs, address bar, DevTools, dialogs, the Agent is driving strip, record, privacy switches (proposal 0002 B) | 2 | [report](0032-report.md): done on Linux (Xvfb, software rendering); window open to first page pixel 110 ms (engine running), 456 ms cold; click to frame p95 19.4 ms; shell +28 MB; no request on `about:blank` in a 10 s net log; 256x256 tiles cut the browser's CPU per frame 2.9 to 0.5 ms on a mostly still page; frame-cost verdict still needs the owner's GPU run |
| [0033](0033-dap-conformance-corpus.md) | The DAP conformance corpus: recorded adapter sessions replayed through the shell on every platform (proposal 0001 G) | 2 | [report](0033-report.md): done on Linux; 8 recordings (Mono 4, lldb-dap 4, 240 KB, goldens 448 KB) replay through the headless shell in 0.36 to 0.67 s each, the corpus in 1.5 to 4.8 s; CI re-records Mono and lldb-dap on Linux and fails on drift; netcoredbg's 3 scenarios await CI's recording |
| [0034](0034-debug-suite-tuning.md) | Tuning the agent debugging suite from the proving run: guide, tool descriptions, compact answers, ACP usage events (proposal 0001 follow-up) | 2/4 | [report](0034-report.md): done on Linux under eludite-dbg-mono; Claude Code stopped on the faulting statement within eight debug calls in 6 of 9 runs (brief 0030: 3 of 9) and on it at all in 9 of 9 (was 3); the misses were conditional breakpoints that never stopped and did not say so; the scripted agent needs 6 debug calls (was 7), `toggle_breakpoint` answers 59 to 401 B; turn usage from `eludite-claude-acp`'s ACP `usage_update` in the Agents window; [recorded run](0034-run/); netcoredbg not run |
| [0035](0035-test-explorer.md) | Test Explorer over MTP, VSTest and cargo test, with Debug Test through the suite (Phase 1 close-out) | 1/2 | [report](0035-report.md): done on Linux; xunit.v3 and MSTest over MTP server mode, xunit 2 and NUnit over VSTest's translation layer (both hand-written, no new dependency), Rust through `cargo test` with libtest parsed; 100-test discovery 0.56 to 0.66 s, the 100-test tree drawn in 3 to 4 ms, Run All of the corpus about 5.5 s (.NET 4.5 to 5.4 s, Rust 0.3 s), first result row under 1 ms after it arrives; Debug Test to the first stop 1.4 to 1.8 s (Rust, lldb-dap) and 1.9 to 2.1 s (net472 under eludite-dbg-mono); MTP 2.4 needs `TESTINGPLATFORM_TELEMETRY_OPTOUT`; [recorded run](0035-run/); netcoredbg not run |
| [0036](0036-debug-breakpoint-failures.md) | Breakpoints that cannot stop say so, Mono name resolution and the pending-breakpoint race, third recorded run (proposal 0001 follow-up) | 2/4 | [report](0036-report.md): done on Linux under eludite-dbg-mono; refused breakpoints and rejected conditions are reported (`breakpoints_failed`, `points_failed`, `toggle_breakpoint`'s `message`); `eludite-dbg-mono` resolves `Coin.Quarter` as Visual Studio does and binds breakpoints set while types load (10 of 10); Claude Code stopped on the faulting statement within eight debug calls in 8 of 9 runs (0034: 6 of 9; 0030: 3 of 9) with 3 to 6 debug calls, $9.14 for the nine (0034: $10.64); [recorded run](0036-run/); netcoredbg not run |
| [0037](0037-browser-launch-integration.md) | Launch integration: F5 on a web project opens it in the Web Browser window (proposal 0002 C) | 2 | [report](0037-report.md): done on Linux (Xvfb); the page opens 0.7 to 0.8 ms after Kestrel's listening line with a fake session (budget 500 ms), 448 to 897 ms with the real `dotnet` and the embedded engine starting cold, 190 ms on a restart with it running; Ctrl+F5 on the web corpus project to the page 5.4 to 5.8 s with the host's build; `session.browser` and the start's answer carry the tab; netcoredbg not run |
| [0038](0038-javascript-debugging.md) | JavaScript debugging in the Web Browser window with vscode-js-debug: attach to a tab, compound with the server, source maps, child sessions (proposal 0002 D, proposal 0001 E3) | 2/4 | open |
| [0039](0039-browser-packaging-linux.md) | Packaging the browser engine on Linux: the layout beside `eludite`, discovery, the sandbox rule and the no-sandbox opt-in (proposal 0002 E, Linux half) | 2 | open |
| [0040](0040-git-changes.md) | Git basics: the Git Changes and Git Repository windows, status decorations, Compare with Unmodified, branches, sync, stashes and worktrees over libgit2, `eludite.git.*` for agents (Phase 1 close-out) | 1 | [report](0040-report.md): done on Linux; no `git` process; `status` 2.3 ms p95 from the cache on 10,000 files with 100 changes (first status 30 ms), 1,000 changed files drawn in 5.4 ms at worst, Compare with Unmodified of 5,000 lines 16 ms, the change margin 120 ms idle + 2 ms; the watcher polls (`notify` is not in the build); https and ssh remotes need libgit2 features that add dependencies; [screenshots](0040-run/screenshots/) |
| [0041](0041-integrated-terminal.md) | The integrated terminal: a PTY and `alacritty_terminal` in our own GPUI view, profiles, shell integration, links, `eludite.terminal.*` for agents (Phase 1 close-out) | 1 | open |
| [0042](0042-find-in-files.md) | Find in Files and Replace in Files at ripgrep speed, the Find Results windows, Replace with preview through the review view, `eludite.search.*` (Phase 1 close-out) | 1 | open |
| [0043](0043-agents-markdown.md) | Markdown in the Agents window's transcript: `pulldown-cmark`, one row per top-level block while it streams | 2 | done on Linux; the tooltips use the same parser |
| [0044](0044-custom-title-bar.md) | Eludite draws the main window's title bar: the menu bar in it, the title, the caption buttons, the Linux frame (ADR-0010) | 2 | done on Linux; macOS and Windows by CI |
| [0045](0045-git-https-and-ssh.md) | Git over https and ssh: git2's `https` and `ssh` features, the credential prompt, proxies, local test servers (follow-up to brief 0040) | 1 | open |

Guides for agents: [docs/agents/debugging.md](../agents/debugging.md) (brief 0027), how an agent drives Eludite's
debugger, also served to hosted agents as the MCP resource `eludite://guides/debugging`; [docs/agents/git.md](../agents/git.md)
(brief 0040), how an agent reads the status, stages, commits and what needs permission, served as `eludite://guides/git`
beside the live resource `eludite://git/status`.

The Phase 0 exit is a written go/no-go on ADR-0001, a list of vendored crates, and a sized brief for the .NET Framework debugger.

Phase 0 is complete on Linux (2026-10-01). The Windows runs are listed in [windows-checklist.md](windows-checklist.md); they ran on 2026-10-03.

A machine with no display can still run the shell: `crates/eludite/tools/xvfb-linux.sh OUT_DIR` draws it on an Xvfb screen with Mesa's software Vulkan and writes a screenshot, and the XTest drivers under `crates/eludite/tools/` run against that display. Timings there are not the reference machine's; use it to prove flows and take screenshots.
