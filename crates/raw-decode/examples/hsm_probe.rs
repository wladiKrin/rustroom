//! How much does the HueSatMap actually change this image?
use raw_decode::{decode_raw, DecodeOptions};
use std::path::{Path, PathBuf};

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = PathBuf::from(a.next().expect("usage: hsm_probe <raw> <dcp>"));
    let dcp = PathBuf::from(a.next().expect("usage: hsm_probe <raw> <dcp>"));

    let p = camera_profile::CameraProfile::from_file(&dcp).expect("profile");
    let m = p.hue_sat_map.as_ref().expect("hue sat map present");
    println!("table dims {:?}, data2 {}", m.dims, m.data2.is_some());

    // The image as the profile path renders it.
    let img = decode_raw(
        Path::new(&raw),
        &DecodeOptions {
            half_size: true,
            camera_profile: Some(dcp),
            ..DecodeOptions::linear_scene()
        },
    )
    .expect("decode");

    // Saturation distribution: the table is the identity at s = 0, so a desaturated image is
    // barely touched however dramatic the table looks.
    let mut buckets = [0usize; 5];
    let mut n = 0usize;
    for y in 0..img.image.height() {
        for x in 0..img.image.width() {
            let s = color::huesatmap::rgb_to_hsv(img.image.pixel(x, y))[1];
            buckets[((s * 5.0) as usize).min(4)] += 1;
            n += 1;
        }
    }
    println!("saturation distribution of the rendered image:");
    for (i, b) in buckets.iter().enumerate() {
        println!(
            "  {:.1}–{:.1}: {:5.1}%",
            i as f32 / 5.0,
            (i + 1) as f32 / 5.0,
            100.0 * *b as f64 / n as f64
        );
    }

    // What the table does to a set of probe colours, at the illuminant-1 table.
    let t = color::huesatmap::Table {
        dims: m.dims,
        data: m.data1.clone(),
    };
    println!("table effect on probe colours:");
    for c in [
        [0.8, 0.2, 0.2],
        [0.2, 0.6, 0.3],
        [0.2, 0.3, 0.8],
        [0.6, 0.5, 0.4],
    ] {
        let before = color::huesatmap::rgb_to_hsv(c);
        let after = color::huesatmap::rgb_to_hsv(t.apply(c));
        println!(
            "  {c:?}: hue {:.1}->{:.1} ({:+.1}deg)  sat {:.3}->{:.3}",
            before[0],
            after[0],
            after[0] - before[0],
            before[1],
            after[1]
        );
    }
}
