//! The document lifecycle and the tab well (brief 0070): File > Save All, Save As, New File, Open File..., Close All
//! and Exit (`eludite.file.*`), the open documents with their unsaved state and encodings, reload after an outside
//! change and Advanced Save Options; the tab well's commands (`eludite.window.*`: Pin Tab, the next document, the IDE
//! Navigator, Reopen Closed Tab, Copy Full Path, Open Containing Folder); bookmarks (`eludite.bookmarks.*`); and Go To
//! Last Edit Location (`eludite.editor.go_to_last_edit`).
//!
//! The schemas are `protocol/schemas/file-*.json`, `window-*.json`, `bookmarks-*.json` and
//! `editor-go-to-last-edit.*.json` (checked in first, CLAUDE.md invariant 4). This module parses input into a typed
//! [`DocumentsRequest`] and registers the commands; the shell implements [`DocumentsTarget`] and answers with JSON in
//! the output schema's shape. Commands that would ask the person (unsaved changes) take an explicit `save` argument
//! instead: agents never see a modal.

use std::sync::Arc;

use serde_json::Value;

use crate::workspace::CloseSave;
use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SAVE_ALL: &str = "eludite.file.save_all";
pub const SAVE_AS: &str = "eludite.file.save_as";
pub const NEW: &str = "eludite.file.new";
pub const OPEN_FILE_DIALOG: &str = "eludite.file.open_file_dialog";
pub const DOCUMENTS: &str = "eludite.file.documents";
pub const CLOSE_ALL: &str = "eludite.file.close_all";
pub const RELOAD: &str = "eludite.file.reload";
pub const SET_ENCODING: &str = "eludite.file.set_encoding";
pub const EXIT: &str = "eludite.file.exit";
pub const PIN_TAB: &str = "eludite.window.pin_tab";
pub const MOVE_TAB: &str = "eludite.window.move_tab";
pub const NEXT_DOCUMENT: &str = "eludite.window.next_document";
pub const NAVIGATOR: &str = "eludite.window.navigator";
pub const REOPEN_CLOSED: &str = "eludite.window.reopen_closed";
pub const COPY_FULL_PATH: &str = "eludite.window.copy_full_path";
pub const OPEN_CONTAINING_FOLDER: &str = "eludite.window.open_containing_folder";
pub const BOOKMARK_TOGGLE: &str = "eludite.bookmarks.toggle";
pub const BOOKMARK_NEXT: &str = "eludite.bookmarks.next";
pub const BOOKMARK_PREVIOUS: &str = "eludite.bookmarks.previous";
pub const BOOKMARK_CLEAR: &str = "eludite.bookmarks.clear";
pub const BOOKMARK_LIST: &str = "eludite.bookmarks.list";
pub const BOOKMARK_UPDATE: &str = "eludite.bookmarks.update";
pub const BOOKMARK_GO_TO: &str = "eludite.bookmarks.go_to";
pub const GO_TO_LAST_EDIT: &str = "eludite.editor.go_to_last_edit";

pub const ALL: [&str; 24] = [
    SAVE_ALL,
    SAVE_AS,
    NEW,
    OPEN_FILE_DIALOG,
    DOCUMENTS,
    CLOSE_ALL,
    RELOAD,
    SET_ENCODING,
    EXIT,
    PIN_TAB,
    MOVE_TAB,
    NEXT_DOCUMENT,
    NAVIGATOR,
    REOPEN_CLOSED,
    COPY_FULL_PATH,
    OPEN_CONTAINING_FOLDER,
    BOOKMARK_TOGGLE,
    BOOKMARK_NEXT,
    BOOKMARK_PREVIOUS,
    BOOKMARK_CLEAR,
    BOOKMARK_LIST,
    BOOKMARK_UPDATE,
    BOOKMARK_GO_TO,
    GO_TO_LAST_EDIT,
];

/// A line ending a document can be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndingChoice {
    CrLf,
    Lf,
    Cr,
}

