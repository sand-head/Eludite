//! Languages as data: a grammar, a highlight query, the file suffixes (and
//! whole file names) it claims and the languages injected into it. Adding a
//! language is one [`LanguageConfig`] value passed to
//! [`LanguageRegistry::register`]; nothing else in the editor changes.
//!
//! The built-ins are C#, Rust (brief 0009), the web languages (brief 0050):
//! TypeScript, TSX, JavaScript (with JSX), HTML (with `<script>` and `<style>`
//! highlighted as JavaScript and CSS), CSS (also SCSS and Less, which its
//! grammar parses in their common subset), JSON and JSON with comments
//! (`.jsonc`, `tsconfig.json`, `jsconfig.json`; the JSON grammar accepts
//! comments, so only the id differs), and Razor (brief 0056: `.razor` and
//! `.cshtml`, from the in-repo `tree-sitter-razor`; its C# by the C# query,
//! its markup by its own, `<script>` and `<style>` as JavaScript and CSS).

use std::path::Path;
use std::sync::Arc;

use tree_sitter::{Query, QueryProperty};

use super::HighlightKind;
use crate::intellisense::emmet::EmmetSyntax;

/// Everything needed to highlight one language. Plain data, so a registration
/// can come from a table, a test, or later an extension manifest.
#[derive(Clone, Copy, Debug)]
pub struct LanguageConfig {
    /// Stable identifier, for example `csharp`.
    pub id: &'static str,
    /// Display name, for example `C#`.
    pub name: &'static str,
    /// File name suffixes without the dot, matched case-insensitively.
    pub path_suffixes: &'static [&'static str],
    /// Whole file names that select this language whatever their suffix
    /// (`tsconfig.json` is JSON with comments), matched case-insensitively
    /// before the suffixes.
    pub file_names: &'static [&'static str],
    /// The tree-sitter grammar.
    pub grammar: fn() -> tree_sitter::Language,
    /// A tree-sitter highlight query. Capture names map to [`HighlightKind`]
    /// through [`HighlightKind::from_capture_name`]; unknown names are ignored.
    pub highlights_query: &'static str,
    /// An injections query (empty for none): each match's
    /// `@injection.content` node is highlighted as the language its pattern
    /// names with `(#set! injection.language "<id>")`, one of [`Self::injected`].
    pub injections_query: &'static str,
    /// The languages the injections query names, by id.
    pub injected: &'static [LanguageConfig],
    /// The Emmet syntax its abbreviations expand in on Tab (brief 0050), if any.
    pub emmet: Option<EmmetSyntax>,
}

