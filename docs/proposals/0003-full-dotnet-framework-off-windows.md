# Proposal 0003: The full .NET Framework on Linux and macOS

Status: Proposed, 2026-10-04 (spike run the same day on Linux; see section 3)
Plan reference: PLAN.md sections 1 (vision: "honest about what each platform can and cannot do for legacy .NET"), 2 (principles 1 to 3, 5, 6, 9), 3 (D4, D7), 4.5 (cross-platform .NET Framework debugging, options 2 and 3), 4.9 (launch), 6, 10 (Phases 3 and 5), 14 (decision 3)
Related: ADR-0007 (remote-capable debuggers), ADR-0011 (proposed with this document), briefs 0003 (legacy load, Mono MSBuild), 0004 (ICorDebug proof), 0022 (Mono adapter), proposal 0002 (the Web Browser window as the page's host)
New paths: `tools/wine/`, `web/host` (`eludite-webhost`), `protocol/schemas/dotnet-framework-runtimes.*.json`

## 1. Goal

A developer on Linux or macOS opens a .NET Framework solution, presses F5, and the program runs on **Microsoft's own .NET Framework**, the real CLR 4.0.30319 and the real `mscorlib`, `System.Web`, `System.ServiceModel` and `System.Windows.Forms`, with the same debugger windows and the same `eludite.debug.*` commands as on Windows. WebForms pages render through the real `System.Web` pipeline in a host Eludite ships, because IIS Express does not exist off Windows. When the real framework is not installed, Eludite says so in one Output line that names the three ways to get it, and falls back to Mono (brief 0022) for what Mono can run.

PLAN.md section 6 accepted "edit, analyze, build anywhere; debug on Windows" for v1, with Mono and Wine as later investigations. This proposal is that investigation, done, with a verdict: **the real framework runs under Wine today, Eludite's own ICorDebug adapter debugs it there unchanged, and the pieces missing are product work, not research.**

## 2. The three ways, stated plainly

There is no fourth. Every path to running .NET Framework code off Windows is one of these, and the proposal uses all three, each where it is the right tool.

| Path | What it is | Fidelity | Cost to the user | Where it fits |
|---|---|---|---|---|
| **A. The real framework under Wine** | Microsoft's `ndp48-x86-x64-allos-enu.exe` installed into a Wine prefix (`winetricks dotnet48`, which removes Wine's own `wine-mono` first). The real CLR, class library, `csc.exe`, `MSBuild.exe` 4.0 and `aspnet_compiler.exe` run as Windows programs on Wine's Win32 layer. | Full for managed code and most of Win32. No kernel drivers, so no IIS Express and no `http.sys` beyond Wine's partial one. Hardware, COM servers and shell integration are Wine's. | Install Wine 10 or later from the distribution, run one `winetricks` command (about 10 minutes, 120 MB from Microsoft). A Windows license, see section 7. | **The local path on Linux and Intel macOS.** Apple Silicon through CrossOver or, later, a native ARM64 Wine (section 6). |
| **B. A Windows guest** | Windows in a VM beside Eludite (QEMU/KVM on Linux; Virtualization.framework, UTM or Parallels on macOS, where Windows 11 ARM64 runs .NET Framework 4.8.1 natively), reached over the remote DAP transport ADR-0007 already requires. | Complete: IIS Express, COM, Windows-only MSBuild targets, the Visual Studio Build Tools. | A Windows license or the 90-day Enterprise evaluation, 20 GB of disk, RAM for the guest; Microsoft's ready-made developer VMs were withdrawn in October 2024. | **The complete path, and the Apple Silicon path until Wine runs there.** |
| **C. A reimplementation** | Framework Mono, since March 2025 maintained by WineHQ (6.14: native macOS ARM64, WinForms on X11, COM fixes). Eludite already debugs on it (brief 0022) and evaluates legacy projects with its MSBuild (brief 0003). Mono's class library is partly Microsoft's MIT reference source and partly Mono's own, which is why WebForms works but WPF does not, WCF is partial and the ASP.NET 4.5 async pipeline is unfinished. | Partial by construction and frozen at roughly .NET 4.7. | One distribution package. | **The zero-setup fallback**, kept as it is. |

