//! The swap: once the shell has quit, a copy of the new executable (the applier) moves the install folder's entries
//! to `.eludite-previous/`, moves the staged layout's entries in, and starts the new Eludite. Every step is a rename
//! on one filesystem; a failure undoes the renames made so far and starts the old Eludite instead. The plan is a JSON
//! file the shell writes and `eludite --apply-update PLAN` reads.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::build::{Build, Platform};
use crate::error::{Error, Result};
use crate::stage::{APPLY_DIR, Stage, Staged};

/// The flag the applier process runs under.
pub const APPLY_FLAG: &str = "--apply-update";
/// The flag the relaunched Eludite gets, with the build it replaced, so it can say so.
pub const UPDATED_FROM_FLAG: &str = "--updated-from";
/// The plan's file name in the stage's `apply/` folder.
pub const PLAN_FILE: &str = "plan.json";
/// The applier's log in the same folder.
pub const LOG_FILE: &str = "apply.log";
/// How long the applier waits for the shell to exit before swapping anyway.
pub const PARENT_EXIT_TIMEOUT: Duration = Duration::from_secs(120);

/// What to start after the swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relaunch {
    /// Arguments for the new `eludite` (the shell's own, with [`UPDATED_FROM_FLAG`]).
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// The applier's instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub install_dir: PathBuf,
    /// The staged layout to move in.
    pub layout_dir: PathBuf,
    /// The release folder of the stage (the layout and the archive), removed after a successful swap.
    pub release_dir: PathBuf,
    pub previous_dir: PathBuf,
    /// `eludite` or `eludite.exe`.
    pub executable: String,
    pub from: Option<Build>,
    pub to: Build,
    pub relaunch: Option<Relaunch>,
    pub log: PathBuf,
    /// Wait for the shell's stdin pipe to close before swapping (false only in tests, which run the applier in
    /// process).
    #[serde(default = "yes")]
    pub wait_for_shell: bool,
}

fn yes() -> bool {
    true
}

impl Plan {
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            serde_json::to_string_pretty(self).expect("a plan serializes"),
        )
    }

    pub fn read(path: &Path) -> std::result::Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The plan for `staged` in `stage`, relaunching with `args`.
pub fn plan(
    stage: &Stage,
    staged: &Staged,
    platform: &Platform,
    from: Option<Build>,
    relaunch: Option<Relaunch>,
) -> Plan {
    Plan {
        install_dir: stage.install_dir.clone(),
        layout_dir: staged.layout.clone(),
        release_dir: stage.dir_of(&staged.tag),
        previous_dir: stage.previous_dir(),
        executable: platform.executable().to_owned(),
        from,
        to: staged.build.clone(),
        relaunch,
        log: stage.root().join(APPLY_DIR).join(LOG_FILE),
        wait_for_shell: true,
    }
}

/// The applier process: a copy of the staged executable, started with its plan and a pipe on its stdin that closes
/// when the shell exits. Keep the returned child (or forget it) until the process exits: dropping it closes the pipe
/// early and the applier swaps while the shell still runs.
#[derive(Debug)]
pub struct Launched {
    pub child: Child,
    pub plan_path: PathBuf,
    pub applier: PathBuf,
}

/// Copy the staged executable to the stage's `apply/` folder, write the plan beside it and start it.
pub fn launch(stage: &Stage, plan: &Plan) -> Result<Launched> {
    let dir = stage.root().join(APPLY_DIR);
    std::fs::create_dir_all(&dir)?;
    let applier = dir.join(applier_name(&plan.executable));
    let source = plan.layout_dir.join(&plan.executable);
    std::fs::copy(&source, &applier)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&applier, std::fs::Permissions::from_mode(0o755))?;
    }
    let plan_path = dir.join(PLAN_FILE);
    plan.write(&plan_path)?;
    let child = Command::new(&applier)
        .arg(APPLY_FLAG)
        .arg(&plan_path)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Launched {
        child,
        plan_path,
        applier,
    })
}

/// `eludite-apply` or `eludite-apply.exe`.
pub fn applier_name(executable: &str) -> String {
    match executable.strip_suffix(".exe") {
        Some(stem) => format!("{stem}-apply.exe"),
        None => format!("{executable}-apply"),
    }
}

