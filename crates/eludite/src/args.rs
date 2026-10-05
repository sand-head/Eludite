//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: eludite [OPTIONS]
       eludite --mcp-relay ADDR
       eludite --apply-update PLAN

Options:
  --solution PATH     open this .sln, .slnx or project file at startup (as
                      File > Open > Project/Solution does) and use the window
                      layout saved for it
  --folder PATH       open this folder (or the folder of this Cargo.toml) at
                      startup, as File > Open > Folder does: its .NET
                      solution and Cargo workspace side by side
  --theme NAME        dark (default), light or blue
  --reset-layout      start from the default layout (the saved file is kept
                      until the layout changes)
  --no-persist        neither load nor save layouts
  -h, --help          print this help

--mcp-relay ADDR is the stdio MCP server Eludite gives a hosted agent: it pipes
stdin and stdout to the IDE's MCP endpoint at ADDR (127.0.0.1:PORT), first
sending the token from ELUDITE_MCP_TOKEN, and opens no window.

Opening a solution starts eludite-host, found beside this executable, else
at ELUDITE_HOST (an eludite-host executable or eludite-host.dll), else on PATH.

Settings (Tools > Options) live in settings.json in the config directory
(~/.config/eludite on Linux, %APPDATA%\\eludite on Windows, ~/Library/
Application Support/eludite on macOS; ELUDITE_CONFIG_DIR replaces it) and in
.eludite/settings.json beside the solution, which wins. These environment
variables override them while set: ELUDITE_BUILD_ON_SAVE (build.onSave, 1 or
0), ELUDITE_CARGO (build.cargoPath), ELUDITE_NETCOREDBG
(debugger.netcoredbgPath), ELUDITE_MONO_PREFIX (debugger.monoPrefix),
ELUDITE_DBG_MONO (debugger.monoAdapterPath), ELUDITE_RUST_ANALYZER
(languageServers.rustAnalyzerPath), ELUDITE_CLAUDE_ACP
(agents.claudeCodeAdapterPath).

