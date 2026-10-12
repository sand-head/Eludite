# Brief 0069: IntelliSense editing depth for C# and Visual Basic

Status: in progress. Report: [0069-report.md](0069-report.md).
Phase: 2 (PLAN.md section 10: "on par with Visual Studio and Rider for .NET work"; accepted by the owner's direct request)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 5, 6, 12), 4.1 (the editor), 4.3 (IntelliSense through Roslyn), 5.1 (every action a command), 9 (budgets)
Related ADRs: ADR-0002 (process topology: Roslyn stays in `eludite-host`), ADR-0003 (protocols: LSP, no bespoke hooks), ADR-0014 (tests fail on behavior only)
Depends on: brief 0013 (completion, Quick Info, Parameter Info), brief 0015 (code actions, the workspace-edit applier), brief 0050 (Format Document, format on save), brief 0063 (Visual Basic through the host), brief 0007 (the host bridge). Runs beside briefs 0066 (inlay hints: defines `editor.inlayHints.*`), 0068 (Tab and indentation, the context menu) and 0070 (the save path).

## Goal

Editing C# and Visual Basic feels like Visual Studio and Rider: completion commits on the characters the language
server names (typing `Console.WriteL(` leaves `Console.WriteLine(`), the light bulb sees the selection (Extract
Method, Introduce Local and every selection refactoring Roslyn offers appear), snippets are tab-stop sessions (LSP
snippets from completion, a built-in set of Visual Studio code snippets with Tab-Tab, Insert Snippet and Surround
With), `override ` completion inserts the whole member, `///` writes the doc comment skeleton, hovering a squiggle
says what is wrong, Format Selection, format on type, on paste and on save work, Remove and Sort Usings, Fix All and
Code Cleanup run, Roslyn's options are settable from Eludite's settings, and full-solution analysis lists errors of
closed files in the Error List.

## Files in scope

**Editor.**
- `crates/editor/src/intellisense.rs`: `CompletionItem::commit_characters`, `CompletionItem::snippet`, `CompletionItem::command` (opaque JSON, for the owner), `DEFAULT_COMMIT_CHARACTERS` (Visual Studio's C# set), new `EditorEvent` variants (`CompletionCommitted`, `CharacterTyped`, `Pasted`), `snippet_to_plain` kept for plain contexts.
- `crates/editor/src/snippet_session.rs` (new): the LSP snippet parser (tab stops, placeholders, nested placeholders, choices, `$0`, variables, escapes) and the session (stops as anchor ranges; linked stops as several selections).
- `crates/editor/src/popups.rs`: commit characters, the commit event, the hover's diagnostics block.
- `crates/editor/src/view.rs` (shared): actions `NextSnippetStop`, `PreviousSnippetStop`, the `in_snippet` key context, one marked call each in `tab`, `newline`, `cancel`, `paste` and `insert_text`; nothing else.
- `crates/editor/src/lib.rs` (shared): the `mod` line and re-exports.

**Snippets.** `crates/eludite/snippets/csharp/*.snippet` and `crates/eludite/snippets/vb/*.snippet` (new, MIT, Visual
Studio's `CodeSnippets` XML format), embedded with `include_str!`.

**Shell.**
- `crates/eludite/src/shell/intellisense.rs`: commit characters from the item, `itemDefaults.commitCharacters` and the server's `completionProvider.allCommitCharacters`; snippet items; `roslyn.client.completionComplexEdit`; diagnostics in Quick Info; `CharacterTyped` and `Pasted` routing.
- `crates/eludite/src/shell/code_actions.rs`: the selection range in `textDocument/codeAction`, the diagnostics overlapping the range's lines, Fix All through `codeAction/resolve`, the organize-imports request.
- `crates/eludite/src/shell/format.rs`: Format Selection (`textDocument/rangeFormatting`), format on type (`textDocument/onTypeFormatting`), format on paste.
- `crates/eludite/src/shell/snippets.rs` (new): the Visual Studio snippet reader (XML to LSP snippet), the built-in and user libraries (`editor.snippets.paths`), the Insert Snippet and Surround With pickers, Tab-Tab.
- `crates/eludite/src/shell/cleanup.rs` (new): `///` through `textDocument/_vs_onAutoInsert`, Remove and Sort Usings, Code Cleanup with its two profiles, the Roslyn options push (`workspace/didChangeConfiguration`), full-solution analysis.
- `crates/eludite/src/shell/intellisense_depth_tests.rs` (new): the headless tests.
- `crates/eludite/src/shell.rs`, `shell/settings.rs`, `shell/error_list.rs` (shared): one marked block each (mod lines, the jobs drain and command dispatch, `apply_settings`' call, the Error List's workspace rows).
- `crates/eludite/Cargo.toml` (`roxmltree`, already in the lockfile, MIT).

**Commands and schemas.**
- `crates/commands/src/intellisense.rs` (new) and its `mod` line: `eludite.editor.insert_snippet`, `eludite.editor.surround_with`, `eludite.editor.snippets`, `eludite.editor.format_selection`, `eludite.editor.remove_and_sort_usings`, `eludite.editor.code_cleanup`.
- `protocol/schemas/editor-*.{input,output}.json` for those six (new); `protocol/schemas/settings.json` (additive keys); `protocol/schemas/host-rpc.md` (the forwarded methods, `workspace/didChangeConfiguration`, the configuration answer, `workspace/diagnostic`).
- `crates/ui/src/menu.rs`, `crates/ui/src/keymap.rs` (shared): one marked block each.

**Host.** `dotnet/src/Eludite.Host/Lsp/LspProxy.cs` (the forwarded list, the configuration answer, client capabilities), `dotnet/src/Eludite.Host/Lsp/RoslynOptions.cs` (new: the mapping table), `dotnet/tests/Eludite.Host.Tests/` (the proxy's unit tests, and real-Roslyn tests that skip without the pinned server).

Nothing in `docs/PLAN.md`, `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, generated bindings or `corpus/dap/`.

## Contract

**1. Commit characters.**
- An item's characters are its `commitCharacters`, else the list's `itemDefaults.commitCharacters`, else the server's `completionProvider.allCommitCharacters`, else `DEFAULT_COMMIT_CHARACTERS` (`.`, `,`, `;`, `(`, `)`, `[`, `]`, `{`, `}`, `<`, `>`, `=`, `+`, `-`, `*`, `/`, `%`, `&`, `|`, `^`, `!`, `~`, `?`, `:`, `"`, `'`, space). The syntax fallback list never commits on a character.
- Typing a commit character while the list shows a hard selection (something was typed into the filter or the selection was moved, and the list is not in suggestion mode) commits the selected item, then inserts the character. Space commits only with a hard selection; a soft selection (the list opened by `(` or `<` with nothing typed) never commits on a character. Tab and Enter commit as before.
- A commit by character emits `EditorEvent::CompletionCommitted` so the owner applies the item's additional edits and command, exactly as `eludite.editor.accept_completion` does.

**2. Code actions with the real range.** `textDocument/codeAction` carries the primary selection's range (the caret when empty) and the diagnostics whose ranges intersect the lines of that range. The bulb's answer is reused by Ctrl+. only for the same range and text. Extract Method (Ctrl+R, Ctrl+M) runs `eludite.editor.code_actions` with `title: "Extract method"`: the action of that title in the selection's answer is applied at once, else the menu opens.

**3. Snippet sessions.**
- `snippet_session::parse` reads `$N`, `${N}`, `${N:text}` (nested), `${N|a,b|}`, `$0`, `$name`, `${name}`, `${name:default}` and the escapes `\$`, `\}`, `\\`; variables resolve through the caller (`TM_SELECTED_TEXT`, `SELECTION`, `TM_CURRENT_LINE`, `TM_CURRENT_WORD`, `TM_LINE_INDEX`, `TM_LINE_NUMBER`, `TM_FILENAME`, `TM_FILENAME_BASE`, `CLIPBOARD`: empty); unknown variables insert their default or their name as a placeholder, as VS Code does.
- Inserting a snippet replaces a range as one undo step, indents every following line with the first line's indentation, and selects stop 1 (every occurrence of it: linked stops are several selections, so typing edits them all). Tab goes to the next stop, Shift+Tab to the previous one; Tab on the last stop, Enter, or Escape ends the session with the caret at `$0` (Escape: where it is). Moving the caret out of every stop's range ends it. A snippet with no stop other than `$0` places the caret and starts no session.
- Completion items with `insertTextFormat` 2 insert as snippets (argument placeholders).
- Visual Studio code snippets: `crates/eludite/snippets/csharp/` holds `prop`, `propfull`, `propg`, `ctor`, `cw`, `for`, `forr`, `foreach`, `if`, `else`, `while`, `do`, `switch`, `try`, `tryf`, `using`, `lock`, `class`, `interface`, `struct`, `record`, `enum`, `namespace`, `svm`, `sim`, `equals`, `exception`, `attribute`, `indexer`, `iterator`, `testx` (xUnit), `testn` (NUnit) and `testm` (MSTest); `vb/` holds Property, If, For, ForEach, Try, Using, Select, Sub, Function, Class. The reader maps `Literal`/`Object` ids to tab stops in order of first appearance (repeats are linked), `$selected$` to the selection, `$end$` to `$0`, `$$` to `$`, the functions `ClassName()` (the enclosing type's name from the text before the caret) and `SimpleTypeName(global::X.Y)` (`Y`).
- Tab with no completion list and no session expands the snippet whose shortcut is the word before the caret (Tab-Tab: the first Tab commits the snippet's completion entry, the second expands). Snippet entries (kind Snippet) join the C# and VB completion lists. Insert Snippet (Ctrl+K, Ctrl+X) and Surround With (Ctrl+K, Ctrl+S) open pickers; `eludite.editor.insert_snippet` and `eludite.editor.surround_with` with a `shortcut` insert without one. User snippets: every `*.snippet` under the folders `editor.snippets.paths` names (`;`-separated), read off the UI thread, overriding a built-in shortcut.
- Tab order (with brief 0068): the completion list first (its own binding), then an active snippet session (`in_snippet` context), then Tab-Tab, then Emmet, then indentation; the hook is one marked block in `EditorView::tab`.

**4. Override and partial-method completion.** An item whose `command` (from the list or `completionItem/resolve`) is `roslyn.client.completionComplexEdit` runs after the commit: its arguments' text edit replaces its range in the committed text (positions in the text the server had, mapped through the commit), as a snippet when the third argument is true, and the caret goes to the fourth argument's offset when it is not -1. The commit and the edit are one undo step.

**5. `///` doc comments.** Typing the third `/` of `///` (C#) or `'` of `'''` (VB), and Enter inside a doc comment, sends `textDocument/_vs_onAutoInsert` when the server advertises `_vs_onAutoInsertProvider` with that character; the answer's edit is applied (as a snippet when `_vs_textEditFormat` is 2) when the document did not change since. The host forwards the method.

**6. Diagnostics in Quick Info.** Hovering (or Ctrl+K, Ctrl+I) where diagnostics overlap shows, above the symbol's Quick Info, one line per diagnostic: its code (a link to `codeDescription.href` when given), its message, and "Show potential fixes (Ctrl+.)". With no server hover, the diagnostics alone show.

**7. Formatting.** Format Selection (Ctrl+K, Ctrl+F; `eludite.editor.format_selection`) sends `textDocument/rangeFormatting` for the selection's whole lines (the caret's line when empty). Format on type sends `textDocument/onTypeFormatting` after a character the server lists in `documentOnTypeFormattingProvider` (Roslyn: `;`, `}`, Enter) when `editor.formatOnType` (default on); format on paste sends `rangeFormatting` for the pasted range when `editor.formatOnPaste` (default on) and the server offers range formatting. Edits are applied through the applier with the version they were computed on, one undo step, dropped when the document changed. Format on save for C# and VB is `editor.formatOnSave.roslyn` (off by default) on brief 0050's path. The host forwards `textDocument/rangeFormatting` and `textDocument/onTypeFormatting`.

**8. Remove and Sort Usings, Fix All, Code Cleanup.**
- Remove and Sort Usings (Ctrl+R, Ctrl+G; `eludite.editor.remove_and_sort_usings`) asks `textDocument/codeAction` over the whole document with `only: ["source.organizeImports"]`, resolves the first action and applies it.
- Fix All: Roslyn's nested Fix All actions (`roslyn.client.fixAllCodeAction`) show under their fix as "Fix all occurrences in: Document / Project / Solution"; choosing one sends `codeAction/resolveFixAll` with the scope and applies the edit.
- Code Cleanup (Ctrl+K, Ctrl+E; `eludite.editor.code_cleanup`) runs profile 1 or 2: Remove and Sort Usings when the profile lists it, then each Fix All the profile lists (by diagnostic id, document scope), then Format Document; settings `editor.codeCleanup.profile1` and `profile2` (lists of steps) and `editor.codeCleanup.onSave` (off).

**9. Roslyn options.** The shell sends `workspace/didChangeConfiguration` `{ settings: { <eludite key>: value } }` to the host after the solution opens and whenever a mapped setting changes; the host keeps the values, answers Roslyn's `workspace/configuration` through `RoslynOptions`' one table (Eludite key to Roslyn section), and forwards `workspace/didChangeConfiguration` upstream so Roslyn pulls again. Mapped (the report lists the table): `dotnet.completion.showItemsFromUnimportedNamespaces`, `dotnet.completion.showNameSuggestions`, `dotnet.backgroundAnalysis.scope` (both analyzer and compiler scope), and the `editor.inlayHints.*` keys of brief 0066 (one block). Tools > Options > Text Editor > C# > Advanced shows them with Visual Studio's labels.

**10. Full-solution analysis.** With `dotnet.backgroundAnalysis.scope` `fullSolution`, the shell pulls `workspace/diagnostic` through the host after the solution loads and on `workspace/diagnostic/refresh`, and the Error List shows the closed files' errors and warnings as IntelliSense rows (removed when the scope goes back to `openFiles`).

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-editor snippet_session`: parse cases (nested, choices, linked, variables, escapes); a session's Tab, Shift+Tab, linked typing, Enter, Escape.
- `cargo test -p eludite-editor`: commit characters (commit then insert; space only on a hard selection; soft never).
- `cargo test -p eludite intellisense_depth_tests` (headless against the fake host): commit characters from `itemDefaults`; the codeAction range and its diagnostics; an LSP snippet item's stops and linked edits; Tab-Tab of `prop`; Surround With `if`; the complexEdit stub with the caret; `///` through `_vs_onAutoInsert`; hover with a diagnostic; `rangeFormatting` edits; on-type `;`; the organize-imports `only`; Fix All's `codeAction/resolveFixAll`; Code Cleanup's steps; the configuration push; workspace diagnostics in the Error List.
- `cargo test -p eludite-commands intellisense`: schemas and parsing.
- `dotnet test` (`Eludite.Host.Tests`): the forwarded methods, the configuration answer from the mapping table and its re-push; with the pinned server built: Extract Method offered on a selection, `///` generates `<summary>`, override completion carries the complex edit, organize usings removes an unused `using`.

## Budget

- Completion popup after trigger: < 50 ms p95 from the host, unchanged (commit characters are a lookup).
- Keystroke to frame submitted: < 8 ms p99 with a snippet session active (linked stops are selections; no extra pass).
- No network at startup; the snippet library is embedded, user folders are read off the UI thread.
- New dependencies: none outside the lockfile (`roxmltree` 0.20, MIT, already used by `eludite-resx`).

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` on every touched crate, `dotnet build dotnet/Eludite.slnx` and the host's tests green on Linux.
2. The report lists what the real Roslyn did for each feature (when the pin is built), the Roslyn option table, and the integration lines.

## Out of scope

Rider-only features (postfix templates, live template editing UI), a snippet designer, snippet `Imports`/`References` elements (read, ignored), Razor documents (brief 0078), F# (FsAutoComplete), the Error List's own columns beyond rows, CodeLens, inlay hints themselves (brief 0066), smart indent and brace completion (brief 0068), the save path itself (brief 0070), Code Cleanup's per-fixer UI beyond a list of ids.