/// A parsed, validated command. `path: None` means the active document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentsRequest {
    SaveAll,
    SaveAs {
        path: Option<String>,
        new_path: Option<String>,
        overwrite: bool,
    },
    New {
        language: Option<String>,
    },
    OpenFileDialog,
    Documents {
        dirty_only: bool,
    },
    CloseAll {
        except: Option<String>,
        keep_pinned: bool,
        save: Option<CloseSave>,
    },
    Reload {
        path: Option<String>,
        discard: bool,
    },
    SetEncoding {
        path: Option<String>,
        encoding: Option<String>,
        bom: Option<bool>,
        line_ending: Option<LineEndingChoice>,
        reopen: bool,
    },
    Exit {
        save: Option<CloseSave>,
    },
    PinTab {
        path: Option<String>,
        pinned: Option<bool>,
    },
    MoveTab {
        path: String,
        index: usize,
    },
    NextDocument {
        backward: bool,
    },
    Navigator {
        backward: bool,
        activate: Option<usize>,
    },
    ReopenClosed,
    CopyFullPath {
        path: Option<String>,
    },
    OpenContainingFolder {
        path: Option<String>,
    },
    BookmarkToggle {
        path: Option<String>,
        line: Option<u32>,
    },
    BookmarkNext,
    BookmarkPrevious,
    BookmarkClear {
        path: Option<String>,
    },
    BookmarkList,
    BookmarkUpdate {
        id: u64,
        name: Option<String>,
        enabled: Option<bool>,
    },
    BookmarkGoTo {
        id: u64,
    },
    GoToLastEdit,
}

impl DocumentsRequest {
    pub fn command(&self) -> &'static str {
        use DocumentsRequest::*;
        match self {
            SaveAll => SAVE_ALL,
            SaveAs { .. } => SAVE_AS,
            New { .. } => NEW,
            OpenFileDialog => OPEN_FILE_DIALOG,
            Documents { .. } => DOCUMENTS,
            CloseAll { .. } => CLOSE_ALL,
            Reload { .. } => RELOAD,
            SetEncoding { .. } => SET_ENCODING,
            Exit { .. } => EXIT,
            PinTab { .. } => PIN_TAB,
            MoveTab { .. } => MOVE_TAB,
            NextDocument { .. } => NEXT_DOCUMENT,
            Navigator { .. } => NAVIGATOR,
            ReopenClosed => REOPEN_CLOSED,
            CopyFullPath { .. } => COPY_FULL_PATH,
            OpenContainingFolder { .. } => OPEN_CONTAINING_FOLDER,
            BookmarkToggle { .. } => BOOKMARK_TOGGLE,
            BookmarkNext => BOOKMARK_NEXT,
            BookmarkPrevious => BOOKMARK_PREVIOUS,
            BookmarkClear { .. } => BOOKMARK_CLEAR,
            BookmarkList => BOOKMARK_LIST,
            BookmarkUpdate { .. } => BOOKMARK_UPDATE,
            BookmarkGoTo { .. } => BOOKMARK_GO_TO,
            GoToLastEdit => GO_TO_LAST_EDIT,
        }
    }
}

/// What applies the commands (the shell). Answers with JSON in the command's output schema.
pub trait DocumentsTarget: Send + Sync {
    fn apply(&self, request: DocumentsRequest) -> Result<Value, CommandError>;
}

/// The object's fields, refusing unknown ones.
struct Input<'a> {
    obj: &'a serde_json::Map<String, Value>,
}

impl<'a> Input<'a> {
    fn new(value: &'a Value, allowed: &[&str]) -> Result<Self, CommandError> {
        let Some(obj) = value.as_object() else {
            return Err(CommandError::InvalidInput("input must be an object".into()));
        };
        if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(CommandError::InvalidInput(format!("unknown field `{k}`")));
        }
        Ok(Self { obj })
    }

    fn get(&self, name: &str) -> Option<&'a Value> {
        self.obj.get(name).filter(|v| !v.is_null())
    }

    fn string(&self, name: &str) -> Result<Option<String>, CommandError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(CommandError::InvalidInput(format!(
                "`{name}` must be a non-empty string"
            ))),
        }
    }

    /// A string that may be empty (a bookmark's name).
    fn any_string(&self, name: &str) -> Result<Option<String>, CommandError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(CommandError::InvalidInput(format!(
                "`{name}` must be a string"
            ))),
        }
    }

    fn opt_bool(&self, name: &str) -> Result<Option<bool>, CommandError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(CommandError::InvalidInput(format!(
                "`{name}` must be a boolean"
            ))),
        }
    }

    fn flag(&self, name: &str) -> Result<bool, CommandError> {
        Ok(self.opt_bool(name)?.unwrap_or(false))
    }

    fn uint(&self, name: &str, min: u64) -> Result<Option<u64>, CommandError> {
        match self.get(name) {
            None => Ok(None),
            Some(v) => match v.as_u64() {
                Some(n) if n >= min => Ok(Some(n)),
                _ => Err(CommandError::InvalidInput(format!(
                    "`{name}` must be an integer of at least {min}"
                ))),
            },
        }
    }

    fn required_id(&self) -> Result<u64, CommandError> {
        self.uint("id", 1)?
            .ok_or_else(|| CommandError::InvalidInput("`id` is required".into()))
    }

    fn save(&self) -> Result<Option<CloseSave>, CommandError> {
        match self.get("save") {
            None => Ok(None),
            Some(Value::String(s)) if s == "save" => Ok(Some(CloseSave::Save)),
            Some(Value::String(s)) if s == "discard" => Ok(Some(CloseSave::Discard)),
            Some(_) => Err(CommandError::InvalidInput(
                "`save` must be \"save\" or \"discard\"".into(),
            )),
        }
    }
}

