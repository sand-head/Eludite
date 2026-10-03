//! Reading a page without the engine: flattening an accessibility tree or a DOM tree into `read_page`'s rows,
//! boxes from box models, and image sizes from encoded screenshots. Pure functions over CDP answers, so they are
//! tested against recorded trees.

use std::collections::HashMap;

use eludite_commands::browser::{BoxRow, Checked, NodeState, ReadFilter};
use eludite_protocol::cdp::accessibility::{AXNode, AXPropertyName, AXValue};
use serde_json::Value;

/// The roles `read_page`'s `interactive` filter keeps (brief 0023).
pub const INTERACTIVE_ROLES: [&str; 14] = [
    "button",
    "link",
    "textbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "tab",
    "slider",
    "spinbutton",
    "switch",
    "searchbox",
];

/// Roles never kept by `interactive` although focusable and named: the document itself.
const DOCUMENT_ROLES: [&str; 2] = ["RootWebArea", "WebArea"];

/// One row of `read_page` before it gets a ref and a box.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub backend_node_id: i64,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub description: Option<String>,
    pub state: NodeState,
    pub depth: usize,
}

fn value_text(v: Option<&AXValue>) -> Option<String> {
    match v?.value.as_ref()? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn truthy(v: &AXValue) -> Option<bool> {
    match v.value.as_ref()? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(s == "true"),
        _ => None,
    }
}

/// The states of an accessibility node, from its properties.
pub fn ax_state(node: &AXNode) -> NodeState {
    let mut st = NodeState::default();
    for p in node.properties.iter().flatten() {
        match &p.name {
            AXPropertyName::Checked => {
                st.checked = match p.value.value.as_ref() {
                    Some(Value::String(s)) if s == "mixed" => Some(Checked::Mixed),
                    _ => truthy(&p.value).map(|b| if b { Checked::True } else { Checked::False }),
                }
            }
            AXPropertyName::Disabled => st.disabled = truthy(&p.value),
            AXPropertyName::Expanded => st.expanded = truthy(&p.value),
            AXPropertyName::Focused => st.focused = truthy(&p.value),
            AXPropertyName::Selected => st.selected = truthy(&p.value),
            AXPropertyName::Required => st.required = truthy(&p.value),
            AXPropertyName::Invalid => {
                // `false`, `true`, `grammar` or `spelling`; only an invalid control says so.
                if let Some(Value::String(s)) = p.value.value.as_ref()
                    && s != "false"
                {
                    st.invalid = Some(true);
                }
            }
            _ => {}
        }
    }
    st
}

fn focusable(node: &AXNode) -> bool {
    node.properties
        .iter()
        .flatten()
        .any(|p| p.name == AXPropertyName::Focusable && truthy(&p.value) == Some(true))
}

/// Whether `interactive` keeps a node of `role` and `name`.
pub fn is_interactive(role: &str, name: &str, focusable: bool) -> bool {
    INTERACTIVE_ROLES.contains(&role)
        || (focusable && !name.trim().is_empty() && !DOCUMENT_ROLES.contains(&role))
}

