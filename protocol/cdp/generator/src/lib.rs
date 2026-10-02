//! `eludite-cdp-generator`: generates the Chrome DevTools Protocol types in `protocol/rust/src/cdp/` (the
//! `eludite-protocol` crate's `cdp` module, behind its `cdp` feature) from the pinned protocol JSON in
//! `protocol/cdp/` (brief 0023, CLAUDE.md invariant 4: generated, never hand-edited).
//!
//! The roots are [`ROOTS`]; every domain they reference through a `$ref`, transitively, is generated too (today
//! `Debugger` and `IO`). For each domain, one file with:
//!
//! - a `serde` struct per `object` type, an alias per string, integer, number, array and free-form object type, and
//!   a string enum per `enum` (inline enums of members become `<Owner><Member>` enums); every enum decodes values
//!   this protocol version does not list as `Other(String)`, since Chrome adds values;
//! - `<Command>Params` and `<Command>Returns` per command, with `METHOD` and an impl of `cdp::Command`;
//! - `<Event>Event` per event, with `NAME` and an impl of `cdp::Event`.
//!
//! Optional members are `Option<T>` skipped when `None`; `any` is `serde_json::Value`; binary data is base64 in a
//! `String`; members whose type can contain their owner by value are boxed; Rust keywords get a `_` suffix and a
//! `#[serde(rename)]`; experimental and deprecated items are kept and documented as such.
//!
//! Public API: [`generate`] (the files as text, unformatted), [`format`] (through `rustfmt`), [`write`] and
//! [`Stats`]. `cargo run -p eludite-cdp-generator` regenerates the checked-in files; the crate's test fails when they
//! are not current. Plain Rust with `std::fmt::Write`: no proc macros, no templating crate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The domains Eludite's browser automation uses (briefs 0023 and 0024); the generator adds what they reference.
pub const ROOTS: [&str; 13] = [
    "Target",
    "Page",
    "DOM",
    "Accessibility",
    "Runtime",
    "Log",
    "Network",
    "Input",
    "Emulation",
    "Browser",
    "Security",
    "Storage",
    "DOMStorage",
];

/// Where things are, relative to this crate.
pub fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `protocol/cdp/`.
pub fn cdp_dir() -> PathBuf {
    manifest_dir().join("..")
}

/// `protocol/rust/src/cdp/`, the checked-in output.
pub fn output_dir() -> PathBuf {
    manifest_dir().join("../../rust/src/cdp")
}

/// The pinned package version from `protocol/cdp/PIN`.
pub fn pinned_version(cdp_dir: &Path) -> Result<String, String> {
    let pin = std::fs::read_to_string(cdp_dir.join("PIN")).map_err(|e| format!("PIN: {e}"))?;
    pin.lines()
        .find_map(|l| l.strip_prefix("version "))
        .map(|v| v.trim().to_owned())
        .ok_or_else(|| "PIN has no version line".into())
}

/// What one generation produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub domains: Vec<String>,
    pub structs: usize,
    pub enums: usize,
    pub aliases: usize,
    pub commands: usize,
    pub events: usize,
}

/// The generated files: file name (`page.rs`, `mod.rs`) to unformatted source.
pub struct Generated {
    pub files: BTreeMap<String, String>,
    pub stats: Stats,
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
];

/// The lints generated code cannot avoid: protocol names (`AXNode`, `XHR`), the protocol's own prose in doc
/// comments, and long parameter structs.
const ALLOWS: &str = "#![allow(\n    clippy::upper_case_acronyms,\n    clippy::doc_lazy_continuation,\n    clippy::doc_overindented_list_items,\n    clippy::large_enum_variant,\n    clippy::empty_docs,\n    clippy::struct_excessive_bools,\n    rustdoc::bare_urls,\n    rustdoc::broken_intra_doc_links,\n    rustdoc::invalid_html_tags,\n    rustdoc::invalid_rust_codeblocks\n)]\n";

