# Eludite IDE — Master Plan

*Status: proposal, v0.5 (2026-10-02). Scaffold exists; nothing is usable yet. v0.1 open questions are resolved in section 14; v0.3 widens the language scope (section 7); v0.5 records the acceptance of proposals [0001](proposals/0001-agent-debugging-suite.md) (the agent debugging suite) and [0002](proposals/0002-web-browser-window.md) (the Web Browser window).*

Eludite is a native, cross-platform, agent-first IDE. It is .NET-first, not .NET-only: the first-class workloads are .NET in all its languages (C#, F#, VB.NET) including .NET Framework, WebForms and WCF; modern web development (TypeScript, JavaScript and the front-end stack); and Rust. The goal is feature parity with Visual Studio Community and JetBrains Rider for the .NET workload, with the responsiveness and restraint of Zed, a layout and keymap that a Visual Studio user recognizes on day one, and agents as a peer of the human at every surface of the product.

It is built by one person directing many agents. That shapes the plan as much as the product does (section 11).

---

## 1. Vision and non-goals

**Vision.** A developer opens a 400-project .NET Framework solution and is editing with full IntelliSense in seconds, not minutes. Keystrokes never wait on analysis. An agent sits in the same workspace, can build, run tests, set a breakpoint, inspect locals and propose a fix, and the human sees exactly what it did and can veto any step. Windows, Linux and macOS are all first-class for the shell. The IDE is honest about what each platform can and cannot do for legacy .NET.

**Non-goals (for the foreseeable future).**
- Not a web app, not Electron, not Tauri, not a WebView in a native frame. The UI is drawn by our own GPU-accelerated renderer. A browser engine may render the user's web application inside a tool window (the Web Browser window, section 4.9, ADR-0008); it never draws any part of the IDE.
- Not a VS Code extension host. We will not run VS Code extensions. We speak the open protocols those extensions are built on (LSP, DAP, ACP, MCP) so the ecosystem is reachable without the runtime.
- Not a Zed skin. We use Zed's rendering framework and may vendor some of its low-level text crates; we do not inherit its chrome, panels, themes or interaction model (section 8).
- No WYSIWYG designers (WebForms, WinForms, WPF) in the first two years. Markup editing, code-behind generation and preview, yes. Drag-and-drop surfaces, no.
- No cloud dependency. Everything works offline except the model calls the user chooses to make.
- No telemetry by default.

---

## 2. Guiding principles

1. **The UI thread never waits.** Every operation that can take more than a frame runs off-thread or out-of-process, is cancelable, and renders a partial result immediately. Budgets are in section 9 and are enforced in CI.
2. **Out-of-process by default.** Roslyn, MSBuild, debuggers, test runners and agents each run in their own process. A hung analyzer cannot freeze the editor. A crashed debugger loses the session, not the IDE.
3. **One command bus, two callers.** Every user-visible action is a command with a stable ID, a JSON schema, and a typed result. The UI invokes commands. Agents invoke the same commands through the same bus. There is no "agent API" separate from "what the IDE can do."
4. **Familiar before clever.** Default layout, window names, keyboard shortcuts and terminology are Visual Studio's. Modern ideas go in without renaming the old ones.
5. **Protocols over plugins.** Language support is LSP plus a documented extension vocabulary. Debugging is DAP. Tests are the Microsoft Testing Platform (MTP) and VSTest protocols. Agents are ACP. Tooling exposure is MCP. We write the first-party implementations; third parties can replace any of them.
6. **Contracts first, because agents build this.** Every process boundary has a schema checked in before the code on either side. Every subsystem has a corpus of real inputs and golden outputs. An agent working in one crate must be able to finish a task without reading the rest of the repo.
7. **Copyleft core, permissive edges.** The product is GPL. The schemas and extension SDK that third parties must link against are MIT, so a GPL core never becomes a reason someone cannot write an agent, extension or alternative host for it.

---

## 3. Key decisions

### D1. Shell: Rust with GPUI, with our own component layer

The shell (window, rendering, editor core, panels, command bus) is Rust on GPUI, the GPU-accelerated hybrid immediate/retained UI framework extracted from Zed. GPUI is Apache-2.0 and is published independently of the Zed checkout, with Metal on macOS, Vulkan over Wayland/X11 on Linux, and DirectX on Windows.

Why not the alternatives:
- **All-C# with Avalonia.** One language and in-process Roslyn are attractive, but a GC pause or a synchronous Roslyn call on the UI thread is precisely the failure mode Visual Studio is known for. Avalonia with NativeAOT remains the documented fallback if the Phase 0 spike finds GPUI unfit on Windows; the out-of-process topology means switching shells does not touch the hosts, protocols or debugger.
- **C++ with Qt or Skia.** Qt licensing fights a GPL product less than a permissive one, but velocity and safety still favor Rust.
- **Other Rust frameworks (iced, Slint, egui, Makepad).** None has shipped an editor at Zed's scale.

What we take from Zed and what we do not. GPL licensing (D5) makes Zed's GPL-3.0 crates legally available to us, which v0.1 of this plan had ruled out. We split the decision:
- **Take, after a per-crate license audit:** GPUI, and the low-level text infrastructure (rope/sum-tree, buffer and anchor model, fuzzy matcher, tree-sitter glue) if their licenses are Apache or GPL-3.0-compatible. These are months of subtle work that have nothing to do with how the product looks.
- **Do not take:** the `editor`, `workspace`, `ui`, `theme`, `project`, `terminal_view`, agent panel or any crate that decides what a user sees or how panels behave. Those are written fresh against the Visual Studio model (docking, tool windows, menus, status bar) so the result cannot look like Zed by construction.
- Vendored crates live under `vendor/` with a pinned upstream commit and a `WHY.md` explaining what we changed. Upstream drift is a cost we accept knowingly, per crate.
- **Vendor over own.** When a non-visual crate does what we need, vendoring beats rewriting; we only own code we must own (owner's decision, 2026-10-01). The brief 0001 audit's "rewrite" verdicts for the tree-sitter and language layers are therefore provisional: the follow-up is to check whether vendoring `language_core` behind a thin theme abstraction is cheaper than porting it, and to rewrite only what is genuinely coupled to Zed's visuals.

### D2. Process topology

```
┌───────────────────────────────────────────────────────────────┐
│ eludite (Rust, GPUI)                                          │
│  window · docking · editor core · command bus · settings      │
│  tree-sitter highlighting · search · terminal · git (libgit2) │
│  ACP client · MCP server · extension host (WASM)              │
└───────┬────────────┬─────────────┬────────────┬───────────────┘
        │ LSP+ext    │ DAP (local  │ MTP/VSTest │ ACP / MCP
        │            │  or remote) │            │
┌───────▼──────┐┌────▼─────────┐┌──▼─────────┐┌──▼──────────────┐
│ eludite-host ││ debug adapt. ││ test hosts ││ agents          │
│ (.NET)       ││ netcoredbg   ││ MTP server ││ Claude Code via │
│ Roslyn LSP   ││ eludite-netfx││ vstest.con.││ ACP adapter,    │
│ project sys. ││ (Win, ICorDbg││            ││ Codex, Gemini,  │
│ MSBuild eval ││  for .NET FX)││            ││ others          │
│ NuGet, EnC   ││              ││            ││                 │
└──────────────┘└──────────────┘└────────────┘└─────────────────┘
```

`eludite-host` is one long-lived .NET process per solution embedding the Roslyn language server (`Microsoft.CodeAnalysis.LanguageServer`, MIT, from dotnet/roslyn), the MSBuild-based project system, NuGet client libraries and the Edit-and-Continue service. We build it from source at a pinned commit rather than consuming the Azure-feed binaries, so we control the version and avoid the C# Dev Kit's proprietary pieces entirely.

Transport is JSON-RPC over stdio for control, with a pipe or shared-memory side channel for bulk data (semantic tokens for a 20k-line file, full-solution symbol index, test output). Every message is cancelable and carries a solution generation number so stale results are dropped, not rendered. Debug adapters speak DAP over stdio locally or over TCP/SSH remotely; the shell does not care which (D7).

### D3. Protocols

| Concern | Protocol | First-party implementation |
|---|---|---|
| Language intelligence | LSP 3.17 + Roslyn's `roslyn/*` extensions + a small `eludite/*` vocabulary (project tree, build, designer-file regeneration) | eludite-host (C#); tree-sitter in the shell for sub-millisecond highlighting while LSP catches up |
| Debugging | DAP, local or remote | netcoredbg (MIT) for .NET Core/5+; `eludite-dbg-netfx` (new, Windows) for .NET Framework; lldb/gdb/debugpy later |
| Testing | MTP server mode (JSON-RPC) and the VSTest translation-layer protocol | Test Explorer speaks both; MTP preferred |
| Agents in the IDE | ACP (Agent Client Protocol) | Any ACP agent plugs into the agent panel |
| IDE as a tool | MCP server exposed by the shell | The command bus, re-exported with schemas |
| Extensions | WASM component model (wasmtime) | Themes, grammars, small commands, language/debugger registrations |

### D4. Project system and builds

- SDK-style projects: evaluate and build with the user's installed .NET SDK via MSBuild APIs in eludite-host.
- Legacy (non-SDK) `.csproj`, including WebForms, WCF service projects and most .NET Framework code: `dotnet build` cannot build these. On Windows we locate MSBuild from Visual Studio Build Tools (free installer, not our binary). On Linux and macOS we locate or bundle Mono's MSBuild, as Rider does. Reference assemblies come from the `Microsoft.NETFramework.ReferenceAssemblies` packages so design-time analysis works everywhere even where building does not.
- Design-time evaluation never blocks on a full build. A fast cached evaluation pass feeds Roslyn; real MSBuild runs only for builds.
- Solution formats: `.sln`, `.slnx`, `.slnf`. Shared projects, multi-targeting, `Directory.Build.props/targets`, `global.json`, `NuGet.config`.

### D5. Licensing

- **Shell, hosts, debuggers, web tooling: GPL-3.0-or-later.** The owner prefers copyleft, it keeps improvements public, and it unlocks Zed's GPL crates (D1). GPL-3.0 can include MIT, Apache-2.0 and BSD code (Roslyn, netcoredbg, GPUI, libgit2, tree-sitter, wasmtime), so the dependency picture is unchanged.
- **`protocol/` (schemas, generated bindings) and `extension-sdk/`: MIT.** Anyone writing an ACP agent, an MCP client, an alternative language host or a WASM extension against Eludite must be able to do so under any license. This mirrors how Zed keeps its extension API Apache while the editor is GPL.
- **AGPL is not needed now.** It only matters for a network service (a future extension registry or collaboration server). Decide when such a service exists.
- Contributor terms: Developer Certificate of Origin, no CLA. Agents' output is contributed under the same terms by the human who directs them.
- Trademark: "Eludite" and the logo are held separately from the code so forks can exist without confusion.

### D6. Extensibility

WASM extensions in v1, sandboxed, with a capability manifest. Out-of-process .NET or native tool hosts later for heavyweight integrations. No in-process native plugins. A VS-style Extensions dialog with a registry we host.

### D7. Debuggers are remote-capable from day one

Every debug adapter is reached through a transport abstraction (stdio child, TCP, SSH-forwarded). `eludite-dbg-netfx` is written as a DAP server that does not assume its client is on the same machine. That is how cross-platform .NET Framework debugging arrives without a Linux implementation of ICorDebug: Eludite on Linux launches or attaches to the adapter on a Windows box, VM or CI runner, and the debugger windows are identical. Section 4.5 has the longer-term options.

---

## 4. Subsystems

### 4.1 Editor core (shell)
Rope buffer with piece-table-style undo, vendored if Zed's text crates pass audit. Multi-cursor, column selection, folding, bracket matching, inlay hints, CodeLens, semantic highlighting from LSP layered over tree-sitter, diff gutter, inline diagnostics, optional minimap, sticky scroll, ligature-aware shaping. Large-file mode above 10 MB degrades semantic features gracefully. Full Unicode, BOM and line-ending preservation (15-year-old WebForms repos depend on it).

### 4.2 Workspace and solution model (shell + host)
The Workspace window (Solution Explorer in VS terms) with VS semantics: solution folders, project dependencies, References and Packages nodes, Show All Files, nested files (`.designer.cs` and `.aspx.cs` under `.aspx`), file properties (Build Action, Copy to Output). Multi-root fallback without a solution. Project property pages for the ~30 properties people actually touch, editing `.csproj` XML directly and preserving formatting.

### 4.3 Language intelligence (host)
Everything the Roslyn LSP provides: completion with our own ranking, signature help, hover with XML docs, go to definition/implementation/base, find all references, rename, code actions and refactorings (extract method/interface/class, inline, move type, change signature, generate members), diagnostics with analyzers and source generators, CodeLens for references and tests, navigate to decompiled source (ILSpy's engine, MIT), semantic tokens, inlay hints, format, organize usings, EditorConfig. Host-side additions: a solution-wide symbol index so Go To All and call hierarchy are instant on 10k-file solutions.

F# via FsAutoComplete (MIT) and VB.NET via the same Roslyn server are Phase 2 work (section 7). Spike 2 must confirm which VB features the Roslyn language server exposes outside Visual Studio; gaps are filled in eludite-host.

### 4.4 Build and Output
Incremental, cancelable builds producing a binary log rendered into Error List with click-through, and an Output window that handles ANSI and 100k lines without jank. Optional build on save. Parallel project builds, node reuse. Design-time build problems surface as warnings, never modal dialogs.

### 4.5 Debugging
DAP client with VS windows: Locals, Autos, Watch, Call Stack, Threads, Modules, Breakpoints (conditional, hit-count, tracepoints), Exception Settings, Immediate; Memory and Disassembly later. Data tips, pin to source, Run to Cursor, Set Next Statement, Edit and Continue (Roslyn EnC service plus adapter support), Hot Reload for .NET 6+, attach to process, remote attach, Just My Code, symbol server, Source Link.

What an adapter lacks, the shell implements on every adapter: hit counts and Run to Cursor already (brief 0018), then tracepoints, `run_until` (one-shot breakpoints, resume, answer with the stop) and `trace` (install tracepoints, run, answer with the collected lines), per proposal 0001 section 5. Attach to process and the Attach to Process dialog land in Phase 2 (proposal 0001, brief C). Each session has an "Allow agents to drive" toggle (Debug toolbar and status bar, default on, policy-settable) that refuses agents' resuming and mutating commands while off; reads keep working.

Adapters:
- **.NET Core/5+:** netcoredbg (MIT). Fixes go upstream, not into a fork.
- **.NET Framework:** no permissive adapter exists and `vsdbg` is license-restricted to Microsoft products. We write `eludite-dbg-netfx`, a DAP server over the ICorDebug COM interfaces (`mscoree`/`mscordbi`), in Rust using the `windows` crate. Windows-only by nature. Largest single piece of new systems work in the plan.
- **Cross-platform .NET Framework debugging, in order of likelihood:**
  1. Remote DAP to a Windows host (D7). Ships with the adapter; costs nothing extra beyond the transport.
  2. Mono soft debugger adapter for apps that run under Mono (console, libraries, some ASP.NET via XSP). Real but narrow.
  3. ICorDebug under Wine with .NET Framework installed in the prefix. Investigate once (1), (2) exist; expected to be fragile.
- Rust and native via CodeLLDB (MIT) or lldb-dap, JavaScript and browsers via vscode-js-debug (MIT): Phases 1 and 2 per section 7. Python and the rest in Phase 5.

### 4.6 Testing
Test Explorer with hierarchy, filtering, run/debug/profile at any node, live results, per-test output, coverage via `Microsoft.CodeCoverage` or coverlet in the gutter. MTP server mode first; VSTest protocol for xUnit 2, NUnit and MSTest v2 projects that have not migrated.

### 4.7 NuGet
Browse, Installed, Updates, Consolidate; multiple sources; credential providers; lock files; Central Package Management; vulnerability badges. NuGet client libraries in the host.

### 4.8 Source control
Git via libgit2 in the shell: status, staged/unstaged, hunks, blame, log graph, branches, stashes, worktrees (agents use these constantly), conflict editor, PR integration for GitHub, GitLab, Azure DevOps, Forgejo and Gitea (Codeberg), and Tangled. No shelling out to `git` on hot paths.

### 4.9 Web: WebForms, MVC, Razor, Blazor
- `.aspx`, `.ascx`, `.master`, `.ashx`: our own parser (tree-sitter grammar for markup plus control-tree analysis) producing the same partial class the ASP.NET page parser would, feeding Roslyn so IntelliSense works in `<% %>` blocks and code-behind. Automatic `.designer.cs` regeneration on markup change, validated byte-for-byte against VS output over a corpus of real projects. Control type resolution from references and `web.config` `<pages><controls>`.
- `web.config` / `app.config`: schema-aware editing with completion for `system.web`, `system.serviceModel`, `connectionStrings`, `appSettings`; transform preview.
- Launch: IIS Express on Windows (detected, not bundled), Kestrel for modern projects, browser launch and attach: F5 on a web project opens its `launchUrl` in the Web Browser window when the engine is present ("Start in external browser" stays in the Debug menu), and a tab is a debug target for vscode-js-debug (proposal 0002 section 4.4).
- The Web Browser window (View > Other Windows > Web Browser): Chromium through CEF in its own process, `eludite-browser`, started when the window first opens, never at startup (ADR-0008, proposal 0002). Tabs, address bar, DevTools as a tab, a per-workspace profile under `.eludite/browser/`, and the `eludite.browser.*` commands of section 5.8 for both drivers. It renders the user's application only; no part of the IDE is HTML.
- Razor and Blazor: the Razor language server from dotnet/razor (MIT), hosted beside Roslyn.

### 4.10 WCF
- Add Service Reference: drives `dotnet-svcutil` (and `svcutil.exe` on Windows for legacy) with a VS-matching dialog; writes `Reference.cs` and config.
- Service hosting and a WCF Test Client replacement (invoke operations, inspect SOAP) on our own minimal host.
- CoreWCF projects are ordinary SDK projects and work in Phase 1.
- Structured `system.serviceModel` editor with validation.

### 4.11 Other VS windows and features in scope
Find in Files at ripgrep speed plus Roslyn structural search, Replace with preview, Bookmarks, Task List, Error List filters, Properties window on a generic property grid, `.resx` editor (a file opens as its whole set, every culture a column, missing, unused and inconsistent translations, references, changes, Excel exchange, translation by the hosted agent or a provider; proposal 0005), `.settings` editor, T4 via `dotnet-t4`, EditorConfig editor, VS-format snippets, keymap editor with VS, Rider and VS Code presets, VS Dark/Light/Blue themes as defaults, integrated terminal, Server Explorer-lite (ADO.NET connections, browse, query) in Phase 4+, dotnet-counters and dotnet-trace as a basic profiler.

### 4.12 Settings and state
Layered settings (user, workspace, solution), live-reloaded, with a GUI organized like VS Options. Layouts persist per solution. Settings sync via the user's own git repo; no accounts.

---

## 5. Agent-first design

"Agentic-first" is a product stance, not a chat panel.

### 5.1 The command bus is the API
Every action (open file, apply edit, build project, run tests matching filter, set breakpoint, step, evaluate expression, read Error List, rename symbol) is a registered command with an ID, JSON schemas for input and output, a permission class and an audit record. The UI calls commands. The MCP server exposes the same commands with the same schemas. There is no feature the human can use that an agent cannot, and nothing an agent can do that the human cannot see.

### 5.2 Hosting agents
- **ACP client built into the shell.** Any ACP-speaking agent appears in the Agents tool window with streaming output, tool-call rendering, inline diff proposals and permission prompts. Multiple agents run concurrently, each optionally in its own worktree.
- **Claude Code first.** The owner's organization has a Claude Code subscription, so the first agent wired through ACP is Claude Code via its ACP adapter, which reuses the existing Claude Code login rather than an API key. Confirm the subscription terms permit third-party ACP hosts before relying on it for users other than the owner. Gemini CLI, Copilot CLI, Codex and OpenCode follow at zero marginal cost since they already speak ACP.
- **IDE MCP server.** Each hosted agent receives the Eludite MCP endpoint, so it can read diagnostics, build, run tests, drive the debugger and query Roslyn rather than grepping.
- **Native first-party agent: pinned.** Not in scope until ACP hosting is excellent. If built, it uses the same ACP surface internally so it is not privileged.
  - 2026-10-05: the owner lifted the pin for one adapter, `eludite-openai-acp` (brief 0060, ADR-0013): an ACP agent over servers that speak the OpenAI Chat Completions API (llama.cpp, Ollama, vLLM, OpenRouter, OpenAI and the rest), since those models have no agent CLI to wrap. It is a separate MIT process whose only tools are the Eludite MCP endpoint every hosted agent gets, so it is not privileged.

### 5.3 Permission model
Four classes: read (always), edit-in-buffer (shown as pending diff until accepted, or auto-accept per policy), execute (build, test, run; per-workspace policy), dangerous (push, delete, external network; prompt unless whitelisted). Policies are per solution and committable.

`agents-policy.json` has, besides rules by tool name, a `debug` object (`drive`: `allow` default, `prompt`, `deny`, whether agents may resume, mutate or start sessions; `attach`: `prompt` default, `deny`; `evaluate`: `allow` default, `prompt`, `deny`, whether agents may run `evaluate`, `set_variable` and expression tracepoints; reads are always allowed) per proposal 0001 section 5.5, and a `browser` object (`origins`: the allowed navigation origins, default `localhost`, `127.0.0.1`, `::1`, the workspace's launch urls and `file://` under the workspace, navigation elsewhere being `dangerous`; `network_bodies`: `allow` default, `deny`; `evaluate`: `allow` default, `prompt`, `deny`) per proposal 0002 section 5.

### 5.4 Agent-visible state
Open editors and selections, Error List, build output, test results with stack traces, debugger state (frame, locals, breakpoints hit), git status, solution graph. Typed data, never screenshots.

### 5.5 Agent-driven debugging
The headline feature. An agent sets a breakpoint, runs tests to it, inspects locals, evaluates expressions, steps, and presents a hypothesis and fix in the normal debugger windows. The human can take over at any time. Brief 0018 built the foundation (the `eludite.debug.*` commands, one state machine for two drivers, one state both read); [proposal 0001](proposals/0001-agent-debugging-suite.md) (accepted 2026-10-02) fixes the rest: its section 4 gives the rules every debug command follows (one command for both drivers, budgeted outputs, the agent's frame as a parameter, resuming commands settle before they answer, the person always wins, shell-side implementations of what adapters lack, named side effects, everything audited and visible) and its section 5 the command surface (`snapshot`, `stack`, `variables`, `output`, `exception_info`, `wait`, `processes`, `modules`, tracepoints and function breakpoints, exception filters by type, `pause`, `attach`, `restart`, `set_variable`, `set_next_statement`, `run_until`, `trace`, multi-session). Its briefs are listed in its section 8.

### 5.6 Review surface
Agent edits land as reviewable changesets: per-file diffs, accept/reject per hunk, "explain this hunk" against the transcript, and a link from every edit to the tool call that made it. Background agents report into a queue, not into your open editor.

### 5.7 Context quality
Agents get what an IDE knows and a terminal does not: semantic symbol search, call hierarchy, the project graph, test-to-code mapping, and build errors with precise locations. The quality of the IDE's own indexing is the quality of the agent.

### 5.8 Agent control of the browser
An agent drives the Web Browser window's tabs (section 4.9) through `eludite.browser.*` commands modeled on Claude in Chrome: `tabs`, `tab_open`, `tab_close`, `tab_select`, `navigate`, `resize`, `screenshot`, `read_page` (an accessibility tree with stable refs), `find`, `page_text`, `console`, `network`, `network_body`, `wait`, `input`, `form_input`, `evaluate`, `upload`, `storage`, `record`, `devtools`, `open_external`. The person uses the same commands from the window's toolbar, context menu and address bar, sees an "Agent is driving" strip while an agent's call is in flight, and can take over at any moment. The command surface, its differences from Claude in Chrome (deterministic `find`, refs over coordinates, an isolated profile, the origin policy) and its budgets are [proposal 0002](proposals/0002-web-browser-window.md) sections 4, 5 and 7 (accepted 2026-10-02).

---

## 6. .NET Framework, WebForms and WCF: the hard parts, stated plainly

| Need | Status | Plan |
|---|---|---|
| Load and analyze legacy `.csproj` on Windows | Solvable | MSBuild from VS Build Tools + reference assemblies |
| Load and analyze legacy `.csproj` on Linux/macOS | Solvable with caveats | Mono MSBuild, reference-assembly packages, our design-time evaluator as fallback |
| Build legacy projects off Windows | Partial | Mono MSBuild handles most; Windows-only targets (COM, SGen, some web targets) fail with a clear Output message |
| Debug .NET Framework locally | Hard, Windows-only | New `eludite-dbg-netfx` over ICorDebug |
| Debug .NET Framework from Linux/macOS | Accepted for v1 as "not local" | Remote DAP to a Windows host (D7), then Mono soft debugger, then Wine investigation |
| WebForms IntelliSense and designer files | Hard but self-contained | Own ASPX parser and designer generator, corpus-validated |
| WebForms visual designer | Out of scope | Markup editor plus live browser preview |
| IIS Express | Windows-only | Detect, launch, attach; not bundled |
| WCF service references, test client, config | Solvable | svcutil wrappers, own test client |
| Edit and Continue for .NET Framework | Late | After EnC on Core and after the adapter is stable |

Legacy .NET is "edit, analyze, build anywhere; debug on Windows, locally or remotely" for v1. The owner has accepted this.

---

## 7. Languages

Eludite is .NET-first, not .NET-only. Three language families are first-class. First-class means the full set of windows (explorer, Error List, Test Explorer, debugger windows, package manager, run configurations), the keymap and the agent surface all work, and the family has its own corpus and CI job.

| Family | Languages | Intelligence | Debugging | Tests | Packages and projects | Phase |
|---|---|---|---|---|---|---|
| .NET | C#, F#, VB.NET | Roslyn language server for C# and VB; FsAutoComplete (MIT) for F# | netcoredbg; `eludite-dbg-netfx` | MTP and VSTest; Expecto and NUnit for F# | NuGet; `.sln`/`.slnx`/`.csproj`/`.fsproj`/`.vbproj` | C# in Phase 1; F# and VB.NET in Phase 2 |
| Web | TypeScript, JavaScript, HTML, CSS/SCSS/Less, JSON; framework servers for Vue, Svelte, Astro, Tailwind; Razor and Blazor share this stack | typescript-language-server over tsserver (MIT); the HTML, CSS and JSON language services extracted from VS Code (MIT); ESLint; Prettier or Biome; Emmet | vscode-js-debug (MIT) as a DAP adapter: Node, Chrome and Edge attach, browser debugging of ASP.NET front-ends, with the embedded Web Browser window's tabs as the js-debug target (compound launch: Kestrel under netcoredbg, the page under js-debug) | Vitest, Jest, Playwright adapters | npm, pnpm, yarn, bun with a package UI like NuGet's; `package.json` scripts as run targets; workspaces | Phase 2 |
| Rust | Rust | rust-analyzer (MIT or Apache-2.0) | CodeLLDB (MIT) or lldb-dap | `cargo test` and cargo-nextest listing and running in Test Explorer | Cargo workspaces as the project model; `Cargo.toml` completion; crates.io search in the package UI | Basic editing, diagnostics and navigation in Phase 1 (Eludite's shell is Rust, so dogfooding needs it); parity in Phase 2 |

Mixed solutions are the normal case, not an edge case: an ASP.NET project with a `ClientApp` or `wwwroot` front-end shows both in one Solution Explorer, builds both, and has compound launch configurations (start Kestrel, then attach the browser debugger). Rust projects inside a .NET repository (native interop) load beside the solution.

Beyond these, a language costs a tree-sitter grammar, an LSP registration, a DAP registration and test-runner glue, all declarable from a WASM extension. Python, C/C++, Go and others arrive that way in Phase 5. Parity for a family is more than registrations: a project model, a package UI, run configurations and a test adapter are real per-family work, which is why the three above are planned rather than left to extensions.

---

## 8. Visual Studio familiarity and visual identity

Eludite should be recognizable to a VS user in the first five seconds and never mistaken for Zed.

- **Chrome.** Menu bar (File, Edit, View, Git, Build, Debug, Test, Tools, Help; decision 14), optional toolbars, a VS-style status bar with build/debug state, line/column, encoding, line endings, branch, and host memory.
- **Docking.** Tool windows dock, tab, float, auto-hide and pin, with VS's docking guides on drag. Document tabs with pinned tabs and a preview tab. Layouts per solution and named layouts (Design, Debug). This alone makes the product look nothing like Zed, which has fixed docks.
- **Default layout.** Workspace (the Solution Explorer role; see the divergence below) right, Properties below it, Error List and Output bottom, Toolbox collapsed left.
- **Keymap.** VS on Windows and Linux; VS for Mac mapping on macOS with Rider as an option. F5, F9, F10, F11, Ctrl+T, Ctrl+Q, Ctrl+Shift+B, Ctrl+K Ctrl+D and the rest.
- **Themes and type.** Our own VS Dark, Light and Blue defaults with VS's token colors, our own UI font stack and iconography. No Zed themes or icons are vendored.
- **Theme system.** User-installable themes are a planned feature, not just built-in defaults: a documented theme format covering UI chrome and editor token colors, loadable from a file or an extension, with VS and VS Code theme import as a stretch goal. The format is ours; it is not required to be Zed's, though Zed's `theme` crate may be vendored as plumbing if the audit finds it worth it (D1). Lands with the extension system in Phase 2.
- **Dialogs.** New Project backed by `dotnet new` templates, Add Reference, Project Properties, Options, Exception Settings, Attach to Process; same organization as VS.
- **Deliberate divergences.** Solution and project are .NET terms and appear only for .NET artifacts; the directory-level context is a workspace in every menu, message and setting (decision 12). The Solution Explorer window is called Workspace, because the same window will show Cargo workspaces, npm workspaces and plain folders, not only .NET solutions (owner's decision, 2026-10-02); its id is `workspace`, its contents and VS semantics are unchanged, and Ctrl+Alt+L still opens it. No modal dialogs during load, no blocking design-time builds, no "busy" banner; a command palette on Ctrl+Shift+P alongside Ctrl+Q; multibuffers for search results and references; inline agent prompts; a project graph view.

---

## 9. Performance budgets (enforced in CI on a reference machine)

| Metric | Budget |
|---|---|
| Cold start to interactive window | < 300 ms |
| 100-project solution to editable text with syntax highlighting | < 1 s (semantic features stream in after) |
| Keystroke to frame submitted (input, layout, render; excludes waiting for the display's next refresh) | < 8 ms at p99 |
| Keystroke to pixel, end to end | < one refresh interval + 8 ms at p99 (24.7 ms at 60 Hz, 14.1 ms at 165 Hz) |
| Scrolling a 50k-line file | sustained monitor refresh rate |
| Shell resident memory, 100-project solution, 20 tabs | < 400 MB (host memory reported separately in the status bar) |
| Completion popup after trigger | < 50 ms p95 from host; tree-sitter fallback immediately |
| Ctrl+Shift+B to first Output line | < 100 ms |

The two keystroke rows exist because a frame-paced renderer like GPUI only draws when the display asks for a frame, so end-to-end latency includes up to one refresh interval of waiting that no amount of optimization removes (brief 0001 report, section 8). The first row is what we optimize; the second is what the user feels.

Every PR touching the shell runs the benchmark suite; regressions over 5 percent block merge.

---

## 10. Phases

Durations are deliberately omitted. With one human and agents, throughput is set by how many independent, well-specified tasks can run in parallel and how fast the human can review them, not by headcount. Each phase ends in something usable daily; nothing ships half-finished across a phase boundary.

### Phase 0: Spikes and de-risking
Five throwaway prototypes, each runnable by an agent in its own worktree with a one-page brief and a measurable exit:
1. GPUI shell: window, docking prototype, text rendering, a 100k-line buffer at full frame rate on all three OSes. Audit of Zed crates for vendoring.
2. eludite-host: Roslyn LSP built from source at a pinned commit, loading a 200-project SDK solution; measure time-to-IntelliSense.
3. Legacy project load on Linux with Mono MSBuild and on Windows with Build Tools; a WebForms project with IntelliSense in code-behind.
4. ICorDebug proof on Windows from Rust: attach to a .NET Framework process, set a breakpoint, read a local, over a TCP DAP transport.
5. ACP: Claude Code hosted in a GPUI panel through its ACP adapter, calling one Eludite MCP tool (read Error List).
Exit: a written go/no-go on D1, a list of vendored crates, and a sized brief for the .NET Framework debugger.

### Phase 1: Daily driver for modern .NET
Editor core, docking and tool windows, Solution Explorer, Roslyn intelligence, build with Error List and Output, run and debug with netcoredbg, Test Explorer via MTP, git basics, terminal, settings, VS keymap and layout, Agents window with ACP (Claude Code) and the MCP server covering open/edit/build/test/diagnostics. rust-analyzer through the generic LSP path, with Cargo workspaces in the explorer, so the Rust shell can be developed in Eludite too. All three OSes. Exit: the owner develops both the Rust shell and the .NET host in Eludite, with agents working through the Agents window.

### Phase 2: VS Community parity for modern .NET
Full refactoring and navigation set, NuGet UI, project property pages, launch profiles, multi-config and multi-target, Razor/Blazor, CodeLens, decompiled navigation, Find in Files and structural search, resx editor (proposal 0005), snippets, extension system v1 and registry, auto-update, signed installers. The other first-class families from section 7: F# and VB.NET; TypeScript, JavaScript and the web stack with vscode-js-debug and a package UI; Rust at parity with CodeLLDB and cargo test in Test Explorer. Agent-driven debugging (proposal 0001 briefs A to C: inspection depth, run control, attach and policy, needing nothing from Phase 3) and the Web Browser window (proposal 0002 briefs S, A, B, C: the spike, the automation commands, the window, the launch integration). Exit: a Rider or VS user can switch for ASP.NET Core, front-end and console/library work without missing features they use weekly.

### Phase 3: .NET Framework, WebForms, WCF (Windows-first)
Legacy project system, Build Tools and Mono MSBuild, `eludite-dbg-netfx` with remote transport, config tooling, IIS Express, ASPX parsing and designer generation, WCF service references, test client and config editor, EnC on Core. Exit: a real WebForms plus WCF solution loads, builds, runs, debugs and ships from Eludite on Windows; edits, builds and remote-debugs from Linux.

### Phase 4: Agentic depth (overlaps Phase 3)
Test-to-fix orchestration over the agent debugging suite of Phase 2 (proposal 0001 brief F's proving scenario), agent-driven testing of web applications with full-stack breakpoints (proposal 0002 brief D: JavaScript debugging in the Web Browser window beside the server's C#), background agents in worktrees, review changesets, permission policies, multi-agent orchestration, semantic context tools, transcripts linked to edits. Exit: an agent takes a failing test to a reviewed, passing fix without the human leaving the IDE.

### Phase 5: More languages and ecosystem (ongoing)
Python, C/C++, Go and others via extensions; profiler views; Server Explorer; EF tooling; Hot Reload everywhere; Mono and Wine debugging investigations; WinForms/WPF designers only if demand justifies the cost.

---

## 11. Development model: one human, many agents

This is the plan's most important operational section. The architecture in sections 3 and 4 was chosen partly because it decomposes into agent-sized work.

**Repository conventions.**
- A root `CLAUDE.md` with the build commands, the crate map, the invariants (section 2), the budgets (section 9) and the definition of done. Per-crate `CLAUDE.md` files for anything with non-obvious rules (the editor core, the debugger, the ASPX generator).
- Architecture Decision Records in `docs/adr/`. Every D-numbered decision above becomes ADR-0001 onward. Agents are told to read the ADRs before proposing structural changes and to write one when they do.
- Small crates with narrow public APIs so an agent can own one without reading the others. Protocol schemas in `protocol/` are the contract between the Rust and .NET sides; both sides generate bindings from them and neither is edited by hand.

**Task shape.** Work is issued as briefs: goal, the files in scope, the contract it must satisfy, the test that proves it, the budget it must not regress. One brief per worktree. The human reviews diffs and benchmark deltas, not transcripts. Briefs that cannot be stated this precisely are not ready to be delegated and go back to design.

**Verification over trust.** Golden-file corpora for the designer generator, the project evaluator and the config editors, drawn from real open-source WebForms and WCF solutions. Protocol conformance tests replayed against recorded LSP, DAP, MTP and ACP sessions. Headless GPUI tests for layout and input. The benchmark suite as a merge gate. CI on all three OSes from the first week of Phase 1. An agent's PR that lacks a test for its change is rejected by policy, not by review.

**Dogfooding as the forcing function.** Phase 1's exit criterion is that Eludite hosts the agents that build Eludite. Every gap in the Agents window, the MCP tool surface or the permission model is felt immediately by the only developer.

**Review bandwidth is the bottleneck.** Parallelism is capped by what one person can review well in a day. Prefer a few large, well-tested PRs per day over many small ones. Keep a public roadmap so outside contributors, human or agent, can pick up briefs.

**Bus factor.** Everything is in the repo: ADRs, briefs, corpora, benchmarks, release scripts. The project must be forkable by a stranger on day one.

---

## 12. Repository layout

```
Eludite/
  CLAUDE.md
  LICENSE                GPL-3.0-or-later
  crates/                Rust workspace (shell)
    eludite/             binary: entry, window, layout
    docking/             tool windows, document tabs, layouts
    editor/              buffer, view, input
    commands/            command bus, schemas, audit
    workspace/           solution/project model (client side)
    lsp/ dap/ acp/ mcp/  protocol clients and servers
    git/                 libgit2 wrapper
    terminal/
    extensions/          wasmtime host
    ui/                  widgets, themes, keymaps, icons
    browser/             eludite.browser.* commands over CDP, engine-neutral (proposal 0002)
    resx/                the .resx model: entries with byte ranges, splices, rules, cultures, sets (proposal 0005)
  vendor/                pinned Zed crates with WHY.md each
  dotnet/                .NET solution (Eludite.slnx, hosts)
    src/Eludite.Host/    Roslyn LSP embedding, project system, NuGet, EnC
    src/Eludite.Web/     ASPX parser, designer generator, config schemas
    src/Eludite.Wcf/     svcutil driver, test client
    src/Eludite.TestBridge/ MTP and VSTest clients
    tests/               one xunit v3 project per src project
  debuggers/
    netfx/               eludite-dbg-netfx (Rust, Windows)
  browsers/
    chromium/            eludite-browser: CEF browser process (proposal 0002, ADR-0008)
  protocol/              MIT: schemas, generated bindings
    cdp/                 pinned Chrome DevTools Protocol JSON and the generated domain types
  extension-sdk/         MIT: WASM extension API
  corpus/                real solutions used as golden tests (submodules)
  bench/                 performance suite and reference solutions
  tools/                 pinned external tools located at run time (Roslyn LS, netcoredbg, rust-analyzer, cef/)
  docs/
    PLAN.md
    adr/
    briefs/
```

Build: Cargo for the shell, `dotnet` for hosts, one `cargo xtask` entry point. GitHub Actions on all three OSes with the benchmark gate. Nightly builds from the first week of Phase 1.

---

## 13. Risks

1. **GPUI on Windows.** Youngest backend. Mitigation: Phase 0 spike; Avalonia fallback preserved by the process topology.
2. **The .NET Framework debugger.** Nobody has shipped a permissive one. Mitigation: start in Phase 0, scope v1 to launch/attach, breakpoints, stepping, locals, watch, exceptions; defer EnC and mixed-mode; remote transport from the start so Linux users get it too.
3. **Roslyn LSP churn.** Built for VS Code and changes with it. Mitigation: pinned commits, our extensions in a separate layer, upstream contributions.
4. **Legacy project edge cases.** Twenty years of `.csproj` variants. Mitigation: the corpus, treated as regression tests.
5. **Vendored Zed crates drift.** Mitigation: vendor only low-level text crates, pin, and document each change; re-sync on a schedule, not ad hoc.
6. **Scope versus one reviewer.** VS and Rider are thousands of engineer-years. Mitigation: phase gates with daily-driver exits, a public "not doing" list, and briefs small enough to review in an hour.
7. **Agent-generated code quality.** Mitigation: section 11; tests and benchmarks are policy gates, not suggestions.
8. **Agent protocol churn.** ACP and MCP are young. Mitigation: the command bus is ours; protocols are adapters over it.
9. **Subscription terms.** Hosting Claude Code via ACP under an org subscription is fine for the owner's own use; verify before documenting it as the recommended path for others.

---

## 14. Decisions recorded (from v0.1 open questions)

| # | Question | Decision |
|---|---|---|
| 1 | License | GPL-3.0-or-later for the product; MIT for `protocol/` and `extension-sdk/`. AGPL deferred until a network service exists. |
| 2 | Team | One human plus agents. Section 11 governs how. Durations removed from phases. |
| 3 | Legacy debugging off Windows | Accepted for v1: edit and build anywhere, debug on Windows. Remote DAP (D7) is the near-term cross-platform path; Mono and Wine are later investigations. |
| 4 | Shell | Rust + GPUI. Vendor Zed's low-level text crates if licenses permit; write all visible UI fresh against the VS model (section 8). |
| 5 | Native agent | Pinned. Claude Code via ACP is the first hosted agent, using the owner's organization subscription. |
| 6 | Name | Niello. No software collisions found in a search; check crates.io, the GitHub org name and a domain before the first release. |
| 7 | Language scope (v0.3) | .NET-first, not .NET-only. All .NET languages, the web stack and Rust are first-class per section 7; Rust basics land in Phase 1 for dogfooding. Other languages via extensions in Phase 5. |
| 8 | Vendoring policy | Vendor over own when owning is not required (D1). Brief 0001's vendor list (sum_tree, rope, text, clock, fuzzy) is accepted; its rewrite verdicts are re-examined with that bias. |
| 9 | Themes | A user-installable theme system is in scope for Phase 2 (section 8). Our own format and defaults; Zed's theme plumbing may be vendored but its look is not. |
| 11 | Workspace window | The Solution Explorer window is named Workspace (id `workspace`) so one window serves .NET solutions, Cargo and npm workspaces and folders; VS semantics kept (section 8). |
| 12 | Workspace terminology | "Solution" and "project" are .NET words. The user opens a folder as a workspace, or a single file; the whole-directory context is a workspace everywhere in the UI: File > Open Workspace..., Close Workspace, "No workspace is open", workspace settings (owner's decision, 2026-10-02). "Project" is the generic word for a buildable unit inside a workspace, in any language: a .NET project, a Cargo package, an npm package; F5 starts the startup project, chosen among them. A .NET solution file is only a source of information (which .NET projects, their order and folders, the configuration mapping); no menu item, dialog, command or message offered to the user is built around a solution, since it is not cross-language (owner's decisions, 2026-10-04; brief 0054). |
| 10 | Name (v0.4) | Eludite, replacing Niello, 2026-10-02. Repository sand-head/Eludite. |
| 13 | Embedded browser engine (v0.5) | Chromium through CEF, out of process (`eludite-browser`), CDP as the automation substrate, the command layer engine-neutral so Servo can follow; the engine renders user content only and never the IDE. [ADR-0008](adr/0008-embedded-browser-cef.md), proposal 0002, 2026-10-02. |
| 14 | Menu bar | Visual Studio's names and order, trimmed to nine menus: File, Edit, View, Git, Build, Debug, Test, Tools, Help. No Project, Analyze or Extensions menu; Window's items (Float, Dock, Auto Hide, Hide, Reset Window Layout) sit at the end of View; the forge windows sit in Git. A menu holds commands that exist or that a planned brief registers, not Visual Studio's full list (owner's decision, 2026-10-04). |
| 15 | ResX | A `.resx` opens as its resource set in one grid, there is no separate manager window (owner's decision, 2026-10-05); the file model in the shell (`eludite-resx`, splices that keep every untouched byte), the project knowledge and the designer generation in the host; the ResX Resource Manager extension's `{Invariant}` marker and reference patterns are kept for interchange (proposal 0005, accepted 2026-10-06). |

**Still open.** Which Zed crates actually pass the vendoring audit (Phase 0 output), and whether the Phase 0 GPUI spike on Windows clears the bar.
