//! Which tests Run Tests (Ctrl+R, T) and Debug Tests (Ctrl+R, Ctrl+T) take from the editor's caret (brief 0076), as
//! Visual Studio's "run tests in context" does:
//!
//! - the caret in a test method (its attributes, signature and body): that method's tests (every data row of it);
//! - the caret on a test class's declaration line, or in its body between members: the class's tests;
//! - anywhere else in a file with tests (a non-test member, a using line): the file's tests, never the test above.
//!
//! The extents come from the tests' reported lines and a scan of the document: braces for C#, Rust and the other brace
//! languages, `End Sub`/`End Function`/`End Class` for Visual Basic, indentation for F#. A test's reported line is its
//! attribute (xunit.v3), its method's opening brace (VSTest's DIA lookup) or its `fn` line (Rust).

/// A test of the document, as the scope rule needs it.
#[derive(Debug, Clone)]
pub struct ScopeTest {
    pub id: String,
    /// 1-based line the runner reported.
    pub line: Option<u32>,
    /// `Namespace.Class.Method` (data rows share it).
    pub full_name: String,
    /// The class's group path (namespace, class).
    pub group: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Braces,
    VisualBasic,
    FSharp,
}

fn syntax_of(path: &str) -> Syntax {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".vb") {
        Syntax::VisualBasic
    } else if lower.ends_with(".fs") || lower.ends_with(".fsx") {
        Syntax::FSharp
    } else {
        Syntax::Braces
    }
}

/// The ids of the tests `caret_line` (1-based) selects in the document `text` at `path` (see the module docs).
pub fn tests_at_caret(path: &str, text: &str, tests: &[ScopeTest], caret_line: u32) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let syntax = syntax_of(path);
    let all: Vec<String> = tests.iter().map(|t| t.id.clone()).collect();
    if lines.is_empty() || tests.is_empty() {
        return all;
    }
    // The methods: one extent per distinct reported line.
    let mut methods: Vec<(u32, u32, &ScopeTest)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for t in tests {
        let Some(l) = t.line else { continue };
        if !seen.insert(l) {
            continue;
        }
        let (start, end) = method_extent(&lines, l, syntax);
        methods.push((start, end, t));
    }
    if let Some((_, _, t)) = methods
        .iter()
        .filter(|(s, e, _)| (*s..=*e).contains(&caret_line))
        .min_by_key(|(s, e, _)| e - s)
    {
        return tests
            .iter()
            .filter(|x| x.full_name == t.full_name)
            .map(|x| x.id.clone())
            .collect();
    }
    // A class: its declaration line, or its body outside any member.
    let mut classes: Vec<&Vec<String>> = tests.iter().map(|t| &t.group).collect();
    classes.dedup();
    for group in classes {
        let in_class: Vec<&ScopeTest> = tests.iter().filter(|t| &t.group == group).collect();
        let Some(first) = methods
            .iter()
            .filter(|(_, _, t)| &t.group == group)
            .map(|(s, _, _)| *s)
            .min()
        else {
            continue;
        };
        let class_name = group.last().map(String::as_str).unwrap_or_default();
        let Some(decl) = class_line(&lines, first, class_name, syntax) else {
            continue;
        };
        let end = class_end(&lines, decl, syntax);
        let ids = || in_class.iter().map(|t| t.id.clone()).collect();
        if caret_line == decl {
            return ids();
        }
        if caret_line > decl && caret_line <= end {
            let here = lines.get(caret_line as usize - 1).map_or("", |l| l.trim());
            let between = here.is_empty()
                || here.starts_with("//")
                || here.starts_with('\'')
                || here == "{"
                || here == "}"
                || here.starts_with("#region")
                || here.starts_with("#endregion");
            if between && !inside_member(&lines, decl, caret_line, syntax) {
                return ids();
            }
            return all;
        }
    }
    all
}

/// The first and last line of the method whose test reported `line`.
fn method_extent(lines: &[&str], line: u32, syntax: Syntax) -> (u32, u32) {
    let at = (line as usize).clamp(1, lines.len());
    let end = match syntax {
        Syntax::Braces => brace_end(lines, at),
        Syntax::VisualBasic => keyword_end(lines, at, &["end sub", "end function"]),
        Syntax::FSharp => indent_end(lines, at),
    };
    // Up over the signature, attributes and doc comments, to a blank line or the end of what precedes.
    let mut start = at;
    while start > 1 {
        let prev = lines[start - 2].trim();
        let lower = prev.to_ascii_lowercase();
        let declares_type = ["class ", "type ", "module ", "structure ", "mod "]
            .iter()
            .any(|k| lower.starts_with(k) || lower.contains(&format!(" {k}")));
        if prev.is_empty()
            || declares_type
            || prev.ends_with('}')
            || prev.ends_with(';')
            || prev.ends_with('{')
            || (syntax == Syntax::VisualBasic && prev.to_ascii_lowercase().starts_with("end "))
        {
            break;
        }
        start -= 1;
    }
    (start as u32, end as u32)
}

