//! Brief 0057: Visual Basic and F# highlighting on the fixtures under
//! `corpus/languages/`, the language table's ids for their suffixes, and the
//! parse of every fixture (clean, or with the known error count for a construct
//! the grammar does not support, so a grammar bump that changes it is noticed).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use text::{Buffer as TextBuffer, BufferId, ReplicaId};

use super::{HighlightKind, HighlightUpdate, Highlighter, LanguageRegistry};
use HighlightKind::{
    Attribute, Comment, Constant, ConstantBuiltin, DocComment, Function, Keyword, Label, Namespace,
    Number, Parameter, Preprocessor, Property, String, Type, TypeBuiltin, Variable,
    VariableBuiltin,
};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/languages")
}

fn fixture(relative: &str) -> std::string::String {
    let path = corpus().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn buffer(text: &str) -> TextBuffer {
    TextBuffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), text)
}

fn highlight(id: &str, source: &str) -> HighlightUpdate {
    let registry = LanguageRegistry::with_builtins();
    let mut h = Highlighter::new(registry.by_id(id).unwrap_or_else(|| panic!("{id}")));
    let b = buffer(source);
    loop {
        let update = h.step(b.snapshot(), 0..0).expect("not cancelled");
        if update.complete {
            return update;
        }
    }
}

/// Kind at the first occurrence of `needle`.
fn kind_at(update: &HighlightUpdate, needle: &str) -> Option<HighlightKind> {
    let text = update.snapshot.text();
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in text"));
    update
        .highlights
        .kind_at(update.snapshot.offset_to_point(offset))
}

#[test]
fn the_language_table_resolves_the_dotnet_suffixes() {
    let registry = LanguageRegistry::with_builtins();
    let id = |name: &str| registry.for_path(Path::new(name)).map(|l| l.id());
    assert_eq!(id("Program.vb"), Some("vb"));
    assert_eq!(id("FORM1.VB"), Some("vb"));
    assert_eq!(id("Library.fs"), Some("fsharp"));
    assert_eq!(id("build.fsx"), Some("fsharp"));
    assert_eq!(id("script.fsscript"), Some("fsharp"));
    assert_eq!(id("Library.fsi"), Some("fsharp-signature"));
    assert_eq!(id("Program.cs"), Some("csharp"));
    assert_eq!(registry.by_id("vb").unwrap().name(), "Visual Basic");
    assert_eq!(registry.by_id("fsharp").unwrap().name(), "F#");
    assert_eq!(
        registry.by_id("fsharp-signature").unwrap().name(),
        "F# signature"
    );
}

