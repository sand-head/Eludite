//! The shell's everyday chrome (brief 0078): the command palette and Feature Search (`eludite.view.command_palette`,
//! `eludite.commands.list`), the keymap (`eludite.keymap.*`, `eludite.help.keyboard_shortcuts`), the color theme
//! (`eludite.theme.*`), full screen, the logs and the issue tracker (`eludite.help.*`), the notifications
//! (`eludite.notifications.*`) and window layouts (`eludite.view.*_window_layout*`), registered by the shell through a
//! [`UxTarget`]. `eludite.help.about` is the built-in command, replaced by the shell with the same input and a superset
//! of its output (`help-about.output.json`). Outputs are JSON objects of the commands' output schemas.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const PALETTE: &str = "eludite.view.command_palette";
pub const COMMANDS_LIST: &str = "eludite.commands.list";
pub const KEYMAP_GET: &str = "eludite.keymap.get";
pub const KEYMAP_SET: &str = "eludite.keymap.set";
pub const KEYMAP_REMOVE: &str = "eludite.keymap.remove";
pub const KEYBOARD_SHORTCUTS: &str = "eludite.help.keyboard_shortcuts";
pub const THEME_LIST: &str = "eludite.theme.list";
pub const THEME_SET: &str = "eludite.theme.set";
pub const FULL_SCREEN: &str = "eludite.view.full_screen";
pub const ABOUT: &str = crate::builtins::ABOUT;
pub const OPEN_LOGS_FOLDER: &str = "eludite.help.open_logs_folder";
pub const SHOW_LOG: &str = "eludite.help.show_log";
pub const REPORT_PROBLEM: &str = "eludite.help.report_problem";
pub const NOTIFICATIONS_LIST: &str = "eludite.notifications.list";
pub const NOTIFICATIONS_SHOW: &str = "eludite.notifications.show";
pub const NOTIFICATIONS_DISMISS: &str = "eludite.notifications.dismiss";
pub const SAVE_LAYOUT: &str = "eludite.view.save_window_layout";
pub const APPLY_LAYOUT: &str = "eludite.view.apply_window_layout";
pub const MANAGE_LAYOUTS: &str = "eludite.view.manage_window_layouts";

pub const ALL: [&str; 19] = [
    PALETTE,
    COMMANDS_LIST,
    KEYMAP_GET,
    KEYMAP_SET,
    KEYMAP_REMOVE,
    KEYBOARD_SHORTCUTS,
    THEME_LIST,
    THEME_SET,
    FULL_SCREEN,
    ABOUT,
    OPEN_LOGS_FOLDER,
    SHOW_LOG,
    REPORT_PROBLEM,
    NOTIFICATIONS_LIST,
    NOTIFICATIONS_SHOW,
    NOTIFICATIONS_DISMISS,
    SAVE_LAYOUT,
    APPLY_LAYOUT,
    MANAGE_LAYOUTS,
];

/// A parsed, validated request.
#[derive(Debug, Clone, PartialEq)]
pub enum UxRequest {
    Palette {
        query: Option<String>,
        features: bool,
    },
    CommandsList {
        query: Option<String>,
        features: bool,
        limit: Option<usize>,
    },
    KeymapGet {
        command: Option<String>,
    },
    KeymapSet {
        keys: String,
        command: String,
        args: Value,
        /// `global` or `editor`.
        scope: String,
    },
    KeymapRemove {
        keys: String,
        command: Option<String>,
    },
    KeyboardShortcuts {
        open: bool,
    },
    ThemeList,
    ThemeSet {
        name: String,
    },
    FullScreen {
        enabled: Option<bool>,
    },
    About,
    OpenLogsFolder,
    ShowLog,
    ReportProblem {
        open: bool,
    },
    NotificationsList,
    NotificationsShow {
        visible: Option<bool>,
    },
    NotificationsDismiss {
        id: Option<u64>,
        all: bool,
    },
    SaveLayout {
        name: Option<String>,
    },
    ApplyLayout {
        name: Option<String>,
        slot: Option<u32>,
    },
    ManageLayouts {
        /// `list`, `rename`, `delete`, `move_up`; `None` opens the dialog.
        action: Option<String>,
        name: Option<String>,
        new_name: Option<String>,
    },
}

