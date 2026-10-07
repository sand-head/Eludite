//! The corpus round trip (proposal 0005): every file under `corpus/resx/` parses, survives every kind of splice byte
//! for byte outside the changed element, and gives the sets, rows and warnings `corpus/resx/README.md` lists.

use std::path::{Path, PathBuf};

use eludite_resx::{Encoding, ResourceSet, ResxFile, Rule, Rules, apply, discover, set_files};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/resx")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> (PathBuf, Vec<u8>) {
    let p = corpus().join(rel);
    let bytes = std::fs::read(&p).unwrap();
    (p, bytes)
}

#[test]
fn every_file_parses_and_writes_back_its_own_bytes() {
    let mut count = 0;
    for set in discover(&corpus(), &[], false).unwrap() {
        let files =
            std::iter::once(set.neutral.clone()).chain(set.cultures.iter().map(|(_, p)| p.clone()));
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            let file = ResxFile::parse(&path, &bytes)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(file.to_bytes(), bytes, "{} round-trips", path.display());
            count += 1;
        }
    }
    assert_eq!(count, 9, "every .resx of the corpus");
}

#[test]
fn the_strings_set_has_the_listed_cultures_rows_and_warnings() {
    let files = set_files(&corpus().join("Strings/Properties/Resources.de.resx")).unwrap();
    assert_eq!(files.base_name, "Resources");
    assert!(files.neutral_exists);
    assert_eq!(
        files
            .cultures
            .iter()
            .map(|(c, _)| c.as_str())
            .collect::<Vec<_>>(),
        ["de", "fr-FR"]
    );
    let set = ResourceSet::load(files).unwrap();
    assert_eq!(set.neutral.encoding(), Encoding::Utf8 { bom: true });
    assert_eq!(set.neutral.newline(), "\r\n");
    assert_eq!(
        set.file("fr-FR").unwrap().encoding(),
        Encoding::Utf8 { bom: false }
    );
    assert_eq!(set.file("fr-FR").unwrap().newline(), "\n");
    assert_eq!(set.neutral.entries().len(), 9);
    assert_eq!(set.neutral.strings().count(), 8);
    assert!(!set.neutral.entry("Icon").unwrap().is_string());
    let rows = set.rows(&Rules::default());
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "Hello",
            "Brand",
            "Save",
            "Multi",
            "Ampersand",
            "1Number",
            "with space",
            "class",
            "Orphan"
        ]
    );
    let hello = &rows[0];
    assert_eq!(
        hello.cell("").unwrap().comment.as_deref(),
        Some("The greeting; {0} is the user name")
    );
    assert_eq!(
        hello.cell("de").unwrap().value.as_deref(),
        Some("Hallo, {0}!")
    );
    assert!(hello.cell("de").unwrap().warnings.is_empty());
    assert_eq!(
        hello
            .cell("fr-FR")
            .unwrap()
            .warnings
            .iter()
            .map(|w| w.rule)
            .collect::<Vec<_>>(),
        [Rule::Placeholders, Rule::Punctuation]
    );
    let brand = &rows[1];
    assert!(brand.invariant);
    assert_eq!(brand.missing_count(), 0);
    assert!(
        brand.cell("fr-FR").unwrap().warnings.is_empty(),
        "invariant keys have no warnings"
    );
    let save = &rows[2];
    assert_eq!(
        save.cell("de")
            .unwrap()
            .warnings
            .iter()
            .map(|w| w.rule)
            .collect::<Vec<_>>(),
        [Rule::Punctuation]
    );
    assert_eq!(
        save.cell("fr-FR")
            .unwrap()
            .warnings
            .iter()
            .map(|w| w.rule)
            .collect::<Vec<_>>(),
        [Rule::Untranslated]
    );
    let multi = &rows[3];
    assert_eq!(
        multi.cell("").unwrap().value.as_deref(),
        Some("Line one\nLine two")
    );
    assert_eq!(
        rows[4].cell("").unwrap().value.as_deref(),
        Some("Fish & Chips <b>")
    );
    assert_eq!(rows[4].missing_count(), 2);
    let orphan = &rows[8];
    assert!(orphan.cell("").unwrap().missing());
    assert_eq!(
        orphan.cell("de").unwrap().value.as_deref(),
        Some("Nur auf Deutsch")
    );
    assert_eq!(hello.cell("").unwrap().line, Some(120));
}

