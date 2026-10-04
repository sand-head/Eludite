# Proposal 0003 run: the real .NET Framework under Wine, 2026-10-04

The scripts and outputs of the spike in [proposal 0003](../0003-full-dotnet-framework-off-windows.md), section 3. Ubuntu 24.04 x86-64 container, Wine 11.0 (WineHQ `winehq-stable`), winetricks 20260125-next, Microsoft .NET Framework 4.8 (4.8.03761) installed by `winetricks dotnet48`, Mono 6.8.0.105 (Ubuntu) for the comparison.

| Path | What |
|---|---|
| `install.sh` | WineHQ packages, Xvfb, mingw-w64, winetricks from git |
| `dotnet48.sh` | The framework into a fresh 64-bit prefix under Xvfb |
| `env.sh` | The variables the spike scripts share |
| `spike1-compile.sh` | Roslyn `csc.exe` (NuGet `Microsoft.Net.Compilers.Toolset` 5.9.0, `tasks/net472`) under Wine: brief 0004's `Program.cs` and `webhost/Host.cs` |
| `spike2-run.sh` | Cold and warm starts of a console program on the real CLR; the registry's version |
| `spike3-debug.sh`, `dap/client.py` | `eludite-dbg-netfx.exe` (cross-compiled, `x86_64-pc-windows-gnu`) under Wine, attached to the running fixture by a DAP client on Linux |
| `spike4-web.sh`, `webhost/` | The 47-line `System.Web.Hosting` host and the page it served, under Wine and under Mono |
| `output/dotnet48-install.log` | The install's tail and timing |
| `output/counter-cold-start.txt` | The fixture's first lines on a cold `wineserver` |
| `output/debug-transcript.txt`, `output/adapter.err` | The DAP session (client side) and the adapter's own log |
| `output/page-wine.html`, `output/page-mono.html` | The same page rendered by Microsoft's `System.Web` under Wine and by Mono's |

Nothing here is part of the build. The scripts download Microsoft's installer through the user's `winetricks`, never through Eludite (proposal 0003 section 7).
