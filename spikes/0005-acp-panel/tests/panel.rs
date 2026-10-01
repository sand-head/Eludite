//! Headless GPUI tests of the panel against the scripted fake agent.
//!
//! The fake agent and the MCP relay are real child processes (this package's
//! binary in `--fake-agent` / `--mcp-relay` mode), so these exercise the same
//! path as the real run: ACP over stdio, `session/new` with a stdio MCP server,
//! the relay into the panel's MCP endpoint, the command bus.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eludite_acp::AgentDescriptor;
use eludite_acp::fake_agent::{DIAGNOSTICS_TOOL, SHELL_TOOL};
use eludite_acp::protocol::ToolCallStatus;
use eludite_commands::diagnostics::{self, DIAGNOSTICS_LIST};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use serde_json::Value;
use spike_acp_panel::panel::Panel;
use spike_acp_panel::session::{AgentStatus, SessionConfig, auto_allow, spike_registry};
use spike_acp_panel::transcript::{PermissionState, Row};

const EXE: &str = env!("CARGO_BIN_EXE_spike-acp-panel");

fn config(scenario: &str) -> SessionConfig {
    SessionConfig {
        agent: AgentDescriptor {
            name: "Fake agent".into(),
            command: EXE.into(),
            args: vec!["--fake-agent".into(), "--scenario".into(), scenario.into()],
            env: vec![],
            env_remove: vec![],
        },
        cwd: std::env::temp_dir(),
        relay_exe: PathBuf::from(EXE),
        registry: spike_registry(),
    }
}

fn open(cx: &mut TestAppContext, scenario: &str) -> (Entity<Panel>, VisualTestContext) {
    cx.executor().allow_parking();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let panel = cx.new(|cx| Panel::new(config(scenario), cx));
            gpui::Focusable::focus_handle(panel.read(cx), cx).focus(window, cx);
            panel
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(2400.)));
    let panel = window.root(&mut vcx).unwrap();
    (panel, vcx)
}

