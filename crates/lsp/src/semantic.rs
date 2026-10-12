//! Semantic tokens (brief 0066): what a server's `textDocument/semanticTokens/*` answers mean.
//!
//! - [`Provider`]: the server's `semanticTokensProvider` (its legend and which requests it serves), from its
//!   `ServerCapabilities` (the host relays Roslyn's in `eludite/languageServer/status`).
//! - [`decode`]: the relative five-integer encoding to absolute tokens ([`Token`]: line, UTF-16 start and length,
//!   type index, modifier bits).
//! - [`apply_edits`]: a `full/delta` answer's edits applied to the kept integers.
//! - [`Classifier`]: a legend's token types and modifiers to [`TokenClass`] and [`Modifiers`], whichever spelling the
//!   server uses: LSP's standard names (`class`, `parameter`), Roslyn's custom ones in its "pure LSP" legend
//!   (`recordClass`, `extensionMethod`, `stringEscapeCharacter`, `excludedCode`, `regexAnchor`, `jsonPropertyName`)
//!   and Visual Studio's classification names (`class name`, `string - escape character`, `regex - anchor`),
//!   rust-analyzer's (`selfKeyword`, `builtinType`, `lifetime`), F#'s (`member`) and TypeScript's (standard).
//!
//! Colors are not here: the shell maps a [`TokenClass`] to the theme's semantic colors.

use eludite_protocol::lsp::SemanticTokensEdit;
use serde_json::Value;

/// A server's legend: the names behind token type indexes and modifier bits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Legend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}

/// A server's `semanticTokensProvider`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provider {
    pub legend: Legend,
    pub full: bool,
    pub delta: bool,
    pub range: bool,
}

impl Provider {
    /// The provider in `capabilities` (`ServerCapabilities`), or `None` when the server does not offer semantic
    /// tokens (no provider, no legend, or neither `full` nor `range`).
    pub fn from_capabilities(capabilities: &Value) -> Option<Self> {
        let p = capabilities.get("semanticTokensProvider")?;
        let legend = p.get("legend")?;
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let legend = Legend {
            token_types: strings(&legend["tokenTypes"]),
            token_modifiers: strings(&legend["tokenModifiers"]),
        };
        let full_v = &p["full"];
        let full = full_v.as_bool() == Some(true) || full_v.is_object();
        let delta = full_v["delta"].as_bool() == Some(true);
        let range_v = &p["range"];
        let range = range_v.as_bool() == Some(true) || range_v.is_object();
        (full || range).then_some(Self {
            legend,
            full,
            delta: full && delta,
            range,
        })
    }
}

/// One token in absolute coordinates: zero-based line, UTF-16 start column and length, the legend's type index and
/// modifier bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub token_type: u32,
    pub modifiers: u32,
}

/// The relative encoding (`[deltaLine, deltaStart, length, type, modifiers]*`) to absolute tokens. A trailing partial
/// group is ignored.
pub fn decode(data: &[u32]) -> Vec<Token> {
    let mut out = Vec::with_capacity(data.len() / 5);
    let (mut line, mut start) = (0u32, 0u32);
    for t in data.chunks_exact(5) {
        if t[0] > 0 {
            line = line.saturating_add(t[0]);
            start = t[1];
        } else {
            start = start.saturating_add(t[1]);
        }
        out.push(Token {
            line,
            start,
            length: t[2],
            token_type: t[3],
            modifiers: t[4],
        });
    }
    out
}

/// A delta's edits do not fit the integers they claim to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaError;

/// Apply a `full/delta` answer's edits to `data` (the integers of the answer it quoted). Every edit refers to the
/// original array, so they apply from the last start to the first. On error `data` is unchanged.
pub fn apply_edits(data: &mut Vec<u32>, edits: &[SemanticTokensEdit]) -> Result<(), DeltaError> {
    let mut sorted: Vec<&SemanticTokensEdit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.start));
    // Overlapping edits, or one beyond the end, are malformed.
    let mut limit = data.len();
    for e in &sorted {
        let end = e.start as usize + e.delete_count as usize;
        if end > limit {
            return Err(DeltaError);
        }
        limit = e.start as usize;
    }
    for e in sorted {
        let start = e.start as usize;
        let end = start + e.delete_count as usize;
        data.splice(start..end, e.data.iter().flatten().copied());
    }
    Ok(())
}

