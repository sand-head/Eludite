//! Headless tests of brief 0029 against the fake adapter in lldb-dap's place: a folder with a Cargo workspace whose
//! startup project is a package (Set as Startup Project, persisted and bold) gets an `lldb` plan (adapter id, the
//! executable under `target/debug`, the formatters' `initCommands`, the workspace root as `cwd`, the run table's
//! arguments and environment) and `runtime` `native`; F5 builds through the Cargo build path first and launches only
//! when the build succeeds; the Rust panics row sends its function breakpoint; Ctrl+F5 runs the executable; `test`
//! builds the test executable with `cargo test --no-run` and debugs it with the filter; the settings reach the search.
//! The Cargo builds run a shell-script `cargo` (Unix); `cargo metadata` for the folder is the real one, offline.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eludite_commands::debug as cmds;
use eludite_commands::workspace;
use eludite_dap::cargo::STEP_AVOID;
use eludite_dap::fake::{self, FakeHandle, FakeProgram, FakeStep, FakeVar};
use eludite_docking::ids;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::documents::normalize_path;
use super::super::tests::{Ws, setup_debug};
use super::DebugSetup;
use super::native::NativeSetup;
use super::state::Mode;

const MAIN_RS: &str = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn main() {\n    let text = String::from(\"hello\");\n    let r = add(1, 2);\n    println!(\"{text} {r}\");\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn my_test() {\n        assert_eq!(super::add(1, 2), 3);\n    }\n}\n";

struct Nt {
    w: Ws,
    fake: Arc<Mutex<Option<FakeHandle>>>,
    /// The Cargo workspace's root (`<temp>/rs`).
    rs: PathBuf,
    store: tempfile::TempDir,
}

/// The test solution's folder with a Cargo workspace beside it in `rs/` (one member, `app`, with a binary and a run
/// table), the fake adapter in lldb-dap's place, a fake sysroot with the formatters, breakpoints persisting in a
/// temporary directory.
fn setup(cx: &mut TestAppContext) -> Nt {
    let store = tempfile::tempdir().unwrap();
    let fake: Arc<Mutex<Option<FakeHandle>>> = Arc::default();
    let rs: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let (f, r) = (fake.clone(), rs.clone());
    let setup = DebugSetup {
        connect: Some(Arc::new(move || {
            let root = r.lock().unwrap().clone().expect("the Cargo workspace");
            let main = normalize_path(&root.join("app/src/main.rs"))
                .to_string_lossy()
                .into_owned();
            let v = FakeVar::new;
            let program = FakeProgram {
                steps: vec![
                    FakeStep::new(
                        &main,
                        7,
                        "app::main",
                        0,
                        vec![v("text", "\"hello\"", "alloc::string::String")],
                    ),
                    FakeStep::new(
                        &main,
                        2,
                        "app::add",
                        1,
                        vec![v("a", "1", "int"), v("b", "2", "int")],
                    ),
                ],
                extra_capabilities: json!({"supportsFunctionBreakpoints": true,
                                           "supportsHitConditionalBreakpoints": true,
                                           "supportsLogPoints": true}),
                ..FakeProgram::default()
            };
            let (conn, handle) = fake::connect(program);
            *f.lock().unwrap() = Some(handle);
            Ok(conn)
        })),
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        platform: eludite_dap::launch::Platform::current(),
        store_dir: Some(store.path().to_path_buf()),
        dotnet: "dotnet".into(),
        js: Default::default(),
    };
    let w = setup_debug(cx, |_| {}, None, Some(setup));
    let root = w.path("rs");
    *rs.lock().unwrap() = Some(root.clone());
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"3\"\n",
    );
    write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[package.metadata.eludite.run]\nargs = [\"--fast\"]\nenv = { APP_MODE = \"dev\" }\n",
    );
    write("app/src/main.rs", MAIN_RS);
    // A sysroot with the formatters, so no `rustc --print sysroot` runs.
    let sysroot = w.path("sysroot");
    let etc = sysroot.join("lib/rustlib/etc");
    std::fs::create_dir_all(&etc).unwrap();
    std::fs::write(etc.join("lldb_lookup.py"), "").unwrap();
    let mut nt = Nt {
        w,
        fake,
        rs: root,
        store,
    };
    nt.w.shell.update(&mut nt.w.vcx, |s, _| {
        s.debug.native = NativeSetup {
            lldb: eludite_dap::discovery::LldbSearch::default(),
            formatters: true,
            sysroot: Some(sysroot),
        };
    });
    nt.set_build_before_run(false);
    nt
}

impl Nt {
    fn set_build_before_run(&mut self, on: bool) {
        self.w
            .commands
            .invoke(
                eludite_commands::settings::SET,
                json!({"key": "build.beforeRun", "value": on}),
            )
            .unwrap();
        self.w.wait("build before run", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.builds().build_before_run == on)
        });
    }

    /// File > Open Folder on `rs` (a folder with a Cargo workspace and no solution), until `cargo metadata` is read.
    fn open_folder(&mut self) {
        let rs = self.rs.to_string_lossy().into_owned();
        self.cmd(workspace::WORKSPACE_OPEN_FOLDER, json!({ "path": rs }))
            .unwrap();
        self.w.wait("the Cargo workspace", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.cargo_workspace().is_some())
        });
    }

    fn cmd(&mut self, command: &str, args: Value) -> Result<Value, eludite_commands::CommandError> {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
        r
    }

    fn state(&mut self) -> Value {
        self.cmd(cmds::STATE, json!({})).unwrap()
    }

    fn wait_mode(&mut self, mode: Mode) {
        self.w.wait(&format!("mode {mode:?}"), |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.debugger().model.mode == mode)
        });
    }

    fn fake(&self) -> FakeHandle {
        self.fake
            .lock()
            .unwrap()
            .clone()
            .expect("a session started")
    }

    fn manifest(&self) -> PathBuf {
        normalize_path(&self.rs.join("app/Cargo.toml"))
    }

    /// The binary cargo would have built: a script printing its arguments and environment.
    fn built_binary(&self) -> PathBuf {
        let exe = self
            .rs
            .join(format!("target/debug/app{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(
            &exe,
            "#!/bin/sh\necho \"app $* mode=$APP_MODE cwd=$(pwd)\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        exe
    }

    #[cfg(unix)]
    fn debug_output(&self) -> Vec<String> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(eludite_commands::build::OutputSource::Debug)
                .tail(usize::MAX)
        })
    }
}

