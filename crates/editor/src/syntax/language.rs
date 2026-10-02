//! Languages as data: a grammar, a highlight query and the file suffixes it
//! claims. Adding a language is one [`LanguageConfig`] value passed to
//! [`LanguageRegistry::register`]; nothing else in the editor changes.

use std::path::Path;
use std::sync::Arc;

use tree_sitter::Query;

use super::HighlightKind;

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
    /// The tree-sitter grammar.
    pub grammar: fn() -> tree_sitter::Language,
    /// A tree-sitter highlight query. Capture names map to [`HighlightKind`]
    /// through [`HighlightKind::from_capture_name`]; unknown names are ignored.
    pub highlights_query: &'static str,
}

/// The C# registration: `tree-sitter-c-sharp` with Niello's highlight query.
pub const CSHARP: LanguageConfig = LanguageConfig {
    id: "csharp",
    name: "C#",
    path_suffixes: &["cs", "csx"],
    grammar: || tree_sitter_c_sharp::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/csharp/highlights.scm"),
};

/// The Rust registration: `tree-sitter-rust` with Niello's highlight query.
pub const RUST: LanguageConfig = LanguageConfig {
    id: "rust",
    name: "Rust",
    path_suffixes: &["rs"],
    grammar: || tree_sitter_rust::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/rust/highlights.scm"),
};

/// A registered language with its compiled query. Shared between the UI
/// thread and highlight workers.
pub struct Language {
    config: LanguageConfig,
    grammar: tree_sitter::Language,
    query: Query,
    /// Highlight kind for each capture index of `query`.
    capture_kinds: Vec<Option<HighlightKind>>,
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
}

impl std::fmt::Display for LanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LanguageError::Grammar(e) => write!(f, "incompatible grammar: {e}"),
            LanguageError::Query(e) => write!(f, "invalid highlight query: {e}"),
        }
    }
}

impl std::error::Error for LanguageError {}

impl Language {
    /// Compile a registration. Fails if the query does not match the grammar.
    pub fn new(config: LanguageConfig) -> Result<Self, LanguageError> {
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
        Ok(Self {
            config,
            grammar,
            query,
            capture_kinds,
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

    /// C# and Rust. Both queries are compiled here; a failure is a bug in the
    /// shipped query and panics, which the crate's tests catch.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        for config in [CSHARP, RUST] {
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

    /// The language whose suffix matches the path's extension.
    pub fn for_path(&self, path: &Path) -> Option<Arc<Language>> {
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
