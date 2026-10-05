//! Razor language support for tree-sitter: the grammar of `.razor` and
//! `.cshtml` files (Razor directives, transitions and control structures over
//! tree-sitter-c-sharp's C#, with HTML markup), as the owner's library
//! `tree-sitter-razor` brought into Eludite (brief 0056; provenance and the
//! changes in `README.md`).
//!
//! Public API: [`LANGUAGE`], the parser for `tree_sitter::Parser::set_language`,
//! and [`NODE_TYPES`]. The editor's queries live in `crates/editor/queries/razor/`;
//! the library's own are kept in `queries/` for reference.
//!
//! ```
//! let code = r#"
//! @page "/"
//! <h1>Hello, @person</h1>
//!
//! @code {
//!     var person = "world";
//! }
//! "#;
//! let mut parser = tree_sitter::Parser::new();
//! parser
//!     .set_language(&tree_sitter_razor::LANGUAGE.into())
//!     .expect("Error loading Razor parser");
//! let tree = parser.parse(code, None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```

// Binding the generated C parser is unsafe by nature, as in every tree-sitter
// grammar crate; nothing else in the crate is.
#![allow(unsafe_code)]

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_razor() -> *const ();
}

/// The tree-sitter [`LanguageFn`] for this grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_razor) };

/// The content of the generated `node-types.json` for this grammar (written by
/// `build.rs` into `OUT_DIR` with the parser; see ADR-0012).
pub const NODE_TYPES: &str = include_str!(concat!(env!("OUT_DIR"), "/src/node-types.json"));

#[cfg(test)]
mod tests {
    #[test]
    fn can_load_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("Error loading Razor parser");
    }
}