/// `eludite --apply-update PLAN`: wait for the shell to exit, swap, relaunch. The exit code: 0 when the new build
/// runs, 1 when the swap failed and the old build was started again, 2 when the plan is unusable.
pub fn run(plan_path: &Path) -> i32 {
    let plan = match Plan::read(plan_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("eludite {APPLY_FLAG}: {e}");
            return 2;
        }
    };
    let mut log = Log::open(&plan.log);
    log.line(&format!(
        "applying {} over {} in {}",
        plan.to.tag(),
        plan.from
            .as_ref()
            .map(Build::tag)
            .unwrap_or_else(|| "an unknown build".into()),
        plan.install_dir.display()
    ));
    if plan.wait_for_shell {
        wait_for_parent(PARENT_EXIT_TIMEOUT, &mut log);
    }
    match swap(
        &plan.install_dir,
        &plan.layout_dir,
        &plan.previous_dir,
        None,
    ) {
        Ok(moved) => {
            log.line(&format!("moved {} entries", moved.len()));
            let note = serde_json::json!({
                "from": plan.from, "to": plan.to, "at": crate::time::now_rfc3339(),
            });
            let _ = std::fs::write(
                plan.previous_dir.join("applied.json"),
                serde_json::to_string_pretty(&note).expect("serializes"),
            );
            let _ = std::fs::remove_file(
                plan.install_dir
                    .join(crate::stage::STAGE_DIR)
                    .join(crate::stage::STAGED_FILE),
            );
            let _ = std::fs::remove_dir_all(&plan.release_dir);
            match relaunch(&plan, true, &mut log) {
                Ok(()) => 0,
                Err(e) => {
                    log.line(&format!("the new build did not start: {e}"));
                    1
                }
            }
        }
        Err(e) => {
            log.line(&format!("{e}; the previous build is back in place"));
            if let Err(e) = relaunch(&plan, false, &mut log) {
                log.line(&format!("the previous build did not start either: {e}"));
            }
            1
        }
    }
}

/// Read stdin to its end (the shell's pipe closes when it exits), for at most `timeout`.
fn wait_for_parent(timeout: Duration, log: &mut Log) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut sink = [0u8; 1024];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        let _ = tx.send(());
    });
    match rx.recv_timeout(timeout) {
        Ok(()) => log.line("the shell has exited"),
        Err(_) => log.line(&format!(
            "the shell did not exit within {} s; swapping anyway",
            timeout.as_secs()
        )),
    }
}

