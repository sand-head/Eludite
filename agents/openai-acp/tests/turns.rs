//! The adapter binary end to end: ACP on its stdio, a loopback fake OpenAI-compatible server for the model, and a
//! fake of Eludite's MCP endpoint behind the stdio relay for the tools.

mod acp;
mod fake_mcp;
mod fake_server;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use acp::{ADAPTER, Adapter, RELAY, stop_reason, temp_dir};
use fake_mcp::{CORE_PRESENT, FakeMcp, eludite_tools, text_result};
use fake_server::{
    End, Ev, FakeServer, Reply, finish, text, text_reply, thought, tool_frag, tool_reply, usage,
};
use serde_json::{Value, json};

const KEY: &str = "sk-live-test-SECRET-6f1c2b9a77d04e15";

struct Rig {
    server: FakeServer,
    mcp: FakeMcp,
    a: Adapter,
    session: String,
    cwd: PathBuf,
}

/// The fake server with two llama.cpp models (16k), the fake MCP endpoint, and a started session.
fn rig(name: &str, extra: &[&str], env: &[(&str, &str)]) -> Rig {
    rig_with(name, extra, env, |s| {
        s.llama_models(&["qwen3-8b", "gemma-3-4b"], 16_384)
    })
}

fn rig_with(
    name: &str,
    extra: &[&str],
    env: &[(&str, &str)],
    models: impl FnOnce(&FakeServer),
) -> Rig {
    let server = FakeServer::start();
    models(&server);
    let mcp = FakeMcp::start(eludite_tools());
    let mut args = vec!["--base-url", server.url.as_str()];
    args.extend_from_slice(extra);
    let mut a = Adapter::spawn(&args, env);
    let cwd = temp_dir(name);
    let session = a.start(&cwd, vec![mcp.stdio_server(RELAY)]);
    Rig {
        server,
        mcp,
        a,
        session,
        cwd,
    }
}

