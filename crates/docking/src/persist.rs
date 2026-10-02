//! Where layouts live on disk, and a background writer so saving never runs on
//! the UI thread.
//!
//! Layout directory: `<config dir>/eludite/layouts/`, where `<config dir>` is
//! the OS user config directory (the `dirs` crate's `config_dir()`):
//!
//! | OS | Layout directory |
//! |---|---|
//! | Linux | `$XDG_CONFIG_HOME/eludite/layouts/`, default `~/.config/eludite/layouts/` |
//! | Windows | `%APPDATA%\eludite\layouts\` (`C:\Users\<user>\AppData\Roaming\eludite\layouts\`) |
//! | macOS | `~/Library/Application Support/eludite/layouts/` |
//!
//! `ELUDITE_CONFIG_DIR`, when set, replaces `<config dir>/eludite`.
//!
//! Files in it:
//! - `default.json`: the layout used when no solution is open, and the starting
//!   point for a solution that has no layout yet.
//! - `solutions/<file stem>-<16 hex digits>.json`: one per solution path. The
//!   hex digits are the FNV-1a 64-bit hash of the absolute solution path, so
//!   two `App.sln` files in different folders do not collide.
//! - `named/<name>.json`: named layouts (Design, Debug).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::model::{DockLayout, ToolWindowRegistry};

/// `<config dir>/eludite`, or `ELUDITE_CONFIG_DIR`.
pub fn eludite_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ELUDITE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    dirs::config_dir().map(|d| d.join("eludite"))
}

/// FNV-1a, 64-bit: stable across Rust versions and platforms (unlike `std`'s hasher).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('.').to_owned();
    if s.is_empty() { "_".into() } else { s }
}

/// Which file a layout came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutSource {
    Solution(PathBuf),
    Default(PathBuf),
    BuiltIn,
}

/// The layout directory and the file naming rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutStore {
    dir: PathBuf,
}

impl LayoutStore {
    /// A store rooted at `dir` (the `layouts` directory itself).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The per-user store described in the module docs.
    pub fn user() -> Option<Self> {
        eludite_config_dir().map(|d| Self::new(d.join("layouts")))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn default_path(&self) -> PathBuf {
        self.dir.join("default.json")
    }

    /// The layout file for a solution path (made absolute, not canonicalized,
    /// so it works for a solution that does not exist yet).
    pub fn solution_path(&self, solution: &Path) -> PathBuf {
        let abs = std::path::absolute(solution).unwrap_or_else(|_| solution.to_owned());
        let stem = abs
            .file_stem()
            .map(|s| sanitize(&s.to_string_lossy()))
            .unwrap_or_else(|| "solution".into());
        let hash = fnv1a64(abs.to_string_lossy().as_bytes());
        self.dir
            .join("solutions")
            .join(format!("{stem}-{hash:016x}.json"))
    }

    pub fn named_path(&self, name: &str) -> PathBuf {
        self.dir
            .join("named")
            .join(format!("{}.json", sanitize(name)))
    }

    /// The file a session saves to: the solution's, or the default.
    pub fn path_for(&self, solution: Option<&Path>) -> PathBuf {
        match solution {
            Some(s) => self.solution_path(s),
            None => self.default_path(),
        }
    }

    /// Load the layout for `solution`: its own file, else `default.json`, else
    /// the built-in VS default. Unreadable or newer-version files are skipped
    /// (and left on disk). The result is normalized against `registry`.
    ///
    /// This reads small files synchronously; call it before the window opens
    /// or from a background task, never from a frame.
    pub fn load(
        &self,
        solution: Option<&Path>,
        registry: &ToolWindowRegistry,
    ) -> (DockLayout, LayoutSource) {
        let mut candidates = Vec::new();
        if let Some(s) = solution {
            candidates.push(LayoutSource::Solution(self.solution_path(s)));
        }
        candidates.push(LayoutSource::Default(self.default_path()));
        for source in candidates {
            let (LayoutSource::Solution(path) | LayoutSource::Default(path)) = &source else {
                continue;
            };
            if let Some(mut layout) = read_layout(path) {
                layout.normalize(registry);
                return (layout, source);
            }
        }
        (DockLayout::default_vs(registry), LayoutSource::BuiltIn)
    }

    /// Named layouts on disk, sorted.
    pub fn list_named(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.join("named"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .collect();
        names.sort();
        names
    }

    pub fn load_named(&self, name: &str, registry: &ToolWindowRegistry) -> Option<DockLayout> {
        let mut l = read_layout(&self.named_path(name))?;
        l.normalize(registry);
        Some(l)
    }
}

/// Read and parse a layout file. `None` if missing, unreadable or unsupported.
pub fn read_layout(path: &Path) -> Option<DockLayout> {
    let text = std::fs::read_to_string(path).ok()?;
    match DockLayout::from_json(&text) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("eludite: ignoring layout {}: {e}", path.display());
            None
        }
    }
}