/// Move `layout`'s entries into `install`, the ones they replace into `previous` (emptied first). Returns the entries
/// moved. On an error every rename made is undone. `fail_at` is the tests' fault injection: fail before moving the
/// entry of that name.
pub fn swap(
    install: &Path,
    layout: &Path,
    previous: &Path,
    fail_at: Option<&str>,
) -> Result<Vec<String>> {
    if previous.exists() {
        std::fs::remove_dir_all(previous).map_err(|e| {
            Error::Apply(format!("{} could not be emptied ({e})", previous.display()))
        })?;
    }
    std::fs::create_dir_all(previous)
        .map_err(|e| Error::Apply(format!("{} could not be created ({e})", previous.display())))?;
    let mut names: Vec<String> = std::fs::read_dir(layout)
        .map_err(|e| Error::Apply(format!("{} could not be read ({e})", layout.display())))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != crate::stage::STAGE_DIR && n != crate::stage::PREVIOUS_DIR)
        .collect();
    names.sort();
    // (name, had an old entry)
    let mut moved: Vec<(String, bool)> = Vec::new();
    let result = (|| -> Result<()> {
        for name in &names {
            if fail_at == Some(name.as_str()) {
                return Err(Error::Apply(format!("test fault at {name}")));
            }
            let old = install.join(name);
            let new = layout.join(name);
            let had_old = old.exists() || old.symlink_metadata().is_ok();
            if had_old {
                std::fs::rename(&old, previous.join(name)).map_err(|e| {
                    Error::Apply(format!("{} could not be moved aside ({e})", old.display()))
                })?;
            }
            if let Err(e) = std::fs::rename(&new, &old) {
                if had_old {
                    let _ = std::fs::rename(previous.join(name), &old);
                }
                return Err(Error::Apply(format!(
                    "{} could not be moved in ({e})",
                    new.display()
                )));
            }
            moved.push((name.clone(), had_old));
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(moved.into_iter().map(|(n, _)| n).collect()),
        Err(e) => {
            for (name, had_old) in moved.into_iter().rev() {
                let _ = std::fs::rename(install.join(&name), layout.join(&name));
                if had_old {
                    let _ = std::fs::rename(previous.join(&name), install.join(&name));
                }
            }
            Err(e)
        }
    }
}

/// Start the install folder's `eludite` with the plan's arguments (`new` adds [`UPDATED_FROM_FLAG`]).
fn relaunch(plan: &Plan, new: bool, log: &mut Log) -> std::io::Result<()> {
    let Some(relaunch) = &plan.relaunch else {
        log.line("nothing to relaunch");
        return Ok(());
    };
    let program = plan.install_dir.join(&plan.executable);
    let mut cmd = Command::new(&program);
    cmd.args(&relaunch.args);
    if new && let Some(from) = &plan.from {
        cmd.arg(UPDATED_FROM_FLAG).arg(from.tag());
    }
    cmd.current_dir(
        relaunch
            .cwd
            .clone()
            .unwrap_or_else(|| plan.install_dir.clone()),
    );
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = cmd.spawn()?;
    log.line(&format!(
        "started {} (pid {})",
        program.display(),
        child.id()
    ));
    Ok(())
}

/// The applier's log: appended lines with a timestamp, also on stderr.
struct Log(Option<std::fs::File>);

impl Log {
    fn open(path: &Path) -> Log {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Log(std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok())
    }

    fn line(&mut self, text: &str) {
        let line = format!("{} {text}\n", crate::time::now_rfc3339());
        eprint!("{line}");
        if let Some(f) = &mut self.0 {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &Path, files: &[(&str, &str)]) {
        for (name, text) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
    }

    fn read(root: &Path, name: &str) -> Option<String> {
        std::fs::read_to_string(root.join(name)).ok()
    }

    #[test]
    fn the_swap_moves_the_layout_in_and_the_old_files_aside() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("install");
        let layout = install.join(".eludite-update/unstable-2/layout");
        let previous = install.join(".eludite-previous");
        tree(
            &install,
            &[
                ("eludite", "old"),
                ("README", "old readme"),
                ("cef/libcef.so", "old cef"),
                ("notes.txt", "mine"),
            ],
        );
        tree(
            &layout,
            &[
                ("eludite", "new"),
                ("README", "new readme"),
                ("cef/libcef.so", "new cef"),
                ("build.json", "{}"),
            ],
        );
        let moved = swap(&install, &layout, &previous, None).unwrap();
        assert_eq!(moved, vec!["README", "build.json", "cef", "eludite"]);
        assert_eq!(read(&install, "eludite").as_deref(), Some("new"));
        assert_eq!(read(&install, "cef/libcef.so").as_deref(), Some("new cef"));
        assert_eq!(read(&install, "build.json").as_deref(), Some("{}"));
        // A file the layout does not carry stays.
        assert_eq!(read(&install, "notes.txt").as_deref(), Some("mine"));
        assert_eq!(read(&previous, "eludite").as_deref(), Some("old"));
        assert_eq!(read(&previous, "cef/libcef.so").as_deref(), Some("old cef"));
        assert!(std::fs::read_dir(&layout).unwrap().next().is_none());
    }

    #[test]
    fn a_failed_swap_puts_everything_back() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("install");
        let layout = install.join(".eludite-update/unstable-2/layout");
        let previous = install.join(".eludite-previous");
        tree(&install, &[("eludite", "old"), ("README", "old readme")]);
        tree(
            &layout,
            &[
                ("eludite", "new"),
                ("README", "new readme"),
                ("build.json", "{}"),
            ],
        );
        let err = swap(&install, &layout, &previous, Some("eludite")).unwrap_err();
        assert!(err.to_string().contains("test fault"), "{err}");
        assert_eq!(read(&install, "eludite").as_deref(), Some("old"));
        assert_eq!(read(&install, "README").as_deref(), Some("old readme"));
        assert!(!install.join("build.json").exists());
        assert_eq!(read(&layout, "README").as_deref(), Some("new readme"));
        assert_eq!(read(&layout, "build.json").as_deref(), Some("{}"));
    }

    #[test]
    fn plans_round_trip_and_name_the_applier() {
        let dir = tempfile::tempdir().unwrap();
        let p = Plan {
            install_dir: dir.path().to_owned(),
            layout_dir: dir.path().join("l"),
            release_dir: dir.path().join("r"),
            previous_dir: dir.path().join("p"),
            executable: "eludite".into(),
            from: None,
            to: Build {
                version: "0.1.0".into(),
                channel: crate::build::Channel::Unstable,
                build: crate::build::BuildId::parse("1").unwrap(),
                commit: None,
                os: "linux".into(),
                arch: "x86_64".into(),
                published: None,
            },
            relaunch: Some(Relaunch {
                args: vec!["--folder".into(), "/x".into()],
                cwd: None,
            }),
            log: dir.path().join("apply.log"),
            wait_for_shell: true,
        };
        let path = dir.path().join("apply/plan.json");
        p.write(&path).unwrap();
        assert_eq!(Plan::read(&path).unwrap(), p);
        assert_eq!(applier_name("eludite"), "eludite-apply");
        assert_eq!(applier_name("eludite.exe"), "eludite-apply.exe");
        assert_eq!(run(&dir.path().join("missing.json")), 2);
    }

    /// The whole applier in process: the swap, `applied.json`, the stage's bookkeeping, the log; no relaunch.
    #[test]
    fn the_applier_swaps_logs_and_cleans_the_stage() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("install");
        let stage = Stage::new(&install);
        let release = stage.dir_of("unstable-2");
        let layout = release.join("layout");
        tree(
            &install,
            &[("eludite", "old"), ("build.json", "{\"old\": true}")],
        );
        tree(
            &layout,
            &[("eludite", "new"), ("build.json", "{\"new\": true}")],
        );
        std::fs::write(
            release.join("eludite-0.1.0-linux-x86_64.tar.gz"),
            b"archive",
        )
        .unwrap();
        std::fs::write(stage.staged_file(), b"{}").unwrap();
        let to = Build {
            version: "0.1.0".into(),
            channel: crate::build::Channel::Unstable,
            build: crate::build::BuildId::parse("2").unwrap(),
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            published: None,
        };
        let from = Build {
            build: crate::build::BuildId::parse("1").unwrap(),
            ..to.clone()
        };
        let plan = Plan {
            install_dir: install.clone(),
            layout_dir: layout.clone(),
            release_dir: release.clone(),
            previous_dir: stage.previous_dir(),
            executable: "eludite".into(),
            from: Some(from),
            to,
            relaunch: None,
            log: stage.root().join(APPLY_DIR).join(LOG_FILE),
            wait_for_shell: false,
        };
        let path = stage.root().join(APPLY_DIR).join(PLAN_FILE);
        plan.write(&path).unwrap();
        assert_eq!(run(&path), 0);
        assert_eq!(read(&install, "eludite").as_deref(), Some("new"));
        assert_eq!(
            read(&install, "build.json").as_deref(),
            Some("{\"new\": true}")
        );
        assert_eq!(
            read(&stage.previous_dir(), "eludite").as_deref(),
            Some("old")
        );
        let applied: serde_json::Value =
            serde_json::from_str(&read(&stage.previous_dir(), "applied.json").unwrap()).unwrap();
        assert_eq!(applied["to"]["build"], "2");
        assert_eq!(applied["from"]["build"], "1");
        assert!(
            !release.exists(),
            "the release folder and its archive are gone"
        );
        assert!(!stage.staged_file().exists());
        let log = std::fs::read_to_string(&plan.log).unwrap();
        assert!(log.contains("applying unstable-2 over unstable-1"), "{log}");
        assert!(log.contains("moved 2 entries"), "{log}");
        assert!(log.contains("nothing to relaunch"), "{log}");
        // The next start removes the previous build.
        assert_eq!(
            stage.clean_after_start(),
            vec![stage.previous_dir(), stage.root().join(APPLY_DIR)]
        );
    }
}