#[test]
fn visual_basic_kinds_in_a_program() {
    let u = highlight("vb", &fixture("vb/Program.vb"));
    // Comments: `'''` documentation, `'` plain.
    assert_eq!(kind_at(&u, "''' <summary>"), Some(DocComment));
    assert_eq!(kind_at(&u, "' Eludite corpus"), Some(Comment));
    // Namespaces: the Imports and the Namespace block.
    assert_eq!(kind_at(&u, "System.IO"), Some(Namespace));
    assert_eq!(kind_at(&u, "Eludite.Corpus\n"), Some(Namespace));
    // Keywords: the modifiers are the only keyword nodes the grammar exposes
    // (brief 0057 report: `Module`, `End Module`, `Sub`, `If`, `Dim`, ... are
    // hidden tokens, so they stay in the default color).
    assert_eq!(kind_at(&u, "Public Module"), Some(Keyword));
    assert_eq!(kind_at(&u, "Private Const"), Some(Keyword));
    assert_eq!(kind_at(&u, "ReadOnly Started"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "Module Program"),
        None,
        "a hidden keyword token"
    );
    assert_eq!(kind_at(&u, "End Module"), None, "a hidden keyword token");
    assert_eq!(kind_at(&u, "If runs"), None, "a hidden keyword token");
    // Declarations.
    assert_eq!(kind_at(&u, "Program\n"), Some(Type));
    assert_eq!(kind_at(&u, "Main("), Some(Function));
    assert_eq!(kind_at(&u, "ReadFirstLine(path"), Some(Function));
    assert_eq!(kind_at(&u, "args As String"), Some(Parameter));
    assert_eq!(kind_at(&u, "Greeting As"), Some(Constant));
    assert_eq!(kind_at(&u, "MaxRetries%"), Some(Constant));
    assert_eq!(kind_at(&u, "Finish:"), Some(Label));
    // Types.
    assert_eq!(kind_at(&u, "Integer = 0"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "StreamReader(path)"), Some(Type));
    // Literals.
    assert_eq!(kind_at(&u, "\"Hello\""), Some(String));
    assert_eq!(kind_at(&u, "\"A\"c"), Some(String));
    assert_eq!(kind_at(&u, "\"She said"), Some(String));
    assert_eq!(kind_at(&u, "$\"{Greeting}"), Some(String));
    assert_eq!(
        kind_at(&u, "name}!"),
        Some(Variable),
        "an interpolation is not string"
    );
    assert_eq!(kind_at(&u, "0L"), Some(Number));
    assert_eq!(kind_at(&u, "1.5R"), Some(Number));
    assert_eq!(kind_at(&u, "&H7F"), Some(Number));
    assert_eq!(kind_at(&u, "#1/15/2026"), Some(Number));
    assert_eq!(kind_at(&u, "Nothing)"), Some(ConstantBuiltin));
    assert_eq!(kind_at(&u, "False AndAlso"), Some(ConstantBuiltin));
    // Calls and the preprocessor.
    assert_eq!(kind_at(&u, "WriteLine"), Some(Function));
    assert_eq!(kind_at(&u, "#If DEBUG"), Some(Preprocessor));
    assert_eq!(kind_at(&u, "total As Long, name"), Some(Parameter));
    assert_eq!(
        kind_at(&u, "runs += 1"),
        None,
        "locals use the default color"
    );
}

#[test]
fn visual_basic_kinds_in_type_declarations() {
    let u = highlight("vb", &fixture("vb/Shapes.vb"));
    assert_eq!(kind_at(&u, "Flags"), Some(Attribute));
    assert_eq!(kind_at(&u, "Serializable"), Some(Attribute));
    assert_eq!(kind_at(&u, "Description(\""), Some(Attribute));
    assert_eq!(
        kind_at(&u, "IShape\n"),
        Some(Type),
        "an interface declaration"
    );
    assert_eq!(kind_at(&u, "Color\n"), Some(Type), "an enum declaration");
    assert_eq!(
        kind_at(&u, "Point\n"),
        Some(Type),
        "a structure declaration"
    );
    assert_eq!(
        kind_at(&u, "Measure(shape"),
        Some(Type),
        "a delegate declaration"
    );
    assert_eq!(
        kind_at(&u, "Circle Inherits"),
        Some(Type),
        "a class declaration"
    );
    assert_eq!(kind_at(&u, "Shape\n"), Some(Type), "the inherited type");
    assert_eq!(kind_at(&u, "Point(0, 0)"), Some(Type), "the created type");
    assert_eq!(kind_at(&u, "Red\n"), Some(Constant), "an enum member");
    assert_eq!(kind_at(&u, "Name As String"), Some(Property));
    assert_eq!(kind_at(&u, "Fill As Color"), Some(Property));
    assert_eq!(kind_at(&u, "Overrides Function"), Some(Keyword));
    assert_eq!(kind_at(&u, "MustInherit"), Some(Keyword));
    assert_eq!(kind_at(&u, "ToString() As"), Some(Function));
    assert_eq!(kind_at(&u, "Double\n"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "Me.X"), Some(VariableBuiltin));
    assert_eq!(kind_at(&u, "MyBase.New"), Some(VariableBuiltin));
    assert_eq!(
        kind_at(&u, "Width = 800"),
        Some(Property),
        "an object initializer's member"
    );
    assert_eq!(
        kind_at(&u, "Math.PI"),
        None,
        "a member read uses the default color"
    );
}