/// A `cargo` that answers `build` (with `fail`, a compiler error) and `test --no-run` (naming a test executable) as
/// cargo's JSON does, recording its arguments.
#[cfg(unix)]
fn fake_cargo(nt: &Nt, fail: bool) -> PathBuf {
    let dir = nt.w.path("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = nt.manifest();
    let error = json!({"reason": "compiler-message", "manifest_path": manifest,
        "message": {"rendered": "error[E0308]: mismatched types", "level": "error", "message": "mismatched types",
                    "code": {"code": "E0308"}, "spans": []}});
    let test_exe = nt.rs.join("target/debug/deps/app-77b1");
    let artifact = json!({"reason": "compiler-artifact", "manifest_path": manifest,
        "target": {"kind": ["bin"], "name": "app"}, "profile": {"test": true}, "executable": test_exe});
    let log = dir.join("cargo.log");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> '{log}'\necho '   Compiling app v0.1.0' >&2\ncase \"$1\" in\n  test)\n    echo '{artifact}'\n    echo '{{\"reason\":\"build-finished\",\"success\":true}}'\n    ;;\n  *)\n    {build}\n    ;;\nesac\n",
        log = log.display(),
        artifact = artifact,
        build = if fail {
            format!(
                "echo '{error}'\n    echo '{{\"reason\":\"build-finished\",\"success\":false}}'\n    exit 101"
            )
        } else {
            "echo '{\"reason\":\"build-finished\",\"success\":true}'".to_owned()
        }
    );
    let path = dir.join(if fail { "cargo-fails" } else { "cargo" });
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[gpui::test]
fn a_cargo_package_is_the_startup_project_and_debugs_under_lldb(cx: &mut TestAppContext) {
    let mut nt = setup(cx);
    nt.open_folder();
    nt.built_binary();
    // Set as Startup Project on the package: kept as its Cargo.toml, drawn bold, marked in the tree.
    let out = nt
        .cmd(
            eludite_commands::project::SET_STARTUP_PROJECT,
            json!({"project": "app"}),
        )
        .unwrap();
    assert_eq!(out["project"], "app");
    assert_eq!(Path::new(out["path"].as_str().unwrap()), nt.manifest());
    let manifest = nt.manifest();
    nt.w.wait("the package drawn bold", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.explorer().read(cx).startup().map(Path::to_path_buf) == Some(manifest.clone())
        })
    });
    let tree = nt
        .cmd(eludite_commands::workspace_tree::WORKSPACE_TREE, json!({}))
        .unwrap();
    let app = tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "app")
        .unwrap()
        .clone();
    assert_eq!(
        (app["kind"].as_str(), app["startup"].as_bool()),
        (Some("cargo"), Some(true))
    );
    // It persists beside the Cargo workspace's manifest (the folder has no solution).
    let store = nt.store.path().to_path_buf();
    nt.w.wait("the startup project saved", |_| {
        std::fs::read_dir(store.join("solutions"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                std::fs::read_to_string(e.path())
                    .is_ok_and(|t| t.contains("\"startup_project\"") && t.contains("Cargo.toml"))
            })
    });

    // F5: the lldb plan.
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Running);
    let fake = nt.fake();
    let init = fake.last("initialize").unwrap();
    assert_eq!(init["adapterID"], "lldb");
    let launch = fake.last("launch").unwrap();
    eprintln!("launch: {launch}");
    assert_eq!(
        Path::new(launch["program"].as_str().unwrap()),
        nt.built_binary()
    );
    assert_eq!(Path::new(launch["cwd"].as_str().unwrap()), nt.rs);
    assert_eq!(launch["args"], json!(["--fast"]));
    assert_eq!(launch["env"], json!(["APP_MODE=dev"]));
    assert_eq!(launch["stopOnEntry"], false);
    let commands: Vec<&str> = launch["initCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(commands[0], STEP_AVOID);
    assert!(
        commands
            .last()
            .unwrap()
            .starts_with("command script import \"")
            && commands
                .last()
                .unwrap()
                .ends_with("lib/rustlib/etc/lldb_lookup.py\""),
        "{commands:?}"
    );
    // The Rust panics row (on by default) is a function breakpoint on rust_panic, before configurationDone.
    let requests = fake.commands();
    let fbp = requests
        .iter()
        .position(|c| c == "setFunctionBreakpoints")
        .expect("setFunctionBreakpoints");
    assert!(
        fbp < requests
            .iter()
            .position(|c| c == "configurationDone")
            .unwrap()
    );
    assert_eq!(
        fake.last("setFunctionBreakpoints").unwrap()["breakpoints"],
        json!([{"name": "rust_panic"}])
    );
    let s = nt.state();
    assert_eq!(s["session"]["runtime"], "native");
    assert!(
        s["session"]["adapter"]
            .as_str()
            .unwrap()
            .starts_with("lldb-dap ("),
        "{}",
        s["session"]["adapter"]
    );
    assert_eq!(
        Path::new(s["session"]["project"].as_str().unwrap()),
        nt.manifest()
    );
    assert_eq!(s["exceptions"]["break_on_rust_panic"], true);
    // lldb-dap's hit conditions are not Visual Studio's: the shell counts hits; log points are the adapter's.
    assert_eq!(s["capabilities"]["hit_conditions"], "shell");
    assert_eq!(s["capabilities"]["log_points"], "adapter");
    // A user's function breakpoint (brief 0026) goes in the same list, before rust_panic: the request replaces them
    // all, so neither may drop the other.
    nt.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"action": "set", "function": "app::run"}),
    )
    .unwrap();
    assert!(fake.wait_for("setFunctionBreakpoints", 2, super::super::tests::T));
    assert_eq!(
        fake.last("setFunctionBreakpoints").unwrap()["breakpoints"],
        json!([{"name": "app::run"}, {"name": "rust_panic"}])
    );
    nt.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"action": "delete", "function": "app::run"}),
    )
    .unwrap();
    assert!(fake.wait_for("setFunctionBreakpoints", 3, super::super::tests::T));
    assert_eq!(
        fake.last("setFunctionBreakpoints").unwrap()["breakpoints"],
        json!([{"name": "rust_panic"}])
    );

    // The row in the Exception Settings window turns it off: the function breakpoint goes.
    nt.w.commands
        .invoke("eludite.view.show", json!({"id": ids::EXCEPTION_SETTINGS}))
        .unwrap();
    nt.w.vcx.run_until_parked();
    nt.w.click("debug-exc-rust-panic");
    assert_eq!(nt.state()["exceptions"]["break_on_rust_panic"], false);
    assert!(fake.wait_for("setFunctionBreakpoints", 4, super::super::tests::T));
    assert_eq!(
        fake.last("setFunctionBreakpoints").unwrap()["breakpoints"],
        json!([])
    );
    nt.cmd(cmds::STOP, json!({})).unwrap();
    nt.wait_mode(Mode::Design);
    // The next session starts without it.
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Running);
    assert!(
        !nt.fake()
            .commands()
            .contains(&"setFunctionBreakpoints".to_owned())
    );
    nt.cmd(cmds::STOP, json!({})).unwrap();
    nt.wait_mode(Mode::Design);

    // An unknown binary target: the start ends naming the package's binaries.
    nt.cmd(cmds::START, json!({"target": "nope"})).unwrap();
    nt.wait_mode(Mode::Design);
    let message = nt.state()["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.contains("no binary target `nope`") && message.contains("app"),
        "{message}"
    );
}

