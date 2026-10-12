//! Autos (brief 0077; Debug > Windows > Autos, Ctrl+Alt+V, A): the variables used on the current and previous
//! statement, as Visual Studio lists them. Roslyn's proximity expressions are not reachable from the shell, so this is a
//! token scan of the two lines: identifiers and member access chains (`order.Name`, `this.x`), skipping keywords,
//! strings, comments, numbers, type names after `new`, and names that are called (`Add(`). Each is evaluated in the
//! selected frame like a watch.

/// Expressions listed at most.
pub const MAX: usize = 20;

const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "var",
    "virtual",
    "void",
    "volatile",
    "while",
    "yield",
    "let",
    "when",
    "where",
    "select",
    "from",
    "nameof",
    "dynamic",
    "record",
    "init",
    "get",
    "set",
    "value",
    "global",
    "Dim",
    "As",
    "Then",
    "End",
    "Sub",
    "Function",
    "mut",
    "fn",
    "pub",
    "impl",
];

/// The expressions of `lines` (the previous statement's line first, then the current one), in order of appearance,
/// without duplicates.
pub fn expressions(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        for e in scan(line) {
            if out.len() >= MAX {
                return out;
            }
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

fn scan(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut after_new = false;
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    while i < chars.len() {
        let c = chars[i];
        // Comments end the line; strings and characters are skipped.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        if c == '"' || c == '\'' {
            let q = c;
            i += 1;
            while i < chars.len() && chars[i] != q {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            while i < chars.len() && (ident(chars[i]) || chars[i] == '.') {
                i += 1;
            }
            continue;
        }
        if !(c.is_alphabetic() || c == '_' || c == '@') {
            i += 1;
            continue;
        }
        // A chain: ident(.ident)*
        let mut parts: Vec<String> = Vec::new();
        loop {
            let s = i;
            if i < chars.len() && chars[i] == '@' {
                i += 1;
            }
            while i < chars.len() && ident(chars[i]) {
                i += 1;
            }
            parts.push(chars[s..i].iter().collect());
            if i + 1 < chars.len()
                && chars[i] == '.'
                && (chars[i + 1].is_alphabetic() || chars[i + 1] == '_')
            {
                i += 1;
                continue;
            }
            break;
        }
        let called = chars[i..].iter().find(|c| !c.is_whitespace()) == Some(&'(');
        let generic = chars.get(i) == Some(&'<');
        let first = parts[0].trim_start_matches('@').to_owned();
        let was_new = after_new;
        after_new = first == "new";
        if KEYWORDS.contains(&first.as_str()) && first != "this" {
            if first == "new" || parts.len() == 1 {
                continue;
            }
        }
        if was_new || generic {
            continue;
        }
        // A called member: the chain up to its object (`order.Total()` gives `order`); a call on a type name
        // (`Calc.Add(`, `System.Console.WriteLine(`) gives nothing.
        if called {
            if first.chars().next().is_some_and(char::is_uppercase) {
                continue;
            }
            parts.pop();
        }
        if parts.is_empty() || (parts.len() == 1 && parts[0] == "this") {
            continue;
        }
        out.push(parts.join("."));
    }
    out
}

/// The previous statement's line for the 0-based row `current` of `text`: the nearest line above that holds code (not
/// blank, not only braces, not a comment).
pub fn previous_line(text: &[&str], current: usize) -> Option<usize> {
    (0..current.min(text.len())).rev().find(|&r| {
        let l = text[r].trim();
        !l.is_empty() && !l.chars().all(|c| c == '{' || c == '}') && !l.starts_with("//")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autos_are_the_variables_of_the_two_statements() {
        // Program.cs of the debug tests: line 6 with line 5 before it.
        assert_eq!(
            expressions(&["        var x = 1;", "        var y = Calc.Add(x, 2);"]),
            ["x", "y"]
        );
        assert_eq!(
            expressions(&[
                "var order = new Order();",
                "System.Console.WriteLine(order.Name + \"a.b\"); // c.d"
            ]),
            ["order", "order.Name"]
        );
        assert_eq!(
            expressions(&["this._count += other.Total() * 2.5;"]),
            ["this._count", "other"]
        );
        assert_eq!(expressions(&["List<int> items = Make<int>();"]), ["items"]);
        assert_eq!(expressions(&["if (done) return;"]), ["done"]);
    }

    #[test]
    fn the_previous_statement_skips_braces_and_blank_lines() {
        let text = ["a();", "{", "", "   }", "b();"];
        assert_eq!(previous_line(&text, 4), Some(0));
        assert_eq!(previous_line(&text, 0), None);
    }
}
