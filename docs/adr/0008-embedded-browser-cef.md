# ADR-0008: Embedded browser: Chromium through CEF, out of process, CDP as the automation substrate

Status: Proposed, 2026-10-02
Plan reference: PLAN.md sections 1, 2 (principles 1 to 3, 5), 3 (D2), 4.9, 5.1 to 5.4, 7; proposal 0002

## Context

Web work is a first-class workload (PLAN.md section 7) and the product is agent-first (section 5). Testing a web application means looking at it in a browser, and an agent testing it means driving a browser: navigate, read the page, click, type, read the console and the network log, take screenshots. Visual Studio has a Web Browser tool window; Claude in Chrome shows what agent control of a browser looks like. Both drivers need the same tabs and the same commands (invariant 3).

The owner's constraint on the IDE itself stands: no Electron, Tauri, WebViews or a VS Code extension runtime (invariant 11); the IDE's UI is drawn by GPUI. A browser engine rendering the user's application in a tool window is a different thing from the IDE's UI being HTML, and this ADR draws that line.

Candidate engines: Chromium via the Chromium Embedded Framework (CEF), Servo, Gecko, and the platform web views.

## Decision

- Embed Chromium through CEF (BSD-3-Clause), using the `cef` crate (Apache-2.0 OR MIT) for bindings. CEF is fetched at run time by a pinned, checksummed script (`tools/cef/`), never vendored and never downloaded at startup.
- Run the engine in its own process, `eludite-browser` (`browsers/chromium`, GPL), started when the Web Browser window first opens. The shell receives frames through shared memory and talks to the engine over JSON-RPC on stdio with a CDP pass-through. Nothing of the engine is linked into the shell.
- Use the Chrome DevTools Protocol as the single substrate for every automation command. Generate the CDP domain types from the pinned protocol JSON into `protocol/cdp/`; never hand-edit them.
- Keep the command layer (`crates/browser`, `eludite.browser.*`) engine-neutral behind one trait, with CDP as the first implementation, so a second engine is an implementation, not a redesign.
- The engine renders user content only. No part of Eludite's own UI is ever drawn by the engine, so invariant 11 and PLAN.md section 1 are unchanged in spirit; their wording is amended on acceptance to say so.
- The engine uses a per-workspace profile under `.eludite/browser/`, never the user's own browser profile.

## Alternatives considered

- Servo (MPL-2.0, Rust): the embedding API matured through 2025 and 2026, DevTools gained breakpoints in 0.0.6, WebDriver and accessibility are in progress. Web compatibility is not yet what a tester needs, there is no CDP so every automation command would be bespoke, and builds take ten minutes or more. Kept as the planned second engine behind the same trait.
- Gecko: no desktop embedding API; GeckoView is Android-only. Firefox stays reachable only as an external browser over WebDriver BiDi.
- Platform web views (WebView2, WKWebView, WebKitGTK): three engines with three behaviors, no CDP on WebKit, and the very thing invariant 11 excludes.
- Driving the user's installed Chrome over CDP with no embedding: weak for the person, since the page is not in the IDE. Used as the first proving step for the commands (proposal 0002, brief A), not as the product.
- Linking libcef into the shell: simpler frame delivery, but hundreds of megabytes mapped at startup, a renderer crash inside the IDE process, and a violation of invariant 2.

## Consequences

Positive:
- The engine users' users run; every automation need (screenshots, accessibility tree, input, console, network, emulation) is one protocol.
- Crash isolation and an untouched cold-start budget.
- JavaScript debugging through vscode-js-debug attaches to the same tab over the same protocol.
- The command layer is the agent's browser API and the person's toolbar at once.

Negative:
- About 100 MB per platform fetched on first use; a SUID sandbox helper on Linux distributions that restrict user namespaces; helper bundles in the macOS application bundle.
- Software offscreen frames pass through GPUI's image path, which has no external-texture element off macOS at the pinned revision. If the spike misses the frame budget, the accelerated path needs a GPUI patch.
- CEF follows Chromium's four-week cadence; updates are deliberate and pinned.
- Without proprietary codecs, H.264 and AAC media do not play in the window.

## Revisit when

- The spike (proposal 0002, brief S) cannot keep the shell under 8 ms p99 frame cost with either the software or the accelerated path.
- Servo renders the project's web corpus correctly and exposes CDP, WebDriver BiDi or an equivalent automation surface; then it becomes the second implementation of the engine trait.
- CEF's license or distribution changes, or the `cef` crate stops tracking releases.
- A GPUI revision adds an external-texture or surface element on Linux and Windows.
