//! Compare two directories of golden references and report how far each one moved.
//!
//! ⚠ Written when Catmull-Rom replaced bilinear (M7.3) and **kept**, because this project's
//! standing rule is that *a golden nobody has looked at is just a hash* — and "looked at" has
//! to mean measured, since a two-level shift is invisible to the eye and a clipped reference is
//! invisible in both directions. Any future change that legitimately moves a reference should
//! run this rather than regenerating blind.
//!
//! Usage: `cargo run --release -p pipeline --example golden_diff <before-dir> <after-dir>
//! [name.png] [row]`
//!
//! With a name and a row it prints that scanline's green channel from both sides instead of the
//! summary — which is what turns "it moved by a mean of 3.4" into a statement about *how*.
//!
//! ⚠ Uses `image`, which is a **dev**-dependency of this crate. Examples may use those; the
//! library itself must not, and does not.

use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let (before, after) = match (args.next(), args.next()) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            eprintln!("usage: golden_diff <before-dir> <after-dir>");
            std::process::exit(2);
        }
    };

    if let (Some(name), Some(row)) = (args.next(), args.next()) {
        let row: u32 = row.parse().expect("row");
        let (w, _, a) = load(Path::new(&before).join(&name)).expect("before");
        let (_, _, b) = load(Path::new(&after).join(&name)).expect("after");
        let line = |p: &[u8]| -> Vec<u8> {
            (0..w)
                .map(|x| p[((row * w + x) * 3 + 1) as usize])
                .collect()
        };
        println!("{name} row {row}, green channel");
        println!("before: {:?}", line(&a));
        println!("after:  {:?}", line(&b));
        return;
    }

    let mut names: Vec<String> = std::fs::read_dir(&after)
        .expect("after dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();

    println!(
        "{:<34} {:>6} {:>8} {:>8} {:>7}",
        "golden", "max", "mean", ">2/255", "clipped"
    );
    let mut moved = 0usize;
    for name in &names {
        let a = load(Path::new(&before).join(name));
        let b = load(Path::new(&after).join(name));
        let (Some((aw, ah, ap)), Some((bw, bh, bp))) = (a, b) else {
            println!("{name:<34}   (missing on one side)");
            continue;
        };
        if (aw, ah) != (bw, bh) {
            println!("{name:<34}   dimensions {aw}x{ah} -> {bw}x{bh}");
            moved += 1;
            continue;
        }
        let mut max = 0u32;
        let mut sum = 0u64;
        let mut over2 = 0usize;
        for (x, y) in ap.iter().zip(bp.iter()) {
            let d = (*x as i32 - *y as i32).unsigned_abs();
            max = max.max(d);
            sum += d as u64;
            if d > 2 {
                over2 += 1;
            }
        }
        if max == 0 {
            continue;
        }
        moved += 1;
        // ⚠ Clipping is reported because a reference pinned at 0 or 255 cannot discriminate a
        // change from a larger one — the weakness two of this project's goldens have shipped.
        let clipped = bp.iter().filter(|v| **v == 0 || **v == 255).count();
        println!(
            "{name:<34} {max:>6} {:>8.3} {:>7.1}% {:>6.1}%",
            sum as f64 / ap.len() as f64,
            100.0 * over2 as f64 / ap.len() as f64,
            100.0 * clipped as f64 / bp.len() as f64,
        );
    }
    println!("\n{moved} of {} references moved", names.len());
}

fn load(p: std::path::PathBuf) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::open(p).ok()?.to_rgb8();
    Some((img.width(), img.height(), img.into_raw()))
}