/// Write `text` to `path` atomically (temp file, then rename), creating directories.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Called on the writer thread after each file is written (tests use it to
/// check which thread wrote).
pub type WriteObserver = Arc<dyn Fn(&Path, &std::io::Result<()>) + Send + Sync>;

enum Msg {
    Save(PathBuf, String),
    Flush(futures::channel::oneshot::Sender<()>),
}

/// A background thread that writes layout files. `save` returns at once;
/// writes to the same path within `debounce` coalesce into one (the last
/// wins). `flush` writes everything pending now and resolves when done.
#[derive(Clone)]
pub struct LayoutWriter {
    tx: mpsc::Sender<Msg>,
}

impl std::fmt::Debug for LayoutWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LayoutWriter")
    }
}

impl LayoutWriter {
    pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(400);

    pub fn spawn(debounce: Duration, observer: Option<WriteObserver>) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        thread::Builder::new()
            .name("eludite-layout-writer".into())
            .spawn(move || writer_loop(rx, debounce, observer))
            .expect("spawn layout writer thread");
        Self { tx }
    }

    /// Queue `layout` for writing to `path`. Serializing is done here, on the
    /// caller's thread (a few kilobytes); the file I/O is not.
    pub fn save(&self, path: PathBuf, layout: &DockLayout) {
        let _ = self.tx.send(Msg::Save(path, layout.to_json()));
    }

    /// Write everything pending now. Await the result off the UI thread's
    /// critical path (for example in an app-quit handler).
    pub fn flush(&self) -> futures::channel::oneshot::Receiver<()> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.tx.send(Msg::Flush(tx));
        rx
    }
}

