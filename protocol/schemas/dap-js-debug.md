# vscode-js-debug: debugging the pages of the Web Browser window

Eludite debugs a web page's JavaScript and TypeScript with vscode-js-debug (Microsoft, MIT, SPDX: MIT), the adapter
behind VS Code's and Visual Studio's JavaScript debugging, run as its standalone DAP server `dapDebugServer.js` on the
Node.js found on the machine (PLAN.md 4.5 and 4.9, proposal 0002 brief D, brief 0038). Neither is bundled nor
vendored. This file records what Eludite sends, what it expects back, and what is known not to work. The version
pinned is in `tools/js-debug/PIN`.

What ran where (brief 0038): the shell's behavior is proven against a fake js-debug (`crates/dap/src/fake.rs`,
`fake::listen_js_debug`) that carries this contract, and against the real release 1.140.0 under Node.js 22 in brief
0038's container: `crates/dap/tests/js_debug.rs` (the external Chrome), the shell's real tests (the embedded engine
under Xvfb, the corpus web project and the Vite counter) and the two recorded scenarios (`corpus/dap/js-debug/`,
replayed on every platform). The release is published on GitHub only; the tests that need it skip without
`ELUDITE_JS_DEBUG`. Anything below that the real adapter contradicts is a bug in this file.

## Discovery

The DAP server script, `JsDebugSearch` (`crates/dap/src/discovery.rs`), first found of:

1. the setting `debugger.jsDebugPath` (`ELUDITE_JS_DEBUG`): `dapDebugServer.js`, or a folder holding
   `src/dapDebugServer.js`, `js-debug/src/dapDebugServer.js` or `dapDebugServer.js`;
2. the cache folder of `tools/js-debug/fetch.sh`: `~/.cache/eludite/js-debug/<version>/js-debug/src/dapDebugServer.js`
   (`%USERPROFILE%\.cache\eludite\js-debug\...` on Windows), the version from `tools/js-debug/PIN`, else the newest
   version folder there;
3. `js-debug/src/dapDebugServer.js` beside the eludite executable.

The version is the cache folder's name (the release ships no `package.json`), else the one pinned.

Node.js, `NodeSearch`: the setting `debugger.nodePath` (`ELUDITE_NODE`), `node` on `PATH`, Volta
(`~/.volta/bin/node`), nvm's default (`~/.nvm/alias/default` names the version under `~/.nvm/versions/node/`), fnm's
default (`~/.local/share/fnm/aliases/default/bin/node`, then `~/.fnm/aliases/default/bin/node`). Its version is read
once, with `node --version`, on the attach thread, and must be at least the release's minimum (`node 18` in `PIN`).
Nothing of either is looked for or loaded before a browser session starts.

A missing script fails the attach with the folders searched and `tools/js-debug/fetch.sh`; a missing or old Node with
where it looked and the minimum. Either way a compound's server session keeps running and its answer's `message` says
so.

## Transport

TCP on loopback. Eludite starts `node <dapDebugServer.js> 0 127.0.0.1` (port 0: the system picks one), reads the
server's first stdout line, `Debug server listening at 127.0.0.1:PORT` (the port is the number after the last colon of
the first line naming `listening`), within 10 s, and connects to `127.0.0.1:PORT` (`transport::start_tcp_server`,
`TcpServer`). The first connection owns the Node process: closing it (Stop, the session ending) kills the server and
with it every child connection. The server's later stdout and stderr lines go to the session's `adapter` output.

`session.adapter` reads `vscode-js-debug 1.140.0 under node v22.22.0 (tcp 127.0.0.1:PORT)`, `adapter_version`
`vscode-js-debug 1.140.0, node v22.22.0`, `session.runtime` `javascript`, `capabilities.adapter` `javascript`.

## The browser session: initialize and attach

`initialize` with `adapterID` `pwa-chrome`, `supportsStartDebuggingRequest: true` (and Eludite's usual
`linesStartAt1`, `columnsStartAt1`, `pathFormat: path`, `supportsVariableType`, `supportsVariablePaging`,
`supportsRunInTerminalRequest: false`). Then `attach` (`attach::browser_attach`):

