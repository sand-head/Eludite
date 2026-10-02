# Brief 0013 report: completion, hover and signature help in the editor

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0013-completion-and-hover`, rebased onto `origin/main` at `d09a61a`. Date: 2026-10-02.
Brief: [0013-completion-and-hover.md](0013-completion-and-hover.md).

## 1. Summary

- **It works end to end on `dotnet/Eludite.slnx`** with the real `eludite-host` and Roslyn. The manual run used real
  XTest input in a nested KWin and produced the three screenshots in section 6:
  - completion after `_sdkDiscoverer.` in `HostRpcTarget.cs`;
  - Parameter Info inside `string.Join(",", ` with the second parameter highlighted;
  - Quick Info with the pointer resting on `JsonRpc` in `HostServer.cs`.
- **Budgets (release build, Wayland backend, nested KWin at 60 Hz, 3 runs of 200 triggers):**

  | Budget | Result |
  |---|---|
  | Completion visible < 50 ms p95 after the trigger | **29.2 to 29.5 ms p95** key to the presented frame showing the list (p50 12.4 to 12.6 ms). Pass |
  | Host latency, reported separately | **p50 6.5 to 6.7 ms, p95 16.8 to 17.8 ms** (request written to reply read, including decoding about 900 items) |
  | UI latency, reported separately | **p50 3.1 to 3.2 ms, p95 4.8 to 5.5 ms** (the shell's own work), plus the idle wait for the next refresh (p95 14.2 to 14.3 ms) |
  | Keystroke frame cost with the list open and filtering < 8 ms p99 | **p99 3.9 to 4.2 ms** (p50 2.8 to 2.9 ms). Brief 0012's typing benchmark, now with IntelliSense active: p99 5.4 to 5.9 ms (0012: 5.0 to 5.8). Pass |
  | Resident memory over 500 completion cycles grows < 5 MB | **+2.8 MB** (93.7 to 96.5 MB). Pass |

  In a nested KWin started with a fresh configuration, the wait for the next frame has a long tail. That raises
  trigger-to-visible to 45 to 49 ms at p95 and 64 to 70 ms at p99. The shell's own work is the same there. See
  section 7.
- **Contract first.** The four command schemas and the typed `textDocument/signatureHelp` schema came first, alone,
  with the `host-rpc.md` change. Then came, in order: the protocol types and drift tests, the editor, the shell wiring,
  the commands, the tests, and the measurements.
- **Tests:**
  - `cargo test --workspace`: 257 passed, 0 failed, 1 ignored (256 before the rebase, which brought one test from
    `main`). Brief 0012 had 221.
  - `dotnet test dotnet/Eludite.slnx`: 129 tests, 125 passed, 4 skipped, 0 failed.
  - fmt and clippy (`-D warnings`) are clean. `dotnet build` reports 0 warnings.
- **New dependency:** `fuzzy` (vendored from Zed at the pin, already in `vendor/` since brief 0009, GPL-3.0-or-later)
  is now used by `eludite-editor`. Its transitive crates (`gpui_util`, `path`, `log`) were already in the build. No
  third-party crate was added. New internal edge: `eludite` depends on `eludite-protocol` (MIT).

## 2. Protocol (schemas first)

**Command schemas** (`protocol/schemas/`):

| Command | Input | Output | Permission |
|---|---|---|---|
| `eludite.editor.complete` | `{path?, line?, column?, trigger?}` | `{path, line, column, state: loading\|open\|closed, source?: languageServer\|syntax, filter, total, selected?, items[≤100]: {label, kind, detail?}}` | read |
| `eludite.editor.accept_completion` | `{path?, label?}` | `{path, accepted, label?, text?, line, column}` | edit_buffer |
| `eludite.editor.hover` | `{path?, line?, column?}` (the caret does not move) | `{path, line, column, state, text?}` (the Markdown rendered as plain text) | read |
| `eludite.editor.signature_help` | `{path?, line?, column?, trigger?}` | `{path, line, column, state, signatures: [{label, documentation?, parameters: [label]}], active_signature?, active_parameter?}` | read |

When an agent calls one of these from another thread, the call waits up to 5 s for the answer. On the UI thread the
call returns at once.

**`textDocument/signatureHelp` is now typed:**
- schema `protocol/schemas/host/signature-help.json`;
- `eludite_protocol::lsp::{SignatureHelpRequest, SignatureHelpParams, SignatureHelpContext, SignatureHelp,
  SignatureInformation, ParameterInformation, ParameterLabel}`;
- `SignatureInformation::parameter_range` resolves both label forms (a substring, or UTF-16 offsets).

In `host-rpc.md`, signatureHelp moved from the untyped table to the typed one. A new paragraph says how the shell uses
the four IntelliSense requests: flush first, newest wins, stale answers dropped, lazy resolve.

**Drift tests:**
- schema files may name typed forwarded methods;
- the marker list includes `SignatureHelpRequest`;
- a conformance test checks params and result against `signature-help.json`, with rejections.

**Host change, forced by the contract.** The host's `LspProxy.TypedRequests` now lists `textDocument/signatureHelp`,
because typed requests are validated before forwarding (a missing `textDocument.uri` gives -32602). The change is one
line. The host test for untyped pass-through now uses `typeDefinition`. A new test,
`SignatureHelp_IsTypedAndValidated`, covers the change. Nothing else in the host changed.

## 3. Editor (`crates/editor`, `crates/ui`)

- **`intellisense` (public, pure parts):**
  - `CompletionItem`, `CompletionEdit`, `CompletionSource`, `CompletionTrigger`, `SignatureTrigger`, `EditorEvent`;
  - snapshots for commands and tests;
  - `snippet_to_plain` (`$1`, `${1:x}`, choices and escapes become plain text);
  - `enclosing_call` (the `(` the caret is inside, and its argument index);
  - `identifiers` (the fallback);
  - fuzzy ranking: prefix matches first, then the `fuzzy` score, then sort text.
- **`popups.rs`** holds the view's state, the trigger logic and drawing. The popups are GPUI `deferred(anchored(..))`
  elements, so they move with scrolling and stay inside the window.
- **Triggers.** The view never talks to a server. It emits `EditorEvent`s, and its owner answers through `open_*` and
  `set_*`, quoting the id that `open_*` returned. An answer with another id is ignored.
  - Completion opens on an identifier character typed at the start of a word, and on `.`, `(` and `<`. It does not
    open on digits, or inside comments or strings: the highlight kinds decide, plus a `//` check for text typed
    before the highlights catch up.
  - Ctrl+Space opens it explicitly.
  - While the list is open, typing filters it with the vendored `fuzzy` crate, off the UI thread. A new request is
    made only when the list is incomplete or a request is still in flight. In that case each keystroke supersedes
    the previous request.
  - The list keeps showing the previous items until the new ones have been filtered, so it does not flicker.
- **List keys:**
  - Up and Down select, Page Up and Page Down page.
  - Tab commits.
  - Enter commits a hard selection. Opening with `(` or `<` and nothing typed gives a soft selection, where Enter
    inserts a new line.
  - Escape closes the innermost popup first.
- **Committing** applies the item's `textEdit`, extended to the caret on the same line when the user typed after the
  request. Without a `textEdit`, the word typed so far is replaced by `insertText` or the label.
- **Lazy documentation.** Only the selected item is resolved. Its detail and Markdown documentation show beside the
  list.
- **Quick Info:**
  - The mouse rests over an identifier for 400 ms (`HOVER_DELAY`), or the user presses Ctrl+K, Ctrl+I.
  - The tooltip renders Markdown: paragraphs, fenced code, code spans, bold and italic, escapes, entities and links.
  - It moves with scrolling.
  - It closes when the mouse leaves the word or the editor, on typing, and on Escape.
- **Parameter Info:**
  - `(`, `,` and Ctrl+Shift+Space open it.
  - The active parameter is bold, in the guide color.
  - The argument index updates from the text at once (`enclosing_call`), and the server's answer to the retrigger
    confirms it.
  - Up and Down cycle the overloads (▲1 of 13▼).
  - `)`, Escape, or the caret leaving the call close it.
- **Fallback.** `complete_from_syntax` lists the identifiers of the buffer's tree-sitter tree, without the word being
  typed. Every leaf node whose kind ends in `identifier` counts, so it is not specific to C#.
  - The parse runs on the syntax thread, the tree is dropped, and `alloc::release_free_memory()` runs (brief 0011).
  - The result is cached per buffer version, and a stale cache is shown at once while it is refreshed.
  - The list's footer reads "Identifiers in this file — the language server is loading".
  - Server items replace syntax items. Syntax items never replace server items. An empty server answer keeps the
    fallback.
- **`crates/ui`:**
  - `popup` holds `CompletionKind`, the 25 LSP kinds with Visual Studio's glyphs and colors (purple methods, blue
    fields and locals, orange classes and events, and so on), plus `completion_row` and `popup_panel`, drawn from the
    theme tokens.
  - `markdown` holds the parser and renderer.
  - The Edit menu gains Complete Word, Parameter Info and Quick Info. The keymap gains Ctrl+Space, Ctrl+Shift+Space
    and Ctrl+K, Ctrl+I.

## 4. Shell wiring (`crates/eludite`, `crates/lsp`, `crates/commands`)

- **`HostSession::request::<R>`** queues a typed request on the session worker. It is therefore written after every
  `didChange` queued before it.
  - A waiter thread blocks on the reply and sends a `Reply` (result, sent and received times) on a oneshot channel.
    The UI awaits that channel and never blocks.
  - `RequestHandle::cancel` sends `$/cancelRequest` for the request's JSON-RPC id.
- **`shell/intellisense.rs`:**
  - Each document has at most one completion, resolve, hover and signature request in flight. A new one cancels the
    old one, and closing a popup cancels its request. A canceled request's task is dropped, so its reply is never
    applied.
  - A reply is applied only if it is the newest request's, and only if the solution generation and the document's
    LSP version are those it was made under. The client also drops results from an old generation.
  - A new generation cancels everything in flight.
- **Before each request**, the pending `didChange` is flushed. Typing that triggers completion therefore sends the
  text at once instead of after the 50 ms debounce, which only applies to edits that trigger nothing.
- **Provider choice** follows `eludite/languageServer/status` and `eludite/solution/status`:
  - `running` and `loaded`: the server only;
  - `starting`, `restarting` or `loading`: the syntax fallback at once, and the server's items when they arrive;
  - no host, `unavailable`, `exited`, `failed`, or a non-C# file: the syntax fallback only.

  When the solution finishes loading, lists still on the fallback ask the server again.
- **The server's capabilities** (trigger characters, `resolveProvider`) decide the LSP trigger kind and whether to
  resolve.
- **Converting items:**
  - each `textEdit` (`TextEdit`, or the insert range of an `InsertReplaceEdit`) is anchored in the text the request
    was made on;
  - `itemDefaults` (`editRange`, `insertTextFormat`, `data`) are honored;
  - snippets become plain text;
  - hover `MarkupContent`, `MarkedString` and arrays of them become Markdown.
- **Everything goes through the bus.** Typing, the mouse and the keys run the four commands. Tab and Enter in the list
  run `eludite.editor.accept_completion` through `RunCommand` bindings in the editor's `showing_completions` and
  `completion_selected` contexts.
  - Every trigger and every commit is therefore in the audit log, and agents drive the same commands.
  - Typed refreshes, and Parameter Info retriggers when the caret moves, also go through the bus.
- **Fake host** (`crates/lsp`, feature `fake`; this change was needed for the proving tests):
  - `respond(method, …)` scripts replies: at once, after a delay, held until canceled, or an error;
  - a request still in flight is answered -32800 on `$/cancelRequest`;
  - `set_ignore_cancel` delivers late results anyway;
  - `set_hold_load` and `finish_load` control the solution status;
  - `set_language_server` sets the state and capabilities the fake reports;
  - received messages now carry their JSON-RPC id.

## 5. Tests

| Where | New | What |
|---|---|---|
| `eludite-protocol` | 2 | signatureHelp round trips and both parameter-label forms; conformance and rejections against `signature-help.json` |
| `eludite-lsp` `tests/in_process.rs` | 2 | typed signatureHelp through the fake host; a held request canceled to RequestCancelled; a late result with cancels ignored; a held load |
| `Eludite.Host.Tests` | 1 | `SignatureHelp_IsTypedAndValidated` |
| `eludite-ui` | 3 | Markdown (a Roslyn hover, breaks, lists, unclosed marks, entities), the kind map |
| `eludite-editor` unit | 4 | word starts, snippets to plain text, enclosing calls, ranking |
| `eludite-editor` `tests/intellisense.rs` | 6 headless | triggers (typing, `.`, `(`, not in comments, not on digits, the three keys); list filter, navigation, Tab commit and stale ids; text edit extended to the caret, soft Enter, commit by label; syntax fallback then a swap to server items; Quick Info after the delay, kept within the word, closed outside it and on leaving the editor, moving with scrolling, closed on typing; Parameter Info active parameter, `,`, caret moves, overload cycling, `)` and Escape |
| `eludite-commands` | (in 4 tests) | parsing, rejections, output schemas and permissions of the four commands |
| `eludite` `shell/intellisense_tests.rs` | 7 headless, against the fake host | see below |
| `eludite` unit | 4 | item conversion (text edits, defaults, snippets, resolve data), hover contents, signatures, capabilities |

The 7 shell tests cover the brief's Proving test:

1. **`completion_opens_on_trigger_filters_and_commits_with_tab`:**
   - `.` sends trigger kind 2, after the `didChange`;
   - the list is sorted;
   - typing `WrL` filters it without a second request;
   - the bus reports what is shown;
   - Tab commits `Console.WriteLine` through `eludite.editor.accept_completion`;
   - Ctrl+Space works and Escape closes the list.
2. **`each_keystroke_cancels_the_previous_request_and_stale_answers_are_dropped`:**
   - a keystroke while the first request is in flight sends `$/cancelRequest` with that request's id, then a new
     request;
   - the host is set to ignore cancels and delivers the first answer late; it is never shown;
   - an answer computed under generation 1 is not shown after the solution is reopened (generation 2).
3. **`the_selected_item_resolves_its_documentation_lazily`:**
   - exactly one `completionItem/resolve`, for the selected item, carrying its `data`;
   - the detail appears;
   - Down resolves the next item.
4. **`falls_back_to_syntax_identifiers_while_loading_and_swaps_to_server_items`:**
   - the language server is `starting` and the load is held;
   - the syntax items show before the server answers (400 ms);
   - the bus reports `source: syntax`;
   - the server's items then replace them.
5. **`quick_info_appears_after_the_delay_and_dismisses`:**
   - nothing at 399 ms, Quick Info at 400 ms, with the request position and the rendered text checked;
   - it closes when the pointer moves to another word;
   - Ctrl+K, Ctrl+I opens it and typing closes it;
   - an agent on another thread gets the text.
6. **`parameter_info_tracks_the_active_parameter`:**
   - `(` sends trigger kind 2;
   - `,` moves to the second parameter at once and sends a retrigger with `isRetrigger`;
   - Left goes back to the first parameter, by the server's answer;
   - `)` closes it.
7. **`agents_complete_and_commit_on_the_bus`:** from another thread, `eludite.editor.complete` with a position waits
   for and returns the list, and `accept_completion {label}` commits it.

**Brief 0012's `open_solution_edit_diagnostics_and_error_list` changed in one place.** It now makes its edit with
`update_editor` instead of typing. Typed identifier characters now trigger completion, which flushes `didChange` at
once (section 4), so the test's assertion of exactly one debounced change no longer held for typed text. The debounce
itself is still tested.

The 7 shell tests passed 5 times in a row.

## 6. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/intellisense-linux.sh OUT_DIR`.
  - Phases 1 and 2 use the X11 backend on the nested Xwayland with real XTest keys and pointer
    (`tools/intellisense.py`).
  - Phase 3 is the benchmarks.
- **Host:** the Debug `eludite-host`, with Roslyn from `~/.cache/eludite/roslyn`.
- **Raw results:** `crates/eludite/results/linux-intellisense.json`.
- **Load average:** 2.3 to 2.5 (1-minute) during phases 1 and 2.

**Screenshots:**

- [`crates/eludite/screenshots/linux-completion.png`](../../crates/eludite/screenshots/linux-completion.png):
  - The input was a new line 77 after `var sdks = await _sdkDiscoverer…`, with `_sdkDiscoverer.` typed on it.
  - Roslyn's list shows DiscoverAsync (selected), Equals, GetHashCode, GetType and ToString, each with the purple
    method glyph.
  - Beside the list is the resolved detail: `(awaitable) Task<IReadOnlyList<DotnetSdk>>
    ISdkDiscoverer.DiscoverAsync(CancellationToken cancellationToken)`.
  - The trace shows the reply: "922 items (host 50.5 ms)". That was the first completion after load; warm runs are
    in section 7.
- [`crates/eludite/screenshots/linux-signature-help.png`](../../crates/eludite/screenshots/linux-signature-help.png):
  - The input was `string.Join(",", ` typed on the same line.
  - The tooltip reads "▲1 of 13▼ string string.Join(char separator, params object?[] values)", with `values`
    highlighted, and the overload's documentation below it.
  - The trace shows "13 signatures, active parameter Some(0)" after `(`, then `Some(1)` after `,`.
- [`crates/eludite/screenshots/linux-quick-info.png`](../../crates/eludite/screenshots/linux-quick-info.png):
  - The input was `HostServer.cs` line 18: Ctrl+F found `JsonRpc CreateConnection`, and the pointer rested on
    `JsonRpc` at the caret position the app reported.
  - Quick Info shows `class StreamJsonRpc.JsonRpc` in the editor font, then "Manages a JSON-RPC connection with
    another entity over a Stream."

None of the edited files were saved; the script checks that the files on disk are unchanged.

## 7. Budgets and measurements

**Method.**
- The harness is `eludite --solution dotnet/Eludite.slnx --open-file …/HostRpcTarget.cs --bench-complete N`. It
  waits for the load and for 2 s more, opens a line after the first `_sdkDiscoverer.`, and types `_sdkDiscoverer`.
- Then, N times:
  - type `.` (`Window::dispatch_keystroke`);
  - wait for the language server's list to be applied and drawn;
  - type `d` and `i` 25 ms apart (filtering with the list open);
  - press Escape, then three Backspaces.
- There are 10 warm-up cycles first.
- **Host latency** runs from the request being written to the reply being read, including decoding the reply.
- **UI latency** is the shell's own work: key to request written, plus reply read to items applied, plus render to
  end of present of the frame that shows the list.
- **Trigger-to-visible** runs from the key to the end of that frame's present. It includes the idle wait for the
  display's next refresh.
- **Keystroke frame cost** uses brief 0009's method: the key handler plus the next frame's render to present.
- The harness turns off GPUI's 33 ms frame limit for unfocused windows, as `--bench-start` already did.

**Results, nested KWin with the user's KWin configuration** (`spikes/0005-acp-panel/tools/nested.sh`). The load
average was 1.9 to 2.5. Each of the three 200-trigger columns is one run; the 500-cycle column is the memory run.

| Measure | Run 1 | Run 2 | Run 3 | 500-cycle run |
|---|---|---|---|---|
| Host latency p50 / p95 / p99 (ms) | 6.7 / 17.8 / 20.3 | 6.5 / 16.8 / 18.3 | 6.5 / 17.2 / 19.6 | 6.6 / 10.4 / 18.7 |
| UI latency p50 / p95 / p99 (ms) | 3.1 / 5.4 / 6.0 | 3.1 / 5.5 / 6.0 | 3.2 / 4.8 / 6.1 | 3.3 / 5.7 / 6.4 |
| Wait for the next frame, p95 (ms) | 14.2 | 14.3 | 14.2 | 14.2 |
| **Trigger-to-visible p50 / p95 / p99 (ms)** | 12.5 / **29.3** / 30.7 | 12.6 / **29.5** / 30.3 | 12.4 / **29.2** / 31.1 | 12.2 / **28.8** / 30.4 |
| **Keystroke frame cost, filtering, p50 / p99 (ms)** | 2.8 / **4.1** | 2.9 / **4.2** | 2.9 / **3.9** | 3.0 / **4.2** |
| Keystroke frame cost, the `.` trigger, p99 (ms) | 4.2 | 3.7 | 3.7 | 3.7 |
| RSS: cycle 0 to the last cycle (MiB) | 93.8 to 94.8 | 94.1 to 95.0 | 94.4 to 95.2 | **93.7 to 96.5 (+2.8)** |

There were no timeouts in 1,100 triggers.

**The same benchmarks from `intellisense-linux.sh`** (a nested KWin with a fresh `XDG_CONFIG_HOME`, which the XTest
phases need). The load average was 2.0 to 2.7.
- Host latency, UI latency and keystroke frame cost were the same as above:
  - host p95 16.6 to 17.1 ms;
  - UI p95 4.6 to 4.9 ms;
  - filtering p99 5.6 to 6.2 ms.
- The wait for the next frame had a long tail: p95 22 to 37 ms, p99 53 to 58 ms. That gives trigger-to-visible p95
  of 31 to 46 ms and p99 of 64 to 70 ms.
- A per-sample trace showed the cause is frame scheduling in that compositor session; the shell's work does not
  change.
  - Under the user's configuration, frames start within one refresh: wait p95 14.3 ms, max 15.4 ms.
  - Explicitly scheduling the next frame from the editor when an answer arrives did not change the tail in that
    session, so it was not kept.
- Memory there grew 94.1 to 96.9 MB over 500 cycles (+2.8 MB).

**Typing (brief 0012's `--bench-type 500` in `LspProxy.cs`, with the solution).** This now has IntelliSense active:
word starts request completion, and the list filters as the user types. Three runs gave p99 5.35, 5.89 and 5.83 ms,
and p50 2.40 to 2.43 ms. Brief 0012 measured 5.0 to 5.8 ms. This is under 8 ms and within run-to-run noise of 0012.

**Memory.** The +2.8 MB over 500 cycles is under the 5 MB budget. It grows roughly linearly, about 0.5 MB per 100
cycles. Known contributors:
- the audit log (append-only by design, one entry per trigger and commit);
- the shell's completion timing record, capped at 4,096 entries.

Neither was separated from allocator retention, which needs a longer run to settle.

**Not measured:** the fallback's own latency on the real host. It shows within a frame or two in the headless tests,
and is reported only by the trace.

## 8. Protocol gaps and other findings

1. **Fixed client capabilities.** The host advertises `signatureHelp: {}`, so Roslyn sends parameter labels as
   strings rather than offsets. The shell copes with both forms, and searches a string label after the previous
   parameter. The shell cannot ask for documentation formats or `activeParameterSupport` either. A settings or
   capabilities method on the bridge (brief 0007's open item) would fix this.
2. **`additionalTextEdits` are not applied.** These are, for example, the `using` directive added by
   unimported-type completion. Committing does not resolve the item first. It needs resolve-on-commit and multi-range
   edits.
3. **Commit characters** (`commitCharacters`, `itemDefaults.commitCharacters`) are not used. Typing `.` or `(` closes
   the list without committing. Visual Studio commits on them.
4. **Snippets** are inserted as plain text. Tab stops are out of scope.
5. **Large lists on stdio.** A member list is about 900 items (64 to 123 KB, brief 0007). Host latency p95 is about
   17 ms, and that includes decoding on the shell's waiter thread. A side channel, or server-side filtering
   (`isIncomplete` lists), would cut it.
6. **The mouse cannot enter Quick Info.** The tooltip closes when the pointer leaves the word, so its text cannot be
   selected. Visual Studio keeps it while the pointer is over it.
7. **No per-document trigger characters from other servers yet.** The editor's trigger set is
   `DEFAULT_COMPLETION_TRIGGERS`. `EditorView::set_completion_triggers` exists for when a non-C# server arrives.
8. **The frame-scheduling tail** described in section 7, in a freshly configured nested KWin. It is worth checking on
   a real session and on the CI reference machine.

## 9. Sizing brief 0014

Brief 0014 covers go to definition, references, rename, code actions and Error List filtering.

**What exists now:**
- typed `definition` and `references` in `eludite-protocol`;
- `eludite.file.open {line, column}` for navigation;
- the request pattern of this brief: `HostSession::request`, newest wins, stale answers dropped, commands through the
  bus;
- popups anchored to text (for a code-action light bulb).

**What is missing:**
- **Go to definition** (F12): a command and keys, metadata-as-source for definitions in compiled assemblies (Roslyn's
  `metadataAsSource` URIs, which need a host request for the generated text), and a navigation history
  (Ctrl+-).
- **Find All References** (Shift+F12): a new tool window, rows grouped by file with previews read off the UI thread,
  and click-through.
- **Rename** (F2) and **code actions** (Ctrl+.):
  - type `rename`, `prepareRename`, `codeAction` and `codeAction/resolve` in the protocol;
  - apply a `WorkspaceEdit` to open buffers and to files on disk (closed files are opened, edited, saved, and
    `didChange` sent), as one undoable step per document;
  - an inline rename UI and a light bulb menu.
- **Error List filtering:** current document, open documents or entire solution, severity toggles, a search box, and
  an `eludite.error_list.filter` command.

**Estimate:** about 1.5 times this brief, roughly two agent-weeks. Split it in two:
- **0014a, about one agent-week:** definition, references with the tool window, and Error List filtering.
- **0014b, about one agent-week:** rename and code actions, with the shared `WorkspaceEdit` application.

Gaps 1 and 2 of section 8 should go into 0014b, which needs resolve-before-apply anyway.

## 10. Commits

1. `2690174` Specify the completion, hover, signature help and accept-completion commands and type
   textDocument/signatureHelp in protocol/schemas (schemas and `host-rpc.md` only)
2. `1bba188` Type textDocument/signatureHelp in eludite-protocol and the host, and script forwarded replies, cancels
   and held loads in the fake host
3. `d435f73` Add the completion list, Quick Info and Parameter Info layers to the editor with trigger logic, fuzzy
   filtering and the tree-sitter identifier fallback
4. `25df0ef` Wire the editor's completion, Quick Info and Parameter Info to eludite-host through the session worker
   with cancellation and stale-answer dropping
5. `600f038` Put completion, accept, Quick Info and Parameter Info on the command bus with Visual Studio's keys and
   Edit menu items, and route typing and mouse triggers through them
6. `e7d3fb5` Test completion, Quick Info and Parameter Info headlessly against the fake host, keep the list steady
   while new items are filtered, and let agents wait for the answer
7. `1634be3` Add the --bench-complete harness, caret bounds for pointing at text, and the IntelliSense manual-run
   tooling
8. `d98e691` Run the completion benchmark unthrottled, record the IntelliSense manual run and add its screenshots
9. This report and the brief's Status line.

## 11. Not done, caveats

- **Windows and macOS:** not run. **CI:** not run.
- **Files touched beyond the brief's list** (each forced by the brief's own requirements):
  - **The host:** one line in `LspProxy.cs` and its tests. Typing signatureHelp makes the host validate it
    (section 2).
  - **`crates/lsp`'s fake host:** extended so the shell tests can script replies, cancels and loading.
- **The 4 skipped .NET tests** need the generated roslyn-200 bench solution or the legacy corpus, which this
  worktree does not have.
- **Not updated:** `docs/briefs/README.md` (index row) and `CLAUDE.md`, which are outside this brief's files. The
  crate map still matches.
- **The 0012 test now edits programmatically** (section 5), because typing now flushes changes for completion.