/// From line `at` (1-based): the line closing the first block opened at or after it, or the first `;` line of an
/// expression-bodied member.
fn brace_end(lines: &[&str], at: usize) -> usize {
    let mut depth = 0i32;
    let mut opened = false;
    for (i, l) in lines.iter().enumerate().skip(at - 1) {
        for c in strip_comment(l).chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                ';' if !opened && depth == 0 => return i + 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return i + 1;
        }
    }
    lines.len()
}

fn strip_comment(l: &str) -> &str {
    l.split_once("//").map_or(l, |(code, _)| code)
}

fn keyword_end(lines: &[&str], at: usize, ends: &[&str]) -> usize {
    lines
        .iter()
        .enumerate()
        .skip(at - 1)
        .find(|(_, l)| {
            let t = l.trim().to_ascii_lowercase();
            ends.iter().any(|e| t.starts_with(e))
        })
        .map_or(lines.len(), |(i, _)| i + 1)
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// F#: the member at `at` (its attributes first) ends before the next line indented no deeper than it.
fn indent_end(lines: &[&str], at: usize) -> usize {
    let mut head = at;
    while head <= lines.len() && lines[head - 1].trim_start().starts_with("[<") {
        head += 1;
    }
    let Some(base) = lines.get(head - 1).map(|l| indent(l)) else {
        return lines.len();
    };
    let mut last = head;
    for (i, l) in lines.iter().enumerate().skip(head) {
        if l.trim().is_empty() {
            continue;
        }
        if indent(l) <= base {
            break;
        }
        last = i + 1;
    }
    last
}

/// The declaration line of the class `name` that holds the member starting at `first`.
fn class_line(lines: &[&str], first: u32, name: &str, syntax: Syntax) -> Option<u32> {
    let keyword = match syntax {
        Syntax::Braces => ["class ", "struct ", "record ", "mod "],
        Syntax::VisualBasic => ["class ", "module ", "structure ", "class "],
        Syntax::FSharp => ["type ", "module ", "type ", "type "],
    };
    (1..first).rev().find_map(|n| {
        let l = lines.get(n as usize - 1)?;
        let lower = l.to_ascii_lowercase();
        let declares = keyword.iter().any(|k| {
            lower.split_once(k).is_some_and(|(before, after)| {
                (before.is_empty() || before.ends_with(' ') || before.ends_with('\t'))
                    && after
                        .trim_start()
                        .to_ascii_lowercase()
                        .starts_with(&name.to_ascii_lowercase().replace('+', "."))
            })
        });
        let simple = name.rsplit(['+', '.']).next().unwrap_or(name);
        let plain = keyword.iter().any(|k| {
            lower.split_once(k).is_some_and(|(before, after)| {
                before.trim().split_whitespace().all(|w| {
                    matches!(
                        w,
                        "public"
                            | "internal"
                            | "private"
                            | "protected"
                            | "sealed"
                            | "static"
                            | "partial"
                            | "abstract"
                            | "pub"
                            | "friend"
                            | "notinheritable"
                            | "pub(crate)"
                    )
                }) && after.trim_start().starts_with(&simple.to_ascii_lowercase())
            })
        });
        (declares || plain).then_some(n)
    })
}

fn class_end(lines: &[&str], decl: u32, syntax: Syntax) -> u32 {
    (match syntax {
        Syntax::Braces => brace_end(lines, decl as usize),
        Syntax::VisualBasic => keyword_end(
            lines,
            decl as usize,
            &["end class", "end module", "end structure"],
        ),
        Syntax::FSharp => indent_end(lines, decl as usize),
    }) as u32
}

/// Whether `line` lies in a member's body of the class declared at `decl` (braces deeper than the class body's).
fn inside_member(lines: &[&str], decl: u32, line: u32, syntax: Syntax) -> bool {
    if syntax != Syntax::Braces {
        return false;
    }
    let mut depth = 0i32;
    let mut body = None;
    for (i, l) in lines
        .iter()
        .enumerate()
        .take(line as usize - 1)
        .skip(decl as usize - 1)
    {
        for c in strip_comment(l).chars() {
            match c {
                '{' => {
                    depth += 1;
                    if body.is_none() {
                        body = Some(depth);
                    }
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        let _ = i;
    }
    body.is_some_and(|b| depth > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS: &str = "using Xunit;\n\
namespace Corpus.Tests\n\
{\n\
    public class CalculatorTests\n\
    {\n\
        [Fact]\n\
        public void Adds()\n\
        {\n\
            Assert.Equal(5, 2 + 3);\n\
        }\n\
\n\
        private int Helper()\n\
        {\n\
            return 1;\n\
        }\n\
\n\
        [Theory]\n\
        [InlineData(1)]\n\
        public void Pairs(int a)\n\
        {\n\
            Assert.True(a > 0);\n\
        }\n\
    }\n\
}\n";

    fn t(id: &str, line: u32, full: &str) -> ScopeTest {
        ScopeTest {
            id: id.into(),
            line: Some(line),
            full_name: full.into(),
            group: vec!["Corpus.Tests".into(), "CalculatorTests".into()],
        }
    }

    fn tests() -> Vec<ScopeTest> {
        vec![
            t("adds", 6, "Corpus.Tests.CalculatorTests.Adds"),
            t("p1", 17, "Corpus.Tests.CalculatorTests.Pairs"),
            ScopeTest {
                line: None,
                ..t("p2", 0, "Corpus.Tests.CalculatorTests.Pairs")
            },
        ]
    }

    #[test]
    fn the_caret_picks_the_method_the_class_or_the_file() {
        let ts = tests();
        let at = |l| tests_at_caret("/s/C.cs", CS, &ts, l);
        // In a test's body, signature and attribute.
        assert_eq!(at(9), ["adds"]);
        assert_eq!(at(7), ["adds"]);
        assert_eq!(at(6), ["adds"]);
        // Every data row of the method.
        assert_eq!(at(21), ["p1", "p2"]);
        assert_eq!(at(18), ["p1", "p2"]);
        // The class line, and between members: the class's tests.
        assert_eq!(at(4), ["adds", "p1", "p2"]);
        assert_eq!(at(11), ["adds", "p1", "p2"]);
        // A non-test member: the file's, never the test above.
        assert_eq!(at(14), ["adds", "p1", "p2"]);
        assert_eq!(at(12), ["adds", "p1", "p2"]);
        // The using line.
        assert_eq!(at(1), ["adds", "p1", "p2"]);
    }

    #[test]
    fn a_debug_info_line_on_the_brace_holds_the_signature() {
        // VSTest reports the opening brace's line.
        let ts = vec![
            t("adds", 8, "Corpus.Tests.CalculatorTests.Adds"),
            t("pairs", 20, "Corpus.Tests.CalculatorTests.Pairs"),
        ];
        assert_eq!(tests_at_caret("/s/C.cs", CS, &ts, 7), ["adds"]);
        assert_eq!(tests_at_caret("/s/C.cs", CS, &ts, 19), ["pairs"]);
        assert_eq!(tests_at_caret("/s/C.cs", CS, &ts, 13), ["adds", "pairs"]);
    }

    #[test]
    fn visual_basic_and_fsharp() {
        let vb = "Imports Microsoft.VisualStudio.TestTools.UnitTesting\n\
<TestClass>\n\
Public Class ParserTests\n\
    <TestMethod>\n\
    Public Sub ParsesNumbers()\n\
        Assert.AreEqual(1, 1)\n\
    End Sub\n\
\n\
    Private Function Helper() As Integer\n\
        Return 1\n\
    End Function\n\
End Class\n";
        let ts = vec![ScopeTest {
            id: "n".into(),
            line: Some(4),
            full_name: "Corpus.ParserTests.ParsesNumbers".into(),
            group: vec!["Corpus".into(), "ParserTests".into()],
        }];
        assert_eq!(tests_at_caret("/s/P.vb", vb, &ts, 6), ["n"]);
        assert_eq!(tests_at_caret("/s/P.vb", vb, &ts, 3), ["n"]);
        assert_eq!(tests_at_caret("/s/P.vb", vb, &ts, 10), ["n"]);
        let fs = "module Tests\n\
\n\
type QueueTests() =\n\
    [<Test>]\n\
    member _.Enqueues() =\n\
        Assert.Pass()\n\
\n\
    member _.Helper() =\n\
        1\n";
        let ts = vec![ScopeTest {
            id: "e".into(),
            line: Some(5),
            full_name: "Tests.QueueTests.Enqueues".into(),
            group: vec!["Tests".into(), "QueueTests".into()],
        }];
        assert_eq!(tests_at_caret("/s/T.fs", fs, &ts, 6), ["e"]);
        assert_eq!(tests_at_caret("/s/T.fs", fs, &ts, 3), ["e"]);
    }
}
