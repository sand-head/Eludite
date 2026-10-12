# Brief 0077: The debugger's windows and inspection at Visual Studio's level

Status: in progress
Phase: 2 (PLAN.md section 10: the .NET debugging experience on par with Visual Studio and Rider; accepted by the owner's direct request of 2026-10-11, "do everything required for Eludite to become on par with Visual Studio/Rider for .NET work")
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 12), 4.5 (debugging: "Locals, Watch, Call Stack, Threads, Breakpoints, Immediate, Modules, data tips, the exception helper"), 5.5 (two drivers), 8 (Visual Studio's layout and shortcuts)
Related ADRs: ADR-0002 (the debugger is a process the shell speaks DAP to), ADR-0003 (DAP, no bespoke hooks), ADR-0014 (tests fail on behavior only)
Depends on: brief 0018 (the debugger and its windows), 0020 (the Output window's Debug source; the Immediate window named as the Debug Console's successor), 0025 (inspection: `exception_info`, `variables`), 0026 (run control: `set_variable`, conditions, hit counts, tracepoints), 0028 (several sessions), 0036 (breakpoint failure reasons), proposal 0001 (the rules of the debug commands).

## Goal

A person debugging a .NET program gets Visual Studio's inspection windows and helpers instead of one-line text: at an exception stop the **exception helper** opens at the execution point with the type, message and inner exceptions and the "Break when this exception type is thrown" checkbox; resting the mouse on an expression shows a **DataTip** that expands objects lazily, edits values and adds watches; the **Immediate window** evaluates in the selected frame with history; **QuickWatch** (Shift+F9) inspects an expression in a dialog; **Autos** lists the variables of the current and previous statement; **Watch 1 to 4** edit expressions in place; Locals marks **changed values in red**; the Call Stack collapses **[External Code]**; Threads shows every thread's location; the **Modules** window lists loaded modules from DAP `module` events; the Breakpoints window and the Debug menu **enable, disable and delete all**; **Show Next Statement** (Alt+Num*) brings the execution point back; the margin glyph has a **right-click menu, a tooltip and an inline Breakpoint Settings panel**; **Just My Code** is a setting passed to the adapter; and the shell **switches to a Debug layout** at session start and back to the Design layout at the end.

## Files in scope

**Shell (owner: this brief).**
- New modules under `crates/eludite/src/shell/debug/`: `inspect.rs` (the shared state of these features in the `Debugger`, the one `Pending::Inspect` answer router, Show Next Statement, changed values, external-code rows, thread locations, modules, Enable/Disable All, the layout switch), `exception_helper.rs`, `datatips.rs`, `immediate.rs`, `quickwatch.rs`, `autos.rs`, `modules.rs`, `breakpoint_settings.rs`, `inspect_tests.rs` (headless tests).
- `crates/eludite/src/shell/debug/windows.rs` and `state.rs`: Watch 1 to 4, the changed-value flag on rows, context menus, keyboard navigation, in-place edits, `[External Code]` rows, Enable/Disable All and search in the Breakpoints window, Hexadecimal Display.
- `crates/eludite/src/shell/debug.rs`: only marked blocks (`// Brief 0077`): the `mod` lines, the `Pending::Inspect` variant and its `on_response` arm, the `module` event arm, the exception info hook, the hover hook, the window refresh hook, the dispatch of the new requests, the session start and end hooks. Nothing is moved out of it or reformatted (briefs 0065 and 0074 edit it too).
- `crates/eludite/src/shell.rs`, `options.rs`, `settings.rs`, `toolbar.rs`: additive registration lines only.

**Editor.** `crates/editor/src/anchored.rs` (new): popups the owner supplies as GPUI views, anchored under a buffer position (the exception helper, DataTips, Breakpoint Settings), with the marked hooks in `popups.rs` (one field, one render line) and `lib.rs`; `crates/editor/src/debugging.rs`: the margin's right click and hover reported as events (`BreakpointMarginContext`, `BreakpointMarginHovered`, in `intellisense.rs`'s `EditorEvent`, marked).

**DAP.** `crates/dap/src/types.rs`: `Event::Module(ModuleEvent)` and `Module` (marked); `crates/dap/src/fake.rs`: `FakeProgram::modules`, sent as `module` events after `configurationDone` (marked); `crates/dap/src/launch.rs` and `attach.rs`: one marked line each reading `justMyCode` from the setting passed in.

