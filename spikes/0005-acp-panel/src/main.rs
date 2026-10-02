//! `spike-acp-panel`: brief 0005 prototype.
//!
//! Usage:
//!   spike-acp-panel [OPTIONS]                 the panel, hosting Claude Code over ACP
//!   spike-acp-panel --fake-agent [ARGS]       run the scripted ACP agent on stdio
//!                                             (ARGS: --scenario S --chunks N --rate HZ)
//!   spike-acp-panel --mcp-relay ADDR          stdio <-> Eludite MCP endpoint relay (token in ELUDITE_MCP_TOKEN);
//!                                             the agent launches this, not the user
//! Options:
//!   --agent claude|fake         default claude (npx -y @agentclientprotocol/claude-agent-acp@0.85.0)
//!   --fake-scenario S           diagnostics | diagnostics-then-shell | stream | login-required
//!   --cwd DIR                   the agent's working directory (default: current directory)
//!   --prompt TEXT               send TEXT once the window is open
//!   --auto-answer allow|deny    answer permission prompts after showing them for 1.5 s
//!   --exit-when-done            quit after the first turn ends (after --linger-ms, default 1500)
//!   --linger-ms N
//!   --transcript-out PATH       write the transcript JSON when the turn ends
//!   --bench-ready               start the agent at window open; print ready timings JSON; quit
//!   --bench-stream              fake agent streams --chunks N (2000) at --rate HZ (200); print frame JSON; quit
//!   --npm-cache DIR             npm cache for the agent's npx (cold-start measurement)
//!   --x11                       note in the output that the run used XWayland/X11 (informational)

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use eludite_acp::{AgentDescriptor, default_agents};
use serde_json::json;
use spike_acp_panel::bench::rss_mib;
use spike_acp_panel::panel::Panel;
use spike_acp_panel::session::{SessionConfig, spike_registry};

#[derive(Default)]
struct Args {
    fake: bool,
    scenario: Option<String>,
    chunks: usize,
    rate: f64,
    cwd: Option<PathBuf>,
    prompt: Option<String>,
    auto_answer: Option<bool>,
    exit_when_done: bool,
    linger_ms: u64,
    transcript_out: Option<PathBuf>,
    bench_ready: bool,
    bench_stream: bool,
    npm_cache: Option<PathBuf>,
}

