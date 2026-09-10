//! M8.2's measurement: what would an on-disk thumbnail cache actually save, and what would it cost?
//!
//! ⚠ Written **before** the design, because the two open questions — what to store, and in what
//! format — turn entirely on numbers rather than on argument. This project has corrected seven
//! cost estimates by measuring and none by reasoning.
//!
//! `cargo run --release -p app --example bench_thumb_cache <folder> [n]`

use image::ImageEncoder;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut a = std::env::args().skip(1);
    let dir = a.next().expect("usage: bench_thumb_cache <folder> [n]");
    let n: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(40);

    let entries = catalog::scan_folder(std::path::Path::new(&dir)).expect("scan");
    let total = entries.len();
    let sample: Vec<_> = entries.iter().take(n).collect();
    println!("{total} entries in the folder; sampling {}", sample.len());

    let (mut raws, mut others) = (0usize, 0usize);
    let (mut extract_ms, mut px_bytes) = (0.0f64, 0usize);
    let (mut raw_ms, mut other_ms) = (0.0f64, 0.0f64);
    // ⚠ The render half is deliberately NOT measured here. `bench_thumb_render` already
    // measures it against the real `load_thumbnail`, and reaching it from a probe would mean
    // either widening production API for a benchmark or copying the kernel — and a copied
    // kernel is what went stale in `bench_hsl`.
    let mut jpeg_bytes = 0usize;
    let mut png_bytes = 0usize;
    let (mut enc_jpeg_ms, mut dec_jpeg_ms) = (0.0f64, 0.0f64);
    let (mut enc_png_ms, mut dec_png_ms) = (0.0f64, 0.0f64);
    let mut dims: Vec<(usize, usize)> = Vec::new();

    for e in &sample {
        let p = e.path.as_path();
        let is_raw = catalog::is_raw(p);

        // ── 1. what the cache would REPLACE: extracting the embedded preview ────────────
        let t = Instant::now();
        let thumb = if is_raw {
            raw_decode::extract_thumbnail(p).ok()
        } else {
            image_decode::thumbnail(p).ok()
        };
        let e_ms = ms(t);
        let Some(thumb) = thumb else { continue };
        extract_ms += e_ms;
        if is_raw {
            raws += 1;
            raw_ms += e_ms;
        } else {
            others += 1;
            other_ms += e_ms;
        }
        px_bytes += thumb.rgba.len();
        dims.push((thumb.width, thumb.height));

        let (w, h) = (thumb.width as u32, thumb.height as u32);

        // ── 2. what the cache would COST to write and read, per candidate format ────────
        // ⚠ RGBA, not RGB: `Thumbnail.rgba` is what the UI uploads as a texture, so a format
        // that cannot carry the alpha channel has to reconstruct it on read. JPEG cannot, so
        // it is encoded from RGB and read back with alpha filled — which is sound only because
        // every thumbnail here is opaque, and the probe asserts that rather than assuming it.
        assert!(
            thumb.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "a thumbnail carried real alpha; the JPEG option would lose it"
        );
        let rgb: Vec<u8> = thumb
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();

        let t = Instant::now();
        let mut jbuf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jbuf, 90)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8)
            .expect("jpeg encode");
        enc_jpeg_ms += ms(t);
        jpeg_bytes += jbuf.len();

        let t = Instant::now();
        let _ = image::load_from_memory_with_format(&jbuf, image::ImageFormat::Jpeg)
            .expect("jpeg decode")
            .to_rgba8();
        dec_jpeg_ms += ms(t);

        let t = Instant::now();
        let mut pbuf = std::io::Cursor::new(Vec::new());
        image::codecs::png::PngEncoder::new(&mut pbuf)
            .write_image(&thumb.rgba, w, h, image::ExtendedColorType::Rgba8)
            .expect("png encode");
        enc_png_ms += ms(t);
        let pbuf = pbuf.into_inner();
        png_bytes += pbuf.len();

        let t = Instant::now();
        let _ = image::load_from_memory_with_format(&pbuf, image::ImageFormat::Png)
            .expect("png decode")
            .to_rgba8();
        dec_png_ms += ms(t);
    }

    let k = (raws + others) as f64;
    assert!(k > 0.0, "no thumbnail could be extracted from that folder");
    let biggest = dims.iter().map(|d| d.0.max(d.1)).max().unwrap_or(0);

    println!("\n{raws} RAW + {others} other, long edge at most {biggest} px");
    println!("\n── per thumbnail, mean ────────────────────────────────");
    println!("  extract (what a cache removes)  {:8.1} ms", extract_ms / k);
    if raws > 0 {
        println!("    of which RAW                  {:8.1} ms", raw_ms / raws as f64);
    }
    if others > 0 {
        println!("    of which JPEG/PNG/TIFF        {:8.1} ms", other_ms / others as f64);
    }
    println!("  raw RGBA in memory              {:8.1} KB", px_bytes as f64 / k / 1024.0);
    println!("\n── candidate on-disk formats ──────────────────────────");
    println!(
        "  JPEG q90   {:7.1} KB   write {:6.2} ms   read {:6.2} ms",
        jpeg_bytes as f64 / k / 1024.0,
        enc_jpeg_ms / k,
        dec_jpeg_ms / k
    );
    println!(
        "  PNG        {:7.1} KB   write {:6.2} ms   read {:6.2} ms",
        png_bytes as f64 / k / 1024.0,
        enc_png_ms / k,
        dec_png_ms / k
    );
    println!(
        "  raw RGBA   {:7.1} KB   write     ~0 ms   read     ~0 ms  (a plain file copy)",
        px_bytes as f64 / k / 1024.0
    );

    println!("\n── extrapolated to this folder's {total} entries ──────────");
    let per = extract_ms / k;
    println!("  cold open, 6 workers   ~{:6.1} s", per * total as f64 / 6.0 / 1000.0);
    for (name, bytes, read) in [
        ("JPEG q90", jpeg_bytes as f64 / k, dec_jpeg_ms / k),
        ("PNG", png_bytes as f64 / k, dec_png_ms / k),
        ("raw RGBA", px_bytes as f64 / k, 0.0),
    ] {
        println!(
            "  {name:<9}  on disk {:7.1} MB   warm open ~{:5.2} s",
            bytes * total as f64 / 1024.0 / 1024.0,
            read * total as f64 / 6.0 / 1000.0
        );
    }
}
