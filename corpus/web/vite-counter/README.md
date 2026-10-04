# corpus/web/vite-counter

A Vite TypeScript counter (MIT) written for brief 0038: a page whose script vscode-js-debug debugs through the Vite
dev server's source maps, with no file on disk for the generated code.

- `index.html`: a page titled "Vite Counter" with a button (`#counter`) reading `count is N`.
- `src/main.ts` wires the button; `src/counter.ts`'s `setupCounter` adds one per click. A breakpoint on line 5
  (`counter = count;`) stops on a click; Vite serves `/src/counter.ts` transformed to JavaScript with an inline
  source map (`sourcesContent` included), which js-debug maps back to `src/counter.ts` under the web root (this folder).
- `package.json` and the committed `package-lock.json`: Vite 8.3.2 (MIT) and its 40 packages (MIT, Apache-2.0, ISC,
  BSD-3-Clause, MPL-2.0 for lightningcss), development tools of this corpus only, never part of Eludite.
  `node_modules/` is never committed: the real test runs `npm ci` (Node.js 20.19 or 22.12 and later) when
  `ELUDITE_JS_DEBUG` is set, then `npm run dev -- --port PORT`.

Used by brief 0038's real-adapter test in `crates/eludite/src/shell/debug/tests.rs`
(`the_vite_counter_stops_through_the_dev_servers_source_maps`).
