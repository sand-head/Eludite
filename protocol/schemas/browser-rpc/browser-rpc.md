# eludite-chromium control protocol and frame ring

Contract between the Eludite shell (client) and `eludite-chromium` (`browsers/chromium`, the CEF browser process;
brief 0031, proposal 0002 section 3, ADR-0008; the Web Browser window's methods, brief 0032). Every method either side
sends is listed here, each with a JSON schema in this folder (`$defs.params`, and `$defs.result` for requests).
Version 1: brief 0032's methods and members are additions a version 1 peer ignores.

Rust: the shell side is `crates/browser/src/embedded.rs` (`EmbeddedChromium`), the engine side
`browsers/chromium/src/`. Both follow this file; the engine's tests read frames with the layout below.

## Transport

- JSON-RPC 2.0 over the engine's stdin and stdout, framed with `Content-Length` headers as LSP base protocol messages
  (the same framing as `host-rpc.md`). Bodies are UTF-8 JSON; field names are camelCase.
- The engine's stdout carries protocol messages only. At startup the engine duplicates its stdout for the protocol
  and points file descriptor 1 at stderr, so nothing CEF, Chromium or a subprocess prints can reach the channel. Logs
  (CEF's included, `--enable-logging=stderr`) go to stderr; the shell drains it and keeps the last lines for errors.
- **Frames do not travel on stdio.** They go through a shared-memory region per tab (below); `tab/frame` only says
  that a slot is ready.
- A closed stdin is `shutdown`. A closed stdout (the engine crashed or exited) fails every pending request; the shell
  loses the tabs, not itself, and the next command that needs the engine starts a new one.

## Lifecycle

1. The shell starts `eludite-chromium --profile <dir> [--cef-dir <dir>] --frame-socket 3` with stdin and stdout piped
   and, on Linux and macOS, one end of a `socketpair(AF_UNIX, SOCK_SEQPACKET)` as file descriptor 3. The engine runs
   `CefExecuteProcess` and `CefInitialize` before it reads stdin.
2. `initialize`, then any number of `tab/*` requests and notifications.
   After its `initialize` answer the engine sends `engine/ready` (brief 0038) once its Chrome DevTools remote
   debugging port answers: a free port on 127.0.0.1 the engine chose before `CefInitialize` (CEF binds it to the
   loopback address only), reported to the shell, which never guesses it. vscode-js-debug attaches to the engine's
   tabs through it (`protocol/schemas/dap-js-debug.md`); a tab's target id there is what `Target.getTargetInfo`
   answers on the tab's own `tab/cdp` channel.
3. `shutdown`: the engine closes its tabs, answers, and exits with code 0.

CEF's renderer, GPU and utility subprocesses are the same executable started by CEF with `--type=...`
(`browser_subprocess_path`); they never see the control channel.

## Methods

| Method | Kind | Direction | Schema | Params | Result |
|---|---|---|---|---|---|
| `initialize` | request | shell to engine | [initialize.json](initialize.json) | `{ clientName, clientVersion, protocolVersion: 1, downloadDir?, maxDownloadBytes? }` | `{ engineName, engineVersion, protocolVersion, cefVersion, chromiumVersion, pid, sandbox, frameTransport }` |
| `shutdown` | request | shell to engine | [shutdown.json](shutdown.json) | `{}` | `{}` |
| `tab/create` | request | shell to engine | [tab-create.json](tab-create.json) | `{ url, width, height, scale?, frameRate? }` | `{ tab }` |
| `tab/close` | request | shell to engine | [tab-close.json](tab-close.json) | `{ tab }` | `{}` |
| `tab/resize` | request | shell to engine | [tab-resize.json](tab-resize.json) | `{ tab, width, height, scale? }` | `{}` |
| `tab/navigate` | request | shell to engine | [tab-navigate.json](tab-navigate.json) | `{ tab, url }` | `{}` |
| `tab/input` | notification | shell to engine | [tab-input.json](tab-input.json) | `{ tab, event }` | |
| `tab/cdp` | notification | shell to engine | [tab-cdp.json](tab-cdp.json) | `{ tab, message: { id, method, params? } }` | |
| `tab/resized` | notification | engine to shell | [tab-resized.json](tab-resized.json) | `{ tab, region: { id, size, headerSize, slotSize, slotCount, name? }, width, height }` | |
| `tab/frame` | notification | engine to shell | [tab-frame.json](tab-frame.json) | `{ tab, region, slot, sequence, width, height, dirty: [{ x, y, width, height }], paintNs, copyNs, popup? }` | |
| `tab/state` | notification | engine to shell | [tab-state.json](tab-state.json) | `{ tab, url?, title?, loading?, favicon?, canGoBack?, canGoForward?, statusText? }` | |
| `tab/cdpEvent` | notification | engine to shell | [tab-cdp-event.json](tab-cdp-event.json) | `{ tab, message }` | |
| `tab/closed` | notification | engine to shell | [tab-closed.json](tab-closed.json) | `{ tab, reason: "closed" \| "crashed" }` | |
| `tab/devtools` | request | shell to engine | [tab-devtools.json](tab-devtools.json) | `{ tab, width, height, scale?, inspectAt? }` | `{ tab, devtools, created }` |
| `tab/action` | notification | shell to engine | [tab-action.json](tab-action.json) | `{ tab, action, url? }` | |
| `tab/dialogAnswer` | notification | shell to engine | [tab-dialog-answer.json](tab-dialog-answer.json) | `{ tab, id, accept, text?, files?, username?, password? }` | |
| `tab/permissionAnswer` | notification | shell to engine | [tab-permission-answer.json](tab-permission-answer.json) | `{ tab, id, allow }` | |
| `tab/popup` | notification | engine to shell | [tab-popup.json](tab-popup.json) | `{ tab, opener, url, disposition?, userGesture? }` | |
| `tab/cursor` | notification | engine to shell | [tab-cursor.json](tab-cursor.json) | `{ tab, cursor }` | |
| `tab/dialog` | notification | engine to shell | [tab-dialog.json](tab-dialog.json) | `{ tab, id, kind, message?, defaultText?, url?, ... }` | |
| `tab/permission` | notification | engine to shell | [tab-permission.json](tab-permission.json) | `{ tab, id, origin, permissions }` | |
| `tab/dialogClosed` | notification | engine to shell | [tab-dialog-closed.json](tab-dialog-closed.json) | `{ tab, id, accepted? }` | |
| `tab/download` | notification | engine to shell | [tab-download.json](tab-download.json) | `{ tab, id, url, file, path?, state, receivedBytes?, totalBytes?, mime?, message? }` | |
| `tab/contextMenu` | notification | engine to shell | [tab-context-menu.json](tab-context-menu.json) | `{ tab, x, y, pageUrl, linkUrl?, imageUrl?, selectionText?, editable, edit }` | |
| `engine/ready` | notification | engine to shell | [engine-ready.json](engine-ready.json) | `{ remoteDebuggingPort, address: "127.0.0.1" }` | |

Errors: JSON-RPC's codes (-32700, -32600, -32601 for every method not listed, -32602, -32603), and -32001
(`NoSuchTab`) for a `tab` the engine does not know.

Notes:

- **Sizes.** `width` and `height` of `tab/create` and `tab/resize` are the view's CSS pixels; frames are
  `ceil(width * scale)` by `ceil(height * scale)` device pixels. Input coordinates are CSS pixels of the view.
- **`tab/create`.** The engine creates the region, sends its descriptor and `tab/resized`, then answers. CEF paints
  at most `frameRate` frames a second (60 by default; `windowless_frame_rate`), only when something changed, with
  `external_begin_frame` off.
- **CDP.** The tab's DevTools agent is the page target, so messages carry no `sessionId`. Ids are the shell's; the
  engine does not look inside `message` beyond forwarding it. CEF delivers every agent message, responses and
  events, through `CefDevToolsMessageObserver::OnDevToolsMessage`; the engine sends each as one `tab/cdpEvent`.
- **Input** is fire-and-forget. Mouse buttons, wheel and keys map to `SendMouseClickEvent`, `SendMouseMoveEvent`,
  `SendMouseWheelEvent` and `SendKeyEvent` (`KEYEVENT_RAWKEYDOWN`, `KEYEVENT_KEYUP`, `KEYEVENT_CHAR`, with Windows
  virtual-key codes on every platform, as CEF requires); IME composition maps to `ImeSetComposition`,
  `ImeCommitText`, `ImeFinishComposingText` and `ImeCancelComposition`. The Web Browser window (brief 0032) forwards
  mouse, wheel, keys and IME composition from GPUI's input handler.

## The Web Browser window's methods (brief 0032)

- **Tabs the page opens.** `window.open`, `target=_blank` and the like become windowless tabs of the engine's own
  (`OnBeforePopup` fills in windowless `WindowInfo` and the tab's client), announced by `tab/resized` and then
  `tab/popup` naming the opener. They are tabs like any other: `tab/cdp`, `tab/input`, `tab/close` work on them, and
  `window.opener` is the page that opened them.
- **`<select>` lists** (and other popup widgets) are painted by CEF separately (`PET_POPUP`). The engine keeps the
  popup's pixels and draws them over the view's next frame (it invalidates the view when the popup repaints), flags
  the frame with `popup` and adds its rectangle to the dirty ones; when the popup closes the view repaints without
  it. The shell needs nothing special: clicks and keys reach the popup through the view's `tab/input`.
- **Title, favicon, history.** `tab/state` carries the address, title, loading state, `canGoBack` and
  `canGoForward`, the status text and the favicon (downloaded by the engine with `DownloadImage`, at most 32 pixels,
  as a PNG data url), each when it changes.
- **Cursor.** `tab/cursor` names the CSS cursor for CEF's cursor type; the shell shows it over the tab.
- **Dialogs and permissions are the shell's.** JavaScript `alert`, `confirm`, `prompt`, `beforeunload`, file choosers
  and authentication challenges are `tab/dialog`; permission requests (geolocation, notifications, camera, microphone,
  clipboard read, ...) are `tab/permission`. The engine holds CEF's callback and the page waits until
  `tab/dialogAnswer` or `tab/permissionAnswer` names the id; `tab/dialogClosed` says the prompt is gone (answered,
  or closed by a navigation or the tab's close). No Chromium dialog or prompt is ever shown. Dialog and permission
  ids share one counter in the engine process.
- **Downloads** never prompt: they go to `downloadDir` (from `initialize`; the workspace's
  `.eludite/browser/downloads/`) under the suggested name made unique, and `tab/download` reports `started`,
  `progress` (at most every 250 ms), `complete`, `canceled`, `interrupted` or `refused`. A download over
  `maxDownloadBytes` (100 MB by default) is refused when its size is known before it starts, and canceled when it
  passes the limit otherwise.
- **The context menu is the shell's.** The engine cancels Chromium's menu (`RunContextMenu` answers handled and
  cancels its callback) and sends `tab/contextMenu` with what is under the pointer; the shell's items run as
  commands (`eludite.browser.navigate` for Back, Forward and Reload, `eludite.browser.devtools` for Inspect,
  `eludite.browser.open_external`) or as `tab/action` (Copy, Paste, Select All, Save Image As...).
- **DevTools** is a windowless tab of its own (`ShowDevTools` with windowless `WindowInfo`), created by
  `tab/devtools`, rendered into its own ring and driven by `tab/input` like a page. It closes with its page, and
  `tab/close` on it closes DevTools only. It is not a page target for `tab/cdp`.

## Privacy: no request the person did not ask for

The engine runs with a fresh per-workspace profile and makes no network request other than the pages it is told to
load (and what those pages load). Chromium's background services are off by switches, feature flags and
preferences, each listed with what it silences in `browsers/chromium/README.md`; the engine's tests run it with
`--log-net-log` on `about:blank` for 10 s and assert that the net log has no request at all.

## The frame ring

One region per tab: a 4096-byte header followed by two slots. All integers little-endian; `state` fields are 32-bit
atomics both processes access with acquire and release ordering.

### Header

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | magic `0x5242_4C45` (the bytes `E L B R`) |
| 4 | u32 | version, 1 |
| 8 | u32 | slot count, 2 |
| 12 | u32 | header size, 4096 |
| 16 | u64 | slot size in bytes (a multiple of 4096) |
| 24 | u32 | width of the view in device pixels when the region was made |
| 28 | u32 | height |
| 32 | u64 | region id |
| 40 | 24 bytes | reserved, zero |
| 64 + 320 * i | 320 bytes | slot i's header, below |

Slot header (offsets from its start):

| Offset | Type | Field |
|---|---|---|
| 0 | u32 (atomic) | state: 0 free, 1 writing, 2 ready, 3 reading |
| 4 | u32 | frame width, device pixels |
| 8 | u32 | frame height |
| 12 | u32 | stride, bytes per row (width * 4) |
| 16 | u64 | sequence (0: never written) |
| 24 | u64 | paint timestamp, `CLOCK_MONOTONIC` nanoseconds when `OnPaint` began |
| 32 | u64 | copy time in nanoseconds |
| 40 | u32 | dirty rectangle count, at most 16 |
| 44 | u32 | reserved |
| 48 | 16 * 16 bytes | dirty rectangles: `i32 x, i32 y, i32 width, i32 height` |

Slot i's pixels start at `4096 + i * slotSize`: `height` rows of `stride` bytes, BGRA, premultiplied alpha, top row
first (CEF's `OnPaint` buffer).

### Rules

- **The engine writes** a frame into a slot it moves from free to writing with a compare-and-swap, preferring the free
  slot with the lower sequence. When neither slot is free it takes the ready slot with the lower sequence (ready to
  writing): the shell had not read that frame and never will. When no swap succeeds, the frame is dropped and counted.
  It copies the whole frame (CEF's buffer is the whole view), writes the slot header, stores ready (release) and sends
  `tab/frame`.
- **The shell consumes** the newest ready slot: ready to reading (acquire), copy or upload, then free (release). It
  never writes pixels and never touches a slot it did not move to reading.
- **The shell peeks** (a screenshot from the ring) at the slot with the highest sequence, free or ready: it moves it to
  reading, checks the sequence did not change, copies, and restores the state it found. A free slot keeps its pixels
  until the engine reuses it.
- **Resize.** When a frame does not fit the slot size, the engine makes a new region sized for the new frame (slots
  never shrink: a smaller frame reuses the region), sends its descriptor and `tab/resized`, and writes later frames
  there. The shell maps the new region on `tab/resized` and unmaps the old one after its current read. Sequences
  continue across regions.
- A crashed engine leaves states as they were; the shell drops the mapping with the tab.

### Passing the region

- **Linux (implemented):** `memfd_create("eludite-frames", MFD_CLOEXEC)`, `ftruncate` to the region's size, mapped
  read-write by the engine. The descriptor goes to the shell with `sendmsg` and `SCM_RIGHTS` over file descriptor 3,
  a `SOCK_SEQPACKET` socket the shell created with `socketpair` and handed to the child, with an 8-byte payload: the
  region id (u64, little-endian). It is sent before `tab/resized` names the region, so the shell's reader receives it
  with one `recvmsg` when that message arrives. The shell maps the header page read-write (it changes `state`) and the
  slots read-only. One `sendmsg` per region (per tab and per growth), never per frame.
- **macOS (documented, not built):** `shm_open` with a random name, `shm_unlink` at once, `ftruncate` and `mmap`; the
  descriptor travels over the same `SCM_RIGHTS` socket, which macOS supports. `region.name` stays unset.
- **Windows (documented, not built):** `CreateFileMappingW(INVALID_HANDLE_VALUE, ..., PAGE_READWRITE, ...)` named
  `Local\eludite-chromium-<pid>-<region id>`, passed as `region.name`; the shell opens it with `OpenFileMappingW` and
  `MapViewOfFile` (header read-write, slots read-only). No socket; the engine closes its handle when the tab closes.

### Timestamps

`paintNs` and the slot's paint timestamp use `CLOCK_MONOTONIC` (Linux and macOS, `clock_gettime`), which both
processes share, so the shell measures paint-to-present as its own `CLOCK_MONOTONIC` at present minus `paintNs`. On
Windows both sides would use `QueryPerformanceCounter` converted to nanoseconds.
