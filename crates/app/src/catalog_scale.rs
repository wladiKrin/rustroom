//! How the Library's catalog-wide paths scale (M10.14e, P-21): All Photos, search, smart collections,
//! the folder tree, and one rail and one grid frame, on a synthetic catalog of 10k, 30k and 100k photos.
//!
//! `cargo test --release --workspace catalog_scale -- --ignored --nocapture`
//!
//! ⚠⚠ **A test in the binary, not an example, on purpose.** The glue that opens All Photos or
//! evaluates a smart collection is `pub(crate)` on `RustroomApp`; an example could only mirror it,
//! and a mirror measures what the app used to do. This drives the real methods on a headless app.
//!
//! ⚠ The synthetic catalog: year/day folders of 70 photos (30 % RAF), a sidecar on 10 %, a keyword
//! on 20 % (50 keywords), a rating on 30 %, two collections of 1,000, three smart rules. The photo
//! files themselves do not exist — a local file that is absent stays visible, and nothing here reads
//! pixels. ⚠ The filesystem cache is warm (every sidecar was just written), so a first open after a
//! reboot will be slower than these numbers.

use crate::test_app::TestDir;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// `f`'s result and how long it took.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, ms(t))
}

/// The median of `n` headless frames of `body`, after one warm-up frame.
fn frame_ms(ctx: &egui::Context, n: usize, mut body: impl FnMut(&mut egui::Ui)) -> f64 {
    let mut run = || {
        let t = Instant::now();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                ..Default::default()
            },
            |ui| body(ui),
        );
        ms(t)
    };
    run();
    let mut all: Vec<f64> = (0..n).map(|_| run()).collect();
    all.sort_by(|a, b| a.total_cmp(b));
    all[n / 2]
}