fn names(tools: &Value) -> Vec<String> {
    tools
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `eludite-openai-acp models ...`: its stdout as JSON.
fn models_command(args: &[&str], env: &[(&str, &str)]) -> Value {
    let mut cmd = std::process::Command::new(ADAPTER);
    cmd.arg("models")
        .args(args)
        .env_remove("ELUDITE_OPENAI_API_KEY");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn models_are_listed_with_their_windows_and_statuses_mean_what_they_say() {
    let s = FakeServer::start();
    s.set_models(Reply::json(
        200,
        json!({"object": "list", "data": [
            {"id": "Qwen3-8B-Q4_K_M.gguf", "object": "model", "owned_by": "llamacpp", "meta": {"n_ctx_train": 40960}},
            {"id": "a-model", "object": "model", "owned_by": "llamacpp"}]}),
    ));
    let url = s.url.clone() + "/";
    let l = models_command(
        &["--base-url", &url, "--header", "X-Title=Eludite"],
        &[("ELUDITE_OPENAI_API_KEY", KEY)],
    );
    assert_eq!(l["listing"], "server");
    assert_eq!(
        l["models"],
        json!([{"id": "a-model"}, {"id": "Qwen3-8B-Q4_K_M.gguf", "contextWindow": 40960}])
    );
    let r = &s.requests()[0];
    assert_eq!(r.path, "/v1/models", "one trailing slash stripped");
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(r.header("x-title"), Some("Eludite"));
    // No key: no Authorization header at all, and OPENAI_API_KEY is never read.
    models_command(
        &["--base-url", &s.url],
        &[("OPENAI_API_KEY", "sk-from-the-shell")],
    );
    assert_eq!(s.requests()[1].header("authorization"), None);

    s.set_models(Reply::json(404, json!({"error": {"message": "Not Found"}})));
    let l = models_command(&["--base-url", &s.url], &[]);
    assert_eq!(
        (l["listing"].as_str(), l["message"].as_str()),
        (Some("none"), Some("this server does not list models"))
    );
    let catalog = r#"[{"id":"mine","name":"My model","contextWindow":8192}]"#;
    let before = s.requests().len();
    let l = models_command(&["--base-url", &s.url, "--catalog", catalog], &[]);
    assert_eq!(
        l,
        json!({"models": [{"id": "mine", "name": "My model", "contextWindow": 8192}], "listing": "catalog"})
    );
    assert_eq!(
        s.requests().len(),
        before,
        "a catalog never asks the server"
    );

    s.set_models(Reply::json(
        401,
        json!({"error": {"message": "Invalid API key"}}),
    ));
    let l = models_command(
        &["--base-url", &s.url],
        &[("ELUDITE_OPENAI_API_KEY", "bad")],
    );
    assert_eq!(l["message"], "the key was rejected (401)");
    s.set_models(Reply::html(
        "<html><body>Welcome to the proxy</body></html>",
    ));
    let l = models_command(&["--base-url", &s.url], &[]);
    assert_eq!(l["listing"], "none");

    // A rejected key fails session/new.
    s.set_models(Reply::json(403, json!({})));
    let mut a = Adapter::spawn(
        &["--base-url", &s.url],
        &[("ELUDITE_OPENAI_API_KEY", "bad")],
    );
    a.initialize();
    let r = a.new_session(&temp_dir("rejected"), vec![]);
    assert_eq!(r["error"]["code"], -32000, "auth_required: {r}");
    assert!(
        r["error"]["data"]
            .as_str()
            .unwrap()
            .contains("the key was rejected (403)"),
        "{r}"
    );
}

#[test]
fn a_turn_streams_text_and_reports_usage() {
    let mut r = rig(
        "text",
        &["--header", "X-Title=Eludite"],
        &[("ELUDITE_OPENAI_API_KEY", KEY)],
    );
    r.server.push(Reply::sse(vec![
        text("Hel"),
        text("lo"),
        finish("stop"),
        usage(50, 2),
    ]));
    let resp = r.a.prompt(&r.session, "hi");
    assert_eq!(stop_reason(&resp), "end_turn");
    assert_eq!(r.a.message_text(), "Hello");
    assert_eq!(r.a.updates_of("agent_message_chunk").len(), 2);
    let commands = r.a.updates_of("available_commands_update");
    let cmds: Vec<&str> = commands[0]["availableCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(cmds, ["compact", "clear"]);
    let u = &r.a.updates_of("usage_update")[0];
    assert_eq!(
        (u["used"].as_u64(), u["size"].as_u64()),
        (Some(52), Some(16_384))
    );
    assert!(u.get("cost").is_none());
    assert_eq!(
        u["_meta"]["eludite"]["usage"],
        json!({"inputTokens": 50, "outputTokens": 2, "totalTokens": 52, "model": "gemma-3-4b"})
    );
    let req = &r.server.chat_requests()[0];
    assert_eq!(
        req.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(req.header("x-title"), Some("Eludite"));
    let body = req.json();
    assert_eq!(
        body["model"], "gemma-3-4b",
        "the first listed model, sorted"
    );
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["max_tokens"], 8192);
    for absent in [
        "temperature",
        "tool_choice",
        "max_completion_tokens",
        "top_p",
    ] {
        assert!(body.get(absent).is_none(), "{absent}");
    }
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(
        body["messages"][1],
        json!({"role": "user", "content": "hi"})
    );
    // The second prompt replays the answer.
    r.server.push(text_reply("Again", 60, 1));
    r.a.prompt(&r.session, "more");
    let body = r.server.chats()[1].clone();
    assert_eq!(
        body["messages"][2],
        json!({"role": "assistant", "content": "Hello"})
    );
    assert_eq!(body["messages"][3]["content"], "more");
}

#[test]
fn session_new_offers_the_models_and_set_config_option_switches_the_next_request() {
    let mut r = rig("models", &["--model", "gemma-3-4b"], &[]);
    let resp = r.a.new_session(&r.cwd, vec![]);
    let opt = &resp["result"]["configOptions"][0];
    assert_eq!(opt["id"], "model");
    assert_eq!(opt["category"], "model");
    assert_eq!(opt["type"], "select");
    assert_eq!(opt["currentValue"], "gemma-3-4b");
    assert_eq!(
        opt["options"],
        json!([{"value": "gemma-3-4b", "name": "gemma-3-4b"}, {"value": "qwen3-8b", "name": "qwen3-8b"}])
    );
    let set = r.a.call(
        "session/set_config_option",
        json!({"sessionId": r.session, "configId": "model", "value": "qwen3-8b"}),
    );
    assert_eq!(
        set["result"]["configOptions"][0]["currentValue"], "qwen3-8b",
        "{set}"
    );
    r.a.drain(Duration::from_millis(100));
    let update = &r.a.updates_of("config_option_update")[0];
    assert_eq!(update["configOptions"][0]["currentValue"], "qwen3-8b");
    let bad = r.a.call(
        "session/set_config_option",
        json!({"sessionId": r.session, "configId": "model", "value": "nope"}),
    );
    assert_eq!(bad["error"]["code"], -32602);
    r.server.push(text_reply("ok", 10, 1));
    r.a.prompt(&r.session, "hi");
    assert_eq!(r.server.chats()[0]["model"], "qwen3-8b");
}

#[test]
fn two_tool_calls_accumulate_across_fragments_and_run_in_order() {
    let mut r = rig("tools", &[], &[]);
    r.server.push(Reply::sse(vec![
        tool_frag(0, Some("call_a"), Some("eludite-file-read"), ""),
        tool_frag(0, None, None, "{\"pa"),
        tool_frag(1, Some("call_b"), Some("diagnostics-list"), "{}"),
        tool_frag(0, None, None, "th\": \"src/ma"),
        tool_frag(0, None, None, "in.rs\"}"),
        finish("tool_calls"),
        usage(900, 40),
    ]));
    r.server
        .push(text_reply("It has a main function.", 1000, 8));
    let resp = r.a.prompt(&r.session, "what is in main.rs?");
    assert_eq!(stop_reason(&resp), "end_turn");
    let calls = r.mcp.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "eludite-file-read");
    assert_eq!(calls[0].arguments, json!({"path": "src/main.rs"}));
    assert_eq!(calls[0].tool_call_id.as_deref(), Some("call_a"));
    assert_eq!(
        (calls[1].name.as_str(), calls[1].tool_call_id.as_deref()),
        ("diagnostics-list", Some("call_b"))
    );
    assert!(calls[0].at <= calls[1].at, "one at a time, in order");
    let body = r.server.chats()[1].clone();
    let m = body["messages"].as_array().unwrap();
    let assistant = &m[2];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["tool_calls"][0]["id"], "call_a");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        "{\"path\": \"src/main.rs\"}"
    );
    assert_eq!(
        assistant["tool_calls"][1]["function"]["name"],
        "diagnostics-list"
    );
    assert_eq!(m[3]["role"], "tool");
    assert_eq!(m[3]["tool_call_id"], "call_a");
    assert!(
        m[3]["content"]
            .as_str()
            .unwrap()
            .starts_with("eludite-file-read ran with")
    );
    assert_eq!(m[4]["tool_call_id"], "call_b");
    // ACP: announced as soon as the name is known, then in progress, then completed.
    let ups = r.a.updates();
    let first = ups
        .iter()
        .position(|u| u["sessionUpdate"] == "tool_call" && u["toolCallId"] == "call_a")
        .unwrap();
    let started = &ups[first];
    // `pending` is ACP's default status, so it may be left out.
    assert!(
        matches!(started["status"].as_str(), None | Some("pending")),
        "{started}"
    );
    assert_eq!(started["kind"], "read");
    let a_updates: Vec<&Value> = ups
        .iter()
        .filter(|u| u["sessionUpdate"] == "tool_call_update" && u["toolCallId"] == "call_a")
        .collect();
    assert_eq!(a_updates[0]["status"], "in_progress");
    assert_eq!(a_updates[0]["title"], "eludite-file-read src/main.rs");
    assert_eq!(
        a_updates[0]["locations"][0]["path"].as_str().unwrap(),
        r.cwd.join("src/main.rs").to_string_lossy()
    );
    assert_eq!(a_updates[1]["status"], "completed");
    assert_eq!(r.a.message_text(), "It has a main function.");
    let usage = r.a.updates_of("usage_update");
    assert_eq!(usage.len(), 2, "one per response");
    assert_eq!(usage[1]["used"], 1008);
    assert_eq!(
        usage[1]["_meta"]["eludite"]["usage"]["totalTokens"],
        940 + 1008
    );
}

