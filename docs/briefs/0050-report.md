# Brief 0050 report: the web language stack

Status: done on Linux. Windows and macOS: not run (no machines; `tools/web-servers/fetch.ps1` written, not run).
Branch: `brief/0050-web-language-stack` on `main` at `470f11f`. Date: 2026-10-04.
Brief: [0050-web-language-stack.md](0050-web-language-stack.md). Screenshots: [0050-run/screenshots/](0050-run/screenshots/).

## Summary

- **A `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs`, `.html`, `.css`, `.scss`, `.less`, `.json` or `.jsonc` file is
  highlighted by tree-sitter at once** (TypeScript, TSX, JavaScript with JSX, HTML with `<script>` and `<style>`
  injected as JavaScript and CSS, CSS, JSON and JSON with comments), and then gets completion, Quick Info, Parameter
  Info, Go To Definition, Find All References, rename, code actions, diagnostics and formatting from the language
  servers, through brief 0019's generic path and `servers.json` entries. The shell has no per-language code: the
  entries, their file kinds and `languageId`s, how their executables are found, their activation rule, their options
  and the formatters are data.
- **Several servers per document.** A TypeScript or JavaScript file is served by typescript-language-server and, when
  the project has an ESLint configuration, vscode-eslint's server. The document's session fans each request out to
  the servers that offer it and merges the answers (`eludite_lsp::fanout`, rules below); diagnostics of both show
  together; ESLint's fixes (commands) run with `workspace/executeCommand` on ESLint, which answers with
  `workspace/applyEdit` through the workspace-edit applier.
- **Located, pinned, never vendored, never at startup.** `tools/web-servers/fetch.sh` (`fetch.ps1`) runs `npm ci` from
  a checked-in `package.json` and `package-lock.json` into `~/.cache/eludite/web-servers/2026-10-04/` and caches the
  SchemaStore schemas by checksum; each server is looked for in the project's `node_modules` first (so the project's
  own TypeScript and ESLint win), then `ELUDITE_<SERVER>`, then that cache, then `PATH`, and run by the Node.js
  `languageServers.nodePath` names or brief 0038's search finds (now shared: `crates/lsp/src/node.rs`). Nothing starts
  until a matching document opens; a server that is not found says `not found (run tools/web-servers/fetch.sh)` in its
  status bar slot and nothing else degrades.
- **Format Document** (Edit > Advanced > Format Document, Ctrl+K, Ctrl+D, `eludite.editor.format_document`): the
  project's Prettier (kept warm in a Node.js worker after its first run) or Biome, else the language server's
  formatting, off the UI thread, applied through the applier as one undo step; format on save per language;
  **Emmet** on Tab in HTML and CSS; **JSON schemas** for `package.json`, `tsconfig.json`, `launchSettings.json`,
  `appsettings*.json` and `global.json` from the cached SchemaStore files, with no network.
- **Real servers ran here**: the Vite counter's completion, a type error fixed by an agent through
  `diagnostics.list`, `code_actions` and `apply_code_action`, an ESLint violation fixed the same way, Prettier, the
  JSON schema and the HTML server, all in one headless test (`real_web_servers_serve_the_vite_counter`), and the Xvfb
  run with screenshots.

## Versions and licenses

The servers and formatters are located tools, never linked or bundled; the cache folder holds exactly
`tools/web-servers/package-lock.json`. Every package with its SPDX id is in [`tools/web-servers/PIN`](../../tools/web-servers/PIN)
(a test checks the PIN against the lockfile).