fn writer_loop(rx: mpsc::Receiver<Msg>, debounce: Duration, observer: Option<WriteObserver>) {
    let mut pending: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut deadline: Option<Instant> = None;
    let write_all = |pending: &mut BTreeMap<PathBuf, String>| {
        for (path, text) in std::mem::take(pending) {
            let result = write_atomic(&path, &text);
            if let Err(e) = &result {
                eprintln!("eludite: failed to save layout {}: {e}", path.display());
            }
            if let Some(o) = &observer {
                o(&path, &result);
            }
        }
    };
    loop {
        let msg = match deadline {
            Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Save(path, text)) => {
                pending.insert(path, text);
                deadline = Some(Instant::now() + debounce);
            }
            Ok(Msg::Flush(done)) => {
                write_all(&mut pending);
                deadline = None;
                let _ = done.send(());
            }
            Err(RecvTimeoutError::Timeout) => {
                write_all(&mut pending);
                deadline = None;
            }
            Err(RecvTimeoutError::Disconnected) => {
                write_all(&mut pending);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::model::{DockSide, ids};

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn paths_per_solution() {
        let store = LayoutStore::new("/cfg/eludite/layouts");
        assert_eq!(
            store.default_path(),
            Path::new("/cfg/eludite/layouts/default.json")
        );
        let a = store.solution_path(Path::new("/src/a/App.sln"));
        let b = store.solution_path(Path::new("/src/b/App.sln"));
        assert_ne!(a, b);
        assert_eq!(a, store.solution_path(Path::new("/src/a/App.sln")));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("App-") && name.ends_with(".json"),
            "{name}"
        );
        assert_eq!(name.len(), "App-".len() + 16 + ".json".len());
        assert!(a.starts_with("/cfg/eludite/layouts/solutions"));
        assert_eq!(
            store.named_path("Debug / x"),
            Path::new("/cfg/eludite/layouts/named/Debug___x.json")
        );
        assert_eq!(store.path_for(None), store.default_path());
        // FNV-1a test vector.
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn load_prefers_solution_then_default_then_builtin() {
        let dir = tempdir();
        let store = LayoutStore::new(dir.path());
        let reg = ToolWindowRegistry::vs_default();
        let sln = Path::new("/work/Shop.sln");

        let (l, src) = store.load(Some(sln), &reg);
        assert_eq!(src, LayoutSource::BuiltIn);
        assert_eq!(l, DockLayout::default_vs(&reg));

        let mut d = DockLayout::default_vs(&reg);
        d.dock_to(ids::OUTPUT, DockSide::Left).unwrap();
        write_atomic(&store.default_path(), &d.to_json()).unwrap();
        let (l, src) = store.load(Some(sln), &reg);
        assert_eq!(src, LayoutSource::Default(store.default_path()));
        assert_eq!(l, d);

        let mut s = DockLayout::default_vs(&reg);
        s.auto_hide(ids::PROPERTIES).unwrap();
        write_atomic(&store.solution_path(sln), &s.to_json()).unwrap();
        let (l, src) = store.load(Some(sln), &reg);
        assert_eq!(src, LayoutSource::Solution(store.solution_path(sln)));
        assert_eq!(l, s);

        // A newer-version file is skipped and left alone.
        std::fs::write(store.solution_path(sln), "{\"version\": 999}").unwrap();
        let (_, src) = store.load(Some(sln), &reg);
        assert_eq!(src, LayoutSource::Default(store.default_path()));
        assert!(
            std::fs::read_to_string(store.solution_path(sln))
                .unwrap()
                .contains("999")
        );
    }

    #[test]
    fn named_layouts() {
        let dir = tempdir();
        let store = LayoutStore::new(dir.path());
        let reg = ToolWindowRegistry::vs_default();
        let mut debug = DockLayout::default_vs(&reg);
        debug.hide(ids::TOOLBOX).unwrap();
        write_atomic(&store.named_path("Debug"), &debug.to_json()).unwrap();
        write_atomic(
            &store.named_path("Design"),
            &DockLayout::default_vs(&reg).to_json(),
        )
        .unwrap();
        assert_eq!(store.list_named(), ["Debug", "Design"]);
        assert_eq!(store.load_named("Debug", &reg), Some(debug));
        assert_eq!(store.load_named("Nope", &reg), None);
    }

    #[test]
    fn writer_debounces_off_thread_and_flushes() {
        let dir = tempdir();
        let path = dir.path().join("sub").join("layout.json");
        let writes: Arc<Mutex<Vec<(PathBuf, thread::ThreadId)>>> = Arc::default();
        let w2 = writes.clone();
        let writer = LayoutWriter::spawn(
            Duration::from_millis(50),
            Some(Arc::new(move |p: &Path, r: &std::io::Result<()>| {
                assert!(r.is_ok());
                w2.lock()
                    .unwrap()
                    .push((p.to_owned(), thread::current().id()));
            })),
        );
        let reg = ToolWindowRegistry::vs_default();
        let mut l = DockLayout::default_vs(&reg);
        for side in DockSide::ALL {
            l.dock_to(ids::OUTPUT, side).unwrap();
            writer.save(path.clone(), &l);
        }
        assert!(
            writes.lock().unwrap().is_empty(),
            "nothing written synchronously"
        );
        futures::executor::block_on(writer.flush()).unwrap();
        let w = writes.lock().unwrap().clone();
        assert_eq!(w.len(), 1, "three saves coalesce into one write");
        assert_ne!(
            w[0].1,
            thread::current().id(),
            "written on the writer thread"
        );
        assert_eq!(read_layout(&path).unwrap(), l);

        // Debounce expiry writes without a flush.
        l.hide(ids::OUTPUT).unwrap();
        writer.save(path.clone(), &l);
        let start = Instant::now();
        while writes.lock().unwrap().len() < 2 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "debounced write never happened"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(read_layout(&path).unwrap(), l);
    }
}
