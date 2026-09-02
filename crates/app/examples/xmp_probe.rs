//! Import a photo's XMP sidecar into a fresh catalog, change the rating, and write it back.
//!
//! Kept because it is how the milestone's "done when" was checked against a real Lightroom-shaped
//! sidecar and a real `exiftool`: it exercises the import, the catalog and the **merging** write
//! in one run, over files on disk rather than fixtures.
//!
//! ```text
//! cargo run --release -p app --example xmp_probe <photo.RAF>
//! ```

use rustroom_app::xmp_sync;
use std::path::PathBuf;

fn main() {
    let Some(photo) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: xmp_probe <photo.RAF>");
        std::process::exit(2);
    };
    let folder = photo.parent().unwrap().to_path_buf();

    let cat = catalog::db::Catalog::open_in_memory().expect("catalog");
    let scan = catalog::scan_folder(&folder).expect("scan");
    cat.sync_folder(&folder, &scan).expect("sync");

    let n = xmp_sync::import_newer(&cat, std::slice::from_ref(&photo));
    let m = cat.marks(&photo, 0).unwrap_or_default();
    println!();
    println!("=== 2. Rustroom imports it ===");
    println!("imported {n} sidecar(s)");
    println!("  rating  {}", m.rating);
    println!(
        "  label   {} ({})",
        m.label,
        xmp_sync::label_to_xmp(m.label).unwrap_or("none")
    );
    for path in xmp_sync::keyword_paths(&cat, &photo) {
        println!("  keyword {}", path.join(" > "));
    }

    // Rate it 5 here, as pressing `5` in the Library would, and write the sidecar back.
    cat.set_marks(&photo, 0, catalog::db::Marks { rating: 5, ..m })
        .expect("set marks");
    let k = cat.create_keyword("Street", None).expect("keyword");
    cat.assign_keyword(k, &photo).expect("assign");
    println!("  → rated 5 and tagged Street, writing back…");
    println!("  wrote: {}", xmp_sync::write_for(&cat, &photo));
}