#[test]
fn fsharp_kinds_in_a_module() {
    let u = highlight("fsharp", &fixture("fsharp/Library.fs"));
    assert_eq!(kind_at(&u, "/// The tax"), Some(DocComment));
    assert_eq!(kind_at(&u, "module Eludite"), Some(Keyword));
    assert_eq!(kind_at(&u, "Eludite.Corpus.Library"), Some(Namespace));
    assert_eq!(kind_at(&u, "open System"), Some(Keyword));
    assert_eq!(kind_at(&u, "System\n"), Some(Namespace));
    assert_eq!(kind_at(&u, "Literal"), Some(Attribute));
    assert_eq!(
        kind_at(&u, "TaxRate"),
        Some(Constant),
        "a [<Literal>] value"
    );
    assert_eq!(kind_at(&u, "0.2m"), Some(Number));
    assert_eq!(kind_at(&u, "let rec factorial"), Some(Keyword));
    assert_eq!(kind_at(&u, "rec factorial"), Some(Keyword));
    assert_eq!(kind_at(&u, "factorial n"), Some(Function));
    assert_eq!(
        kind_at(&u, "fib n acc1"),
        Some(Function),
        "after an access modifier"
    );
    assert_eq!(kind_at(&u, "mutable private"), Some(Keyword));
    assert_eq!(kind_at(&u, "private counter"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "counter = 0"),
        None,
        "a value binding uses the default color"
    );
    assert_eq!(kind_at(&u, "true"), Some(ConstantBuiltin));
    assert_eq!(kind_at(&u, "addTax"), Some(Function));
    assert_eq!(kind_at(&u, "rate: decimal"), Some(Parameter));
    assert_eq!(kind_at(&u, "decimal) (amount"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "usd>"), Some(Type), "a unit of measure");
    assert_eq!(kind_at(&u, "sprintf"), Some(Function));
    assert_eq!(kind_at(&u, "\"%s x%d\""), Some(String));
    assert_eq!(kind_at(&u, "with Name = newName"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "Name = newName"),
        Some(Property),
        "a record copy's field"
    );
    assert_eq!(
        kind_at(&u, "Discounted rate ->"),
        Some(Type),
        "an active pattern case"
    );
    assert_eq!(kind_at(&u, "Some tag"), Some(Type), "a union case");
    assert_eq!(kind_at(&u, "List.filter"), Some(Namespace));
    assert_eq!(kind_at(&u, "filter (fun"), Some(Function));
    assert_eq!(kind_at(&u, "fun o"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "sum\n"),
        Some(Function),
        "the function side of a pipe"
    );
    assert_eq!(
        kind_at(&u, "async {"),
        Some(Function),
        "a computation expression's builder"
    );
    assert_eq!(kind_at(&u, "let! text"), Some(Keyword));
    assert_eq!(kind_at(&u, "return text"), Some(Keyword));
    assert_eq!(kind_at(&u, "try\n"), Some(Keyword));
    assert_eq!(kind_at(&u, "with\n    | :?"), Some(Keyword));
    assert_eq!(kind_at(&u, "FileNotFoundException as e"), Some(Type));
    assert_eq!(
        kind_at(&u, "e.Message"),
        None,
        "a value uses the default color"
    );
    assert_eq!(kind_at(&u, "Message\n"), Some(Property));
    assert_eq!(kind_at(&u, "finally"), Some(Keyword));
    assert_eq!(kind_at(&u, "$\"Order {order.Id}"), Some(String));
    assert_eq!(
        kind_at(&u, "order.Id}"),
        Some(Variable),
        "an interpolation is not string"
    );
    assert_eq!(kind_at(&u, "@\"C:"), Some(String));
    assert_eq!(
        kind_at(&u, "\"\"\"\n"),
        Some(String),
        "a triple-quoted string"
    );
    assert_eq!(kind_at(&u, "'-'"), Some(String));
    assert_eq!(kind_at(&u, "#if INTERACTIVE"), Some(Preprocessor));
    assert_eq!(kind_at(&u, "INTERACTIVE"), Some(Preprocessor));
}