/// What a token is, whatever the server calls it. The theme colors these (Visual Studio's classifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenClass {
    Namespace,
    Class,
    Record,
    Delegate,
    Interface,
    Struct,
    RecordStruct,
    Enum,
    EnumMember,
    TypeParameter,
    Module,
    /// A type the server does not say more about (`type`, a type alias).
    Type,
    BuiltinType,
    Method,
    ExtensionMethod,
    Function,
    Property,
    Field,
    Event,
    Constant,
    Local,
    Parameter,
    Label,
    Keyword,
    ControlKeyword,
    SelfKeyword,
    Operator,
    OverloadedOperator,
    String,
    VerbatimString,
    StringEscape,
    Number,
    Comment,
    /// Code a `#if` excludes.
    ExcludedCode,
    Attribute,
    Lifetime,
    Macro,
    RegexComment,
    RegexCharacterClass,
    RegexAnchor,
    RegexQuantifier,
    RegexGrouping,
    RegexAlternation,
    RegexText,
    RegexSelfEscapedCharacter,
    RegexOtherEscape,
    JsonComment,
    JsonNumber,
    JsonString,
    JsonKeyword,
    JsonPropertyName,
    JsonPunctuation,
    XmlDocComment,
    XmlDocCommentText,
}

/// A token's modifiers, as bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(pub u16);

impl Modifiers {
    pub const STATIC: Modifiers = Modifiers(1);
    pub const DEPRECATED: Modifiers = Modifiers(1 << 1);
    pub const READONLY: Modifiers = Modifiers(1 << 2);
    pub const DECLARATION: Modifiers = Modifiers(1 << 3);
    pub const DEFAULT_LIBRARY: Modifiers = Modifiers(1 << 4);
    /// rust-analyzer's `mutable`, Roslyn's `ReassignedVariable`.
    pub const MUTABLE: Modifiers = Modifiers(1 << 5);
    pub const ASYNC: Modifiers = Modifiers(1 << 6);
    pub const ABSTRACT: Modifiers = Modifiers(1 << 7);

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

/// A name lowercased with spaces, dashes and underscores removed: `string - escape character`, `stringEscapeCharacter`
/// and `string_escape_character` are one name.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The class of a token type name, or `None` for one the editor leaves to the syntax colors (punctuation, plain
/// text, preprocessor text, an unresolved reference).
pub fn classify(token_type: &str) -> Option<TokenClass> {
    use TokenClass::*;
    let n = normalize(token_type);
    let n = n
        .strip_suffix("name")
        .filter(|s| !s.is_empty())
        .unwrap_or(&n);
    Some(match n {
        "namespace" | "toolmodule" | "crate" => Namespace,
        "class" => Class,
        "recordclass" | "record" => Record,
        "delegate" => Delegate,
        "interface" | "trait" => Interface,
        "struct" | "union" => Struct,
        "recordstruct" => RecordStruct,
        "enum" => Enum,
        "enummember" | "variant" => EnumMember,
        "typeparameter" | "constparameter" => TypeParameter,
        "module" => Module,
        "type" | "typealias" => Type,
        "builtintype" => BuiltinType,
        "method" | "member" => Method,
        "extensionmethod" => ExtensionMethod,
        "function" => Function,
        "property" => Property,
        "field" => Field,
        "event" => Event,
        "constant" => Constant,
        "variable" | "local" | "identifier" => Local,
        "parameter" => Parameter,
        "label" => Label,
        "keyword" | "boolean" => Keyword,
        "controlkeyword" | "keywordcontrol" => ControlKeyword,
        "selfkeyword" | "selftypekeyword" => SelfKeyword,
        "operator" => Operator,
        "operatoroverloaded" => OverloadedOperator,
        "string" | "stringliteral" => String,
        "stringverbatim" | "verbatimstringliteral" => VerbatimString,
        "stringescapecharacter" | "escapesequence" | "formatspecifier" => StringEscape,
        "number" | "numericliteral" => Number,
        "comment" => Comment,
        "excludedcode" => ExcludedCode,
        "attribute" | "builtinattribute" | "decorator" | "derive" | "derivehelper" => Attribute,
        "lifetime" => Lifetime,
        "macro" => Macro,
        "regexcomment" => RegexComment,
        "regexcharacterclass" => RegexCharacterClass,
        "regexanchor" => RegexAnchor,
        "regexquantifier" => RegexQuantifier,
        "regexgrouping" => RegexGrouping,
        "regexalternation" => RegexAlternation,
        "regextext" | "regexp" => RegexText,
        "regexselfescapedcharacter" => RegexSelfEscapedCharacter,
        "regexotherescape" => RegexOtherEscape,
        "jsoncomment" => JsonComment,
        "jsonnumber" => JsonNumber,
        "jsonstring" => JsonString,
        "jsonkeyword" => JsonKeyword,
        "jsonproperty" => JsonPropertyName,
        "jsonoperator" | "jsonpunctuation" | "jsonarray" | "jsonobject" | "jsonconstructor" => {
            JsonPunctuation
        }
        "xmldoccommenttext" | "xmldoccommententityreference" => XmlDocCommentText,
        _ if n.starts_with("xmldoccomment") => XmlDocComment,
        _ => return None,
    })
}

/// The modifier a modifier name sets.
pub fn modifier(name: &str) -> Modifiers {
    match normalize(name).as_str() {
        "static" | "staticsymbol" => Modifiers::STATIC,
        "deprecated" | "obsoletesymbol" => Modifiers::DEPRECATED,
        "readonly" | "constant" => Modifiers::READONLY,
        "declaration" | "definition" => Modifiers::DECLARATION,
        "defaultlibrary" | "library" => Modifiers::DEFAULT_LIBRARY,
        "mutable" | "reassignedvariable" => Modifiers::MUTABLE,
        "async" => Modifiers::ASYNC,
        "abstract" => Modifiers::ABSTRACT,
        _ => Modifiers::default(),
    }
}

/// A legend resolved once: each type index's class and each modifier bit's meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Classifier {
    types: Vec<Option<TokenClass>>,
    modifiers: Vec<Modifiers>,
}

