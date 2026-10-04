//! The sandbox rule (brief 0039, replacing brief 0031's): how Chromium's sandbox runs, decided before CEF starts.
//!
//! On Linux Chromium sandboxes its renderers with unprivileged user namespaces when the kernel allows them, and
//! otherwise with the setuid `chrome-sandbox` helper; with neither it cannot sandbox at all, and it never sandboxes
//! root. The engine follows the same order and refuses where Chromium would have to run unsandboxed:
//!
//! | Probes | `--allow-no-sandbox` absent | `--allow-no-sandbox` present |
//! |---|---|---|
//! | `--no-sandbox` on the command line | refused | `none` |
//! | running as root | refused | `none` |
//! | user namespaces available | `namespaces` | `namespaces` |
//! | no namespaces, a usable setuid-root helper | `helper` | `helper` |
//! | no namespaces, helper missing, not setuid root, or unusable | refused | `none` |
//!
//! A refusal ([`Refusal`]) names both remedies (allowing user namespaces, or `sudo chown root:root chrome-sandbox &&
//! sudo chmod 4755 chrome-sandbox`) and the opt-in; the engine prints it and exits with [`REFUSED_EXIT`], which the
//! shell answers with its opt-in dialog. `--allow-no-sandbox` is the only way to `--no-sandbox`: the shell passes it
//! only for the workspace's opt-in (`browser.allowNoSandbox`) or `ELUDITE_CHROME_NO_SANDBOX=1` in its own environment.
//!
//! The probes ([`Probes::probe`]) are real system calls; [`decide`] is a pure function of them, so the table is tested
//! with the probes faked.

// The probes are system calls (fork, unshare, waitpid, geteuid, getuid); each use is commented.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

/// The engine's switch that permits `--no-sandbox` (and the only one).
pub const ALLOW_NO_SANDBOX: &str = "--allow-no-sandbox";
/// Chromium's switch, refused without [`ALLOW_NO_SANDBOX`].
pub const NO_SANDBOX: &str = "--no-sandbox";
/// brief 0023's variable: the shell turns it into [`ALLOW_NO_SANDBOX`]; the engine itself does not read it.
pub const NO_SANDBOX_ENV: &str = "ELUDITE_CHROME_NO_SANDBOX";
/// The engine's exit code when it refuses to start without the sandbox (before it reads stdin).
pub const REFUSED_EXIT: u8 = 5;
/// The helper's file name.
pub const HELPER: &str = "chrome-sandbox";

/// What the helper probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Helper {
    /// No `chrome-sandbox` beside the engine or beside `libcef.so`.
    Missing { path: PathBuf },
    /// Present, but not owned by root with the setuid bit (a copy, as cargo's build and a tarball leave it).
    NotSetuidRoot { path: PathBuf },
    /// Setuid root beside `libcef.so` only, while the engine belongs to another user: Chromium reads
    /// `CHROME_DEVEL_SANDBOX` only for an executable owned by the user running it, and otherwise looks for the helper
    /// beside itself alone.
    Unusable {
        path: PathBuf,
        beside_engine: PathBuf,
    },
    /// Owned by root with mode 4755 where Chromium will use it.
    SetuidRoot { path: PathBuf },
}

impl Helper {
    /// The helper's path (the one a remedy names).
    pub fn path(&self) -> &Path {
        match self {
            Helper::Missing { path }
            | Helper::NotSetuidRoot { path }
            | Helper::Unusable { path, .. }
            | Helper::SetuidRoot { path } => path,
        }
    }
}

/// What the engine found before deciding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probes {
    /// The effective user is root.
    pub root: bool,
    /// Unprivileged user namespaces work ([`namespaces_available`]).
    pub namespaces: bool,
    pub helper: Helper,
}

/// How the engine runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Sandboxed through unprivileged user namespaces.
    Namespaces,
    /// Sandboxed through this setuid helper (`CHROME_DEVEL_SANDBOX`).
    Helper(PathBuf),
    /// `--no-sandbox`, permitted by `--allow-no-sandbox`.
    NoSandbox,
}

