//! Code coverage (brief 0076; Visual Studio's Test > Analyze Code Coverage): a coverage run is a test run with
//! `coverage` (see `test_runs`); `eludite-host` collects Cobertura from each test project's collector and sends one
//! `coverage` update ([`eludite_lsp::host::TestCoverage`]) before the run's end. This module keeps the last coverage,
//! answers `eludite.test.coverage` and `eludite.test.coverage_results`, feeds the Code Coverage Results window
//! ([`super::coverage_window`]) and colors the editor's lines.
//!
//! - **Coloring.** Each open document of a covered file gets the decoration layer [`COVERAGE_LAYER`]: a line
//!   background per coverable line, covered, partially covered (some branches not taken) or not covered, in the theme's
//!   `coverage_*` colors. The decorations are computed on a background thread from the document's snapshot, then set on
//!   the UI thread (one decoration set per document). Show Code Coverage Coloring (`eludite.test.coverage_coloring`,
//!   test.showCoverageColoring) turns it off and on.
//! - **Stale.** A document edited after the coverage ran keeps its coloring (anchored, it follows the edits) in muted
//!   colors, and `eludite.test.coverage_results` says `stale`, until the next coverage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eludite_commands::CommandError;
use eludite_commands::build::OutputSource;
use eludite_commands::test::{
    ColoringOutput, CoverageFileLines, CoverageModuleSummary, CoverageOutput,
    CoverageResultsOutput, CoverageRow, CoverageSummary, TestOutput, TestRequest, percent,
};
use eludite_commands::view::{ViewRequest, ViewTarget as _};
use eludite_docking::ids;
use eludite_editor::text;
use eludite_editor::{Decoration, DecorationStyle, EditorView};
use eludite_lsp::host::TestCoverage;
use gpui::{Context, Entity, Rgba, Subscription, Window};

use super::Shell;
use super::documents::normalize_path;

/// The editor decoration layer of the coverage coloring.
pub const COVERAGE_LAYER: &str = "coverage";

/// A coverable line's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineState {
    Covered,
    Partial,
    Uncovered,
}

/// The last coverage and what it colors.
#[derive(Default)]
pub struct CoverageState {
    /// The run it came from.
    pub run: Option<u64>,
    pub data: Option<Arc<TestCoverage>>,
    /// Per file (normalized), its coverable lines' states in line order.
    pub lines: Arc<HashMap<PathBuf, Vec<(u32, LineState)>>>,
    /// The summary per coverage run (for `eludite.test.coverage`'s answer).
    pub summaries: HashMap<u64, CoverageSummary>,
    /// The Code Coverage Results window's rows, in tree order.
    pub rows: Vec<CoverageRow>,
    /// Documents edited since the coverage ran (by document id).
    pub stale: HashSet<String>,
    /// The colored documents, with the buffer version their coloring was computed on.
    colored: HashMap<String, clock::Global>,
    watching: HashMap<String, Subscription>,
    /// Bumped by every coloring pass: a background result of an older pass is dropped.
    pass: u64,
}

impl CoverageState {
    pub fn summary(&self) -> Option<&CoverageSummary> {
        self.run.and_then(|r| self.summaries.get(&r))
    }
}

/// The states of every coverable line of `coverage`, per normalized path.
pub fn line_states(coverage: &TestCoverage) -> HashMap<PathBuf, Vec<(u32, LineState)>> {
    let mut out: HashMap<PathBuf, Vec<(u32, LineState)>> = HashMap::new();
    for f in &coverage.files {
        let mut lines: Vec<(u32, LineState)> = f
            .lines
            .iter()
            .filter_map(|l| {
                let (line, hits) = (*l.first()?, *l.get(1)?);
                let state = match (hits, l.get(2), l.get(3)) {
                    (0, _, _) => LineState::Uncovered,
                    (_, Some(c), Some(v)) if c < v => LineState::Partial,
                    _ => LineState::Covered,
                };
                Some((line, state))
            })
            .collect();
        lines.sort_by_key(|(l, _)| *l);
        out.entry(normalize_path(Path::new(&f.path)))
            .or_default()
            .extend(lines);
    }
    out
}

