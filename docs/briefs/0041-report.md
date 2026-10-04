# Brief 0041 report: The integrated terminal

Status: done on Linux. Windows and macOS: not built here (no targets or machines); the Windows paths (ConPTY through
`alacritty_terminal::tty`, the Developer PowerShell profile) compile only under `cfg(windows)` and were not run. CI:
not run (nothing pushed).
Branch: `brief/0041-integrated-terminal`, based on `main` at `0fc40cd`; not rebased (the coordinator merges).
Date: 2026-10-04. Brief: [0041-integrated-terminal.md](0041-integrated-terminal.md).

## 1. Summary

- **View > Terminal (Ctrl+`)** shows Visual Studio's Terminal tool window, tabbed at the bottom with the Error List and
  Output (the default layout; layout schema 4 migrates older layouts), with a shell in the workspace's folder. Each
  terminal has a tab with its profile's name (`bash`, `sh (2)`, `Developer PowerShell`), New Terminal (Ctrl+Shift+`),
  the profile dropdown, Split (two terminals side by side in one tab), Kill (asking when a command runs), Clear, the
  tab's close button, and after the shell ends the line "[Process exited with code N]" with Restart. Nothing starts at
  startup: the window shows "No terminal is open" until the menu, the key or a command opens one.
- **A real PTY and `alacritty_terminal`** (0.26.0, Apache-2.0, crates.io; never vendored, never Zed's view): the
  shell runs on a PTY from `alacritty_terminal::tty` (Unix PTYs; ConPTY on Windows), read by the terminal's own I/O
  thread, which parses into the grid in chunks of at most 16 KB under the grid's lock and raises one "changed"
  notification until the view has drawn (one grid snapshot per frame at most). The tools Eludite located go first on
  PATH once each (`dotnet` and `DOTNET_ROOT`, Cargo, Node.js): `dotnet --version` in the terminal printed 10.0.302, the
  SDK `global.json` pins (screenshot `terminal-dotnet.png`).
- **Our own GPUI view** (`eludite_terminal::TerminalView`): cells shaped with the cell width forced, the theme's 16
  colors (and xterm's 256 and true color), bold, italic, underline, inverse, the cursor's shapes, mouse selection
  (double-click a word), Visual Studio's clipboard keys, wheel and Shift+PageUp/PageDown scrolling, find in the
  scrollback (Ctrl+F in the window), the bell's flash, Ctrl+click links, bracketed paste.
- **Agents use the same terminals** through eight commands (`eludite.terminal.open`, `list`, `send`, `read`, `wait`,
  `resize`, `close`, `clear`; schemas `protocol/schemas/terminal-*.json`). What an agent types shows live; the tab says
  "Agent <name> is typing" while its `send` or `wait` runs (and 0.8 s after); the person's keystroke during the
  agent's `wait` ends it with `interrupted_by: "user"` and refuses the agent's next `send` until it `read`s. The
  policy's `terminal.run` decides for agents: `prompt` (default) asks at the first call of an agent session, and the
  prompt's "Allow for this session" lets the rest run without asking until the session ends (nothing written); `allow`
  runs them; `deny` refuses them. `read`, `wait` and `list` are always allowed. The transcript shows an agent's
  `send` as the text typed and a `wait` with an excerpt of what came (the last five lines). The guide is
  [docs/agents/terminal.md](../agents/terminal.md) (569 words), the MCP resource `eludite://guides/terminal`.
- **Shell integration** for bash (tested), zsh, fish and PowerShell (written; those shells are not installed here):
  OSC 133 prompt and command marks and OSC 7 folder reports from a script passed through each shell's startup options;
  no dotfile is touched. `wait` with `prompt` answers the command's exit code and its output without prompt or command
  line; without integration a heuristic applies (`integration: false`).
- **Budgets** (Ubuntu container, 4 cores, debug build with optimized dependencies; the other worktree's agent built
  and tested beside this one):

  | Budget | Result |
  |---|---|
  | Keystroke to the echoed character painted, under 16 ms p95 (the view's frame counter) | **2.9 ms p95** (p50 2.1 ms), 22 keys into `sh`, headless GPUI (`terminal_tests::typing_reaches_the_shell_and_the_output_renders`). The Xvfb run proved the flow but does not export the counter. Pass |
  | `cat` of a 100 MB file: frame p99 under 8 ms with the grid update deferred | **1.18 ms p99** (p50 0.52 ms, worst 4.4 ms) over 4,523 frames of the terminal element (`assert_budget`). Pass |
  | The output finishes within 2x the plain terminal's time | **0.95x**: 11.16 s drawn vs 11.70 s for the same PTY and emulator with nothing drawn. Pass |
  | 10,000 lines of scrollback under 20 MB per terminal | **19.2 MB at 80 columns; 28.8 MB at 120** (28.3 MB at the test window's 118). `alacritty_terminal` keeps 24 bytes per cell for every column of every line, so the budget holds up to 83 columns and grows with the width. **Over budget for wide terminals**: section 7.2 |
  | One new dependency | `alacritty_terminal` 0.26.0, Apache-2.0 (section 8). Pass |

- **Tests**: section 6; counts in section 11.

## 2. What was built

Commits on top of `0fc40cd`, in order:

1. `d54959c` Mark brief 0041 in progress.
2. `1e077d3` The schemas, first and alone: `terminal-{open,list,send,read,wait,resize,close,clear}.{input,output}.json`,
   the `terminal` object of `agents-policy.json`, the eight `terminal.*` settings (the Options page "Terminal", listed
   last), `view-show`'s `terminal` id, `mcp-resource.json`'s description of the new guide.
3. `b302694` `crates/terminal` (section 3) with its unit and PTY tests.
4. `d9dce4c` `crates/commands/src/terminal.rs` (parsing, typed outputs, classes, the escalation hooks), the policy's
   `TerminalPolicy`, `AlwaysAllow::Session` and `AlwaysAllow::Granted` with `PolicySnapshot::session_grants`, the
   registry keeping a granted call's class; `crates/mcp`: the guide resource.
5. `60fdaab` `crates/docking` (`ids::TERMINAL`, the default layout, schema 4 and its migration) and `crates/ui` (View
   > Terminal, Ctrl+`, Ctrl+Shift+`, Tools > Command Line opening a terminal, `WORKSPACE_TERMINAL_ITEM`).
6. `b95b3c8` The Agents window: Always Allow reads "Allow for this session" for a call whose escalation names a session
   grant; the grant lives in the agent session's policy store.
7. `73de31f` The transcript's terminal rows.
8. `67dec40` The shell: `shell/terminal.rs` (service, window, hooks) and `shell/terminal_tests.rs`.
9. `923198a` Transcript marks only grow when a line is redrawn (a bug the Xvfb run found: section 5).
10. `708c149` The view falls back to an installed monospace font (the Xvfb run drew a proportional fallback).
11. `e3b3fb1` The tool windows' chords stay the shell's (the Xvfb run typed Ctrl+\, Ctrl+C into the shell).
12. `fc32b07` `crates/eludite/tools/terminal-linux.sh` and the screenshots.
13. `6204745` The Options page test, the README's status line.
14. The shell integration scripts move from the temp folder to Eludite's cache folder.
15. This report, the brief's status, the index row and `CLAUDE.md`.

## 3. `crates/terminal`

- `pty`: `Terminal::spawn` (a PTY from `alacritty_terminal::tty::new`, the child's environment and folder), the I/O
  loop (modeled on `alacritty_terminal`'s event loop: `polling` on the PTY and the child's exit; queued writes;
  synchronized updates honored; resizes applied there), `write`, `paste` (bracketed when mode 2004 is on; ESC
  stripped from the text), `resize`, `clear` (scrolls the cursor's line to the top and drops the scrollback, so the
  prompt stays), `close` (SIGHUP, SIGKILL after a second; with `kill` the foreground job and the session at once; a
  dropped terminal is killed), `busy` (the terminal's foreground group is not the shell, by `tcgetpgrp`) and
  `foreground` (its name from `/proc`), `screen`, `scrollback`, `since(mark)`, and `wait` (prompt, pattern, exit,
  timeout, the person's interruption, a cancel check every 50 ms).
- `text`: the plain-text transcript (escapes removed, `\r` and backspace applied) with monotonic marks mapped through
  a table of line starts; 2 MB kept.
- `scan`: OSC 133 and OSC 7 taken out of the byte stream before the emulator (it ignores them), split reads held.
- `integration`: the scripts (`shell-integration/`), installed under Eludite's cache folder per version
  (`$XDG_CACHE_HOME/eludite/shell-integration`, else `~/.cache/...`; `%LOCALAPPDATA%` on Windows; the temp folder
  per user only without either), and how each shell gets them (section 4).
- `profile`: the platform's profiles and the user's; `env`: the located tools and the terminal's variables
  (`TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=Eludite`).
- `links`: urls and paths with `(line,col)`, `(line)`, `:line:col`, `:line`; `keys`: xterm's sequences.
- `view`: `TerminalView` and its `GridElement`, `bind_keys`.

## 4. Shell integration matrix

| Shell | How it gets the script | Marks | Tested here |
|---|---|---|---|
| bash | `--rcfile <dir>/bash.sh -i` (a login profile's `-l` becomes the script's own login sequence: `/etc/profile`, then the first of `~/.bash_profile`, `~/.bash_login`, `~/.profile`; otherwise `~/.bashrc`) | A, B (end of PS1), C (PS0), D;code (first in PROMPT_COMMAND), OSC 7 | **Yes**: bash 5.2.21, with no startup files and with Debian root's `.bashrc` (colored prompt, title escape); exit codes 0, 2, 3; wrapped command lines; the Xvfb run |
| zsh | `ZDOTDIR=<dir>/zsh` (its `.zshenv`, `.zprofile`, `.zshrc`, `.zlogin` source the user's from `ELUDITE_USER_ZDOTDIR`, their `ZDOTDIR` or home; `ZDOTDIR` is theirs again after `.zshrc`) | precmd: D;code, A, OSC 7, B in PS1; preexec: C | No (zsh not installed); the launch arguments are unit-tested |
| fish | `--init-command "source '<dir>/eludite.fish'"` (after the user's config) | fish_preexec: C; fish_postexec: D;code; fish_prompt wrapped: A, OSC 7, B | No (fish not installed); arguments unit-tested |
| PowerShell (5.1 and 7) | `-NoExit -Command ". '<dir>\eludite.ps1'"`; a Developer PowerShell's own `-Command` runs first (`& '...Launch-VsDevShell.ps1' -SkipAutomaticLocation; . '...eludite.ps1'`) | the prompt function wrapped: D;code (from `$?` and `LASTEXITCODE`, when the history grew), A, OSC 7, B; PSReadLine's Enter: C | No (PowerShell not installed here; "untested here" as the brief expected); arguments unit-tested |
| sh, dash, Command Prompt | none | none: the prompt heuristic (`$ `, `> `, `# `, `% ` at the cursor after 300 ms of silence), `integration: false`, no exit code | sh (dash 0.5.12): yes |

A shell told to run a command (`-c`, `-File`, `--norc`) is left alone. The terminal records the folder from OSC 7, so
links resolve against the folder the person `cd`ed into.

## 5. The bug the Xvfb run found

The first Xvfb run left the agent's `wait` running after the prompt came. bash's line editor wrapped the long command
line (a 100-column prompt in a 118-column terminal) by writing a lone `\r` and the rest of the line; the transcript
applied the `\r` by truncating its current line, so the text, and with it the marks, went back 91 bytes, and the
prompt's marks landed before the agent's mark. Marks are now the bytes printed, overwritten or not (they only grow), and
a line table maps a mark to the text kept (commit 9). `crates/terminal/tests/pty.rs::a_line_redrawn_after_a_mark_keeps_the_marks_growing`
reproduces it (it fails on the old transcript, passes on the new) and the unit test
`text::marks_only_grow_when_a_line_is_redrawn` pins the mapping. The same run showed two more defects, both fixed: no
"Noto Sans Mono" here, so a proportional fallback broke the grid (the view now picks the first installed of Cascadia
Mono, Noto Sans Mono, DejaVu Sans Mono, Liberation Mono, Menlo, Consolas, Ubuntu Mono, Courier New), and Ctrl+\, Ctrl+C
(the Agents window) went to the shell (the tool windows' chords are now reserved).

## 6. Tests

- **`crates/terminal`** (24 unit, 16 PTY tests on a real PTY, Unix):
  - unit: the scanner (marks in order, split reads, other sequences through, OSC 7 paths); the transcript (escapes,
    `\r` and backspace, marks across the cap, **marks only grow on a redraw**); links (MSBuild's and the compilers'
    forms, Windows paths, urls, what is not a link, character ranges); keys (control characters, Alt, xterm's cursor and
    function keys with modifiers, application cursor mode, typed text); profiles (shell kinds, Unix order, **Visual
    Studio's Windows names and the Developer PowerShell with and without Build Tools**, merging by name, picking the
    default); integration (each shell's startup option, never a dotfile; the scripts installed once); the
    environment (**tool folders first, once**; `dotnet` located by `DOTNET_ROOT`); bracketed paste bytes; the 256 colors.
  - PTY (`tests/pty.rs`): **the `echo` round trip** and the screen; **50,000 lines capped at a 1,000-line scrollback**
    (the transcript keeps the run); **resize changes what `tput cols` and `stty size` print**; **the exit code** (and
    its event); **selection text across wrapped lines** (10 columns); **links printed by the shell found on the
    screen**; **OSC 133 marks give per-command exit codes under bash** with the script (0, 3, 2; the output without the
    prompt; OSC 7's folder; read since a mark); **the heuristic without integration** (not before 300 ms of silence
    after the last output); **the environment has the tool paths once** (`PATH`, `DOTNET_ROOT`, `TERM_PROGRAM`);
    **Ctrl+C reaches a `sleep`** (the foreground group, exit 130); **bracketed paste reaches an application that asked**
    (raw bytes through `od`); **the bell event**; **find in the scrollback**; the person's input interrupting a wait and
    `kill` ending a busy terminal; Clear keeping the prompt line and the marks; **a line redrawn after a mark**.
- **`crates/commands`** (6 new, 2 updated): every schema parses and names its command; parsing and validation (ids,
  defaults, the prompt default of `wait`, regex checked, bounds); **every output conforms to its schema**; **the
  `terminal` policy object** (prompt asks with "for this session", granted runs at execute, allow, deny, `kill`
  dangerous); **through the registry: the first `send` asks, a granted one is allowed by the policy without asking, a
  deny rule still refuses**; the policy file round-trips the object and Always Allow writes nothing for a session
  grant; the settings' key list and the Terminal page, profiles and enum validation.
- **`crates/mcp`** (1 new, 1 updated): the resource list has three guides; **the terminal guide is under 600 words,
  served as `eludite://guides/terminal` and names only real commands**.
- **`crates/docking`** (1 new, 8 updated): **a version 3 layout gets the Terminal after Output in its bottom group**
  (active tab kept; after the Error List when Output is closed; closed at the bottom with neither); the default layout's
  bottom group; the tests whose bottom group now has the Terminal.
- **`crates/ui`**: View > Terminal shows Ctrl+`; Ctrl+` and Ctrl+Shift+` in the keymap; keystrokes parse.
- **`crates/eludite` headless** (`shell/terminal_tests.rs`, 13 tests with real `sh` and `bash`; 2 unit tests in
  `shell/terminal.rs`; 1 in `agents/transcript.rs`):
  - `ctrl_backtick_opens_the_window_with_a_terminal_in_the_workspace`: nothing at startup, the window beside Output,
    Ctrl+` shows it with a shell whose `pwd` is the workspace, named `sh`, focused, drawn; Ctrl+` again opens no second.
  - `new_terminal_split_kill_and_the_exit_line_with_restart`: New Terminal (`sh (2)`), the dropdown's `bash`, Split
    (two panes side by side), Kill, `exit 3` showing the exit line, Restart in its place with its name, the tab's close.
  - `typing_reaches_the_shell_and_the_output_renders`: keys typed reach the shell and the drawn rows show the output;
    **the keystroke-to-echo budget**; Ctrl+C without a selection interrupts, with one copies and sends nothing.
  - `an_agent_opens_sends_waits_and_reads_with_the_marker_on_the_tab`: an agent opens `bash` (integration on, in the
    workspace; the window shown without taking focus), waits for the first prompt, sends, **the marker shows during
    its wait**, the wait answers `prompt`, exit 0 and the output, `read since` the mark, the marker goes, the person's
    view drew it, `list`, the audit.
  - `the_persons_keystroke_interrupts_an_agents_wait_and_refuses_its_next_send`: Ctrl+C typed by the person ends the
    agent's `wait` with `interrupted_by: "user"` (17 ms after the key); its `send` is refused naming `read`; after a
    `read` it sends.
  - `ctrl_click_on_a_path_printed_by_the_shell_opens_the_editor_at_the_line`: a plain click opens nothing; **Ctrl+click
    on `src/lib.rs:3:1` opens the editor at line 3**.
  - `open_in_terminal_from_the_workspace_uses_the_projects_folder`: the project's context menu, Open in Terminal, the
    terminal's folder is the project's (`pwd`).
  - `the_reserved_chords_reach_the_shell_window_and_the_rest_the_terminal`: Ctrl+S, Ctrl+Q, Ctrl+R and Ctrl+C reach
    the shell (no Save); **Ctrl+Shift+B builds while the terminal has focus**; Ctrl+\, Ctrl+E shows the Error List;
    Escape leaves the terminal.
  - `closing_the_workspace_ends_the_shells_and_a_running_command_asks`: a `sleep` running: Close Workspace asks (No
    keeps it, Yes ends the shell and closes the tabs); nothing running: no question; Kill over a running command asks.
  - `the_policy_refuses_with_deny_and_prompt_asks_once_per_session` (the scripted fake agent through the MCP endpoint
    and the Agents window): `deny` refuses the open naming the policy; `prompt` asks once (dangerous, the reason, the
    button "Allow for this session"), then the two sends run at class execute without asking and the wait for `two`
    completes; the transcript's terminal rows; **the policy file is unchanged**.
  - `cat_of_a_large_file_keeps_frames_short_and_the_scrollback_small`: **the 100 MB budget, the 2x budget, the memory**.
  - `marks_are_bytes_of_plain_text_and_clear_keeps_them`: an agent's `wait` text without escapes, `clear` keeps marks,
    `resize`, a bad id refused naming `list`.
  - `the_options_page_and_the_terminal_settings_apply`: Tools > Options opens the Terminal page; `fontSize`,
    `copyOnSelect`, `bell: none` reach the views (no flash on a bell); `scrollback: 500` applies to a new terminal.
- **The Xvfb run** (`crates/eludite/tools/terminal-linux.sh`, section 13).

## 7. Deviations and gaps

1. **"Allow for this session"** needed a mechanism the policy did not have. `AlwaysAllow::Session(key)` (a terminal
   call's escalation names the grant `terminal.run`) makes the prompt's Always Allow button read "Allow for this
   session"; answering it records the grant in the agent session's policy store (a new store per session, so the grant
   ends with the session) and writes nothing. The terminal's hooks see the grant through
   `PolicySnapshot::session_grants` and then raise the call with `AlwaysAllow::Granted`, which the gate allows without
   asking unless a deny rule matches; `allow` uses the same. The registry now keeps a `Granted` escalation's reason and
   marker at the declared class (ADR-0009 had only raises above it). **This extends ADR-0009's model; the owner may
   want a note on that ADR** (no ADR is in this brief's files).
2. **Memory**: 10,000 lines of scrollback stay under 20 MB only up to 83 columns (24 bytes per cell in
   `alacritty_terminal`): 28.8 MB at 120 columns. Meeting the budget at any width would need a smaller default
   scrollback, a budget per column, or a compact history (not possible without changing `alacritty_terminal`). The
   test asserts the 80-column figure and prints the others.
3. **Restoring closed tabs' names at startup** is not done: the window shows "No terminal is open" (no shell starts,
   which is the part that matters).
4. **Selecting a terminal tab** is the window's state (it sets the active terminal agents default to); there is no
   `select` command in the schemas.
5. **Ctrl+Tab and Alt menus**: the shell binds neither yet (the menu bar has no Alt accelerators), so there is nothing
   to reserve; Alt+letter goes to the terminal (readline's Alt+B and Alt+F work).
6. **Escape** goes to the editor as the brief says, so a full-screen program in the terminal gets Escape only as
   Ctrl+[ (which sends ESC).
7. **No input method**: characters come from key events (`key_char`); IME composition is not supported. Mouse
   reporting to full-screen programs is not forwarded (the wheel sends arrows in the alternate screen).
8. **The UI thread and the grid's lock**: a frame takes its snapshot only if the lock is free, or waits for at most
   one 16 KB parse chunk when the terminal changed; copy, paste-mode checks and `screen` use the same short wait; find
   runs off the UI thread.
9. **Dependencies used directly** that were already in the build (as `crates/browser` does): `polling` 3 (the I/O
   loop, as `alacritty_terminal`'s own event loop uses it), `regex` 1 (patterns and links), `libc` 0.2 on Unix
   (`tcgetpgrp`, `killpg`).
10. **Files outside the brief's list**, each a hook: `crates/commands/src/policy.rs` (the `terminal` object, the two
    `AlwaysAllow` variants, `session_grants`), `registry.rs` (the `Granted` arm), `lib.rs`, `settings.rs` (its test's key
    list); `crates/docking/src/controller.rs` and `tests.rs` (expectations with the Terminal in the bottom group);
    `crates/ui/src/lib.rs` (export); `crates/eludite/src/app.rs` (`bind_keys`), `shell/tests.rs` (the harness's
    `bind_keys`), `shell/settings.rs` (two lines), `shell/explorer.rs` (Open in Terminal), `shell/agents.rs` and
    `agents/window.rs` (the session grant and its button), `agents/transcript.rs` (the terminal rows),
    `crates/eludite/Cargo.toml`, `README.md` (the status line). `shell.rs` gains the module, the `Services` fields, the
    registration, the `tool_body` parameter, the field, the install call, the `run` hook and the Close Workspace hook.
11. **Windows and macOS** were not built: no targets are installed here. The Windows code paths are cfg-gated
    (`escape_args`, the child's pid from the ConPTY watcher, no `busy` detection); the PTY tests are `cfg(unix)`, so
    the Windows job runs the unit tests only.

## 8. The dependency

`alacritty_terminal` **0.26.0** from crates.io, **SPDX: Apache-2.0**, with `default-features = false` (no `serde`).
Crates it adds to `Cargo.lock`:

| Crate | Version | SPDX |
|---|---|---|
| `alacritty_terminal` | 0.26.0 | Apache-2.0 |
| `vte` | 0.15.0 | Apache-2.0 OR MIT |
| `cursor-icon` (through `vte`) | 1.2.0 | MIT OR Apache-2.0 OR Zlib |
| `signal-hook` | 0.4.4 | MIT OR Apache-2.0 |
| `rustix-openpty` | 0.2.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `home` | 0.5.12 | MIT OR Apache-2.0 |
| `miow` (Windows) | 0.6.1 | MIT OR Apache-2.0 |

Its other dependencies were already in the build: `base64` 0.22, `bitflags` 2, `libc`, `log`, `parking_lot`, `piper`,
`polling` 3, `regex-automata` 0.4, `rustix` 1, `signal-hook-registry`, `unicode-width` 0.2, `windows-sys` 0.59. All are
compatible with GPL-3.0-or-later. It builds with Rust 1.98.1 (edition 2024 workspace; the crate is edition 2021,
`rust-version` 1.85).

## 9. Key routing

While a terminal has focus:

| Keys | Go to | Why |
|---|---|---|
| Ctrl+`, Ctrl+Shift+` | the shell window | View > Terminal, New Terminal |
| F5, Ctrl+F5, Shift+F5, Ctrl+Shift+F5, F9, F10, F11, Shift+F11, Ctrl+F10, Ctrl+Shift+F10, Ctrl+Alt+Break, Ctrl+Alt+P, Ctrl+Alt+B | the shell window | the debug keys |
| Ctrl+Shift+B, F6, Shift+F6 | the shell window | build |
| Shift+Esc, Ctrl+Alt+L, Ctrl+Alt+O, Ctrl+Alt+X, Ctrl+\ Ctrl+E, Ctrl+\ Ctrl+C, Ctrl+0 Ctrl+G, Ctrl+0 Ctrl+R | the shell window | hide a window, show the tool windows (a lone Ctrl+\ or Ctrl+0 reaches the shell after the chord's timeout) |
| Escape | the editor (or the shell window without one) | the brief's "Escape to the editor" |
| Ctrl+C with a selection, Ctrl+Shift+C, Ctrl+Insert | copy | Visual Studio's |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | paste (bracketed when asked) | Visual Studio's |
| Ctrl+F | find in the scrollback | the window's own find |
| Shift+PageUp, Shift+PageDown | scroll the scrollback | |
| everything else, including the shell's other keymap entries (Ctrl+S, Ctrl+Z, Ctrl+Y, Ctrl+R chords, Ctrl+Space, F12, F2, F4, Ctrl+., Alt+Enter, Ctrl+Shift+O, Ctrl+E T) | the terminal | `RESERVED_KEYS` in `shell/terminal.rs`; the rest are disabled in the terminal's key context |

## 10. Developer PowerShell on Windows

- **Located, never shipped** (CLAUDE.md invariant 9): `BuildTools::locate` looks at `ELUDITE_VS_INSTALL`, then
  `%ProgramFiles(x86)%` and `%ProgramFiles%\Microsoft Visual Studio\{18,2022,2019}\{BuildTools,Enterprise,Professional,Community}`
  for `Common7\Tools\Launch-VsDevShell.ps1`. No process is started to find it (no `vswhere`).
- **With it**: Developer PowerShell runs `powershell.exe` (`pwsh.exe` when it is on PATH) with
  `-NoLogo -NoExit -Command "& '<install>\Common7\Tools\Launch-VsDevShell.ps1' -SkipAutomaticLocation"`, then the
  integration script; Developer Command Prompt runs `cmd.exe /k <install>\Common7\Tools\VsDevCmd.bat`.
- **Without it**: a plain PowerShell named Developer PowerShell (and PowerShell, Command Prompt).
- What it needs from the located Build Tools: the two scripts above (they set MSBuild's, the compilers' and the SDKs'
  paths), and nothing else; Eludite's own `dotnet`, Cargo and Node.js folders are still put first on PATH.
- Not run here: the profiles' arguments are unit-tested with a fake installation folder.

## 11. Counts

This machine, `DISPLAY=:99`, `CEF_PATH`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1` and `ELUDITE_DBG_MONO` set,
`dotnet build dotnet/Eludite.slnx` and `bash corpus/tests/build.sh` first:

- `cargo test --workspace --no-fail-fast --features eludite-chromium/cef`: COUNTS
- `cargo fmt --check`: clean. `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`:
  clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.

## 12. How to reproduce

```
cargo test -p eludite-terminal                                   # the crate: unit and PTY tests
cargo test -p eludite --bins terminal -- --nocapture             # the shell; prints the timing lines
crates/eludite/tools/terminal-linux.sh OUT_DIR                   # Xvfb, xdotool, jq, ImageMagick; the screenshots
```

## 13. Screenshots (Xvfb run)

`crates/eludite/tools/terminal-linux.sh` on a small Cargo package, with the scripted fake agent "Terminal Runner", in
[0041-run/screenshots](0041-run/screenshots/):

- `terminal-dotnet.png`: Ctrl+`: the Terminal window beside the Error List and Output, `bash` in the workspace;
  `dotnet --version` typed by hand prints **10.0.302**, the SDK `global.json` pins.
- `terminal-permission.png`: the agent's first `eludite.terminal.send` asks (class dangerous, `terminal.run: prompt`),
  with Allow, **Allow for this session** and Deny.
- `terminal-agent.png`: allowed for the session: the agent's `sleep 2 && cargo --version` typed into the same
  terminal, the tab reading **"Agent Terminal Runner is typing"** while its `wait` runs.
- `terminal-done.png`: `cargo 1.97.0` printed, the wait answered at the prompt (2,052 ms), the turn ended, the marker
  gone.
