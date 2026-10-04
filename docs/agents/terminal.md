# The terminal in Eludite: a guide for agents

Eludite's terminal commands (`eludite.terminal.*`) drive the Terminal window the person sees (View > Terminal,
Ctrl+`). A terminal is a real shell on a PTY, in the workspace's folder, with the `dotnet`, Cargo and Node.js Eludite
located first on PATH. The person watches what you type and can type into the same terminal.

## 1. Open or pick a terminal

- `eludite.terminal.list` shows the terminals (id, name, profile, whether a command is `busy`) and the profiles.
- `eludite.terminal.open` starts one (`profile`, `cwd`, `env`, `name`) and answers its id (`term1`). Prefer your own
  terminal to typing into one the person is using.

Every other command takes `terminal`; without it, the active one.

## 2. Run a command and wait for it

1. `eludite.terminal.send` with `text` types it and presses Enter (`newline: false` types only). It answers a `mark`:
   the position in the output before your text.
2. `eludite.terminal.wait` with that `mark` (`prompt: true` is the default) returns when the shell shows its next
   prompt: `matched: "prompt"`, the command's output in `text` and, with shell integration (`integration: true`),
   its `exit_code`. Do not sleep instead.

Without integration (`sh`, or a shell that declined it) a prompt is a line ending in `$ `, `> `, `# ` or `% ` after
300 ms of silence, and `exit_code` is unknown: run `echo $?` to see it.

- `pattern` waits for output matching a regular expression (a server's "listening on" line), `exit: true` for the
  shell to end. `timeout_ms` is 10,000 by default, at most 60,000; `matched: "timeout"` is not a failure.
- A long build: wait in steps, reading what came so far with `eludite.terminal.read`.

## 3. Read

`eludite.terminal.read` with `mode`:

- `screen` (default): the visible grid and the cursor, for full-screen programs and prompts asking a question;
- `since` with a `mark`: everything printed after it, as plain text (escape sequences removed);
- `scrollback` with `max_lines`: the last lines.

Every answer carries the next `mark`. Reads and waits never ask the person.

## 4. When the person types

If the person types into the terminal while you wait, the wait ends at once with `interrupted_by: "user"`, and your
next `eludite.terminal.send` is refused until you call `eludite.terminal.read`. Read the screen, see what they did,
and do not undo it.

## 5. Interrupting, keys and cleaning up

- Ctrl+C is `send` with `text: "\u0003"` and `newline: false`; Ctrl+D is `"\u0004"`.
- `eludite.terminal.resize` changes the size programs see; the window sets it back for a terminal on screen.
- `eludite.terminal.clear` erases the screen and scrollback; marks stay valid.
- `eludite.terminal.close` ends a terminal; with a command running it needs `kill: true`.

## 6. What needs permission

The solution policy's `terminal.run` decides `open`, `send`, `resize`, `clear` and `close` for agents:

- `prompt` (the default): your first such call in a session asks the person, as a dangerous action does. When they
  answer "Allow for this session", your later calls run without asking until your session ends.
- `allow`: they run without asking. `deny`: they are refused, naming the policy; ask the person to do it instead.

`close` with `kill` always asks. `list`, `read` and `wait` are always allowed.

## 7. Prefer the IDE's commands

Use `eludite.build.*` to build, `eludite.test.*` for tests, `eludite.debug.*` to debug and `eludite.git.*` for git:
their results are structured and they show in the IDE's own windows. Use the terminal for what has no command.
