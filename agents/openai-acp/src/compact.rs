//! Keeping the conversation inside the model's window.
//!
//! Before each request, tool results older than the last [`TRIM_KEEP`] are cut to their first [`TRIM_CHARS`]
//! characters ([`trim_tool_results`]). When a response's `prompt_tokens` passes [`THRESHOLD`] of a known window,
//! or the server answers a context-length error, the turn asks the model (one request, no tools) for a summary of
//! everything but the last [`KEEP_LAST`] messages ([`summary_request`]) and replaces them with one `user` message
//! holding it ([`apply_summary`]). `/compact` does the same on demand.

use serde_json::{Value, json};

/// Compact when the prompt passes this share of the window.
pub const THRESHOLD: f64 = 0.85;
/// Messages kept verbatim after a compaction.
pub const KEEP_LAST: usize = 4;
/// Tool results kept whole.
pub const TRIM_KEEP: usize = 10;
/// What an older tool result is cut to.
pub const TRIM_CHARS: usize = 2_000;

/// The fixed instruction of the summary request.
pub const INSTRUCTION: &str = "You summarize a coding agent's conversation so the agent can continue the task \
with less context. Write a concise summary in plain text: the person's requests and constraints, what was done \
(files read and changed, commands and tools run, their results and errors), decisions made, and what remains to \
do. Keep exact file paths, symbol names, error messages and numbers. Do not add anything that is not in the \
conversation.";

/// The prefix of the message that replaces the summarized part.
pub const SUMMARY_PREFIX: &str =
    "Summary of the conversation so far (older messages were compacted):";

/// Whether `prompt_tokens` passed the threshold of a known `window`.
pub fn should_compact(prompt_tokens: u64, window: u64) -> bool {
    window > 0 && prompt_tokens as f64 > THRESHOLD * window as f64
}

/// Estimated tokens of `messages` (bytes of their JSON / 4).
pub fn estimate_tokens(messages: &[Value]) -> u64 {
    messages
        .iter()
        .map(|m| m.to_string().len() as u64)
        .sum::<u64>()
        .div_ceil(4)
}

/// Cut every tool result but the last [`TRIM_KEEP`] to [`TRIM_CHARS`] characters. Returns how many were cut.
pub fn trim_tool_results(messages: &mut [Value]) -> usize {
    let tools: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m["role"] == "tool")
        .map(|(i, _)| i)
        .collect();
    let mut cut = 0;
    for &i in tools.iter().rev().skip(TRIM_KEEP) {
        let Some(text) = messages[i]["content"].as_str() else {
            continue;
        };
        let total = text.chars().count();
        if total <= TRIM_CHARS
            || (text.ends_with(" more characters]") && text.contains("\n[trimmed: "))
        {
            continue;
        }
        let head: String = text.chars().take(TRIM_CHARS).collect();
        messages[i]["content"] = json!(format!(
            "{head}\n[trimmed: {} more characters]",
            total - TRIM_CHARS
        ));
        cut += 1;
    }
    cut
}

/// Where the kept tail starts: the last [`KEEP_LAST`] messages, moved back so it never starts with a `tool`
/// message (its assistant call would be summarized away). `None` when there is nothing before it to summarize.
pub fn split_point(messages: &[Value]) -> Option<usize> {
    let mut at = messages.len().saturating_sub(KEEP_LAST);
    while at > 0 && messages[at]["role"] == "tool" {
        at -= 1;
    }
    (at > 0).then_some(at)
}

fn content_text(m: &Value) -> String {
    match &m["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The messages as plain text for the summary request.
pub fn render_transcript(messages: &[Value]) -> String {
    let mut out = String::new();
    for m in messages {
        let text = content_text(m);
        match m["role"].as_str().unwrap_or("") {
            "user" => out.push_str(&format!("User: {text}\n\n")),
            "assistant" => {
                if !text.is_empty() {
                    out.push_str(&format!("Assistant: {text}\n\n"));
                }
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "Assistant called {} with {}\n\n",
                        c["function"]["name"].as_str().unwrap_or("?"),
                        c["function"]["arguments"].as_str().unwrap_or("")
                    ));
                }
            }
            "tool" => {
                let head: String = text.chars().take(TRIM_CHARS).collect();
                let more = if head.len() < text.len() {
                    " [...]"
                } else {
                    ""
                };
                out.push_str(&format!("Tool result: {head}{more}\n\n"));
            }
            _ => {}
        }
    }
    out
}