**Commands and protocol (schemas first).** `protocol/schemas/debug-watch.input.json` (additive: `window`, `edit`, `move`), new `debug-show-next-statement.input.json`, `debug-modules.input.json`, `debug-modules.output.json`, `debug-quickwatch.input.json`, `debug-enable-breakpoints.input.json`; `crates/commands/src/debug.rs` (additive, marked: the ids, specs, parsing, one `DebugRequest::Inspect(InspectRequest)` variant), `crates/commands/src/settings.rs` keys (additive).

**Docking, menus, keys.** `crates/docking/src/model.rs` (window ids `immediate`, `autos`, `watch_2`, `watch_3`, `watch_4`, `modules`, registered), `crates/docking/src/persist.rs` (named Design and Debug layouts kept and swapped), `crates/ui/src/menu.rs`, `crates/ui/src/keymap.rs` (additive blocks).

**Docs.** This file and `docs/briefs/0077-report.md`. `README.md`, `CLAUDE.md`, `docs/briefs/README.md` and `docs/PLAN.md` are the integrator's: the report's Integration notes carry their lines.

## Contract

**Commands.** Every action is a command reachable by agents over MCP (invariant 3), schema first (invariant 4):
- `eludite.debug.watch` gains `window` (1 to 4, default 1: Watch 1's list is the existing `watches`), `edit` (`{"index", "expression"}`: replaces an expression in place) and `move` (`{"from", "to"}`). One of `add`, `remove`, `edit`, `move` per call. The answer is the state, whose `watches` stays Watch 1's list; Watches 2 to 4 are listed in `watch_windows` only when non-empty (additive to the state output).
- `eludite.debug.show_next_statement` (Alt+Num*, Debug > Show Next Statement, the Debug toolbar): needs break mode; selects the current thread's frame 0, opens its document at the statement and scrolls to it, and reopens the exception helper at an exception stop. Answers the state.
- `eludite.debug.modules` (Read): the session's modules from `module` events: `id`, `name`, `path`, `optimized`, `user_code`, `symbol_status`, `version`, in load order, at most 2,000.
- `eludite.debug.quickwatch` (Read; Shift+F9, Debug > QuickWatch...): opens the QuickWatch dialog with `expression` (default: the selection, else the expression at the caret) and evaluates it in the selected frame with context `watch`; needs break mode. Answers `eludite.debug.evaluate`'s output shape.
- `eludite.debug.enable_breakpoints` (Execute): `{"enabled": bool}` with `all: true` enables or disables every breakpoint (Debug > Enable All Breakpoints / Disable All Breakpoints, the Breakpoints window's buttons); with `path` and `line` (default: the caret's line) it sets that breakpoint's, or toggles it when `enabled` is absent (Ctrl+F9). Delete All stays `eludite.debug.toggle_breakpoint` with `delete_all`, now bound to Ctrl+Shift+F9.
- The Immediate window submits `eludite.debug.evaluate` with context `repl`; QuickWatch's View Details from the exception helper is `eludite.debug.quickwatch` with `$exception`; the helper's checkbox is `eludite.debug.exception_settings` with the exception type; the Breakpoint Settings panel writes through `eludite.debug.toggle_breakpoint` with `action: set`.

**Exception helper.** When `exceptionInfo` answers at an exception stop, a popup opens under the execution point's line in its document: the title "Exception Thrown" (break mode `always`) or "Exception User-Unhandled" (`userUnhandled`, `unhandled`), the exception type, the message, the inner exceptions from `details.innerException` (each "Inner Exception: Type: message", nested), the links View Details, Copy Details and Open Exception Settings, and the checkbox "Break when this exception type is thrown" checked from the exception settings (the type's row, else the category's `break_when_thrown`). Toggling it adds or updates the type's row. "Except when thrown from" is shown with the frame's module name when the adapter reports one, read only (Visual Studio's module condition needs exception options the adapters ignore). Escape or the close glyph dismisses it; it closes when the debuggee resumes; Show Next Statement reopens it.