/// Parse and validate a command's input.
pub fn parse(id: &str, value: Value) -> Result<DocumentsRequest, CommandError> {
    use DocumentsRequest::*;
    let input = |allowed: &[&str]| Input::new(&value, allowed);
    Ok(match id {
        SAVE_ALL => {
            input(&[])?;
            SaveAll
        }
        SAVE_AS => {
            let i = input(&["path", "new_path", "overwrite"])?;
            SaveAs {
                path: i.string("path")?,
                new_path: i.string("new_path")?,
                overwrite: i.flag("overwrite")?,
            }
        }
        NEW => {
            let i = input(&["language"])?;
            New {
                language: i.string("language")?,
            }
        }
        OPEN_FILE_DIALOG => {
            input(&[])?;
            OpenFileDialog
        }
        DOCUMENTS => {
            let i = input(&["dirty_only"])?;
            Documents {
                dirty_only: i.flag("dirty_only")?,
            }
        }
        CLOSE_ALL => {
            let i = input(&["except", "keep_pinned", "save"])?;
            CloseAll {
                except: i.string("except")?,
                keep_pinned: i.flag("keep_pinned")?,
                save: i.save()?,
            }
        }
        RELOAD => {
            let i = input(&["path", "discard"])?;
            Reload {
                path: i.string("path")?,
                discard: i.flag("discard")?,
            }
        }
        SET_ENCODING => {
            let i = input(&["path", "encoding", "bom", "line_ending", "reopen"])?;
            let line_ending = match i.get("line_ending") {
                None => None,
                Some(Value::String(s)) if s == "crlf" => Some(LineEndingChoice::CrLf),
                Some(Value::String(s)) if s == "lf" => Some(LineEndingChoice::Lf),
                Some(Value::String(s)) if s == "cr" => Some(LineEndingChoice::Cr),
                Some(_) => {
                    return Err(CommandError::InvalidInput(
                        "`line_ending` must be \"crlf\", \"lf\" or \"cr\"".into(),
                    ));
                }
            };
            let reopen = i.flag("reopen")?;
            let encoding = i.string("encoding")?;
            if reopen && encoding.is_none() {
                return Err(CommandError::InvalidInput(
                    "`reopen` needs `encoding`".into(),
                ));
            }
            SetEncoding {
                path: i.string("path")?,
                encoding,
                bom: i.opt_bool("bom")?,
                line_ending,
                reopen,
            }
        }
        EXIT => {
            let i = input(&["save"])?;
            Exit { save: i.save()? }
        }
        PIN_TAB => {
            let i = input(&["path", "pinned"])?;
            PinTab {
                path: i.string("path")?,
                pinned: i.opt_bool("pinned")?,
            }
        }
        MOVE_TAB => {
            let i = input(&["path", "index"])?;
            MoveTab {
                path: i
                    .string("path")?
                    .ok_or_else(|| CommandError::InvalidInput("`path` is required".into()))?,
                index: i
                    .uint("index", 0)?
                    .ok_or_else(|| CommandError::InvalidInput("`index` is required".into()))?
                    as usize,
            }
        }
        NEXT_DOCUMENT => {
            let i = input(&["backward"])?;
            NextDocument {
                backward: i.flag("backward")?,
            }
        }
        NAVIGATOR => {
            let i = input(&["backward", "activate"])?;
            Navigator {
                backward: i.flag("backward")?,
                activate: i.uint("activate", 0)?.map(|n| n as usize),
            }
        }
        REOPEN_CLOSED => {
            input(&[])?;
            ReopenClosed
        }
        COPY_FULL_PATH => {
            let i = input(&["path"])?;
            CopyFullPath {
                path: i.string("path")?,
            }
        }
        OPEN_CONTAINING_FOLDER => {
            let i = input(&["path"])?;
            OpenContainingFolder {
                path: i.string("path")?,
            }
        }
        BOOKMARK_TOGGLE => {
            let i = input(&["path", "line"])?;
            BookmarkToggle {
                path: i.string("path")?,
                line: i.uint("line", 1)?.map(|n| n.min(u32::MAX as u64) as u32),
            }
        }
        BOOKMARK_NEXT => {
            input(&[])?;
            BookmarkNext
        }
        BOOKMARK_PREVIOUS => {
            input(&[])?;
            BookmarkPrevious
        }
        BOOKMARK_CLEAR => {
            let i = input(&["path"])?;
            BookmarkClear {
                path: i.string("path")?,
            }
        }
        BOOKMARK_LIST => {
            input(&[])?;
            BookmarkList
        }
        BOOKMARK_UPDATE => {
            let i = input(&["id", "name", "enabled"])?;
            BookmarkUpdate {
                id: i.required_id()?,
                name: i.any_string("name")?,
                enabled: i.opt_bool("enabled")?,
            }
        }
        BOOKMARK_GO_TO => {
            let i = input(&["id"])?;
            BookmarkGoTo {
                id: i.required_id()?,
            }
        }
        GO_TO_LAST_EDIT => {
            input(&[])?;
            GoToLastEdit
        }
        other => {
            return Err(CommandError::InvalidInput(format!(
                "not a documents command: {other}"
            )));
        }
    })
}