"Build it yourself", the other half of the question, has two readings and both are rejected:

- *A new reimplementation.* Mono is twenty years of exactly that work and still lacks WPF and half of WCF. Nothing Eludite could write would be more complete, and PLAN.md principle 6 (speak protocols, do not rebuild runtimes) and the one-person team rule it out.
- *A compatibility shim that runs `net48` assemblies on .NET 10.* The modern runtime has no `System.Web.UI`, `System.ServiceModel` server stack, `AppDomain`s, remoting, CAS or `System.EnterpriseServices`, and the WebForms and WCF code this product exists for uses all of them. Microsoft's own answer was CoreWCF and Blazor migration, not a shim. Reject.

What Eludite does build itself is the one Windows-only piece every path lacks: a **WebForms host** that replaces IIS Express (section 4.3). It is a few hundred lines of C# over `System.Web.Hosting`, the design Cassini and XSP used, and it runs on the real framework under Wine, on Mono, and on Windows.

## 3. The spike: the real framework under Wine, debugged by Eludite's adapter

Run on 2026-10-04 in this session's container (Ubuntu 24.04, x86-64, 4 cores, 15 GB, no GPU, no display: Xvfb), with the WineHQ `winehq-stable` 11.0 packages for noble, `winetricks` 20260125-next, Microsoft's `ndp48-x86-x64-allos-enu.exe` as `winetricks dotnet48` fetches it, Roslyn's `csc.exe` from the NuGet package `Microsoft.Net.Compilers.Toolset` 5.9.0 (MIT) and `eludite-dbg-netfx` from this checkout cross-compiled with `cargo build -p eludite-dbg-netfx --target x86_64-pc-windows-gnu --release` (mingw-w64; no source change). The scripts are in section 3.3 and [`0003-run/`](0003-run/).

### 3.1 What ran