#[test]
fn a_json_body_a_missing_done_and_reasoning() {
    let mut r = rig("shapes", &[], &[]);
    r.server.push(Reply::json(
        200,
        json!({"id": "c", "object": "chat.completion", "choices": [{"index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": "Plain body."}}], "usage": {"prompt_tokens": 7, "completion_tokens": 3}}),
    ));
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "one")), "end_turn");
    assert_eq!(r.a.message_text(), "Plain body.");
    assert_eq!(r.a.updates_of("usage_update")[0]["used"], 10);
    r.a.clear();
    r.server.push(Reply::Sse {
        events: vec![text("No done"), finish("stop")],
        end: End::NoDone,
    });
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "two")), "end_turn");
    assert_eq!(
        r.server.chats().len(),
        2,
        "complete without [DONE]: no retry"
    );
    r.a.clear();
    r.server.push(Reply::sse(vec![
        thought("Let me"),
        Ev::Data(fake_server::chunk(json!({"reasoning": " think."}), None)),
        text("Answer"),
        finish("stop"),
    ]));
    r.a.prompt(&r.session, "three");
    let thoughts: String =
        r.a.updates_of("agent_thought_chunk")
            .iter()
            .map(|u| u["content"]["text"].as_str().unwrap().to_owned())
            .collect();
    assert_eq!(thoughts, "Let me think.");
    assert_eq!(r.a.message_text(), "Answer");
    r.server.push(text_reply("ok", 1, 1));
    r.a.prompt(&r.session, "four");
    let last = r.server.chats().last().unwrap().clone();
    let replayed = &last["messages"][6];
    assert_eq!(
        *replayed,
        json!({"role": "assistant", "content": "Answer"}),
        "never with reasoning"
    );
}

