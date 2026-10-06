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
//! comments, so only the id differs), Razor (brief 0056: `.razor` and
//! `.cshtml`, from the in-repo `tree-sitter-razor`; its C# by the C# query,
//! its markup by its own, `<script>` and `<style>` as JavaScript and CSS),
//! and the other .NET languages (brief 0063): Visual Basic (`.vb`, from
//! `tree-sitter-vb-dotnet` with a query written here, since the grammar
//! ships none) and F# (`.fs`, `.fsx`, `.fsscript`, from `tree-sitter-fsharp`
//! with its query adapted to the editor's capture names) with its signature
//! files (`.fsi`, the same crate's signature grammar and a reduced query).

use std::path::Path;
use std::sync::{Arc, OnceLock};

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

/// Visual Basic: `tree-sitter-vb-dotnet` with Eludite's highlight query (the
/// grammar ships none). The grammar's keywords are hidden tokens, so only the
/// modifiers (`Public`, `Shared`, ...) can be highlighted as keywords; see the
/// query's header and `docs/briefs/0063-report.md` for what it does not parse.
pub const VISUAL_BASIC: LanguageConfig = LanguageConfig {
    id: "vb",
    name: "Visual Basic",
    path_suffixes: &["vb"],
    file_names: &[],
    grammar: || tree_sitter_vb_dotnet::LANGUAGE.into(),
    highlights_query: include_str!("../../queries/vb/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// F#: `tree-sitter-fsharp`'s main grammar with its highlight query adapted to
/// the editor's capture names and precedence rule.
pub const FSHARP: LanguageConfig = LanguageConfig {
    id: "fsharp",
    name: "F#",
    path_suffixes: &["fs", "fsx", "fsscript"],
    file_names: &[],
    grammar: || tree_sitter_fsharp::LANGUAGE_FSHARP.into(),
    highlights_query: include_str!("../../queries/fsharp/highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// F# signature files (`.fsi`): the same crate's signature grammar, which has
/// no expression layer, so the main query does not compile against it and a
/// reduced one is used.
pub const FSHARP_SIGNATURE: LanguageConfig = LanguageConfig {
    id: "fsharp-signature",
    name: "F# signature",
    path_suffixes: &["fsi"],
    file_names: &[],
    grammar: || tree_sitter_fsharp::LANGUAGE_SIGNATURE.into(),
    highlights_query: include_str!("../../queries/fsharp/signature-highlights.scm"),
    injections_query: "",
    injected: &[],
    emmet: None,
};

/// Every built-in language, in registration order.
pub const BUILTINS: &[LanguageConfig] = &[
    CSHARP,
    RUST,
    TYPESCRIPT,
    TSX,
    JAVASCRIPT,
    HTML,
    RAZOR,
    CSS,
    JSON,
    JSONC,
    VISUAL_BASIC,
    FSHARP,
    FSHARP_SIGNATURE,
];

/// A language injected into another one's text, and where.
pub(crate) struct Injections {
    pub(crate) query: Query,
    /// Capture index of `@injection.content`.
    pub(crate) content: u32,
    /// The injected language of each pattern.
    pub(crate) languages: Vec<Option<Arc<Language>>>,
}

/// A registered language. Shared between the UI thread and highlight workers.
///
/// The grammar is loaded and its ABI checked when the language is registered
/// (microseconds). The highlight and injection queries compile on first use
/// ([`Language::compile`], or the first highlight step on the syntax thread),
/// because `Query::new` costs tens to hundreds of milliseconds against a large
/// grammar (about 65 ms for C#, 90 ms for Razor, 300 ms for F#, brief 0063's
/// measurement), and the shell builds the registry on its startup path.
pub struct Language {
    config: LanguageConfig,
    grammar: tree_sitter::Language,
    compiled: OnceLock<Result<Compiled, LanguageError>>,
}

/// What compiling a language's queries produces.
struct Compiled {
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
    /// Register a language: load its grammar and check the ABI. Fails only
    /// for an incompatible grammar; the queries compile on first use (or
    /// [`Language::compile`]), and a query that does not match the grammar
    /// leaves the language without highlights ([`Language::compile`] reports
    /// the error; the crate's tests compile every built-in query).
    pub fn new(config: LanguageConfig) -> Result<Self, LanguageError> {
        super::alloc::install();
        let grammar = (config.grammar)();
        // Probe the ABI the same way a parser would.
        tree_sitter::Parser::new()
            .set_language(&grammar)
            .map_err(LanguageError::Grammar)?;
        Ok(Self {
            config,
            grammar,
            compiled: OnceLock::new(),
        })
    }

    /// Compile the highlight and injection queries now, if they are not yet,
    /// and report a query that does not match the grammar. Idempotent; safe
    /// from any thread (a second caller waits for the first).
    pub fn compile(&self) -> Result<(), &LanguageError> {
        self.compiled().as_ref().map(|_| ())
    }

    /// Whether the queries have been compiled (or failed to).
    pub fn is_compiled(&self) -> bool {
        self.compiled.get().is_some()
    }

    fn compiled(&self) -> &Result<Compiled, LanguageError> {
        self.compiled
            .get_or_init(|| Self::compile_queries(&self.config, &self.grammar))
    }

    fn compile_queries(
        config: &LanguageConfig,
        grammar: &tree_sitter::Language,
    ) -> Result<Compiled, LanguageError> {
        let query = Query::new(grammar, config.highlights_query).map_err(LanguageError::Query)?;
        let capture_kinds = query
            .capture_names()
            .iter()
            .map(|name| HighlightKind::from_capture_name(name))
            .collect();
        let injections = if config.injections_query.is_empty() {
            None
        } else {
            Some(Self::build_injections(config, grammar)?)
        };
        Ok(Compiled {
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

    /// The highlight query, compiled on first call; `None` when it does not
    /// compile against the grammar.
    pub(crate) fn query(&self) -> Option<&Query> {
        self.compiled().as_ref().ok().map(|c| &c.query)
    }

    pub(crate) fn injections(&self) -> Option<&Injections> {
        self.compiled()
            .as_ref()
            .ok()
            .and_then(|c| c.injections.as_ref())
    }

    pub(crate) fn capture_kind(&self, capture_index: u32) -> Option<HighlightKind> {
        self.compiled()
            .as_ref()
            .ok()?
            .capture_kinds
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

    /// The [`BUILTINS`], with their grammars loaded and nothing compiled yet
    /// (the queries compile on first use, or [`LanguageRegistry::warm_in_background`]);
    /// an incompatible grammar is a bug in the build and panics. The crate's
    /// tests compile every built-in query.
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
    /// earlier one. Fails for an incompatible grammar; the queries compile on
    /// first use ([`Language::compile`] checks them now).
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

    /// Compile every registered language's queries on a thread of its own,
    /// so the first highlight of each language finds them ready while the
    /// caller (the shell at startup) goes on at once. Query compilation is
    /// small allocations, not a parse tree, so it needs no place on the
    /// syntax thread; a language used before the warm-up reaches it compiles
    /// on whichever thread asks first, and the other waits.
    pub fn warm_in_background(&self) -> std::thread::JoinHandle<()> {
        let languages: Vec<Arc<Language>> = self.languages.clone();
        std::thread::Builder::new()
            .name("eludite-syntax-warm".into())
            .spawn(move || {
                for language in &languages {
                    let _ = language.compile();
                }
            })
            .expect("spawn the syntax warm-up thread")
    }
}

#[cfg(test)]
mod lazy_tests {
    use super::*;

    #[test]
    fn the_registry_compiles_no_query_until_a_language_is_used() {
        let started = std::time::Instant::now();
        let registry = LanguageRegistry::with_builtins();
        let building = started.elapsed();
        assert!(registry.languages().all(|l| !l.is_compiled()));
        let csharp = registry.by_id("csharp").unwrap();
        csharp.compile().unwrap();
        assert!(csharp.is_compiled());
        assert!(registry.by_id("fsharp").unwrap().query().is_some());
        eprintln!("timing: with_builtins without compiling queries {building:?}");
    }

    #[test]
    fn every_builtin_query_compiles() {
        let registry = LanguageRegistry::with_builtins();
        for language in registry.languages() {
            language
                .compile()
                .unwrap_or_else(|e| panic!("built-in language {}: {e}", language.id()));
        }
    }

    #[test]
    fn the_warm_up_compiles_everything_off_the_caller() {
        let registry = LanguageRegistry::with_builtins();
        registry.warm_in_background().join().unwrap();
        assert!(registry.languages().all(|l| l.is_compiled()));
    }

    #[test]
    fn a_query_that_does_not_match_leaves_the_language_without_highlights() {
        let broken = LanguageConfig {
            id: "broken",
            name: "Broken",
            path_suffixes: &["broken"],
            file_names: &[],
            grammar: || tree_sitter_json::LANGUAGE.into(),
            highlights_query: "(no_such_node) @keyword",
            injections_query: "",
            injected: &[],
            emmet: None,
        };
        let language = Language::new(broken).expect("the grammar is fine");
        assert!(matches!(language.compile(), Err(LanguageError::Query(_))));
        assert!(language.query().is_none());
        assert_eq!(language.capture_kind(0), None);
    }
}
