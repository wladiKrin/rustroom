//! Do the RAW and JPEG readers agree about the same shot?
//!
//! ⚠⚠ The check the whole normalization design rests on. Before it, one body appeared as two
//! cameras ("Fujifilm X-T4" vs "FUJIFILM X-T4"), the lens came back padded with 51 empty strings,
//! and the RAW carried a timezone the JPEG did not.
//!
//! `cargo run --release -p app --example pair_check <file.RAF> <file.JPG>`
fn main() {
    let raf = std::env::args().nth(1).unwrap();
    let jpg = std::env::args().nth(2).unwrap();
    let a = raw_decode::read_capture(std::path::Path::new(&raf)).unwrap();
    let b = image_decode::read_capture(std::path::Path::new(&jpg)).unwrap();
    println!("RAW  = {a:?}");
    println!("JPEG = {b:?}");
    println!();
    println!(
        "camera agrees : {}  ({:?} vs {:?})",
        a.camera == b.camera,
        a.camera,
        b.camera
    );
    println!(
        "taken  agrees : {}  ({:?} vs {:?})",
        a.taken == b.taken,
        a.taken,
        b.taken
    );
    println!(
        "lens   agrees : {}  ({:?} vs {:?})",
        a.lens == b.lens,
        a.lens,
        b.lens
    );
    println!("iso    agrees : {}", a.iso == b.iso);
}