/// `backendDOMNodeId` to `backend_dom_node_id`.
pub fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_lower)
            {
                out.push('_');
            }
        }
        if c == '-' || c == '.' || c == ' ' {
            out.push('_');
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// `captureScreenshot` to `CaptureScreenshot`.
fn upper_first(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// An enum value to a variant name: `DOMContentLoaded` stays, `same-origin` is `SameOrigin`, `2d` is `V2d`.
fn variant_name(value: &str) -> String {
    let mut out = String::new();
    for piece in value.split(|c: char| !c.is_ascii_alphanumeric()) {
        out.push_str(&upper_first(piece));
    }
    if out.is_empty() {
        out.push_str("Empty");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'V');
    }
    if out == "Other" {
        // `Other(String)` holds the values this protocol version does not list.
        out.push_str("Value");
    }
    out
}

fn field_name(name: &str) -> String {
    let s = snake_case(name);
    if KEYWORDS.contains(&s.as_str()) {
        format!("{s}_")
    } else {
        s
    }
}

fn module_name(domain: &str) -> String {
    snake_case(domain)
}

fn doc(out: &mut String, indent: &str, text: &str) {
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            let _ = writeln!(out, "{indent}///");
        } else {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
}

fn flags(item: &Value) -> String {
    let mut f = Vec::new();
    if item["experimental"].as_bool() == Some(true) {
        f.push("**Experimental.**");
    }
    if item["deprecated"].as_bool() == Some(true) {
        f.push("**Deprecated.**");
    }
    f.join(" ")
}

fn description(item: &Value, prefix: &str) -> String {
    let mut parts = Vec::new();
    if !prefix.is_empty() {
        parts.push(prefix.to_owned());
    }
    if let Some(d) = item["description"].as_str()
        && !d.trim().is_empty()
    {
        parts.push(d.trim().to_owned());
    }
    let f = flags(item);
    if !f.is_empty() {
        parts.push(f);
    }
    parts.join("\n\n")
}

/// The domains to generate: the roots and their transitive `$ref`s, in name order.
fn closure(domains: &BTreeMap<String, Value>) -> Result<Vec<String>, String> {
    fn refs(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::String(r)) = o.get("$ref")
                    && let Some((d, _)) = r.split_once('.')
                {
                    out.insert(d.to_owned());
                }
                o.values().for_each(|v| refs(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| refs(v, out)),
            _ => {}
        }
    }
    let mut seen = BTreeSet::new();
    let mut todo: Vec<String> = ROOTS.iter().map(|s| (*s).to_owned()).collect();
    while let Some(d) = todo.pop() {
        if !seen.insert(d.clone()) {
            continue;
        }
        let domain = domains
            .get(&d)
            .ok_or_else(|| format!("domain {d} is not in the protocol"))?;
        let mut r = BTreeSet::new();
        refs(domain, &mut r);
        todo.extend(r.into_iter().filter(|x| !seen.contains(x)));
    }
    Ok(seen.into_iter().collect())
}

/// What a named type is.
#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Struct,
    Enum,
    Alias,
}

struct Model<'a> {
    domains: BTreeMap<String, &'a Value>,
    /// `Domain.Type` to its kind.
    kinds: HashMap<String, Kind>,
    /// `Domain.Type` struct edges by value (non-array `$ref` members), for boxing.
    by_value: HashMap<String, Vec<String>>,
    /// Structs whose required members all have a `Default`.
    defaultable: BTreeSet<String>,
}

fn qualify(domain: &str, r: &str) -> String {
    if r.contains('.') {
        r.to_owned()
    } else {
        format!("{domain}.{r}")
    }
}

fn members(t: &Value) -> &[Value] {
    t["properties"].as_array().map_or(&[], Vec::as_slice)
}

