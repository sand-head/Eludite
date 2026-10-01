//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: eludite [OPTIONS]

Options:
  --solution PATH     open this .sln, .slnx or project file at startup (as
                      File > Open > Project/Solution does) and use the window
                      layout saved for it
  --theme NAME        dark (default), light or blue
  --reset-layout      start from the default layout (the saved file is kept
                      until the layout changes)
  --no-persist        neither load nor save layouts
  -h, --help          print this help

Opening a solution starts eludite-host, found beside this executable, else
at ELUDITE_HOST (an eludite-host executable or eludite-host.dll), else on PATH.

Measurement harness (prints one JSON line to stdout, then exits):
  --bench-start       time from process start to the first presented frame;
                      set ELUDITE_LAUNCH_WALL_NS to the launch time (ns since
                      the Unix epoch) to include process creation
  --bench-drag N      drag the Output tab over the document area with the
                      docking guides visible for N frames; per-frame cost
  --exit-after-ms N   quit after N ms (smoke runs, screenshots)
  --open-file PATH    with --solution: open this file right after the solution
                      open command (eludite.file.open)
  --timings-out PATH  with --solution: write the times from the open command
                      to editable text, to the Solution Explorer tree and to
                      the first diagnostics as JSON to PATH (once all arrived,
                      or after 180 s); does not exit
  --bounds-out PATH   every 200 ms, write the window-relative bounds of tabs,
                      title bars, buttons, strips and guides to PATH as JSON
                      (for tools/drive.py, which drives the UI with real X11
                      pointer and key events)

Environment:
  ELUDITE_CONFIG_DIR  replaces <user config dir>/eludite (layouts go in its
                      layouts/ subdirectory)
";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub solution: Option<PathBuf>,
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
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut a = Args::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--solution" => a.solution = Some(value("--solution")?.into()),
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
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        Ok(a)
    }

    pub fn benching(&self) -> bool {
        self.bench_start || self.bench_drag.is_some()
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
    }
}