/// Flatten `Accessibility.getFullAXTree`'s nodes in document order, from the root or from the node of
/// `root_backend`. Ignored nodes are skipped (their children are not); nodes without a DOM node cannot have a ref
/// and are skipped too. `depth` counts the rows above a row.
pub fn flatten_ax(
    nodes: &[AXNode],
    filter: ReadFilter,
    root_backend: Option<i64>,
) -> Result<Vec<Row>, String> {
    let by_id: HashMap<&str, &AXNode> = nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();
    let root = match root_backend {
        Some(b) => nodes
            .iter()
            .find(|n| n.backend_dom_node_id == Some(b))
            .ok_or("the ref's element is not in the accessibility tree (it may be hidden); try mode `dom`")?,
        None => match nodes.iter().find(|n| n.parent_id.is_none()).or(nodes.first()) {
            Some(n) => n,
            None => return Ok(Vec::new()),
        },
    };
    let mut rows = Vec::new();
    let mut stack: Vec<(&AXNode, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        let role = value_text(node.role.as_ref()).unwrap_or_default();
        // Labels wrapping their control give names like "Name ": collapse the whitespace.
        let name = collapse(&value_text(node.name.as_ref()).unwrap_or_default());
        let keep = !node.ignored
            && node.backend_dom_node_id.is_some()
            && match filter {
                ReadFilter::All => true,
                ReadFilter::Interactive => is_interactive(&role, &name, focusable(node)),
            };
        if keep {
            let state = ax_state(node);
            rows.push(Row {
                backend_node_id: node.backend_dom_node_id.unwrap_or_default(),
                role,
                name,
                value: value_text(node.value.as_ref()).filter(|v| !v.is_empty()),
                description: value_text(node.description.as_ref()).filter(|v| !v.is_empty()),
                state,
                depth,
            });
        }
        let child_depth = if keep { depth + 1 } else { depth };
        for c in node.child_ids.iter().flatten().rev() {
            if let Some(child) = by_id.get(c.as_str()) {
                stack.push((child, child_depth));
            }
        }
    }
    Ok(rows)
}

/// The row of one accessibility node from `Accessibility.getPartialAXTree`, when `interactive` keeps it (depth 0:
/// the caller knows the ancestry).
pub fn interactive_row(node: &AXNode) -> Option<Row> {
    let role = value_text(node.role.as_ref()).unwrap_or_default();
    let name = collapse(&value_text(node.name.as_ref()).unwrap_or_default());
    if node.ignored || !is_interactive(&role, &name, focusable(node)) {
        return None;
    }
    Some(Row {
        backend_node_id: node.backend_dom_node_id?,
        role,
        name,
        value: value_text(node.value.as_ref()).filter(|v| !v.is_empty()),
        description: value_text(node.description.as_ref()).filter(|v| !v.is_empty()),
        state: ax_state(node),
        depth: 0,
    })
}

/// Depths of candidate elements given each one's nearest candidate ancestor (`parents[i]`, an index before `i`, or
/// -1) and whether it was kept: a row's depth counts the kept rows above it, as [`flatten_ax`] counts them.
/// Candidates not examined (past `kept.len()`) get no depth.
pub fn candidate_depths(parents: &[i64], kept: &[bool]) -> Vec<usize> {
    let mut depth = vec![0usize; kept.len()];
    for i in 0..kept.len() {
        let mut p = parents.get(i).copied().unwrap_or(-1);
        depth[i] = loop {
            match usize::try_from(p).ok().filter(|&p| p < i) {
                Some(p) if kept[p] => break depth[p] + 1,
                Some(q) => p = parents.get(q).copied().unwrap_or(-1),
                None => break 0,
            }
        };
    }
    depth
}

fn attrs(node: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(a) = node["attributes"].as_array() {
        for pair in a.chunks(2) {
            if let [k, v] = pair {
                out.insert(
                    k.as_str().unwrap_or_default().to_ascii_lowercase(),
                    v.as_str().unwrap_or_default().to_owned(),
                );
            }
        }
    }
    out
}

fn text_of(node: &Value, deep: bool, out: &mut String) {
    for c in node["children"].as_array().into_iter().flatten() {
        if out.len() > 200 {
            return;
        }
        match c["nodeType"].as_i64() {
            Some(3) => out.push_str(c["nodeValue"].as_str().unwrap_or_default()),
            Some(1) if deep => text_of(c, true, out),
            _ => {}
        }
    }
}

/// Runs of whitespace as one space, trimmed.
pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn squash(s: &str) -> String {
    let mut out: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > 100 {
        out = out.chars().take(100).collect::<String>() + "…";
    }
    out
}

/// Elements `read_page`'s `dom` mode skips with their subtrees.
const SKIPPED_TAGS: [&str; 9] = [
    "head", "script", "style", "meta", "link", "title", "noscript", "template", "base",
];

