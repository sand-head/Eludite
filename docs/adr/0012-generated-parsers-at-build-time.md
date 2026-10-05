# ADR-0012: In-repo tree-sitter grammars generate their parser at build time from the checked-in grammar JSON, cached by content

Status: Accepted, 2026-10-05 (the owner's decision on brief 0056's pull request)
Plan reference: PLAN.md sections 2 (principle 4's rule for generated code), 4.1, 7

## Context

Brief 0056 brought the owner's Razor grammar into the repository as `grammars/razor`, the first tree-sitter grammar
Eludite owns rather than takes from crates.io. A tree-sitter grammar is `grammar.js`; the parser is C the tree-sitter
CLI generates from it: one file of state tables, 1.1 million lines and 40 MB for a grammar that embeds all of C# and
HTML, plus `node-types.json` and three headers. Every grammar crate on crates.io checks that C in, and the brief did
the same, so that `cargo build` needs no Node and no CLI on any platform. The pull request then carried 1.1 million
generated lines, three versions of the file above GitHub's 50 MB recommendation in its history, and a diff nobody
can review. The owner decided the parser is generated at build time, and cached so it is not generated on every build.

What a build needs in order to generate: the grammar in JSON (the CLI evaluates `grammar.js` with a JavaScript
runtime to get it), and the generator. The generator is a Rust library, `tree-sitter-generate`, the same code the
CLI runs; from `grammar.json` it needs no JavaScript at all. Generation takes about 15 s for this grammar.

## Decision

- An in-repo grammar checks in **`grammar.js` and `src/grammar.json`** (the JSON written from it by the pinned CLI
  through `generate.sh`, marked generated for diffs) and the hand-written `src/scanner.c` if it has one. **`parser.c`,
  `node-types.json` and `src/tree_sitter/*.h` are never checked in**; `.gitignore` refuses them and `generate.sh`
  removes the copies the CLI writes for its own test run.
- The crate's **`build.rs` generates them into `OUT_DIR`** with `tree-sitter-generate`, `default-features = false`
  (no JavaScript engine; the `load` feature only, for the version in `tree-sitter.json`), pinned exactly to the CLI
  version in `PIN`; a test asserts the two pins agree. ABI 15 and the CLI's default state merging, so the parser is
  byte for byte the one `tree-sitter generate` writes. `NODE_TYPES` is included from `OUT_DIR`.
- **Cached by content.** The output is stored under Eludite's cache folder (`ELUDITE_CACHE_DIR`, else
  `$XDG_CACHE_HOME/eludite`, else `~/.cache/eludite`; `%USERPROFILE%` on Windows, as `tools/*/fetch.sh`) at
  `grammars/<name>/<key>/`, the key an FNV-1a hash of `PIN`, `grammar.json`, `tree-sitter.json` and the ABI. A build
  whose key is present copies instead of generating, so a second worktree or a fresh `target/` pays a copy, not 15 s.
  An entry is written whole under a temporary name and renamed into place; a cache that cannot be written is not an
  error. Within one checkout cargo's own rerun rules apply (`rerun-if-changed` on the inputs), so one target
  directory generates once. CI caches the folder with `actions/cache`, keyed on the same files.
- `generate.sh` stays the only way `grammar.json` changes: it runs the pinned CLI's `generate` and `test` (the CLI's
  own corpus and highlight assertions need Node), then removes the parser. `cargo test -p <grammar>` replays the
  corpus without Node, as before.

## Alternatives considered

- Keep the generated C checked in (the brief, and every crates.io grammar): no generator in the build and no 15 s on a
  cold build, but 40 MB per revision of the grammar in history forever, a pull request diff of a million lines, and
  three oversize blobs already. Lost: the owner's call.
- Git LFS for `parser.c`: keeps the repository small but adds a second transport every clone, CI runner and agent
  worktree must have, and the file still regenerates from a 300 KB JSON in 15 s. Lost.
- Generate from `grammar.js` at build time with the generator's QuickJS runtime (`qjs-rt`): nothing generated in the
  repository at all, but it compiles a JavaScript engine into every build of every platform and makes the build's
  grammar depend on how that engine evaluates the DSL, a second path beside the CLI's Node run. `grammar.json` is
  the CLI's own intermediate and the generator's native input. Lost; revisit below.
- Run `npx tree-sitter generate` from `build.rs`: Node on every machine and CI runner that builds the shell. Lost;
  the rule since brief 0050 is that Node is for optional tools, never for `cargo build`.
- A cache keyed by git revision or by mtime: a key that is not the content regenerates or, worse, reuses a stale
  parser. Lost.

## Consequences

Positive:
- The repository holds the grammar's sources (`grammar.js`, 300 KB of `grammar.json`, the scanner) and nothing
  derived from them; a grammar change is a readable diff.
- The parser is proven equal to the CLI's: the build and `generate.sh` run one generator at one pinned version.
- A cold build of a grammar costs 15 s once per machine per grammar revision, then a copy.

Negative:
- `tree-sitter-generate` and its dependencies (all MIT or Apache-2.0, in the build graph only) are a build
  dependency of the shell; bumping the CLI pin means bumping it and the `tree-sitter` runtime together.
- A machine with no writable home generates on every fresh `target/`.
- `src/grammar.json` is still generated and still diffed as a blob; the sync between it and `grammar.js` rests on
  `generate.sh` being the only way it changes, as before for the parser.

## Revisit when

- A second in-repo grammar arrives: the build script and cache become a shared crate (`grammars/build-support`) rather
  than a copy.
- The generator's QuickJS runtime becomes the CLI's default (`--js-runtime native`): then `grammar.json` can go too.
- Generation of a grammar passes about a minute: look at caching the compiled object as well as the C.