/// The summary `eludite.test.coverage` answers.
pub fn summary_of(coverage: &TestCoverage) -> CoverageSummary {
    CoverageSummary {
        lines_covered: coverage.lines_covered,
        lines_valid: coverage.lines_valid,
        percent: percent(coverage.lines_covered, coverage.lines_valid),
        branches_covered: coverage.branches_covered,
        branches_valid: coverage.branches_valid,
        modules: coverage
            .modules
            .iter()
            .map(|m| CoverageModuleSummary {
                name: m.name.clone(),
                lines_covered: m.lines_covered,
                lines_valid: m.lines_valid,
                percent: percent(m.lines_covered, m.lines_valid),
            })
            .collect(),
        missing: coverage
            .containers
            .iter()
            .filter_map(|c| {
                let name = c
                    .container
                    .split('|')
                    .next()
                    .and_then(|p| Path::new(p).file_stem())
                    .map_or(c.container.clone(), |s| s.to_string_lossy().into_owned());
                c.message.as_ref().map(|m| format!("{name}: {m}"))
            })
            .collect(),
    }
}

/// The Code Coverage Results rows: assembly > namespace > class > method.
pub fn rows_of(coverage: &TestCoverage) -> Vec<CoverageRow> {
    let row = |kind: &str, name: &str, depth: u32, covered: u32, valid: u32| CoverageRow {
        kind: kind.into(),
        name: name.into(),
        depth,
        file: None,
        line: None,
        covered,
        not_covered: valid.saturating_sub(covered),
        percent: percent(covered, valid),
    };
    let mut rows = Vec::new();
    let mut modules: Vec<_> = coverage.modules.iter().collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    for m in modules {
        rows.push(row("assembly", &m.name, 0, m.lines_covered, m.lines_valid));
        let mut namespaces: BTreeMap<String, Vec<&eludite_lsp::host::CoverageClass>> =
            BTreeMap::new();
        for c in &m.classes {
            let ns = c.namespace.clone().unwrap_or_else(|| {
                c.name
                    .rsplit_once('.')
                    .map_or(String::new(), |(n, _)| n.to_owned())
            });
            namespaces.entry(ns).or_default().push(c);
        }
        for (ns, mut classes) in namespaces {
            let covered = classes.iter().map(|c| c.lines_covered).sum();
            let valid = classes.iter().map(|c| c.lines_valid).sum();
            let label = if ns.is_empty() {
                "(global)"
            } else {
                ns.as_str()
            };
            rows.push(row("namespace", label, 1, covered, valid));
            classes.sort_by(|a, b| a.name.cmp(&b.name));
            for c in classes {
                let short = c.name.strip_prefix(&format!("{ns}.")).unwrap_or(&c.name);
                let mut r = row("class", short, 2, c.lines_covered, c.lines_valid);
                r.file = c.file.clone();
                r.line = c.methods.iter().filter_map(|m| m.line).min();
                rows.push(r);
                for mt in &c.methods {
                    let mut r = row(
                        "method",
                        &format!("{}{}", mt.name, mt.signature.as_deref().unwrap_or("")),
                        3,
                        mt.lines_covered,
                        mt.lines_valid,
                    );
                    r.file = c.file.clone();
                    r.line = mt.line;
                    rows.push(r);
                }
            }
        }
    }
    rows
}

/// The theme's coverage colors: covered, partially covered, not covered; muted for a stale document.
fn colors(theme: &eludite_ui::Theme, stale: bool) -> [Rgba; 3] {
    let mut c = [
        theme.coverage_covered,
        theme.coverage_partial,
        theme.coverage_uncovered,
    ];
    if stale {
        for x in &mut c {
            x.a *= 0.4;
        }
    }
    c
}

