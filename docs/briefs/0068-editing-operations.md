# Brief 0068: Visual Studio's editing operations

Status: in progress
Phase: 2 (PLAN.md section 10: the editor at Visual Studio's level for .NET work)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5), 4.1 (the editor core: "Visual Studio's editing behavior", multi-caret, find), 5.1 (every action a command), 8 (Visual Studio's names, menus and shortcuts)
Related ADRs: ADR-0014 (tests fail on behavior only; budgets through `eludite-test-support`)
Depends on: brief 0009 (the editor core), brief 0012 (editor commands), brief 0013 (the editor's popups and Tab), brief 0020 (settings and Tools > Options), brief 0057 (`TextInput`)

## Goal

The editor edits like Visual Studio's code window. Tab and Shift+Tab indent and outdent selected lines instead of
deleting them; brackets and quotes close themselves and Enter between braces opens an indented body; Ctrl+K, Ctrl+C
comments lines in every built-in language; Edit > Advanced's line commands, case commands and white space command
work with every caret; Shift+Alt+= grows the selection by syntax; Alt+drag and Shift+Alt+arrows make a rectangular
selection; Ctrl+F and Ctrl+H open Quick Find and Quick Replace with Match Case, Match Whole Word, Regular Expressions
and Find in Selection; Ctrl+G is Go To Line; indentation follows the settings and `.editorconfig`; right-click opens
the code window's context menu. Every operation is an `eludite.editor.*` command an agent can call on a document.

## Files in scope

Owned (new unless noted):
- `crates/editor/src/editor.rs` (the editing model: hooks for the new operations, Tab and Enter), and the new modules
  `crates/editor/src/indent.rs` (indentation settings, block indent and outdent, smart indent), `autoclose.rs`
  (brace and quote completion), `comments.rs`, `lines.rs` (Edit > Advanced's line and case commands, the Clipboard
  Ring), `expand_selection.rs`, `box_selection.rs`, `find.rs` (the search engine of Quick Find and Quick Replace),
  `find_bar.rs` (the bar's view), `editorconfig.rs`; `crates/editor/src/syntax/editing.rs` (the per-language editing
  table: comment tokens, bracket pairs, quotes, string forms, indent rules), with a one-line accessor in
  `crates/editor/src/syntax/language.rs` and its `mod` line in `syntax/mod.rs`.
- `crates/editor/src/view.rs`: one contiguous block of actions and key bindings marked "brief 0068", the listeners
  for them, and the find bar's and context menu's hooks (additive).
- `crates/editor/src/lib.rs` (module lines, re-exports, crate docs: additive).
- `crates/editor/tests/editing.rs` (headless view tests through keys).
- `crates/commands/src/editing.rs`: the `eludite.editor.*` editing commands, their parsing and specs, the
  `EditingCommands` target trait; its `mod` line in `crates/commands/src/lib.rs`.
- `protocol/schemas/editor-*.json` new files only (listed under Contract).
- `crates/eludite/src/shell/editing.rs` (the shell's target, Go To Line, the context menu, indentation settings and
  EditorConfig per document) and `crates/eludite/src/shell/editing_tests.rs`; their `mod` lines and registration in
  `crates/eludite/src/shell.rs`, one-line hooks where documents open and save (`shell/documents.rs`).
- Additive entries in `crates/ui/src/menu.rs` (`vs_menus`: Edit, Edit > Advanced), `crates/ui/src/keymap.rs`
  (`vs_keymap`), `protocol/schemas/settings.json` and the key list test in `crates/commands/src/settings.rs`.
- This brief and `docs/briefs/0068-report.md`.

Not touched: `crates/editor/src/display.rs` and the vertical display map (brief 0067), `buffer.rs` (brief 0070),
`shell/intellisense.rs`, `code_actions.rs`, `format.rs` (brief 0069), `shell/navigation.rs`, `references.rs`
(brief 0071), `README.md`, `CLAUDE.md`, `docs/PLAN.md`, `docs/briefs/README.md`.

## Contract

### Editing model (`crates/editor`, no GPUI)

Every operation below is a method on `Editor`, works on every selection, and is one undo step that restores the
selections it started with. Offsets are UTF-8 bytes, as today.

**Indentation settings.** `IndentSettings { insert_spaces, indent_size, tab_size, auto_indent: None | Block | Smart,
auto_close: bool }`, default 4 spaces, Smart, on (`Editor::set_indent_settings`). An indent unit is `indent_size`
spaces, or a tab when tabs are used (with `indent_size == tab_size`; otherwise tabs then spaces up to the width, as
Visual Studio writes mixed indentation). Widths count a tab to the next multiple of `tab_size`.

**Tab and Shift+Tab** (`Editor::tab`, `Editor::outdent`). Tab with a selection spanning more than one line, or a whole
line, indents every line the selections touch by one unit (empty lines stay empty) and keeps each selection over the
same text, extended to the start of its first line as Visual Studio does. Tab with carets or a selection within a line
inserts white space to the next indent stop (a tab character with tabs). Shift+Tab outdents every touched line by one
unit: up to one unit of leading white space is removed, a tab counting as `tab_size` columns and mixed tabs and spaces
outdented by width (re-written in the setting's style when the remaining width is not representable); with a caret
inside the white space before the first character and no selection, the caret line is outdented. `EditorView::tab` is
the single function Tab dispatches to; the completion list (its own binding) and Emmet keep taking Tab first, and
`EditorView::set_tab_handler` lets another owner (snippet sessions, brief 0069) intercept it before indentation.

**Brace completion** (`autoclose.rs`). With `auto_close` on, typing an opening character of the language's pairs at
a caret inserts the pair and leaves the caret between: `(` `[` `{` everywhere, `"` and `'` where the language has
them as quotes (not `'` in Visual Basic, F#'s type parameters or Rust lifetimes: `'` closes only after a non-word
character in Rust and F#), `` ` `` in JavaScript and TypeScript, `<` never (Razor and HTML tags are Emmet's).
It does not auto-close inside a comment or a string (lexical scan of the line, with the language's line comment and
string forms: C#'s `@"..."`, `$"..."`, `$@"..."`, `"""raw"""`; Rust's `r#"..."#`; F#'s `"""..."""`; VB's `""`
doubling), nor when the character after the caret is a word character. A quote closes only when the line's quote
count before the caret is even. With a selection, typing an opener surrounds the selection (Visual Studio 17.x).
Typing a closer immediately before the same closer that the editor inserted skips over it; Backspace between an
empty auto-inserted pair deletes both. The editor remembers inserted closers as anchors and forgets them when the
caret leaves their line.

**Smart indent.** Enter keeps the line's indentation (Block) or, in Smart mode, also follows the language's lexical
rules (`syntax/editing.rs`): C#, Rust, JavaScript, TypeScript, CSS, JSON, Razor's code: one unit more after a line
ending in `{`, `(` or `[` (ignoring a trailing comment); Enter between `{` and `}` (also `(`/`)`, `[`/`]`) puts the
caret on an indented line and the closer on its own line at the opener's indentation. Visual Basic: one more after a
line opening a block (`Sub`, `Function`, `Property`, `Class`, `Module`, `Structure`, `Interface`, `Enum`,
`Namespace`, `If ... Then` without a statement after `Then`, `Else`, `ElseIf`, `For`, `For Each`, `While`, `Do`,
`Select Case`, `Case`, `Try`, `Catch`, `Finally`, `Using`, `With`, `SyncLock`, `Get`, `Set`), unless the line is a
one-liner or declared `MustOverride`/`Declare`. F# (offside): one more after a line ending in `=`, `->`, `then`,
`else`, `do`, `with`, `try`, `finally`, `(`, `[`, `[|`, `{`, or a `begin`/`struct`/`class`/`interface`. HTML and
Razor markup: one more after an opening tag that is not closed on the line and not a void element. Typing `}` (or
`)`, `]`) as the first non-blank character of a line re-indents the line to its matching opener's line indentation;
in Visual Basic typing the space or Enter after `End ...`, `Next`, `Loop`, `Wend`, `Else`, `ElseIf`, `Case`,
`Catch`, `Finally` re-indents the line one unit less than the block's line. The rules are lexical and synchronous
(the keystroke budget, invariant 1): the syntax tree lives on the syntax thread, so tree-sitter `indents.scm`
queries are not used (see Out of scope).

**Comments** (`comments.rs`). Each language has an optional line comment and an optional block comment: `//` and
`/* */` for C#, Rust, JavaScript, TypeScript, TSX, JSON with comments; `'` for Visual Basic; `//` and `(* *)` for F#;
`/* */` for CSS; `<!-- -->` for HTML; Razor: `@* *@` in markup, `//` inside a code block (detected lexically from the
nearest unclosed `@code {`/`@{`/`@functions {`). Comment Selection (Ctrl+K, Ctrl+C) puts the line comment at the
lowest indentation of the touched non-blank lines followed by one space, or, for a language with only a block comment
or a selection within one line, wraps the selection in the block comment. Uncomment Selection (Ctrl+K, Ctrl+U)
removes one line comment token (and the space after it) from each touched line that has one, or the block comment
around or inside the selection. Toggle Line Comment (Ctrl+/) comments when any touched non-blank line is
uncommented, else uncomments. Toggle Block Comment (Ctrl+Shift+/) wraps or unwraps the selection (the word at a
caret) in the block comment. JSON (no comments) does nothing.

**Line commands** (`lines.rs`), each multi-caret, one undo step:
- Duplicate (Ctrl+D): a caret duplicates its line below and moves to the copy; a selection duplicates its text after
  itself and selects the copy.
- Move Selected Lines Up and Down (Alt+Up, Alt+Down): the lines touched by each selection block swap with the line
  above or below; selections move with them; blocks at the top or bottom stay.
- Delete Line (Ctrl+Shift+L): the touched lines go, without the clipboard. Line Cut (Ctrl+L): the touched lines go
  to the clipboard (returned) and go.
- Open Line Above (Ctrl+Enter) and Open Line Below (Ctrl+Shift+Enter): a new line with the line's indentation.
- Make Uppercase (Ctrl+Shift+U) and Make Lowercase (Ctrl+U): the selection (the character at a caret, which then
  moves right, as Visual Studio does), Unicode case mapping.
- Scroll Line Up and Down (Ctrl+Up, Ctrl+Down): the view scrolls one line; the caret stays unless it leaves the view
  (view-only, no edit).
- Transpose Line (Shift+Alt+T): the caret line swaps with the line below, the caret follows.
- Delete Horizontal White Space (Ctrl+K, Ctrl+\\): the spaces and tabs around each caret (or in each selection) go.
- Cycle Clipboard Ring (Ctrl+Shift+V): the editor keeps the last 20 copied or cut texts; Ctrl+Shift+V pastes the
  newest, and again immediately replaces that paste with the next older one, wrapping.

**Expand and Contract Selection** (`expand_selection.rs`; Shift+Alt+= and Shift+Alt+-). Expand grows each selection
to the next larger syntax range: the word, the string's contents, the string, then each enclosing tree-sitter node
whose range is strictly larger (expressions, the statement, a bracket's contents, the block, the member, the type),
then the whole document. Contract returns through the stack of earlier steps; any other selection change clears it.
The view asks for a syntax tree on the syntax thread (never parsing on the UI thread); while none is current, or for
a buffer with no language, the steps are lexical: word, quotes' contents, quotes, brackets' contents, brackets, line,
document. `textDocument/selectionRange` is not used (Out of scope).

**Box selection** (`box_selection.rs`). Alt+drag and Shift+Alt+Left/Right/Up/Down make a rectangular selection
between two (row, visual column) corners, columns past a line's end included (virtual space). It is held as one
selection per row (a row shorter than the box's left column gets a caret at its end) plus the box's corners, so every
other operation (typing, Backspace, Delete, copy) works per row. Typing on a row whose end is left of the box's
column pads it with spaces to the column first. Copy of a box puts its rows, one per line, on the clipboard and
remembers it as a block; paste of that block at a single caret inserts row i at the caret's visual column on the
caret's row + i (padding short rows, adding rows at the end of the document). Shift+Alt+Up and Down in a zero-width
box are carets on consecutive rows (today's Add Caret Above/Below, which they replace). Multi-caret: Insert Next
Matching Caret (Shift+Alt+., as today), Insert Carets at All Matching (Shift+Alt+;), Remove Last Secondary Caret
(Shift+Alt+,), Skip Occurrence (Ctrl+Shift+Alt+.: the newest selection moves to the next match).

**Quick Find and Quick Replace** (`find.rs`, `find_bar.rs`). `FindQuery` gains `whole_word`, `regex` and
`in_selection` (default false). Matching: case-insensitive matching folds Unicode case (`regex`'s `(?i)` with the
pattern escaped for plain text); whole word means the match neither starts after nor ends before a word character
(Visual Studio's rule, as in `eludite-search`); a regular expression uses .NET-like syntax as far as the `regex`
crate takes it (an invalid pattern shows its error in the bar and finds nothing); Find in Selection limits the scope
to the selections at the time it was turned on (anchors). `Editor::replace_next(replacement)` replaces the current
match if the primary selection is one, then selects the next; `Editor::replace_all(replacement)` replaces every match
in scope as one undo step and returns the count; with a regex, `$1`, `${name}` and `$$` expand. The bar (Ctrl+F
Quick Find, Ctrl+H Quick Replace) has a find box and, in replace mode, a replace box (both `TextInput`), toggles
Match Case (Alt+C), Match Whole Word (Alt+W), Use Regular Expressions (Alt+E), Find in Selection (Alt+I), buttons
for Find Previous, Find Next, Replace Next (Alt+R) and Replace All (Alt+A), a match count "3 of 17" ("No results"),
and the find box's history (the last 20 queries, Up and Down). Enter is Find Next, Shift+Enter Find Previous,
Escape closes; F3 and Shift+F3 repeat with the bar closed; Ctrl+F3 finds the next occurrence of the selection (the
word at a caret), Ctrl+Shift+F3 the previous. The bar opens with a single-line selection as the query and, with a
multi-line selection, Find in Selection on.

**Go To Line.** `Editor::go_to_line(line)` (1-based, clamped) puts the caret at the line's first non-blank
character.

**EditorConfig** (`editorconfig.rs`). `EditorConfig::resolve(path)` reads `.editorconfig` files from the file's
folder up to the first with `root = true` (or the file system root), and applies sections in order, farther files
first and later sections of a file over earlier ones (nearest wins), with EditorConfig's glob rules: a pattern
without `/` matches the file name in any folder below the file, with `/` it is relative to the file's folder; `*`,
`**`, `?`, `[abc]`, `[!abc]`, `{a,b}`, `{1..3}`, `\` escapes; keys and values case-insensitive; `unset` removes a
value. It returns `indent_style`, `indent_size` (`tab` meaning `tab_width`), `tab_width` (defaulting to
`indent_size`), `trim_trailing_whitespace`, `insert_final_newline`, `end_of_line` and `charset`. It is our own parser
(no dependency).

### Commands and schemas (`crates/commands/src/editing.rs`, `protocol/schemas/editor-*.json`)

Every command takes an optional `path` (an open document; default the active one) and returns
`editor-edit.output.json`: `path`, `changed` (the text changed), `selections` (1-based `line`, `column` of each
selection's start and end, `primary` index), `line_count`. Class `EditBuffer` for those that edit, `Read` for those
that only move the selection. Ids and extra input:
- `eludite.editor.indent`, `outdent`, `comment_selection`, `uncomment_selection`, `toggle_line_comment`,
  `toggle_block_comment`, `duplicate`, `delete_line`, `line_cut`, `open_line_above`, `open_line_below`,
  `make_uppercase`, `make_lowercase`, `transpose_line`, `delete_horizontal_white_space`: `editor-edit.input.json`
  (`path` only).
- `eludite.editor.move_lines`: `direction` `up` or `down` (`editor-move-lines.input.json`).
- `eludite.editor.expand_selection`, `contract_selection` (Read): `editor-edit.input.json`.
- `eludite.editor.select`: `ranges` (1-based `line`, `column`, `end_line`, `end_column`) or `box` (two corners) to
  set selections, so an agent can set up the other operations (`editor-select.input.json`; Read).
- `eludite.editor.go_to_line`: `line` (1-based); without `line` it opens the Go To Line dialog
  (`editor-go-to-line.input.json`, output `editor-edit.output.json` plus `dialog`; Read).
- `eludite.editor.replace`: `query`, `replacement`, `match_case`, `whole_word`, `regex`, `in_selection`, `all`
  (default false: Replace Next) (`editor-replace.input.json`, `editor-replace.output.json`: `path`, `replaced`,
  `matches` before replacing, `error` for an invalid pattern; EditBuffer). Without `query` it opens Quick Replace.
- `eludite.editor.paste_ring` (Ctrl+Shift+V) and the Edit menu's `eludite.edit.cut`, `copy`, `paste`, `select_all`
  (EditBuffer except copy and select_all, Read): `editor-edit.input.json`.

The shell registers them through `EditingCommands`, runs them on the UI thread (agents' calls are queued to it and
answered within the bus's timeout), and the keys reach the editor's actions directly (the same `Editor` methods).

### Shell

- Edit menu: Undo, Redo, Cut, Copy, Paste, Select All work (Cut, Copy, Paste, Select All are registered by this
  brief); Edit > Find and Replace > Quick Find (Ctrl+F), Quick Replace (Ctrl+H); Go To > Go To Line... (Ctrl+G);
  Edit > Advanced: Format Document (moved under it only if brief 0069 does not), Comment Selection, Uncomment
  Selection, Toggle Line Comment, Toggle Block Comment, Make Uppercase, Make Lowercase, Move Selected Lines Up, Move
  Selected Lines Down, Duplicate, Delete Line, Line Cut, Transpose Line, Delete Horizontal White Space, Expand
  Selection, Contract Selection, Increase Line Indent, Decrease Line Indent; the shortcuts shown are `vs_keymap`'s.
- Go To Line: a small modal over the editor, "Go To Line", label "Line number (1 - N):", a text box with the caret's
  line selected, OK and Cancel; Enter goes, Escape cancels, a number out of range is clamped.
- The context menu on right-click in the text: Quick Actions and Refactorings... (Ctrl+.), Rename... (Ctrl+R,
  Ctrl+R), Go To Definition (F12), Find All References (Shift+F12), Run Tests and Debug Tests (when CodeLens knows a
  test at the caret's member), Breakpoint > Insert Breakpoint (F9), Cut, Copy, Paste, Comment Selection, Uncomment
  Selection. Right-click outside the selection moves the caret there first. Other briefs add items with one
  `ContextMenuItem` line in `EDITOR_CONTEXT_MENU` (label, command, args, shortcut lookup, optional `when`).
- Settings (Tools > Options > Text Editor > All Languages > General and > Tabs, and per language > Tabs, generated from
  the schema): `editor.autoClosingBrackets` (true), `editor.autoIndent` (`none`, `block`, `smart`; `smart`),
  `editor.insertSpaces` (true), `editor.tabSize` (4), `editor.indentSize` (4), and per language
  (`csharp`, `vb`, `fsharp`, `rust`, `typescript`, `javascript`, `json`, `css`, `html`, `razor`)
  `editor.languages.<id>.indentStyle` (`default`, `spaces`, `tabs`), `editor.languages.<id>.tabSize` and
  `indentSize` (0: follow All Languages). Resolution for a document: All Languages, then the language's page, then
  `.editorconfig` (resolved off the UI thread when the document opens; until it arrives the settings apply).
  On save, `trim_trailing_whitespace` and `insert_final_newline` apply to the buffer first (one undo step).

## Proving test

- `cargo test -p eludite-editor`: unit tests per operation: block indent of three lines keeping the selection; outdent
  of lines indented with tabs, spaces and both; Tab inside a line; brace pairs, skip-over, Backspace of an empty pair,
  no pair inside a string or comment (C# verbatim, interpolated and raw strings), surround a selection; Enter between
  braces; `}` outdent; VB and F# smart indent; comment and uncomment per language (C#, VB, F#, CSS, HTML, Razor);
  duplicate and move with three carets; delete line, line cut, open line, case, transpose, white space, the
  Clipboard Ring; expand selection steps on a C# fixture with a tree and lexically; box selection typing with virtual
  space and block paste; find with whole word, regex, Unicode case and in selection; Replace All as one undo step;
  EditorConfig resolution order, globs and `root = true`.
- `crates/editor/tests/editing.rs`: headless view tests through keys: Tab and Shift+Tab, `{` Enter, Ctrl+K Ctrl+C,
  Alt+Down, Shift+Alt+=, Alt+drag, Ctrl+H and Replace All.
- `crates/commands`: the schemas parse, name their commands, and the input is checked.
- `crates/eludite/src/shell/editing_tests.rs`: headless shell tests through the commands (an agent's comment, move,
  replace all, go to line, expand selection), the Edit menu's items, Ctrl+G's dialog, the context menu's items, and
  `.editorconfig` applied to a document opened from a temporary folder.
- Budgets (`assert_budget`): on a 50,000-line C# buffer, block indent of every line, move-line of a 1,000-line block,
  comment of every line, Replace All of 10,000 matches, and expand selection each finish in under 8 ms.

## Budget

Every operation under 8 ms on a 50,000-line file on the UI thread (the keystroke budget); syntax trees for Expand
Selection come from the syntax thread. No new dependency.

## Exit criterion

The proving tests pass; `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` are clean for every crate
touched; the report records the numbers, what was not run, and the integration lines.

## Out of scope

Tree-sitter `indents.scm` queries (smart indent is lexical and synchronous; a later brief can move it to the syntax
thread's tree), `textDocument/selectionRange` and `textDocument/onTypeFormatting` (brief 0069's formatting work),
Roslyn's own smart indent, virtual space outside box selection (Tools > Options' "Enable virtual space"), Find in
Selection across a box's virtual columns, multi-line find boxes, the display's tab width (brief 0067 owns the display
map; it keeps drawing a tab as four columns until it reads the setting), `end_of_line` and `charset` (applied by the
document code for new files, brief 0070), outlining items in the context menu (brief 0067 adds its line).
