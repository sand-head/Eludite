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

## Privacy: no request nobody asked for

The engine makes no network request other than the pages the person or an agent navigates to (and what those pages
load). CEF 154 runs Chrome's browser process with its background services; brief 0031's net log of a fresh profile
found five destinations, and brief 0032's found two more. Each is off, in `src/privacy.rs`:

| Request | Turned off by |
|---|---|
| `clients2.google.com/time` (network time) | `--disable-features=NetworkTimeServiceQuerying` |
| `www.google.com/async/folae` (AI Mode eligibility) | `--disable-features=AimEnabled,AimServerEligibilityEnabled,AimServerRequestOnStartupEnabled` |
| `www.google.com/` (a preconnect to the search engine) | `--disable-features=PreconnectToSearch` and the preference `net.network_prediction_options: 2` (no prediction) |
| `dns.google/dns-query` and UDP to the system's resolver for it (the DNS-over-HTTPS upgrade probe) | `--disable-features=DnsOverHttpsUpgrade` and the local-state preference `dns_over_https.mode: "off"` |
| `accounts.google.com/ListAccounts` (Chrome's check of the Google accounts in the cookie jar) | No switch, preference or policy stops it in Chromium 154 (`GaiaCookieManagerService` lists the accounts whenever anything asks, sign-in allowed or not): `--gaia-config-contents={"urls":{"list_accounts_url":{"url":"data:,"}}}` points that one url at an address that is not on the network, so each attempt fails inside the engine. Pages that sign in with Google are unaffected. |
| `update.googleapis.com/service/update2/json` (the component updater: an on-demand check for the on-device model's manifest, which `--disable-component-update` leaves on) | `--component-updater=url-source=data:,` (the updater's server is not on the network) beside `--disable-component-update` |

Also off, as defense in depth: `--disable-background-networking`, `--disable-sync`, `--disable-default-apps`,
`--no-pings`, `--no-service-autorun`, `--disable-breakpad`, `--disable-client-side-phishing-detection`,
`--disable-domain-reliability`, `--disable-field-trial-config`, `--disable-search-engine-choice-screen`,
`--metrics-recording-only`, the features `OptimizationHints`, `MediaRouter`, `DialMediaRouteProvider`, `Translate`,
`CertificateTransparencyComponentUpdater`, `LensOverlay` and `AutofillServerCommunication` (added to CEF's own
`--disable-features` list, never replacing it), the profile preferences `signin.allowed` and
`signin.allowed_on_next_startup` false, Safe Browsing off (`safebrowsing.enabled`), search suggestions, the spelling
service, translate, alternate error pages, the password manager's and autofill's services off, and Chrome policies
written to `<profile>/Policies/managed/eludite.json` and read through CEF's `chrome_policy_id` (`BrowserSignin: 0`,
`SyncDisabled`, `ComponentUpdatesEnabled: false`, `SafeBrowsingProtectionLevel: 0`, `NetworkPredictionOptions: 2`,
`DnsOverHttpsMode: "off"`, `MetricsReportingEnabled: false`, `GenAILocalFoundationalModelSettings: 1` and the rest in
`privacy::policies`). The preferences are written into the profile before CEF starts, keeping its other keys.

The proof is `tests/engine.rs`'s `the_engine_makes_no_request_on_about_blank`: the engine runs with
`--log-net-log=FILE --net-log-capture-mode=Everything` on `about:blank` for 10 s, and the test reads the log for any
request to an `http`, `https`, `ws` or `ftp` url, any host resolution, and any TCP, UDP, SSL or QUIC connection; it
asserts there is none (`ELUDITE_NET_LOG_KEEP=FILE` keeps the log to read by hand).

## Tests

`cargo test -p eludite-chromium` runs the ring, framing, options and sandbox unit tests. With `--features cef` (and
`CEF_PATH`), `tests/engine.rs` drives the real engine: a tab rendering four colored quadrants read back from the ring
at known points, resize into a new region, a CDP `Runtime.evaluate`, mouse and key input changing the page, errors,
close, shutdown, and a closed stdin. As root they set `ELUDITE_CHROME_NO_SANDBOX=1` and say so.