/// The decorations of `lines` in `snapshot` (runs on a background thread).
pub fn decorations(
    snapshot: &text::BufferSnapshot,
    lines: &[(u32, LineState)],
    colors: [Rgba; 3],
) -> Vec<Decoration> {
    let max = snapshot.max_point().row;
    lines
        .iter()
        .filter(|(l, _)| *l >= 1 && l - 1 <= max)
        .map(|(l, state)| {
            let row = l - 1;
            let start = snapshot.point_to_offset(text::Point::new(row, 0));
            let end = snapshot.point_to_offset(text::Point::new(row, snapshot.line_len(row)));
            let color = match state {
                LineState::Covered => colors[0],
                LineState::Partial => colors[1],
                LineState::Uncovered => colors[2],
            };
            Decoration {
                range: snapshot.anchor_before(start)..snapshot.anchor_after(end),
                style: DecorationStyle::Background(color),
            }
        })
        .collect()
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn coverage(&self) -> &CoverageState {
        &self.coverage
    }

    /// The host's `coverage` update of `run`.
    pub(super) fn coverage_received(
        &mut self,
        run: u64,
        coverage: TestCoverage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let summary = summary_of(&coverage);
        self.output.update(cx, |o, cx| {
            o.append(
                OutputSource::Tests,
                &format!(
                    "Code coverage: {:.1}% of {} lines covered ({} covered){}\n",
                    summary.percent,
                    summary.lines_valid,
                    summary.lines_covered,
                    if summary.missing.is_empty() {
                        String::new()
                    } else {
                        format!("; no coverage from {}", summary.missing.join("; "))
                    }
                ),
                cx,
            )
        });
        self.coverage.rows = rows_of(&coverage);
        self.coverage.lines = Arc::new(line_states(&coverage));
        self.coverage.data = Some(Arc::new(coverage));
        self.coverage.summaries.insert(run, summary);
        self.coverage.run = Some(run);
        self.coverage.stale.clear();
        self.coverage.colored.clear();
        self.refresh_coverage_window(cx);
        // Visual Studio shows the results window when a coverage run ends.
        let _ = self.controller.apply(ViewRequest::Show {
            id: ids::CODE_COVERAGE_RESULTS.into(),
        });
        let _ = window;
        self.apply_coverage_coloring(cx);
    }

    pub(super) fn refresh_coverage_window(&mut self, cx: &mut Context<Self>) {
        let rows = self.coverage.rows.clone();
        let summary = self.coverage.summary().cloned();
        let show = self.tests.show_coverage;
        let stale = !self.coverage.stale.is_empty();
        self.coverage_window
            .update(cx, |w, cx| w.set_data(rows, summary, show, stale, cx));
    }

    /// `eludite.test.coverage`'s answer for `run`.
    pub(super) fn coverage_output(&self, run: u64, timed_out: bool) -> CoverageOutput {
        let mut out = CoverageOutput {
            run: self.run_output(run, timed_out),
            coverage: self.coverage.summaries.get(&run).cloned(),
        };
        if out.coverage.is_none()
            && self.tests.run(run).is_some_and(|r| r.state.done())
            && out.run.message.is_none()
        {
            out.run.message = Some("the run ended without coverage from eludite-host".into());
        }
        out
    }

    /// `eludite.test.coverage_results`.
    pub(super) fn coverage_results(
        &mut self,
        request: &TestRequest,
    ) -> Result<TestOutput, CommandError> {
        let TestRequest::CoverageResults {
            file,
            filter,
            max_items,
            cursor,
        } = request
        else {
            unreachable!("a coverage results request")
        };
        let (Some(run), Some(summary)) = (self.coverage.run, self.coverage.summary().cloned())
        else {
            return Err(CommandError::Failed(
                "no code coverage has been collected yet (eludite.test.coverage)".into(),
            ));
        };
        let needle = filter.as_deref().map(str::to_lowercase);
        let rows: Vec<CoverageRow> = self
            .coverage
            .rows
            .iter()
            .filter(|r| {
                needle
                    .as_deref()
                    .is_none_or(|n| r.name.to_lowercase().contains(n))
            })
            .cloned()
            .collect();
        let start: usize = cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
        let total = rows.len();
        let page: Vec<CoverageRow> = rows.into_iter().skip(start).take(*max_items).collect();
        let next = start + page.len();
        let file = match file {
            Some(f) => {
                let p = Path::new(f);
                let abs = match self.workspace_root() {
                    Some(root) if p.is_relative() => root.join(p),
                    _ => p.to_path_buf(),
                };
                let key = normalize_path(&abs);
                let lines = self.coverage.lines.get(&key).ok_or_else(|| {
                    CommandError::InvalidInput(format!(
                        "{} has no coverable line in the last coverage",
                        abs.display()
                    ))
                })?;
                let of = |s: LineState| {
                    lines
                        .iter()
                        .filter(|(_, x)| *x == s)
                        .map(|(l, _)| *l)
                        .collect::<Vec<_>>()
                };
                Some(CoverageFileLines {
                    path: abs.to_string_lossy().into_owned(),
                    covered: of(LineState::Covered),
                    partial: of(LineState::Partial),
                    uncovered: of(LineState::Uncovered),
                })
            }
            None => None,
        };
        Ok(TestOutput::CoverageResults(Box::new(
            CoverageResultsOutput {
                run,
                summary,
                rows: page,
                total: total as u32,
                next_cursor: (next < total).then(|| next.to_string()),
                truncated: next < total,
                stale: !self.coverage.stale.is_empty(),
                file,
            },
        )))
    }

    /// Test > Windows > Code Coverage Results.
    pub(super) fn show_coverage_window(&mut self) -> Result<TestOutput, CommandError> {
        let place = self.controller.apply(ViewRequest::Show {
            id: ids::CODE_COVERAGE_RESULTS.into(),
        })?;
        Ok(TestOutput::CoverageWindow(place.to_json()))
    }

    /// Show Code Coverage Coloring: on, off, or toggled.
    pub(super) fn coverage_coloring(
        &mut self,
        show: Option<bool>,
        cx: &mut Context<Self>,
    ) -> ColoringOutput {
        let show = show.unwrap_or(!self.tests.show_coverage);
        self.tests.show_coverage = show;
        let _ = self.settings.set(
            "test.showCoverageColoring",
            serde_json::Value::Bool(show),
            eludite_commands::settings::SettingScope::User,
        );
        self.coverage.colored.clear();
        self.apply_coverage_coloring(cx);
        self.refresh_coverage_window(cx);
        let documents = if show {
            self.views
                .borrow()
                .keys()
                .filter(|id| {
                    self.coverage
                        .lines
                        .contains_key(&normalize_path(Path::new(id.as_str())))
                })
                .count() as u32
        } else {
            0
        };
        ColoringOutput { show, documents }
    }

    /// A document opened: color it when the last coverage covers it.
    pub(super) fn coverage_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.coverage.data.is_some() {
            self.coverage.colored.remove(id);
            self.apply_coverage_coloring(cx);
        }
    }

    pub(super) fn coverage_document_closed(&mut self, id: &str) {
        self.coverage.colored.remove(id);
        self.coverage.watching.remove(id);
    }

    /// Color every open document the coverage covers (or clear the coloring when it is off). The decorations are
    /// computed on a background thread.
    pub(super) fn apply_coverage_coloring(&mut self, cx: &mut Context<Self>) {
        let views: Vec<(String, Entity<EditorView>)> = self
            .views
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !self.tests.show_coverage || self.coverage.data.is_none() {
            for (_, v) in &views {
                v.update(cx, |v, cx| v.clear_decorations(COVERAGE_LAYER, cx));
            }
            self.coverage.colored.clear();
            return;
        }
        self.coverage.pass += 1;
        let pass = self.coverage.pass;
        let theme = self.theme;
        for (id, view) in views {
            let Some(lines) = self
                .coverage
                .lines
                .get(&normalize_path(Path::new(&id)))
                .cloned()
            else {
                continue;
            };
            let (snapshot, version) = {
                let v = view.read(cx);
                let b = v.editor().buffer();
                (b.snapshot().clone(), b.version())
            };
            if self.coverage.colored.get(&id) == Some(&version) {
                continue;
            }
            // The first coloring of a document is fresh; one after an edit is stale.
            let stale =
                self.coverage.colored.contains_key(&id) || self.coverage.stale.contains(&id);
            if stale {
                self.coverage.stale.insert(id.clone());
            }
            self.coverage.colored.insert(id.clone(), version);
            if !self.coverage.watching.contains_key(&id) {
                let watched = id.clone();
                let sub = cx.observe(&view, move |shell, view, cx| {
                    shell.coverage_view_changed(&watched, &view, cx)
                });
                self.coverage.watching.insert(id.clone(), sub);
            }
            let palette = colors(&theme, stale);
            let task = cx
                .background_executor()
                .spawn(async move { decorations(&snapshot, &lines, palette) });
            cx.spawn(async move |this, cx| {
                let decorations = task.await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.coverage.pass != pass || !shell.tests.show_coverage {
                        return;
                    }
                    view.update(cx, |v, cx| {
                        v.set_decorations(COVERAGE_LAYER, decorations, cx)
                    });
                });
            })
            .detach();
        }
        self.refresh_coverage_window(cx);
    }

    /// A colored document's view changed: an edit makes its coloring stale.
    fn coverage_view_changed(
        &mut self,
        id: &str,
        view: &Entity<EditorView>,
        cx: &mut Context<Self>,
    ) {
        let version = view.read(cx).editor().buffer().version();
        let Some(colored) = self.coverage.colored.get(id) else {
            return;
        };
        if *colored == version || self.coverage.stale.contains(id) {
            return;
        }
        self.coverage.stale.insert(id.to_owned());
        self.apply_coverage_coloring(cx);
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use eludite_lsp::host::{
        CoverageClass, CoverageCollector, CoverageContainer, CoverageFile, CoverageMethod,
        CoverageModule,
    };

    pub(crate) fn sample(path: &str) -> TestCoverage {
        TestCoverage {
            lines_covered: 3,
            lines_valid: 5,
            branches_covered: Some(1),
            branches_valid: Some(2),
            containers: vec![
                CoverageContainer {
                    container: "/s/T/T.csproj|net10.0".into(),
                    collector: CoverageCollector::Mtp,
                    report: Some("/tmp/r.xml".into()),
                    message: None,
                },
                CoverageContainer {
                    container: "/s/U/U.csproj|net10.0".into(),
                    collector: CoverageCollector::None,
                    report: None,
                    message: Some("add a collector".into()),
                },
            ],
            modules: vec![CoverageModule {
                name: "Corpus".into(),
                lines_covered: 3,
                lines_valid: 5,
                branches_covered: None,
                branches_valid: None,
                classes: vec![CoverageClass {
                    name: "Corpus.Calculator".into(),
                    namespace: Some("Corpus".into()),
                    file: Some(path.into()),
                    lines_covered: 3,
                    lines_valid: 5,
                    branches_covered: None,
                    branches_valid: None,
                    methods: vec![
                        CoverageMethod {
                            name: "Add".into(),
                            signature: Some("(int,int)".into()),
                            line: Some(3),
                            lines_covered: 3,
                            lines_valid: 3,
                            branches_covered: Some(1),
                            branches_valid: Some(2),
                        },
                        CoverageMethod {
                            name: "Divide".into(),
                            signature: None,
                            line: Some(8),
                            lines_covered: 0,
                            lines_valid: 2,
                            branches_covered: None,
                            branches_valid: None,
                        },
                    ],
                }],
            }],
            files: vec![CoverageFile {
                path: path.into(),
                lines: vec![
                    vec![3, 1],
                    vec![4, 2, 1, 2],
                    vec![5, 1],
                    vec![8, 0],
                    vec![9, 0],
                ],
            }],
        }
    }

    #[test]
    fn states_rows_and_summary() {
        let c = sample("/s/T/Calculator.cs");
        let states = line_states(&c);
        assert_eq!(
            states[&normalize_path(Path::new("/s/T/Calculator.cs"))],
            [
                (3, LineState::Covered),
                (4, LineState::Partial),
                (5, LineState::Covered),
                (8, LineState::Uncovered),
                (9, LineState::Uncovered)
            ]
        );
        let s = summary_of(&c);
        assert_eq!(s.percent, 60.);
        assert_eq!(s.missing, ["U: add a collector"]);
        let rows = rows_of(&c);
        let shown: Vec<String> = rows
            .iter()
            .map(|r| {
                format!(
                    "{}{} {} {} {}",
                    "  ".repeat(r.depth as usize),
                    r.name,
                    r.covered,
                    r.not_covered,
                    r.percent
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                "Corpus 3 2 60",
                "  Corpus 3 2 60",
                "    Calculator 3 2 60",
                "      Add(int,int) 3 0 100",
                "      Divide 0 2 0"
            ]
        );
        assert_eq!(rows[2].line, Some(3));
        assert_eq!(rows[4].file.as_deref(), Some("/s/T/Calculator.cs"));
    }
}