| Package | Version | SPDX | What it is |
|---|---|---|---|
| typescript-language-server | 5.3.0 | Apache-2.0 | The TypeScript and JavaScript server (over tsserver) |
| typescript | 6.0.3 | Apache-2.0 | The fallback tsserver when the project has no `node_modules/typescript` |
| vscode-langservers-extracted | 4.10.0 | MIT | `vscode-html-language-server`, `vscode-css-language-server`, `vscode-json-language-server`, `vscode-eslint-language-server` (its own TypeScript 4.9.5, Apache-2.0, for HTML's embedded scripts) |
| prettier | 3.9.9 | MIT | Formatter (when the project has none, with `editor.formatter: prettier`) |
| @biomejs/biome | 2.5.15 | MIT OR Apache-2.0 | Formatter, only when the project names Biome (`biome.json`); one platform package of the eight `@biomejs/cli-*` (MIT OR Apache-2.0) installs |

Transitive packages (all in PIN): MIT (`@vscode/l10n`, `core-js`, `dom-serializer`, `he`, `jsonc-parser`,
`node-html-parser`, `picomatch`, `regenerator-runtime`, `request-light`, `vscode-css-languageservice` and its
`vscode-languageserver-types`, `vscode-html-languageservice`, `vscode-json-languageservice`, `vscode-jsonrpc`,
`vscode-languageserver`, `vscode-languageserver-protocol`, `vscode-languageserver-textdocument`,
`vscode-languageserver-types`, `vscode-markdown-languageservice`, `vscode-nls`, `vscode-uri`), BSD-2-Clause
(`css-select`, `css-what`, `domelementtype`, `domhandler`, `domutils`, `entities`, `nth-check`), ISC (`boolbase`,
`semver`). The JSON schemas are SchemaStore's (Apache-2.0) at commit `de76181a`, 15 files: the five the JSON entry
associates and every schema they reference (`package.json` refers to `ava`, `eslintrc`, `jscpd`, `madge`, `nodemon`,
`prettierrc`, `quikrun`, `semantic-release`, `stylelintrc`, and `eslintrc` to `partial-eslint-plugins`), each checked
against its SHA-256.

The brief expected MIT for every server: typescript-language-server and TypeScript are Apache-2.0, Biome MIT OR
Apache-2.0; all are compatible with GPL-3.0-or-later and none is linked. Two pins differ from "latest" on purpose:
TypeScript 7 (npm's latest, 7.0.2) is the native port with no `tsserver.js`, which typescript-language-server needs, so
the fallback is 6.0.3 (a project's own TypeScript is used first, whatever its version); typescript-language-server 6.x
requires Node.js 22.22.2 or later, so 5.3.0 (Node.js 20 or later) is pinned. The Node.js minimum for the web servers is
20 (`PIN`'s `node`), vscode-js-debug's stays 18.

New Rust dependencies (crates.io, all MIT): `tree-sitter-typescript` 0.23.2 (TypeScript and TSX),
`tree-sitter-javascript` 0.25.0, `tree-sitter-html` 0.23.2, `tree-sitter-css` 0.25.0, `tree-sitter-json` 0.24.8, in
`crates/editor`. `eludite-dap` now depends on `eludite-lsp` (path dependency) for the shared Node.js search.

## What was built

1. **Schemas** (`87996b2`): `settings.json` gains `languageServers.nodePath` (`ELUDITE_NODE`, the same search as
   `debugger.nodePath`), `languageServers.typescriptPath`, `languageServers.eslint` (`auto`, `on`, `off`),
   `editor.formatter` (`auto`, `prettier`, `biome`, `server`), `editor.formatOnSave.typescript`, `.html`, `.css`,
   `.json` (per language: the key is the registration's id) and `editor.emmet`, on five new Options pages appended
   after the existing ones (Text Editor > All Languages, CSS, HTML, JavaScript/TypeScript, JSON); the Options dialog is
   generated from the schema, so nothing else was needed. Format Document is a user-visible action, so it is a
   command (CLAUDE.md invariant 3): `editor-format-document.input.json` and `.output.json`. host-rpc.md's generic-server
   section documents the entries, the search order, activation, substitutions, pushed settings, the JSON schemas, the
   fan-out and merge rules, commands of code actions and the formatters.
2. **`tools/web-servers/`** (`ec222ae`, `9af220d`): `package.json`, `package-lock.json`, `PIN`, `fetch.sh`, `fetch.ps1`
   and `rewrite-refs.mjs` (which points the cached schemas' `$ref`s to SchemaStore's URLs at the cached copies).
   `fetch.sh` ran here: `npm ci` of 38 packages in 4 to 6 s, 259 MB in the cache.
3. **Grammars and Emmet** (`2f73abb`): the five grammars with Eludite's queries in `crates/editor/queries/` (adapted
   from each grammar's `highlights.scm` for Eludite's first-pattern-wins precedence; TypeScript's query is TypeScript's
   additions then JavaScript's; JSX's patterns come before JavaScript's), four new highlight kinds (`Tag`,
   `AttributeName`, `PropertyName`, `Selector`) with Visual Studio Dark's colors, `LanguageConfig::file_names`
   (`tsconfig.json`, `jsconfig.json` and `.eslintrc.json` are JSON with comments), injections
   (`LanguageConfig::injections_query`/`injected`: HTML's `<script>` and `<style>`, parsed on their own per highlighted
   range and cached per version; template literals' embedded languages are off), SCSS and Less on the CSS grammar,
   `.cshtml` on HTML, and the Emmet expander (`crates/editor/src/intellisense/emmet.rs`) with `LanguageConfig::emmet`
   and `EditorView::set_emmet`/`expand_emmet` on Tab.
4. **The registry, fan-out and Node.js search** (`1380742`): `ServerRegistry::all_for_path`, `languageIds` per glob,
   `CommandSpec::npm_package` and `probe` (`version` or `packageJson`), `Activation`, `ModuleSpec`
   (`locate_module`), `substitute` (`${root}`, `${rootUri}`, `${rootName}`, `${cache}`, `${cacheUri}`,
   `${module:<name>}`), `pushSettings`, `formatters` with `pick_formatter`, `webServers` with `web_servers_cache`;
   `eludite_lsp::fanout` (`offers`, `route`, `merge`, `command_owner`, `dispatch` over a `Member` trait implemented for
   `ServerClient` and the shell's sessions); `eludite_lsp::node` (moved from `crates/dap/src/discovery.rs`, which
   re-exports it with its own `JS_DEBUG_NODE` user); the generic client answers `workspace/workspaceFolders`, treats an
   empty configuration `section` as the whole settings (ESLint asks so) and sends `workspace/didChangeConfiguration`
   for `pushSettings`.
5. **`servers.json`** (in `1380742` and `9af220d`): `typescript`, `eslint`, `html` (also `*.cshtml`), `css`, `json`,
   the formatters `prettier` (with the warm worker's API) and `biome`, and `webServers`.
6. **The shell** (`b89d030`, `25b6e62`): `servers.rs` resolves every activated registration for a file and makes the
   document's session a fan-out (`ServerSession::fan_out`; its requests merge on a waiter thread; resolve requests go
   back to the item's server; cancels reach every server asked); each server's diagnostics are kept apart and merged
   per document, a restart of one keeps the others'; a document's generation is the sum of its servers' (and an edit's
   key is the group, `ServerKey::Group`); `doc_features` is the union; status bar slots show `not found (run
   tools/web-servers/fetch.sh)` and the located module (`TypeScript: ready (TypeScript 5.9.3, project)`); the code
   action applier runs a command a server lists (ESLint's) with `workspace/executeCommand`; `format.rs` holds Format
   Document, format on save and the warm worker; Edit > Advanced > Format Document and Ctrl+K, Ctrl+D (also bound in
   the editor's key context); Emmet follows `editor.emmet`.
7. **Real servers, corpus and Xvfb** (`da62343`, `169fa20`): the fixtures, the real test and
   `crates/eludite/tools/web-linux.sh`.

## Merge rules as shipped

| Method | Rule |
|---|---|
| `textDocument/completion` | Concatenated in server order, each list's `itemDefaults` written into its items, `labelDetails.description` set to the server's name when empty, the server kept in `data` (`eludite.server`); `isIncomplete` if any is |
| `completionItem/resolve`, `codeAction/resolve` | Only the item's server (its own `data` restored); a server that does not resolve gets the item back unchanged |
| `textDocument/codeAction` | Concatenated in server order (TypeScript's, then ESLint's); the menu then groups them (fixes first) |
| `textDocument/hover`, `textDocument/signatureHelp` | First non-empty answer in server order |
| `textDocument/definition`, `references`, `implementation`, `typeDefinition` | Concatenated, duplicate locations dropped |
| `textDocument/formatting`, `prepareRename`, `rename` and any other | The first server, in order, that offers it and answers (one at a time: formatting never runs twice) |
| `workspace/executeCommand` | The server whose `executeCommandProvider.commands` lists the command |
| diagnostics | Each server's own list (push or pull), shown together in the document's server order |

A server that does not offer a method is not asked; one that fails is left out; the request fails only when all fail.

## Tests (what each proves)

`crates/editor` (`syntax/web_tests.rs`, `intellisense/emmet.rs`):

| Test | Proves |
|---|---|
| `typescript_kinds`, `javascript_and_jsx_kinds` | Captures to kinds (and so theme colors) for TypeScript, TSX and JavaScript with JSX: keywords, types, builtins, functions, parameters, constants, properties, strings, template substitutions in the default color, regexes, tags and attribute names; locals stay default |
| `html_kinds_and_its_script_and_style_injections` | HTML's tags, attributes, values, entities, comments, doctype; `<style>` highlighted as CSS and `<script>` as JavaScript; `.cshtml` resolves to HTML |
| `an_injection_spanning_highlight_steps_is_highlighted_in_each` | A script longer than one step's rows is JavaScript in every step |
| `css_kinds`, `json_and_json_with_comments`, `the_language_table_resolves_every_web_suffix` | CSS (selectors, properties, at-rules, custom properties, units), SCSS and Less on it; JSON keys apart from values, comments in `.json` and `.jsonc`; `tsconfig.json` is JSON with comments; every suffix |
| `a_keystroke_in_a_10000_line_typescript_file_is_highlighted_within_the_budget` | The budget (below) |
| `emmet::tests` (5) | The expander's forms: elements, classes, ids, attributes, text, `>`, `+`, `^`, `*N`, `$`, groups, implicit tags, void elements, indentation; CSS shorthands with units and keywords; plain words and broken abbreviations are not expanded; where an abbreviation starts |

`crates/lsp`:

| Test | Proves |
|---|---|
| `registry::several_servers_resolve_for_one_file_in_registration_order` | `.ts`/`.js` kinds get TypeScript then ESLint, the others one server; `languageId` per glob; every web entry is an npm package with its variable; the JSON entry's schema associations and file-only schemas |
| `registry::npm_servers_are_found_in_the_project_then_the_variable_then_the_cache_then_path` | The search order, a script under the Node.js found, `--version` under Node.js, the package version for servers without `--version`, the messages without Node.js and without the server |
| `registry::eslint_activates_on_a_configuration_file` | `auto` with flat or legacy configuration at or above the root, `on`, `off` |
| `registry::modules_are_the_setting_then_the_project_then_the_pinned_one`, `substitutions_fill_strings_everywhere` | The project's TypeScript wins, the setting over it; `tsserver.path` and the other substitutions |
| `registry::the_format_chain_is_project_prettier_then_biome_then_the_server` | `auto` (project Prettier, else Biome by `biome.json`, else the server), `prettier`/`biome` anywhere, `server`, file kinds, not-found error naming the fetch command |
| `registry::the_pin_matches_the_lockfile_and_servers_json`, `the_web_servers_cache_is_the_variable_else_the_pinned_folder` | PIN lists every lockfile package with version and SPDX id; the pin servers.json searches; the cache folder rule |
| `fanout::tests` (5) and `tests/fanout.rs` (5, two `FakeServer`s behind real `ServerClient`s) | The merge rules above, capabilities deciding who is asked, resolve routing, ESLint's command run on ESLint only, formatting from the first server only (and the next one when it fails), references deduplicated, failures left out |
| `node::tests` (2) | The shared Node.js search order and the per-user minimum version (PIN's `node`) |
| `server::tests` | The empty configuration section, the new methods documented in host-rpc.md |

`crates/eludite` headless (`shell/web_tests.rs`, fake servers behind the real registrations):

| Test | Proves |
|---|---|
| `nothing_starts_until_a_web_document_opens_then_typescript_and_eslint_serve_it` | No server at startup (the session and connection counters); a `.ts` opens both TypeScript and ESLint with `languageId` `typescript`, `tsserver.path` at the project's TypeScript and the slot naming it; ESLint's configuration with the workspace folder; `.js` as `javascript` on the same processes; HTML only the HTML server |
| `the_eslint_setting_decides_whether_eslint_runs` | `languageServers.eslint: off` through `eludite.settings.set` |
| `ts_editor_features_and_both_servers_diagnostics` | Completion, Quick Info, Go To Definition, Find All References and rename in a `.ts` file; TypeScript's and ESLint's diagnostics merged (squiggles, Error List rows, `diagnostics.list`) with ESLint's rule id |
| `eslints_fix_runs_its_command_and_applies_as_one_undo_step` | ESLint's actions listed with TypeScript's, its diagnostic in the request's context, its fix run with `workspace/executeCommand` on ESLint only, the applied edit one undo step |
| `format_document_runs_the_projects_prettier_then_the_server` (Unix) | Ctrl+K, Ctrl+D runs the project's Prettier (a script package in `node_modules`), one undo step; `server` formats through TypeScript; a failing formatter leaves the document and writes an Output line |
| `format_on_save_formats_before_the_file_is_written` (Unix) | `editor.formatOnSave.typescript` formats before writing; off writes as typed |
| `emmet_expands_on_tab_in_html_and_css` | Tab expands in HTML (caret in the first item) and CSS, not a plain word, not with `editor.emmet` off, not in TypeScript |
| `a_server_that_is_not_found_names_the_fetch_command` | The slot `CSS: not found (run tools/web-servers/fetch.sh)`, the reason in Output, highlighting and editing unaffected |
| `a_cshtml_view_gets_html_highlighting_and_the_html_servers_completion` | `.cshtml` as `html` to the HTML server, HTML grammar, its completion |
| `the_json_server_gets_the_cached_schema_associations` | Pushed settings with the cached schema URLs, `handledSchemaProtocols: ["file"]`, `jsonc` for `tsconfig.json` |
| `a_crashed_eslint_restarts_and_typescripts_diagnostics_stay` | One server's crash restarts it with the document replayed, the document's generation moves, the other's diagnostics stay |
| `format::tests::the_warm_worker_formats_reports_errors_and_gives_way` | The worker loads a package's API once, answers each request with its configuration, reports a formatter error, and gives way when the package cannot load |
| `real_web_servers_serve_the_vite_counter` | The real servers (below); skips without `ELUDITE_WEB_SERVERS` |

Also updated: `eludite-commands`' settings schema test (the new keys and pages), `eludite-ui`'s menu test (Format
Document's shortcut), `eludite-dap`'s js-debug test (the shared search).

## Real servers (run here)

`real_web_servers_serve_the_vite_counter`, with `ELUDITE_WEB_SERVERS="$(tools/web-servers/fetch.sh)"`, copies
`corpus/web/vite-counter` and `corpus/web/minimal-api/wwwroot/fixtures`, runs `npm ci` (Vite, TypeScript 5.9.3, ESLint
9.39.5, Prettier 3.9.9) and opens it headless with no fakes:

- `main.ts`: `querySelector` after `document.` from typescript-language-server 5.3.0 on the project's TypeScript 5.9.3
  (`TypeScript: ready (TypeScript 5.9.3, project)`).
- An agent fixes `src/typeError.ts`'s TS2552 through the bus: `diagnostics.list` → `eludite.editor.code_actions` at
  its row → `eludite.editor.apply_code_action` with "Change spelling to 'count'" (`state: applied`; the text has
  `count * 2`).
- An agent fixes `src/lint.js`'s ESLint `prefer-const` the same way: the list has "Fix this prefer-const problem",
  "Fix all auto-fixable problems" and the disable actions; applying the fix runs `eslint.applySingleFix` and `let`
  becomes `const`; a new violation is reported after a save.
- `src/unformatted.ts`: Format Document runs the project's Prettier 3.9.9 (`export function label(name: string, count:
  number) {`, `export const values = [1, 2, 3];`).
- `fixtures/package.json`: `Incorrect type. Expected "boolean".` from the JSON server against the cached SchemaStore
  schema (no network; before the referenced schemas were cached, the server reported that it could not load
  `nodemon.json`, which is why the whole closure is pinned).
- `index.html`: `div` among the HTML server's completion items after `<`.

## Budget numbers

Machine: this container (16 threads; the coordinator's verification build ran at the same time, load 2 to 11), debug
builds (no release build was allowed by disk), Xvfb with Mesa's software Vulkan for the UI run.

| Budget | Measured | Verdict |
|---|---|---|
| Highlighting within the 8 ms p99 keystroke budget, 10,000-line TypeScript | UI thread's part per keystroke (moving the shown spans through the edit): **0.55 to 0.70 ms p99** (3 runs, 272 keystrokes each). The re-parse and visible-row highlight run on the syntax thread: p50 11.7 to 12.9 ms, p99 226 to 279 ms (typing an unfinished statement, `{` or `"` open, makes tree-sitter's error recovery re-parse far); full first highlight 134 to 164 ms | Met for the frame (the UI thread never parses) |
| Keystroke frame cost in the shell, 10,000-line TypeScript with TypeScript and ESLint running (`--bench-type 300`, Xvfb) | p50 291 ms, p95 454 ms, p99 630 ms; the same file without servers 252/307/354 ms; a 10,000-line Rust file without servers 362/458/516 ms | Not comparable to the budget: the debug build on software rendering is slow for every language (Rust is slower than TypeScript); needs a release build on the reference machine |
| First completion from typescript-language-server, cold, under 1.5 s | **1.50 to 1.53 s** from opening `main.ts` (server start, tsserver loading the project) | At the budget under load |
| Completion warm under 100 ms p95 | **92 to 95 ms p95** (20 requests, through the fan-out) | Met |
| ESLint diagnostics within 2 s of a save | **159 to 176 ms** | Met |
| Format Document, 2,000 lines, Prettier warm, under 500 ms | First run on the file 0.90 s (Prettier's own JIT warm-up), then **0.47 to 0.57 s**, request to applied (the worker's Prettier takes 0.46 to 0.56 s of it; a fresh `node prettier` process took 0.70 to 0.75 s, and in-process Prettier 0.26 to 0.35 s at lower load) | At the budget under load; Prettier itself dominates |

## Xvfb run

`crates/eludite/tools/web-linux.sh OUT_DIR` (the Vite counter copied and `npm ci`'d, the real servers):
[completion in `main.ts`](0050-run/screenshots/web-completion.png) with the status bar's `TypeScript: ready
(TypeScript 5.9.3, project)` and `ESLint: ready`; [Ctrl+. on ESLint's violation](0050-run/screenshots/web-eslint-menu.png)
(ESLint's fixes first, TypeScript's refactorings after) and [the fix applied](0050-run/screenshots/web-eslint-fixed.png);
[before](0050-run/screenshots/web-format-before.png) and [after](0050-run/screenshots/web-format-after.png) Ctrl+K,
Ctrl+D; [Emmet `ul>li.item$*3` and Tab in `index.html`](0050-run/screenshots/web-emmet.png).

## Not done, and why

- **Semantic tokens and inlay hints.** The brief lists them "as C# does today", but the shell requests neither for any
  server (C# included); adding them is a feature of the editor and the shell for every language, not this brief's.
  The fan-out treats `textDocument/semanticTokens/full` and `textDocument/inlayHint` as first-server methods for when
  they come.
- **The status bar has no tooltips** (`crates/ui/src/status.rs`, outside this brief): the TypeScript version and its
  source are in the slot's text instead.
- **Emmet from the HTML server**: vscode-langservers-extracted's HTML server has no Emmet (`@vscode/emmet-helper` is
  VS Code's extension, not the server's), so Tab uses the editor's expander for every form the brief names.
- **Format on save** runs for File > Save (Ctrl+S) and `eludite.editor.save`; Save All and closing with save write
  without formatting.
- **The warm Prettier worker** uses Prettier's API, which does not read `.prettierignore` (the process path does);
  Biome (a native program) runs as a process each time.
- **Release-build keystroke numbers**: not taken (disk); the debug-build Xvfb numbers above are for comparison only.
- **Windows and macOS**: `fetch.ps1` and the Windows `npm.cmd` path are written, not run; `.bin` shims are not used
  (the search reads the package's `bin` entry, which is what `.bin` links to, the same on every OS).

Files touched outside the brief's list, because the brief's contract needed them: `crates/commands`
(`eludite.editor.format_document`, the settings test), `crates/ui/src/keymap.rs` (Ctrl+K, Ctrl+D), `crates/dap` (the
Node.js search moved out), `crates/eludite/src/shell.rs`, `documents.rs`, `code_actions.rs` (`workspace/executeCommand`),
`rename.rs`, `workspace_edit.rs`, `settings.rs`, `project_properties.rs` (`SaveOutput::pending`) and the debug call
sites of the Node.js search; new files `crates/lsp/src/fanout.rs`, `crates/lsp/tests/fanout.rs`,
`crates/eludite/src/shell/format.rs`, `crates/editor/src/intellisense/emmet.rs`, `tools/web-servers/rewrite-refs.mjs`.
`eludite.editor.format_document` is new for agents (the brief said "nothing new"), because Format Document is a
user-visible action and CLAUDE.md invariant 3 makes it a command.

## What Razor and Blazor need (the next brief)

- A Razor language server: Roslyn's Razor cohosting inside `eludite-host` (Microsoft.CodeAnalysis.Razor and the
  Razor compiler, so C# in `@code` and `@expr` regions gets Roslyn's features), or the standalone `rzls` behind the
  host; either way it stays out of the shell (invariant 2).
- The HTML parts delegated to `vscode-html-language-server` the way VS Code's Razor client does: Razor asks the client
  for completion, hover and formatting in an HTML virtual document; the fan-out and `servers.json` here can carry that
  document if the shell answers Razor's `razor/htmlUpdate`-style notifications (a new section in host-rpc.md first).
- A Razor grammar for tree-sitter (none is on crates.io; `.cshtml` and `.razor` use the HTML grammar now, which colors
  Razor code as text), `.razor` routed (now unhandled) and `.cshtml` moved from the `html` entry to the Razor one.
- Blazor's component tag completion and `_Imports.razor` from the project system (the host's MSBuild evaluation).

## What Vitest, Jest and Playwright need from the Test Explorer

- A JavaScript test runner process speaking the Test Explorer's protocol, out of the shell (invariant 2) with a schema
  in `protocol/` (invariant 4): discovery from `vitest list --json`, `jest --listTests` plus `--json
  --testLocationInResults`, `playwright test --list --reporter=json`; runs streaming results from their JSON reporters
  (Vitest's and Playwright's reporters stream per test; Jest's `--json` arrives at the end unless a custom reporter is
  used), mapped to file > describe > test with locations.
- The runner found like the formatters here: the project's `node_modules/.bin`, run by the shared Node.js search.
- Debug Test through brief 0038's vscode-js-debug: a Node.js launch (`--inspect-brk`) of the runner for the chosen
  test, compound with nothing; Playwright's browser tests through the embedded engine's debugging port.
- Run targets from `package.json` scripts are brief 0051's.

## For merging

- `Cargo.lock`: the five grammar crates, and `eludite-dap` → `eludite-lsp`.
- `protocol/schemas/settings.json`: five new pages appended after NuGet's, nine keys appended at the end (the settings
  test and the Options dialog's page indexes keep the earlier pages' places).
- `corpus/web/vite-counter/package-lock.json` now has 128 packages (TypeScript, ESLint, Prettier with Vite), so brief
  0038's real Vite test's `npm ci` installs more; the page and the breakpoint lines are unchanged.
- The real web-server test needs `ELUDITE_WEB_SERVERS="$(tools/web-servers/fetch.sh)"` (npm's registry and
  raw.githubusercontent.com reachable) and npm for the corpus's `npm ci`.
