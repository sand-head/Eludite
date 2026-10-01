//! `eludite.view.*`: show, hide, float, auto-hide, dock and reset tool windows
//! (PLAN.md 5.1, 8; brief 0008).
//!
//! The schemas are the files in `protocol/schemas/view-*.json` (checked in
//! first, CLAUDE.md invariant 4), embedded at compile time. This module only
//! parses and validates input into a typed [`ViewRequest`] and serializes the
//! typed [`ViewOutput`]; the docking layout itself lives in `eludite-docking`,
//! which implements [`ViewTarget`]. The menu bar, the keymap, the docking
//! guides and agents all reach the layout through these commands.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SHOW: &str = "eludite.view.show";
pub const HIDE: &str = "eludite.view.hide";
pub const FLOAT: &str = "eludite.view.float";
pub const AUTO_HIDE: &str = "eludite.view.auto_hide";
pub const DOCK: &str = "eludite.view.dock";
pub const RESET_LAYOUT: &str = "eludite.view.reset_layout";

/// Every command this module registers.
pub const ALL: [&str; 6] = [SHOW, HIDE, FLOAT, AUTO_HIDE, DOCK, RESET_LAYOUT];

/// `protocol/schemas/view-tool-window.output.json`, the output of every command but reset.
pub const TOOL_WINDOW_OUTPUT_SCHEMA: &str =
    include_str!("../../../protocol/schemas/view-tool-window.output.json");

fn schemas(id: &str) -> (&'static str, &'static str, &'static str) {
    // (title, input schema, output schema)
    match id {
        SHOW => (
            "View: Show Tool Window",
            include_str!("../../../protocol/schemas/view-show.input.json"),
            TOOL_WINDOW_OUTPUT_SCHEMA,
        ),
        HIDE => (
            "Window: Hide",
            include_str!("../../../protocol/schemas/view-hide.input.json"),
            TOOL_WINDOW_OUTPUT_SCHEMA,
        ),
        FLOAT => (
            "Window: Float",
            include_str!("../../../protocol/schemas/view-float.input.json"),
            TOOL_WINDOW_OUTPUT_SCHEMA,
        ),
        AUTO_HIDE => (
            "Window: Auto Hide",
            include_str!("../../../protocol/schemas/view-auto-hide.input.json"),
            TOOL_WINDOW_OUTPUT_SCHEMA,
        ),
        DOCK => (
            "Window: Dock",
            include_str!("../../../protocol/schemas/view-dock.input.json"),
            TOOL_WINDOW_OUTPUT_SCHEMA,
        ),
        RESET_LAYOUT => (
            "Window: Reset Window Layout",
            include_str!("../../../protocol/schemas/view-reset-layout.input.json"),
            include_str!("../../../protocol/schemas/view-reset-layout.output.json"),
        ),
        other => unreachable!("not a view command: {other}"),
    }
}

/// A dock edge, as on the docking guides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockEdge {
    Left,
    Right,
    Bottom,
}

/// Requested floating window bounds, logical pixels, screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloatBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Where `eludite.view.dock` sends a window.
#[derive(Debug, Clone, PartialEq)]
pub enum DockTarget {
    /// Back where it belongs: a floating window to its last dock, an
    /// auto-hidden one pinned into its dock.
    Home,
    /// A new group on this edge.
    Side(DockEdge),
    /// A tab in the group holding this tool window.
    TabWith(String),
}

/// A parsed, validated `eludite.view.*` invocation. `id: None` means the
/// active tool window.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewRequest {
    Show {
        id: String,
    },
    Hide {
        id: Option<String>,
    },
    Float {
        id: Option<String>,
        bounds: Option<FloatBounds>,
    },
    AutoHide {
        id: Option<String>,
    },
    Dock {
        id: Option<String>,
        target: DockTarget,
    },
    ResetLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    Docked,
    Floating,
    AutoHidden,
    Hidden,
    Document,
}

/// `view-tool-window.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolWindowState {
    pub id: String,
    pub title: String,
    pub state: WindowState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<DockEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<Vec<String>>,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flyout_open: Option<bool>,
}

/// The typed result of a view command.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewOutput {
    Window(ToolWindowState),
    /// `view-reset-layout.output.json`.
    Reset(Vec<ToolWindowState>),
}

impl ViewOutput {
    pub fn to_json(&self) -> Value {
        match self {
            ViewOutput::Window(w) => serde_json::to_value(w),
            ViewOutput::Reset(all) => serde_json::to_value(ResetOutput { tool_windows: all }),
        }
        .expect("view outputs serialize")
    }
}

