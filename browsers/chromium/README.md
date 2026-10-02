# eludite-chromium

Eludite's embedded browser engine (brief 0031, proposal 0002 section 3, ADR-0008): CEF's browser process. The shell
starts it the first time a browser tab needs it and never loads CEF itself. Each tab renders off screen (CEF's
windowless mode, software `OnPaint`) into a shared-memory ring the shell maps; the engine takes its orders as JSON-RPC
on stdin and answers on stdout. The protocol and the ring are specified in
[`protocol/schemas/browser-rpc/browser-rpc.md`](../../protocol/schemas/browser-rpc/browser-rpc.md).

GPL-3.0-or-later. CEF (BSD-3-Clause) is fetched by [`tools/cef/`](../../tools/cef/), never vendored.

## Building

```
export CEF_PATH="$(tools/cef/fetch.sh)"        # 326 MB download, 558 MB unpacked, once
cargo build -p eludite-chromium --features eludite-chromium/cef
```

Without the `cef` feature (the default, so a workspace build downloads nothing) the executable only prints how to
build the real engine. Linux only so far; Windows and macOS are documented in `tools/cef/README.md` and the report of
brief 0031. The build copies CEF's runtime files (`libcef.so`, the paks, `locales/`, `chrome-sandbox`) beside the
executable in `target/<profile>/`; the executable finds `libcef.so` there through its `$ORIGIN` run path.

## Running it alone

```
eludite-chromium --profile DIR [--cef-dir DIR] [--frame-socket FD]
```

- `--profile`: the root cache path (cookies, storage, cache); the shell passes `<workspace>/.eludite/browser/profile`.
  One engine per profile at a time.
- `--cef-dir`: CEF's resources and locales (default: the executable's folder).
- `--frame-socket`: the Unix socket (`SOCK_SEQPACKET`) the region descriptors are sent over, 3 by default. Run alone
  without it, the engine still works; frames just reach no one.
- The sandbox: on Linux the engine starts only when `chrome-sandbox` in the CEF folder is owned by root with the
  setuid bit, and otherwise says how to install it. `ELUDITE_CHROME_NO_SANDBOX=1` runs it with `--no-sandbox` (needed
  as root, for example in a container).
- `ELUDITE_CHROMIUM_GPU=1` keeps Chromium's GPU process for compositing (default: `--disable-gpu
  --disable-gpu-compositing`, the software path). Without `DISPLAY` and `WAYLAND_DISPLAY`, the engine uses Chromium's
  headless Ozone platform (no display needed; no system clipboard).
- Logs, CEF's included, go to stderr. stdout carries the protocol only: the engine points descriptor 1 at stderr at
  startup and keeps a private copy for the protocol.

A session by hand (each message framed with `Content-Length`, as LSP):

```
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"me","clientVersion":"0","protocolVersion":1}}
{"jsonrpc":"2.0","id":2,"method":"tab/create","params":{"url":"https://example.com","width":1280,"height":800}}
{"jsonrpc":"2.0","method":"tab/cdp","params":{"tab":"1","message":{"id":1,"method":"Runtime.evaluate","params":{"expression":"document.title"}}}}
{"jsonrpc":"2.0","id":3,"method":"shutdown","params":{}}
```

## Tests

`cargo test -p eludite-chromium` runs the ring, framing, options and sandbox unit tests. With `--features cef` (and
`CEF_PATH`), `tests/engine.rs` drives the real engine: a tab rendering four colored quadrants read back from the ring
at known points, resize into a new region, a CDP `Runtime.evaluate`, mouse and key input changing the page, errors,
close, shutdown, and a closed stdin. As root they set `ELUDITE_CHROME_NO_SANDBOX=1` and say so.