**DataTips.** In break mode a hover evaluates the expression with context `hover` (unchanged). A success opens a DataTip under the expression: rows of name, value and type; a row with members expands lazily through `variables` (the Locals tree model, `VarNode`); a double click on a value edits it in place, Enter sends `eludite.debug.set_variable` (a top-level local by name, a member through its parent's reference, an expression with `setExpression` through the same command where the adapter supports it); each row offers Copy Value and Add Watch (to Watch 1). A failed evaluation shows no tip. Holding Ctrl makes the tip transparent (opacity 0.2). The tip closes at the next hover elsewhere, Escape, and when the debuggee resumes. A members answer is applied in the frame it arrives. Pinning is out of scope.

**Immediate window** (`immediate`; Debug > Windows > Immediate, Ctrl+Alt+I): a transcript and a one-line input (`TextInput`); Enter evaluates the line in the selected frame with context `repl`; `?expr` and `? expr` evaluate `expr`; `>cmd` lines are refused with Visual Studio's message that commands are not supported; Up and Down walk the history (100 entries, no duplicates in a row); results are written as text, an object result followed by its expandable members as indented lines when the user clicks it. Outside break mode a line answers "The expression cannot be evaluated while the debuggee runs.".

**QuickWatch.** A modal dialog: Expression box with its history (Up and Down), Reevaluate, Add Watch, Close; the result tree with Name, Value, Type, lazily expanding, its values editable as in DataTips.

**Autos** (`autos`; Ctrl+Alt+V, A): at each stop and frame selection, the identifiers and member access chains (`order.Name`, `this.x`) found in the current statement's line and the previous statement's line of the frame's document (a token scan over the buffer's text, skipping keywords, string and comment text, and names followed by `(`), at most 20, in order of appearance, each evaluated with context `watch` in the selected frame, shown as a variables tree. When the last step was a step out and the adapter returns a `$ReturnValue` or a `Return value` local, it is listed first.

**Watch 1 to 4** (`watch`, `watch_2`, `watch_3`, `watch_4`; Ctrl+Alt+W, 1 to 4): the last row is Visual Studio's empty "Add item to watch" row (typing in it adds); F2 or a double click on a name edits the expression in place (Enter commits through `edit`, Escape cancels); Delete removes the selected watch; a double click on a value edits it (`set_variable` through the watch's parent, or `setExpression` for a top-level watch where supported); a Locals row's Add Watch adds its evaluate name; the editor's Add Watch adds the selection. Each window evaluates in the selected frame at every break.

**Locals, Call Stack, Threads, Breakpoints.** Rows whose value differs from the previous stop's value at the same path in the same frame function are drawn red (Locals, Autos, Watch 1 to 4). A right click opens Visual Studio's context menu: Copy, Copy Value, Edit Value, Add Watch, Hexadecimal Display, Expand All Children, Collapse All Parents (variables windows); Copy, Copy All, Show External Code, Switch To Frame, Run To Cursor (Call Stack); Up and Down move the selection, Right and Left expand and collapse, Enter switches frame (Call Stack). With `debugger.showExternalCode` off (the default), consecutive frames that are external (no source, or `presentationHint` `subtle`) are drawn as one `[External Code]` row; the setting and the context menu item show them. Threads shows each thread's top frame name (one `stackTrace` with `levels: 1` per thread at each stop, capped at 64 threads). The Breakpoints window gains Enable All, Disable All, Delete All and a search box filtering rows by file, line, function, condition or message; Ctrl+F9 toggles the caret line's breakpoint enabled; Ctrl+Shift+F9 deletes all. Hexadecimal Display (`debugger.hexadecimalDisplay`, off) formats decimal integer values as `0x...` in every variables view; the evaluation stays the adapter's.

**Modules** (`modules`; Ctrl+Alt+U): DAP `module` events (`reason` `new`, `changed`, `removed`) decoded as `Event::Module` and kept per session; columns Name, Path, Optimized, User Code, Symbol Status, Version; and Visual Studio's line in the Output window's Debug source for each new module: `Loaded '<path>'. Symbols loaded.` (or `Skipped loading symbols.` when the status says so).

**Breakpoint glyphs and settings.** A right click on a margin glyph opens Delete Breakpoint, Disable Breakpoint / Enable Breakpoint, Conditions..., Actions... (the editor reports `BreakpointMarginContext { row, position }`; the shell draws the menu). A hover over a glyph shows a tooltip: `Program.cs, line 6`, the condition, the hit count condition, `Hits: n`, and the binding (`The breakpoint will not currently be hit: <reason>` when unbound). Conditions... and Actions... open the inline Breakpoint Settings panel as an anchored popup under the line: Conditions (Conditional Expression: "Is true" / "When changed" with the expression; Hit Count: "=", "Is a multiple of", ">=" with the count), Actions (Log a message to Output window, Continue code execution), Close; changes write through `toggle_breakpoint` `set`. "When changed" is evaluated by the shell where the adapter has no such option: the condition text is sent as is with a "when changed" flag kept in the shell (out of scope beyond the UI if the model has no field: the report says so).

