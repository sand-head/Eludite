# eludite-chromium control protocol and frame ring

Contract between the Eludite shell (client) and `eludite-chromium` (`browsers/chromium`, the CEF browser process;
brief 0031, proposal 0002 section 3, ADR-0008). Every method either side sends is listed here, each with a JSON schema
in this folder (`$defs.params`, and `$defs.result` for requests). Version 1.

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
3. `shutdown`: the engine closes its tabs, answers, and exits with code 0.

CEF's renderer, GPU and utility subprocesses are the same executable started by CEF with `--type=...`
(`browser_subprocess_path`); they never see the control channel.

## Methods

| Method | Kind | Direction | Schema | Params | Result |
|---|---|---|---|---|---|
| `initialize` | request | shell to engine | [initialize.json](initialize.json) | `{ clientName, clientVersion, protocolVersion: 1 }` | `{ engineName, engineVersion, protocolVersion, cefVersion, chromiumVersion, pid, sandbox, frameTransport }` |
| `shutdown` | request | shell to engine | [shutdown.json](shutdown.json) | `{}` | `{}` |
| `tab/create` | request | shell to engine | [tab-create.json](tab-create.json) | `{ url, width, height, scale?, frameRate? }` | `{ tab }` |
| `tab/close` | request | shell to engine | [tab-close.json](tab-close.json) | `{ tab }` | `{}` |
| `tab/resize` | request | shell to engine | [tab-resize.json](tab-resize.json) | `{ tab, width, height, scale? }` | `{}` |
| `tab/navigate` | request | shell to engine | [tab-navigate.json](tab-navigate.json) | `{ tab, url }` | `{}` |
| `tab/input` | notification | shell to engine | [tab-input.json](tab-input.json) | `{ tab, event }` | |
| `tab/cdp` | notification | shell to engine | [tab-cdp.json](tab-cdp.json) | `{ tab, message: { id, method, params? } }` | |
| `tab/resized` | notification | engine to shell | [tab-resized.json](tab-resized.json) | `{ tab, region: { id, size, headerSize, slotSize, slotCount, name? }, width, height }` | |
| `tab/frame` | notification | engine to shell | [tab-frame.json](tab-frame.json) | `{ tab, region, slot, sequence, width, height, dirty: [{ x, y, width, height }], paintNs, copyNs }` | |
| `tab/state` | notification | engine to shell | [tab-state.json](tab-state.json) | `{ tab, url?, title?, loading? }` | |
| `tab/cdpEvent` | notification | engine to shell | [tab-cdp-event.json](tab-cdp-event.json) | `{ tab, message }` | |
| `tab/closed` | notification | engine to shell | [tab-closed.json](tab-closed.json) | `{ tab, reason: "closed" \| "crashed" }` | |

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
  `ImeCommitText`, `ImeFinishComposingText` and `ImeCancelComposition`. The spike's shell forwards mouse and
  keyboard; IME is specified here and forwarded by brief B.

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