fn parse_args(args: Vec<String>) -> Args {
    let mut a = Args {
        chunks: 2000,
        rate: 200.,
        linger_ms: 1500,
        ..Default::default()
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--agent" => a.fake = val() == "fake",
            "--fake-scenario" => {
                a.fake = true;
                a.scenario = Some(val());
            }
            "--chunks" => a.chunks = val().parse().expect("chunks"),
            "--rate" => a.rate = val().parse().expect("rate"),
            "--cwd" => a.cwd = Some(val().into()),
            "--prompt" => a.prompt = Some(val()),
            "--auto-answer" => a.auto_answer = Some(val() == "allow"),
            "--exit-when-done" => a.exit_when_done = true,
            "--linger-ms" => a.linger_ms = val().parse().expect("ms"),
            "--transcript-out" => a.transcript_out = Some(val().into()),
            "--bench-ready" => a.bench_ready = true,
            "--bench-stream" => a.bench_stream = true,
            "--npm-cache" => a.npm_cache = Some(val().into()),
            "--x11" => {}
            "-h" | "--help" => {
                println!("see the doc comment at the top of src/main.rs");
                std::process::exit(0);
            }
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

fn main() {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // Child-process modes: no window.
    if argv.first().map(String::as_str) == Some("--fake-agent") {
        let opts = eludite_acp::fake_agent::Options::from_args(argv.drain(1..))
            .unwrap_or_else(|e| panic!("{e}"));
        eludite_acp::fake_agent::run(std::io::stdin().lock(), std::io::stdout().lock(), opts)
            .expect("fake agent");
        return;
    }
    if argv.first().map(String::as_str) == Some("--mcp-relay") {
        let addr = argv
            .get(1)
            .and_then(|a| a.parse().ok())
            .expect("--mcp-relay ADDR");
        let token = std::env::var(eludite_mcp::transport::TOKEN_ENV).expect("token in environment");
        if let Err(e) = eludite_mcp::transport::relay_stdio(addr, &token) {
            eprintln!("mcp relay: {e}");
            std::process::exit(1);
        }
        return;
    }

    let a = parse_args(argv);
    let exe = std::env::current_exe().expect("current exe");
    let agent = if a.fake || a.bench_stream {
        let scenario = if a.bench_stream {
            "stream".into()
        } else {
            a.scenario
                .clone()
                .unwrap_or_else(|| "diagnostics-then-shell".into())
        };
        AgentDescriptor {
            name: format!("Fake agent ({scenario})"),
            command: exe.to_string_lossy().into_owned(),
            args: vec![
                "--fake-agent".into(),
                "--scenario".into(),
                scenario,
                "--chunks".into(),
                a.chunks.to_string(),
                "--rate".into(),
                a.rate.to_string(),
            ],
            env: vec![],
            env_remove: vec![],
        }
    } else {
        let mut d = default_agents().remove(0);
        if let Some(cache) = &a.npm_cache {
            d.env.push((
                "npm_config_cache".into(),
                cache.to_string_lossy().into_owned(),
            ));
        }
        d
    };
    let config = SessionConfig {
        agent,
        cwd: a
            .cwd
            .clone()
            .unwrap_or_else(|| std::env::current_dir().expect("cwd")),
        relay_exe: exe,
        registry: spike_registry(),
    };

    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(960.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Eludite spike 0005: Agents (ACP)".into()), ..Default::default() }),
            app_id: Some("eludite-spike".into()),
            inactive_frame_interval: if a.bench_stream { None } else { WindowOptions::default().inactive_frame_interval },
            ..Default::default()
        };
        let window = cx
            .open_window(options, |window, cx| {
                let panel = cx.new(|cx| Panel::new(config, cx));
                panel.focus_handle(cx).focus(window, cx);
                panel
            })
            .expect("open window");
        cx.activate(true);
        let panel = window.entity(cx).expect("panel");

        let linger = Duration::from_millis(a.linger_ms);
        let transcript_out = a.transcript_out.clone();
        let exit_when_done = a.exit_when_done || a.bench_stream;
        let bench_stream = a.bench_stream;
        let fake = a.fake;
        panel.update(cx, |p, _| {
            p.automation.auto_answer = a.auto_answer.map(|allow| (allow, Duration::from_millis(1500)));
            if bench_stream {
                p.probes.borrow_mut().recording = true;
            }
            p.automation.on_turn_end = Some(Box::new(move |p, cx| {
                println!("TURN-ENDED");
                if let Some(path) = &transcript_out {
                    let out = json!({
                        "status": format!("{:?}", p.status),
                        "timings_ms": p.timings.iter().map(|(n, m)| json!({n.to_string(): m})).collect::<Vec<_>>(),
                        "mcp_calls": p.mcp_calls,
                        "stop": format!("{:?}", p.last_stop),
                        "transcript": p.transcript.to_json(),
                    });
                    std::fs::write(path, serde_json::to_string_pretty(&out).expect("json")).expect("write transcript");
                }
                if bench_stream {
                    let mut report = p.probes.borrow().report();
                    report["bench"] = json!("stream");
                    report["platform"] = json!({
                        "os": std::env::consts::OS,
                        "wayland_display": std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
                        "display": std::env::var("DISPLAY").unwrap_or_default(),
                    });
                    println!("{}", serde_json::to_string(&report).expect("json"));
                }
                if exit_when_done {
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(linger).await;
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                }
            }));
            if a.bench_ready {
                p.automation.on_ready = Some(Box::new(move |p, cx| {
                    let out = json!({
                        "bench": "ready",
                        "agent": if fake { "fake" } else { "claude" },
                        "timings_ms": p.timings.iter().map(|(n, m)| (n.to_string(), json!(m))).collect::<serde_json::Map<_, _>>(),
                        "status": format!("{:?}", p.status),
                        "rss_mib": rss_mib(),
                    });
                    println!("{}", serde_json::to_string(&out).expect("json"));
                    cx.quit();
                }));
            }
        });
        if a.bench_ready {
            panel.update(cx, |p, cx| p.ensure_session(cx));
        }
        let prompt = if a.bench_stream { Some("stream".to_owned()) } else { a.prompt.clone() };
        if let Some(text) = prompt {
            panel.update(cx, |p, cx| p.send(Some(text), cx));
        }
    });
}
