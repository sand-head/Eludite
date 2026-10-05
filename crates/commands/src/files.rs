//! `eludite.file.read` and `eludite.file.edit` (brief 0060): the two file commands an agent loop needs, visible to
//! every agent (Claude Code's too).
//!
//! `file.read` (class `read`) answers a file's lines with their numbers, from the open document when the file is
//! open (unsaved text included) else from disk, at most [`MAX_LINES`] lines or [`MAX_BYTES`] per call. `file.edit`
//! (class `edit_buffer`) replaces text that must occur exactly once (or every occurrence with `replaceAll`) and
//! hands the result to `eludite.workspace.apply_edit` as one workspace edit, so an agent's edit is a pending change
//! the person reviews like any other.
//!
//! The schemas are `protocol/schemas/file-read.*.json` and `file-edit.*.json`. The logic is here and tested here;
//! the shell implements [`FilesTarget`] (the workspace folder, open documents' text, and the applier) and calls
//! [`register`].

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const FILE_READ: &str = "eludite.file.read";
pub const FILE_EDIT: &str = "eludite.file.edit";

/// Every command this module registers.
pub const ALL: [&str; 2] = [FILE_READ, FILE_EDIT];

/// The most lines one `file.read` returns.
pub const MAX_LINES: usize = 2_000;
/// The most bytes of text one `file.read` returns.
pub const MAX_BYTES: usize = 256 * 1024;
/// The prefix of a file looked at for NUL bytes.
const BINARY_SNIFF: usize = 8 * 1024;

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    match id {
        FILE_READ => (
            "File: Read",
            include_str!("../../../protocol/schemas/file-read.input.json"),
            include_str!("../../../protocol/schemas/file-read.output.json"),
            PermissionClass::Read,
        ),
        FILE_EDIT => (
            "File: Edit",
            include_str!("../../../protocol/schemas/file-edit.input.json"),
            include_str!("../../../protocol/schemas/file-edit.output.json"),
            PermissionClass::EditBuffer,
        ),
        other => unreachable!("not a file command: {other}"),
    }
}

/// The public description of file command `id` (one of [`ALL`]). Both are agent-visible.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// What the shell supplies. Called on the invoking thread.
pub trait FilesTarget: Send + Sync {
    /// The open workspace's folder; `None` without one.
    fn workspace_root(&self) -> Option<PathBuf>;
    /// The open document's text for `path` (unsaved changes included); `None` when it is not open.
    fn open_text(&self, path: &Path) -> Option<String>;
    /// Apply `input` (`workspace-apply-edit.input.json`) exactly as `eludite.workspace.apply_edit` does for the
    /// current caller (for an agent: held as a pending change), answering its output.
    fn apply_edit(&self, input: Value) -> Result<Value, CommandError>;
}

/// `file-read.input.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadRequest {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

/// `file-edit.input.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// Where the text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    Buffer,
    Disk,
}

/// `file-read.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadOutput {
    pub path: String,
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_line: Option<usize>,
    pub source: TextSource,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outside_workspace: bool,
}

fn invalid(e: impl std::fmt::Display) -> CommandError {
    CommandError::InvalidInput(e.to_string())
}

/// Parse `file.read`'s input.
pub fn parse_read(value: Value) -> Result<ReadRequest, CommandError> {
    let r: ReadRequest = serde_json::from_value(value).map_err(invalid)?;
    if r.path.is_empty() {
        return Err(invalid("`path` must not be empty"));
    }
    if r.start_line == Some(0) || r.end_line == Some(0) {
        return Err(invalid("lines are 1-based"));
    }
    if let (Some(s), Some(e)) = (r.start_line, r.end_line)
        && e < s
    {
        return Err(invalid("`endLine` is before `startLine`"));
    }
    Ok(r)
}

/// Parse `file.edit`'s input.
pub fn parse_edit(value: Value) -> Result<EditRequest, CommandError> {
    let r: EditRequest = serde_json::from_value(value).map_err(invalid)?;
    if r.path.is_empty() {
        return Err(invalid("`path` must not be empty"));
    }
    if r.old_text.is_empty() {
        return Err(invalid("`oldText` must not be empty"));
    }
    Ok(r)
}