impl<'a> Model<'a> {
    fn new(domains: BTreeMap<String, &'a Value>) -> Self {
        let mut kinds = HashMap::new();
        for (name, d) in &domains {
            for t in d["types"].as_array().into_iter().flatten() {
                let id = t["id"].as_str().unwrap_or_default();
                let kind = if t.get("enum").is_some() {
                    Kind::Enum
                } else if t["type"] == "object" && t.get("properties").is_some() {
                    Kind::Struct
                } else {
                    Kind::Alias
                };
                kinds.insert(format!("{name}.{id}"), kind);
            }
        }
        let mut by_value: HashMap<String, Vec<String>> = HashMap::new();
        for (name, d) in &domains {
            for t in d["types"].as_array().into_iter().flatten() {
                let id = format!("{name}.{}", t["id"].as_str().unwrap_or_default());
                for m in members(t) {
                    if let Some(r) = m["$ref"].as_str() {
                        let target = qualify(name, r);
                        if kinds.get(&target) == Some(&Kind::Struct) {
                            by_value.entry(id.clone()).or_default().push(target);
                        }
                    }
                }
            }
        }
        let mut model = Self {
            domains,
            kinds,
            by_value,
            defaultable: BTreeSet::new(),
        };
        // Fixed point: a struct has a `Default` when every required member's type has one.
        loop {
            let mut grew = false;
            for (name, d) in &model.domains {
                for t in d["types"].as_array().into_iter().flatten() {
                    let id = format!("{name}.{}", t["id"].as_str().unwrap_or_default());
                    if model.kinds[&id] != Kind::Struct || model.defaultable.contains(&id) {
                        continue;
                    }
                    let ok = members(t).iter().all(|m| {
                        m["optional"].as_bool() == Some(true) || model.member_default(name, m)
                    });
                    if ok {
                        model.defaultable.insert(id);
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        model
    }

    fn type_def(&self, qualified: &str) -> Option<&'a Value> {
        let (d, id) = qualified.split_once('.')?;
        self.domains.get(d)?["types"]
            .as_array()?
            .iter()
            .find(|t| t["id"] == id)
    }

    /// Whether a member's type has a `Default`.
    fn member_default(&self, domain: &str, m: &Value) -> bool {
        if let Some(r) = m["$ref"].as_str() {
            let q = qualify(domain, r);
            return match self.kinds.get(&q) {
                Some(Kind::Struct) => self.defaultable.contains(&q),
                Some(Kind::Enum) | None => false,
                Some(Kind::Alias) => self.type_def(&q).is_some_and(|t| {
                    self.member_default(q.split_once('.').map_or(domain, |x| x.0), t)
                }),
            };
        }
        m.get("enum").is_none()
    }

    /// Whether `from` reaches `to` through by-value struct members.
    fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut todo = vec![from.to_owned()];
        while let Some(n) = todo.pop() {
            if n == to {
                return true;
            }
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(next) = self.by_value.get(&n) {
                todo.extend(next.iter().cloned());
            }
        }
        false
    }

    /// The Rust path of a `$ref` used from `domain`.
    fn path(&self, domain: &str, r: &str) -> String {
        match r.split_once('.') {
            Some((d, id)) if d != domain => format!("super::{}::{id}", module_name(d)),
            Some((_, id)) => id.to_owned(),
            None => r.to_owned(),
        }
    }
}

/// One domain's file being written.
struct DomainWriter<'m, 'a> {
    model: &'m Model<'a>,
    domain: String,
    out: String,
    /// Inline enums to emit after the item that uses them: (name, description, values).
    pending_enums: Vec<(String, String, Vec<String>)>,
    names: BTreeSet<String>,
    stats: Stats,
}

impl DomainWriter<'_, '_> {
    fn claim(&mut self, name: &str) -> Result<(), String> {
        if self.names.insert(name.to_owned()) {
            Ok(())
        } else {
            Err(format!("{}: two items named {name}", self.domain))
        }
    }

    /// The Rust type of a member, registering an inline enum named `owner` + member.
    fn member_type(&mut self, owner: &str, m: &Value, boxed: bool) -> Result<String, String> {
        let inner = if let Some(r) = m["$ref"].as_str() {
            self.model.path(&self.domain, r)
        } else if let Some(values) = m["enum"].as_array() {
            let name = format!(
                "{owner}{}",
                upper_first(m["name"].as_str().unwrap_or_default())
            );
            let values = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            self.pending_enums.push((
                name.clone(),
                description(
                    m,
                    &format!(
                        "Values of `{owner}`'s `{}`.",
                        m["name"].as_str().unwrap_or_default()
                    ),
                ),
                values,
            ));
            name
        } else {
            self.plain_type(owner, m)?
        };
        Ok(if boxed {
            format!("Box<{inner}>")
        } else {
            inner
        })
    }

    fn plain_type(&mut self, owner: &str, m: &Value) -> Result<String, String> {
        Ok(match m["type"].as_str() {
            Some("string" | "binary") => "String".into(),
            Some("integer") => "i64".into(),
            Some("number") => "f64".into(),
            Some("boolean") => "bool".into(),
            Some("any") => "serde_json::Value".into(),
            Some("object") => "serde_json::Map<String, serde_json::Value>".into(),
            Some("array") => {
                let items = &m["items"];
                let item = if let Some(r) = items["$ref"].as_str() {
                    self.model.path(&self.domain, r)
                } else if items.get("enum").is_some() {
                    return Err(format!("{}.{owner}: inline enum in an array", self.domain));
                } else {
                    self.plain_type(owner, items)?
                };
                format!("Vec<{item}>")
            }
            other => {
                return Err(format!(
                    "{}.{owner}: unsupported type {other:?}",
                    self.domain
                ));
            }
        })
    }

    /// A struct of `fields` (protocol members), with `derive_default` when every required member allows it.
    fn write_struct(
        &mut self,
        name: &str,
        enum_owner: &str,
        doc_text: &str,
        fields: &[Value],
        qualified: Option<&str>,
    ) -> Result<(), String> {
        self.claim(name)?;
        self.stats.structs += 1;
        let default = match qualified {
            Some(q) => self.model.defaultable.contains(q),
            None => fields.iter().all(|m| {
                m["optional"].as_bool() == Some(true) || self.model.member_default(&self.domain, m)
            }),
        };
        let mut body = String::new();
        let mut seen_fields = BTreeSet::new();
        for m in fields {
            let json_name = m["name"]
                .as_str()
                .ok_or_else(|| format!("{name}: a member has no name"))?;
            let rust_name = field_name(json_name);
            if !seen_fields.insert(rust_name.clone()) {
                return Err(format!(
                    "{}.{name}: two fields named {rust_name}",
                    self.domain
                ));
            }
            let boxed = match (qualified, m["$ref"].as_str()) {
                (Some(q), Some(r)) => {
                    let target = qualify(&self.domain, r);
                    self.model.kinds.get(&target) == Some(&Kind::Struct)
                        && self.model.reaches(&target, q)
                }
                _ => false,
            };
            let ty = self.member_type(enum_owner, m, boxed)?;
            let optional = m["optional"].as_bool() == Some(true);
            doc(&mut body, "    ", &description(m, ""));
            let mut attrs = Vec::new();
            if rust_name != json_name {
                attrs.push(format!("rename = \"{json_name}\""));
            }
            if optional {
                attrs.push("default".to_owned());
                attrs.push("skip_serializing_if = \"Option::is_none\"".to_owned());
            }
            if !attrs.is_empty() {
                let _ = writeln!(body, "    #[serde({})]", attrs.join(", "));
            }
            if optional {
                let _ = writeln!(body, "    pub {rust_name}: Option<{ty}>,");
            } else {
                let _ = writeln!(body, "    pub {rust_name}: {ty},");
            }
        }
        doc(&mut self.out, "", doc_text);
        let derives = if default {
            "Debug, Clone, PartialEq, Default, Serialize, Deserialize"
        } else {
            "Debug, Clone, PartialEq, Serialize, Deserialize"
        };
        let _ = writeln!(self.out, "#[derive({derives})]");
        if body.is_empty() {
            let _ = writeln!(self.out, "pub struct {name} {{}}\n");
        } else {
            let _ = writeln!(self.out, "pub struct {name} {{\n{body}}}\n");
        }
        self.flush_enums()
    }

    fn write_enum(&mut self, name: &str, doc_text: &str, values: &[String]) -> Result<(), String> {
        self.claim(name)?;
        self.stats.enums += 1;
        let mut variants: Vec<(String, &String)> = Vec::new();
        for v in values {
            let mut n = variant_name(v);
            let base = n.clone();
            let mut i = 2;
            while variants.iter().any(|(x, _)| *x == n) {
                n = format!("{base}{i}");
                i += 1;
            }
            variants.push((n, v));
        }
        doc(&mut self.out, "", doc_text);
        let _ = writeln!(
            self.out,
            "#[derive(Debug, Clone, PartialEq, Eq, Hash)]\npub enum {name} {{"
        );
        for (n, v) in &variants {
            let _ = writeln!(self.out, "    /// `{v}`\n    {n},");
        }
        let _ = writeln!(
            self.out,
            "    /// A value this version of the protocol does not list.\n    Other(String),\n}}\n"
        );
        let _ = writeln!(self.out, "string_enum! {{\n    {name} {{");
        for (n, v) in &variants {
            let _ = writeln!(self.out, "        {n} => \"{v}\",");
        }
        let _ = writeln!(self.out, "    }}\n}}\n");
        Ok(())
    }

    fn flush_enums(&mut self) -> Result<(), String> {
        for (name, d, values) in std::mem::take(&mut self.pending_enums) {
            self.write_enum(&name, &d, &values)?;
        }
        Ok(())
    }

    fn write_domain(&mut self, d: &Value) -> Result<(), String> {
        let domain = self.domain.clone();
        for t in d["types"].as_array().into_iter().flatten() {
            let id = t["id"].as_str().ok_or("a type has no id")?.to_owned();
            let q = format!("{domain}.{id}");
            let text = description(t, "");
            match self.model.kinds[&q] {
                Kind::Struct => self.write_struct(&id, &id, &text, members(t), Some(&q))?,
                Kind::Enum => {
                    let values: Vec<String> = t["enum"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    self.write_enum(&id, &text, &values)?;
                }
                Kind::Alias => {
                    self.claim(&id)?;
                    self.stats.aliases += 1;
                    let ty = self.plain_type(&id, t)?;
                    doc(&mut self.out, "", &text);
                    let _ = writeln!(self.out, "pub type {id} = {ty};\n");
                }
            }
        }
        for c in d["commands"].as_array().into_iter().flatten() {
            let cmd = c["name"].as_str().ok_or("a command has no name")?;
            let method = format!("{domain}.{cmd}");
            let base = upper_first(cmd);
            let params = format!("{base}Params");
            let returns = format!("{base}Returns");
            let empty = Vec::new();
            let p = c["parameters"].as_array().unwrap_or(&empty);
            let r = c["returns"].as_array().unwrap_or(&empty);
            self.write_struct(
                &params,
                &base,
                &description(c, &format!("Parameters of `{method}`.")),
                p,
                None,
            )?;
            // Inline enums of the answer are `<Command><Member>` like the parameters', or `<Command>Returns<Member>`
            // when a parameter took that name.
            let clash = r.iter().any(|m| {
                m.get("enum").is_some()
                    && self.names.contains(&format!(
                        "{base}{}",
                        upper_first(m["name"].as_str().unwrap_or_default())
                    ))
            });
            let owner = if clash { returns.clone() } else { base.clone() };
            self.write_struct(
                &returns,
                &owner,
                &format!("The answer to `{method}`."),
                r,
                None,
            )?;
            let _ = writeln!(
                self.out,
                "impl {params} {{\n    pub const METHOD: &'static str = \"{method}\";\n}}\n\nimpl super::Command for {params} {{\n    const METHOD: &'static str = \"{method}\";\n    type Returns = {returns};\n}}\n"
            );
            self.stats.commands += 1;
        }
        for e in d["events"].as_array().into_iter().flatten() {
            let ev = e["name"].as_str().ok_or("an event has no name")?;
            let name = format!("{}Event", upper_first(ev));
            let full = format!("{domain}.{ev}");
            let empty = Vec::new();
            let p = e["parameters"].as_array().unwrap_or(&empty);
            self.write_struct(
                &name,
                &upper_first(ev),
                &description(e, &format!("The event `{full}`.")),
                p,
                None,
            )?;
            let _ = writeln!(
                self.out,
                "impl {name} {{\n    pub const NAME: &'static str = \"{full}\";\n}}\n\nimpl super::Event for {name} {{\n    const NAME: &'static str = \"{full}\";\n}}\n"
            );
            self.stats.events += 1;
        }
        Ok(())
    }
}

fn header(version: &str) -> String {
    format!(
        "// Generated by eludite-cdp-generator from devtools-protocol {version} (protocol/cdp/PIN). Do not edit:\n// change the generator or the pin and run `cargo run -p eludite-cdp-generator`.\n"
    )
}

/// Generate the `cdp` module from the two protocol files' text.
pub fn generate(
    browser_protocol: &str,
    js_protocol: &str,
    version: &str,
) -> Result<Generated, String> {
    let mut domains = BTreeMap::new();
    let parsed: Vec<Value> = [browser_protocol, js_protocol]
        .iter()
        .map(|t| serde_json::from_str(t).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    for p in &parsed {
        for d in p["domains"].as_array().ok_or("no domains")? {
            let name = d["domain"].as_str().ok_or("a domain has no name")?;
            domains.insert(name.to_owned(), d.clone());
        }
    }
    let wanted = closure(&domains)?;
    let model_domains: BTreeMap<String, &Value> =
        wanted.iter().map(|d| (d.clone(), &domains[d])).collect();
    let model = Model::new(model_domains);
    let mut files = BTreeMap::new();
    let mut stats = Stats {
        domains: wanted.clone(),
        ..Stats::default()
    };
    for name in &wanted {
        let d = &domains[name];
        let mut w = DomainWriter {
            model: &model,
            domain: name.clone(),
            out: String::new(),
            pending_enums: Vec::new(),
            names: BTreeSet::new(),
            stats: Stats::default(),
        };
        w.write_domain(d)?;
        let mut file = header(version);
        let mut doc_text = format!("The CDP domain `{name}`.");
        let extra = description(d, "");
        if !extra.is_empty() {
            doc_text.push_str("\n\n");
            doc_text.push_str(&extra);
        }
        for line in doc_text.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                file.push_str("//!\n");
            } else {
                let _ = writeln!(file, "//! {line}");
            }
        }
        file.push('\n');
        file.push_str(ALLOWS);
        file.push_str("\nuse serde::{Deserialize, Serialize};\n\n");
        file.push_str(&w.out);
        stats.structs += w.stats.structs;
        stats.enums += w.stats.enums;
        stats.aliases += w.stats.aliases;
        stats.commands += w.stats.commands;
        stats.events += w.stats.events;
        files.insert(format!("{}.rs", module_name(name)), file);
    }
    files.insert("mod.rs".into(), mod_rs(version, &wanted));
    Ok(Generated { files, stats })
}

fn mod_rs(version: &str, domains: &[String]) -> String {
    let mut out = header(version);
    let roots = ROOTS.join(", ");
    let _ = write!(
        out,
        "//! Chrome DevTools Protocol types, generated from the pinned protocol JSON in `protocol/cdp/` (brief 0023).\n//!\n//! The domains Eludite uses ({roots}) and every domain they reference. Each command has `<Command>Params`\n//! (implementing [`Command`]) and `<Command>Returns`; each event a `<Event>Event` (implementing [`Event`]). Every\n//! enum decodes a value this protocol version does not list as `Other(String)`, since Chrome adds values. The\n//! message envelope (`id`, `method`, `params`, `sessionId`, `result`, `error`) is not described by the protocol\n//! JSON; clients type it themselves.\n\n{ALLOWS}\n"
    );
    out.push_str(
        "/// A CDP command: its parameters, its method name and the type of its answer.\npub trait Command: serde::Serialize {\n    const METHOD: &'static str;\n    type Returns: serde::de::DeserializeOwned;\n}\n\n/// A CDP event's parameters and its name.\npub trait Event: serde::de::DeserializeOwned {\n    const NAME: &'static str;\n}\n\n",
    );
    out.push_str(
        "/// The methods of a string enum with an `Other(String)` catch-all: `as_str`, `From<&str>`, `Display` and\n/// serde as a plain string.\nmacro_rules! string_enum {\n    ($name:ident { $($variant:ident => $value:literal,)* }) => {\n        impl $name {\n            /// The protocol's string.\n            pub fn as_str(&self) -> &str {\n                match self {\n                    $(Self::$variant => $value,)*\n                    Self::Other(s) => s,\n                }\n            }\n        }\n\n        impl From<&str> for $name {\n            fn from(s: &str) -> Self {\n                match s {\n                    $($value => Self::$variant,)*\n                    other => Self::Other(other.to_owned()),\n                }\n            }\n        }\n\n        impl std::fmt::Display for $name {\n            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n                f.write_str(self.as_str())\n            }\n        }\n\n        impl serde::Serialize for $name {\n            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {\n                serializer.serialize_str(self.as_str())\n            }\n        }\n\n        impl<'de> serde::Deserialize<'de> for $name {\n            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {\n                let s = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(deserializer)?;\n                Ok(Self::from(s.as_ref()))\n            }\n        }\n    };\n}\n\n",
    );
    for d in domains {
        let _ = writeln!(out, "pub mod {};", module_name(d));
    }
    out
}

/// Format `files` with `rustfmt` (edition 2024, the workspace's style), in a temporary directory beside `dir`.
pub fn format(
    files: &BTreeMap<String, String>,
    scratch: &Path,
) -> Result<BTreeMap<String, String>, String> {
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (name, text) in files {
        let p = scratch.join(name);
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        paths.push(p);
    }
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let status = Command::new(&rustfmt)
        .arg("--edition")
        .arg("2024")
        .args(&paths)
        .status()
        .map_err(|e| format!("running rustfmt ({}): {e}", rustfmt.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("rustfmt failed: {status}"));
    }
    let mut out = BTreeMap::new();
    for (name, p) in files.keys().zip(&paths) {
        out.insert(
            name.clone(),
            std::fs::read_to_string(p).map_err(|e| e.to_string())?,
        );
    }
    Ok(out)
}

/// Generate from `cdp_dir`'s JSON and format: the files to check in.
pub fn generate_formatted(cdp_dir: &Path, scratch: &Path) -> Result<Generated, String> {
    let read = |n: &str| std::fs::read_to_string(cdp_dir.join(n)).map_err(|e| format!("{n}: {e}"));
    let version = pinned_version(cdp_dir)?;
    let g = generate(
        &read("browser_protocol.json")?,
        &read("js_protocol.json")?,
        &version,
    )?;
    Ok(Generated {
        files: format(&g.files, scratch)?,
        stats: g.stats,
    })
}

/// Replace the `.rs` files of `out_dir` with `files`.
pub fn write(files: &BTreeMap<String, String>, out_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(out_dir).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|e| e == "rs")
            && !files.contains_key(&*p.file_name().unwrap_or_default().to_string_lossy())
        {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
    }
    for (name, text) in files {
        std::fs::write(out_dir.join(name), text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(snake_case("backendDOMNodeId"), "backend_dom_node_id");
        assert_eq!(snake_case("frameId"), "frame_id");
        assert_eq!(snake_case("URL"), "url");
        assert_eq!(snake_case("x509Certificate"), "x509_certificate");
        assert_eq!(field_name("type"), "type_");
        assert_eq!(field_name("override"), "override_");
        assert_eq!(variant_name("DOMContentLoaded"), "DOMContentLoaded");
        assert_eq!(variant_name("same-origin"), "SameOrigin");
        assert_eq!(variant_name("2d"), "V2d");
        assert_eq!(variant_name("Other"), "OtherValue");
        assert_eq!(variant_name("other"), "OtherValue");
        assert_eq!(module_name("DOM"), "dom");
        assert_eq!(module_name("IO"), "io");
    }
}