#[test]
fn fsharp_kinds_in_type_definitions() {
    let u = highlight("fsharp", &fixture("fsharp/Domain.fs"));
    assert_eq!(kind_at(&u, "(* Units"), Some(Comment));
    assert_eq!(kind_at(&u, "namespace Eludite"), Some(Keyword));
    assert_eq!(kind_at(&u, "Measure"), Some(Attribute));
    assert_eq!(kind_at(&u, "kg\n"), Some(Type));
    assert_eq!(kind_at(&u, "type Customer"), Some(Keyword));
    assert_eq!(kind_at(&u, "Customer =\n"), Some(Type));
    assert_eq!(kind_at(&u, "Id: Guid"), Some(Property), "a record field");
    assert_eq!(kind_at(&u, "Guid\n"), Some(Type));
    assert_eq!(kind_at(&u, "string option"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "override this"), Some(Keyword));
    assert_eq!(kind_at(&u, "this.Equals"), Some(VariableBuiltin));
    assert_eq!(kind_at(&u, "Equals(other"), Some(Function));
    assert_eq!(kind_at(&u, "Widget of"), Some(Type), "a union case");
    assert_eq!(kind_at(&u, "of count"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "count: int"),
        Some(Property),
        "a named union field"
    );
    assert_eq!(kind_at(&u, "Draft = 0"), Some(Type), "an enum case");
    assert_eq!(kind_at(&u, "exception OrderError"), Some(Keyword));
    assert_eq!(kind_at(&u, "OrderError of"), Some(Type));
    assert_eq!(kind_at(&u, "abstract Total"), Some(Keyword));
    assert_eq!(
        kind_at(&u, "Total: decimal"),
        Some(Function),
        "a member signature"
    );
    assert_eq!(
        kind_at(&u, "Order(id: int"),
        Some(Type),
        "a class with a primary constructor"
    );
    assert_eq!(kind_at(&u, "id: int, customer"), Some(Parameter));
    assert_eq!(kind_at(&u, "member this.Place"), Some(Keyword));
    assert_eq!(kind_at(&u, "this.Place"), Some(VariableBuiltin));
    assert_eq!(kind_at(&u, "Place()"), Some(Function));
    assert_eq!(kind_at(&u, "raise ("), Some(Keyword));
    assert_eq!(
        kind_at(&u, "OrderError(\"no items\""),
        Some(Type),
        "an exception constructor"
    );
    assert_eq!(kind_at(&u, "interface IPriced with"), Some(Keyword));
    assert_eq!(kind_at(&u, "IPriced with"), Some(Type));
    assert_eq!(
        kind_at(&u, "new IPriced"),
        Some(Keyword),
        "an object expression"
    );
    assert_eq!(kind_at(&u, "0m<usd>"), Some(Number));
    assert_eq!(kind_at(&u, "false"), Some(ConstantBuiltin));
}

#[test]
fn fsharp_kinds_in_a_program_and_a_script() {
    let u = highlight("fsharp", &fixture("fsharp/Program.fs"));
    assert_eq!(kind_at(&u, "EntryPoint"), Some(Attribute));
    assert_eq!(kind_at(&u, "main argv"), Some(Function));
    assert_eq!(kind_at(&u, "argv ="), Some(Parameter));
    assert_eq!(
        kind_at(&u, "Customer.Create"),
        Some(Type),
        "a type-qualified call's type"
    );
    assert_eq!(kind_at(&u, "Create \"Ada\""), Some(Function));
    assert_eq!(kind_at(&u, "printfn"), Some(Function));
    assert_eq!(kind_at(&u, "task {"), Some(Function));
    assert_eq!(kind_at(&u, "0\n"), Some(Number));

    let s = highlight("fsharp", &fixture("fsharp/Script.fsx"));
    assert_eq!(kind_at(&s, "#r \"nuget: FSharp.Data"), Some(Preprocessor));
    assert_eq!(kind_at(&s, "\"nuget: FSharp.Data, 6.4.0\""), Some(String));
    assert_eq!(kind_at(&s, "#load"), Some(Preprocessor));
    assert_eq!(kind_at(&s, "\"Domain.fs\""), Some(String));
    assert_eq!(kind_at(&s, "// Eludite corpus"), Some(Comment));
    assert_eq!(kind_at(&s, "AddPrinter"), Some(Function));
    assert_eq!(kind_at(&s, "2.5<kg>"), Some(Number));
}

