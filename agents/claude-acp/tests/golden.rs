//! The full claude -> ACP mapping of the recorded session, pinned as a golden
//! file: every `session/update` the adapter derives from each recorded
//! message, and each turn's end. Regenerate deliberately with
//! `UPDATE_GOLDEN=1 cargo test --test golden` after re-recording, and review
//! the diff.

use std::path::PathBuf;

use eludite_claude_acp::translate::{Translator, TurnEnd};
use serde_json::{Value, json};

const CWD: &str = "/work";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn mapped(name: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    let mut tr = Translator::new(CWD.into());
    let mut cancelled = false;
    let mut out = Vec::new();
    let mut updates = Vec::new();
    for line in text.lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        let m = serde_json::to_string(&rec["m"])
            .unwrap()
            .replace("{{CWD}}", CWD)
            .replace("{{SESSION_ID}}", "session-1");
        let m: Value = serde_json::from_str(&m).unwrap();
        match rec["dir"].as_str() {
            Some("in") if m["type"] == "user" => {
                tr.begin_turn();
                cancelled = false;
                out.push(json!({"prompt": m["message"]["content"][0]["text"]}));
            }
            Some("in") if m["request"]["subtype"] == "interrupt" => {
                cancelled = true;
                out.push(json!({"cancel": true}));
            }
            Some("out") => {
                let end = tr.on_message(&m, cancelled, &mut updates);
                for u in updates.drain(..) {
                    out.push(json!({"from": m["type"], "update": u}));
                }
                if let Some(end) = end {
                    out.push(json!({"turn_end": match end {
                        TurnEnd::Stop(r) => serde_json::to_value(r).unwrap(),
                        TurnEnd::AuthRequired => json!("auth_required"),
                        TurnEnd::Error(e) => json!({"error": e}),
                    }}));
                }
            }
            _ => {}
        }
    }
    out
}

fn check(fixture_name: &str, golden_name: &str) {
    let got = mapped(fixture_name);
    let golden = fixture(golden_name);
    let rendered: String = got.iter().map(|v| format!("{v}\n")).collect();
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(&golden, &rendered).unwrap();
    }
    let want = std::fs::read_to_string(&golden).expect("golden file (UPDATE_GOLDEN=1 to create)");
    for (i, (g, w)) in rendered.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "line {} of {golden_name}", i + 1);
    }
    assert_eq!(rendered.lines().count(), want.lines().count());
}

#[test]
fn recorded_session_maps_exactly() {
    check(
        "claude-2.1.287-session.jsonl",
        "claude-2.1.287-session.acp.jsonl",
    );
    let got = mapped("claude-2.1.287-session.jsonl");
    let ends: Vec<&Value> = got.iter().filter_map(|v| v.get("turn_end")).collect();
    assert_eq!(
        ends,
        [&json!("end_turn"), &json!("end_turn"), &json!("cancelled")]
    );
    let kinds = |k: &str| {
        got.iter()
            .filter(|v| v["update"]["sessionUpdate"] == k)
            .count()
    };
    // 1 text delta per chunk: 5 + 58 + 14 + 4 (before and after the interrupt).
    assert_eq!(kinds("agent_message_chunk"), 81);
    assert_eq!(kinds("tool_call"), 3);
    assert_eq!(kinds("tool_call_update"), 6);
}

#[test]
fn each_recorded_turn_reports_its_usage() {
    // Brief 0034: every `result` is preceded by a `usage_update` with the turn's counts as Claude Code's stream gives
    // them; the logged-out turn has none.
    let got = mapped("claude-2.1.287-session.jsonl");
    let usage: Vec<&Value> = got
        .iter()
        .filter(|v| v["update"]["sessionUpdate"] == "usage_update")
        .map(|v| &v["update"])
        .collect();
    assert_eq!(usage.len(), 3);
    for (i, v) in got.iter().enumerate() {
        if v.get("turn_end").is_some() {
            assert_eq!(got[i - 1]["update"]["sessionUpdate"], "usage_update");
            assert_eq!(got[i - 1]["from"], "result");
        }
    }
    let first = usage[0];
    // The first turn: two model calls; the last one had 32 + 22,228 + 402 tokens in context.
    assert_eq!(first["used"], 32 + 22_228 + 402);
    assert_eq!(first["size"], 1_000_000);
    assert_eq!(
        first["cost"],
        json!({"amount": 0.26172724999999997, "currency": "USD"})
    );
    assert_eq!(
        first["_meta"]["claudeCode"]["usage"],
        json!({
            "inputTokens": 66, "cachedReadTokens": 55_789, "cachedWriteTokens": 10_821,
            "outputTokens": 614, "thoughtTokens": 57, "totalTokens": 66 + 55_789 + 10_821 + 614,
            "model": "claude-fable-5-1"
        })
    );
    // The cost is the session's running total, as ACP's `cost` is.
    assert_eq!(usage[1]["cost"]["amount"], 0.30284075);
    assert_eq!(usage[1]["_meta"]["claudeCode"]["usage"]["inputTokens"], 34);
    // The interrupted turn: no tokens, the same running cost.
    assert_eq!(usage[2]["_meta"]["claudeCode"]["usage"]["totalTokens"], 0);
    let out = mapped("claude-2.1.287-logged-out.jsonl");
    assert!(
        !out.iter()
            .any(|v| v["update"]["sessionUpdate"] == "usage_update")
    );
}

#[test]
fn logged_out_session_maps_exactly() {
    check(
        "claude-2.1.287-logged-out.jsonl",
        "claude-2.1.287-logged-out.acp.jsonl",
    );
    let got = mapped("claude-2.1.287-logged-out.jsonl");
    assert_eq!(got.last().unwrap()["turn_end"], "auth_required");
}
