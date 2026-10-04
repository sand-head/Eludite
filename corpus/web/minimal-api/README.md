# corpus/web/minimal-api

A minimal ASP.NET Core project (`Microsoft.NET.Sdk.Web`, `net10.0`, MIT) written for brief 0037: F5 or Ctrl+F5 in
Eludite runs it with its launch profile and opens its page in the Web Browser window once Kestrel says it listens.

- `GET /`: a page titled "Minimal API" with a form (a text box labelled "Name", a "Greet" button) that posts back to
  itself, and a "What time is it?" button that reads `/api/time`.
- `POST /` with `name=Ann`: the same page with `<p id="greeting">Hello, Ann!</p>` (the name HTML-encoded).
- `GET /api/time`: `{"utc": "<ISO 8601 time>", "app": "MinimalApi"}`.
- `wwwroot/` (brief 0038), served as static files: `app.ts`, the page's script, and `app.js` with `app.js.map`
  compiled from it once, committed so no TypeScript is needed at build or test time. The page has a "Price" box (5),
  an "Add" button and a "Total" (`#total`). Add's handler (`onAdd`, app.ts lines 20 to 27) pushes the price into a
  cart and shows `total(cart)`, which has a bug: its loop starts at 1, so the first Add shows 0. A breakpoint on
  app.ts line 25 (`const sum = total(cart);`) stops there on a click; js-debug maps it to app.js line 18. Compiled
  with TypeScript 5.9.3:

  ```
  cd corpus/web/minimal-api && npx -y -p typescript@5.9.3 tsc --target es2020 --sourceMap --strict wwwroot/app.ts
  ```
- `wwwroot/fixtures/` (brief 0050), static files the page does not link: `index.html` (markup with a `<style>` and a
  `<script>` block, highlighted as CSS and JavaScript, for the HTML server's completion and Emmet), `site.css`, and
  `package.json` whose `"private": "yes"` SchemaStore's package.json schema rejects (the JSON server reports
  `Incorrect type. Expected "boolean".` on line 4). The page's own script, `app.ts`, has no `tsconfig.json`: the
  TypeScript server serves it as an inferred project.
- `Properties/launchSettings.json`: Visual Studio's template profiles, `http` (`http://localhost:5180`) first and
  `https` (`https://localhost:7180;http://localhost:5180`), both with `launchBrowser: true` and `launchUrl: ""`, so
  F5 opens `http://localhost:5180/`, or `https://localhost:7180/` with the `https` profile when the ASP.NET Core
  development certificate is there (`dotnet dev-certs https --check`).
- Kestrel writes `Now listening on: http://localhost:5180` (Microsoft.Hosting.Lifetime) to stdout when it is up.

Build it with `build.sh` (the Debug configuration, no NuGet package: the SDK's ASP.NET Core shared framework).

Used by `crates/eludite/src/shell/debug/tests.rs`
(`ctrl_f5_on_the_corpus_web_project_opens_the_page_in_the_embedded_engine`, which copies it into a temporary solution
and builds it, and its netcoredbg twin), the Xvfb run `crates/eludite/tools/web-launch-linux.sh` (also a copy), and
brief 0038's JavaScript debugging: `crates/dap/src/sourcemap.rs` and `crates/dap/tests/js_debug.rs` (the map, and the
real vscode-js-debug on the page), the shell's `ctrl_f5_on_the_corpus_web_project_then_js_debug_stops_in_app_ts` and its
netcoredbg twin, the js-debug conformance scenarios (`corpus/dap/js-debug/`) and the Xvfb run
`crates/eludite/tools/js-debug-linux.sh`.
