//! Brief 0073's watcher: what a tick sees, the thread's debounced reports, and the cost of a tick.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_test_support::{assert_budget, wait_until};

use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "eludite-watch-{name}-{}-{:?}",
        std::process::id(),
        Instant::now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn a_tick_sees_added_removed_and_renamed_files_and_folders() {
    let root = temp("tick");
    write(&root.join("a.txt"), "a");
    write(&root.join("src/lib.cs"), "class A {}");
    write(&root.join("bin/Debug/app.dll"), "x");
    write(&root.join(".git/HEAD"), "ref");
    let mut s = Snapshot::scan(std::slice::from_ref(&root));
    // bin and .git are never walked.
    assert_eq!(s.folders(), 2);
    assert!(s.tick().is_empty());

    write(&root.join("b.txt"), "b");
    std::fs::rename(root.join("src/lib.cs"), root.join("src/Lib2.cs")).unwrap();
    write(&root.join("docs/guide/intro.md"), "# Intro");
    write(&root.join("bin/Debug/other.dll"), "x");
    let r = s.tick();
    assert_eq!(
        r.added,
        [
            root.join("b.txt"),
            root.join("docs"),
            root.join("docs/guide"),
            root.join("docs/guide/intro.md"),
            root.join("src/Lib2.cs"),
        ]
    );
    assert_eq!(r.removed, [root.join("src/lib.cs")]);
    assert!(!r.project_files);

    std::fs::remove_dir_all(root.join("docs")).unwrap();
    let r = s.tick();
    assert_eq!(
        r.removed,
        [
            root.join("docs"),
            root.join("docs/guide"),
            root.join("docs/guide/intro.md")
        ]
    );
    // A content change of an ordinary file is not a change of the tree.
    write(&root.join("a.txt"), "changed");
    assert!(s.tick().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_project_file_write_is_a_project_change() {
    let root = temp("project");
    let csproj = root.join("App/App.csproj");
    write(&csproj, "<Project Sdk=\"Microsoft.NET.Sdk\" />");
    let mut s = Snapshot::scan(std::slice::from_ref(&root));
    assert!(s.tick().is_empty());
    write(&csproj, "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup />\n</Project>\n");
    let r = s.tick();
    assert!(r.project_files && r.added.is_empty() && r.removed.is_empty());
    write(&root.join("Directory.Build.props"), "<Project />");
    let r = s.tick();
    assert!(r.project_files);
    assert_eq!(r.added, [root.join("Directory.Build.props")]);
    assert!(is_project_file(Path::new("/x/S.slnx")));
    assert!(is_project_file(Path::new("/x/A.FSPROJ")));
    assert!(!is_project_file(Path::new("/x/a.cs")));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_thread_reports_once_a_change_settles() {
    let root = temp("thread");
    write(&root.join("App/Program.cs"), "");
    let reports: Arc<Mutex<Vec<WatchReport>>> = Arc::default();
    let sink = reports.clone();
    let w = WorkspaceWatcher::start(
        vec![root.clone()],
        vec![root.join("App")],
        WatchOptions {
            poll: Duration::from_millis(20),
            debounce: Duration::from_millis(20),
        },
        move |r| sink.lock().unwrap().push(r),
    );
    wait_until("the first scan", Duration::from_secs(5), || w.ready());
    write(&root.join("App/New.cs"), "");
    wait_until("a report", Duration::from_secs(5), || {
        !reports.lock().unwrap().is_empty()
    });
    let r = reports.lock().unwrap().remove(0);
    assert_eq!(r.added, [root.join("App/New.cs")]);
    assert!(r.under_projects && !r.project_files);

    // A rebaseline swallows what changed before it.
    write(&root.join("Outside.txt"), "");
    w.rebaseline();
    write(&root.join("App/App.csproj"), "<Project />");
    wait_until("the project report", Duration::from_secs(5), || {
        !reports.lock().unwrap().is_empty()
    });
    let r = reports.lock().unwrap().remove(0);
    assert!(r.project_files, "{r:?}");
    assert!(!r.added.contains(&root.join("Outside.txt")), "{r:?}");
    drop(w);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_tick_over_ten_thousand_files_is_cheap() {
    let root = temp("budget");
    // 10,000 files in 500 folders of 20, with a project file in each tenth folder.
    for d in 0..500 {
        let dir = root.join(format!("p{}/f{d}", d / 50));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..20 {
            std::fs::write(dir.join(format!("File{f}.cs")), "").unwrap();
        }
        if d % 10 == 0 {
            std::fs::write(dir.join("P.csproj"), "<Project />").unwrap();
        }
    }
    let started = Instant::now();
    let mut s = Snapshot::scan(std::slice::from_ref(&root));
    let scan = started.elapsed();
    assert_eq!(s.files, 10_050);
    // Let the folders' times settle so a tick stats only.
    std::thread::sleep(Duration::from_millis(2100));
    let _ = s.tick();
    let ticks = 10;
    let started = Instant::now();
    for _ in 0..ticks {
        assert!(s.tick().is_empty());
    }
    let tick = started.elapsed() / ticks;
    eprintln!(
        "watcher on 10,050 files in {} folders: first scan {:.1} ms, tick {:.2} ms ({:.2} % of the 1 s poll)",
        s.folders(),
        scan.as_secs_f64() * 1e3,
        tick.as_secs_f64() * 1e3,
        tick.as_secs_f64() * 100.0
    );
    // Under 1 percent CPU at the 1 s poll.
    assert_budget(
        "a watcher tick over 10,000 files",
        tick,
        Duration::from_millis(10),
    );
    let _ = std::fs::remove_dir_all(&root);
}
