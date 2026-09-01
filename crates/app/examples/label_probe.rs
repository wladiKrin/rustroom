//! Print what the M6.0 read rule makes of each copy in a folder's sidecars.
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: label_probe <dir>"));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("readable dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    for j in files {
        let photo = j.with_extension("");
        let s = rustroom_app::sidecar::load_all(&photo);
        for c in &s.copies {
            println!(
                "{}  copy {}  label = {:?}",
                photo.file_name().unwrap().to_string_lossy(),
                c.copy,
                c.label
            );
        }
    }
}