/// Pump the UI until `cond` holds. Events come from real threads, so park
/// briefly in real time between pumps.
fn wait_until(
    vcx: &mut VisualTestContext,
    panel: &Entity<Panel>,
    what: &str,
    cond: impl Fn(&Panel) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        vcx.run_until_parked();
        if panel.read_with(vcx, |p, _| cond(p)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:#}",
            panel.read_with(vcx, |p, _| p.transcript.to_json())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn click(vcx: &mut VisualTestContext, sel: &str) {
    let b = vcx
        .debug_bounds(Box::leak(sel.to_owned().into_boxed_str()))
        .unwrap_or_else(|| panic!("no element {sel}"));
    vcx.simulate_click(b.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
}

/// `simulate_keystrokes` syntax for typing `text` key by key.
fn keystrokes(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            ' ' => "space".to_owned(),
            c if c.is_ascii_uppercase() => format!("shift-{}", c.to_ascii_lowercase()),
            c => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pending_key(p: &Panel) -> Option<u64> {
    p.transcript.pending_permissions().first().copied()
}

/// The proving flow: type the prompt, Send; the agent calls Eludite's read
/// tool (no prompt shown, answered by policy), receives the fixture through
/// MCP, answers; then asks to run a shell command, which shows a prompt that
/// the user denies by clicking "No"; the call fails and the agent says so.
#[gpui::test]
fn diagnostics_read_runs_without_prompt_and_shell_is_denied(cx: &mut TestAppContext) {
    let (panel, mut vcx) = open(cx, "diagnostics-then-shell");
    let prompt = "List the current errors in the Error List and tell me which file has the most.";
    vcx.simulate_keystrokes(&keystrokes(prompt));
    assert_eq!(panel.read_with(&vcx, |p, _| p.input.clone()), prompt);
    assert_eq!(
        panel.read_with(&vcx, |p, _| p.status.clone()),
        AgentStatus::NotStarted,
        "no agent before Send"
    );
    click(&mut vcx, "send");

    wait_until(&mut vcx, &panel, "shell permission prompt", |p| {
        pending_key(p).is_some()
    });
    let key = panel.read_with(&vcx, |p, _| pending_key(p).unwrap());

    panel.read_with(&vcx, |p, _| {
        assert!(
            matches!(&p.status, AgentStatus::Ready { protocol: 1, .. }),
            "{:?}",
            p.status
        );
        let names: Vec<_> = p
            .transcript
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Permission(perm) => Some((perm.title.clone(), perm.state.clone())),
                _ => None,
            })
            .collect();
        // The read tool was allowed by policy (no prompt); the shell tool is waiting on the user.
        assert_eq!(
            names,
            [
                (
                    DIAGNOSTICS_TOOL.to_owned(),
                    PermissionState::Auto {
                        command: DIAGNOSTICS_LIST.into()
                    }
                ),
                (SHELL_TOOL.to_owned(), PermissionState::Pending),
            ]
        );
        let diag = p
            .transcript
            .tools()
            .find(|t| t.agent_tool_name() == Some(DIAGNOSTICS_TOOL))
            .unwrap();
        assert_eq!(diag.status, Some(ToolCallStatus::Completed));
        assert_eq!(diag.raw_input.as_ref().unwrap()["severity"], "error");
        // The result came through Eludite's MCP endpoint, from the command bus.
        assert_eq!(p.mcp_calls.len(), 1, "{:?}", p.mcp_calls);
        assert!(p.mcp_calls[0].starts_with("diagnostics.list (read)"));
        // Served on the MCP endpoint's connection thread, not the UI thread.
        assert!(
            p.mcp_calls[0].ends_with("thread=mcp-conn"),
            "{}",
            p.mcp_calls[0]
        );
        let result: Value = serde_json::from_str(&diag.content_text()).unwrap();
        let rows = &result["result"];
        let schema: Value = serde_json::from_str(diagnostics::OUTPUT_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors: Vec<String> = validator.iter_errors(rows).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{errors:?}");
        let expected: Vec<Value> = diagnostics::fixture()
            .into_iter()
            .filter(|d| d.severity == diagnostics::Severity::Error)
            .map(|d| serde_json::to_value(d).unwrap())
            .collect();
        assert_eq!(rows.as_array().unwrap(), &expected);
        assert!(
            p.transcript
                .agent_message()
                .contains("`src/Contoso.Web/Controllers/OrderController.cs` with 3")
        );
    });
    // The tool call card and the prompt are on screen.
    assert!(vcx.debug_bounds("tool-2").is_some() || vcx.debug_bounds("tool-1").is_some());
    click(&mut vcx, &format!("perm-{key}-deny"));

    wait_until(&mut vcx, &panel, "turn end", |p| !p.running);
    panel.read_with(&vcx, |p, _| {
        let shell = p.transcript.tools().find(|t| t.agent_tool_name() == Some(SHELL_TOOL)).unwrap();
        assert_eq!(shell.status, Some(ToolCallStatus::Failed));
        assert!(p.transcript.agent_message().contains("You denied permission to run `rm -rf obj/`"));
        assert!(p.transcript.rows.iter().any(|r| matches!(r, Row::Permission(perm) if matches!(&perm.state, PermissionState::Answered { allowed: false, .. }))));
        assert_eq!(p.last_stop, Some(Ok("EndTurn".into())));
        // Audit line: exactly one command ran over MCP, the read one.
        assert_eq!(p.mcp_calls.len(), 1);
    });
}

/// Logged out: the panel shows the state and the agent's own login command.
#[gpui::test]
fn login_required_shows_state_and_instructions(cx: &mut TestAppContext) {
    let (panel, mut vcx) = open(cx, "login-required");
    panel.update(&mut vcx, |p, cx| p.send(Some("hi".into()), cx));
    wait_until(&mut vcx, &panel, "login state", |p| {
        matches!(p.status, AgentStatus::LoginRequired { .. }) && !p.running
    });
    panel.read_with(&vcx, |p, _| {
        let AgentStatus::LoginRequired { label, methods } = &p.status else {
            unreachable!()
        };
        assert_eq!(label, "Not logged in");
        assert!(
            methods[0]
                .2
                .ends_with("--fake-agent --scenario login-required --cli auth login --claudeai"),
            "{methods:?}"
        );
        assert!(
            p.transcript
                .rows
                .iter()
                .any(|r| matches!(r, Row::Notice(t) if t.contains("not logged in")))
        );
    });
}

#[gpui::test]
fn stop_cancels_a_pending_prompt(cx: &mut TestAppContext) {
    let (panel, mut vcx) = open(cx, "diagnostics-then-shell");
    panel.update(&mut vcx, |p, cx| p.send(Some("go".into()), cx));
    wait_until(&mut vcx, &panel, "prompt", |p| pending_key(p).is_some());
    click(&mut vcx, "send"); // the button reads "Stop" while running
    wait_until(&mut vcx, &panel, "cancelled", |p| !p.running);
    assert_eq!(
        panel.read_with(&vcx, |p, _| p.last_stop.clone()),
        Some(Ok("Cancelled".into()))
    );
}

#[test]
fn policy_only_auto_allows_eludite_read_tools() {
    use eludite_acp::protocol::ToolCall;
    let reg = spike_registry();
    let call = |name: &str, server: Option<&str>| ToolCall {
        tool_call_id: "x".into(),
        title: Some(name.into()),
        meta: Some(match server {
            Some(s) => {
                serde_json::json!({"claudeCode": {"toolName": name, "mcpServer": {"name": s}}})
            }
            None => serde_json::json!({"claudeCode": {"toolName": name}}),
        }),
        ..Default::default()
    };
    assert_eq!(
        auto_allow(&call(DIAGNOSTICS_TOOL, Some("eludite")), &reg)
            .unwrap()
            .as_str(),
        DIAGNOSTICS_LIST
    );
    assert_eq!(
        auto_allow(&call(DIAGNOSTICS_TOOL, None), &reg)
            .unwrap()
            .as_str(),
        DIAGNOSTICS_LIST
    );
    for (name, server) in [
        (DIAGNOSTICS_TOOL, Some("other")), // same tool name from another server
        ("mcp__other__diagnostics-list", None), // another server
        ("mcp__eludite__eludite-help-about", None), // registered but not exposed
        ("mcp__eludite__nope", None),
        ("Bash", None),
        ("Read", None),
        ("Write", None),
    ] {
        assert!(
            auto_allow(&call(name, server), &reg).is_none(),
            "{name} {server:?}"
        );
    }
}
