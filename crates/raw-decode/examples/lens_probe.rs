//! What lens metadata does libraw actually report for a RAW file?
//!
//! `cargo run --release -p raw-decode --example lens_probe <raw>...`
fn main() {
    for path in std::env::args().skip(1) {
        let buf = std::fs::read(&path).expect("read");
        let raw = rsraw::RawImage::open(&buf).expect("open");
        let l = raw.lens_info();
        println!(
            "{}\n  camera : {} {}\n  lens   : make={:?} name={:?}\n  mount  : {:?}\n  focal  : {} mm  (range {}-{})\n  apert  : f/{}  focus_type={:?}",
            std::path::Path::new(&path).file_name().unwrap().to_string_lossy(),
            raw.make(), raw.model(),
            l.lens_make, l.lens_name, l.mounts,
            raw.focal_len(), l.min_focal, l.max_focal,
            raw.aperture(), l.focus_type,
        );
    }
}
