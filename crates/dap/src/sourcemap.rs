//! Source maps for vscode-js-debug's frames (brief 0038, `protocol/schemas/dap-js-debug.md`): js-debug answers
//! `stackTrace` with the original location (an `app.ts` under the page's web root) when a source map applies, and says
//! nothing of the script that runs. The client's sink of a browser session rewrites each `stackTrace` answer with
//! [`adapt_stack`] before the shell sees it: a frame whose source is not a file (js-debug's `repl`, a source served by
//! reference) loses its path, and a frame in a file a map on disk leads to gets the generated script's location
//! (`x-eluditeGenerated`, [`crate::types::GeneratedLocation`]). It runs on the client's reader thread (it reads files),
//! never on the UI thread.
//!
//! The map is found beside the original file: `<stem>.js.map`, `<stem>.mjs.map`, `<stem>.map`, else any `*.map` in
//! the same folder (at most [`MAX_SCANNED`]) whose `sources` name it. Maps are cached by path and modification time.
//! Only version 3 maps with `mappings` are read; `sections` (index maps) are not.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{Value, json};

/// The most `*.map` files of a folder looked into for one original file.
pub const MAX_SCANNED: usize = 64;
/// The largest map read (bytes).
pub const MAX_MAP_BYTES: u64 = 16 * 1024 * 1024;

/// One mapping: where in the generated line, and what it maps to (source, line, column; 0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Segment {
    column: u32,
    source: Option<(u32, u32, u32)>,
}

/// A parsed version 3 source map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMap {
    /// The generated script: the map's `file` beside the map, else the map's path without `.map`.
    pub generated: PathBuf,
    /// The sources, resolved against the map's folder and `sourceRoot` (`None` for a url or a scheme).
    pub sources: Vec<Option<PathBuf>>,
    /// Per generated line, its segments in column order.
    lines: Vec<Vec<Segment>>,
}

/// `path` with `.` and `..` resolved lexically.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn base64(c: u8) -> Option<i64> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as i64)
}

/// The base64 VLQ numbers of one segment.
fn vlq(segment: &str) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    let (mut value, mut shift) = (0i64, 0u32);
    for c in segment.bytes() {
        let d = base64(c).ok_or_else(|| format!("bad mapping character {:?}", c as char))?;
        value += (d & 31) << shift;
        if d & 32 != 0 {
            shift += 5;
            if shift > 60 {
                return Err("a mapping number is too long".into());
            }
        } else {
            let negative = value & 1 == 1;
            let v = value >> 1;
            out.push(if negative { -v } else { v });
            value = 0;
            shift = 0;
        }
    }
    if shift != 0 {
        return Err("a mapping ends inside a number".into());
    }
    Ok(out)
}