#[test]
fn the_corpus_lists_eight_sets_and_the_aspx_rule_holds() {
    let sets = discover(&corpus(), &[], false).unwrap();
    let names: Vec<String> = sets
        .iter()
        .map(|s| {
            format!(
                "{}:{}",
                s.neutral
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap(),
                s.base_name
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            "Legacy:Default.aspx",
            "Legacy:Form1",
            "Properties:Resources",
            "Public:Messages",
            "Properties:Resources"
        ]
    );
    let form = ResourceSet::load(sets[1].clone()).unwrap();
    assert_eq!(
        form.neutral.strings().count(),
        2,
        ">>$this.Name and >>$this.Type are untyped strings"
    );
    assert_eq!(form.neutral.entries().len(), 3);
    let sets = discover(&corpus(), &["Legacy/**".into()], false).unwrap();
    assert_eq!(sets.len(), 2);
}

#[test]
fn every_splice_keeps_every_other_byte() {
    let (path, bytes) = read("Strings/Properties/Resources.resx");
    let file = ResxFile::parse(&path, &bytes).unwrap();
    let text = file.text();
    let check = |splices: Vec<eludite_resx::Splice>, expect_changed: &str| {
        let new = apply(text, &splices);
        let mut edited = ResxFile::from_text(&path, new.clone(), file.encoding()).unwrap();
        assert!(edited.text().contains(expect_changed), "{expect_changed}");
        // Undo through the model and get the original bytes back.
        let undo: Vec<_> = splices
            .iter()
            .map(|s| eludite_resx::Splice {
                range: s.range.start..s.range.start + s.text.len(),
                text: text[s.range.clone()].to_string(),
            })
            .collect();
        assert_eq!(splices.len(), 1, "one splice per check");
        edited.apply(&undo).unwrap();
        assert_eq!(edited.to_bytes(), bytes);
    };
    check(
        vec![file.set_value("Save", "Store?").unwrap()],
        "<value>Store?</value>",
    );
    check(
        vec![file.set_comment("Hello", Some("Changed")).unwrap()],
        "<comment>Changed</comment>",
    );
    check(
        vec![file.rename("Save", "Keep").unwrap()],
        "<data name=\"Keep\" xml:space=\"preserve\">",
    );
    check(
        vec![file.add("Added", "A", Some("c")).unwrap()],
        "  <data name=\"Added\" xml:space=\"preserve\">\r\n    <value>A</value>\r\n    <comment>c</comment>\r\n  </data>\r\n</root>",
    );
    check(
        vec![file.remove("Multi").unwrap()],
        "</data>\r\n  <data name=\"Ampersand\"",
    );

    // The LF file without a byte order mark keeps both.
    let (path, bytes) = read("Strings/Properties/Resources.fr-FR.resx");
    let file = ResxFile::parse(&path, &bytes).unwrap();
    let mut edited = file.clone();
    edited
        .apply(&[file.add("Save2", "x", None).unwrap()])
        .unwrap();
    assert!(edited.text().ends_with(
        "  <data name=\"Save2\" xml:space=\"preserve\">\n    <value>x</value>\n  </data>\n</root>\n"
    ));
    assert!(!edited.to_bytes().starts_with(&[0xEF, 0xBB, 0xBF]));
    edited.apply(&[edited.remove("Save2").unwrap()]).unwrap();
    assert_eq!(edited.to_bytes(), bytes);
}

#[test]
fn a_new_culture_file_takes_the_neutral_header() {
    let files = set_files(&corpus().join("Public/Messages.resx")).unwrap();
    let mut set = ResourceSet::load(files).unwrap();
    let it = set.add_culture("de").unwrap();
    assert_eq!(it.path(), corpus().join("Public/Messages.de.resx"));
    assert!(it.entries().is_empty());
    assert!(it.text().contains("<resheader name=\"writer\">"));
    assert!(it.text().ends_with("  </resheader>\r\n</root>"));
    assert_eq!(set.culture_names(), ["", "de", "es"]);
}