#[test]
fn a_dropped_stream_is_retried_once_then_fails() {
    let mut r = rig("dropped", &[], &[]);
    let dropped = || Reply::Sse {
        events: vec![text("par")],
        end: End::Drop,
    };
    r.server.push(dropped());
    r.server.push(text_reply("whole", 5, 1));
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "one")), "end_turn");
    assert_eq!(r.server.chats().len(), 2, "retried once");
    r.server.push(dropped());
    r.server.push(dropped());
    let resp = r.a.prompt(&r.session, "two");
    assert_eq!(
        resp["error"]["message"], "The server closed the stream before the answer was complete",
        "{resp}"
    );
    assert_eq!(r.server.chats().len(), 4);
    // A server error's message is the turn's error.
    r.server.push(Reply::json(
        500,
        json!({"error": {"message": "model crashed"}}),
    ));
    let resp = r.a.prompt(&r.session, "three");
    assert_eq!(
        resp["error"]["message"],
        "The server answered 500: model crashed"
    );
    // Failed prompts leave nothing unanswered behind: the roles still alternate.
    r.server.push(text_reply("fine", 5, 1));
    r.a.prompt(&r.session, "four");
    let m = r.server.chats().last().unwrap()["messages"].clone();
    let roles: Vec<&str> = m
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user", "assistant", "user"]);
    assert_eq!(m[3]["content"], "four");
}

#[test]
fn cancel_mid_stream_answers_cancelled_at_once_and_drops_the_request() {
    let mut r = rig("cancel", &[], &[]);
    r.server.push(Reply::Sse {
        events: vec![text("Working")],
        end: End::Stall,
    });
    let id = r.a.send(
        "session/prompt",
        json!({"sessionId": r.session, "prompt": [{"type": "text", "text": "go"}]}),
    );
    r.a.wait_for(|v| v["params"]["update"]["sessionUpdate"] == "agent_message_chunk");
    let t0 = Instant::now();
    r.a.notify("session/cancel", json!({"sessionId": r.session}));
    let (resp, at) = r.a.wait(id);
    assert_eq!(stop_reason(&resp), "cancelled");
    let took = at.duration_since(t0);
    assert!(took < Duration::from_millis(100), "{took:?}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !r.server.client_closed.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the request was not dropped");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The conversation goes on.
    r.server.push(text_reply("Back", 5, 1));
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "again")), "end_turn");
    let body = r.server.chats().last().unwrap().clone();
    assert_eq!(
        body["messages"][2],
        json!({"role": "assistant", "content": "Working"})
    );
}

#[test]
fn cancel_while_a_tool_runs_answers_the_call_in_the_history() {
    let mut r = rig("cancel-tool", &[], &[]);
    r.mcp.set_delay("eludite-build-solution", 2_000);
    r.server
        .push(tool_reply("call_b", "eludite-build-solution", "{}", 10));
    let id = r.a.send(
        "session/prompt",
        json!({"sessionId": r.session, "prompt": [{"type": "text", "text": "build"}]}),
    );
    r.a.wait_for(|v| v["params"]["update"]["status"] == "in_progress");
    let t0 = Instant::now();
    r.a.notify("session/cancel", json!({"sessionId": r.session}));
    let (resp, at) = r.a.wait(id);
    assert_eq!(stop_reason(&resp), "cancelled");
    assert!(at.duration_since(t0) < Duration::from_millis(100));
    r.server.push(text_reply("ok", 5, 1));
    r.a.prompt(&r.session, "next");
    let m = r.server.chats().last().unwrap()["messages"].clone();
    assert_eq!(m[3]["tool_call_id"], "call_b");
    assert!(
        m[3]["content"]
            .as_str()
            .unwrap()
            .contains("cancelled the turn while this tool was running")
    );
}

