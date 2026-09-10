//! M8.4's measurement: what does the demosaic tier actually change, **on foliage**?
//!
//! ⚠⚠ **The `Fast` default was validated on the wrong content.** It was chosen by comparing
//! *hair-strand separation* against `Best` on a kitten frame — and X-Trans demosaic does not fail
//! on hair. It fails on **foliage and fine repeating texture**, with the "worms" artefact, which
//! is why the roadmap's M8.4 asks for a foliage comparison before the default is treated as
//! settled.
//!
//! ⚠⚠ **A whole-frame mean is the wrong statistic here, and this project has been misled by that
//! exact mistake before**: the dehaze probe ranked frames by how much the stage *moved* them and
//! found flat lilac walls rather than haze. Most of any frame is sky, skin and render, where every
//! tier agrees — so a global mean divides the real difference by the area that cannot show it.
//! This restricts the comparison to a hand-picked **stress** region per frame, and reports a
//! **control** region beside it: the control should be near zero, and if it is not, the tiers are
//! differing for some reason other than the detail under test.
//!
//! ⚠⚠ **Measured and cropped on the RENDERED frame, not the decode.** The first version compared
//! `linear_to_srgb` of the scene-linear decode while saving crops that were the same buffer
//! quantized with a bare `×255` — so the numbers and the pictures were in two different spaces,
//! and the pictures were in neither: near-black, with the artefact invisible. Both halves now run
//! the decode through the real pipeline at default edits, which is where a judgement about a
//! photograph is actually made, and the pipeline's own output is already sRGB-encoded so the
//! comparison is a plain `×255`. Same reasoning the Whites slider's weights were corrected under.
//!
//! `cargo run --release -p app --example demosaic_foliage <raw>...`

use core_image::ImageBuf;
use raw_decode::{DecodeOptions, Demosaic};
use std::time::Instant;

/// Where to look on each frame, as fractions of the frame, chosen **by eye from the thumbnails**.
///
/// ⚠⚠ **The automatic "busiest window" picker was WRONG and is deleted, not tuned.** It scored by
/// local variance and landed on the road sign's **lettering** on DSCF6291 — black text on white
/// has far more local variance than any leaf. But demosaic does not fail on hard edges; it fails
/// on dense fine *repeating* texture, which is only middling-variance. That is the same mistake
/// `dehaze_bright_probe` made by ranking frames on how much the stage *moved* them and finding
/// flat lilac walls: **"most extreme" is not "the content under test"**, and a metric cannot be
/// talked into knowing the difference.
///
/// ⚠ Each frame contributes a **stress** region and a **control** region, so every number has
/// something beside it that ought not to move.
struct Region {
    stem: &'static str,
    stress: (f32, f32),
    stress_what: &'static str,
    control: (f32, f32),
    control_what: &'static str,
}

const REGIONS: [Region; 4] = [
    Region {
        stem: "DSCF6291",
        stress: (0.62, 0.78),
        stress_what: "bushes, dense leaves",
        control: (0.50, 0.34),
        control_what: "sign face, flat white",
    },
    Region {
        stem: "DSCF6226",
        stress: (0.14, 0.63),
        stress_what: "lattice bridge + treeline at distance",
        control: (0.45, 0.10),
        control_what: "sky",
    },
    Region {
        stem: "DSCF6377",
        stress: (0.30, 0.80),
        stress_what: "sunlit grass, raking angle",
        control: (0.35, 0.08),
        control_what: "sky",
    },
    Region {
        stem: "DSCF6400",
        stress: (0.22, 0.14),
        stress_what: "hanging birch foliage",
        control: (0.72, 0.30),
        control_what: "rendered wall",
    },
];

/// Run one decode through the real pipeline at default edits.
///
/// ⚠⚠ **The comparison belongs here, not on the decode.** A scene-linear buffer is not what
/// anyone looks at: the worms artefact lives in shadowed foliage, and the default rendering lifts
/// exactly that range. Comparing before the lift measures the artefact where it is smallest and
/// then shows it in a picture too dark to read.
///
/// ⚠ `as_shot` is passed for the same reason the CLI passes it: without the decode's own
/// white-balance baseline the render is not the one the GUI would produce. It is constant across
/// tiers either way, but a probe that renders differently from the app answers a different
/// question than the one asked.
fn render(decoded: raw_decode::DecodedRaw) -> ImageBuf {
    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        &edit_model::Edits::default(),
        pipeline::RenderScale::FULL,
        None,
        None,
        pipeline::RenderSpec {
            baseline: decoded.metadata.as_shot.unwrap_or_default(),
            ..pipeline::RenderSpec::default()
        },
        None,
        pipeline::ProfileRendering::default(),
    );
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    pipeline::Pipeline::new(stages).run(decoded.image)
}

/// The difference between two renders inside one square window, in 8-bit levels.
///
/// ⚠ A plain `×255`: the buffers reaching here have already been through the pipeline's
/// output transform, so they are sRGB-encoded. Applying `linear_to_srgb` again would be a second
/// encoding, which is what the first version of this probe did to the decode.
fn window_diff(a: &ImageBuf, b: &ImageBuf, at: (usize, usize), size: usize) -> (f64, f64, f64) {
    let (mut sum, mut n, mut moved) = (0.0f64, 0.0f64, 0.0f64);
    let mut ds: Vec<f32> = Vec::new();
    for y in at.1..(at.1 + size).min(a.height()) {
        for x in at.0..(at.0 + size).min(a.width()) {
            let (pa, pb) = (a.pixel(x, y), b.pixel(x, y));
            let d = (0..3)
                .map(|c| ((pa[c] - pb[c]) * 255.0).abs())
                .fold(0.0f32, f32::max);
            sum += d as f64;
            n += 1.0;
            ds.push(d);
            if d > 1.0 {
                moved += 1.0;
            }
        }
    }
    ds.sort_by(|p, q| p.partial_cmp(q).unwrap());
    let p99 = ds
        .get(((ds.len() as f64 * 0.99) as usize).saturating_sub(1))
        .copied()
        .unwrap_or(0.0) as f64;
    (sum / n.max(1.0), p99, moved / n.max(1.0) * 100.0)
}

