//! Compare the thumbnail of a RAW against the thumbnail of its JPEG sibling.
//!
//! Both are supposed to be *the camera's own rendering* — the RAF's embedded preview and the
//! standalone JPEG — so they should look alike. This says whether they do.
//!
//! `cargo run --release -p app --example thumb_pair <raw> <jpg>`
fn stats(t: &core_image::Thumbnail, name: &str) {
    let mut sat = 0.0f64;
    let mut mean = [0.0f64; 3];
    let n = (t.rgba.len() / 4) as f64;
    for px in t.rgba.as_chunks::<4>().0 {
        let (r, g, b) = (px[0] as f64, px[1] as f64, px[2] as f64);
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        if mx > 0.0 {
            sat += (mx - mn) / mx;
        }
        mean[0] += r;
        mean[1] += g;
        mean[2] += b;
    }
    println!(
        "{name:>28}  {}x{}  mean RGB {:5.1} {:5.1} {:5.1}   mean saturation {:.4}",
        t.width,
        t.height,
        mean[0] / n,
        mean[1] / n,
        mean[2] / n,
        sat / n
    );
}

fn main() {
    let raw = std::env::args()
        .nth(1)
        .expect("usage: thumb_pair <raw> <jpg>");
    let jpg = std::env::args()
        .nth(2)
        .expect("usage: thumb_pair <raw> <jpg>");

    let a = raw_decode::extract_thumbnail(std::path::Path::new(&raw)).expect("raw thumbnail");
    stats(&a, "RAF embedded preview");

    let b = image_decode::thumbnail(std::path::Path::new(&jpg)).expect("jpeg thumbnail");
    stats(&b, "JPEG file thumbnail");
}
