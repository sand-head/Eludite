# Brief 0066: Semantic highlighting and inlay hints from the language servers

Status: in progress
Phase: 2 (PLAN.md section 10: an IDE on par with Visual Studio and Rider for .NET work; brief 0050's report: "semantic tokens and inlay hints not in the shell for any language yet")
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 6, 12), 4.1 (the editor: tree-sitter first paint, LSP layers on top), 4.3 (LSP through the host and the generic servers), 4.12 (settings)
Related ADRs: ADR-0002 (process topology: Roslyn stays in the host, rust-analyzer, FsAutoComplete and the web servers are processes), ADR-0003 (protocols: plain LSP 3.17, no bespoke hooks), ADR-0014 (tests fail on behavior only)
Depends on: brief 0007 (the host bridge), brief 0009 (the editor's decoration hooks), brief 0019 (generic servers), brief 0050 (several servers per file), brief 0052 (CodeLens: the request, debounce and stale-drop pattern this brief copies), brief 0063 (VB and F#).

## Goal

A person opens a C#, Visual Basic, F#, Rust or TypeScript file and sees what Visual Studio and Rider show: tree-sitter colors at once, then the language server's classifications painted over them (class, record and delegate names, interfaces, structs, enums, type parameters, locals, parameters, fields, properties, methods, extension methods, static members, namespaces, string escapes, regex and JSON-in-string tokens, `#if`-excluded code grayed), unnecessary code (unused `using`s, unread assignments: `DiagnosticTag.Unnecessary`) faded, and inlay hints (parameter names at call sites, inferred types of `var`, lambda parameters and `let` bindings) drawn inline as muted chips that are not text: the caret skips them, copy never includes them, line numbers and the layout of lines do not change. Everything stays correct while the person types (the layers move with the text) and nothing stale is ever painted.

## Files in scope

**Protocol (schemas first).**
- `protocol/rust/src/lsp.rs`: the typed `textDocument/semanticTokens/full`, `/full/delta` and `/range`, `textDocument/inlayHint` and `inlayHint/resolve` (params, `SemanticTokens`, `SemanticTokensDelta`, `SemanticTokensEdit`, `InlayHint`, `InlayHintLabel`, `InlayHintLabelPart`), each keeping unknown members in `extra`.
- `protocol/rust/src/host.rs`: the five methods in `FORWARDED_TYPED_REQUESTS` (semanticTokens full and range move there from the untyped list), `eludite/semanticTokens/refresh` and `eludite/inlayHint/refresh` in `HOST_TO_SHELL` with their notification types and params (`{ eluditeGeneration }`).
- `protocol/schemas/host/semantic-tokens.json`, `semantic-tokens-delta.json`, `semantic-tokens-range.json`, `inlay-hint.json`, `inlay-hint-resolve.json`, `semantic-tokens-refresh.json`, `inlay-hint-refresh.json` (new); `protocol/schemas/host-rpc.md` (the method tables, the client capabilities, the configuration answers, the refresh rows); `protocol/rust/src/schema_tests.rs` (conformance of each).
- `protocol/schemas/editor-toggle-inlay-hints.input.json` and `.output.json` (new); `protocol/schemas/settings.json` (the settings below, additive).

**Language-server client (`crates/lsp`).**
- `crates/lsp/src/semantic.rs` (new, `pub mod semantic`): the legend and the provider's options from `ServerCapabilities.semanticTokensProvider`, decoding the relative five-integer encoding to absolute tokens, applying a delta's edits, classifying a token type and modifiers (LSP standard names, Roslyn's custom names such as `recordClass`, `extensionMethod`, `stringEscapeCharacter`, `excludedCode`, `regex*`, `json*`, and Visual Studio's classification names such as `class name`, `static symbol`, `string - escape character`) into `TokenClass`.
- `crates/lsp/src/inlay.rs` (new, `pub mod inlay`): a hint's display text (string or label parts, with the paddings) and kind, and the tooltip text after resolve.
- `crates/lsp/src/connection.rs`, `client.rs`, `server.rs`, `lib.rs`: `Event::SemanticTokensRefresh` and `Event::InlayHintRefresh` from the host's relays and from a generic server's `workspace/semanticTokens/refresh` and `workspace/inlayHint/refresh` (still answered `null`); the generic client's capabilities gain `textDocument.semanticTokens`, `textDocument.inlayHint`, `workspace.semanticTokens.refreshSupport`, `workspace.inlayHint.refreshSupport` and `publishDiagnostics.tagSupport`.
- `crates/lsp/src/fanout.rs`: `offers` knows the delta, range and resolve requests.
- `crates/lsp/src/servers.json`: TypeScript's inlay hint preferences (`typescript.inlayHints.*`, `javascript.inlayHints.*`) in its settings.

**Host (`dotnet/src/Eludite.Host/Lsp/` only).**
- `LspProxy.cs`: the forwarded lists, the client capabilities passed to Roslyn (semantic tokens with full, delta and range, `augmentsSyntaxTokens`; inlay hints with resolve; both refresh supports; `publishDiagnostics.tagSupport`), Roslyn's `workspace/semanticTokens/refresh` and `workspace/inlayHint/refresh` relayed to the shell with the generation, and the configuration answers turning Roslyn's inlay hints on for C# and Visual Basic (`csharp|inlay_hints.dotnet_enable_inlay_hints_for_parameters` and the rest the pinned server reads: `dotnet_enable_inlay_hints_for_literal_parameters`, `_indexer_parameters`, `_object_creation_parameters`, `_other_parameters`, `csharp_enable_inlay_hints_for_types`, `_implicit_variable_types`, `_lambda_parameter_types`, `_implicit_object_creation`). Tests in `dotnet/tests/Eludite.Host.Tests/` (the lists, the capabilities, the configuration answers, the relays).

**Editor (`crates/editor`).**
- `crates/editor/src/semantic.rs` (new): `SemanticRows`, the server's colored spans per buffer row, moved through edits like `LineHighlights` (spans left of an edit stay, right of it shift, inside it go), and the request pipeline (`CodeLensPipeline` reused).
- `crates/editor/src/inlay.rs` (new): `InlayHint` (offset, label, kind, paddings, tooltip), anchored; the row's display string with the hints inserted and the conversions between buffer columns and display offsets that skip them; the request pipeline for the visible rows plus a margin.
- `crates/editor/src/view.rs`, `intellisense.rs`, `lib.rs`: hooks only: the semantic color between the syntax color and the decorations in `text_runs`; `DecorationStyle::Faded` (the color at reduced opacity) for unnecessary code; hint chips in the row layout (a rounded background in the theme's inlay colors, the label in the muted color and a smaller size), hit testing and caret placement through the hint-aware conversions; `EditorEvent::SemanticTokensRequested`, `InlayHintsRequested`, `InlayHintResolve`.

**Theme (`crates/ui/src/theme.rs`, additive).** `SemanticColors` on `Theme` (`semantic`), Visual Studio Dark's C# colors on Eludite Dark and Visual Studio Light's on Eludite Light and Classic; `inlay_hint_text` and `inlay_hint_background`.

**Shell.**
- `crates/eludite/src/shell/semantic.rs` (new): requests, the delta and range choices, off-thread decode, stale-drop, settings, unnecessary-code fading; `crates/eludite/src/shell/semantic_tests.rs` (new).
- `crates/eludite/src/shell/inlay_hints.rs` (new): requests for the visible window, resolve for tooltips, refresh, settings, the toggle command; `crates/eludite/src/shell/inlay_hints_tests.rs` (new).
- Wiring, each a small contiguous block naming this brief: `crates/eludite/src/shell.rs` (mod lines, the editor events, the host's capabilities and refresh events, command registration), `crates/eludite/src/shell/servers.rs` (a generic server's capabilities, generation and refresh events), `crates/eludite/src/shell/session.rs` (the two refresh events), `crates/eludite/src/shell/documents.rs` (open, close and the unnecessary layer beside the squiggles).
- `crates/commands/src/inlay_hints.rs` (new: `eludite.editor.toggle_inlay_hints`), `crates/commands/src/lib.rs` and `builtins.rs` (registration), `crates/commands/src/settings.rs` (the settings key list in its test), `crates/ui/src/keymap.rs` (Alt+F1) and `crates/ui/src/menu.rs` (Edit > IntelliSense > Toggle Inline Hints), additive.

**Docs.** This file and `docs/briefs/0066-report.md`. `README.md`, `CLAUDE.md` and `docs/briefs/README.md` are the integrator's (the report's Integration notes).

## Contract

**Requests (invariants 1 and 12).**
- A document asks for semantic tokens when it opens, 150 ms after the last edit, on a refresh (`workspace/semanticTokens/refresh`, relayed by the host as `eludite/semanticTokens/refresh`), on a new generation, when its server's capabilities arrive and when the solution finishes loading. One request in flight per document; a newer one cancels the older. A server without `semanticTokensProvider` is never asked.
- The method: `full/delta` with the last `resultId` when the server offers `full.delta` and the document has an answer from the same server generation; else, for a document over 2,000 lines whose server offers `range`, `range` for the visible rows and 100 lines around them first, then `full`; else `full`. A delta applies its edits to the kept integer array; an answer the delta cannot apply to (a bad edit) forgets the array and asks for `full`.
- Decoding the integers into row spans runs on a background thread. An answer is dropped (never painted) when the generation or the document version moved since the request, or the editor's buffer changed since the request (the debounce asks again).
- Tokens are UTF-16 positions; the shell converts them to byte columns on the snapshot the request was made on.
- Inlay hints are asked for the visible rows plus 100 lines on each side (`textDocument/inlayHint` with that range), on open, 150 ms after the last edit, on scrolling out of the covered rows, on `workspace/inlayHint/refresh` (relayed by the host as `eludite/inlayHint/refresh`) and on a new generation; the same drop rules apply. `inlayHint/resolve` runs when the mouse rests on a hint without a tooltip (the tooltip shows as Quick Info does).

**Editor layers.**
- Tree-sitter highlighting is the first paint and is never delayed. Semantic colors paint over it; decorations (diagnostics, references, the execution point) over both. A token whose class has no color in the theme leaves the syntax color.
- Semantic spans and hints are anchored: they move with the text while the person types and are replaced whole when the fresh answer arrives. Rows an edit touched keep the spans outside the edited range.
- An unnecessary diagnostic (tag 1) fades its range to 55% opacity whatever its severity (Roslyn reports unused `using`s as hidden).
- An inlay hint is drawn at its position as a chip: rounded background `inlay_hint_background`, label `inlay_hint_text`, 85% of the editor's font size, with a space's width of padding for `paddingLeft`/`paddingRight`. It holds no buffer text: the caret, selections, find, copy and the IME see the buffer only; a click on a hint places the caret at the hint's position; a caret at a hint's position is drawn after a parameter hint (it belongs to the text after it) and before a type hint (it belongs to the text before it).

**Classification and colors.** `TokenClass` (namespace, class, record, delegate, interface, struct, record struct, enum, enum member, type parameter, module, method, extension method, function, property, field, event, constant, local, parameter, label, keyword, control keyword, operator, overloaded operator, string, verbatim string, string escape, number, comment, excluded code, preprocessor, regex parts, JSON parts, XML doc comment parts, macro, lifetime, self keyword, builtin type, attribute) and the modifiers `static`, `deprecated`, `readonly`, `mutable` (rust-analyzer), `declaration`, `defaultLibrary`. Visual Studio Dark: classes, records, delegates, modules and attributes `#4EC9B0`; interfaces, enums `#B8D7A3`; structs, record structs `#86C691`; type parameters `#B8D7A3`; methods, extension methods, functions `#DCDCAA`; locals, parameters `#9CDCFE`; properties, fields, events `#DCDCDC` (Visual Studio's identifier color: the default text); enum members, constants `#DCDCDC`... (exact table in `theme.rs`, Visual Studio 2022's C# defaults); control keywords `#D8A0DF`; string escapes `#FFD68F`; regex and JSON parts in Visual Studio's colors; excluded code `#9B9B9B`; namespaces default text. Visual Studio Light: classes `#2B91AF`, interfaces and enums `#2B91AF`, methods `#74531F`, locals and parameters `#1F377F`, control keywords `#8F08C4`, string escapes `#B776FB`, excluded code `#808080`, and so on. Deprecated adds a strikethrough.

**Settings** (`protocol/schemas/settings.json`, Tools > Options > Text Editor > All Languages > Advanced and > Inlay Hints): `editor.semanticHighlighting` (true), `editor.inlayHints.enabled` (true), `editor.inlayHints.parameterNames` (true), `editor.inlayHints.types` (true), and `editor.languages.<id>.semanticHighlighting` / `editor.languages.<id>.inlayHints` (`default`, `on`, `off`) for `csharp`, `vb`, `fsharp`, `rust`, `typescript`, `javascript`. The host always enables Roslyn's hints; the shell filters by kind (1 type, 2 parameter; a hint with no kind shows when either is on). Turning one off removes the layer at once; turning it on asks at once.

**Command.** `eludite.editor.toggle_inlay_hints` (Edit > IntelliSense > Toggle Inline Hints, Alt+F1): input `{ "path"?: string, "enabled"?: boolean }`, output `{ "path", "enabled", "hints" }`: hints shown for the active (or named) document after the toggle (count as of now). It flips the shown state for the session (not the setting); `enabled` sets it. GPUI exposes key-up only per element, so Visual Studio's show-while-held is a toggle here (the report says so).

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-protocol`: the new schemas conform; the method sets match `host-rpc.md`.
- `cargo test -p eludite-lsp semantic inlay`: decode of a known array (positions, lengths, multi-row deltas), delta application (insert, delete, replace, a bad edit), classification of LSP, Roslyn and Visual Studio names, the hint text with paddings and parts; the generic client's refresh events.
- `cargo test -p eludite-editor`: semantic spans move through edits; the hint display mapping (to and from display offsets, a click inside a hint, the caret side by kind, tabs); `text_runs` puts the semantic color between syntax and decorations; a headless view: tokens applied for request `id` are dropped for an older id or after an edit; hints change no line count, copy is the buffer text, Right moves over a hint in one step.
- `cargo test -p eludite semantic_tests inlay_hints_tests`: headless against the fake host (C#) and a `FakeServer` (rust-analyzer's shape): exact token ranges to theme colors; a delta after an edit; a stale answer (an edit while the server answers, an older generation) dropped; `refresh` asks again; the settings and the command; inlay positions, the caret skipping, copy without hints; a fade for an unnecessary diagnostic.
- `dotnet test --filter LspProxy`: the forwarded lists, capabilities, configuration answers and relays.
- Against the real servers when present (skips otherwise): Roslyn through the host (`tools/roslyn-pin/build.sh`), rust-analyzer.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` on every crate touched, `dotnet build` and `dotnet test` of the host.

## Budget

- Keystroke to frame p99 < 8 ms unchanged with 2,000 semantic tokens and 200 inlay hints on screen (asserted with `assert_budget`, reported on CI).
- Decoding a full answer for a 10,000-line file (about 60,000 tokens) runs off the UI thread; the UI-thread work to apply it is bounded (installing the rows) and measured.

## Exit criterion

Every proving test passes here; CI green on the Rust and .NET jobs; the report records the numbers, what ran against real servers and what is owed.

## Out of scope

Folding, multi-caret and editing commands, navigation, CodeLens (other briefs). Custom token colors in Tools > Options > Fonts and Colors (a later brief: the colors are theme tokens now). Semantic tokens for Razor (brief 0056's grammar stays; Razor IntelliSense is its own brief). `textDocument/semanticTokens/full` for documents several servers serve beyond the first that offers it.
