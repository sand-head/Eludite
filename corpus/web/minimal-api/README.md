# corpus/web/minimal-api

A minimal ASP.NET Core project (`Microsoft.NET.Sdk.Web`, `net10.0`, MIT) written for brief 0037: F5 or Ctrl+F5 in
Eludite runs it with its launch profile and opens its page in the Web Browser window once Kestrel says it listens.

- `GET /`: a page titled "Minimal API" with a form (a text box labelled "Name", a "Greet" button) that posts back to
  itself, and a "What time is it?" button that reads `/api/time`.
- `POST /` with `name=Ann`: the same page with `<p id="greeting">Hello, Ann!</p>` (the name HTML-encoded).
- `GET /api/time`: `{"utc": "<ISO 8601 time>", "app": "MinimalApi"}`.
- `Properties/launchSettings.json`: Visual Studio's template profiles, `http` (`http://localhost:5180`) first and
  `https` (`https://localhost:7180;http://localhost:5180`), both with `launchBrowser: true` and `launchUrl: ""`, so
  F5 opens `http://localhost:5180/`, or `https://localhost:7180/` with the `https` profile when the ASP.NET Core
  development certificate is there (`dotnet dev-certs https --check`).
- Kestrel writes `Now listening on: http://localhost:5180` (Microsoft.Hosting.Lifetime) to stdout when it is up.

Build it with `build.sh` (the Debug configuration, no NuGet package: the SDK's ASP.NET Core shared framework).

Used by `crates/eludite/src/shell/debug/tests.rs`
(`ctrl_f5_on_the_corpus_web_project_opens_the_page_in_the_embedded_engine`, which copies it into a temporary solution
and builds it, and its netcoredbg twin) and the Xvfb run `crates/eludite/tools/web-launch-linux.sh` (also a copy).