impl Decision {
    /// `engine/ready`'s `sandbox`: `namespaces`, `helper` or `none`.
    pub fn mode(&self) -> &'static str {
        match self {
            Decision::Namespaces => "namespaces",
            Decision::Helper(_) => "helper",
            Decision::NoSandbox => "none",
        }
    }
}

/// Why the engine refuses, as the line it prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the engine's command line asks: `--allow-no-sandbox`, and Chromium's own `--no-sandbox`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Asked {
    pub allow_no_sandbox: bool,
    pub no_sandbox: bool,
}

impl Asked {
    /// From the engine's arguments (CEF reads the same command line, so `--no-sandbox` anywhere counts).
    pub fn from_args(args: &[String]) -> Asked {
        let has = |name: &str| {
            args.iter()
                .any(|a| a == name || a.split_once('=').is_some_and(|(n, _)| n == name))
        };
        Asked {
            allow_no_sandbox: has(ALLOW_NO_SANDBOX),
            no_sandbox: has(NO_SANDBOX),
        }
    }
}

const OPT_IN: &str = "or let this workspace run the browser without the sandbox (the Web Browser window offers it; \
    the setting browser.allowNoSandbox; ELUDITE_CHROME_NO_SANDBOX=1 for tests), which starts the engine with \
    --allow-no-sandbox";

/// The remedy that installs the helper at `p`.
pub fn helper_command(p: &Path) -> String {
    format!(
        "sudo chown root:root {p} && sudo chmod 4755 {p}",
        p = p.display()
    )
}

/// The decision table (module documentation).
pub fn decide(probes: &Probes, asked: Asked) -> Result<Decision, Refusal> {
    if asked.no_sandbox {
        return if asked.allow_no_sandbox {
            Ok(Decision::NoSandbox)
        } else {
            Err(Refusal(format!(
                "refusing {NO_SANDBOX}: the engine drops Chromium's sandbox only with {ALLOW_NO_SANDBOX}, which \
                 Eludite passes after the workspace's opt-in (browser.allowNoSandbox) or with {NO_SANDBOX_ENV}=1"
            )))
        };
    }
    if probes.root {
        return if asked.allow_no_sandbox {
            Ok(Decision::NoSandbox)
        } else {
            Err(Refusal(format!(
                "Chromium's sandbox cannot start on this machine: Eludite runs as root, and Chromium does not \
                 sandbox root. Run Eludite as a normal user, {OPT_IN}."
            )))
        };
    }
    if probes.namespaces {
        return Ok(Decision::Namespaces);
    }
    if let Helper::SetuidRoot { path } = &probes.helper {
        return Ok(Decision::Helper(path.clone()));
    }
    if asked.allow_no_sandbox {
        return Ok(Decision::NoSandbox);
    }
    let p = probes.helper.path();
    let (state, install) = match &probes.helper {
        Helper::Missing { .. } => ("is missing", p.to_path_buf()),
        Helper::NotSetuidRoot { .. } => (
            "is not owned by root with the setuid bit (mode 4755)",
            p.to_path_buf(),
        ),
        Helper::Unusable { beside_engine, .. } => (
            "is setuid root, but Chromium uses a helper outside its own folder only when the engine belongs to the \
             user running it; put the helper beside the engine",
            beside_engine.clone(),
        ),
        Helper::SetuidRoot { .. } => unreachable!("decided above"),
    };
    Err(Refusal(format!(
        "Chromium's sandbox cannot start on this machine: unprivileged user namespaces are not available, and the \
         setuid helper {} {state}. Either allow user namespaces (`sudo sysctl -w kernel.unprivileged_userns_clone=1`, \
         or on Ubuntu 23.10 and later an AppArmor profile that permits them for eludite-chromium), or install the \
         helper with `{}`, {OPT_IN}.",
        p.display(),
        helper_command(&install),
    )))
}