/// `path` without `.` and with `..` applied, lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The absolute path for `path` and whether it is outside the workspace. A relative path is resolved against the
/// workspace folder and may not leave it.
pub fn resolve(root: Option<&Path>, path: &str) -> Result<(PathBuf, bool), CommandError> {
    let p = Path::new(path);
    let root = root.map(normalize);
    if p.is_absolute() {
        let abs = normalize(p);
        let outside = root.as_ref().is_none_or(|r| !abs.starts_with(r));
        return Ok((abs, outside));
    }
    let Some(root) = root else {
        return Err(CommandError::Failed(format!(
            "{path} is relative and no workspace is open; give an absolute path"
        )));
    };
    let abs = normalize(&root.join(p));
    if !abs.starts_with(&root) {
        return Err(CommandError::Failed(format!(
            "{path} leaves the workspace folder; give an absolute path to read outside it"
        )));
    }
    Ok((abs, false))
}

/// A file's text from disk, or why it cannot be read as text.
fn disk_text(path: &Path) -> Result<String, CommandError> {
    let bytes = std::fs::read(path)
        .map_err(|e| CommandError::Failed(format!("could not read {}: {e}", path.display())))?;
    decode(&bytes).ok_or_else(|| {
        CommandError::Failed(format!(
            "{} is a binary file ({} bytes)",
            path.display(),
            bytes.len()
        ))
    })
}