impl SourceMap {
    /// Parse `text`, the map at `map_path`.
    pub fn parse(text: &str, map_path: &Path) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        if v["version"] != 3 {
            return Err("not a version 3 source map".into());
        }
        let mappings = v["mappings"]
            .as_str()
            .ok_or("a source map without `mappings`")?;
        let dir = map_path.parent().unwrap_or(Path::new(""));
        let root = v["sourceRoot"].as_str().unwrap_or_default();
        let sources = v["sources"]
            .as_array()
            .map(|s| {
                s.iter()
                    .map(|s| {
                        let s = s.as_str()?;
                        let joined = format!(
                            "{root}{}{s}",
                            if root.is_empty() || root.ends_with('/') {
                                ""
                            } else {
                                "/"
                            }
                        );
                        if joined.contains("://") || joined.starts_with("data:") {
                            return None;
                        }
                        Some(normalize(&dir.join(joined)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let generated = match v["file"].as_str().filter(|f| !f.is_empty()) {
            Some(f) => normalize(&dir.join(f)),
            None => {
                let s = map_path.to_string_lossy();
                PathBuf::from(s.strip_suffix(".map").unwrap_or(&s))
            }
        };
        let mut lines = Vec::new();
        let (mut source, mut sline, mut scol, mut name) = (0i64, 0i64, 0i64, 0i64);
        for line in mappings.split(';') {
            let mut column = 0i64;
            let mut segs = Vec::new();
            for seg in line.split(',').filter(|s| !s.is_empty()) {
                let n = vlq(seg)?;
                column += n[0];
                let mapped = if n.len() >= 4 {
                    source += n[1];
                    sline += n[2];
                    scol += n[3];
                    if n.len() >= 5 {
                        name += n[4];
                    }
                    Some((source as u32, sline as u32, scol as u32))
                } else {
                    None
                };
                segs.push(Segment {
                    column: column.max(0) as u32,
                    source: mapped,
                });
            }
            segs.sort_by_key(|s| s.column);
            lines.push(segs);
        }
        let _ = name;
        Ok(Self {
            generated,
            sources,
            lines,
        })
    }

    /// The index of `original` among the sources.
    pub fn source_index(&self, original: &Path) -> Option<usize> {
        let want = normalize(original);
        self.sources
            .iter()
            .position(|s| s.as_deref() == Some(want.as_path()))
    }

    /// Where 1-based (`line`, `column`) of source `source` runs in the generated script, 1-based: the mapping of that
    /// line nearest before the column (the line's first when none is), the earliest generated place when several
    /// map there.
    pub fn generated(&self, source: usize, line: u32, column: u32) -> Option<(u32, u32)> {
        let (line0, col0) = (line.checked_sub(1)?, column.saturating_sub(1));
        let mut best: Option<((u32, u32), u32)> = None; // ((gen line, gen col), source column)
        let mut first: Option<((u32, u32), u32)> = None;
        for (gl, segs) in self.lines.iter().enumerate() {
            for s in segs {
                let Some((si, sl, sc)) = s.source else {
                    continue;
                };
                if si as usize != source || sl != line0 {
                    continue;
                }
                let at = (gl as u32, s.column);
                if first.is_none_or(|(_, c)| sc < c) {
                    first = Some((at, sc));
                }
                if sc <= col0 && best.is_none_or(|(_, c)| sc > c) {
                    best = Some((at, sc));
                }
            }
        }
        best.or(first).map(|((l, c), _)| (l + 1, c + 1))
    }
}

/// Maps read, by path and modification time, and which map leads to each original file.
#[derive(Debug, Default)]
pub struct MapCache {
    maps: HashMap<PathBuf, (Option<SystemTime>, Arc<SourceMap>)>,
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

impl MapCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The map at `path`, read again when it changed.
    fn map(&mut self, path: &Path) -> Option<Arc<SourceMap>> {
        let m = modified(path);
        if let Some((at, map)) = self.maps.get(path)
            && *at == m
        {
            return Some(map.clone());
        }
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_MAP_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let map = Arc::new(SourceMap::parse(&text, path).ok()?);
        self.maps.insert(path.to_path_buf(), (m, map.clone()));
        Some(map)
    }

    /// The map leading to `original`, and its index there.
    fn find(&mut self, original: &Path) -> Option<(Arc<SourceMap>, usize)> {
        let dir = original.parent()?;
        let stem = original.file_stem()?.to_string_lossy().into_owned();
        let named = [
            dir.join(format!("{stem}.js.map")),
            dir.join(format!("{stem}.mjs.map")),
            dir.join(format!("{stem}.map")),
        ];
        let mut tried = Vec::new();
        for p in named {
            if let Some(m) = self.map(&p)
                && let Some(i) = m.source_index(original)
            {
                return Some((m, i));
            }
            tried.push(p);
        }
        let others: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "map") && !tried.contains(p))
            .take(MAX_SCANNED)
            .collect();
        others.into_iter().find_map(|p| {
            let m = self.map(&p)?;
            let i = m.source_index(original)?;
            Some((m, i))
        })
    }

    /// Where 1-based (`line`, `column`) of `original` runs, when a map on disk leads to it.
    pub fn generated_for(
        &mut self,
        original: &Path,
        line: u32,
        column: u32,
    ) -> Option<crate::types::GeneratedLocation> {
        let (map, source) = self.find(original)?;
        let (l, c) = map.generated(source, line, column)?;
        Some(crate::types::GeneratedLocation {
            path: map.generated.to_string_lossy().into_owned(),
            line: Some(i64::from(l)),
            column: Some(i64::from(c)),
        })
    }
}

/// Whether `path` names a file on this machine (an absolute path), not js-debug's `repl` or a page's url.
fn is_file_path(path: &str) -> bool {
    Path::new(path).is_absolute() || (path.len() > 2 && path.as_bytes()[1] == b':')
}

/// A browser session's `stackTrace` answer (`body`), rewritten for the shell: frames whose source is no file lose
/// the path (they show as external code), and frames a map on disk leads to get the generated script's location.
pub fn adapt_stack(body: &mut Value, cache: &mut MapCache) {
    let Some(frames) = body["stackFrames"].as_array_mut() else {
        return;
    };
    for f in frames {
        let Some(path) = f["source"]["path"].as_str().map(str::to_owned) else {
            continue;
        };
        if !is_file_path(&path) {
            if let Some(src) = f["source"].as_object_mut() {
                src.remove("path");
            }
            continue;
        }
        let (Some(line), Some(column)) = (f["line"].as_u64(), f["column"].as_u64()) else {
            continue;
        };
        if line == 0 {
            continue;
        }
        if let Some(g) = cache.generated_for(Path::new(&path), line as u32, column.max(1) as u32)
            && normalize(Path::new(&g.path)) != normalize(Path::new(&path))
        {
            f["x-eluditeGenerated"] = json!(g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web corpus's app.ts, compiled by tsc 5.9 with `--sourceMap` (corpus/README.md).
    fn corpus() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/web/minimal-api/wwwroot")
    }

    #[test]
    fn vlq_numbers_decode() {
        assert_eq!(vlq("AAAA").unwrap(), vec![0, 0, 0, 0]);
        assert_eq!(vlq("SAAS").unwrap(), vec![9, 0, 0, 9]);
        assert_eq!(vlq("D").unwrap(), vec![-1]);
        assert_eq!(vlq("gB").unwrap(), vec![16]);
        assert!(vlq("g").is_err());
        assert!(vlq("!").is_err());
    }

    /// The corpus's map: app.ts's lines map to app.js's, a frame in `total` and one in `onAdd` (where js-debug stopped
    /// on the real page) get app.js's line and column, and the frames of no file lose their path.
    #[test]
    fn the_corpus_map_leads_from_app_ts_to_app_js() {
        let dir = normalize(&corpus());
        let ts = dir.join("app.ts");
        let text = std::fs::read_to_string(dir.join("app.js.map")).unwrap();
        let map = SourceMap::parse(&text, &dir.join("app.js.map")).unwrap();
        assert_eq!(map.generated, dir.join("app.js"));
        assert_eq!(map.source_index(&ts), Some(0));
        // app.ts 15:5 `sum += items[i].price;` is app.js 9:9; 25:15 `total(cart)` is app.js 18:17.
        assert_eq!(map.generated(0, 15, 5), Some((9, 9)));
        assert_eq!(map.generated(0, 25, 15), Some((18, 17)));
        // A line with no code (the interface) maps nowhere.
        assert_eq!(map.generated(0, 6, 1), None);
        let mut cache = MapCache::new();
        let g = cache.generated_for(&ts, 15, 5).unwrap();
        assert_eq!(
            (g.path.as_str(), g.line, g.column),
            (dir.join("app.js").to_str().unwrap(), Some(9), Some(9))
        );
        let mut body = json!({"stackFrames": [
            {"id": 0, "name": "total", "line": 15, "column": 5,
             "source": {"name": "app.ts", "path": ts.to_string_lossy(), "sourceReference": 0}},
            {"id": 2, "name": "<anonymous>", "line": 1, "column": 48,
             "source": {"name": "repl", "path": "repl", "sourceReference": 66237813}},
            {"name": "setTimeout", "id": 7, "line": 0, "column": 0, "presentationHint": "label"},
            {"id": 3, "name": "x", "line": 9, "column": 9,
             "source": {"name": "app.js", "path": dir.join("app.js").to_string_lossy()}}
        ]});
        adapt_stack(&mut body, &mut cache);
        let f = &body["stackFrames"];
        assert_eq!(f[0]["x-eluditeGenerated"]["line"], 9);
        assert_eq!(f[0]["x-eluditeGenerated"]["column"], 9);
        assert!(f[1]["source"].get("path").is_none(), "{}", f[1]);
        assert_eq!(f[1]["source"]["name"], "repl");
        assert!(f[2].get("x-eluditeGenerated").is_none());
        // The generated script itself has no map leading to it.
        assert!(f[3].get("x-eluditeGenerated").is_none());
        let decoded: crate::types::StackTraceResponse = serde_json::from_value(body).unwrap();
        assert_eq!(
            decoded.stack_frames[0].generated.as_ref().map(|g| g.line),
            Some(Some(9))
        );
        assert_eq!(decoded.stack_frames[1].path(), None);
    }

    /// A map found by its `sources` rather than its name, with a `sourceRoot`, and one that changed on disk.
    #[test]
    fn maps_are_found_by_their_sources_and_read_again_when_they_change() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let original = src.join("main.ts");
        std::fs::write(&original, "let a = 1;\n").unwrap();
        // bundle.js line 2 col 1 <- src/main.ts line 1 col 1 (sourceRoot "src/", sources ["main.ts"]).
        let map = json!({"version": 3, "file": "bundle.js", "sourceRoot": "../src/", "sources": ["main.ts"],
                         "mappings": ";AAAA"});
        let out = t.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("bundle.js.map"), map.to_string()).unwrap();
        // Not beside the original: not found.
        let mut cache = MapCache::new();
        assert!(cache.generated_for(&original, 1, 1).is_none());
        // Beside it, under another name: found by its sources.
        let map2 = json!({"version": 3, "file": "../out/bundle.js", "sources": ["main.ts"], "mappings": ";AAAA"});
        std::fs::write(src.join("bundle.map"), map2.to_string()).unwrap();
        let g = cache.generated_for(&original, 1, 1).unwrap();
        assert_eq!(
            (g.path, g.line),
            (
                out.join("bundle.js").to_string_lossy().into_owned(),
                Some(2)
            )
        );
        // Rewritten: the new mapping (line 3).
        std::thread::sleep(std::time::Duration::from_millis(20));
        let map3 = json!({"version": 3, "file": "../out/bundle.js", "sources": ["main.ts"], "mappings": ";;AAAA"});
        std::fs::write(src.join("bundle.map"), map3.to_string()).unwrap();
        let f = std::fs::File::options()
            .append(true)
            .open(src.join("bundle.map"))
            .unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(cache.generated_for(&original, 1, 1).unwrap().line, Some(3));
        assert!(SourceMap::parse("{\"version\": 2}", Path::new("/x.map")).is_err());
    }
}
