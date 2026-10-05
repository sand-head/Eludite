# tree-sitter-razor

The tree-sitter grammar for Razor (`.razor` Blazor components, `.cshtml` Razor Pages and MVC views): Razor's
directives, transitions, expressions and control structures over tree-sitter-c-sharp's C#, with HTML markup. It is the
owner's library, brought into this repository by brief 0056 so the editor highlights Razor at once and so fixes land
here with their tests.

License: MIT (`LICENSE`; the upstream author's copyright line is kept). It is not part of the GPL product's licensing
question: the grammar is linked into the shell like every other MIT grammar.

## Provenance

- Source: `https://git.sand.town/sand_head/tree-sitter-razor` at commit `bed3116ffead1dd67a9c702d5f85e74c429694cd`
  (2026), the owner's fork of [tris203/tree-sitter-razor](https://github.com/tris203/tree-sitter-razor) (MIT, Tristan
  Knight).
- `vendor/tree-sitter-c-sharp/grammar.js` is tree-sitter-c-sharp 0.23.5's grammar definition, copied verbatim (its
  `package.json` there says so) so `tree-sitter generate` needs no npm install. It is the same version the editor's
  `tree-sitter-c-sharp` crate is, so the C# node names match and the editor's C# highlight query applies to Razor's
  code regions unchanged. Bump both together.
- Dropped from the library's tree: the Node, Python, Go, Swift and C bindings, `package.json`, `binding.gyp`,
  `CMakeLists.txt`, `Makefile`, `Package.swift`, `pyproject.toml`, `setup.py`. The Rust binding is `src/lib.rs` and
  `build.rs`. Kept: `grammar.js`, `queries/` (the library's own queries, for reference and for `tree-sitter test`'s
  highlight assertions under `test/highlight/`; the editor's queries are `crates/editor/queries/razor/`),
  `test/corpus/`, `tree-sitter.json`, `src/` (generated).

## Layout

| Path | What |
|---|---|
| `grammar.js` | The grammar definition. Extends `vendor/tree-sitter-c-sharp/grammar.js`. |
| `src/grammar.json` | The grammar as JSON, written from `grammar.js` by `generate.sh`; never edited by hand. The input of `build.rs`. |
| `src/scanner.c` | tree-sitter-c-sharp's external scanner (the interpolated and raw string tokens), carried by the grammar. |
| `src/lib.rs`, `build.rs`, `Cargo.toml` | The Rust crate `tree-sitter-razor`: `LANGUAGE` and `NODE_TYPES`. `build.rs` generates `parser.c`, `node-types.json` and `tree_sitter/*.h` from `src/grammar.json` into `OUT_DIR` with `tree-sitter-generate` (the CLI's own generator, pinned to `PIN`'s version), caches them under `~/.cache/eludite/grammars/razor/<key>/` keyed by the inputs, and compiles them with the scanner (ADR-0012). Nothing generated is checked in. |
| `test/corpus/*.txt` | The grammar's cases, in `tree-sitter test`'s format. `tests/corpus.rs` replays them with `cargo test`, so CI needs no CLI. |
| `tests/fixtures.rs` | Every file under `corpus/web/razor/` parses with no error node. |
| `generate.sh`, `generate.ps1`, `PIN` | Regeneration of `src/grammar.json` with the pinned CLI (`npx`), then the CLI's own test run; the parser the CLI writes for that run is removed after. |

## Regenerating

```
grammars/razor/generate.sh      # generate.ps1 on Windows
cargo test -p tree-sitter-razor
```

The CLI version is `PIN` (0.27.0, matching the editor's `tree-sitter` 0.27 runtime; the parser's ABI is 15), and
`Cargo.toml` pins the `tree-sitter-generate` build dependency at the same version (`tests/pin.rs` checks), so the
parser `build.rs` writes is the one the CLI's test run proved. A different generator writes a different parser for the
same grammar, so bump `PIN`, the build dependency and the runtime crate together. Generation takes about 15 s once per
cache key; `cargo build` then reuses it from `target/` and, in a fresh checkout, from the cache folder
(`ELUDITE_CACHE_DIR`, else `$XDG_CACHE_HOME/eludite`, else `~/.cache/eludite`).

## Changes from the library (`bed3116`)

Each is meant to go back upstream; the corpus case that proves it is named.

- (none yet beyond the layout above; brief 0056 lists the gaps it closes and this section grows with them)