#[gpui::test]
fn the_settings_reach_the_lldb_search_and_turn_the_formatters_off(cx: &mut TestAppContext) {
    let mut nt = setup(cx);
    let set = |nt: &mut Nt, key: &str, value: Value| {
        nt.w.commands
            .invoke(
                eludite_commands::settings::SET,
                json!({"key": key, "value": value}),
            )
            .unwrap();
    };
    set(
        &mut nt,
        "debugger.lldbDapPath",
        json!("/opt/llvm/bin/lldb-dap"),
    );
    set(&mut nt, "debugger.rustFormatters", json!(false));
    nt.w.wait("the settings applied", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            let n = &s.debugger().native;
            n.lldb.configured.as_deref() == Some(Path::new("/opt/llvm/bin/lldb-dap"))
                && !n.formatters
        })
    });
    // Without the formatters only the stepping filter is sent.
    nt.open_folder();
    nt.built_binary();
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Running);
    let launch = nt.fake().last("launch").unwrap();
    assert_eq!(launch["initCommands"], json!([STEP_AVOID]));
    nt.cmd(cmds::STOP, json!({})).unwrap();
    nt.wait_mode(Mode::Design);
    // Without an adapter (no fake), F5 says where it looked and what to install.
    nt.w.shell.update(&mut nt.w.vcx, |s, _| {
        s.debug.setup.connect = None;
        s.debug.native.lldb = eludite_dap::discovery::LldbSearch::default();
    });
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Design);
    let message = nt.state()["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    eprintln!("{message}");
    assert!(
        message.contains("lldb-dap was not found") && message.contains("tools/lldb-dap/README.md"),
        "{message}"
    );
}