#[test]
fn fifty_requests_end_the_turn_with_max_turn_requests() {
    let mut r = rig("max", &[], &[]);
    // Every answer calls a tool, with no id (the adapter makes one).
    r.server.set_fallback(Reply::sse(vec![
        tool_frag(0, Some(""), Some("diagnostics-list"), "{}"),
        finish("tool_calls"),
    ]));
    let resp = r.a.prompt(&r.session, "loop forever");
    assert_eq!(stop_reason(&resp), "max_turn_requests");
    assert_eq!(r.server.chats().len(), 50);
    assert_eq!(r.mcp.calls().len(), 50);
    let ids: std::collections::HashSet<_> = r
        .mcp
        .calls()
        .iter()
        .map(|c| c.tool_call_id.clone().unwrap())
        .collect();
    assert_eq!(ids.len(), 50, "unique tool call ids");
}

#[test]
fn compaction_at_85_percent_of_a_16k_window() {
    let catalog = r#"[{"id":"small","contextWindow":16384}]"#;
    let mut r = rig_with(
        "compact85",
        &["--catalog", catalog, "--model", "small"],
        &[],
        |_| {},
    );
    assert!(
        r.server.requests().is_empty(),
        "the catalog is not asked for"
    );
    r.server.push(text_reply("first", 100, 5));
    r.a.prompt(&r.session, "one");
    r.server
        .push(tool_reply("c1", "diagnostics-list", "{}", 14_000));
    r.server.push(text_reply(
        "SUMMARY: the person asked twice; diagnostics ran.",
        3_000,
        20,
    ));
    r.server.push(text_reply("done", 600, 5));
    let resp = r.a.prompt(&r.session, "two");
    assert_eq!(stop_reason(&resp), "end_turn");
    let text = r.a.message_text();
    assert!(
        text.contains("Compacted the conversation (14010 tokens to "),
        "{text}"
    );
    let chats = r.server.chats();
    assert_eq!(chats.len(), 4);
    let summary = &chats[2];
    assert!(
        summary.get("tools").is_none(),
        "the summary request has no tools"
    );
    assert!(
        summary["messages"][0]["content"]
            .as_str()
            .unwrap()
            .starts_with("You summarize")
    );
    assert!(
        summary["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("User: one")
    );
    let after = chats[3]["messages"].as_array().unwrap();
    assert!(
        after[1]["content"]
            .as_str()
            .unwrap()
            .starts_with("Summary of the conversation so far")
    );
    assert!(
        after[1]["content"]
            .as_str()
            .unwrap()
            .contains("SUMMARY: the person asked twice")
    );
    assert_eq!(
        after.last().unwrap()["tool_call_id"],
        "c1",
        "the last messages are kept"
    );
    assert!(after.len() < 8);
}

#[test]
fn a_context_length_error_compacts_and_retries_once() {
    // OpenAI-style listing: no window known, so only the server's error triggers it.
    let mut r = rig_with("ctxerr", &[], &[], |s| {
        s.set_models(Reply::json(
            200,
            json!({"data": [{"id": "gpt-x", "object": "model"}]}),
        ))
    });
    for (p, a) in [("one", "first"), ("two", "second")] {
        r.server.push(text_reply(a, 10, 1));
        r.a.prompt(&r.session, p);
    }
    r.server.push(Reply::json(
        400,
        json!({"error": {"code": 400, "message": "the request exceeds the available context size, try increasing it",
            "type": "exceed_context_size_error", "n_prompt_tokens": 17000, "n_ctx": 16384}}),
    ));
    r.server.push(text_reply("SUMMARY", 100, 5));
    r.server.push(text_reply("fits now", 300, 2));
    let resp = r.a.prompt(&r.session, "three");
    assert_eq!(stop_reason(&resp), "end_turn", "{resp}");
    assert!(r.a.message_text().contains("Compacted the conversation"));
    assert!(r.a.message_text().ends_with("fits now"));
    assert_eq!(r.server.chats().len(), 2 + 3);
    // A second error in the same request is the turn's error, reported as a context-length error.
    r.server.push(Reply::json(
        400,
        json!({"error": {"message": "context_length_exceeded", "code": "context_length_exceeded"}}),
    ));
    r.server.push(text_reply("SUMMARY 2", 100, 5));
    r.server.push(Reply::json(
        400,
        json!({"error": {"message": "still too long", "code": "context_length_exceeded"}}),
    ));
    let resp = r.a.prompt(&r.session, "four");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("The conversation is longer than the model's context window"),
        "{resp}"
    );
}