#[derive(Serialize)]
struct ResetOutput<'a> {
    tool_windows: &'a [ToolWindowState],
}

/// Whatever owns the layout. Called on the invoking thread (the UI thread for
/// menus and keys, a server thread for agents), so it must not block for long.
pub trait ViewTarget: Send + Sync {
    fn apply(&self, request: ViewRequest) -> Result<ViewOutput, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowIn {
    id: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdIn {
    id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FloatIn {
    id: Option<String>,
    bounds: Option<FloatBounds>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DockIn {
    id: Option<String>,
    side: Option<DockEdge>,
    tab_with: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetIn {}

fn from_input<T: for<'de> Deserialize<'de> + Default>(input: Value) -> Result<T, CommandError> {
    if input.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn non_empty(id: Option<String>) -> Result<Option<String>, CommandError> {
    match id {
        Some(s) if s.is_empty() => Err(CommandError::InvalidInput("`id` must not be empty".into())),
        other => Ok(other),
    }
}

/// Parse and validate the input of the view command `id`.
pub fn parse(id: &str, input: Value) -> Result<ViewRequest, CommandError> {
    match id {
        SHOW => {
            let i: ShowIn = serde_json::from_value(input)
                .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
            Ok(ViewRequest::Show {
                id: non_empty(Some(i.id))?.expect("some"),
            })
        }
        HIDE => Ok(ViewRequest::Hide {
            id: non_empty(from_input::<IdIn>(input)?.id)?,
        }),
        AUTO_HIDE => Ok(ViewRequest::AutoHide {
            id: non_empty(from_input::<IdIn>(input)?.id)?,
        }),
        FLOAT => {
            let i: FloatIn = from_input(input)?;
            if let Some(b) = i.bounds
                && !(b.width > 0. && b.height > 0.)
            {
                return Err(CommandError::InvalidInput(
                    "`bounds.width` and `bounds.height` must be positive".into(),
                ));
            }
            Ok(ViewRequest::Float {
                id: non_empty(i.id)?,
                bounds: i.bounds,
            })
        }
        DOCK => {
            let i: DockIn = from_input(input)?;
            let target = match (i.side, i.tab_with) {
                (Some(_), Some(_)) => {
                    return Err(CommandError::InvalidInput(
                        "give at most one of `side` and `tab_with`".into(),
                    ));
                }
                (Some(side), None) => DockTarget::Side(side),
                (None, Some(w)) if w.is_empty() => {
                    return Err(CommandError::InvalidInput(
                        "`tab_with` must not be empty".into(),
                    ));
                }
                (None, Some(w)) => DockTarget::TabWith(w),
                (None, None) => DockTarget::Home,
            };
            Ok(ViewRequest::Dock {
                id: non_empty(i.id)?,
                target,
            })
        }
        RESET_LAYOUT => {
            from_input::<ResetIn>(input)?;
            Ok(ViewRequest::ResetLayout)
        }
        other => Err(CommandError::UnknownCommand(other.to_owned())),
    }
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The public description of view command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: parse_schema(input),
        output_schema: parse_schema(output),
        // Moving windows around changes no file and runs nothing (PLAN.md 5.3).
        permission: PermissionClass::Read,
        // Window layout is the user's; agents read and edit code, not the docking layout.
        agent_visible: false,
    }
}

/// Register every `eludite.view.*` command, applying them to `target`.
pub fn register(
    registry: &mut CommandRegistry,
    target: Arc<dyn ViewTarget>,
) -> Result<(), CommandError> {
    for id in ALL {
        let target = target.clone();
        registry.register(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use serde_json::json;

    /// Records requests and answers with a fixed state.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<ViewRequest>>);

    impl ViewTarget for Recorder {
        fn apply(&self, request: ViewRequest) -> Result<ViewOutput, CommandError> {
            let reset = request == ViewRequest::ResetLayout;
            self.0.lock().unwrap().push(request);
            let w = ToolWindowState {
                id: "output".into(),
                title: "Output".into(),
                state: WindowState::Docked,
                side: Some(DockEdge::Bottom),
                group: Some(vec!["error_list".into(), "output".into()]),
                active: true,
                flyout_open: None,
            };
            Ok(if reset {
                ViewOutput::Reset(vec![w])
            } else {
                ViewOutput::Window(w)
            })
        }
    }

    fn setup() -> (CommandRegistry, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        let mut r = CommandRegistry::new();
        register(&mut r, rec.clone()).unwrap();
        (r, rec)
    }

    #[test]
    fn specs_use_protocol_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.id.as_str(), id);
            assert_eq!(s.permission, PermissionClass::Read);
            assert_eq!(s.input_schema["type"], "object", "{id}");
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["type"], "object", "{id}");
        }
        assert_eq!(spec(SHOW).input_schema["required"], json!(["id"]));
        assert_eq!(
            spec(DOCK).input_schema["properties"]["side"]["enum"],
            json!(["left", "right", "bottom"])
        );
        assert_eq!(
            spec(RESET_LAYOUT).output_schema["required"],
            json!(["tool_windows"])
        );
    }

    #[test]
    fn parses_every_command() {
        let (r, rec) = setup();
        r.invoke(SHOW, json!({"id": "output"})).unwrap();
        r.invoke(HIDE, json!({})).unwrap();
        r.invoke(HIDE, Value::Null).unwrap();
        r.invoke(AUTO_HIDE, json!({"id": "toolbox"})).unwrap();
        r.invoke(
            FLOAT,
            json!({"id": "output", "bounds": {"x": 1, "y": 2, "width": 300, "height": 200}}),
        )
        .unwrap();
        r.invoke(DOCK, json!({"id": "output", "side": "left"}))
            .unwrap();
        r.invoke(DOCK, json!({"id": "output", "tab_with": "properties"}))
            .unwrap();
        r.invoke(DOCK, json!({"id": "toolbox"})).unwrap();
        let out = r.invoke(RESET_LAYOUT, json!({})).unwrap();
        assert_eq!(out["tool_windows"][0]["id"], "output");

        let got = rec.0.lock().unwrap().clone();
        assert_eq!(
            got,
            [
                ViewRequest::Show {
                    id: "output".into()
                },
                ViewRequest::Hide { id: None },
                ViewRequest::Hide { id: None },
                ViewRequest::AutoHide {
                    id: Some("toolbox".into())
                },
                ViewRequest::Float {
                    id: Some("output".into()),
                    bounds: Some(FloatBounds {
                        x: 1.,
                        y: 2.,
                        width: 300.,
                        height: 200.
                    })
                },
                ViewRequest::Dock {
                    id: Some("output".into()),
                    target: DockTarget::Side(DockEdge::Left)
                },
                ViewRequest::Dock {
                    id: Some("output".into()),
                    target: DockTarget::TabWith("properties".into())
                },
                ViewRequest::Dock {
                    id: Some("toolbox".into()),
                    target: DockTarget::Home
                },
                ViewRequest::ResetLayout,
            ]
        );
    }

    #[test]
    fn output_matches_schema_fields() {
        let (r, _) = setup();
        let out = r.invoke(SHOW, json!({"id": "output"})).unwrap();
        let schema = spec(SHOW).output_schema;
        let props = schema["properties"].as_object().unwrap();
        for key in out.as_object().unwrap().keys() {
            assert!(props.contains_key(key), "{key} not in schema");
        }
        for req in schema["required"].as_array().unwrap() {
            assert!(out.get(req.as_str().unwrap()).is_some(), "{req} missing");
        }
        assert_eq!(out["state"], "docked");
        assert_eq!(out["side"], "bottom");
    }

    #[test]
    fn rejects_bad_input() {
        let (r, rec) = setup();
        for (cmd, bad) in [
            (SHOW, json!({})),
            (SHOW, json!({"id": ""})),
            (SHOW, Value::Null),
            (HIDE, json!({"id": 3})),
            (HIDE, json!({"name": "output"})),
            (
                FLOAT,
                json!({"bounds": {"x": 0, "y": 0, "width": 0, "height": 10}}),
            ),
            (FLOAT, json!({"bounds": {"x": 0, "y": 0}})),
            (DOCK, json!({"side": "top"})),
            (DOCK, json!({"side": "left", "tab_with": "output"})),
            (DOCK, json!({"tab_with": ""})),
            (RESET_LAYOUT, json!({"hard": true})),
        ] {
            assert!(
                matches!(
                    r.invoke(cmd, bad.clone()),
                    Err(CommandError::InvalidInput(_))
                ),
                "{cmd} {bad}"
            );
        }
        assert!(rec.0.lock().unwrap().is_empty());
        assert!(r.audit_log().entries().iter().all(|e| !e.is_ok()));
    }
}