/// The C# registration: `tree-sitter-c-sharp` with Eludite's highlight query.
pub const CSHARP: LanguageConfig = LanguageConfig {
    id: "csharp",
    name: "C#",
    path_suffixes: &["cs", "csx"],
    file_names: &[],
    grammar: || tree_sitter_c_sharp::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/csharp/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// The Rust registration: `tree-sitter-rust` with Eludite's highlight query.
pub const RUST: LanguageConfig = LanguageConfig {
    id: "rust",
    name: "Rust",
    path_suffixes: &["rs"],
    file_names: &[],
    grammar: || tree_sitter_rust::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/rust/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// JavaScript with JSX: `tree-sitter-javascript` with Eludite's query (JSX's patterns first, so an attribute name is
/// not a plain property).
pub const JAVASCRIPT: LanguageConfig = LanguageConfig {
    id: "javascript",
    name: "JavaScript",
    path_suffixes: &["js", "jsx", "mjs", "cjs"],
    file_names: &[],
    grammar: || tree_sitter_javascript::LANGUAGE.into(),
    highlights_query: concat!(
        include_str!("../../queries/javascript/highlights-jsx.scm"),
        include_str!("../../queries/javascript/highlights.scm")
    ),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// TypeScript: `tree-sitter-typescript`'s TypeScript grammar; its query is
/// TypeScript's additions first, then JavaScript's.
pub const TYPESCRIPT: LanguageConfig = LanguageConfig {
    id: "typescript",
    name: "TypeScript",
    path_suffixes: &["ts", "mts", "cts"],
    file_names: &[],
    grammar: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
    highlights_query: concat!(
        include_str!("../../queries/typescript/highlights.scm"),
        include_str!("../../queries/javascript/highlights.scm")
    ),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// TSX: `tree-sitter-typescript`'s TSX grammar, the TypeScript query plus JSX.
pub const TSX: LanguageConfig = LanguageConfig {
    id: "tsx",
    name: "TypeScript JSX",
    path_suffixes: &["tsx"],
    file_names: &[],
    grammar: || tree_sitter_typescript::LANGUAGE_TSX.into(),
    highlights_query: concat!(
        include_str!("../../queries/typescript/highlights.scm"),
        include_str!("../../queries/javascript/highlights-jsx.scm"),
        include_str!("../../queries/javascript/highlights.scm")
    ),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// CSS, and SCSS and Less in the subset the CSS grammar parses.
pub const CSS: LanguageConfig = LanguageConfig {
    id: "css",
    name: "CSS",
    path_suffixes: &["css", "scss", "less"],
    file_names: &[],
    grammar: || tree_sitter_css::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/css/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: Some(EmmetSyntax::Css),
};

/// HTML with `<script>` as JavaScript and `<style>` as CSS.
pub const HTML: LanguageConfig = LanguageConfig {
    id: "html",
    name: "HTML",
    path_suffixes: &["html", "htm"],
    file_names: &[],
    grammar: || tree_sitter_html::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/html/highlights.scm"),
    injections_query: include_str!("../../queries/html/injections.scm"),
    injected: &[JAVASCRIPT, CSS],
    emmet: Some(EmmetSyntax::Html),
};

/// Razor, for Blazor components (`.razor`) and MVC and Razor Pages views
/// (`.cshtml`): `tree-sitter-razor` (grammars/razor, which extends the C#
/// grammar). Its query is the C# query followed by the Razor additions, so on
/// the same node C# wins inside code; `<script>` is JavaScript and `<style>`
/// CSS, and Emmet expands HTML abbreviations as in HTML.
pub const RAZOR: LanguageConfig = LanguageConfig {
    id: "razor",
    name: "Razor",
    path_suffixes: &["razor", "cshtml"],
    file_names: &[],
    grammar: || tree_sitter_razor::LANGUAGE.into(),
    highlights_query: concat!(
        include_str!("../../queries/csharp/highlights.scm"),
        include_str!("../../queries/razor/highlights.scm")
    ),
    injections_query: include_str!("../../queries/razor/injections.scm"),
    injected: &[JAVASCRIPT, CSS],
    emmet: Some(EmmetSyntax::Html),
};

/// JSON.
pub const JSON: LanguageConfig = LanguageConfig {
    id: "json",
    name: "JSON",
    path_suffixes: &["json"],
    file_names: &[],
    grammar: || tree_sitter_json::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/json/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// JSON with comments: the JSON grammar (which accepts comments) under its
/// own id, for `.jsonc` and the configuration files TypeScript and VS Code
/// read with comments.
pub const JSONC: LanguageConfig = LanguageConfig {
    id: "jsonc",
    name: "JSON with Comments",
    path_suffixes: &["jsonc"],
    file_names: &["tsconfig.json", "jsconfig.json", ".eslintrc.json"],
    grammar: || tree_sitter_json::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/json/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// Every built-in language, in registration order.
pub const BUILTINS: &[LanguageConfig] = &[
    CSHARP, RUST, TYPESCRIPT, TSX, JAVASCRIPT, HTML, RAZOR, CSS, JSON, JSONC,
];

/// A language injected into another one's text, and where.
pub(crate) struct Injections {
    pub(crate) query: Query,
    /// Capture index of `@injection.content`.
    pub(crate) content: u32,
    /// The injected language of each pattern.
    pub(crate) languages: Vec<Option<Arc<Language>>>,
}

/// A registered language with its compiled query. Shared between the UI
/// thread and highlight workers.
pub struct Language {
    config: LanguageConfig,
    grammar: tree_sitter::Language,
    query: Query,
    /// Highlight kind for each capture index of `query`.
    capture_kinds: Vec<Option<HighlightKind>>,
    injections: Option<Injections>,
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Language")
            .field("id", &self.config.id)
            .finish_non_exhaustive()
    }
}

/// Why a registration failed.
#[derive(Debug)]
pub enum LanguageError {
    /// The grammar's ABI version is not supported by the linked tree-sitter.
    Grammar(tree_sitter::LanguageError),
    /// The highlight query does not compile against the grammar.
    Query(tree_sitter::QueryError),
    /// The injections query names a language the configuration does not inject.
    Injection(String),
}

impl std::fmt::Display for LanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LanguageError::Grammar(e) => write!(f, "incompatible grammar: {e}"),
            LanguageError::Query(e) => write!(f, "invalid highlight query: {e}"),
            LanguageError::Injection(e) => write!(f, "invalid injection: {e}"),
        }
    }
}

impl std::error::Error for LanguageError {}

impl Language {
    /// Compile a registration. Fails if the query does not match the grammar.
    pub fn new(config: LanguageConfig) -> Result<Self, LanguageError> {
        super::alloc::install();
        let grammar = (config.grammar)();
        // Probe the ABI the same way a parser would.
        tree_sitter::Parser::new()
            .set_language(&grammar)
            .map_err(LanguageError::Grammar)?;
        let query = Query::new(&grammar, config.highlights_query).map_err(LanguageError::Query)?;
        let capture_kinds = query
            .capture_names()
            .iter()
            .map(|name| HighlightKind::from_capture_name(name))
            .collect();
        let injections = if config.injections_query.is_empty() {
            None
        } else {
            Some(Self::build_injections(&config, &grammar)?)
        };
        Ok(Self {
            config,
            grammar,
            query,
            capture_kinds,
            injections,
        })
    }

    fn build_injections(
        config: &LanguageConfig,
        grammar: &tree_sitter::Language,
    ) -> Result<Injections, LanguageError> {
        let query = Query::new(grammar, config.injections_query).map_err(LanguageError::Query)?;
        let content = query
            .capture_index_for_name("injection.content")
            .ok_or_else(|| LanguageError::Injection("no @injection.content capture".into()))?;
        let injected = config
            .injected
            .iter()
            .map(|c| Language::new(*c).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()?;
        let languages = (0..query.pattern_count())
            .map(|pattern| {
                let name = query
                    .property_settings(pattern)
                    .iter()
                    .find(|p: &&QueryProperty| &*p.key == "injection.language")
                    .and_then(|p| p.value.as_deref())
                    .map(str::to_owned);
                match name {
                    None => Ok(None),
                    Some(name) => injected
                        .iter()
                        .find(|l| l.id() == name)
                        .cloned()
                        .map(Some)
                        .ok_or_else(|| {
                            LanguageError::Injection(format!(
                                "{} injects {name}, which it does not list",
                                config.id
                            ))
                        }),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Injections {
            query,
            content,
            languages,
        })
    }

    pub fn id(&self) -> &'static str {
        self.config.id
    }

    pub fn name(&self) -> &'static str {
        self.config.name
    }

    pub fn config(&self) -> &LanguageConfig {
        &self.config
    }

    pub(crate) fn grammar(&self) -> &tree_sitter::Language {
        &self.grammar
    }

    pub(crate) fn query(&self) -> &Query {
        &self.query
    }

    pub(crate) fn injections(&self) -> Option<&Injections> {
        self.injections.as_ref()
    }

    pub(crate) fn capture_kind(&self, capture_index: u32) -> Option<HighlightKind> {
        self.capture_kinds
            .get(capture_index as usize)
            .copied()
            .flatten()
    }
}

/// The set of known languages.
#[derive(Debug, Default, Clone)]
pub struct LanguageRegistry {
    languages: Vec<Arc<Language>>,
}

impl LanguageRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The [`BUILTINS`]. Every query is compiled here; a failure is a bug in
    /// the shipped query and panics, which the crate's tests catch.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for &config in BUILTINS {
            registry
                .register(config)
                .unwrap_or_else(|e| panic!("built-in language {}: {e}", config.id));
        }
        registry
    }

    /// Add a language. A later registration with the same id replaces the
    /// earlier one.
    pub fn register(&mut self, config: LanguageConfig) -> Result<Arc<Language>, LanguageError> {
        let language = Arc::new(Language::new(config)?);
        self.languages.retain(|l| l.id() != config.id);
        self.languages.push(language.clone());
        Ok(language)
    }

    pub fn by_id(&self, id: &str) -> Option<Arc<Language>> {
        self.languages.iter().find(|l| l.id() == id).cloned()
    }

    /// The language whose file names hold the path's file name, else the one
    /// whose suffix matches its extension.
    pub fn for_path(&self, path: &Path) -> Option<Arc<Language>> {
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && let Some(l) = self.languages.iter().find(|l| {
                l.config
                    .file_names
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(name))
            })
        {
            return Some(l.clone());
        }
        let ext = path.extension()?.to_str()?;
        self.languages
            .iter()
            .find(|l| {
                l.config
                    .path_suffixes
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(ext))
            })
            .cloned()
    }

    pub fn languages(&self) -> impl Iterator<Item = &Arc<Language>> {
        self.languages.iter()
    }
}
