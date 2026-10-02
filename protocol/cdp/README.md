# Chrome DevTools Protocol

The pinned Chrome DevTools Protocol (CDP) and the generator of Eludite's Rust types for it (brief 0023, proposal
0002 section 6, ADR-0008). `crates/browser` drives Chrome and Chromium with these types; nothing here is hand-written
except the generator.

| File | What |
|---|---|
| `PIN` | The npm package `devtools-protocol` version, tarball URL and SHA-256 (0.0.1709723, pinned 2026-10-02) |
| `fetch.sh`, `fetch.ps1` | Download the tarball, verify it, copy the two JSON files and the license here (`--check` / `-Check`: compare instead) |
| `browser_protocol.json`, `js_protocol.json` | Byte-identical to the package's `json/` files |
| `LICENSE.chromium` | The package's license, BSD-3-Clause (The Chromium Authors) |
| `generator/` | `eludite-cdp-generator` (MIT): reads the JSON, writes `protocol/rust/src/cdp/` |

## What is generated

`protocol/rust/src/cdp/` in the `eludite-protocol` crate, behind its `cdp` feature: one file per domain and `mod.rs`,
each starting with a "generated, do not edit" header. The domains are the ones the browser commands use and every
domain they reference through a `$ref`, transitively:

- Roots: `Target`, `Page`, `DOM`, `Accessibility`, `Runtime`, `Log`, `Network`, `Input`, `Emulation`, `Browser`,
  `Security`.
- Referenced: `Debugger` (`Page.searchInResource` and `Network.searchInResponseBody` answer `Debugger.SearchMatch`)
  and `IO` (`IO.StreamHandle` from `Page.printToPDF` and `Network.takeResponseBodyAsStream`).

For each domain:

- a `serde` struct per `object` type; an alias per string, integer, number, array and free-form object type;
- a string enum per `enum`, inline enums of members named `<Owner><Member>` (`CaptureScreenshotFormat`); each has
  `as_str`, `From<&str>`, `Display`, and an `Other(String)` variant that holds values this version does not list,
  because Chrome adds values (a listed value named `Other` becomes the variant `OtherValue`);
- `<Command>Params` and `<Command>Returns` per command, `Params` with `const METHOD` and an impl of `cdp::Command`
  (whose `Returns` is the answer type);
- `<Event>Event` per event, with `const NAME` and an impl of `cdp::Event`.

Optional members are `Option<T>` with `skip_serializing_if`; `any` is `serde_json::Value`; binary data is a base64
`String`; members whose type contains their owner by value are boxed (`DOM.Node`'s `contentDocument`,
`Runtime.StackTrace`'s `parent`); identifiers that are Rust keywords get a `_` suffix and `#[serde(rename)]`
(`type_`, `override_`); `experimental` and `deprecated` items are kept and say so in their docs. Structs whose
required members all have defaults derive `Default`, so parameters read `..Default::default()`. The message envelope
(`id`, `method`, `params`, `sessionId`, `result`, `error`) is not in the JSON and is typed in `crates/browser`.

Size at this pin: 13 domains, 960 structs, 140 enums, 32 aliases, 337 commands, 121 events; 14 files, about 24,400
lines and 800 KB. The generator is about 840 lines, with 180 lines of tests.

## Regenerating

```
protocol/cdp/fetch.sh                  # only when PIN changes: fetch and verify the JSON
cargo run -p eludite-cdp-generator     # rewrite protocol/rust/src/cdp/ (runs rustfmt, edition 2024)
cargo test -p eludite-cdp-generator    # fails when the checked-in output is not current
```

The generator is plain Rust with `std::fmt::Write` (no proc macros, no templating crate). It needs `rustfmt`, which
the pinned toolchain ships; `RUSTFMT` names another one.