/// What applies the requests (the shell).
pub trait UxTarget: Send + Sync {
    fn apply(&self, id: &str, request: UxRequest) -> Result<Value, CommandError>;
}

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(msg.into())
}

fn no_extra(value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let obj = value
        .as_object()
        .ok_or_else(|| invalid("input must be an object"))?;
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(invalid(format!("unknown field `{k}`"))),
        None => Ok(()),
    }
}

fn opt_str(value: &Value, name: &str) -> Result<Option<String>, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("`{name}` must be a string"))),
    }
}

fn req_str(value: &Value, name: &str) -> Result<String, CommandError> {
    opt_str(value, name)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid(format!("`{name}` is required")))
}

fn opt_bool(value: &Value, name: &str) -> Result<Option<bool>, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(invalid(format!("`{name}` must be a boolean"))),
    }
}

fn opt_u64(value: &Value, name: &str, min: u64, max: u64) -> Result<Option<u64>, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if (min..=max).contains(&n) => Ok(Some(n)),
            _ => Err(invalid(format!(
                "`{name}` must be an integer from {min} to {max}"
            ))),
        },
    }
}

fn one_of(
    value: Option<String>,
    name: &str,
    allowed: &[&str],
) -> Result<Option<String>, CommandError> {
    match value {
        Some(v) if !allowed.contains(&v.as_str()) => Err(invalid(format!(
            "`{name}` must be one of {}",
            allowed.join(", ")
        ))),
        v => Ok(v),
    }
}

/// Parse and validate a command's input.
pub fn parse(id: &str, value: Value) -> Result<UxRequest, CommandError> {
    let v = &value;
    Ok(match id {
        PALETTE => {
            no_extra(v, &["query", "mode"])?;
            let mode = one_of(opt_str(v, "mode")?, "mode", &["commands", "features"])?;
            UxRequest::Palette {
                query: opt_str(v, "query")?,
                features: mode.as_deref() == Some("features"),
            }
        }
        COMMANDS_LIST => {
            no_extra(v, &["query", "features", "limit"])?;
            UxRequest::CommandsList {
                query: opt_str(v, "query")?,
                features: opt_bool(v, "features")?.unwrap_or(false),
                limit: opt_u64(v, "limit", 1, 5000)?.map(|n| n as usize),
            }
        }
        KEYMAP_GET => {
            no_extra(v, &["command"])?;
            UxRequest::KeymapGet {
                command: opt_str(v, "command")?,
            }
        }
        KEYMAP_SET => {
            no_extra(v, &["keys", "command", "args", "scope"])?;
            let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
            if !args.is_object() {
                return Err(invalid("`args` must be an object"));
            }
            UxRequest::KeymapSet {
                keys: req_str(v, "keys")?,
                command: req_str(v, "command")?,
                args,
                scope: one_of(opt_str(v, "scope")?, "scope", &["global", "editor"])?
                    .unwrap_or_else(|| "global".into()),
            }
        }
        KEYMAP_REMOVE => {
            no_extra(v, &["keys", "command"])?;
            UxRequest::KeymapRemove {
                keys: req_str(v, "keys")?,
                command: opt_str(v, "command")?,
            }
        }
        KEYBOARD_SHORTCUTS => {
            no_extra(v, &["open"])?;
            UxRequest::KeyboardShortcuts {
                open: opt_bool(v, "open")?.unwrap_or(false),
            }
        }
        THEME_LIST => {
            no_extra(v, &[])?;
            UxRequest::ThemeList
        }
        THEME_SET => {
            no_extra(v, &["name"])?;
            UxRequest::ThemeSet {
                name: req_str(v, "name")?,
            }
        }
        FULL_SCREEN => {
            no_extra(v, &["enabled"])?;
            UxRequest::FullScreen {
                enabled: opt_bool(v, "enabled")?,
            }
        }
        ABOUT => {
            no_extra(v, &[])?;
            UxRequest::About
        }
        OPEN_LOGS_FOLDER => {
            no_extra(v, &[])?;
            UxRequest::OpenLogsFolder
        }
        SHOW_LOG => {
            no_extra(v, &[])?;
            UxRequest::ShowLog
        }
        REPORT_PROBLEM => {
            no_extra(v, &["open"])?;
            UxRequest::ReportProblem {
                open: opt_bool(v, "open")?.unwrap_or(true),
            }
        }
        NOTIFICATIONS_LIST => {
            no_extra(v, &[])?;
            UxRequest::NotificationsList
        }
        NOTIFICATIONS_SHOW => {
            no_extra(v, &["visible"])?;
            UxRequest::NotificationsShow {
                visible: opt_bool(v, "visible")?,
            }
        }
        NOTIFICATIONS_DISMISS => {
            no_extra(v, &["id", "all"])?;
            let id = opt_u64(v, "id", 0, u64::MAX)?;
            let all = opt_bool(v, "all")?.unwrap_or(false);
            if id.is_none() && !all {
                return Err(invalid("give `id` or `all: true`"));
            }
            UxRequest::NotificationsDismiss { id, all }
        }
        SAVE_LAYOUT => {
            no_extra(v, &["name"])?;
            UxRequest::SaveLayout {
                name: opt_str(v, "name")?.filter(|n| !n.trim().is_empty()),
            }
        }
        APPLY_LAYOUT => {
            no_extra(v, &["name", "slot"])?;
            UxRequest::ApplyLayout {
                name: opt_str(v, "name")?,
                slot: opt_u64(v, "slot", 1, 9)?.map(|n| n as u32),
            }
        }
        MANAGE_LAYOUTS => {
            no_extra(v, &["action", "name", "new_name"])?;
            let action = one_of(
                opt_str(v, "action")?,
                "action",
                &["list", "rename", "delete", "move_up"],
            )?;
            let name = opt_str(v, "name")?;
            let new_name = opt_str(v, "new_name")?;
            match action.as_deref() {
                Some("rename") if name.is_none() || new_name.is_none() => {
                    return Err(invalid("`rename` needs `name` and `new_name`"));
                }
                Some("delete" | "move_up") if name.is_none() => {
                    return Err(invalid("this action needs `name`"));
                }
                _ => {}
            }
            UxRequest::ManageLayouts {
                action,
                name,
                new_name,
            }
        }
        other => return Err(invalid(format!("not a brief 0078 command: {other}"))),
    })
}

