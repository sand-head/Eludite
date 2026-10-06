//! A resource set as the editor and the commands hold it (proposal 0005): the files parsed into `eludite-resx`'s
//! model, the text each file had on disk when it was read or written, which cultures are dirty, and the edits as
//! operations on keys and cells that mark what they touch. The grid's rows come from here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eludite_commands::resx::{AddStatus, RenameStatus, WriteStatus};
use eludite_resx::{
    ResourceSet, Row, Rules, canonical_culture, has_invariant, set_files, with_invariant,
};

/// A file to write: its culture, path, text, and whether it is new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub culture: String,
    pub path: PathBuf,
    pub text: String,
    pub created: bool,
    /// The text the file had on disk, for the whole-file edit's range (`None` for a new file).
    pub previous: Option<String>,
}

/// The keys removed with the number of files each was in, and the keys that were in no file.
pub type Removed = (Vec<(String, u32)>, Vec<String>);

pub struct SetModel {
    pub set: ResourceSet,
    /// The text of each culture's file as last read or written; absent for a file not on disk yet.
    disk: BTreeMap<String, String>,
    dirty: BTreeSet<String>,
    /// Keys were added, removed or renamed in the neutral file since the last save: the designer regenerates.
    pub keys_changed: bool,
    rules: Rules,
    rows: Vec<Row>,
}

impl SetModel {
    /// Load the set `path` belongs to from disk.
    pub fn load(path: &Path, rules: Rules) -> Result<SetModel, String> {
        let files = set_files(path).map_err(|e| e.to_string())?;
        let set = ResourceSet::load(files).map_err(|e| e.to_string())?;
        let mut disk = BTreeMap::new();
        if set.files.neutral_exists {
            disk.insert(String::new(), set.neutral.text().to_owned());
        }
        for (c, f) in &set.cultures {
            disk.insert(c.clone(), f.text().to_owned());
        }
        let mut m = SetModel {
            set,
            disk,
            dirty: BTreeSet::new(),
            keys_changed: false,
            rules,
            rows: Vec::new(),
        };
        m.recompute();
        Ok(m)
    }

    fn recompute(&mut self) {
        self.rows = self.set.rows(&self.rules);
    }

    pub fn neutral_path(&self) -> &Path {
        &self.set.files.neutral
    }

