//! Headless GPUI tests of brief 0066 against the in-process fake `eludite-host` and the scripted fake generic server:
//! Roslyn's tokens paint exact ranges in Visual Studio's colors, a delta follows an edit, a stale answer (an edit while
//! the server answers) is dropped and asked again, `eludite/semanticTokens/refresh` asks again, the settings turn the
//! layer off per language, unnecessary code fades, inlay hints are asked for the visible rows and filtered by kind,
//! their tooltip is resolved, Alt+F1's command hides and shows them, a rust-analyzer-style server's tokens and hints go
//! through the same path, and decoding a 10,000-line answer happens off the UI thread.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_commands::inlay_hints::TOGGLE;
use eludite_commands::settings::SET;
use eludite_commands::workspace;
use eludite_editor::{DecorationStyle, EditorView, SEMANTIC_DEBOUNCE, SemanticSpan};
use eludite_lsp::fake::{FakeHost, FakeReply};
use eludite_lsp::fake_server::{FakeServer, analyzer_capabilities};
use eludite_lsp::semantic::{Classifier, Legend};
use gpui::{Entity, TestAppContext, rgb};
use serde_json::{Value, json};

use super::semantic::{UNNECESSARY_LAYER, UNNECESSARY_OPACITY, build_update};
use super::test_runs_tests as tr;
use super::tests::{Ws, setup_services, setup_with};

/// Roslyn's "pure LSP" legend, abridged: the standard types first, then its custom names.
fn roslyn_legend() -> Value {
    json!({
        "tokenTypes": ["namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter",
                       "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword",
                       "modifier", "comment", "string", "number", "regexp", "operator", "decorator",
                       "controlKeyword", "excludedCode", "field", "recordClass", "stringEscapeCharacter"],
        "tokenModifiers": ["static", "ReassignedVariable", "deprecated"]
    })
}

fn host_caps() -> Value {
    json!({
        "semanticTokensProvider": {"legend": roslyn_legend(), "full": {"delta": true}, "range": true},
        "inlayHintProvider": {"resolveProvider": true},
        "hoverProvider": true
    })
}

/// Program.cs (`class Program` on row 0, `static void Main()` on row 2): `Program` is a class, `Main` a static method.
const PROGRAM_TOKENS: [u32; 10] = [0, 6, 7, 2, 0, 2, 16, 4, 13, 1];

fn spans(w: &Ws, view: &Entity<EditorView>, row: u32) -> Vec<SemanticSpan> {
    view.read_with(&w.vcx, |v, _| v.semantic_rows().spans(row).to_vec())
}

fn span_count(w: &Ws, view: &Entity<EditorView>) -> usize {
    view.read_with(&w.vcx, |v, _| v.semantic_rows().span_count())
}

fn set(w: &mut Ws, key: &str, value: Value) {
    w.commands
        .invoke(SET, json!({"key": key, "value": value}))
        .unwrap();
}

fn colored(start: u32, end: u32, color: u32) -> SemanticSpan {
    SemanticSpan {
        start,
        end,
        color: Some(rgb(color)),
        strikethrough: false,
    }
}