fn dom_interactive(tag: &str, a: &HashMap<String, String>) -> bool {
    match tag {
        "a" => a.contains_key("href"),
        "button" | "select" | "textarea" | "option" | "summary" => true,
        "input" => a
            .get("type")
            .is_none_or(|t| !t.eq_ignore_ascii_case("hidden")),
        _ => {
            a.get("role")
                .is_some_and(|r| INTERACTIVE_ROLES.contains(&r.as_str()))
                || a.get("tabindex")
                    .is_some_and(|t| t.trim().parse::<i32>().is_ok_and(|n| n >= 0))
                || a.get("contenteditable")
                    .is_some_and(|v| v.is_empty() || v == "true")
        }
    }
}

fn dom_state(a: &HashMap<String, String>) -> NodeState {
    let flag = |k: &str| a.contains_key(k).then_some(true);
    let aria = |k: &str| a.get(k).map(|v| v == "true");
    NodeState {
        checked: match a.get("aria-checked").map(String::as_str) {
            Some("mixed") => Some(Checked::Mixed),
            Some("true") => Some(Checked::True),
            Some("false") => Some(Checked::False),
            _ => flag("checked").map(|_| Checked::True),
        },
        disabled: flag("disabled").or_else(|| aria("aria-disabled")),
        expanded: aria("aria-expanded"),
        focused: None,
        selected: flag("selected").or_else(|| aria("aria-selected")),
        required: flag("required").or_else(|| aria("aria-required")),
        invalid: aria("aria-invalid").filter(|v| *v),
    }
}

/// Flatten a `DOM.Node` tree (from `DOM.getDocument` or `describeNode` with `depth: -1`) into rows: elements only,
/// in document order, the tag name as the role. The name is the first of `aria-label`, `alt`, `title`,
/// `placeholder`, `name` and the element's text. State comes from attributes (markup, not live state).
pub fn flatten_dom(root: &Value, filter: ReadFilter) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut stack: Vec<(&Value, usize)> = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        let mut child_depth = depth;
        if node["nodeType"].as_i64() == Some(1) {
            let tag = node["localName"]
                .as_str()
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    node["nodeName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                });
            if SKIPPED_TAGS.contains(&tag.as_str()) {
                continue;
            }
            let a = attrs(node);
            let interactive = dom_interactive(&tag, &a);
            if filter == ReadFilter::All || interactive {
                let label = ["aria-label", "alt", "title", "placeholder", "name"]
                    .iter()
                    .find_map(|k| a.get(*k).filter(|v| !v.trim().is_empty()))
                    .cloned();
                let name = label.unwrap_or_else(|| {
                    let mut t = String::new();
                    text_of(node, interactive, &mut t);
                    t
                });
                let state = dom_state(&a);
                rows.push(Row {
                    backend_node_id: node["backendNodeId"].as_i64().unwrap_or_default(),
                    role: tag.clone(),
                    name: squash(&name),
                    value: a.get("value").cloned().filter(|v| !v.is_empty()),
                    description: None,
                    state,
                    depth,
                });
                child_depth = depth + 1;
            }
        }
        if let Some(children) = node["children"].as_array() {
            for c in children.iter().rev() {
                stack.push((c, child_depth));
            }
        }
    }
    rows
}

/// A border quad (`[x1, y1, ..., x4, y4]`) as a box, rounded to tenths of a CSS pixel.
pub fn quad_box(quad: &[f64]) -> Option<BoxRow> {
    if quad.len() < 8 {
        return None;
    }
    let xs = quad.iter().step_by(2);
    let ys = quad.iter().skip(1).step_by(2);
    let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    let (y0, y1) = ys.fold((f64::MAX, f64::MIN), |(a, b), &y| (a.min(y), b.max(y)));
    let r = |v: f64| (v * 10.).round() / 10.;
    Some(BoxRow {
        x: r(x0),
        y: r(y0),
        width: r(x1 - x0),
        height: r(y1 - y0),
    })
}

