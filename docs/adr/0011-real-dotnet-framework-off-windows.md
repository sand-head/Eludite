# ADR-0011: The real .NET Framework off Windows runs under Wine, Eludite distributes none of it

Status: Proposed, 2026-10-04 (with proposal 0003)
Plan reference: PLAN.md sections 3 (D4, D7), 4.5, 4.9, 6, 10 (Phases 3 and 5), 14 (decision 3)

## Context

PLAN.md section 6 accepted that legacy .NET is "edit, analyze, build anywhere; debug on Windows" for v1, with Mono and Wine as later investigations (ADR-0007). Brief 0022 shipped the Mono soft-debugger adapter, which runs only what Mono runs: no WPF, partial WCF, an unfinished ASP.NET 4.5 async pipeline. Users on Linux and macOS with WebForms and WCF code, the product's primary workload, still had no local way to run their program on the runtime it will ship on.

Proposal 0003's spike (2026-10-04, Linux, Wine 11.0) installed Microsoft's own .NET Framework 4.8 into a Wine prefix with `winetricks dotnet48`, compiled with Roslyn's `csc.exe` there, ran the result on the real CLR 4.0.30319, served a WebForms page through the real `System.Web` from a 60-line `System.Web.Hosting` host, and debugged the process with this repository's `eludite-dbg-netfx` cross-compiled for Windows and run under Wine, unchanged: attach, a breakpoint on a line, the stack, locals, continue, detach.

Microsoft's license for the framework says the user must hold a Windows license and that the Distributable Code may not be distributed "to run on a platform other than the Windows platform". IIS Express is a Windows installer over `http.sys` and does not run under Wine. Apple Silicon has no upstream Wine build; CrossOver runs Wine through Rosetta 2 today and natively through FEX in its 2026 preview, and Apple removes most of Rosetta 2 in macOS 28.

## Decision

- Off Windows, a .NET Framework target runs and debugs on the real framework in a Wine prefix when one is found, on Mono when only Mono is found, and over the remote DAP transport (ADR-0007) to a Windows machine or VM otherwise. One setting, `dotnet.frameworkRuntime`, overrides the choice. Nothing of Wine or Mono is touched before F5.
- `eludite-dbg-netfx` stays one code base: a DAP server over ICorDebug that runs wherever Windows user mode runs, Windows or Wine. Linux builds it for `x86_64-pc-windows-gnu`, packages it beside `eludite`, and runs its end-to-end tests under Wine in CI.
- Eludite never ships, downloads or caches Microsoft's framework installer. The user installs it with their own tools; Eludite locates the result, prints the command when it is missing, and states the Windows-license condition wherever it offers to run that command.
- Eludite writes and ships `eludite-webhost` (GPL, C#, `System.Web.Hosting` over `HttpListener`) as the WebForms host where IIS Express is absent: under Wine, under Mono and on Windows without IIS Express.
- On Apple Silicon the product recommends a Windows 11 ARM64 guest (the framework is native ARM64 there since 4.8.1) over the remote transport, and uses CrossOver's Wine when it is installed.

## Alternatives considered

- A new runtime or a `net48`-on-.NET-10 shim: Mono is two decades of the first and still partial; the second has no `System.Web.UI`, `AppDomain`s or WCF server stack to shim onto. Both fail PLAN.md principle 6 and the one-person team.
- Mono only: works, already shipped, but runs a different runtime than production and cannot run WPF or most WCF.
- Remote to Windows only: honest, but makes every Linux user keep a Windows machine warm to run a console program.
- Bundling the framework or running `winetricks` silently: forbidden by the license's distribution clause; the user's own install is what every Wine user does today.
- Porting IIS Express or running it under Wine: it is a Windows installer over a kernel driver Wine implements only partly, and its license is Windows-bound too.

## Consequences

Positive:
- Linux gets a local run-and-debug path on the real framework with no new debugger, and the ICorDebug adapter gets Linux CI coverage.
- WebForms pages run, with the browser and server debugged together (proposal 0002 brief D), on Linux.
- The product's statement in PLAN.md section 6 improves from "debug on Windows" to "debug anywhere the real framework runs".

Negative:
- Wine is the user's to install and keep working; Eludite explains failures in the prefix and fixes none of them.
- Startup under Wine costs a wineserver start and an emulated loader; the proposal records the numbers and shows them in the Output window.
- 32-bit debuggees need the i686 adapter build and a bitness rule, the same one Windows needs.
- Apple Silicon stays a VM or a commercial product until a native ARM64 Wine exists upstream.

## Revisit when

- An upstream WineHQ build runs on macOS ARM64 and the ARM64 framework installs in it: the Apple Silicon recommendation flips to Wine.
- Wine's debug API fails a step, exception or Edit and Continue case the e2e suite needs and WineHQ does not fix it within a release: fall back to Mono for that feature and say so in the status bar.
- Microsoft changes the framework's license terms on distribution or platform.
- A permissive, cross-platform .NET Framework debugger or runtime appears.