struct Ids {
    smart: Vec<(&'static str, i64)>,
}

/// Fill `cat` with `n` photos under `root`, the shape described in the module doc.
fn populate(cat: &catalog::db::Catalog, root: &Path, n: usize) -> Ids {
    use rustroom_app::rule::{Cmp, Criterion, Expr};
    const PER_FOLDER: usize = 70;
    let keywords: Vec<i64> = (0..50)
        .map(|k| cat.create_keyword(&format!("keyword {k}"), None).unwrap())
        .collect();
    let collections = [
        cat.create_collection("Portfolio").unwrap(),
        cat.create_collection("Prints").unwrap(),
    ];
    let mut i = 0usize;
    let mut day = 0usize;
    while i < n {
        let (year, doy) = (2015 + day / 365, day % 365);
        let dir = root.join(format!("{year}")).join(format!(
            "{year}-{:02}-{:02}",
            doy / 31 + 1,
            doy % 28 + 1
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut entries = Vec::new();
        for _ in 0..PER_FOLDER.min(n - i) {
            let ext = if i % 10 < 3 { "RAF" } else { "JPG" };
            let name = format!("DSCF{i:06}.{ext}");
            let path = dir.join(&name);
            entries.push(catalog::CatalogEntry {
                path: path.clone(),
                file_name: name,
                modified: SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(1_500_000_000 + i as u64),
                edited: None,
            });
            i += 1;
        }
        cat.sync_folder(&dir, &entries).unwrap();
        for e in &entries {
            let k = e.file_name[4..10].parse::<usize>().unwrap();
            if k % 10 == 0 {
                let edits = edit_model::Edits {
                    exposure: 0.3,
                    ..Default::default()
                };
                rustroom_app::sidecar::save(&e.path, &edits).unwrap();
            }
            if k % 5 == 1 {
                cat.assign_keyword(keywords[k % 50], &e.path).unwrap();
            }
            if k % 10 < 3 {
                let marks = catalog::db::Marks {
                    rating: (k % 5) as u8 + 1,
                    ..Default::default()
                };
                cat.set_marks(&e.path, 0, marks).unwrap();
            }
            if k < 2000 {
                cat.add_to_collection(collections[k % 2], &e.path, 0)
                    .unwrap();
            }
        }
        day += 1;
    }
    let rule = |e: Expr| serde_json::to_string(&e).unwrap();
    let smart = vec![
        (
            "rating >= 3",
            cat.create_smart(
                "Three stars",
                &rule(Expr::Is(Criterion::Rating {
                    op: Cmp::AtLeast,
                    value: 3,
                })),
            )
            .unwrap(),
        ),
        (
            "text DSCF01",
            cat.create_smart("Text", &rule(Expr::Is(Criterion::Text("DSCF01".into()))))
                .unwrap(),
        ),
        (
            "keyword 1",
            cat.create_smart("Keyword", &rule(Expr::Is(Criterion::Keyword(keywords[1]))))
                .unwrap(),
        ),
    ];
    Ids { smart }
}

fn one_size(n: usize) {
    let dir = TestDir::new(&format!("scale-{n}"));
    let root: PathBuf = dir.0.join("photos");
    let cat = catalog::db::Catalog::open(&dir.0.join("catalog.db")).unwrap();
    let (ids, populate_ms) = timed(|| populate(&cat, &root, n));
    println!(
        "\n== {n} photos (populated in {:.1} s)",
        populate_ms / 1000.0
    );

    let ctx = crate::test_ui::ctx();
    let mut app = crate::test_app::for_test();
    app.catalog = Some(cat);

    // The parts of `open_all_photos`, each through the function the app calls.
    let cat = app.catalog.as_ref().unwrap();
    let (rows, t_rows) = timed(|| cat.all_photos().unwrap());
    let (folders, t_folders) = timed(|| cat.folders().unwrap());
    let ((), t_tree) = timed(|| {
        let _ = rustroom_app::folder_tree::build(&folders, &Default::default());
    });
    let members: Vec<(PathBuf, u32)> = rows.iter().map(|r| (r.path.clone(), r.copy)).collect();
    let (scan, t_scan) = timed(|| {
        rustroom_app::collections::scan_of(&members, rustroom_app::sidecar::edited_at, |_| {
            SystemTime::UNIX_EPOCH
        })
    });
    let (_, t_entries) =
        timed(|| rustroom_app::entries::build(&scan, rustroom_app::sidecar::load_all));
    println!(
        "  catalog rows {t_rows:8.1} ms   folders {t_folders:6.1} ms ({} rows)   folder tree {t_tree:6.1} ms",
        folders.len()
    );
    println!("  scan_of (a sidecar stat each) {t_scan:8.1} ms   entries::build (a sidecar load each) {t_entries:8.1} ms");

    // The whole open, the real method.
    let ((), t_open) = timed(|| app.open_all_photos(&ctx));
    let shown = app.library.as_ref().map_or(0, |l| l.entries.len());
    println!("  open_all_photos (whole)          {t_open:8.1} ms   ({shown} shown)");
    let ((), t_xmp) = timed(|| app.import_xmp());
    let ((), t_marks) = timed(|| app.reload_marks());
    let ((), t_caps) = timed(|| app.reload_captures());
    println!("    of which, again: import_xmp {t_xmp:7.1} ms   reload_marks {t_marks:7.1} ms   reload_captures {t_caps:7.1} ms");

    // Search: the text box, applied to the open view.
    let mut f = app.filter.clone();
    f.text = "DSCF01".into();
    let ((), t_search) = timed(|| app.library.as_mut().unwrap().refilter(&f));
    let hits = app.library.as_ref().map_or(0, |l| l.visible.len());
    println!("  search \"DSCF01\" (refilter)       {t_search:8.1} ms   ({hits} hits)");
    let cleared = app.filter.clone();
    app.library.as_mut().unwrap().refilter(&cleared);

    // Smart collections: evaluated in full on every open and on every catalog change.
    for (label, id) in &ids.smart {
        let (m, t) = timed(|| app.smart_members(*id));
        let k = m.map_or(0, |m| m.members.len());
        println!("  smart \"{label}\" {t:8.1} ms   ({k} members)");
    }

    // One frame of the rail, one of the Library grid — the per-frame cost.
    let rail = frame_ms(&ctx, 5, |ui| {
        crate::collections_panel::show(&mut app, ui, &ctx)
    });
    let grid = frame_ms(&ctx, 5, |ui| crate::library::show(&mut app, ui, &ctx));
    println!("  per frame: rail {rail:6.2} ms   grid {grid:6.2} ms");
}

#[test]
#[ignore]
fn catalog_scale() {
    for n in [10_000, 30_000, 100_000] {
        one_size(n);
    }
}