#[gpui::test]
fn roslyn_tokens_paint_visual_studio_colors_and_a_delta_follows_an_edit(cx: &mut TestAppContext) {
    let mut w = setup_with(cx, |fake: &FakeHost| {
        fake.set_language_server("running", Some(host_caps()));
        fake.respond("textDocument/semanticTokens/full", |_| {
            FakeReply::Result(json!({"resultId": "1", "data": PROGRAM_TOKENS}))
        });
        // After the edit (`int x;` typed into Main's body on row 2), one more token: `x`, a local.
        // Later deltas (a refresh) change nothing.
        fake.respond("textDocument/semanticTokens/full/delta", |p| {
            if p["previousResultId"] == "1" {
                FakeReply::Result(
                    json!({"resultId": "2", "edits": [{"start": 10, "deleteCount": 0, "data": [0, 11, 1, 8, 0]}]}),
                )
            } else {
                FakeReply::Result(json!({"resultId": "3", "edits": []}))
            }
        });
        fake.respond("textDocument/inlayHint", |_| FakeReply::Result(json!([])));
    });
    let (_, view) = w.open_program();
    w.wait("Roslyn's tokens", |w| span_count(w, &view) == 2);
    // Exact ranges in Visual Studio Dark's colors: the class name #4EC9B0, the method #DCDCAA.
    assert_eq!(spans(&w, &view, 0), [colored(6, 13, 0x4EC9B0)]);
    assert_eq!(spans(&w, &view, 2), [colored(16, 20, 0xDCDCAA)]);
    let first = &w.fake.received_params("textDocument/semanticTokens/full")[0];
    assert_eq!(first["eluditeGeneration"], w.fake.generation());

    // Type into Main's body: the spans move with the text at once, and the delta comes after the debounce.
    let body = w.text(&view).find("{ }").unwrap() + 2;
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(body))
    });
    w.vcx.simulate_input("int x; ");
    w.vcx.run_until_parked();
    assert_eq!(spans(&w, &view, 2), [colored(16, 20, 0xDCDCAA)]);
    w.vcx.executor().advance_clock(SEMANTIC_DEBOUNCE);
    w.wait("the delta applied", |w| span_count(w, &view) == 3);
    assert_eq!(
        w.fake
            .received_params("textDocument/semanticTokens/full/delta")
            .len(),
        1
    );
    assert_eq!(
        spans(&w, &view, 2),
        [colored(16, 20, 0xDCDCAA), colored(27, 28, 0x9CDCFE)]
    );
    let methods = w.shell.read_with(&w.vcx, |s, _| {
        s.semantic().docs.values().next().unwrap().methods.clone()
    });
    assert_eq!(
        methods,
        [
            "textDocument/semanticTokens/full",
            "textDocument/semanticTokens/full/delta"
        ]
    );

    // A refresh from Roslyn asks again; a setting turns the layer off for C# and the syntax colors stay.
    let before = w
        .fake
        .received_params("textDocument/semanticTokens/full/delta")
        .len();
    w.fake.notify(
        "eludite/semanticTokens/refresh",
        json!({"eluditeGeneration": w.fake.generation()}),
    );
    w.wait("the refresh's request", |w| {
        w.fake
            .received_params("textDocument/semanticTokens/full/delta")
            .len()
            > before
    });
    set(
        &mut w,
        "editor.languages.csharp.semanticHighlighting",
        json!("off"),
    );
    w.wait("the layer off", |w| span_count(w, &view) == 0);
    assert!(!view.read_with(&w.vcx, |v, _| v.semantic_highlighting_enabled()));
    // Back on, the whole document is asked for again (this fake's full answer is the one for the text before the edit).
    set(
        &mut w,
        "editor.languages.csharp.semanticHighlighting",
        json!("default"),
    );
    w.wait("the layer back", |w| span_count(w, &view) == 2);
    assert_eq!(
        w.fake
            .received_params("textDocument/semanticTokens/full")
            .len(),
        2
    );
}

#[gpui::test]
fn an_answer_for_text_changed_since_is_dropped_and_asked_again(cx: &mut TestAppContext) {
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(gate_rx));
    let calls = Arc::new(Mutex::new(0u32));
    let mut w = setup_with(cx, {
        let calls = calls.clone();
        move |fake: &FakeHost| {
            fake.set_language_server(
                "running",
                Some(json!({"semanticTokensProvider": {"legend": roslyn_legend(), "full": true}})),
            );
            fake.respond("textDocument/semanticTokens/full", move |_| {
                let n = {
                    let mut c = calls.lock().unwrap();
                    *c += 1;
                    *c
                };
                if n == 1 {
                    // The first answer waits until the test has edited the text.
                    let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(30));
                    FakeReply::Result(json!({"data": PROGRAM_TOKENS}))
                } else {
                    // On the edited text (`// ` before `class`), `Program` moved three columns right.
                    FakeReply::Result(json!({"data": [0, 9, 7, 2, 0]}))
                }
            });
        }
    });
    let (_, view) = w.open_program();
    w.wait("the first request", |w| {
        !w.fake
            .received_params("textDocument/semanticTokens/full")
            .is_empty()
    });
    view.update(&mut w.vcx, |v, cx| v.update_editor(cx, |e| e.set_caret(0)));
    w.vcx.simulate_input("// ");
    let id = w
        .shell
        .read_with(&w.vcx, |s, _| s.documents.keys().next().unwrap().clone());
    w.shell.update(&mut w.vcx, |s, cx| s.flush_change(&id, cx));
    gate_tx.send(()).unwrap();
    w.vcx.executor().advance_clock(SEMANTIC_DEBOUNCE);
    w.wait("the second answer", |w| span_count(w, &view) == 1);
    // The first answer (for the text before the edit) was never painted: only the second's span is there.
    assert_eq!(spans(&w, &view, 0), [colored(9, 16, 0x4EC9B0)]);
    assert_eq!(*calls.lock().unwrap(), 2);
    let applied = w.shell.read_with(&w.vcx, |s, _| s.semantic().timings.len());
    assert_eq!(applied, 1, "only the fresh answer was applied");
}