#[test]
fn slash_compact_and_clear() {
    let mut r = rig("slash", &[], &[]);
    let resp = r.a.prompt(&r.session, "/compact");
    assert_eq!(stop_reason(&resp), "end_turn");
    assert_eq!(r.a.message_text(), "There is nothing to compact yet.");
    for p in ["one", "two", "three"] {
        r.server.push(text_reply(&format!("answer {p}"), 10, 1));
        r.a.prompt(&r.session, p);
    }
    r.a.clear();
    r.server.push(text_reply("SUMMARY of one", 50, 5));
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "/compact")), "end_turn");
    assert!(
        r.a.message_text()
            .starts_with("Compacted the conversation ("),
        "{}",
        r.a.message_text()
    );
    assert_eq!(r.server.chats().len(), 4);
    r.a.clear();
    assert_eq!(stop_reason(&r.a.prompt(&r.session, "/clear")), "end_turn");
    assert_eq!(
        r.a.message_text(),
        "Cleared the conversation. The next prompt starts fresh."
    );
    assert_eq!(r.server.chats().len(), 4, "no model call for /clear");
    r.server.push(text_reply("fresh", 10, 1));
    r.a.prompt(&r.session, "hello again");
    let m = r.server.chats()[4]["messages"].clone();
    assert_eq!(m.as_array().unwrap().len(), 2);
    assert_eq!(m[1]["content"], "hello again");
}

#[test]
fn the_key_never_reaches_the_log() {
    let dir = temp_dir("log");
    let log = dir.join("adapter.log");
    let log_s = log.to_string_lossy().into_owned();
    let mut r = rig(
        "log",
        &["--header", "X-Title=Eludite"],
        &[
            ("ELUDITE_OPENAI_API_KEY", KEY),
            ("ELUDITE_OPENAI_ACP_LOG", &log_s),
        ],
    );
    r.server.push(tool_reply(
        "c1",
        "eludite-file-read",
        "{\"path\": \"a.rs\"}",
        10,
    ));
    r.server.push(text_reply("done", 10, 1));
    // The key even appears in the conversation; it is still replaced in the log.
    r.a.prompt(&r.session, &format!("my key is {KEY}, do not print it"));
    drop(r.a);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("POST ") && text.contains("/chat/completions"),
        "the log was written: {text}"
    );
    assert!(text.contains("GET "), "{text}");
    assert!(!text.contains(KEY), "the key is in the log");
    assert!(
        !text.contains(fake_mcp::TOKEN),
        "the MCP token is in the log"
    );
    assert!(text.contains("<redacted>"));
    assert!(
        !text.to_ascii_lowercase().contains("authorization"),
        "no authorization header logged"
    );
    assert_eq!(
        r.server.chat_requests()[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str()),
        "the key was sent"
    );
}