/// Brief 0029 with brief 0020's build gate: F5 builds the package through the Cargo build path, launches when it
/// succeeds and not when it fails; Ctrl+F5 runs the executable; `test` debugs the test executable with a filter.
#[cfg(unix)]
#[gpui::test]
fn f5_builds_the_package_through_cargo_then_launches_and_ctrl_f5_runs_it(cx: &mut TestAppContext) {
    let mut nt = setup(cx);
    nt.open_folder();
    nt.built_binary();
    nt.set_build_before_run(true);
    let failing = fake_cargo(&nt, true);
    let cargo = fake_cargo(&nt, false);
    let log = nt.w.path("bin/cargo.log");
    // The setting build.cargoPath (what the build path and the test build both run).
    let use_cargo = |nt: &mut Nt, program: &Path| {
        nt.w.commands
            .invoke(
                eludite_commands::settings::SET,
                json!({"key": "build.cargoPath", "value": program.to_string_lossy()}),
            )
            .unwrap();
        let want = program.as_os_str().to_owned();
        nt.w.wait("build.cargoPath applied", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.builds.cargo_program == want)
        });
    };
    // A failing build: no launch, the reason in the message.
    use_cargo(&mut nt, &failing);
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Design);
    assert!(nt.w.audit().contains(&"eludite.build.project".to_owned()));
    let message = nt.state()["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        message.starts_with("Not started: the build failed (1 error"),
        "{message}"
    );
    assert!(
        nt.fake.lock().unwrap().is_none(),
        "no adapter after a failed build"
    );
    let built = std::fs::read_to_string(&log).unwrap();
    assert!(
        built.contains("build --message-format=json-diagnostic-rendered-ansi")
            && built.contains("-p app"),
        "{built}"
    );
    // A good build, then the launch.
    use_cargo(&mut nt, &cargo);
    nt.w.vcx.simulate_keystrokes("f5");
    nt.wait_mode(Mode::Running);
    assert_eq!(nt.fake().last("initialize").unwrap()["adapterID"], "lldb");
    nt.cmd(cmds::STOP, json!({})).unwrap();
    nt.wait_mode(Mode::Design);
    // Ctrl+F5: the executable runs in the workspace root with the run table's arguments and environment.
    nt.set_build_before_run(false);
    nt.w.vcx.simulate_keystrokes("ctrl-f5");
    nt.w.wait("the program's end", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().model.mode == Mode::Design && s.debugger().model.exit_code.is_some()
        })
    });
    let out = nt.debug_output();
    let rs = nt.rs.clone();
    // macOS's temporary folder is a symlink (`/var` to `/private/var`); the program prints its physical cwd.
    let real = std::fs::canonicalize(&rs).unwrap_or_else(|_| rs.clone());
    assert!(
        out.iter().any(|l| {
            l == &format!("app --fast mode=dev cwd={}", rs.display())
                || l == &format!("app --fast mode=dev cwd={}", real.display())
        }),
        "{out:?}"
    );
    // `test` with a filter: `cargo test --no-run` names the test executable, which runs with the filter and
    // --nocapture; cargo's lines reach the Debug source.
    nt.cmd(cmds::START, json!({"test": true, "args": ["my_test"]}))
        .unwrap();
    nt.wait_mode(Mode::Running);
    let launch = nt.fake().last("launch").unwrap();
    assert_eq!(
        Path::new(launch["program"].as_str().unwrap()),
        nt.rs.join("target/debug/deps/app-77b1")
    );
    assert_eq!(launch["args"], json!(["my_test", "--nocapture"]));
    let built = std::fs::read_to_string(&log).unwrap();
    assert!(built.contains("test --no-run"), "{built}");
    let out = nt.debug_output();
    assert!(out.iter().any(|l| l.contains("Compiling app")), "{out:?}");
    nt.cmd(cmds::STOP, json!({})).unwrap();
    nt.wait_mode(Mode::Design);
}