#[gpui::test]
fn unnecessary_code_fades(cx: &mut TestAppContext) {
    let mut w = setup_with(cx, |_| {});
    let (program, view) = w.open_program();
    let uri = super::documents::path_to_uri(&program);
    w.fake.publish_diagnostics(
        &uri,
        1,
        json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                "severity": 4, "code": "IDE0005", "message": "Using directive is unnecessary.", "tags": [1]}]),
    );
    w.wait("the faded range", |w| {
        view.read_with(&w.vcx, |v, _| !v.decorations(UNNECESSARY_LAYER).is_empty())
    });
    let style = view.read_with(&w.vcx, |v, _| v.decorations(UNNECESSARY_LAYER)[0].style);
    assert_eq!(style, DecorationStyle::Faded(UNNECESSARY_OPACITY));
}

fn hint_caps_script(fake: &FakeHost) {
    fake.set_language_server("running", Some(host_caps()));
    fake.respond("textDocument/semanticTokens/full", |_| {
        FakeReply::Result(json!({"resultId": "1", "data": []}))
    });
    // A type hint after `Program` (row 0) and a parameter hint inside Main's parentheses (row 2).
    fake.respond("textDocument/inlayHint", |_| {
        FakeReply::Result(json!([
            {"position": {"line": 0, "character": 13}, "label": ": object", "kind": 1, "paddingLeft": false},
            {"position": {"line": 2, "character": 21}, "label": "args:", "kind": 2, "paddingRight": true,
             "data": {"i": 1}}
        ]))
    });
    fake.respond("inlayHint/resolve", |p| {
        FakeReply::Result(json!({"position": p["position"], "label": p["label"], "kind": 2,
                                 "tooltip": {"kind": "markdown", "value": "string[] args"}, "data": p["data"]}))
    });
}

fn hint_labels(w: &Ws, view: &Entity<EditorView>) -> Vec<(usize, String)> {
    view.read_with(&w.vcx, |v, _| {
        v.inlay_hints()
            .into_iter()
            .map(|(o, h)| (o, h.label))
            .collect()
    })
}

#[gpui::test]
fn inlay_hints_are_asked_for_the_visible_rows_filtered_resolved_and_toggled(
    cx: &mut TestAppContext,
) {
    let mut w = setup_with(cx, hint_caps_script);
    let (program, view) = w.open_program();
    w.wait("two hints", |w| hint_labels(w, &view).len() == 2);
    let text = w.text(&view);
    assert_eq!(text, super::tests::PROGRAM, "hints are not text");
    let main_paren = text.find("Main(").unwrap() + 5;
    assert_eq!(
        hint_labels(&w, &view),
        [
            (13, ": object".to_owned()),
            (main_paren, "args:".to_owned())
        ]
    );
    let asked = &w.fake.received_params("textDocument/inlayHint")[0];
    assert_eq!(asked["range"]["start"]["line"], 0);
    assert_eq!(asked["eluditeGeneration"], w.fake.generation());

    // The tooltip of the parameter hint: resolved, then shown in Quick Info.
    let _ = &program;
    let id = w
        .shell
        .read_with(&w.vcx, |s, _| s.documents.keys().next().unwrap().clone());
    let hint_id = w.shell.read_with(&w.vcx, |s, _| {
        *s.inlay_hint_state().docs[&id]
            .hints
            .iter()
            .find(|(_, h)| h.kind == Some(2))
            .unwrap()
            .0
    });
    w.shell.update(&mut w.vcx, |s, cx| {
        s.inlay_hint_hovered(&id, hint_id, main_paren, cx)
    });
    w.wait("the tooltip", |w| {
        view.read_with(&w.vcx, |v, _| {
            v.hover()
                .and_then(|h| h.text)
                .is_some_and(|t| t.contains("string[] args"))
        })
    });
    assert_eq!(w.fake.received_params("inlayHint/resolve").len(), 1);
    assert_eq!(
        w.fake.received_params("inlayHint/resolve")[0]["data"]["i"],
        1
    );

    // Type hints off: only the parameter hint is left.
    set(&mut w, "editor.inlayHints.types", json!(false));
    w.wait("the type hint gone", |w| hint_labels(w, &view).len() == 1);
    assert_eq!(hint_labels(&w, &view)[0].1, "args:");

    // Alt+F1's command hides them for the session, and shows them again.
    let toggle = |w: &mut Ws, args: Value| {
        w.shell
            .update_in(&mut w.vcx, |s, window, cx| {
                s.invoke(TOGGLE, args, window, cx)
            })
            .unwrap()
    };
    let out = toggle(&mut w, json!({}));
    assert_eq!(out["enabled"], false);
    assert!(hint_labels(&w, &view).is_empty());
    let out = toggle(&mut w, json!({"enabled": true}));
    assert_eq!(out["enabled"], true);
    w.wait("the hints back", |w| hint_labels(w, &view).len() == 1);
    // The setting itself never changed.
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.inlay_hint_state().settings.switch.enabled)
    );
}

