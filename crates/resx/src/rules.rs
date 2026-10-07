//! The consistency rules between a translation and its neutral value (proposal 0005 section 2): the ResX Resource
//! Manager extension's checks, each switchable by a setting (`resx.rules.*`). A rule answers a [`Warning`]; nothing
//! is refused.

/// A rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// The translation's format placeholders (`{0}`, `{name}`, `{0:N2}`, `%s`, `%d`) differ from the neutral value's.
    Placeholders,
    /// The leading or trailing punctuation differs.
    Punctuation,
    /// The leading or trailing white space differs.
    Whitespace,
    /// The translation equals the neutral value.
    Untranslated,
}

impl Rule {
    /// The rule's name as the schemas spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Placeholders => "placeholders",
            Rule::Punctuation => "punctuation",
            Rule::Whitespace => "whitespace",
            Rule::Untranslated => "untranslated",
        }
    }

    /// Every rule, in the schemas' order.
    pub const ALL: [Rule; 4] = [
        Rule::Placeholders,
        Rule::Punctuation,
        Rule::Whitespace,
        Rule::Untranslated,
    ];
}

/// Which rules run. All on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub placeholders: bool,
    pub punctuation: bool,
    pub whitespace: bool,
    pub untranslated: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            placeholders: true,
            punctuation: true,
            whitespace: true,
            untranslated: true,
        }
    }
}

impl Rules {
    /// No rule.
    pub const NONE: Rules = Rules {
        placeholders: false,
        punctuation: false,
        whitespace: false,
        untranslated: false,
    };

    /// The rules that run, in order.
    pub fn enabled(&self) -> Vec<Rule> {
        Rule::ALL.into_iter().filter(|r| self.is_on(*r)).collect()
    }

    fn is_on(&self, rule: Rule) -> bool {
        match rule {
            Rule::Placeholders => self.placeholders,
            Rule::Punctuation => self.punctuation,
            Rule::Whitespace => self.whitespace,
            Rule::Untranslated => self.untranslated,
        }
    }
}

/// A rule violation on one cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub key: String,
    pub culture: String,
    pub rule: Rule,
    pub message: String,
}

/// The warnings on `translation` (the value in `culture`) against `neutral`, under `rules`. A missing or empty
/// translation has no warnings (missing is its own state).
pub fn check(
    rules: &Rules,
    key: &str,
    culture: &str,
    neutral: &str,
    translation: &str,
) -> Vec<Warning> {
    let mut out = Vec::new();
    if translation.is_empty() || neutral.is_empty() {
        return out;
    }
    let warn = |out: &mut Vec<Warning>, rule: Rule, message: String| {
        out.push(Warning {
            key: key.to_string(),
            culture: culture.to_string(),
            rule,
            message,
        });
    };
    if rules.placeholders {
        let (a, b) = (placeholders(neutral), placeholders(translation));
        if a != b {
            warn(
                &mut out,
                Rule::Placeholders,
                format!(
                    "Placeholders differ: the neutral value has {}, this one has {}.",
                    list(&a),
                    list(&b)
                ),
            );
        }
    }
    if rules.punctuation {
        let (a, b) = (edge_punctuation(neutral), edge_punctuation(translation));
        if a.0 != b.0 {
            warn(
                &mut out,
                Rule::Punctuation,
                format!("Leading punctuation differs: {:?} against {:?}.", a.0, b.0),
            );
        } else if a.1 != b.1 {
            warn(
                &mut out,
                Rule::Punctuation,
                format!("Trailing punctuation differs: {:?} against {:?}.", a.1, b.1),
            );
        }
    }
    if rules.whitespace {
        let (a, b) = (edge_whitespace(neutral), edge_whitespace(translation));
        if a.0 != b.0 {
            warn(
                &mut out,
                Rule::Whitespace,
                format!(
                    "Leading white space differs: {} against {} characters.",
                    a.0, b.0
                ),
            );
        } else if a.1 != b.1 {
            warn(
                &mut out,
                Rule::Whitespace,
                format!(
                    "Trailing white space differs: {} against {} characters.",
                    a.1, b.1
                ),
            );
        }
    }
    if rules.untranslated && translation == neutral && neutral.chars().any(char::is_alphabetic) {
        warn(
            &mut out,
            Rule::Untranslated,
            "The value equals the neutral value: untranslated?".to_string(),
        );
    }
    out
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(" ")
    }
}

