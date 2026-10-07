//! Resource sets: a neutral file with the culture files beside it, found from any of them or by walking a folder,
//! and their rows for the grid.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::cultures::CULTURES;
use crate::file::{ParseError, ResxFile};
use crate::rules::{self, Rules, Warning};

/// The comment marker of an invariant key, as the ResX Resource Manager extension stores it.
pub const INVARIANT_MARKER: &str = "{Invariant}";

fn culture_table() -> &'static HashMap<String, &'static str> {
    static TABLE: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| {
        CULTURES
            .iter()
            .map(|c| (c.to_ascii_lowercase(), *c))
            .collect()
    })
}

/// The culture name as the table spells it, for any casing (`de-de` to `de-DE`); `None` for a name that is not one.
pub fn canonical_culture(name: &str) -> Option<&'static str> {
    culture_table().get(&name.to_ascii_lowercase()).copied()
}

pub fn is_culture(name: &str) -> bool {
    canonical_culture(name).is_some()
}

/// A `.resx` file name split into its base name and its culture: `Resources.de-DE.resx` is `("Resources",
/// Some("de-DE"))`, `Default.aspx.resx` is `("Default.aspx", None)`; a name that is not `.resx` is `None`.
pub fn base_name(file_name: &str) -> Option<(String, Option<&'static str>)> {
    let stem = file_name
        .strip_suffix(".resx")
        .or_else(|| file_name.strip_suffix(".RESX"))?;
    if let Some((base, suffix)) = stem.rsplit_once('.')
        && !base.is_empty()
        && let Some(culture) = canonical_culture(suffix)
    {
        return Some((base.to_string(), Some(culture)));
    }
    Some((stem.to_string(), None))
}

/// The files of a set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetFiles {
    /// The neutral file, whether or not it exists.
    pub neutral: PathBuf,
    pub neutral_exists: bool,
    pub base_name: String,
    /// The culture files beside it, by culture name.
    pub cultures: Vec<(String, PathBuf)>,
}

impl SetFiles {
    /// The path of `culture`'s file, existing or not; the neutral file for the empty culture.
    pub fn culture_path(&self, culture: &str) -> PathBuf {
        if culture.is_empty() {
            return self.neutral.clone();
        }
        if let Some((_, p)) = self
            .cultures
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(culture))
        {
            return p.clone();
        }
        self.neutral
            .with_file_name(format!("{}.{}.resx", self.base_name, culture))
    }
}

/// The set a `.resx` file belongs to, from the files beside it. Reads the directory only.
pub fn set_files(path: &Path) -> io::Result<SetFiles> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let Some((base, _)) = base_name(file_name) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a .resx file", path.display()),
        ));
    };
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut cultures = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some((b, Some(c))) = base_name(name)
            && b == base
        {
            cultures.push((c.to_string(), entry.path()));
        }
    }
    cultures.sort_by(|a, b| a.0.cmp(&b.0));
    let neutral = dir.join(format!("{base}.resx"));
    let neutral_exists = neutral.is_file();
    Ok(SetFiles {
        neutral,
        neutral_exists,
        base_name: base,
        cultures,
    })
}

/// A set being collected: whether its neutral file was seen, and its culture files.
type Group = (bool, Vec<(String, PathBuf)>);

/// The sets under `root`: every `.resx` the walker finds (`.gitignore` honored when `use_gitignore`; `excludes` as
/// `.gitignore` patterns against the path inside `root`), grouped by folder and base name, by folder then name.
pub fn discover(
    root: &Path,
    excludes: &[String],
    use_gitignore: bool,
) -> io::Result<Vec<SetFiles>> {
    let mut overrides = ignore::overrides::OverrideBuilder::new(root);
    for glob in excludes {
        overrides
            .add(&format!("!{glob}"))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    }
    let overrides = overrides
        .build()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let walker = ignore::WalkBuilder::new(root)
        .overrides(overrides)
        .git_ignore(use_gitignore)
        .git_global(use_gitignore)
        .git_exclude(use_gitignore)
        .ignore(use_gitignore)
        .hidden(false)
        .build();
    let mut groups: BTreeMap<(PathBuf, String), Group> = BTreeMap::new();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        let Some((base, culture)) = base_name(name) else {
            continue;
        };
        let dir = entry
            .path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let group = groups.entry((dir, base)).or_default();
        match culture {
            None => group.0 = true,
            Some(c) => group.1.push((c.to_string(), entry.path().to_path_buf())),
        }
    }
    Ok(groups
        .into_iter()
        .map(|((dir, base), (neutral_exists, mut cultures))| {
            cultures.sort_by(|a, b| a.0.cmp(&b.0));
            SetFiles {
                neutral: dir.join(format!("{base}.resx")),
                neutral_exists,
                base_name: base,
                cultures,
            }
        })
        .collect())
}