macro_rules! schema {
    ($name:literal) => {
        (
            include_str!(concat!("../../../protocol/schemas/", $name, ".input.json")),
            include_str!(concat!("../../../protocol/schemas/", $name, ".output.json")),
        )
    };
}

pub fn spec(id: &str) -> CommandSpec {
    use PermissionClass::*;
    let (title, (input, output), permission) = match id {
        SAVE_ALL => ("File: Save All", schema!("file-save-all"), EditBuffer),
        SAVE_AS => ("File: Save As", schema!("file-save-as"), EditBuffer),
        NEW => ("File: New File", schema!("file-new"), Read),
        OPEN_FILE_DIALOG => ("File: Open File", schema!("file-open-file-dialog"), Read),
        DOCUMENTS => ("File: Open Documents", schema!("file-documents"), Read),
        // Closing can save or drop unsaved changes, as eludite.file.close does.
        CLOSE_ALL => (
            "Window: Close All Documents",
            schema!("file-close-all"),
            EditBuffer,
        ),
        RELOAD => ("File: Reload", schema!("file-reload"), EditBuffer),
        SET_ENCODING => (
            "File: Advanced Save Options",
            schema!("file-set-encoding"),
            EditBuffer,
        ),
        // Quits the IDE and every agent session in it, as eludite.update.apply does.
        EXIT => ("File: Exit", schema!("file-exit"), Dangerous),
        PIN_TAB => ("Window: Pin Tab", schema!("window-pin-tab"), Read),
        MOVE_TAB => ("Window: Move Tab", schema!("window-move-tab"), Read),
        NEXT_DOCUMENT => (
            "Window: Next Document Window",
            schema!("window-next-document"),
            Read,
        ),
        NAVIGATOR => ("Window: IDE Navigator", schema!("window-navigator"), Read),
        REOPEN_CLOSED => (
            "Window: Reopen Closed Tab",
            schema!("window-reopen-closed"),
            Read,
        ),
        COPY_FULL_PATH => (
            "Window: Copy Full Path",
            schema!("window-copy-full-path"),
            Read,
        ),
        OPEN_CONTAINING_FOLDER => (
            "Window: Open Containing Folder",
            schema!("window-open-containing-folder"),
            Read,
        ),
        // Bookmarks are the workspace's navigation state, not its files.
        BOOKMARK_TOGGLE => (
            "Edit: Bookmarks: Toggle Bookmark",
            schema!("bookmarks-toggle"),
            Read,
        ),
        BOOKMARK_NEXT => (
            "Edit: Bookmarks: Next Bookmark",
            schema!("bookmarks-next"),
            Read,
        ),
        BOOKMARK_PREVIOUS => (
            "Edit: Bookmarks: Previous Bookmark",
            schema!("bookmarks-previous"),
            Read,
        ),
        BOOKMARK_CLEAR => (
            "Edit: Bookmarks: Clear Bookmarks",
            schema!("bookmarks-clear"),
            Read,
        ),
        BOOKMARK_LIST => ("View: Bookmark Window", schema!("bookmarks-list"), Read),
        BOOKMARK_UPDATE => (
            "Bookmarks: Rename or Enable",
            schema!("bookmarks-update"),
            Read,
        ),
        BOOKMARK_GO_TO => ("Bookmarks: Go To", schema!("bookmarks-go-to"), Read),
        GO_TO_LAST_EDIT => (
            "Edit: Go To Last Edit Location",
            schema!("editor-go-to-last-edit"),
            Read,
        ),
        other => unreachable!("not a documents command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register the commands on `registry`, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn DocumentsTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |value| target.apply(parse(id, value)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
        }
        let files = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/schemas"
        ))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("bookmarks-") || n.starts_with("window-"))
        .count();
        let ours = ALL
            .iter()
            .filter(|id| id.contains(".bookmarks.") || id.contains(".window."))
            .count();
        assert_eq!(files, ours * 2, "two schemas per command, no stray one");
    }

    #[test]
    fn classes() {
        use PermissionClass::*;
        assert_eq!(spec(SAVE_ALL).permission, EditBuffer);
        assert_eq!(spec(CLOSE_ALL).permission, EditBuffer);
        assert_eq!(spec(EXIT).permission, Dangerous);
        assert_eq!(spec(DOCUMENTS).permission, Read);
        assert_eq!(spec(BOOKMARK_TOGGLE).permission, Read);
    }

    #[test]
    fn input_is_checked() {
        use DocumentsRequest::*;
        assert_eq!(parse(SAVE_ALL, json!({})).unwrap(), SaveAll);
        assert!(parse(SAVE_ALL, json!({"x": 1})).is_err());
        assert!(parse(SAVE_ALL, json!([])).is_err());
        assert_eq!(
            parse(CLOSE_ALL, json!({"except": "/a.cs", "save": "discard"})).unwrap(),
            CloseAll {
                except: Some("/a.cs".into()),
                keep_pinned: false,
                save: Some(CloseSave::Discard)
            }
        );
        assert!(parse(CLOSE_ALL, json!({"save": "maybe"})).is_err());
        assert_eq!(
            parse(EXIT, json!({})).unwrap(),
            Exit { save: None },
            "no answer: the shell asks the person, or refuses an agent"
        );
        assert_eq!(
            parse(
                SET_ENCODING,
                json!({"encoding": "utf-16le", "line_ending": "lf"})
            )
            .unwrap(),
            SetEncoding {
                path: None,
                encoding: Some("utf-16le".into()),
                bom: None,
                line_ending: Some(LineEndingChoice::Lf),
                reopen: false
            }
        );
        assert!(parse(SET_ENCODING, json!({"line_ending": "mixed"})).is_err());
        assert!(parse(SET_ENCODING, json!({"reopen": true})).is_err());
        assert_eq!(
            parse(BOOKMARK_TOGGLE, json!({"line": 3})).unwrap(),
            BookmarkToggle {
                path: None,
                line: Some(3)
            }
        );
        assert!(parse(BOOKMARK_TOGGLE, json!({"line": 0})).is_err());
        assert!(parse(BOOKMARK_UPDATE, json!({"name": "x"})).is_err());
        assert_eq!(
            parse(BOOKMARK_UPDATE, json!({"id": 2, "name": ""})).unwrap(),
            BookmarkUpdate {
                id: 2,
                name: Some(String::new()),
                enabled: None
            }
        );
        assert_eq!(
            parse(NAVIGATOR, json!({"activate": 0})).unwrap(),
            Navigator {
                backward: false,
                activate: Some(0)
            }
        );
        assert!(parse("eludite.file.other", json!({})).is_err());
        for id in ALL {
            assert_eq!(parse(id, json!({})).map(|r| r.command()).unwrap_or(id), id);
        }
    }

    #[test]
    fn registered_commands_reach_the_target() {
        struct Echo;
        impl DocumentsTarget for Echo {
            fn apply(&self, request: DocumentsRequest) -> Result<Value, CommandError> {
                Ok(json!({"command": request.command()}))
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        assert_eq!(
            registry.invoke(SAVE_ALL, json!({})).unwrap()["command"],
            SAVE_ALL
        );
        assert!(registry.invoke(EXIT, json!({"save": 1})).is_err());
    }
}