#[test]
fn the_system_prompt_stays_under_4000_tokens_with_the_real_guides() {
    let server = FakeServer::start();
    server.llama_models(&["m"], 16_384);
    let mcp = FakeMcp::start(eludite_tools());
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/agents");
    for (uri, file) in [
        ("eludite://guides/debugging", "debugging.md"),
        ("eludite://guides/git", "git.md"),
        ("eludite://guides/terminal", "terminal.md"),
    ] {
        mcp.set_guide(uri, &std::fs::read_to_string(docs.join(file)).unwrap());
    }
    let mut a = Adapter::spawn(&["--base-url", &server.url], &[]);
    let cwd = temp_dir("prompt");
    let session = a.start(&cwd, vec![mcp.stdio_server(RELAY)]);
    server.push(text_reply("ok", 1, 1));
    a.prompt(&session, "hi");
    let system = server.chats()[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    let estimate = system.len().div_ceil(4);
    assert!(estimate < 4_000, "{estimate} estimated tokens");
    assert!(system.contains(&cwd.to_string_lossy().into_owned()));
    for heading in [
        "# Driving the debugger",
        "# Using git",
        "# Running commands in the terminal",
    ] {
        assert!(system.contains(heading), "{heading}");
    }
    assert!(system.contains("(The rest of eludite://guides/debugging is left out here.)"));
    assert!(
        system.contains("eludite-file-edit") && system.contains("eludite-workspace-apply_edit")
    );
    println!("system prompt: {} bytes, ~{estimate} tokens", system.len());
}

#[test]
fn core_tools_with_eludite_tools_and_enabling_more() {
    let mut r = rig("core", &[], &[]);
    r.server.push(tool_reply(
        "t1",
        "eludite-tools",
        "{\"prefix\": \"eludite-debug-\"}",
        10,
    ));
    r.server.push(tool_reply(
        "t2",
        "eludite-tools",
        "{\"enable\": [\"eludite-debug-evaluate\"]}",
        10,
    ));
    r.server.push(text_reply("ok", 10, 1));
    r.a.prompt(&r.session, "debug it");
    let chats = r.server.chats();
    let mut core: Vec<String> = CORE_PRESENT
        .iter()
        .map(|(c, _)| c.replace('.', "-"))
        .collect();
    core.push("eludite-tools".into());
    assert_eq!(names(&chats[0]["tools"]), core);
    let estimate = chats[0].to_string().len().div_ceil(4);
    assert!(
        estimate < 12_000,
        "core request {estimate} estimated tokens"
    );
    let listing = chats[1]["messages"][3]["content"].as_str().unwrap();
    assert!(
        listing.contains("eludite-debug-evaluate: Runs eludite.debug.evaluate."),
        "{listing}"
    );
    assert!(listing.contains("eludite-debug-step_over"));
    assert!(!listing.contains("eludite-git-push"), "filtered by prefix");
    assert!(names(&chats[2]["tools"]).contains(&"eludite-debug-evaluate".to_owned()));
    assert!(!names(&chats[1]["tools"]).contains(&"eludite-debug-evaluate".to_owned()));
    assert!(
        r.mcp.calls().is_empty(),
        "the meta tool is the adapter's own"
    );
    // --tools all: everything, no meta tool.
    let mut all = rig("all", &["--tools", "all"], &[]);
    all.server.push(text_reply("ok", 1, 1));
    all.a.prompt(&all.session, "hi");
    let n = names(&all.server.chats()[0]["tools"]);
    assert_eq!(n.len(), eludite_tools().len());
    assert!(!n.contains(&"eludite-tools".to_owned()));
    let t = &all.server.chats()[0]["tools"][0]["function"]["parameters"];
    assert!(t.get("$schema").is_none() && t.get("title").is_none());
}

#[test]
fn malformed_arguments_unknown_tools_and_refusals_go_back_to_the_model() {
    let mut r = rig("bad", &[], &[]);
    r.mcp.set_handler(|name, _| {
        if name == "eludite-terminal-send" {
            text_result("permission denied: `eludite.terminal.send` is class execute and the person denied it", true)
        } else {
            text_result("fine", false)
        }
    });
    r.server.push(Reply::sse(vec![
        tool_frag(0, Some("c1"), Some("eludite-file-read"), "{\"path\": "),
        tool_frag(1, Some("c2"), Some("eludite-made-up"), "{}"),
        tool_frag(
            2,
            Some("c3"),
            Some("eludite-terminal-send"),
            "{\"text\": \"rm -rf /\"}",
        ),
        finish("tool_calls"),
    ]));
    r.server.push(text_reply("Sorry.", 10, 1));
    r.a.prompt(&r.session, "try");
    let calls = r.mcp.calls();
    assert_eq!(calls.len(), 1, "only the well-formed, known call ran");
    assert_eq!(calls[0].name, "eludite-terminal-send");
    let m = r.server.chats()[1]["messages"].clone();
    assert_eq!(
        m[2]["tool_calls"][0]["function"]["arguments"], "{}",
        "malformed arguments replayed as {{}}"
    );
    assert!(
        m[3]["content"].as_str().unwrap().contains("not valid JSON"),
        "{}",
        m[3]
    );
    assert!(
        m[4]["content"]
            .as_str()
            .unwrap()
            .contains("There is no tool named \"eludite-made-up\"")
    );
    assert!(
        m[5]["content"]
            .as_str()
            .unwrap()
            .starts_with("permission denied")
    );
    let failed: Vec<String> =
        r.a.updates_of("tool_call_update")
            .iter()
            .filter(|u| u["status"] == "failed")
            .map(|u| u["toolCallId"].as_str().unwrap().to_owned())
            .collect();
    assert_eq!(failed, ["c1", "c2", "c3"]);
}

#[test]
fn images_ride_the_next_user_message_and_the_tool_list_follows_list_changed() {
    let mut r = rig("image", &["--tools", "all"], &[]);
    r.mcp.set_handler(|_, _| {
        json!({"content": [{"type": "text", "text": "{\"image\":\"(image content)\"}"},
            {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}], "isError": false})
    });
    r.server
        .push(tool_reply("s1", "eludite-browser-navigate", "{}", 10));
    r.server.push(text_reply("I see it.", 10, 1));
    r.a.prompt(&r.session, "look");
    let m = r.server.chats()[1]["messages"].clone();
    assert!(
        m[3]["content"]
            .as_str()
            .unwrap()
            .contains("image(s) from this tool follow")
    );
    assert_eq!(m[4]["role"], "user");
    assert_eq!(
        m[4]["content"][2]["image_url"]["url"],
        "data:image/png;base64,iVBORw0KGgo="
    );
    // A command registered at runtime appears on the next request.
    assert_eq!(r.mcp.lists(), 1);
    r.mcp
        .add_tool(fake_mcp::tool("eludite.web.new_thing", "read"));
    std::thread::sleep(Duration::from_millis(300));
    r.server.push(text_reply("ok", 1, 1));
    r.a.prompt(&r.session, "again");
    assert_eq!(r.mcp.lists(), 2);
    assert!(names(&r.server.chats()[2]["tools"]).contains(&"eludite-web-new_thing".to_owned()));
}

#[test]
fn an_http_mcp_server_works_too() {
    let server = FakeServer::start();
    server.llama_models(&["m"], 0);
    let mcp = FakeMcp::start(eludite_tools());
    mcp.set_guide("eludite://guides/git", "Stage with eludite-git-stage.");
    let mut a = Adapter::spawn(&["--base-url", &server.url], &[]);
    let session = a.start(&temp_dir("http"), vec![mcp.http_server()]);
    server.push(tool_reply("h1", "eludite-git-status", "{}", 10));
    server.push(text_reply("clean", 10, 1));
    assert_eq!(stop_reason(&a.prompt(&session, "status?")), "end_turn");
    assert_eq!(mcp.calls()[0].name, "eludite-git-status");
    assert_eq!(mcp.calls()[0].tool_call_id.as_deref(), Some("h1"));
    let system = server.chats()[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(system.contains("# Using git\n\nStage with eludite-git-stage."));
    assert!(
        server.chats()[1]["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("eludite-git-status ran")
    );
}

/// What a recorded stream says: its text, its reasoning and its tool calls' names (parsed here, independently of
/// the adapter's parser).
fn expected(sse: &str) -> (String, String, Vec<String>) {
    let (mut text, mut thought, mut tools) = (String::new(), String::new(), Vec::new());
    for line in sse.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let d = &v["choices"][0]["delta"];
        text += d["content"].as_str().unwrap_or("");
        thought += d["reasoning_content"]
            .as_str()
            .or(d["reasoning"].as_str())
            .unwrap_or("");
        for c in d["tool_calls"].as_array().into_iter().flatten() {
            if let Some(n) = c["function"]["name"].as_str() {
                tools.push(n.to_owned());
            }
        }
    }
    (text, thought, tools)
}

#[test]
fn stream_fixtures_replay() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("sse") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let sse = std::fs::read_to_string(&path).unwrap();
        let models: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(format!("{name}.models.json"))).unwrap(),
        )
        .unwrap();
        let mut r = rig_with(&name, &[], &[], |s| {
            s.set_models(Reply::json(200, models.clone()))
        });
        r.server.push(Reply::Sse {
            events: vec![Ev::Raw(sse.clone())],
            end: End::NoDone,
        });
        r.server.set_fallback(text_reply("after the tools", 1, 1));
        let resp = r.a.prompt(&r.session, "replay");
        assert_eq!(stop_reason(&resp), "end_turn", "{name}: {resp}");
        let (text, thought, tools) = expected(&sse);
        let shown = r.a.message_text();
        assert!(
            shown.starts_with(&text),
            "{name}: {shown:?} does not start with {text:?}"
        );
        let thoughts: String =
            r.a.updates_of("agent_thought_chunk")
                .iter()
                .map(|u| u["content"]["text"].as_str().unwrap().to_owned())
                .collect();
        assert_eq!(thoughts, thought, "{name}");
        let called: Vec<String> = r.mcp.calls().iter().map(|c| c.name.clone()).collect();
        assert_eq!(called, tools, "{name}");
        let usage = &r.a.updates_of("usage_update")[0];
        assert!(
            usage["used"].as_u64().unwrap() > 0,
            "{name}: the stream's usage"
        );
        let window = models["data"][0]["meta"]["n_ctx_train"]
            .as_u64()
            .or(models["data"][0]["context_length"].as_u64())
            .unwrap_or(0);
        assert_eq!(usage["size"].as_u64(), Some(window), "{name}");
        seen += 1;
    }
    assert!(seen >= 3, "the fixtures were found");
}