/// Whether the two sysctls let unprivileged users create user namespaces: `kernel.unprivileged_userns_clone`
/// (Debian's patch; absent elsewhere) absent or 1, and `user.max_user_namespaces` above 0.
pub fn sysctls_allow_namespaces(
    unprivileged_userns_clone: Option<&str>,
    max_user_namespaces: Option<&str>,
) -> bool {
    let clone_ok = unprivileged_userns_clone.is_none_or(|v| v.trim() == "1");
    let max_ok = max_user_namespaces
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|n| n > 0);
    clone_ok && max_ok
}

/// Whether unprivileged user namespaces work: the sysctls allow them, and a forked child can `unshare` a user
/// namespace together with the PID and network namespaces Chromium's zygote asks for (an AppArmor restriction, as on
/// Ubuntu 23.10 and later, lets the user namespace be created but takes its capabilities, so the second part fails).
#[cfg(target_os = "linux")]
pub fn namespaces_available() -> bool {
    let read = |p: &str| std::fs::read_to_string(p).ok();
    if !sysctls_allow_namespaces(
        read("/proc/sys/kernel/unprivileged_userns_clone").as_deref(),
        read("/proc/sys/user/max_user_namespaces").as_deref(),
    ) {
        return false;
    }
    // SAFETY: the child calls only unshare and _exit (async-signal-safe) and never returns into Rust; the parent
    // reaps it.
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            let r = libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNET);
            libc::_exit(if r == 0 { 0 } else { 1 });
        }
        if pid < 0 {
            return false;
        }
        let mut status = 0;
        if libc::waitpid(pid, &mut status, 0) != pid {
            return false;
        }
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
    }
}

#[cfg(not(target_os = "linux"))]
pub fn namespaces_available() -> bool {
    false
}

/// The helper for the engine at `engine` with CEF in `cef_dir`: beside the engine first (Chromium always looks
/// there), then beside `libcef.so` (used through `CHROME_DEVEL_SANDBOX`, which Chromium honors only for an engine owned
/// by the user running it).
pub fn probe_helper(engine: &Path, cef_dir: &Path) -> Helper {
    let engine_dir = engine.parent().unwrap_or(Path::new("."));
    let beside_engine = engine_dir.join(HELPER);
    let beside_cef = cef_dir.join(HELPER);
    let mut first_present = None;
    for p in [&beside_engine, &beside_cef] {
        match setuid_root(p) {
            Some(true) => {
                return if *p == beside_engine || owned_by_me(engine) {
                    Helper::SetuidRoot { path: p.clone() }
                } else {
                    Helper::Unusable {
                        path: p.clone(),
                        beside_engine: beside_engine.clone(),
                    }
                };
            }
            Some(false) => {
                first_present.get_or_insert_with(|| p.clone());
            }
            None => {}
        }
    }
    match first_present {
        Some(path) => Helper::NotSetuidRoot { path },
        None => Helper::Missing { path: beside_cef },
    }
}

