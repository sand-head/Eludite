//! The generated types compile and round-trip real CDP messages through serde (brief 0023).

use eludite_protocol::cdp::{Command, Event, accessibility, page, runtime};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn round_trip<T: Serialize + DeserializeOwned + std::fmt::Debug + PartialEq>(v: Value) -> T {
    let typed: T = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), v, "{typed:?}");
    let again: T = serde_json::from_value(serde_json::to_value(&typed).unwrap()).unwrap();
    assert_eq!(again, typed);
    typed
}

#[test]
fn capture_screenshot_returns() {
    let r: page::CaptureScreenshotReturns = round_trip(json!({"data": "iVBORw0KGgo="}));
    assert_eq!(r.data, "iVBORw0KGgo=");
    assert_eq!(
        <page::CaptureScreenshotParams as Command>::METHOD,
        "Page.captureScreenshot"
    );
    assert_eq!(
        page::CaptureScreenshotParams::METHOD,
        "Page.captureScreenshot"
    );
    let params = page::CaptureScreenshotParams {
        format: Some(page::CaptureScreenshotFormat::Png),
        clip: Some(page::Viewport {
            x: 0.,
            y: 0.,
            width: 1280.,
            height: 800.,
            scale: 0.5,
        }),
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({"format": "png", "clip": {"x": 0.0, "y": 0.0, "width": 1280.0, "height": 800.0, "scale": 0.5}})
    );
}

#[test]
fn console_api_called_event() {
    let e: runtime::ConsoleAPICalledEvent = round_trip(json!({
        "type": "error",
        "args": [
            {"type": "string", "value": "boom"},
            {"type": "object", "subtype": "error", "className": "Error", "description": "Error: x\n    at f (http://127.0.0.1/a.js:3:9)", "objectId": "1.2.3"}
        ],
        "executionContextId": 1,
        "timestamp": 1759446000123.5,
        "stackTrace": {
            "callFrames": [{"functionName": "f", "scriptId": "7", "url": "http://127.0.0.1/a.js", "lineNumber": 2, "columnNumber": 8}],
            "parent": {"description": "Promise.then", "callFrames": []}
        }
    }));
    assert_eq!(e.type_, runtime::ConsoleAPICalledType::Error);
    assert_eq!(
        <runtime::ConsoleAPICalledEvent as Event>::NAME,
        "Runtime.consoleAPICalled"
    );
    assert_eq!(
        e.stack_trace
            .unwrap()
            .parent
            .unwrap()
            .description
            .as_deref(),
        Some("Promise.then")
    );
}

#[test]
fn ax_node_with_unknown_values() {
    let n: accessibility::AXNode = round_trip(json!({
        "nodeId": "42",
        "ignored": false,
        "role": {"type": "internalRole", "value": "brandNewRole"},
        "name": {"type": "computedString", "value": "Submit"},
        "properties": [
            {"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}},
            {"name": "brandNewProperty", "value": {"type": "brandNewValueType", "value": 1}}
        ],
        "childIds": ["43"],
        "backendDOMNodeId": 17
    }));
    assert_eq!(n.backend_dom_node_id, Some(17));
    let role = n.role.unwrap();
    assert_eq!(role.type_, accessibility::AXValueType::InternalRole);
    assert_eq!(role.value, Some(json!("brandNewRole")));
    let props = n.properties.unwrap();
    assert_eq!(props[0].name, accessibility::AXPropertyName::Focusable);
    assert_eq!(
        props[1].name,
        accessibility::AXPropertyName::Other("brandNewProperty".into())
    );
    assert_eq!(
        props[1].value.type_,
        accessibility::AXValueType::Other("brandNewValueType".into())
    );
    assert_eq!(props[1].name.as_str(), "brandNewProperty");
}