/// Decode standard base64 (padding optional, whitespace ignored), up to `limit` bytes.
pub fn base64_decode(s: &str, limit: usize) -> Vec<u8> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Width and height of a PNG or JPEG image given as base64.
pub fn image_size(base64: &str) -> Option<(u32, u32)> {
    let head = base64_decode(&base64[..base64.len().min(64)], 33);
    if head.starts_with(b"\x89PNG\r\n\x1a\n") && head.len() >= 24 {
        let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if head.starts_with(&[0xFF, 0xD8]) {
        let bytes = base64_decode(base64, usize::MAX);
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) || marker == 0xFF
            {
                i += if marker == 0xFF { 1 } else { 2 };
                continue;
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            let sof = matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF);
            if sof {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((w, h));
            }
            i += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn recorded_form_tree() -> Vec<AXNode> {
        let text = include_str!("../tests/fixtures/form-axtree.json");
        let v: Value = serde_json::from_str(text).unwrap();
        serde_json::from_value(v["nodes"].clone()).unwrap()
    }

    #[test]
    fn the_interactive_filter_over_a_recorded_tree() {
        let nodes = recorded_form_tree();
        let rows = flatten_ax(&nodes, ReadFilter::Interactive, None).unwrap();
        let summary: Vec<(String, String)> = rows
            .iter()
            .map(|r| (r.role.clone(), r.name.clone()))
            .collect();
        let expect = |role: &str, name: &str| {
            assert!(
                summary.iter().any(|(r, n)| r == role && n == name),
                "no {role} `{name}` in {summary:?}"
            )
        };
        expect("textbox", "Name");
        expect("textbox", "Email");
        expect("checkbox", "Subscribe");
        expect("combobox", "Plan");
        expect("button", "Submit");
        expect("link", "Terms");
        assert!(
            summary
                .iter()
                .all(|(r, _)| r != "RootWebArea" && r != "StaticText" && r != "heading")
        );
        // The required, checked and disabled states come through.
        let email = rows.iter().find(|r| r.name == "Email").unwrap();
        assert_eq!(email.state.required, Some(true));
        let sub = rows.iter().find(|r| r.name == "Subscribe").unwrap();
        assert_eq!(sub.state.checked, Some(Checked::True));
        let disabled = rows.iter().find(|r| r.name == "Disabled").unwrap();
        assert_eq!(disabled.state.disabled, Some(true));
        // `all` lists more, the heading and the text included, in document order with depths.
        let all = flatten_ax(&nodes, ReadFilter::All, None).unwrap();
        assert!(all.len() > rows.len());
        assert_eq!(all[0].role, "RootWebArea");
        assert_eq!(all[0].depth, 0);
        assert!(all.iter().any(|r| r.role == "heading"));
        assert!(all.iter().skip(1).all(|r| r.depth >= 1));
        let heading = all.iter().position(|r| r.role == "heading").unwrap();
        let button = all
            .iter()
            .position(|r| r.role == "button" && r.name == "Submit")
            .unwrap();
        assert!(heading < button, "document order");
        // From a root: only its subtree.
        let form = all
            .iter()
            .find(|r| r.role == "form")
            .expect("a form landmark");
        let sub_rows =
            flatten_ax(&nodes, ReadFilter::Interactive, Some(form.backend_node_id)).unwrap();
        assert!(sub_rows.iter().any(|r| r.name == "Submit"));
        assert!(
            sub_rows.iter().all(|r| r.name != "Terms"),
            "the link is outside the form"
        );
        assert!(flatten_ax(&nodes, ReadFilter::All, Some(-5)).is_err());
    }

    #[test]
    fn depths_of_candidates_count_kept_ancestors() {
        // 0 is a kept toolbar; 1 (kept) and 2 (dropped) sit in it; 3 sits in 2; 4 is outside.
        let parents = [-1, 0, 0, 2, -1];
        let kept = [true, true, false, true, true];
        assert_eq!(candidate_depths(&parents, &kept), vec![0, 1, 1, 1, 0]);
        // Only what was examined gets a depth; a bad parent index counts as none.
        assert_eq!(candidate_depths(&[-1, 7, 0], &[true, true]), vec![0, 0]);
    }

    #[test]
    fn the_partial_tree_row_of_a_recorded_node() {
        let nodes = recorded_form_tree();
        let submit = nodes
            .iter()
            .find(|n| value_text(n.name.as_ref()).as_deref() == Some("Submit"))
            .expect("the Submit button");
        let row = interactive_row(submit).expect("a button is interactive");
        assert_eq!(
            (row.role.as_str(), row.name.as_str(), row.depth),
            ("button", "Submit", 0)
        );
        let heading = nodes
            .iter()
            .find(|n| value_text(n.role.as_ref()).as_deref() == Some("heading"))
            .expect("the heading");
        assert_eq!(interactive_row(heading), None);
    }

    #[test]
    fn interactive_rules() {
        assert!(is_interactive("button", "", false));
        assert!(is_interactive("generic", "Card", true));
        assert!(!is_interactive("generic", "", true));
        assert!(!is_interactive("RootWebArea", "Title", true));
        assert!(!is_interactive("heading", "Title", false));
    }

    #[test]
    fn dom_rows() {
        let doc = json!({"nodeType": 9, "nodeName": "#document", "backendNodeId": 1, "children": [
        {"nodeType": 1, "nodeName": "HTML", "localName": "html", "backendNodeId": 2, "children": [
            {"nodeType": 1, "nodeName": "HEAD", "localName": "head", "backendNodeId": 3, "children": [
                {"nodeType": 1, "nodeName": "TITLE", "localName": "title", "backendNodeId": 4}]},
            {"nodeType": 1, "nodeName": "BODY", "localName": "body", "backendNodeId": 5, "children": [
                {"nodeType": 1, "nodeName": "A", "localName": "a", "backendNodeId": 6, "attributes": ["href", "/t"],
                 "children": [{"nodeType": 3, "nodeName": "#text", "nodeValue": " Terms  of use ", "backendNodeId": 7}]},
                {"nodeType": 1, "nodeName": "INPUT", "localName": "input", "backendNodeId": 8,
                 "attributes": ["type", "email", "placeholder", "you@example.com", "required", ""]},
                {"nodeType": 1, "nodeName": "INPUT", "localName": "input", "backendNodeId": 9, "attributes": ["type", "hidden"]},
                {"nodeType": 1, "nodeName": "BUTTON", "localName": "button", "backendNodeId": 10, "attributes": ["disabled", ""],
                 "children": [{"nodeType": 1, "nodeName": "SPAN", "localName": "span", "backendNodeId": 11,
                    "children": [{"nodeType": 3, "nodeValue": "Save", "backendNodeId": 12}]}]}
            ]}]}]});
        let rows = flatten_dom(&doc, ReadFilter::Interactive);
        let got: Vec<(&str, &str, usize)> = rows
            .iter()
            .map(|r| (r.role.as_str(), r.name.as_str(), r.depth))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", "Terms of use", 0),
                ("input", "you@example.com", 0),
                ("button", "Save", 0)
            ]
        );
        assert_eq!(rows[1].state.required, Some(true));
        assert_eq!(rows[2].state.disabled, Some(true));
        let all = flatten_dom(&doc, ReadFilter::All);
        let tags: Vec<&str> = all.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(
            tags,
            ["html", "body", "a", "input", "input", "button", "span"]
        );
        assert_eq!(all[2].depth, 2);
        assert_eq!(all[6].depth, 3);
    }

    #[test]
    fn boxes_and_image_sizes() {
        assert_eq!(
            quad_box(&[10., 20., 110., 20., 110., 40.04, 10., 40.04]),
            Some(BoxRow {
                x: 10.,
                y: 20.,
                width: 100.,
                height: 20.
            })
        );
        assert_eq!(quad_box(&[1., 2.]), None);
        // A 1x1 PNG.
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        assert_eq!(image_size(png), Some((1, 1)));
        // A minimal JPEG header with an SOF0 of 640x480 after an APP0 segment.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend([
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0,
        ]);
        let b64 = encode(&jpeg);
        assert_eq!(image_size(&b64), Some((640, 480)));
        assert_eq!(base64_decode("aGVsbG8=", 100), b"hello");
        assert_eq!(image_size("aGVsbG8="), None);
    }

    fn encode(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}