impl Probes {
    /// The real probes, for the engine at `engine` with CEF in `cef_dir`.
    pub fn probe(engine: &Path, cef_dir: &Path) -> Probes {
        let root = is_root();
        Probes {
            root,
            // Root is decided before namespaces; do not fork for nothing.
            namespaces: !root && namespaces_available(),
            helper: probe_helper(engine, cef_dir),
        }
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[cfg(unix)]
fn setuid_root(p: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    Some(m.uid() == 0 && m.mode() & 0o4000 != 0)
}

#[cfg(not(unix))]
fn setuid_root(_: &Path) -> Option<bool> {
    None
}

#[cfg(unix)]
fn owned_by_me(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions.
    let me = unsafe { libc::getuid() };
    std::fs::metadata(p).is_ok_and(|m| m.uid() == me)
}

#[cfg(not(unix))]
fn owned_by_me(_: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probes(root: bool, namespaces: bool, helper: Helper) -> Probes {
        Probes {
            root,
            namespaces,
            helper,
        }
    }

    fn missing() -> Helper {
        Helper::Missing {
            path: "/e/cef/chrome-sandbox".into(),
        }
    }

    fn setuid() -> Helper {
        Helper::SetuidRoot {
            path: "/e/cef/chrome-sandbox".into(),
        }
    }

    fn plain() -> Helper {
        Helper::NotSetuidRoot {
            path: "/e/cef/chrome-sandbox".into(),
        }
    }

    const NOTHING: Asked = Asked {
        allow_no_sandbox: false,
        no_sandbox: false,
    };
    const ALLOWED: Asked = Asked {
        allow_no_sandbox: true,
        no_sandbox: false,
    };

    /// The decision table of the module documentation, row by row, with the probes faked.
    #[test]
    fn the_decision_table() {
        // Namespaces: sandboxed whatever the helper, with or without the opt-in.
        for helper in [missing(), plain(), setuid()] {
            for asked in [NOTHING, ALLOWED] {
                assert_eq!(
                    decide(&probes(false, true, helper.clone()), asked),
                    Ok(Decision::Namespaces)
                );
            }
        }
        // No namespaces, the helper setuid root: sandboxed through it.
        for asked in [NOTHING, ALLOWED] {
            assert_eq!(
                decide(&probes(false, false, setuid()), asked),
                Ok(Decision::Helper("/e/cef/chrome-sandbox".into()))
            );
        }
        // The helper present but not setuid root: refused, naming the command that installs it, unless opted in.
        let e = decide(&probes(false, false, plain()), NOTHING)
            .unwrap_err()
            .0;
        assert!(e.contains("not owned by root with the setuid bit"), "{e}");
        assert!(
            e.contains(
                "sudo chown root:root /e/cef/chrome-sandbox && sudo chmod 4755 /e/cef/chrome-sandbox"
            ),
            "{e}"
        );
        assert!(e.contains("user namespaces"), "{e}");
        assert!(e.contains("browser.allowNoSandbox"), "{e}");
        assert_eq!(
            decide(&probes(false, false, plain()), ALLOWED),
            Ok(Decision::NoSandbox)
        );
        // Neither: refused with both remedies, unless opted in.
        let e = decide(&probes(false, false, missing()), NOTHING)
            .unwrap_err()
            .0;
        assert!(
            e.contains("is missing") && e.contains("sudo chmod 4755"),
            "{e}"
        );
        assert!(e.contains("kernel.unprivileged_userns_clone"), "{e}");
        assert_eq!(
            decide(&probes(false, false, missing()), ALLOWED),
            Ok(Decision::NoSandbox)
        );
        // A setuid helper Chromium would not use: refused, naming where it must go.
        let unusable = Helper::Unusable {
            path: "/opt/e/cef/chrome-sandbox".into(),
            beside_engine: "/opt/e/chrome-sandbox".into(),
        };
        let e = decide(&probes(false, false, unusable), NOTHING)
            .unwrap_err()
            .0;
        assert!(
            e.contains("sudo chown root:root /opt/e/chrome-sandbox"),
            "{e}"
        );
        // Root: refused whatever else is there, unless opted in (brief 0031's finding: Chromium refuses root).
        for namespaces in [false, true] {
            for helper in [missing(), plain(), setuid()] {
                let e = decide(&probes(true, namespaces, helper.clone()), NOTHING)
                    .unwrap_err()
                    .0;
                assert!(
                    e.contains("root") && e.contains("browser.allowNoSandbox"),
                    "{e}"
                );
                assert_eq!(
                    decide(&probes(true, namespaces, helper), ALLOWED),
                    Ok(Decision::NoSandbox)
                );
            }
        }
    }

    /// `--no-sandbox` never runs without `--allow-no-sandbox`, from the command line or the decision.
    #[test]
    fn no_sandbox_needs_the_allow_switch() {
        let asked = Asked::from_args(&["eludite-chromium".into(), "--no-sandbox".into()]);
        assert_eq!(
            asked,
            Asked {
                allow_no_sandbox: false,
                no_sandbox: true
            }
        );
        for p in [
            probes(false, true, setuid()),
            probes(false, false, missing()),
            probes(true, false, missing()),
        ] {
            let e = decide(&p, asked).unwrap_err().0;
            assert!(e.contains(ALLOW_NO_SANDBOX), "{e}");
            assert_eq!(
                decide(
                    &p,
                    Asked {
                        allow_no_sandbox: true,
                        no_sandbox: true
                    }
                ),
                Ok(Decision::NoSandbox)
            );
        }
        let both = Asked::from_args(&[
            "--profile=/p".into(),
            "--allow-no-sandbox".into(),
            "--no-sandbox=1".into(),
        ]);
        assert!(both.allow_no_sandbox && both.no_sandbox);
        assert_eq!(Asked::from_args(&["--no-sandboxes".into()]), NOTHING);
        // Exhaustively: NoSandbox comes out only when allowed.
        for root in [false, true] {
            for namespaces in [false, true] {
                for helper in [missing(), plain(), setuid()] {
                    for no_sandbox in [false, true] {
                        let d = decide(
                            &probes(root, namespaces, helper.clone()),
                            Asked {
                                allow_no_sandbox: false,
                                no_sandbox,
                            },
                        );
                        assert_ne!(d, Ok(Decision::NoSandbox));
                    }
                }
            }
        }
        assert_eq!(Decision::Namespaces.mode(), "namespaces");
        assert_eq!(Decision::Helper("/h".into()).mode(), "helper");
        assert_eq!(Decision::NoSandbox.mode(), "none");
    }

    #[test]
    fn the_sysctls() {
        assert!(sysctls_allow_namespaces(None, Some("64301\n")));
        assert!(sysctls_allow_namespaces(Some("1\n"), Some("10")));
        assert!(!sysctls_allow_namespaces(Some("0\n"), Some("64301")));
        assert!(!sysctls_allow_namespaces(None, Some("0")));
        assert!(!sysctls_allow_namespaces(None, None));
    }

    /// The helper probe on real files: missing, a plain copy, and (as root) setuid root beside the engine or beside
    /// CEF with the engine owned by us.
    #[cfg(unix)]
    #[test]
    fn the_helper_probe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let cef = dir.path().join("cef");
        std::fs::create_dir_all(&cef).unwrap();
        let engine = dir.path().join("eludite-chromium");
        std::fs::write(&engine, b"").unwrap();
        assert_eq!(
            probe_helper(&engine, &cef),
            Helper::Missing {
                path: cef.join(HELPER)
            }
        );
        std::fs::write(cef.join(HELPER), b"").unwrap();
        assert_eq!(
            probe_helper(&engine, &cef),
            Helper::NotSetuidRoot {
                path: cef.join(HELPER)
            }
        );
        if is_root() {
            std::fs::set_permissions(cef.join(HELPER), std::fs::Permissions::from_mode(0o4755))
                .unwrap();
            assert_eq!(
                probe_helper(&engine, &cef),
                Helper::SetuidRoot {
                    path: cef.join(HELPER)
                },
                "beside libcef.so, the engine ours"
            );
            std::fs::write(dir.path().join(HELPER), b"").unwrap();
            std::fs::set_permissions(
                dir.path().join(HELPER),
                std::fs::Permissions::from_mode(0o4755),
            )
            .unwrap();
            assert_eq!(
                probe_helper(&engine, &cef),
                Helper::SetuidRoot {
                    path: dir.path().join(HELPER)
                },
                "beside the engine first"
            );
        } else {
            eprintln!("not root: the setuid rows of the helper probe need root to chmod 4755");
        }
    }

    /// What this machine says (recorded in brief 0039's report); never fails.
    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine() {
        let t = std::time::Instant::now();
        let ns = namespaces_available();
        eprintln!(
            "root {}, user namespaces {ns} (probed in {:?})",
            is_root(),
            t.elapsed()
        );
    }
}