/// A window's top-left, from a centre given as a fraction of the frame.
fn window_at(c: (f32, f32), w: usize, h: usize, size: usize) -> (usize, usize) {
    let x = ((c.0 * w as f32) as usize).saturating_sub(size / 2);
    let y = ((c.1 * h as f32) as usize).saturating_sub(size / 2);
    (x.min(w.saturating_sub(size)), y.min(h.saturating_sub(size)))
}

/// ⚠ **PNG, not JPEG.** The artefact under test is a few pixels across and low in amplitude,
/// which is precisely what a JPEG quantiser discards — saving the comparison lossily would let the
/// encoder answer the question instead of the demosaic.
fn save_crop(img: &ImageBuf, at: (usize, usize), size: usize, path: &str) {
    let c = img.crop(at.0, at.1, size, size);
    let bytes = export::to_rgb8(&c);
    image::save_buffer(
        path,
        &bytes,
        c.width() as u32,
        c.height() as u32,
        image::ColorType::Rgb8,
    )
    .expect("save");
}

/// Side-by-side of every tier over one small window, magnified **nearest-neighbour**.
///
/// ⚠⚠ At 1:1 a 420-pixel crop of foliage is unreadable — every tier looks like foliage, which is
/// the trap of "I compared them and saw nothing". Worms are a per-pixel artefact, so the
/// comparison has to be shown per pixel: a small window blown up with no interpolation, tiers
/// abutting so the eye can do the differencing it is good at.
///
/// ⚠ Nearest-neighbour on purpose. Any smooth kernel would blur the artefact away in the act of
/// displaying it — the same reason the tint's sigma is fixed in rendered pixels.
fn save_montage(imgs: &[(Demosaic, ImageBuf, f64)], at: (usize, usize), win: usize, path: &str) {
    const ZOOM: usize = 4;
    const GAP: usize = 8;
    let n = imgs.len();
    let (cw, ch) = (win * ZOOM, win * ZOOM);
    let w = n * cw + (n - 1) * GAP;
    let mut out = vec![32u8; w * ch * 3];
    for (k, (_, img, _)) in imgs.iter().enumerate() {
        let x0 = k * (cw + GAP);
        for y in 0..ch {
            for x in 0..cw {
                let p = img.pixel(at.0 + x / ZOOM, at.1 + y / ZOOM);
                let o = (y * w + x0 + x) * 3;
                for c in 0..3 {
                    out[o + c] = (p[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
    }
    image::save_buffer(path, &out, w as u32, ch as u32, image::ColorType::Rgb8).expect("save");
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: demosaic_foliage <raw>...");
    std::fs::create_dir_all("/tmp/m84/crops").ok();
    const CROP: usize = 420;

    for path in &files {
        let stem = std::path::Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let Some(r) = REGIONS.iter().find(|r| r.stem == stem) else {
            println!("\n=== {stem} === (no hand-picked region; skipped)");
            continue;
        };
        println!("\n=== {stem} ===");

        let mut decoded: Vec<(Demosaic, ImageBuf, f64)> = Vec::new();
        for tier in Demosaic::ALL {
            let mut o = DecodeOptions::linear_scene();
            o.demosaic = tier;
            let t = Instant::now();
            let d = raw_decode::decode_raw(std::path::Path::new(path), &o).expect("decode");
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let img = render(d);
            println!(
                "  {tier:?} decoded in {ms:8.0} ms (+ render {:.0} ms)",
                t.elapsed().as_secs_f64() * 1000.0 - ms
            );
            decoded.push((tier, img, ms));
        }

        let best = &decoded.last().unwrap().1;
        let (w, h) = (best.width(), best.height());
        let stress = window_at(r.stress, w, h, CROP);
        let control = window_at(r.control, w, h, CROP);
        println!(
            "  {w}x{h}   stress @{:?} ({})   control @{:?} ({})",
            stress, r.stress_what, control, r.control_what
        );
        println!("  vs Best      STRESS mean   p99    >1 level  |  CONTROL mean  >1 level");
        for (tier, img, _) in &decoded {
            let (sm, sp, smv) = window_diff(img, best, stress, CROP);
            let (cm, _, cmv) = window_diff(img, best, control, CROP);
            println!("  {tier:<8?}   {sm:9.3} {sp:7.3}  {smv:7.2}%  |  {cm:9.3}  {cmv:7.2}%");
            save_crop(
                img,
                stress,
                CROP,
                &format!("/tmp/m84/crops/{stem}_{tier:?}.png"),
            );
        }
        // ⚠ Centred inside the stress window, so the montage shows the same content the numbers
        // above were computed over rather than a second, unrelated pick.
        let mid = (stress.0 + CROP / 2 - 55, stress.1 + CROP / 2 - 55);
        save_montage(
            &decoded,
            mid,
            110,
            &format!("/tmp/m84/crops/{stem}_montage.png"),
        );
    }
    println!("\ncrops in /tmp/m84/crops (1:1, rendered at default edits, hand-picked windows)");
}