/// The format placeholders of a value, sorted: `{0}`, `{name}` and `{0:N2}` as `{0}`, `{{` and `}}` ignored, and
/// printf's `%s`, `%d`, `%i`, `%f`, `%u`, `%x` (with flags and widths) as `%s`-style tokens.
pub fn placeholders(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                if chars.get(i + 1) == Some(&'{') {
                    i += 2;
                    continue;
                }
                if let Some(close) = chars[i + 1..].iter().position(|c| *c == '}' || *c == '{') {
                    let end = i + 1 + close;
                    if chars[end] == '}' {
                        let inner: String = chars[i + 1..end].iter().collect();
                        let name = inner.split([':', ',']).next().unwrap_or("").trim();
                        if !name.is_empty() && !name.contains(char::is_whitespace) {
                            out.push(format!("{{{name}}}"));
                        }
                        i = end + 1;
                        continue;
                    }
                }
                i += 1;
            }
            '%' => {
                if chars.get(i + 1) == Some(&'%') {
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_digit() || "-+ #.".contains(chars[j])) {
                    j += 1;
                }
                if j < chars.len() && "sdifuxXeEgGc".contains(chars[j]) {
                    out.push(format!("%{}", chars[j]));
                    i = j + 1;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.sort();
    out
}

fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '.' | '!' | '?' | ':' | ';' | ',' | '…' | '¿' | '¡' | '。' | '！' | '？' | '：'
    )
}

fn edge_punctuation(value: &str) -> (String, String) {
    let value = value.trim();
    let lead: String = value.chars().take_while(|c| is_punctuation(*c)).collect();
    let trail: String = value
        .chars()
        .rev()
        .take_while(|c| is_punctuation(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (lead, trail)
}

fn edge_whitespace(value: &str) -> (usize, usize) {
    let lead = value.chars().take_while(|c| c.is_whitespace()).count();
    let trail = if lead == value.chars().count() {
        0
    } else {
        value
            .chars()
            .rev()
            .take_while(|c| c.is_whitespace())
            .count()
    };
    (lead, trail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_of(neutral: &str, translation: &str) -> Vec<Rule> {
        check(&Rules::default(), "K", "de", neutral, translation)
            .into_iter()
            .map(|w| w.rule)
            .collect()
    }

    #[test]
    fn placeholders_are_compared_as_sets() {
        assert_eq!(
            placeholders("{0} of {1:N2}, {{literal}} {name,5}"),
            vec!["{0}", "{1}", "{name}"]
        );
        assert_eq!(
            placeholders("%d items, %5.2f%% done %s"),
            vec!["%d", "%f", "%s"]
        );
        assert_eq!(rules_of("Hello {0}", "Hallo {0}"), Vec::<Rule>::new());
        assert_eq!(rules_of("{1} of {0}", "{0} von {1}"), Vec::<Rule>::new());
        assert_eq!(rules_of("Hello {0}", "Hallo"), vec![Rule::Placeholders]);
        assert_eq!(
            rules_of("Hello {0}", "Hallo {0} {1}"),
            vec![Rule::Placeholders]
        );
    }

    #[test]
    fn punctuation_and_whitespace_at_the_edges() {
        assert_eq!(rules_of("Save?", "Speichern"), vec![Rule::Punctuation]);
        assert_eq!(rules_of("Save", "Speichern..."), vec![Rule::Punctuation]);
        assert_eq!(rules_of("¿Save?", "Save?"), vec![Rule::Punctuation]);
        assert_eq!(rules_of("Name: ", "Name:"), vec![Rule::Whitespace]);
        assert_eq!(rules_of(" Name", "Name"), vec![Rule::Whitespace]);
        assert_eq!(rules_of("Save?", "Speichern?"), Vec::<Rule>::new());
        let w = check(&Rules::default(), "K", "de", "Save?", "Speichern");
        assert_eq!(
            w[0].message,
            "Trailing punctuation differs: \"?\" against \"\"."
        );
    }

    #[test]
    fn untranslated_needs_letters_and_the_rule_on() {
        assert_eq!(rules_of("Cancel", "Cancel"), vec![Rule::Untranslated]);
        assert_eq!(rules_of("100%", "100%"), Vec::<Rule>::new());
        assert_eq!(rules_of("Cancel", ""), Vec::<Rule>::new());
        let off = Rules {
            untranslated: false,
            ..Rules::default()
        };
        assert!(check(&off, "K", "de", "Cancel", "Cancel").is_empty());
        assert_eq!(
            off.enabled(),
            vec![Rule::Placeholders, Rule::Punctuation, Rule::Whitespace]
        );
        assert!(Rules::NONE.enabled().is_empty());
    }
}
