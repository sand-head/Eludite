# Brief 0019: Rust through the generic paths, so Eludite can build Eludite

Status: open
Phase: 1 (exit condition)
Plan reference: PLAN.md sections 2 (principles 5, 6), 7 (Rust row), 8, 9, 10 (Phase 1 exit)
Related ADR: ADR-0003
Depends on: briefs 0012 (Workspace window, documents), 0013 to 0015 (editor LSP features), 0017 (build, Output, Error List)

## Goal

Open the Eludite repository itself in Eludite: the Workspace window shows the Cargo workspace (members, targets, source files) beside any .NET solution, Rust files get rust-analyzer through a generic LSP client path that is not Roslyn-specific (diagnostics, completion, hover, signature help, definition, references, rename, code actions, all reusing the brief 0013 to 0015 UI), and Build runs `cargo build` through the same Output window and Error List as MSBuild. After this brief the owner can develop Eludite's shell in Eludite, which is Phase 1's exit condition.

## Files in scope

- `protocol/schemas/` first and alone: `eludite/workspace/open` (a folder or a `Cargo.toml`; the existing solution open stays for .NET), `eludite/workspace/tree` extended with a `kind` per project (`csproj`, `cargo`, `folder`) and Cargo targets, `eludite/build/start` extended with a `system` (`msbuild` or `cargo`); command schemas `eludite.workspace.open_folder`
- `protocol/rust/**`, `crates/lsp/**`: a generic language-server client that speaks plain LSP to a server the shell launches directly (rust-analyzer), sharing the request, cancellation and document-notification code with the host bridge; server registration as data (language id, file globs, command, initialization options)
- `crates/workspace/**`: the Cargo model from `cargo metadata` (members, targets, dependencies) and a plain-folder model
- `crates/eludite/**`: Workspace window nodes for Cargo and folders, language registration for Rust pointing at rust-analyzer, the cargo build path into Output and Error List (parse `cargo build --message-format=json-diagnostic-rendered-ansi` for diagnostics with spans), the status bar language-server slot showing rust-analyzer's state and progress, File > Open Folder
- `crates/editor/**` only for language registration data (the Rust grammar exists since brief 0009)
- `crates/commands/src/**`, `crates/ui/**`, `crates/docking/**` as needed
- `tools/rust-analyzer/` (new): a fetch script pinned to a release with its SPDX id (MIT OR Apache-2.0), discovery beside the executable, `ELUDITE_RUST_ANALYZER`, PATH, then rustup's component
- `docs/briefs/0019-report.md` (new)

Do not touch the .NET host beyond the tree `kind` field if the host serves mixed trees; the Cargo tree is computed in the shell from `cargo metadata`.

## Contract

- The generic client and the host bridge share one abstraction for "a language server for these documents" so the editor features do not know which it is; rust-analyzer is launched per workspace with `cargo metadata`-derived roots; its `$/progress` drives the status bar; crashes restart it with the brief 0007 policy.
- Diagnostics from rust-analyzer (pull or push, whichever the pinned version does) show as squiggles and Error List rows with source `live`; `cargo build` diagnostics show as `build` and dedup with live rows exactly as MSBuild's do.
- The Workspace window for a Cargo workspace: workspace root, members with their kind (bin, lib, example, test, bench) as children, `src/` trees, `Cargo.toml` and `Cargo.lock`; mixed repositories (this one) show the .NET solution and the Cargo workspace as siblings under the folder root.
- Build menu: Ctrl+Shift+B builds the active system (the one owning the active document; both when none), with cargo's output streamed through the same Output pipeline; cancel kills cargo; "Build succeeded" and error counts in the status bar as for MSBuild.
- Every feature from briefs 0013 to 0015 works in a Rust file without C#-specific code paths; where rust-analyzer uses an LSP extension the UI needs (for example `experimental/serverStatus`), document it in `host-rpc.md`'s generic-client section.
- The UI thread never waits on rust-analyzer or cargo.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless tests against a scripted fake generic server: registration, launch, document notifications, progress to the status bar, diagnostics, completion through the shared popup, crash restart; the Cargo model from a recorded `cargo metadata`; cargo diagnostic parsing from recorded JSON into Error List rows with dedup.
- Manual, recorded with three screenshots: open this repository's root folder in Eludite; the Workspace window showing `Eludite.slnx` and the Cargo workspace side by side; open `crates/editor/src/buffer.rs`, see rust-analyzer diagnostics and completion; introduce an error, Ctrl+Shift+B, see the cargo error in the Error List with click-through; revert.

## Budget

- Open this repository to editable text under 1 s; rust-analyzer ready (first diagnostics for the open file) reported, expected 10 to 60 s cold on this workspace.
- Completion popup under 50 ms p95 after rust-analyzer answers; report host and UI latency separately as before.
- Keystroke frame cost under 8 ms p99 while rust-analyzer indexes.

## Exit criterion

1. The manual flow works with screenshots: Eludite editing and building Eludite.
2. All tests green; workspace fmt, clippy, tests green.
3. The report states what remained Roslyn-specific, the rust-analyzer version pinned, and sizes the next briefs (TypeScript and the web stack through the same generic path; F# and VB.NET through the host; the Phase 1 close-out list).

## Out of scope

- Debugging Rust (CodeLLDB, a later brief), cargo test in Test Explorer, crates.io package UI, Cargo.toml completion.
- Any other language than Rust through the generic path (the mechanism must not be Rust-specific).
- Windows and macOS runs.