| Step | Result |
|---|---|
| `winetricks -q dotnet48` into a fresh `WINEARCH=win64` prefix (Xvfb, no desktop) | Installed in 7 min 20 s wall clock. The prefix is 2.3 GB with the installer's temporary files. `HKLM\Software\Microsoft\NET Framework Setup\NDP\v4\Full` reads `Release` 528040 (0x80eb1) and `Version` 4.8.03761, the same values a Windows machine shows for .NET Framework 4.8. The prefix reports Windows 7 SP1, as winetricks sets it; `Framework64\v4.0.30319` holds `clr.dll`, `mscordbi.dll`, `csc.exe` (C# 5, 4.8.3761.0), `MSBuild.exe` 4.8.3761.0, `aspnet_compiler.exe` and `System.Web.dll`. |
| Roslyn `csc.exe` 5.9.0 (`net472`) under Wine compiling brief 0004's `Program.cs` with `-debug:portable -platform:x64 -optimize-` | `Counter.exe` and a portable `Counter.pdb` in 6.0 s wall clock including Wine's start; the host program below in 5.4 s. |
| A console program printing the runtime, cold (no `wineserver`) and warm | `CLR 4.0.30319.42000 on Microsoft Windows NT 6.1.7601 Service Pack 1 64-bit=True`. 1.23 s cold, 0.28 and 0.29 s warm. The same program under Mono 6.8 on this machine: 0.03 s. |
| `eludite-dbg-netfx.exe` (this checkout, `x86_64-pc-windows-gnu`, release, no change) run as `wine eludite-dbg-netfx.exe --listen 127.0.0.1:47110`, a Python DAP client on Linux attaching to `Counter.exe`'s Wine pid | **Works unchanged.** `initialize`; `attach` to `initialized` in 51 ms then 48 ms on a second run (ICorDebug created in 20.5 ms, `DebugActiveProcess` 24 to 28 ms; brief 0004 measured 34 to 41 ms on Windows 11); `setBreakpoints` answers unverified ("No loaded module has symbols for this line yet") and the `breakpoint` event binds it to `0x06000002+IL_0005` 24 ms later, as on Windows; `stopped` (`breakpoint`, thread 336) 29 ms after `configurationDone`; `threads` lists two; `stackTrace` gives `Program.Tick` line 30 and `Program.Main` line 22 with the PDB's `Z:\...\Program.cs` path in 65 ms; `scopes` and `variables` read `counter` = 198 then, after `continue`, 200 (194 and 196 on the second run); callback to `stopped` event 0.63 ms inside the adapter; `disconnect` detaches and the debuggee keeps ticking. The adapter's stderr log is the one Windows prints. |
| A 47-line `System.Web.Hosting` host (`ApplicationHost.CreateApplicationHost`, `HttpListener`, `SimpleWorkerRequest`) serving a page with `<%@ Page %>`, a `Label`, a data-bound `Repeater` and a `Button` under Wine | `HTTP 200`. First request 1.83 s (the real page compiler runs the framework's `csc.exe` in the background and writes `Temporary ASP.NET Files`), then 6.1 and 4.7 ms. The page says `System.Web 4.0.0.0 on CLR 4.0.30319.42000, OS Microsoft Windows NT 6.1.7601`, with `__VIEWSTATE`, `__VIEWSTATEGENERATOR` and `__EVENTVALIDATION` exactly as IIS renders them. |
| The same host, same source, compiled with `mcs` and run under Mono 6.8 | `HTTP 200`. First request 0.80 s, then 4.2 ms. Mono's `System.Web` renders differently: no `__VIEWSTATEGENERATOR`, a different `action` form, a different ViewState encoding. Both are "WebForms", but only one is what the user's production IIS does. |

### 3.2 Verdict and what the spike did not do

- **The real framework runs under Wine 11 on Linux, and Eludite's own ICorDebug adapter debugs it there with no code change.** That removes the one reason ADR-0007 rated this path "expected to be fragile": the Win32 debug API Wine offers (`DebugActiveProcess`, debug events, `ReadProcessMemory`, thread contexts) is enough for `mscordbi`.
- Not run: stepping (`SetThreadContext` single-steps, brief 0004 had no `next`), exception events, `launch` (the adapter has attach only), 32-bit debuggees, a program with a window (WinForms or WPF under Wine's user32 and D3D), WCF (`System.ServiceModel` over `HttpListener` should behave as the host did), macOS. Brief W2 runs the adapter's full e2e suite under Wine and will find what Wine lacks.
- Not measured: the adapter's working set under Wine (Wine's processes do not report through `ps` the way Linux ones do), benchmarks with a cold page cache.
- Nothing in the repository was edited for the spike; the scripts, the host, the page, the adapter log, the DAP transcript and the rendered pages are in [`0003-run/`](0003-run/).

### 3.3 Reproduction

```
# Ubuntu 24.04, root or sudo. Wine from WineHQ (the distribution's 9.0 is too old for the new WoW64), winetricks from git.
dpkg --add-architecture i386 && apt-get install -y winehq-stable cabextract xvfb mingw-w64
curl -sSfL -o /usr/local/bin/winetricks https://raw.githubusercontent.com/Winetricks/winetricks/master/src/winetricks && chmod +x /usr/local/bin/winetricks
export WINEARCH=win64 WINEPREFIX=$PWD/prefix WINEDEBUG=-all
xvfb-run -a winetricks -q dotnet48            # fetches ndp48-x86-x64-allos-enu.exe from Microsoft; 7 minutes here
cargo build -p eludite-dbg-netfx --target x86_64-pc-windows-gnu --release
bash docs/proposals/0003-run/spike1-compile.sh  # Roslyn csc.exe from the Microsoft.Net.Compilers.Toolset nupkg, under Wine
bash docs/proposals/0003-run/spike2-run.sh      # cold and warm starts, the registry's version
bash docs/proposals/0003-run/spike3-debug.sh    # the adapter under Wine, driven by dap/client.py
bash docs/proposals/0003-run/spike4-web.sh      # the WebForms host on the real System.Web
```

## 4. Design

### 4.1 One runtime choice for .NET Framework targets

`crates/dap`'s classification (brief 0022) already separates CoreCLR from .NET Framework. For a .NET Framework target the shell now chooses a **framework runtime**:

| Runtime | When chosen (`dotnet.frameworkRuntime` = `auto`) | Run (Ctrl+F5) | Debug (F5) | Web project |
|---|---|---|---|---|
| `windows` | On Windows | `<exe>` | `eludite-dbg-netfx` | IIS Express when found, else `eludite-webhost` |
| `wine` | Off Windows, a Wine prefix with the real framework was found | `wine <exe>` in the prefix | `wine eludite-dbg-netfx.exe --listen 127.0.0.1:0` in the prefix, over TCP | `wine eludite-webhost.exe` |
| `mono` | Off Windows, Mono found and no such prefix (or the setting says so) | `mono <exe>` | `eludite-dbg-mono` (brief 0022) | `mono eludite-webhost.exe` |
| `remote` | The setting names a host, or nothing local was found and the user picks it from the message | the user's | `eludite-dbg-netfx` on the Windows host over TCP (ADR-0007) | the host's IIS Express or `eludite-webhost` |

`auto` prefers `wine` over `mono` because the real framework runs the user's program as it will run in production; the setting flips it per workspace. Nothing of Wine or Mono is touched before F5 (cold-start budget). `eludite.debug.state`'s `session.runtime` gains `wine` beside `coreclr`, `mono` and `netfx`; the status bar reads `eludite-dbg-netfx under wine-11.0 (.NET Framework 4.8.09032)`.

Discovery (`crates/dap/src/discovery.rs`, same shape as Mono's): the setting `dotnet.winePrefix` (`ELUDITE_WINE_PREFIX`), then `$WINEPREFIX`, then `~/.wine`, then `~/.local/share/eludite/wine` (the prefix the guided setup of 4.4 creates). A prefix counts when `drive_c/windows/Microsoft.NET/Framework64/v4.0.30319/clr.dll` exists and the registry's `HKLM\Software\Microsoft\NET Framework Setup\NDP\v4\Full\Release` is 528040 or higher (4.8), read from `system.reg` without starting Wine. `wine` itself: `dotnet.winePath`, then `wine` on `PATH`, then `/opt/wine-stable/bin/wine`, then CrossOver's `wine` under `/Applications/CrossOver.app` on macOS. Everything found is reported by the new read command `eludite.dotnet.framework_runtimes` and shown in Options > Debugging > .NET Framework.

### 4.2 Debugging: the same adapter, run as a Windows program

`eludite-dbg-netfx` is a DAP server over TCP that never assumed a local client (ADR-0007). Under Wine it is one: the shell runs `wine eludite-dbg-netfx.exe` in the prefix, reads `listening on 127.0.0.1:PORT` from its stderr and connects over TCP as it would to a remote Windows box. Nothing in the adapter changes; the spike ran the checkout's binary. Two things change around it:

- **Paths.** The adapter sees `Z:\home\user\src\App\Program.cs` where the shell has `/home/user/src/App/Program.cs`. The shell maps through the prefix's drive table (`dosdevices/`), the general remote-path-mapping rule of ADR-0007 applied locally. The PDB's document names are whatever the compiler wrote; the Wine-compiled fixture had `Z:\...`, a project built by Mono MSBuild or `dotnet build` has `/home/...`, and the adapter matches by file name today (brief 0004), so both already work; the mapping makes `source.path` in `stackTrace` answers exact.
- **Launch.** Brief 0004 did attach only. The real adapter's `launch` (0004 report, section 7) creates the debuggee with `CreateProcess`; under Wine that is the same call, so F5 is launch, Ctrl+F5 is `wine <exe>`.

Packaging: `tools/package/linux.sh` adds the `x86_64-pc-windows-gnu` build of `eludite-dbg-netfx.exe` beside `eludite` (1.1 MB, section 3). CI's Linux Rust job gains that target and, with `winehq-stable` and a cached prefix, runs the netfx e2e tests under Wine: the ICorDebug adapter gets Linux CI coverage it has never had.

### 4.3 `eludite-webhost`: the WebForms host IIS Express is on Windows

`web/host` (GPL-3.0-or-later, C#, `net472`, no dependencies beyond the framework) is a console program: `eludite-webhost --app <physical path> --port N [--vpath /]`. It creates the ASP.NET application domain with `ApplicationHost.CreateApplicationHost`, accepts connections with `HttpListener` on loopback, and hands each request to `HttpRuntime.ProcessRequest` through its own `HttpWorkerRequest` (headers, POST bodies, cookies, static files, `Global.asax`, `App_Code`, `web.config` handlers and modules, `Temporary ASP.NET Files` under the workspace's `.eludite/`). This is Cassini's design (Microsoft, Ms-PL, the "ASP.NET Development Server" of Visual Studio 2005 to 2010) and XSP's; the spike's 47-line version rendered a page with a `Label`, a `Repeater` and a `Button` through the real `System.Web` 4.0.0.0 under Wine (section 3). It runs on three runtimes without a code change: the real framework under Wine, Mono (`mono eludite-webhost.exe`), and Windows when IIS Express is not installed.

F5 on a web project (brief 0037's launch integration): the host starts under the chosen runtime, the debugger attaches to it (it is a normal .NET Framework process), the Web Browser window opens the page, and vscode-js-debug attaches to the tab (brief 0038): the full-stack breakpoint scenario of proposal 0002 brief D, for WebForms, on Linux. The host's stdout carries nothing but its ready line (`listening http://127.0.0.1:N/`); logs go to stderr (invariant 10).

What it is not: a production server (no `http.sys`, no app pools, no Windows authentication, no HTTPS in the first brief), and not IIS Express's `applicationhost.config` model; projects that need IIS Express's settings keep it on Windows.

### 4.4 Getting the framework into the prefix, without Eludite distributing it

Microsoft's license (section 7) forbids distributing the framework for non-Windows platforms, so Eludite never ships, downloads or caches `ndp48-*.exe`. The Output line when a .NET Framework project is run off Windows and no runtime is found:

> No .NET Framework runtime found. Install Mono (`sudo apt install mono-devel`) for most console and library projects, or install Wine and run `winetricks dotnet48` for Microsoft's framework (needs a Windows license; about 10 minutes), or debug on a Windows machine or VM (Debug > Attach to Remote). Options > Debugging > .NET Framework.

The Options page has "Prepare a Wine prefix…", a command (`eludite.dotnet.prepare_wine_prefix`, `dangerous` class, agents refused by default) that shows the license summary and a consent dialog, then runs the user's installed `winetricks dotnet48` into `~/.local/share/eludite/wine` with its output in the Output window. Eludite runs a tool the user installed, which fetches from Microsoft to the user's machine at the user's request; the owner decides whether even that is wanted (section 9, risks).

### 4.5 Building under the prefix

Brief 0003 settled builds off Windows: Mono's MSBuild 16.10 for evaluation and build, the .NET SDK's MSBuild as the fallback, Windows-only targets failing with a clear message. The prefix adds one option, named for a later brief and not promised: the framework's own `MSBuild.exe` 4.0 and `csc.exe` 5 under `Framework64\v4.0.30319` build `ToolsVersion` 4.0 projects with Microsoft's own `Microsoft.Common.targets`, but not web application projects (those targets come from Visual Studio) and not `PackageReference`. The Visual Studio Build Tools installer does not run under Wine. Builds stay as they are.

## 5. Command surface and schemas

- `eludite.dotnet.framework_runtimes` (read): `{ windows?: {version}, wine?: {prefix, wine, version, release}, mono?: {prefix, version}, remote?: {host, port}, chosen: "windows"|"wine"|"mono"|"remote"|null }`. Shown in Options and in `eludite/host/info`.
- `eludite.dotnet.prepare_wine_prefix` (dangerous): `{ prefix?: string }` → `{ prefix, log_path }`.
- `eludite.debug.start` is unchanged; its `debug-state.output.json` `session.runtime` enum gains `wine`.
- Settings (`settings.json`, section Debugging > .NET Framework): `dotnet.frameworkRuntime` (`auto`, `wine`, `mono`, `remote`), `dotnet.winePrefix`, `dotnet.winePath`, `dotnet.remoteDebugHost`, `dotnet.webHost` (`auto`, `iisexpress`, `eludite-webhost`), `dotnet.webHostPort` (0 = free port).
- `protocol/schemas/dap-netfx.md` gains "Under Wine": the path mapping rule and the launch arguments the shell sends.

Everything is a bus command with schemas first (invariant 3, 4); agents and the Options page call the same thing.

## 6. macOS

- **Intel Macs:** identical to Linux with Wine from Homebrew or WineHQ's macOS packages.
- **Apple Silicon:** the framework is x86 and x64 code (plus a native ARM64 build for Windows 11 ARM64 since 4.8.1). WineHQ has no native ARM64 macOS build; CrossOver 26 runs Wine 11 through Rosetta 2 today and its July 2026 preview adds a native ARM64 build with FEX for x86 code. Apple removes most of Rosetta 2 in macOS 28 (2027), keeping a subset for old games. So on Apple Silicon this proposal's path A means CrossOver (commercial, detected, never bundled) now and a native ARM64 Wine running the ARM64 framework later, and path B (a Windows 11 ARM64 guest in Virtualization.framework, UTM or Parallels, running .NET Framework 4.8.1 natively) is the recommended one. Eludite's part is the same remote DAP transport and path mapping either way.

## 7. Licensing

- **.NET Framework 4.8 and 4.8.1** (Microsoft Software Supplemental License Terms, read from the 4.8.1 text): "If you are licensed to use Microsoft Windows operating system software, you may use this supplement. You may not use it if you do not have a license for the software." Distribution restriction: "You may not distribute Distributable Code to run on a platform other than the Windows platform." Consequences: Eludite never redistributes or downloads the framework; the person installs it and is responsible for the Windows license; the consent dialog of 4.4 says both things. This is the same posture PLAN.md takes for Build Tools and IIS Express ("detected, not bundled").
- **Wine:** LGPL-2.1-or-later, located, not bundled (distributions ship it). **winetricks:** LGPL-2.1-or-later, the user's. **wine-mono:** MIT and LGPL, removed from the prefix by `dotnet48`. **Framework Mono:** MIT, LGPL and GPL mixture, located (brief 0003).
- **CrossOver:** commercial; detected on macOS like any other tool.
- **`eludite-webhost`:** GPL-3.0-or-later, written fresh against the public `System.Web.Hosting` API; Cassini (Ms-PL) is read for its design, not copied, because Ms-PL and GPL are incompatible.
- **Windows for path B:** the user's license or Microsoft's 90-day Enterprise evaluation; Eludite ships nothing of Windows.
- New Rust dependencies: none. New NuGet dependencies: none at run time; `Microsoft.Net.Compilers.Toolset` (MIT) only if a brief builds fixtures with it.

## 8. Briefs

| Brief | Content | Size (agent-weeks) | Depends on |
|---|---|---|---|
| W1. Framework runtime discovery and run | The runtime table of 4.1, Wine prefix and `wine` discovery, `eludite.dotnet.framework_runtimes`, the settings, Ctrl+F5 under Wine, `session.runtime` `wine`, the Output message, the Options page; `tools/wine/prefix.sh` (creates a prefix and runs the user's `winetricks dotnet48`, used by CI and by 4.4) | 1 | brief 0022 |
| W2. Debugging under Wine | `wine eludite-dbg-netfx.exe` as the F5 adapter, the drive-table path mapping, the `x86_64-pc-windows-gnu` build in `tools/package/linux.sh`, the netfx e2e tests under Wine in Linux CI with a cached prefix, launch once the real adapter has it | 1 | W1; the real `eludite-dbg-netfx` brief (0004 report section 7) for launch, attach works today |
| W3. `eludite-webhost` | `web/host`, the worker request (POST, cookies, static files, `Global.asax`, `App_Code`, handlers), the three runtimes, F5 on a web project opening the Web Browser window with the debugger attached, js-debug on the tab, the WebForms corpus projects of brief 0003 served and screenshotted | 1.5 | W1, briefs 0037 and 0038 |
| W4. Guided remote debugging | Debug > Attach to Remote with the host, port and path mapping dialog; `dotnet.remoteDebugHost`; the SSH-forwarded transport of ADR-0007; a written setup for a QEMU/KVM guest on Linux and a Windows 11 ARM64 guest on macOS with the adapter started at logon; the second-machine run brief 0004 still owes | 1 | the real `eludite-dbg-netfx` |
| W5. macOS runs | Intel Wine and Apple Silicon CrossOver runs of W1 to W3; a report with what differs; detection of CrossOver's `wine` | 0.5 | W1 to W3 |
| W6. Consent and prepare | `eludite.dotnet.prepare_wine_prefix` with the license dialog, only if the owner wants it (risks) | 0.5 | W1 |

Total: about 5.5 agent-weeks. W1 and W3 can start now; W2's attach half can start now, its launch half waits for the real adapter.

## 9. Later, named so they are not forgotten

- The framework's `MSBuild.exe` 4.0 in the prefix as a build engine for `ToolsVersion` 4.0 projects (4.5).
- A native ARM64 Wine on macOS running the ARM64 framework; CrossOver's preview is the signal to watch.
- Local VM lifecycle from the IDE (create, start, snapshot the guest) over path B; today the guest is the user's.
- `eludite-webhost` HTTPS, `applicationhost.config` reading for IIS Express parity, and the WCF test client hosting a service under the same pattern (`ServiceHost` in the host process).
- Running `eludite-dbg-netfx`'s unit tests under Wine on macOS CI.

## 10. Changes to PLAN.md on acceptance

- 3 (D7): the cross-platform list's item 3 becomes "The real framework under Wine, debugged by `eludite-dbg-netfx` run as a Windows program: the local path on Linux (proposal 0003)", and the order becomes Wine, remote, Mono.
- 4.5: the same list; `eludite-dbg-netfx` is "Windows-only by nature" becomes "needs a Windows user-mode environment: Windows, or Wine".
- 4.9: Launch gains `eludite-webhost` off Windows and when IIS Express is absent.
- 6: "Debug .NET Framework from Linux/macOS" becomes "Solvable with caveats: the real framework under Wine (x86-64; Apple Silicon through CrossOver or a VM), Mono as the fallback, remote to Windows for everything else"; "IIS Express" gains "or `eludite-webhost` everywhere"; the closing sentence becomes "edit, analyze, build anywhere; debug anywhere the real framework runs, which includes Linux".
- 10: Phase 3 gains W1 to W3 ("…remote-debugs from Linux" becomes "…runs and debugs from Linux under Wine"); Phase 5 loses "Mono and Wine debugging investigations".
- 12: `tools/wine/`, `web/host`.
- 14: decision 3 gains "Revised 2026-10 by proposal 0003: the real framework under Wine is the local path"; decision 14 points to ADR-0011.

## 11. Risks

- **The license.** Microsoft's terms tie the framework to a Windows license and forbid distributing it for other platforms. Eludite distributes nothing and the person installs it, which is what every Wine user does today, but the owner should decide whether Eludite even offers to run `winetricks` (W6) or only prints the command.
- **Wine's debug API.** The spike proves ICorDebug attach, breakpoints, stack and locals under Wine 11 (section 3). Stepping, exceptions, `SetThreadContext` single-steps and Edit and Continue are untested there; W2's e2e runs under Wine will find what Wine lacks, and a failure is reported to WineHQ, not patched around.
- **Startup cost.** The first Wine process in a prefix starts `wineserver` and `wine` costs a CLR start on an emulated loader; the spike's numbers are in section 3. Ctrl+F5 shows its own Output line so the person sees where the time goes; the budget is a run, not a frame.
- **32-bit projects.** `Prefer32Bit` and `x86` projects run as 32-bit processes; the 64-bit adapter cannot attach to them (ICorDebug attaches within one bitness) on Windows either. W2 ships both the x64 and the i686 adapter builds and picks by the debuggee's bitness, the same rule the Windows path needs.
- **Apple Silicon.** No upstream Wine; CrossOver is commercial; Rosetta ends in 2027. Path B is the honest recommendation there and this proposal says so in the product.
- **Mono's future.** WineHQ maintains it for Wine's needs; it is a fallback here, not a foundation.
- **Scope creep toward a Windows emulator.** The product runs programs under Wine; it does not configure, repair or explain Wine. Problems in the prefix are the user's and the Output window says what command to run.