pub fn spec(id: &str) -> CommandSpec {
    use PermissionClass::*;
    macro_rules! schemas {
        ($name:literal) => {
            (
                include_str!(concat!("../../../protocol/schemas/", $name, ".input.json")),
                include_str!(concat!("../../../protocol/schemas/", $name, ".output.json")),
            )
        };
    }
    let (title, (input, output), permission) = match id {
        PALETTE => (
            "View: Command Palette",
            schemas!("view-command-palette"),
            Read,
        ),
        COMMANDS_LIST => ("Tools: List Commands", schemas!("commands-list"), Read),
        KEYMAP_GET => ("Tools: Keyboard Bindings", schemas!("keymap-get"), Read),
        // A file the person keeps: written as settings are (eludite.settings.set is edit_buffer too).
        KEYMAP_SET => (
            "Tools: Assign Keyboard Shortcut",
            schemas!("keymap-set"),
            EditBuffer,
        ),
        KEYMAP_REMOVE => (
            "Tools: Remove Keyboard Shortcut",
            schemas!("keymap-remove"),
            EditBuffer,
        ),
        KEYBOARD_SHORTCUTS => (
            "Help: Keyboard Shortcuts",
            schemas!("help-keyboard-shortcuts"),
            Read,
        ),
        THEME_LIST => ("Tools: List Themes", schemas!("theme-list"), Read),
        THEME_SET => ("Tools: Theme", schemas!("theme-set"), Read),
        FULL_SCREEN => ("View: Full Screen", schemas!("view-full-screen"), Read),
        ABOUT => (
            "Help: About Eludite",
            (
                r#"{"type": "object", "properties": {}, "additionalProperties": false}"#,
                include_str!("../../../protocol/schemas/help-about.output.json"),
            ),
            Read,
        ),
        OPEN_LOGS_FOLDER => (
            "Help: Open Logs Folder",
            schemas!("help-open-logs-folder"),
            Read,
        ),
        SHOW_LOG => ("Help: Show Log", schemas!("help-show-log"), Read),
        // Opens the system browser on an external site.
        REPORT_PROBLEM => (
            "Help: Report a Problem",
            schemas!("help-report-problem"),
            Dangerous,
        ),
        NOTIFICATIONS_LIST => (
            "View: List Notifications",
            schemas!("notifications-list"),
            Read,
        ),
        NOTIFICATIONS_SHOW => ("View: Notifications", schemas!("notifications-show"), Read),
        NOTIFICATIONS_DISMISS => (
            "View: Dismiss Notifications",
            schemas!("notifications-dismiss"),
            Read,
        ),
        SAVE_LAYOUT => (
            "Window: Save Window Layout",
            schemas!("view-save-window-layout"),
            Read,
        ),
        APPLY_LAYOUT => (
            "Window: Apply Window Layout",
            schemas!("view-apply-window-layout"),
            Read,
        ),
        MANAGE_LAYOUTS => (
            "Window: Manage Window Layouts",
            schemas!("view-manage-window-layouts"),
            Read,
        ),
        other => unreachable!("not a brief 0078 command: {other}"),
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

/// Register (or replace) the commands on `registry`, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn UxTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |value| target.apply(id, parse(id, value)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            if id != ABOUT {
                assert_eq!(s.input_schema["title"], format!("{id} input"));
            }
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
            assert!(
                s.title.contains(": "),
                "{id}: palette titles are `Category: Name`"
            );
        }
        // The built-in About's output fields are kept.
        let about = spec(ABOUT).output_schema;
        assert_eq!(about["required"][0], "name");
        assert_eq!(about["required"][1], "version");
    }

    #[test]
    fn input_is_checked() {
        assert_eq!(
            parse(PALETTE, json!({"mode": "features", "query": "build"})).unwrap(),
            UxRequest::Palette {
                query: Some("build".into()),
                features: true
            }
        );
        assert!(parse(PALETTE, json!({"mode": "x"})).is_err());
        assert!(parse(PALETTE, json!({"x": 1})).is_err());
        assert_eq!(
            parse(
                KEYMAP_SET,
                json!({"keys": "ctrl-k ctrl-o", "command": "a.b"})
            )
            .unwrap(),
            UxRequest::KeymapSet {
                keys: "ctrl-k ctrl-o".into(),
                command: "a.b".into(),
                args: json!({}),
                scope: "global".into()
            }
        );
        assert!(parse(KEYMAP_SET, json!({"keys": "f1"})).is_err());
        assert!(
            parse(
                KEYMAP_SET,
                json!({"keys": "f1", "command": "a.b", "args": 1})
            )
            .is_err()
        );
        assert!(
            parse(
                KEYMAP_SET,
                json!({"keys": "f1", "command": "a.b", "scope": "x"})
            )
            .is_err()
        );
        assert!(parse(NOTIFICATIONS_DISMISS, json!({})).is_err());
        assert_eq!(
            parse(NOTIFICATIONS_DISMISS, json!({"all": true})).unwrap(),
            UxRequest::NotificationsDismiss {
                id: None,
                all: true
            }
        );
        assert!(parse(APPLY_LAYOUT, json!({"slot": 10})).is_err());
        assert!(parse(MANAGE_LAYOUTS, json!({"action": "rename", "name": "a"})).is_err());
        assert_eq!(
            parse(REPORT_PROBLEM, json!({})).unwrap(),
            UxRequest::ReportProblem { open: true }
        );
        assert_eq!(parse(ABOUT, json!({})).unwrap(), UxRequest::About);
        assert!(parse("eludite.nope", json!({})).is_err());
    }

    #[test]
    fn classes() {
        assert_eq!(spec(KEYMAP_SET).permission, PermissionClass::EditBuffer);
        assert_eq!(spec(REPORT_PROBLEM).permission, PermissionClass::Dangerous);
        assert_eq!(spec(THEME_LIST).permission, PermissionClass::Read);
    }
}