**Options and layouts.** Tools > Options > Debugging > General: Enable Just My Code (`debugger.justMyCode`, default on, passed as `justMyCode` at launch and attach), Enable property evaluation and other implicit function calls (`debugger.enablePropertyEvaluation`, default on, passed as netcoredbg's `enableStepFiltering`-independent `evaluationOptions` only where the adapter names one; otherwise kept for later adapters), Show External Code (`debugger.showExternalCode`), Hexadecimal Display. At a session's start (design to debugging) the shell saves the current layout as the named layout `Design` and applies the named layout `Debug` when one was saved (else today's `show_debug_windows`); when the last session ends it saves the current layout as `Debug` and applies `Design`. Each keeps the user's changes.

**Shortcuts and menus** (Visual Studio's): Ctrl+Alt+E Exception Settings, Ctrl+Alt+C Call Stack, Ctrl+Alt+V, L Locals, Ctrl+Alt+V, A Autos, Ctrl+Alt+W, 1 to 4 Watch 1 to 4, Ctrl+Alt+H Threads, Ctrl+Alt+I Immediate, Ctrl+Alt+U Modules, Shift+F9 QuickWatch, Alt+Num* Show Next Statement, Ctrl+F9 Enable Breakpoint, Ctrl+Shift+F9 Delete All Breakpoints; Debug > Windows gains Immediate, Autos, Watch 2 to 4, Modules; Debug gains QuickWatch..., Show Next Statement, Enable All Breakpoints, Disable All Breakpoints.

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

`cargo test -p eludite inspect_tests` (headless, against `eludite_dap::fake`):
- An exception stop opens the helper in Program.cs with the title, type, message and inner exception; its checkbox adds the type with `break_when_thrown` to the exception settings and the next `setExceptionBreakpoints` carries it; resuming closes it; Show Next Statement reopens it.
- A hover over `order` opens a DataTip whose row expands to `Id` and `Name` through one `variables` request; editing `Id` sends `setVariable`; Add Watch adds `order.Id`; a failed hover evaluation shows no tip.
- The Immediate window evaluates `?x` with context `repl` and writes the answer; Up recalls it; outside break mode it answers the running message.
- Shift+F9 with `y` opens QuickWatch, evaluates it, Add Watch adds it to Watch 1.
- Autos at Program.cs line 6 lists `y`, `x` (and not `Calc`, `Add`, `var`).
- Watch 2: add, edit in place, delete through the window and through the command; Watch 1 is untouched.
- Locals: `x` is red after a step that changed it, not before.
- The Call Stack collapses `[Native Frames]` into `[External Code]` with the setting off and lists it with the setting on.
- Threads lists the worker thread's top frame.
- Modules: the fake's two `module` events give two rows and the Output lines.
- Enable All / Disable All through the menu and Ctrl+F9 at the caret; Ctrl+Shift+F9 deletes all.
- The Breakpoint Settings panel sets a condition and a log message through `toggle_breakpoint`.
- Design/Debug layout: a window docked while debugging is back when the next session starts and gone after it ends.
- `justMyCode: false` reaches `launch` after the setting is set off.
`cargo test -p eludite-dap`: `module` events decode. `cargo test -p eludite-commands`: the new inputs parse and their schemas load. `cargo test -p eludite-editor`: an anchored popup is drawn under its row and dropped with its key. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` on every touched crate.

## Budget

- Every window's render with 500 rows under 8 ms at p99 (asserted with `assert_budget`, reported on CI; ADR-0014).
- A DataTip members answer is applied in the same frame it arrives (no extra round trip, no timer).
- No UI-thread wait on the adapter anywhere (invariant 1); every answer quotes its session generation and stop and is dropped when stale (invariant 12).
- No new dependency.

## Exit criterion

1. The proving tests pass; `cargo fmt --check` and clippy are clean on every crate touched; the existing debug tests still pass.
2. The report lists what was built, what was measured, and what is owed, with the Integration notes.

## Out of scope

Pinned DataTips and DataTip comments; Visual Studio's Parallel Stacks, Parallel Watch, Tasks, Memory, Disassembly, Registers and Diagnostic Tools windows; breakpoint labels, export and import; dependent breakpoints ("only enable when another breakpoint is hit") and filter conditions, unless the model gains them cheaply; Step Into Specific (no target adapter advertises `supportsStepInTargetsRequest`); symbol loading settings; Edit and Continue; the editor's own right-click menu entries (brief 0068 adds Add Watch, QuickWatch and the run-control items there); the Save/Apply Window Layout commands (brief 0078).