| Argument | Value |
|---|---|
| `type`, `request`, `name` | `pwa-chrome`, `attach`, the tab's title |
| `address`, `port` | The browser's remote debugging port: the embedded engine's `engine/ready` (`127.0.0.1`, a free port it chose; browser-rpc.md), the external Chrome's `--remote-debugging-port`, or the host and port of a `ws://` url |
| `targetId` | The tab's Chrome DevTools target id (`eludite.browser.tabs`' `target_id`; the page id of a `ws://.../devtools/page/<id>` url) |
| `urlFilter` | The tab's url (js-debug attaches to the targets whose url matches; with `targetId` it is the fallback of a js-debug that reads only the filter) |
| `targetSelection` | `automatic` (never js-debug's quick pick) |
| `webRoot` | The folder the page's urls map to: the start's or attach's `web_root`, else the `wwwroot` of the project whose launch opened the tab, else that project's folder, else the open solution's or folder's |
| `sourceMaps`, `pauseForSourceMap` | `true`, `true` (a breakpoint in a mapped file binds before the script's first statement runs) |
| `resolveSourceMapLocations` | `null` (maps anywhere, a dev server's included) |
| `skipFiles` | `[]` |

The browser session is `attached`, named after the tab's title (renamed on the Web Browser window's `tab/state`
title), `session.tab` and `session.url` set. Stop sends `disconnect` with `terminateDebuggee: false`: the page and the
browser keep running. The tab closing ends the session (and its children) with mode `design` and the message
`The tab tN closed.`.

## Child sessions: `startDebugging`

js-debug in server mode debugs each target (the page, its iframes, workers) in a session of its own, which it asks the
client to start with the reverse request `startDebugging` on the parent's connection:

```json
{"command": "startDebugging", "arguments": {"request": "attach",
  "configuration": {"type": "pwa-chrome", "name": "<target title>", "request": "attach", "__pendingTargetId": "<id>"}}}
```

Eludite answers it at once (`success: true`), opens a second connection to the same server, and runs the handshake
there with `adapterID` `pwa-chrome`, `request` from the arguments and the configuration as the `attach` (or `launch`)
arguments, unchanged. What the real adapter does then (recorded in `corpus/dap/js-debug/`): on the parent, `attach`
is answered after `configurationDone`, right after the reverse request; on the child, `initialized` comes at once, the
breakpoints set before the page's script is bound are answered `verified: false` with the message
`breakpoint.provisionalBreakpoint` (pending, not refused), and once the child's `configurationDone` is answered the
child's `attach` is answered, a second `initialized` comes, `thread` `started` with `threadId` 0 (named after the
tab's title), and `breakpoint` `changed` events bind them through the source map. The parent sends `output` events of
category `telemetry` (Eludite shows none). The child is a session with `parent` (the browser session's id), `attached`, named after
`configuration.name`, listed under its parent by `eludite.debug.sessions` and the Call Stack's selector. A child may
start children of its own the same way.

- **Breakpoints and exception filters go to the children**, which own the scripts; the browser session (a parent)
  gets none, so `breakpoints[].sessions` lists the children only.
- Commands addressed to the parent act on the parent only. `wait` on a parent ends at the first stop of the parent or
  any of its children and answers that session's summary (brief 0028's compound rule).
- Stop on the parent detaches its children first (`disconnect`, `terminateDebuggee: false`; each answers with
  `thread` `exited` and `terminated`), then, once the last has ended, the parent itself (js-debug sends the parent
  `terminated` when its last child goes, and answers the parent's `disconnect` only then).

## Breakpoints by file kind

Every breakpoint goes to every session whose adapter claims its file's kind (`session::AdapterFamily`); a kind no
adapter claims goes to every session, as before brief 0038.

| Adapter | Claims |
|---|---|
| netcoredbg, eludite-dbg-mono, eludite-dbg-netfx (`coreclr`, `mono`, `netfx`) | `.cs`, `.vb`, `.fs`, `.fsx`, `.cshtml`, `.razor` |
| vscode-js-debug (`javascript`, child sessions) | `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`, `.tsx`, `.jsx`, `.vue`, `.svelte` |
| lldb-dap (`native`) | `.rs`, `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`, `.m`, `.mm`, `.swift` |

A breakpoint in `app.ts` is set with its own path: js-debug binds it through the page's source maps (`setBreakpoints`
on the original path, `verified` once the generated script loaded). js-debug's `verified: false` with a message counts
as a refusal in `breakpoints_failed` (brief 0036) unless the message says it is pending; js-debug's own notes
(`breakpoint.provisionalBreakpoint`, `Unbound breakpoint`) are pending until the page loads the script
(`eludite_commands::debug::pending_message`).

## Exceptions

The Exception Settings window's two boxes map to js-debug's filters (`state::exception_plan_for`):

| Setting | Filter |
|---|---|
| Break When Thrown (`break_when_thrown`) | `all` (caught exceptions too) |
| User-Unhandled (`break_when_user_unhandled`) | `uncaught` |

Exception types (`types`) are .NET's and are not sent to js-debug (its filter options take a JavaScript condition,
which Eludite does not offer yet). The window shows a JavaScript Exceptions row with the same two boxes while a
browser session is live. `exceptionInfo` answers `exception_info` as for the other adapters.

## Source maps and frames

js-debug answers `stackTrace` with the original location when a source map applies (`source.path` is the `.ts` file
under `webRoot`; its scopes are `Block: f` and `Local: f`, then `Script` and `Global`, the last `expensive`: the Locals
window shows `Local: f`, else the first one not expensive). A stop has `allThreadsStopped: false` and, on a
breakpoint, `hitBreakpointIds`. A script with no file under `webRoot` (an inline handler) is named after its url
(`127.0.0.1꞉PORT/(index)꞉10:64`, with modifier letter colons) and mapped to the page's file when one matches
(`index.html`). Eludite keeps that path as the frame's `path`: the execution point, the Call Stack and the
Breakpoints window all work on the original file. It then looks for the map beside the original file (`<stem>.js.map`,
`<stem>.map`, or a `*.map` in the same folder listing it in `sources`; read on the client's reader thread, cached by
modification time) and, when one maps the frame's line, adds `source: { original, generated, generated_line,
generated_column }` to the frame (`eludite.debug.stack`, the stop summary, the state's frames). A frame js-debug could
not map (`app.js` itself) has no `source`. A map served only over HTTP (a Vite dev server's) leaves `generated` out:
the frame still carries the original path js-debug resolved.

## DAP features used

`initialize`, `attach`, `configurationDone`, `setBreakpoints` (with `condition`, `hitCondition`, `logMessage`: js-debug
has log points, so tracepoints are the adapter's; a log point prints an `output` event of category `stdout` with the
`source` and `line` of the tracepoint), `setExceptionBreakpoints`, `threads`, `stackTrace` (with
`startFrame` and `levels`), `scopes`, `variables` (paged), `evaluate`, `setVariable`, `exceptionInfo`, `continue`,
`next`, `stepIn`, `stepOut`, `pause`, `disconnect`; events `initialized`, `stopped`, `continued`, `thread`, `output`,
`breakpoint`, `loadedSource` (ignored), `terminated`; the reverse request `startDebugging`. `run_until`, `trace`,
`snapshot`, `wait` and `interrupted_by` work as for every adapter.

## Known not to work, or not verified

- **A page stopped in the debugger answers no input**: `eludite.browser.input` stops waiting for its events' answers
  once the shell's debugger marks the tab stopped and answers `paused: true` (brief 0038).
- **`targetId`** is honored only by js-debug versions that read it; with one that does not, `urlFilter` picks every
  tab showing the same url.
- Exception types, function breakpoints (js-debug has none), Set Next Statement (no `gotoTargets`), restart of a
  browser session (refused, as for any attach).
- Node.js programs (`pwa-node`), Edge, Firefox and WebDriver BiDi: later briefs.
- If the Node dependency proves unacceptable, proposal 0002 section 9's Rust DAP adapter over CDP would replace
  `dapDebugServer.js` behind the same `AdapterTransport` and session model: it would have to provide the source-map
  resolution of breakpoints and frames, the per-target child sessions (or one session with a thread per target), the
  `all`/`uncaught` pause-on-exceptions, `Runtime.evaluate`-based `evaluate` and `setVariable`, and log points.