/// The summary request's messages for `older` (no tools are sent with it).
pub fn summary_request(older: &[Value]) -> Vec<Value> {
    vec![
        json!({"role": "system", "content": INSTRUCTION}),
        json!({"role": "user", "content": format!("Summarize this conversation:\n\n{}", render_transcript(older))}),
    ]
}

/// The conversation after a compaction: one `user` message with `summary`, then `messages[at..]`. A kept tail that
/// starts with a `user` message absorbs the summary, so roles still alternate.
pub fn apply_summary(messages: &[Value], at: usize, summary: &str) -> Vec<Value> {
    let head = format!("{SUMMARY_PREFIX}\n\n{}", summary.trim());
    let mut tail = messages[at..].to_vec();
    match tail.first_mut() {
        Some(first) if first["role"] == "user" && first["content"].is_string() => {
            let original = first["content"].as_str().unwrap_or("").to_owned();
            first["content"] = json!(format!("{head}\n\n---\n\n{original}"));
            tail
        }
        _ => {
            let mut out = vec![json!({"role": "user", "content": head})];
            out.extend(tail);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": "fix the bug"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "1", "type": "function", "function": {"name": "eludite-file-read", "arguments": "{\"path\":\"a.rs\"}"}}]}),
            json!({"role": "tool", "tool_call_id": "1", "content": "fn main() {}"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "2", "type": "function", "function": {"name": "diagnostics-list", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "2", "content": "[]"}),
            json!({"role": "assistant", "content": "Done."}),
            json!({"role": "user", "content": "thanks"}),
        ]
    }

    #[test]
    fn thresholds_and_split() {
        assert!(should_compact(14_000, 16_384));
        assert!(!should_compact(13_000, 16_384));
        assert!(!should_compact(1_000_000, 0), "unknown window");
        let c = conv();
        // The last four start with a tool message, so the split moves back to its call.
        assert_eq!(split_point(&c), Some(3));
        assert_eq!(split_point(&c[..3]), None);
        let out = apply_summary(&c, 3, "S");
        assert_eq!(out[0]["role"], "user");
        assert!(out[0]["content"].as_str().unwrap().ends_with("S"));
        assert_eq!(out[1]["tool_calls"][0]["id"], "2");
        // A tail starting with the user absorbs the summary.
        let out = apply_summary(&c, 6, "S");
        assert_eq!(out.len(), 1);
        assert!(
            out[0]["content"]
                .as_str()
                .unwrap()
                .ends_with("---\n\nthanks")
        );
        let t = render_transcript(&c[..3]);
        assert!(
            t.contains("User: fix the bug")
                && t.contains("called eludite-file-read")
                && t.contains("Tool result: fn main")
        );
        let req = summary_request(&c[..3]);
        assert_eq!(req[0]["content"], INSTRUCTION);
    }

    #[test]
    fn old_tool_results_are_trimmed() {
        let mut m: Vec<Value> = (0..12)
            .map(|i| json!({"role": "tool", "tool_call_id": i.to_string(), "content": "x".repeat(3_000)}))
            .collect();
        assert_eq!(trim_tool_results(&mut m), 2);
        assert!(
            m[0]["content"]
                .as_str()
                .unwrap()
                .ends_with("[trimmed: 1000 more characters]")
        );
        assert_eq!(m[2]["content"].as_str().unwrap().len(), 3_000);
        assert_eq!(trim_tool_results(&mut m), 0, "idempotent");
        assert!(estimate_tokens(&m) > 0);
    }
}