/// Text from bytes: UTF-8 (a BOM dropped), UTF-16 by its BOM; `None` for a binary file.
fn decode(bytes: &[u8]) -> Option<String> {
    let utf16 = |be: bool| -> Option<String> {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| {
                let pair = [c[0], *c.get(1).unwrap_or(&0)];
                if be {
                    u16::from_be_bytes(pair)
                } else {
                    u16::from_le_bytes(pair)
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xFF, 0xFE, ..] => return utf16(false),
        [0xFE, 0xFF, ..] => return utf16(true),
        _ => {}
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

/// The text to work on: the open document's, else the file's.
fn current_text(
    target: &dyn FilesTarget,
    path: &Path,
) -> Result<(String, TextSource), CommandError> {
    match target.open_text(path) {
        Some(t) => Ok((t, TextSource::Buffer)),
        None => Ok((disk_text(path)?, TextSource::Disk)),
    }
}

/// The lines of `text`: split on `\n`, a `\r` before it dropped; a final line break does not start a line.
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// `eludite.file.read`.
pub fn read(target: &dyn FilesTarget, req: &ReadRequest) -> Result<ReadOutput, CommandError> {
    let root = target.workspace_root();
    let (path, outside) = resolve(root.as_deref(), &req.path)?;
    let (text, source) = current_text(target, &path)?;
    let all = lines(&text);
    let total = all.len();
    let start = req.start_line.unwrap_or(1);
    if total == 0 {
        if start > 1 {
            return Err(CommandError::Failed(format!("{} is empty", path.display())));
        }
        return Ok(ReadOutput {
            path: path.to_string_lossy().into_owned(),
            text: String::new(),
            start_line: 1,
            end_line: 0,
            total_lines: 0,
            truncated: false,
            next_line: None,
            source,
            outside_workspace: outside,
        });
    }
    if start > total {
        return Err(CommandError::Failed(format!(
            "startLine {start} is past the end of {} ({total} lines)",
            path.display()
        )));
    }
    let wanted_end = req.end_line.unwrap_or(total).min(total);
    let mut out = String::new();
    let mut last = start - 1;
    for (i, line) in all.iter().enumerate().take(wanted_end).skip(start - 1) {
        let n = i + 1;
        if n - start >= MAX_LINES {
            break;
        }
        let row = format!("{n}\t{line}");
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + row.len() > MAX_BYTES {
            if out.is_empty() {
                // One line longer than the cap: its head.
                let mut end = MAX_BYTES;
                while !row.is_char_boundary(end) {
                    end -= 1;
                }
                out.push_str(&row[..end]);
                last = n;
            }
            break;
        }
        if sep == 1 {
            out.push('\n');
        }
        out.push_str(&row);
        last = n;
    }
    let truncated = last < wanted_end;
    Ok(ReadOutput {
        path: path.to_string_lossy().into_owned(),
        text: out,
        start_line: start,
        end_line: last,
        total_lines: total,
        truncated,
        next_line: truncated.then_some(last + 1),
        source,
        outside_workspace: outside,
    })
}

/// `file:///abs/path` with reserved and non-ASCII bytes percent-encoded (the shell's rule).
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The LSP position (0-based line, UTF-16 column) of byte `offset` in `text`.
fn position(text: &str, line_starts: &[usize], offset: usize) -> Value {
    let line = line_starts.partition_point(|&s| s <= offset) - 1;
    let character: usize = text[line_starts[line]..offset].encode_utf16().count();
    json!({"line": line, "character": character})
}

/// The text edits replacing `old` with `new` in `text`: exactly one occurrence, or all with `replace_all`. When
/// `old` is not found and the text uses CRLF, `old` and `new` are tried with CRLF line breaks.
pub fn text_edits(
    text: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    shown_path: &str,
) -> Result<Vec<Value>, CommandError> {
    if old == new {
        return Err(CommandError::Failed(
            "`newText` is the same as `oldText`".into(),
        ));
    }
    let crlf_old;
    let crlf_new;
    let (old, new) = if !text.contains(old) && text.contains("\r\n") && old.contains('\n') {
        crlf_old = old.replace("\r\n", "\n").replace('\n', "\r\n");
        crlf_new = new.replace("\r\n", "\n").replace('\n', "\r\n");
        (crlf_old.as_str(), crlf_new.as_str())
    } else {
        (old, new)
    };
    let offsets: Vec<usize> = text.match_indices(old).map(|(i, _)| i).collect();
    match offsets.len() {
        0 => {
            return Err(CommandError::Failed(format!(
                "`oldText` was not found in {shown_path}; read the file again and copy the text exactly"
            )));
        }
        1 => {}
        n if !replace_all => {
            return Err(CommandError::Failed(format!(
                "`oldText` occurs {n} times in {shown_path}; include more surrounding text so it occurs once, \
                 or set replaceAll"
            )));
        }
        _ => {}
    }
    let mut line_starts = vec![0];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    Ok(offsets
        .iter()
        .map(|&o| {
            json!({
                "range": {"start": position(text, &line_starts, o), "end": position(text, &line_starts, o + old.len())},
                "newText": new,
            })
        })
        .collect())
}

/// `eludite.file.edit`: the edit as `workspace-apply-edit.input.json`, applied through the target.
pub fn edit(target: &dyn FilesTarget, req: &EditRequest) -> Result<Value, CommandError> {
    let root = target.workspace_root();
    let (path, _) = resolve(root.as_deref(), &req.path)?;
    let (text, _) = current_text(target, &path)?;
    let edits = text_edits(
        &text,
        &req.old_text,
        &req.new_text,
        req.replace_all,
        &req.path,
    )?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| req.path.clone());
    let mut changes = serde_json::Map::new();
    changes.insert(path_to_uri(&path), Value::Array(edits));
    target.apply_edit(json!({"edit": {"changes": changes}, "label": format!("Edit {name}")}))
}

/// Register both commands, applying them through `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn FilesTarget>) {
    let t = target.clone();
    registry.replace(spec(FILE_READ), move |input| {
        let req = parse_read(input)?;
        Ok(serde_json::to_value(read(t.as_ref(), &req)?).expect("outputs serialize"))
    });
    registry.replace(spec(FILE_EDIT), move |input| {
        let req = parse_edit(input)?;
        edit(target.as_ref(), &req)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Fake {
        root: PathBuf,
        open: HashMap<PathBuf, String>,
        applied: Mutex<Vec<Value>>,
    }

    impl FilesTarget for Fake {
        fn workspace_root(&self) -> Option<PathBuf> {
            Some(self.root.clone())
        }
        fn open_text(&self, path: &Path) -> Option<String> {
            self.open.get(path).cloned()
        }
        fn apply_edit(&self, input: Value) -> Result<Value, CommandError> {
            self.applied.lock().unwrap().push(input);
            Ok(
                json!({"state": "applied", "applied": true, "files": 1, "edits": 1, "open_documents": 0,
                "files_on_disk": 1, "created": 0, "renamed": 0, "deleted": 0, "paths": []}),
            )
        }
    }

    fn workspace(name: &str) -> Fake {
        let root =
            std::env::temp_dir().join(format!("eludite-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Fake {
            root,
            open: HashMap::new(),
            applied: Mutex::default(),
        }
    }

    fn conforms(schema: &Value, value: &Value) {
        let obj = value.as_object().expect("an object");
        for r in schema["required"].as_array().unwrap() {
            assert!(obj.contains_key(r.as_str().unwrap()), "missing {r}");
        }
        for k in obj.keys() {
            assert!(schema["properties"].get(k).is_some(), "unexpected {k}");
        }
    }

    #[test]
    fn specs_classes_and_parsing() {
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
        }
        assert_eq!(spec(FILE_READ).permission, PermissionClass::Read);
        assert_eq!(spec(FILE_EDIT).permission, PermissionClass::EditBuffer);
        assert!(parse_read(json!({"path": "a", "startLine": 0})).is_err());
        assert!(parse_read(json!({"path": "a", "startLine": 5, "endLine": 2})).is_err());
        assert!(parse_read(json!({"path": "a", "x": 1})).is_err());
        assert!(parse_edit(json!({"path": "a", "oldText": "", "newText": "b"})).is_err());
        assert!(
            parse_edit(json!({"path": "a", "oldText": "x", "newText": "b", "replaceAll": true}))
                .unwrap()
                .replace_all
        );
    }

    #[test]
    fn read_an_open_buffer_with_unsaved_text() {
        let mut ws = workspace("buffer");
        let path = ws.root.join("a.rs");
        std::fs::write(&path, "on disk\n").unwrap();
        ws.open
            .insert(path.clone(), "fn main() {\r\n    unsaved();\r\n}\n".into());
        let out = read(&ws, &parse_read(json!({"path": "a.rs"})).unwrap()).unwrap();
        assert_eq!(out.source, TextSource::Buffer);
        assert_eq!(out.text, "1\tfn main() {\n2\t    unsaved();\n3\t}");
        assert_eq!(
            (out.start_line, out.end_line, out.total_lines, out.truncated),
            (1, 3, 3, false)
        );
        assert!(!out.outside_workspace);
        conforms(
            &spec(FILE_READ).output_schema,
            &serde_json::to_value(&out).unwrap(),
        );
    }

    #[test]
    fn read_a_line_range_the_cap_and_refusals() {
        let ws = workspace("range");
        let text: String = (1..=5_000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(ws.root.join("big.txt"), &text).unwrap();
        let out = read(
            &ws,
            &parse_read(json!({"path": "big.txt", "startLine": 10, "endLine": 12})).unwrap(),
        )
        .unwrap();
        assert_eq!(out.text, "10\tline 10\n11\tline 11\n12\tline 12");
        assert_eq!(out.source, TextSource::Disk);
        assert!(!out.truncated && out.next_line.is_none());
        // The 2,000-line cap.
        let out = read(&ws, &parse_read(json!({"path": "big.txt"})).unwrap()).unwrap();
        assert_eq!(
            (out.end_line, out.total_lines, out.truncated, out.next_line),
            (2_000, 5_000, true, Some(2_001))
        );
        assert_eq!(out.text.lines().count(), MAX_LINES);
        let out = read(
            &ws,
            &parse_read(json!({"path": "big.txt", "startLine": 4_500})).unwrap(),
        )
        .unwrap();
        assert_eq!((out.end_line, out.truncated), (5_000, false));
        conforms(
            &spec(FILE_READ).output_schema,
            &serde_json::to_value(&out).unwrap(),
        );
        // The 256 KB cap.
        let wide: String = (0..400)
            .map(|_| format!("{}\n", "x".repeat(1_000)))
            .collect();
        std::fs::write(ws.root.join("wide.txt"), &wide).unwrap();
        let out = read(&ws, &parse_read(json!({"path": "wide.txt"})).unwrap()).unwrap();
        assert!(out.truncated && out.text.len() <= MAX_BYTES);
        assert_eq!(out.next_line, Some(out.end_line + 1));
        // A binary file is refused with its size.
        std::fs::write(ws.root.join("b.bin"), [1u8, 0, 2, 3]).unwrap();
        let e = read(&ws, &parse_read(json!({"path": "b.bin"})).unwrap())
            .unwrap_err()
            .to_string();
        assert!(e.contains("is a binary file (4 bytes)"), "{e}");
        // Leaving the workspace with a relative path is refused; an absolute path outside is read and flagged.
        let e = read(&ws, &parse_read(json!({"path": "../x"})).unwrap())
            .unwrap_err()
            .to_string();
        assert!(e.contains("leaves the workspace folder"), "{e}");
        let outside =
            std::env::temp_dir().join(format!("eludite-files-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "hello\n").unwrap();
        let out = read(
            &ws,
            &parse_read(json!({"path": outside.to_string_lossy()})).unwrap(),
        )
        .unwrap();
        assert!(out.outside_workspace);
        assert!(
            read(
                &ws,
                &parse_read(json!({"path": "big.txt", "startLine": 9_000})).unwrap()
            )
            .is_err()
        );
        std::fs::write(ws.root.join("empty.txt"), "").unwrap();
        let out = read(&ws, &parse_read(json!({"path": "empty.txt"})).unwrap()).unwrap();
        assert_eq!(
            (out.total_lines, out.end_line, out.text.as_str()),
            (0, 0, "")
        );
    }

    #[test]
    fn edit_needs_exactly_one_occurrence_or_replace_all() {
        let ws = workspace("edit");
        let path = ws.root.join("a.cs");
        std::fs::write(&path, "int x = 1;\nint y = 1;\n// é𝄞 x = 1;\n").unwrap();
        let e = |old: &str, all: bool| {
            edit(
                &ws,
                &parse_edit(
                    json!({"path": "a.cs", "oldText": old, "newText": "Z", "replaceAll": all}),
                )
                .unwrap(),
            )
        };
        // Zero occurrences.
        let err = e("nope", false).unwrap_err().to_string();
        assert!(err.contains("was not found in a.cs"), "{err}");
        // Two occurrences.
        let err = e("= 1;", false).unwrap_err().to_string();
        assert!(err.contains("occurs 3 times in a.cs"), "{err}");
        assert!(ws.applied.lock().unwrap().is_empty(), "nothing applied");
        // One occurrence: one apply_edit with the matching range.
        let out = e("int y", false).unwrap();
        assert_eq!(out["state"], "applied");
        let applied = ws.applied.lock().unwrap().pop().unwrap();
        let uri = path_to_uri(&path);
        let edits = &applied["edit"]["changes"][&uri];
        assert_eq!(edits.as_array().unwrap().len(), 1);
        assert_eq!(
            edits[0]["range"],
            json!({"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}})
        );
        assert_eq!(edits[0]["newText"], "Z");
        assert_eq!(applied["label"], "Edit a.cs");
        // replaceAll: every occurrence, UTF-16 columns after astral characters.
        e("x = 1", true).unwrap();
        let applied = ws.applied.lock().unwrap().pop().unwrap();
        let edits = applied["edit"]["changes"][&uri].as_array().unwrap().clone();
        assert_eq!(edits.len(), 2);
        assert_eq!(
            edits[1]["range"]["start"],
            json!({"line": 2, "character": 7}),
            "é is 1 unit, 𝄞 is 2"
        );
        assert!(e("x", false).unwrap_err().to_string().contains("occurs"));
        assert!(
            edit(
                &ws,
                &parse_edit(
                    json!({"path": "a.cs", "oldText": "int", "newText": "int", "replaceAll": true})
                )
                .unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn edit_matches_the_open_buffer_and_crlf() {
        let mut ws = workspace("crlf");
        let path = ws.root.join("w.txt");
        std::fs::write(&path, "a\r\nb\r\nc\r\n").unwrap();
        edit(
            &ws,
            &parse_edit(json!({"path": "w.txt", "oldText": "a\nb", "newText": "A\nB"})).unwrap(),
        )
        .unwrap();
        let applied = ws.applied.lock().unwrap().pop().unwrap();
        let ed = &applied["edit"]["changes"][path_to_uri(&path)][0];
        assert_eq!(ed["newText"], "A\r\nB");
        assert_eq!(ed["range"]["end"], json!({"line": 1, "character": 1}));
        ws.open.insert(path.clone(), "only in the buffer\n".into());
        edit(
            &ws,
            &parse_edit(
                json!({"path": path.to_string_lossy(), "oldText": "buffer", "newText": "tab"}),
            )
            .unwrap(),
        )
        .unwrap();
        let reg = CommandRegistry::new();
        register(&reg, Arc::new(ws));
        let out = reg.invoke(FILE_READ, json!({"path": "w.txt"})).unwrap();
        assert_eq!(out["source"], "buffer");
        assert!(reg.invoke(FILE_EDIT, json!({"path": "w.txt"})).is_err());
    }

    #[test]
    fn uris() {
        assert_eq!(path_to_uri(Path::new("/w/a b.rs")), "file:///w/a%20b.rs");
        assert_eq!(path_to_uri(Path::new("C:\\w\\a.rs")), "file:///C:/w/a.rs");
        let root = std::env::temp_dir().join("w");
        assert_eq!(
            resolve(Some(&root), "./src/../a.rs").unwrap(),
            (root.join("a.rs"), false)
        );
        assert!(resolve(None, "a.rs").is_err());
        assert!(
            resolve(None, &root.join("a.rs").to_string_lossy())
                .unwrap()
                .1
        );
    }
}