impl Classifier {
    pub fn new(legend: &Legend) -> Self {
        Self {
            types: legend.token_types.iter().map(|t| classify(t)).collect(),
            modifiers: legend.token_modifiers.iter().map(|m| modifier(m)).collect(),
        }
    }

    /// A token's class (`None`: an index outside the legend, or a type the editor leaves alone) and modifiers.
    pub fn classify(&self, token: &Token) -> (Option<TokenClass>, Modifiers) {
        let class = self.types.get(token.token_type as usize).copied().flatten();
        let mut m = Modifiers::default();
        let mut bits = token.modifiers;
        let mut i = 0usize;
        while bits != 0 {
            if bits & 1 == 1
                && let Some(x) = self.modifiers.get(i)
            {
                m = m | *x;
            }
            bits >>= 1;
            i += 1;
        }
        (class, m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(start: u32, delete_count: u32, data: &[u32]) -> SemanticTokensEdit {
        SemanticTokensEdit {
            start,
            delete_count,
            data: (!data.is_empty()).then(|| data.to_vec()),
        }
    }

    #[test]
    fn provider_from_capabilities() {
        let caps = json!({"semanticTokensProvider": {
            "legend": {"tokenTypes": ["namespace", "class"], "tokenModifiers": ["static"]},
            "full": {"delta": true}, "range": true
        }});
        let p = Provider::from_capabilities(&caps).unwrap();
        assert!(p.full && p.delta && p.range);
        assert_eq!(p.legend.token_types, ["namespace", "class"]);
        let full_only = json!({"semanticTokensProvider": {
            "legend": {"tokenTypes": [], "tokenModifiers": []}, "full": true
        }});
        let p = Provider::from_capabilities(&full_only).unwrap();
        assert!(p.full && !p.delta && !p.range);
        assert!(Provider::from_capabilities(&json!({})).is_none());
        assert!(
            Provider::from_capabilities(&json!({"semanticTokensProvider": {"legend": {}}}))
                .is_none()
        );
    }

    #[test]
    fn decodes_relative_positions() {
        // `class A { void M() {} }` over two lines: tokens at (0,6), (0,15) then (1,4).
        let data = [0, 6, 1, 1, 0, 0, 9, 1, 2, 1, 1, 4, 3, 0, 0, 7];
        let t = decode(&data);
        assert_eq!(
            t,
            [
                Token {
                    line: 0,
                    start: 6,
                    length: 1,
                    token_type: 1,
                    modifiers: 0
                },
                Token {
                    line: 0,
                    start: 15,
                    length: 1,
                    token_type: 2,
                    modifiers: 1
                },
                Token {
                    line: 1,
                    start: 4,
                    length: 3,
                    token_type: 0,
                    modifiers: 0
                },
            ]
        );
    }

    #[test]
    fn applies_delta_edits_from_the_last() {
        let mut data = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        apply_edits(&mut data, &[edit(0, 1, &[10, 11]), edit(5, 5, &[])]).unwrap();
        assert_eq!(data, [10, 11, 1, 2, 3, 4]);
        // An insert at the end, and a replace.
        apply_edits(&mut data, &[edit(6, 0, &[7, 7]), edit(2, 2, &[9])]).unwrap();
        assert_eq!(data, [10, 11, 9, 3, 4, 7, 7]);
        // Beyond the end, or overlapping: refused, nothing changed.
        let before = data.clone();
        assert_eq!(apply_edits(&mut data, &[edit(6, 5, &[])]), Err(DeltaError));
        assert_eq!(
            apply_edits(&mut data, &[edit(0, 3, &[]), edit(2, 1, &[])]),
            Err(DeltaError)
        );
        assert_eq!(data, before);
    }

    #[test]
    fn classifies_every_spelling() {
        use TokenClass::*;
        for (names, class) in [
            (&["class", "class name", "ClassName"][..], Class),
            (&["recordClass", "record class name"], Record),
            (&["interface", "interface name", "trait"], Interface),
            (&["struct", "struct name"], Struct),
            (&["typeParameter", "type parameter name"], TypeParameter),
            (&["method", "method name", "member"], Method),
            (
                &["extensionMethod", "extension method name"],
                ExtensionMethod,
            ),
            (&["variable", "local name"], Local),
            (&["parameter", "parameter name"], Parameter),
            (&["controlKeyword", "keyword - control"], ControlKeyword),
            (
                &["stringEscapeCharacter", "string - escape character"],
                StringEscape,
            ),
            (&["excludedCode", "excluded code"], ExcludedCode),
            (&["regexAnchor", "regex - anchor"], RegexAnchor),
            (
                &["jsonPropertyName", "json - property name"],
                JsonPropertyName,
            ),
            (&["selfKeyword"], SelfKeyword),
            (&["builtinType"], BuiltinType),
            (&["enumMember", "enum member name"], EnumMember),
            (
                &["xmlDocCommentDelimiter", "xml doc comment - delimiter"],
                XmlDocComment,
            ),
        ] {
            for n in names {
                assert_eq!(classify(n), Some(class), "{n}");
            }
        }
        for n in [
            "punctuation",
            "text",
            "preprocessorText",
            "unresolvedReference",
        ] {
            assert_eq!(classify(n), None, "{n}");
        }
    }

    #[test]
    fn classifier_reads_the_legend() {
        let legend = Legend {
            token_types: vec!["class".into(), "method".into(), "punctuation".into()],
            token_modifiers: vec![
                "static".into(),
                "ReassignedVariable".into(),
                "deprecated".into(),
            ],
        };
        let c = Classifier::new(&legend);
        let t = |token_type, modifiers| Token {
            line: 0,
            start: 0,
            length: 1,
            token_type,
            modifiers,
        };
        assert_eq!(
            c.classify(&t(1, 0b101)),
            (
                Some(TokenClass::Method),
                Modifiers::STATIC | Modifiers::DEPRECATED
            )
        );
        assert!(c.classify(&t(0, 0b10)).1.contains(Modifiers::MUTABLE));
        assert_eq!(c.classify(&t(2, 0)).0, None);
        assert_eq!(c.classify(&t(9, 1 << 20)).0, None);
    }
}