    pub fn base_name(&self) -> &str {
        &self.set.files.base_name
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    pub fn set_rules(&mut self, rules: Rules) {
        if self.rules != rules {
            self.rules = rules;
            self.recompute();
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, key: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.key == key)
    }

    /// The cultures, the neutral file first as the empty string.
    pub fn cultures(&self) -> Vec<String> {
        self.set.culture_names()
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dirty_cultures(&self) -> Vec<String> {
        self.dirty.iter().cloned().collect()
    }

    fn mark(&mut self, culture: &str) {
        self.dirty.insert(culture.to_owned());
        self.recompute();
    }

    fn culture_name(&self, culture: &str) -> Result<String, String> {
        if culture.is_empty() {
            return Ok(String::new());
        }
        if let Some((c, _)) = self
            .set
            .cultures
            .iter()
            .find(|(c, _)| c.eq_ignore_ascii_case(culture))
        {
            return Ok(c.clone());
        }
        match canonical_culture(culture) {
            Some(c) => Ok(c.to_owned()),
            None => Err(format!("`{culture}` is not a culture name")),
        }
    }

    /// Write a cell's value: `None` removes the culture's entry. A culture file that does not exist is created
    /// (with the neutral file's header) only with `create`.
    pub fn set_value(
        &mut self,
        key: &str,
        culture: &str,
        value: Option<&str>,
        create: bool,
    ) -> Result<WriteStatus, String> {
        let culture = self.culture_name(culture)?;
        let mut created_file = false;
        if self.set.file(&culture).is_none() {
            if !create {
                return Err(format!(
                    "{} has no {culture} file; pass create_culture to create it",
                    self.base_name()
                ));
            }
            self.set.add_culture(&culture).map_err(|e| e.to_string())?;
            created_file = true;
        }
        let neutral = culture.is_empty();
        let file = self.set.file_mut(&culture).expect("the file exists now");
        let status = match (file.entry(key).filter(|e| e.is_string()), value) {
            (Some(_), None) if neutral => {
                return Err(
                    "a neutral value cannot be removed here; use eludite.resx.remove".into(),
                );
            }
            (Some(_), None) => {
                let s = file.remove(key).expect("the entry exists");
                file.apply(&[s]).map_err(|e| e.to_string())?;
                WriteStatus::Written
            }
            (None, None) => return Ok(WriteStatus::Unchanged),
            (Some(e), Some(v)) if e.value == v => {
                if created_file {
                    WriteStatus::Created
                } else {
                    return Ok(WriteStatus::Unchanged);
                }
            }
            (Some(_), Some(v)) => {
                let s = file.set_value(key, v).expect("a string entry");
                file.apply(&[s]).map_err(|e| e.to_string())?;
                WriteStatus::Written
            }
            (None, Some(v)) => {
                if file.entry(key).is_some() {
                    return Err(format!("{key} is not a string resource"));
                }
                let s = file.add(key, v, None).expect("the key is new");
                file.apply(&[s]).map_err(|e| e.to_string())?;
                if neutral {
                    self.keys_changed = true;
                }
                WriteStatus::Created
            }
        };
        self.mark(&culture);
        Ok(status)
    }

    /// Write a cell's comment (`None` or empty removes it). The neutral entry keeps its invariant marker.
    pub fn set_comment(
        &mut self,
        key: &str,
        culture: &str,
        comment: Option<&str>,
    ) -> Result<WriteStatus, String> {
        let culture = self.culture_name(culture)?;
        let Some(file) = self.set.file_mut(&culture) else {
            return Err(format!("no {culture} file"));
        };
        let Some(entry) = file.entry(key).filter(|e| e.is_string()) else {
            return Err(format!(
                "{key} has no entry in {}",
                if culture.is_empty() {
                    "the neutral file"
                } else {
                    &culture
                }
            ));
        };
        let comment = comment.map(str::trim).filter(|c| !c.is_empty());
        let text = if culture.is_empty() {
            let invariant = has_invariant(entry.comment.as_deref());
            with_invariant(comment, invariant)
        } else {
            comment.map(str::to_owned)
        };
        if entry.comment == text {
            return Ok(WriteStatus::Unchanged);
        }
        match file.set_comment(key, text.as_deref()) {
            Some(s) => file.apply(&[s]).map_err(|e| e.to_string())?,
            None => return Ok(WriteStatus::Unchanged),
        }
        self.mark(&culture);
        Ok(WriteStatus::Written)
    }

    pub fn set_invariant(&mut self, key: &str, invariant: bool) -> Result<WriteStatus, String> {
        let Some(entry) = self.set.neutral.entry(key).filter(|e| e.is_string()) else {
            return Err(format!("{key} has no entry in the neutral file"));
        };
        if has_invariant(entry.comment.as_deref()) == invariant {
            return Ok(WriteStatus::Unchanged);
        }
        let text = with_invariant(entry.comment.as_deref(), invariant);
        match self.set.neutral.set_comment(key, text.as_deref()) {
            Some(s) => self.set.neutral.apply(&[s]).map_err(|e| e.to_string())?,
            None => return Ok(WriteStatus::Unchanged),
        }
        self.mark("");
        Ok(WriteStatus::Written)
    }

    /// Add a key to the neutral file.
    pub fn add_key(
        &mut self,
        key: &str,
        value: &str,
        comment: Option<&str>,
        invariant: bool,
    ) -> Result<(AddStatus, Option<u32>), String> {
        if self.set.neutral.entry(key).is_some() {
            return Ok((
                AddStatus::Exists,
                self.set.neutral.entry(key).map(|e| e.line),
            ));
        }
        let comment = with_invariant(comment, invariant);
        let s = self
            .set
            .neutral
            .add(key, value, comment.as_deref())
            .expect("the key is new");
        self.set.neutral.apply(&[s]).map_err(|e| e.to_string())?;
        self.keys_changed = true;
        self.mark("");
        Ok((
            AddStatus::Added,
            self.set.neutral.entry(key).map(|e| e.line),
        ))
    }

    /// Remove keys from every file. Answers (key, files it was in) for the removed ones, and the keys in no file.
    pub fn remove_keys(&mut self, keys: &[String]) -> Result<Removed, String> {
        let mut removed = Vec::new();
        let mut missing = Vec::new();
        for key in keys {
            let mut files = 0;
            let cultures = self.cultures();
            for culture in cultures {
                let file = self.set.file_mut(&culture).expect("listed");
                if let Some(s) = file.remove(key) {
                    file.apply(&[s]).map_err(|e| e.to_string())?;
                    files += 1;
                    if culture.is_empty() {
                        self.keys_changed = true;
                    }
                    self.dirty.insert(culture);
                }
            }
            if files == 0 {
                missing.push(key.clone());
            } else {
                removed.push((key.clone(), files));
            }
        }
        self.recompute();
        Ok((removed, missing))
    }

    /// Rename a key in every file. Answers the status and the files it was renamed in.
    pub fn rename_key(&mut self, key: &str, new_key: &str) -> Result<(RenameStatus, u32), String> {
        let cultures = self.cultures();
        if cultures
            .iter()
            .any(|c| self.set.file(c).is_some_and(|f| f.entry(new_key).is_some()))
        {
            return Ok((RenameStatus::Exists, 0));
        }
        let mut files = 0;
        for culture in cultures {
            let file = self.set.file_mut(&culture).expect("listed");
            if let Some(s) = file.rename(key, new_key) {
                file.apply(&[s]).map_err(|e| e.to_string())?;
                files += 1;
                if culture.is_empty() {
                    self.keys_changed = true;
                }
                self.dirty.insert(culture);
            }
        }
        self.recompute();
        Ok((
            if files == 0 {
                RenameStatus::Missing
            } else {
                RenameStatus::Renamed
            },
            files,
        ))
    }

    /// The dirty files' texts to write (sorted by key with `sort_on_save`).
    pub fn writes(&self, sort_on_save: bool) -> Vec<FileWrite> {
        self.dirty
            .iter()
            .filter_map(|culture| {
                let file = self.set.file(culture)?;
                let text = if sort_on_save {
                    file.sorted_text().unwrap_or_else(|| file.text().to_owned())
                } else {
                    file.text().to_owned()
                };
                Some(FileWrite {
                    culture: culture.clone(),
                    path: file.path().to_path_buf(),
                    text,
                    created: !self.disk.contains_key(culture),
                    previous: self.disk.get(culture).cloned(),
                })
            })
            .collect()
    }

    /// The files were written: their texts are what is on disk now.
    pub fn saved(&mut self, writes: &[FileWrite]) {
        for w in writes {
            self.disk.insert(w.culture.clone(), w.text.clone());
            self.dirty.remove(&w.culture);
            if let Some(f) = self.set.file_mut(&w.culture)
                && f.text() != w.text
            {
                let _ = f.set_text(w.text.clone());
            }
        }
        if writes.iter().any(|w| w.culture.is_empty()) || self.dirty.is_empty() {
            self.keys_changed = false;
        }
        self.recompute();
    }

    /// The non-string entries of the neutral file (Other Resources): name and type.
    pub fn others(&self) -> Vec<(String, String)> {
        self.set
            .neutral
            .entries()
            .iter()
            .filter(|e| !e.is_string())
            .map(|e| {
                let kind = match &e.kind {
                    eludite_resx::EntryKind::Other {
                        type_name,
                        mimetype,
                    } => type_name
                        .as_deref()
                        .map(|t| t.split(',').next().unwrap_or(t).trim().to_owned())
                        .or_else(|| mimetype.clone())
                        .unwrap_or_else(|| "binary".into()),
                    eludite_resx::EntryKind::String => "string".into(),
                };
                (e.name.clone(), kind)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_resx::{ResxFile, apply};

    fn file(dir: &Path, name: &str, entries: &[(&str, &str, Option<&str>)]) -> PathBuf {
        let f = ResxFile::empty("x.resx");
        let mut text = f.text().to_owned();
        for (k, v, c) in entries {
            let f = ResxFile::from_text("x.resx", text.clone(), f.encoding()).unwrap();
            text = apply(f.text(), &[f.add(k, v, *c).unwrap()]);
        }
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn edits_mark_their_files_and_writes_carry_the_texts() {
        let dir = tempfile::tempdir().unwrap();
        let neutral = file(
            dir.path(),
            "R.resx",
            &[("A", "a", None), ("B", "b", Some("c"))],
        );
        file(dir.path(), "R.de.resx", &[("A", "ä", None)]);
        let mut m = SetModel::load(&neutral, Rules::default()).unwrap();
        assert_eq!(m.cultures(), ["", "de"]);
        assert!(!m.is_dirty());
        assert_eq!(
            m.set_value("A", "de", Some("ä"), false).unwrap(),
            WriteStatus::Unchanged
        );
        assert_eq!(
            m.set_value("A", "de", Some("A!"), false).unwrap(),
            WriteStatus::Written
        );
        assert_eq!(
            m.set_value("B", "de", Some("B"), false).unwrap(),
            WriteStatus::Created
        );
        assert!(m.set_value("B", "fr", Some("x"), false).is_err());
        assert_eq!(
            m.set_value("B", "fr", Some("x"), true).unwrap(),
            WriteStatus::Created
        );
        assert!(m.set_value("B", "nope-XX", Some("x"), true).is_err());
        assert!(m.set_value("A", "", None, false).is_err());
        assert_eq!(
            m.set_value("A", "de", None, false).unwrap(),
            WriteStatus::Written
        );
        assert!(m.row("A").unwrap().cell("de").unwrap().missing());
        assert_eq!(m.dirty_cultures(), ["de", "fr"]);
        assert!(!m.keys_changed);
        assert_eq!(
            m.set_comment("A", "", Some("note")).unwrap(),
            WriteStatus::Written
        );
        assert_eq!(m.set_invariant("A", true).unwrap(), WriteStatus::Written);
        assert_eq!(
            m.set.neutral.entry("A").unwrap().comment.as_deref(),
            Some("{Invariant} note")
        );
        assert_eq!(
            m.set_comment("A", "", Some("other")).unwrap(),
            WriteStatus::Written
        );
        assert_eq!(
            m.set.neutral.entry("A").unwrap().comment.as_deref(),
            Some("{Invariant} other")
        );
        assert!(m.row("A").unwrap().invariant);
        assert_eq!(
            m.add_key("C", "c", None, false).unwrap().0,
            AddStatus::Added
        );
        assert_eq!(
            m.add_key("C", "c", None, false).unwrap().0,
            AddStatus::Exists
        );
        assert!(m.keys_changed);
        assert_eq!(m.rename_key("C", "A").unwrap().0, RenameStatus::Exists);
        assert_eq!(m.rename_key("Zed", "Z").unwrap().0, RenameStatus::Missing);
        assert_eq!(m.rename_key("B", "B2").unwrap(), (RenameStatus::Renamed, 3));
        let (removed, missing) = m.remove_keys(&["B2".into(), "Q".into()]).unwrap();
        assert_eq!(removed, [("B2".to_string(), 3)]);
        assert_eq!(missing, ["Q"]);
        let writes = m.writes(false);
        assert_eq!(
            writes
                .iter()
                .map(|w| w.culture.as_str())
                .collect::<Vec<_>>(),
            ["", "de", "fr"]
        );
        assert!(writes[2].created && writes[2].previous.is_none());
        assert!(writes[0].previous.is_some());
        assert_eq!(writes[2].path, dir.path().join("R.fr.resx"));
        m.saved(&writes);
        assert!(!m.is_dirty() && !m.keys_changed);
        let sorted = m.writes(true);
        assert!(sorted.is_empty());
        assert_eq!(m.others().len(), 0);
    }
}