/// Whether a comment carries the invariant marker.
pub fn has_invariant(comment: Option<&str>) -> bool {
    comment.is_some_and(|c| {
        c.to_ascii_lowercase()
            .contains(&INVARIANT_MARKER.to_ascii_lowercase())
    })
}

/// The comment with the marker added or removed, trimmed; `None` when nothing is left.
pub fn with_invariant(comment: Option<&str>, invariant: bool) -> Option<String> {
    let mut text = comment.unwrap_or("").to_string();
    let marker = INVARIANT_MARKER.to_ascii_lowercase();
    if let Some(i) = text.to_ascii_lowercase().find(&marker) {
        text.replace_range(i..i + marker.len(), "");
        return with_invariant(Some(text.trim()), invariant);
    }
    let text = text.trim().to_string();
    if invariant {
        Some(if text.is_empty() {
            INVARIANT_MARKER.to_string()
        } else {
            format!("{INVARIANT_MARKER} {text}")
        })
    } else if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Why a set could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, io::Error),
    Parse(PathBuf, ParseError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            LoadError::Parse(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for LoadError {}

/// One culture's value and comment for a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The empty string for the neutral file.
    pub culture: String,
    /// `None` when the culture file has no entry for the key.
    pub value: Option<String>,
    pub comment: Option<String>,
    pub line: Option<u32>,
    pub warnings: Vec<Warning>,
}

impl Cell {
    pub fn missing(&self) -> bool {
        self.value.is_none()
    }
}

/// A key with its cells, the neutral one first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub invariant: bool,
    pub cells: Vec<Cell>,
}

impl Row {
    pub fn cell(&self, culture: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.culture == culture)
    }

    /// Cultures with no value, invariant keys excluded.
    pub fn missing_count(&self) -> usize {
        if self.invariant {
            0
        } else {
            self.cells.iter().skip(1).filter(|c| c.missing()).count()
        }
    }

    pub fn warning_count(&self) -> usize {
        self.cells.iter().map(|c| c.warnings.len()).sum()
    }
}

/// A loaded set.
#[derive(Debug, Clone)]
pub struct ResourceSet {
    pub files: SetFiles,
    pub neutral: ResxFile,
    /// By culture name.
    pub cultures: Vec<(String, ResxFile)>,
}

impl ResourceSet {
    /// Load the set's files. A neutral file that does not exist is Visual Studio's empty file, not yet on disk.
    pub fn load(files: SetFiles) -> Result<ResourceSet, LoadError> {
        let read = |p: &Path| -> Result<ResxFile, LoadError> {
            let bytes = std::fs::read(p).map_err(|e| LoadError::Io(p.to_path_buf(), e))?;
            ResxFile::parse(p, &bytes).map_err(|e| LoadError::Parse(p.to_path_buf(), e))
        };
        let neutral = if files.neutral_exists {
            read(&files.neutral)?
        } else {
            ResxFile::empty(&files.neutral)
        };
        let mut cultures = Vec::with_capacity(files.cultures.len());
        for (c, p) in &files.cultures {
            cultures.push((c.clone(), read(p)?));
        }
        Ok(ResourceSet {
            files,
            neutral,
            cultures,
        })
    }

    /// The cultures, the neutral file first as the empty string.
    pub fn culture_names(&self) -> Vec<String> {
        std::iter::once(String::new())
            .chain(self.cultures.iter().map(|(c, _)| c.clone()))
            .collect()
    }