Measurement harness (prints one JSON line to stdout, then exits):
  --bench-start       time from process start to the first presented frame;
                      set ELUDITE_LAUNCH_WALL_NS to the launch time (ns since
                      the Unix epoch) to include process creation
  --bench-drag N      drag the Output tab over the document area with the
                      docking guides visible for N frames; per-frame cost
  --exit-after-ms N   quit after N ms (smoke runs, screenshots)
  --open-file PATH    open this file at startup (eludite.file.open), after the
                      solution when --solution is given
  --timings-out PATH  with --solution or --folder: write the times from the
                      open command
                      to editable text, to the Workspace tree and to
                      the first diagnostics of the opened file as JSON to PATH
                      (once all arrived, or after 180 s); does not exit
  --bench-type N      with --open-file: type N keys into the file (bursts of
                      25, 15 to 45 ms apart, 250 ms pauses so diagnostics
                      arrive while typing; with --solution, once it has
                      loaded) and report the keystroke frame cost; the file
                      is not saved
  --bench-complete N  with --solution and --open-file: once loaded, open a new
                      line after the first line containing `_sdkDiscoverer.`
                      (or ELUDITE_BENCH_COMPLETE_AFTER), type that word, then N
                      times: `.` (completion from the language server), two
                      filter keys, Escape and three Backspaces; report the
                      host and UI latency to the visible list, the keystroke
                      frame cost with the list open and resident memory
  --bench-navigate N  with --solution and --open-file: once loaded, N times F12
                      on the first `ISdkDiscoverer` in the file (or
                      ELUDITE_BENCH_DEFINITION) and Ctrl+- back, then N/4 (at
                      least 10) times Shift+F12 on `HostRpcTarget` (or
                      ELUDITE_BENCH_REFERENCES), then 300 keys with the Find
                      All References window holding 1000 rows; report host
                      and UI latency and the keystroke frame cost
  --bench-refactor N  with --solution and --open-file: once loaded, N times
                      move the caret between the first `ELUDITE_BENCH_BULB_A`
                      and `ELUDITE_BENCH_BULB_B` and wait for the light bulb;
                      300 keys with the light bulb active; N name changes in
                      the Rename dialog on `ELUDITE_BENCH_RENAME` (preview
                      latency); 20 applies of a no-op edit to 10 closed files
                      of the solution; report host and UI latency
  --agent NAME        select this agent of the Agents window's registry
  --transcript-out PATH
                      write the Agents window's transcript as JSON to PATH
                      whenever a turn ends
  --bench-agent-ready N
                      from the first frame, start the selected agent N times
                      (no prompt) and report spawn, initialize and
                      session/new times and window-open-to-ready
  --bench-agent-stream PATH
                      run the fake ACP agent at PATH (eludite-fake-acp-agent)
                      streaming 2000 chunks at 200/s into the Agents window;
                      report the UI frame work and the per-batch apply cost
  --bench-diff N      N times, hold a 20-edit change to a 2000-line file as a
                      pending change and open its review view; report the
                      time to the first frame showing the diff
  --bench-output SECS stream 10,000 lines a second for SECS seconds into the
                      Output window (as build output, 160-line chunks every
                      16 ms, the host's chunking) and report the frame cost
                      and the append cost per chunk (brief 0017)
  --bench-build N     with --solution: once loaded, N times press
                      Ctrl+Shift+B, wait for the build to finish; report key
                      to the first Output line on screen and the host's
                      finished notification to the Error List rows on screen
  --bench-debug N     with --solution and --open-file: once loaded, set a
                      breakpoint on the first line of the file containing
                      `ELUDITE_BENCH_BREAK` (default: Ping's first statement in
                      HostRpcTarget.cs) and run N debugging sessions of the
                      startup project (F5): each writes `ELUDITE_BENCH_PINGS`
                      (default 20) eludite/ping requests into the debuggee's
                      stdin (Linux), steps over twice at each break (F10) and
                      continues; then Shift+F5. Report F5 to the first break,
                      step round trips, frame cost while stepping, and the
                      Locals window drawing 200 variables
  --spike-browser URL open URL in the embedded browser (eludite-chromium, CEF;
                      brief 0031's spike) in a hidden document tab, Web
                      Browser; the engine is found beside this executable, at
                      ELUDITE_CHROMIUM or in the cargo target folder, CEF at
                      ELUDITE_CEF, CEF_PATH or tools/cef/fetch.sh's cache
  --bench-browser SECS
                      open an animation page (a 60 fps canvas filling the
                      view) in a 1600 by 1000 embedded browser tab, record
                      SECS seconds of frames (the RenderImage upload, the img
                      paint, the frame cost, the engine's paint to present),
                      then the engine's memory and tab_open to the first frame
                      with the engine running; the window opens at 2200 by
                      1500 so the whole tab shows. ELUDITE_BENCH_BROWSER_PAGE=box
                      plays a still page with a 200 by 200 animated box
                      instead; ELUDITE_BROWSER_TILES=0 uploads whole frames
                      instead of the 256 by 256 tiles the change touches
  --bounds-out PATH   every 200 ms, write the window-relative bounds of tabs,
                      title bars, buttons, strips and guides to PATH as JSON
                      (for tools/drive.py, which drives the UI with real X11
                      pointer and key events)

Updates (brief 0055): a packaged Eludite (build.json beside this executable)
checks GitHub's releases of its channel on Help > Check for Updates and, once
the person has said so (the setting updates.mode: ask, notify, download, off),
on its own about 15 s after the window opens and every 4 hours; a build is
downloaded to .eludite-update/ beside this executable, verified against the
release's SHA256SUMS, unpacked and swapped in when the person restarts.
  --apply-update PLAN run as the applier: wait for the shell to exit, swap the
                      install folder's files with the staged layout the plan
                      names, start the new eludite; no window
  --updated-from TAG  passed by the applier to the new eludite: say in the
                      Output window and the status bar what was replaced
  ELUDITE_UPDATE_API  the releases API to read instead of
                      https://api.github.com (a mirror, a test server)

Environment:
  ELUDITE_CONFIG_DIR  replaces <user config dir>/eludite (layouts go in its
                      layouts/ subdirectory)
";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub solution: Option<PathBuf>,
    /// `--folder PATH` (brief 0019).
    pub folder: Option<PathBuf>,
    pub theme: Option<String>,
    pub reset_layout: bool,
    pub no_persist: bool,
    pub help: bool,
    pub bench_start: bool,
    pub bench_drag: Option<usize>,
    pub exit_after_ms: Option<u64>,
    pub bounds_out: Option<PathBuf>,
    pub open_file: Option<PathBuf>,
    pub timings_out: Option<PathBuf>,
    pub bench_type: Option<usize>,
    pub bench_complete: Option<usize>,
    pub bench_navigate: Option<usize>,
    pub bench_refactor: Option<usize>,
    /// `--mcp-relay ADDR`: run as the agent's stdio MCP server, relaying to the IDE's endpoint.
    pub mcp_relay: Option<std::net::SocketAddr>,
    pub agent: Option<String>,
    pub transcript_out: Option<PathBuf>,
    pub bench_agent_ready: Option<usize>,
    pub bench_agent_stream: Option<PathBuf>,
    pub bench_diff: Option<usize>,
    /// `--bench-output SECS` (brief 0017).
    pub bench_output: Option<u64>,
    /// `--bench-build N` (brief 0017).
    pub bench_build: Option<usize>,
    /// `--bench-debug N` (brief 0018).
    pub bench_debug: Option<usize>,
    /// `--spike-browser URL` (brief 0031).
    pub spike_browser: Option<String>,
    /// `--bench-browser SECS` (brief 0031).
    pub bench_browser: Option<u64>,
    /// `--print-engine-discovery` (brief 0039, hidden: the package smoke test): print where the embedded engine and
    /// CEF are found, as JSON on stdout, and exit.
    pub print_engine_discovery: bool,
    /// `--apply-update PLAN` (brief 0055): run as the update applier and exit.
    pub apply_update: Option<PathBuf>,
    /// `--updated-from TAG` (brief 0055): this start follows an update from that build.
    pub updated_from: Option<String>,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut a = Args::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--solution" => a.solution = Some(value("--solution")?.into()),
                "--folder" => a.folder = Some(value("--folder")?.into()),
                "--theme" => {
                    let t = value("--theme")?;
                    if eludite_ui::Theme::by_name(&t).is_none() {
                        return Err(format!("unknown theme `{t}` (dark, light, blue)"));
                    }
                    a.theme = Some(t);
                }
                "--reset-layout" => a.reset_layout = true,
                "--no-persist" => a.no_persist = true,
                "-h" | "--help" => a.help = true,
                "--bench-start" => a.bench_start = true,
                "--bench-debug" => {
                    let n = value("--bench-debug")?;
                    a.bench_debug =
                        Some(n.parse().map_err(|_| format!("bad session count `{n}`"))?);
                }
                "--bench-drag" => {
                    let n = value("--bench-drag")?;
                    a.bench_drag = Some(n.parse().map_err(|_| format!("bad frame count `{n}`"))?);
                }
                "--exit-after-ms" => {
                    let n = value("--exit-after-ms")?;
                    a.exit_after_ms = Some(n.parse().map_err(|_| format!("bad duration `{n}`"))?);
                }
                "--bounds-out" => a.bounds_out = Some(value("--bounds-out")?.into()),
                "--open-file" => a.open_file = Some(value("--open-file")?.into()),
                "--timings-out" => a.timings_out = Some(value("--timings-out")?.into()),
                "--bench-type" => {
                    let n = value("--bench-type")?;
                    a.bench_type = Some(n.parse().map_err(|_| format!("bad key count `{n}`"))?);
                }
                "--bench-complete" => {
                    let n = value("--bench-complete")?;
                    a.bench_complete =
                        Some(n.parse().map_err(|_| format!("bad trigger count `{n}`"))?);
                }
                "--bench-navigate" => {
                    let n = value("--bench-navigate")?;
                    a.bench_navigate = Some(n.parse().map_err(|_| format!("bad run count `{n}`"))?);
                }
                "--bench-refactor" => {
                    let n = value("--bench-refactor")?;
                    a.bench_refactor = Some(n.parse().map_err(|_| format!("bad run count `{n}`"))?);
                }
                "--agent" => a.agent = Some(value("--agent")?),
                "--transcript-out" => a.transcript_out = Some(value("--transcript-out")?.into()),
                "--bench-agent-ready" => {
                    let n = value("--bench-agent-ready")?;
                    a.bench_agent_ready =
                        Some(n.parse().map_err(|_| format!("bad run count `{n}`"))?);
                }
                "--bench-agent-stream" => {
                    a.bench_agent_stream = Some(value("--bench-agent-stream")?.into())
                }
                "--bench-diff" => {
                    let n = value("--bench-diff")?;
                    a.bench_diff = Some(n.parse().map_err(|_| format!("bad run count `{n}`"))?);
                }
                "--bench-output" => {
                    let n = value("--bench-output")?;
                    a.bench_output = Some(n.parse().map_err(|_| format!("bad duration `{n}`"))?);
                }
                "--bench-build" => {
                    let n = value("--bench-build")?;
                    a.bench_build = Some(n.parse().map_err(|_| format!("bad run count `{n}`"))?);
                }
                "--spike-browser" => a.spike_browser = Some(value("--spike-browser")?),
                "--print-engine-discovery" => a.print_engine_discovery = true,
                "--apply-update" => a.apply_update = Some(value("--apply-update")?.into()),
                "--updated-from" => a.updated_from = Some(value("--updated-from")?),
                "--bench-browser" => {
                    let n = value("--bench-browser")?;
                    a.bench_browser = Some(n.parse().map_err(|_| format!("bad duration `{n}`"))?);
                }
                "--mcp-relay" => {
                    let addr = value("--mcp-relay")?;
                    a.mcp_relay = Some(
                        addr.parse()
                            .map_err(|_| format!("bad endpoint address `{addr}`"))?,
                    );
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(a)
    }

    /// Measurement runs: no saved layout, and no frame-rate limit when the compositor withholds focus (a nested
    /// session gives the window none, and GPUI would then draw at most every 33 ms).
    pub fn benching(&self) -> bool {
        self.bench_start
            || self.bench_drag.is_some()
            || self.bench_complete.is_some()
            || self.bench_navigate.is_some()
            || self.bench_refactor.is_some()
            || self.bench_agent_ready.is_some()
            || self.bench_agent_stream.is_some()
            || self.bench_diff.is_some()
            || self.bench_output.is_some()
            || self.bench_build.is_some()
            || self.bench_debug.is_some()
            || self.bench_browser.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &[&str]) -> Result<Args, String> {
        Args::parse(s.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses() {
        let a = parse(&[
            "--solution",
            "/w/Shop.sln",
            "--theme",
            "blue",
            "--bench-drag",
            "300",
        ])
        .unwrap();
        assert_eq!(
            a.solution.as_deref(),
            Some(std::path::Path::new("/w/Shop.sln"))
        );
        assert_eq!(a.theme.as_deref(), Some("blue"));
        assert_eq!(a.bench_drag, Some(300));
        assert!(a.benching());
        assert_eq!(parse(&[]).unwrap(), Args::default());
        assert!(parse(&["--theme", "pink"]).is_err());
        assert!(parse(&["--solution"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--bench-drag", "x"]).is_err());
        let a = parse(&[
            "--solution",
            "/w/App.slnx",
            "--open-file",
            "/w/A.cs",
            "--timings-out",
            "/tmp/t.json",
        ])
        .unwrap();
        assert_eq!(
            a.open_file.as_deref(),
            Some(std::path::Path::new("/w/A.cs"))
        );
        assert_eq!(
            a.timings_out.as_deref(),
            Some(std::path::Path::new("/tmp/t.json"))
        );
        assert!(parse(&["--open-file"]).is_err());
        assert_eq!(
            parse(&["--bench-complete", "200"]).unwrap().bench_complete,
            Some(200)
        );
        assert!(parse(&["--bench-complete", "x"]).is_err());
        let a = parse(&["--bench-navigate", "100"]).unwrap();
        assert_eq!(a.bench_navigate, Some(100));
        assert!(a.benching());
        assert!(parse(&["--bench-navigate", "x"]).is_err());
        let a = parse(&["--bench-refactor", "50"]).unwrap();
        assert_eq!(a.bench_refactor, Some(50));
        assert!(a.benching());
        assert!(parse(&["--bench-refactor"]).is_err());
        let a = parse(&["--mcp-relay", "127.0.0.1:4567"]).unwrap();
        assert_eq!(a.mcp_relay, Some("127.0.0.1:4567".parse().unwrap()));
        assert!(!a.benching());
        assert!(parse(&["--mcp-relay", "nowhere"]).is_err());
        let a = parse(&[
            "--agent",
            "Claude Code",
            "--transcript-out",
            "/tmp/t.json",
            "--bench-diff",
            "20",
        ])
        .unwrap();
        assert_eq!(a.agent.as_deref(), Some("Claude Code"));
        assert_eq!(a.bench_diff, Some(20));
        assert!(a.benching());
        assert!(parse(&["--bench-agent-ready", "x"]).is_err());
        assert!(parse(&["--bench-agent-stream", "/f"]).unwrap().benching());
        let a = parse(&["--bench-output", "10"]).unwrap();
        assert_eq!(a.bench_output, Some(10));
        assert!(a.benching());
        assert_eq!(parse(&["--bench-build", "5"]).unwrap().bench_build, Some(5));
        assert!(parse(&["--bench-build", "x"]).is_err());
        let a = parse(&["--bench-browser", "20"]).unwrap();
        assert_eq!(a.bench_browser, Some(20));
        assert!(a.benching());
        assert!(parse(&["--bench-browser", "x"]).is_err());
        // Brief 0039's hidden flag for the package smoke test; not in the usage text.
        assert!(
            parse(&["--print-engine-discovery"])
                .unwrap()
                .print_engine_discovery
        );
        assert!(!USAGE.contains("--print-engine-discovery"));
        let a = parse(&["--spike-browser", "https://example.com"]).unwrap();
        assert_eq!(a.spike_browser.as_deref(), Some("https://example.com"));
        assert!(!a.benching());
    }
}