/// rust-analyzer's legend (abridged) and hints through the generic server path.
#[gpui::test]
fn a_rust_analyzer_style_server_paints_tokens_and_hints(cx: &mut TestAppContext) {
    let ra = FakeServer::new();
    let mut caps = analyzer_capabilities();
    caps["semanticTokensProvider"] = json!({
        "legend": {"tokenTypes": ["comment", "keyword", "function", "variable", "struct", "selfKeyword", "builtinType"],
                   "tokenModifiers": ["declaration", "mutable"]},
        "full": {"delta": true}, "range": true
    });
    caps["inlayHintProvider"] = json!({"resolveProvider": false});
    ra.set_capabilities(caps);
    let rs = std::env::temp_dir().join(format!("eludite-0066-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rs);
    tr::copy_rust_corpus(&rs);
    let lib = rs.join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    // The first `fn name` of the file: a function token, and a type hint after its first `let` binding if any.
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("fn "))
        .expect("a function");
    let col = (line.find("fn ").unwrap() + 3) as u32;
    let len = line[col as usize..]
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap() as u32;
    let row = row as u32;
    ra.respond("textDocument/semanticTokens/full", move |_| {
        FakeReply::Result(json!({"resultId": "a", "data": [row, col, len, 2, 1]}))
    });
    ra.respond("textDocument/inlayHint", move |_| {
        FakeReply::Result(json!([{"position": {"line": row, "character": col + len}, "label": ": i32", "kind": 1}]))
    });
    let connector = ra.connector();
    let mut w = setup_services(
        cx,
        |_| {},
        None,
        None,
        move |s| {
            s.launches
                .in_process
                .insert("rust-analyzer".into(), connector);
        },
    );
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::WORKSPACE_OPEN_FOLDER,
                json!({"path": rs.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::FILE_OPEN,
                json!({"path": lib.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    let view = w.editor(&lib);
    w.wait("rust-analyzer's token", |w| span_count(w, &view) == 1);
    assert_eq!(spans(&w, &view, row), [colored(col, col + len, 0xDCDCAA)]);
    w.wait("rust-analyzer's hint", |w| hint_labels(w, &view).len() == 1);
    assert_eq!(w.text(&view), text, "hints are not text");
    let _ = std::fs::remove_dir_all(&rs);
}

/// A 10,000-line document's answer (60,000 tokens) is decoded off the UI thread; the UI thread only installs the
/// rows.
#[test]
fn decoding_a_10000_line_answer_is_off_the_ui_thread_work() {
    let mut text = String::new();
    for i in 0..10_000 {
        text.push_str(&format!(
            "    var a{i:05} = new List<int>(); Use(a{i:05}, b, c);\n"
        ));
    }
    let buffer = eludite_editor::text::Buffer::new(
        eludite_editor::text::ReplicaId::LOCAL,
        eludite_editor::text::BufferId::new(1).unwrap(),
        text.as_str(),
    );
    let snapshot = buffer.snapshot().clone();
    let mut data = Vec::with_capacity(10_000 * 30);
    for i in 0..10_000u32 {
        // var (keyword), a… (variable), List (class), int (keyword), Use (method), a… (variable).
        let line = [
            (4, 3, 15),
            (4, 6, 8),
            (13, 4, 2),
            (5, 3, 15),
            (6, 3, 13),
            (4, 6, 8),
        ];
        for (k, (delta, len, ty)) in line.into_iter().enumerate() {
            data.extend([if k == 0 { u32::from(i > 0) } else { 0 }, delta, len, ty, 0]);
        }
    }
    let legend: Legend = Legend {
        token_types: roslyn_legend()["tokenTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_owned())
            .collect(),
        token_modifiers: vec![],
    };
    let classifier = Classifier::new(&legend);
    let colors = eludite_ui::Theme::dark().semantic;
    let t = Instant::now();
    let update = build_update(&snapshot, &data, &classifier, &colors, 0..10_001);
    let decode = t.elapsed();
    let tokens: usize = update.spans.iter().map(|(_, s)| s.len()).sum();
    assert_eq!(tokens, 60_000);
    eprintln!(
        "timing: decoding 60,000 tokens of a 10,000-line document {:.1} ms (on a background thread)",
        decode.as_secs_f64() * 1e3
    );
    eludite_test_support::assert_budget(
        "decoding a 10,000-line answer off the UI thread",
        decode,
        Duration::from_millis(500),
    );
}