#[test]
fn fsharp_signature_kinds() {
    let u = highlight("fsharp-signature", &fixture("fsharp/Domain.fsi"));
    assert_eq!(kind_at(&u, "/// Eludite corpus"), Some(DocComment));
    assert_eq!(kind_at(&u, "(* Units"), Some(Comment));
    assert_eq!(kind_at(&u, "namespace Eludite"), Some(Keyword));
    assert_eq!(kind_at(&u, "Eludite.Corpus.Domain"), Some(Namespace));
    assert_eq!(kind_at(&u, "open System"), Some(Keyword));
    assert_eq!(kind_at(&u, "Measure"), Some(Attribute));
    assert_eq!(kind_at(&u, "type Customer"), Some(Keyword));
    assert_eq!(kind_at(&u, "Customer =\n"), Some(Type));
    assert_eq!(kind_at(&u, "Id: Guid"), Some(Property));
    assert_eq!(kind_at(&u, "Guid\n"), Some(Type));
    assert_eq!(kind_at(&u, "string option"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "Widget of"), Some(Type));
    assert_eq!(kind_at(&u, "count: int"), Some(Property));
    assert_eq!(kind_at(&u, "Draft = 0"), Some(Type));
    assert_eq!(kind_at(&u, "0\n"), Some(Number));
    assert_eq!(kind_at(&u, "exception OrderError"), Some(Keyword));
    assert_eq!(kind_at(&u, "OrderError of"), Some(Type));
    assert_eq!(kind_at(&u, "abstract Total"), Some(Keyword));
    assert_eq!(kind_at(&u, "Total: decimal"), Some(Function));
    assert_eq!(kind_at(&u, "module Patterns"), Some(Keyword));
    assert_eq!(kind_at(&u, "Patterns ="), Some(Namespace));
    assert_eq!(kind_at(&u, "val free"), Some(Keyword));
    assert_eq!(kind_at(&u, "IPriced\n"), Some(Type));
}

/// The `ERROR` and `MISSING` nodes of `node`'s tree, one line each.
fn errors(node: tree_sitter::Node, text: &str, out: &mut Vec<std::string::String>) {
    if node.is_error() || node.is_missing() {
        let range = node.range();
        let end = range.end_byte.min(range.start_byte + 60);
        let snippet: std::string::String = text[range.start_byte..end]
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        out.push(format!(
            "    {}:{} {} `{snippet}`",
            range.start_point.row + 1,
            range.start_point.column + 1,
            if node.is_missing() {
                "MISSING"
            } else {
                "ERROR"
            }
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        errors(child, text, out);
    }
}

/// Fixtures for constructs the grammars do not support, with the number of
/// `ERROR` and `MISSING` nodes each produces today (tree-sitter-vb-dotnet
/// 0.1.0, tree-sitter-fsharp 0.3.12). Every other fixture parses clean. A
/// grammar bump that changes a count fails this test so the fixture and
/// `corpus/languages/README.md` are revisited. The gaps are listed in
/// `docs/briefs/0057-report.md`.
const KNOWN_ERROR_COUNTS: &[(&str, usize)] = &[
    ("vb/AddHandler.vb", 5),
    ("vb/ArrayDeclarations.vb", 3),
    ("vb/AsNew.vb", 5),
    ("vb/AsyncAwait.vb", 3),
    ("vb/AttributeInline.vb", 2),
    ("vb/CollectionInitializer.vb", 1),
    ("vb/CustomEvent.vb", 12),
    ("vb/EnumBase.vb", 1),
    ("vb/ForEachTyped.vb", 1),
    ("vb/Generics.vb", 9),
    ("vb/Handles.vb", 2),
    ("vb/Header.vb", 2),
    ("vb/IfCoalesce.vb", 1),
    ("vb/ImplementsMember.vb", 2),
    ("vb/InheritsLine.vb", 2),
    ("vb/Iterator.vb", 2),
    ("vb/KeywordPrefixes.vb", 5),
    ("vb/Linq.vb", 4),
    ("vb/NullConditional.vb", 2),
    ("vb/Operators.vb", 9),
    ("vb/RaiseEvent.vb", 2),
    ("vb/Regions.vb", 8),
    ("vb/TypeOfIs.vb", 1),
    ("vb/WithBlock.vb", 3),
    ("vb/XmlLiteral.vb", 9),
    ("fsharp/Members.fsi", 3),
];

fn fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fixtures(&path, out);
        } else if path.extension().is_some_and(|e| e != "md") {
            out.push(path);
        }
    }
}