    /// The file of a culture (the empty string for the neutral file).
    pub fn file(&self, culture: &str) -> Option<&ResxFile> {
        if culture.is_empty() {
            return Some(&self.neutral);
        }
        self.cultures
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(culture))
            .map(|(_, f)| f)
    }

    pub fn file_mut(&mut self, culture: &str) -> Option<&mut ResxFile> {
        if culture.is_empty() {
            return Some(&mut self.neutral);
        }
        self.cultures
            .iter_mut()
            .find(|(c, _)| c.eq_ignore_ascii_case(culture))
            .map(|(_, f)| f)
    }

    /// Add a culture file to the set (an empty file with the neutral file's header, not yet on disk).
    pub fn add_culture(&mut self, culture: &str) -> Result<&ResxFile, ParseError> {
        let culture = canonical_culture(culture)
            .map(str::to_string)
            .unwrap_or_else(|| culture.to_string());
        if self.file(&culture).is_some() {
            return Ok(self.file(&culture).expect("just checked"));
        }
        let path = self.files.culture_path(&culture);
        let file = ResxFile::from_text(&path, self.neutral.header_only(), self.neutral.encoding())?;
        self.files.cultures.push((culture.clone(), path));
        self.files.cultures.sort_by(|a, b| a.0.cmp(&b.0));
        self.cultures.push((culture.clone(), file));
        self.cultures.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(self.file(&culture).expect("just added"))
    }

    pub fn is_invariant(&self, key: &str) -> bool {
        self.neutral
            .entry(key)
            .is_some_and(|e| has_invariant(e.comment.as_deref()))
    }

    /// The grid's rows: one per string key of the neutral file in its order, then keys only a culture file has, by
    /// key; the rule warnings under `rules` on every culture cell with a value.
    pub fn rows(&self, rules: &Rules) -> Vec<Row> {
        let mut rows = Vec::with_capacity(self.neutral.entries().len());
        let mut extra: BTreeMap<String, ()> = BTreeMap::new();
        for (_, f) in &self.cultures {
            for e in f.strings() {
                if self.neutral.entry(&e.name).is_none() {
                    extra.insert(e.name.clone(), ());
                }
            }
        }
        let keys = self
            .neutral
            .strings()
            .map(|e| e.name.clone())
            .chain(extra.into_keys());
        for key in keys {
            let neutral = self.neutral.entry(&key).filter(|e| e.is_string());
            let invariant = neutral.is_some_and(|e| has_invariant(e.comment.as_deref()));
            let mut cells = vec![Cell {
                culture: String::new(),
                value: neutral.map(|e| e.value.clone()),
                comment: neutral.and_then(|e| e.comment.clone()),
                line: neutral.map(|e| e.line),
                warnings: Vec::new(),
            }];
            for (culture, f) in &self.cultures {
                let entry = f.entry(&key).filter(|e| e.is_string());
                let warnings = match (neutral, entry) {
                    (Some(n), Some(t)) if !invariant => {
                        rules::check(rules, &key, culture, &n.value, &t.value)
                    }
                    _ => Vec::new(),
                };
                cells.push(Cell {
                    culture: culture.clone(),
                    value: entry.map(|e| e.value.clone()),
                    comment: entry.and_then(|e| e.comment.clone()),
                    line: entry.map(|e| e.line),
                    warnings,
                });
            }
            rows.push(Row {
                key,
                invariant,
                cells,
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::apply;

    #[test]
    fn culture_suffixes_are_recognized_by_the_table() {
        assert_eq!(
            base_name("Resources.resx"),
            Some(("Resources".into(), None))
        );
        assert_eq!(
            base_name("Resources.de.resx"),
            Some(("Resources".into(), Some("de")))
        );
        assert_eq!(
            base_name("Resources.de-de.resx"),
            Some(("Resources".into(), Some("de-DE")))
        );
        assert_eq!(
            base_name("Strings.sr-Latn-RS.resx"),
            Some(("Strings".into(), Some("sr-Latn-RS")))
        );
        assert_eq!(
            base_name("Default.aspx.resx"),
            Some(("Default.aspx".into(), None))
        );
        assert_eq!(
            base_name("Form1.Designer.resx"),
            Some(("Form1.Designer".into(), None))
        );
        assert_eq!(base_name(".de.resx"), Some((".de".into(), None)));
        assert_eq!(base_name("Program.cs"), None);
        assert!(is_culture("fr-FR") && !is_culture("aspx"));
    }

    #[test]
    fn invariant_marker_round_trips() {
        assert!(has_invariant(Some("{Invariant}")));
        assert!(has_invariant(Some("A brand {invariant} name")));
        assert!(!has_invariant(Some("plain")) && !has_invariant(None));
        assert_eq!(with_invariant(None, true).as_deref(), Some("{Invariant}"));
        assert_eq!(
            with_invariant(Some("Brand"), true).as_deref(),
            Some("{Invariant} Brand")
        );
        assert_eq!(
            with_invariant(Some("{Invariant} Brand"), false).as_deref(),
            Some("Brand")
        );
        assert_eq!(with_invariant(Some("{Invariant}"), false), None);
        assert_eq!(
            with_invariant(Some("Brand {Invariant}"), true).as_deref(),
            Some("{Invariant} Brand")
        );
    }

    fn resx(entries: &[(&str, &str, Option<&str>)]) -> String {
        let f = ResxFile::empty("x.resx");
        let mut text = f.text().to_string();
        for (k, v, c) in entries {
            let f = ResxFile::from_text("x.resx", text.clone(), f.encoding()).unwrap();
            text = apply(f.text(), &[f.add(k, v, *c).unwrap()]);
        }
        text
    }

    #[test]
    fn a_set_is_found_from_any_of_its_files_and_rows_carry_cells_and_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(
            d.join("Resources.resx"),
            resx(&[
                ("Hello", "Hello {0}", None),
                ("Brand", "Eludite", Some("{Invariant}")),
                ("Save", "Save?", None),
            ]),
        )
        .unwrap();
        std::fs::write(
            d.join("Resources.de.resx"),
            resx(&[("Hello", "Hallo", None), ("Extra", "x", None)]),
        )
        .unwrap();
        std::fs::write(
            d.join("Resources.fr-FR.resx"),
            resx(&[("Save", "Save?", None)]),
        )
        .unwrap();
        std::fs::write(d.join("Resources.aspx.resx"), resx(&[])).unwrap();
        std::fs::write(d.join("Other.resx"), resx(&[])).unwrap();
        let files = set_files(&d.join("Resources.de.resx")).unwrap();
        assert_eq!(files.neutral, d.join("Resources.resx"));
        assert!(files.neutral_exists);
        assert_eq!(
            files
                .cultures
                .iter()
                .map(|(c, _)| c.as_str())
                .collect::<Vec<_>>(),
            ["de", "fr-FR"]
        );
        assert_eq!(files.culture_path("es"), d.join("Resources.es.resx"));
        assert_eq!(files.culture_path("DE"), d.join("Resources.de.resx"));
        let set = ResourceSet::load(files).unwrap();
        assert_eq!(set.culture_names(), ["", "de", "fr-FR"]);
        let rows = set.rows(&Rules::default());
        assert_eq!(
            rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["Hello", "Brand", "Save", "Extra"]
        );
        let hello = &rows[0];
        assert_eq!(hello.cell("de").unwrap().value.as_deref(), Some("Hallo"));
        assert_eq!(
            hello.cell("de").unwrap().warnings[0].rule,
            crate::Rule::Placeholders
        );
        assert!(hello.cell("fr-FR").unwrap().missing());
        assert_eq!(hello.missing_count(), 1);
        assert_eq!(hello.warning_count(), 1);
        let brand = &rows[1];
        assert!(brand.invariant && brand.missing_count() == 0);
        let save = &rows[2];
        assert_eq!(
            save.cell("fr-FR").unwrap().warnings[0].rule,
            crate::Rule::Untranslated
        );
        let extra = &rows[3];
        assert!(extra.cell("").unwrap().missing());
        assert_eq!(extra.cell("de").unwrap().line, Some(123));
        assert_eq!(
            rows.iter()
                .map(|r| r.key.as_str())
                .collect::<Vec<_>>()
                .len(),
            4
        );

        let sets = discover(d, &[], false).unwrap();
        assert_eq!(
            sets.iter()
                .map(|s| s.base_name.as_str())
                .collect::<Vec<_>>(),
            ["Other", "Resources", "Resources.aspx"]
        );
        let sets = discover(d, &["Other.resx".into()], false).unwrap();
        assert_eq!(sets.len(), 2);
    }

    #[test]
    fn a_missing_neutral_file_is_the_empty_template_and_cultures_can_be_added() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("Only.de.resx"), resx(&[("K", "v", None)])).unwrap();
        let files = set_files(&d.join("Only.de.resx")).unwrap();
        assert!(!files.neutral_exists);
        let mut set = ResourceSet::load(files).unwrap();
        assert!(set.neutral.entries().is_empty());
        let rows = set.rows(&Rules::default());
        assert_eq!(rows[0].key, "K");
        assert!(rows[0].cell("").unwrap().missing());
        let f = set.add_culture("fr-fr").unwrap();
        assert_eq!(f.path(), d.join("Only.fr-FR.resx"));
        assert!(f.entries().is_empty());
        assert_eq!(set.culture_names(), ["", "de", "fr-FR"]);
        assert!(!set.is_invariant("K"));
    }
}
