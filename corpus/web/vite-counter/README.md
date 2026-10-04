# corpus/web/vite-counter

A Vite TypeScript counter (MIT) written for brief 0038: a page whose script vscode-js-debug debugs through the Vite
dev server's source maps, with no file on disk for the generated code.

- `index.html`: a page titled "Vite Counter" with a button (`#counter`) reading `count is N`.
- `src/main.ts` wires the button; `src/counter.ts`'s `setupCounter` adds one per click. A breakpoint on line 5
  (`counter = count;`) stops on a click; Vite serves `/src/counter.ts` transformed to JavaScript with an inline
  source map (`sourcesContent` included), which js-debug maps back to `src/counter.ts` under the web root (this folder).
- `package.json` and the committed `package-lock.json`: Vite 8.3.2 (MIT) and, for brief 0050, TypeScript 5.9.3
  (Apache-2.0), ESLint 9.39.5 (MIT) and Prettier 3.9.9 (MIT), with their 128 packages (MIT, Apache-2.0, ISC,
  BSD-2-Clause, BSD-3-Clause, MPL-2.0 for lightningcss, Python-2.0 for argparse), development tools of this corpus only,
  never part of Eludite.
  `node_modules/` is never committed: the real test runs `npm ci` (Node.js 20.19 or 22.12 and later) when
  `ELUDITE_JS_DEBUG` is set, then `npm run dev -- --port PORT`.

Brief 0050's fixtures for the web language servers (none of them is imported by the page, so the debugger's test is
unchanged):

- `eslint.config.js`: ESLint's flat configuration, JavaScript files only, with `prefer-const` as an error.
- `src/lint.js`: `let total = 1;` is never reassigned: ESLint reports `prefer-const` on line 2, and its fix ("Fix this
  prefer-const problem", a `workspace/executeCommand` that ESLint answers with `workspace/applyEdit`) makes it `const`.
- `src/typeError.ts`: `conut * 2` on line 4: TypeScript reports TS2552 ("Cannot find name 'conut'. Did you mean
  'count'?") and offers "Change spelling to 'count'".
- `src/unformatted.ts`: Prettier 3.9.9 (default options) rewrites it to `export function label(name: string, count:
  number) {`, `return name + ": " + count;` and `export const values = [1, 2, 3];`.
- `tsconfig.json` (strict, `src`): typescript-language-server runs on this project's own TypeScript (5.9.3, from
  `node_modules/typescript`), which the status bar names.

Used by brief 0038's real-adapter test in `crates/eludite/src/shell/debug/tests.rs`
(`the_vite_counter_stops_through_the_dev_servers_source_maps`), and brief 0050's real web-server test
(`crates/eludite/src/shell/web_tests.rs`, `real_web_servers_serve_the_vite_counter`) and Xvfb run
(`crates/eludite/tools/web-linux.sh`).