#[test]
fn every_fixture_parses_cleanly_or_with_its_known_error_count() {
    let root = corpus();
    let mut files = Vec::new();
    fixtures(&root, &mut files);
    files.sort();
    assert!(
        files.len() > 30,
        "{} fixtures under {}",
        files.len(),
        root.display()
    );

    let registry = LanguageRegistry::with_builtins();
    let mut failures = Vec::new();
    let mut seen = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let language = registry
            .for_path(file)
            .unwrap_or_else(|| panic!("no language for {relative}"));
        let text = std::fs::read_to_string(file).unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language.grammar()).unwrap();
        let tree = parser.parse(&text, None).unwrap();
        let mut found = Vec::new();
        errors(tree.root_node(), &text, &mut found);
        let expected = KNOWN_ERROR_COUNTS
            .iter()
            .find(|(name, _)| *name == relative)
            .map_or(0, |(_, n)| *n);
        if expected != 0 {
            seen.push(relative.clone());
        }
        if found.len() != expected {
            failures.push(format!(
                "  {relative} ({}): {} error or missing nodes, expected {expected}\n{}",
                language.id(),
                found.len(),
                found.join("\n")
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    for (name, _) in KNOWN_ERROR_COUNTS {
        assert!(
            seen.iter().any(|s| s == name),
            "{name} is listed but not a fixture"
        );
    }
}

/// A 2,000-line sample made from a fixture: its header once, its body repeated.
fn long_sample(relative: &str, header_lines: usize, lines: usize) -> std::string::String {
    let text = fixture(relative);
    let mut all = text.lines();
    let header: Vec<&str> = all.by_ref().take(header_lines).collect();
    let body: Vec<&str> = all.collect();
    let mut out = header.join("\n");
    out.push('\n');
    while out.lines().count() < lines {
        out.push_str(&body.join("\n"));
        out.push('\n');
    }
    out
}

/// Brief 0057's budget: a 2,000-line VB file and a 2,000-line F# file each
/// parse in under 10 ms in release. Measured on request (`--release
/// --ignored`), printed, and held to a loose ceiling so a pathological grammar
/// is caught; the measured values are in `docs/briefs/0057-report.md`.
#[test]
#[ignore = "a timing measurement: run with --release --ignored --nocapture"]
fn a_2000_line_file_parses_within_the_budget() {
    let registry = LanguageRegistry::with_builtins();
    let samples = [
        ("vb", long_sample("vb/Program.vb", 6, 2000)),
        ("fsharp", long_sample("fsharp/Library.fs", 9, 2000)),
        (
            "fsharp-signature",
            long_sample("fsharp/Domain.fsi", 5, 2000),
        ),
    ];
    for (id, text) in &samples {
        let language = registry.by_id(id).unwrap();
        let mut best = Duration::MAX;
        for _ in 0..10 {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(language.grammar()).unwrap();
            let started = Instant::now();
            let tree = parser.parse(text, None).unwrap();
            best = best.min(started.elapsed());
            assert!(
                !tree.root_node().has_error(),
                "{id}: the sample must parse clean"
            );
        }
        println!(
            "{id}: {} lines parse in {best:?} (budget 10 ms)",
            text.lines().count()
        );
        assert!(best < Duration::from_millis(100), "{id}: {best:?}");
    }
}
