//! Diagnostic: what does libraw's `user_qual` actually cost and produce on this file?
//!
//! Temporary. The demosaic-backend design depends on facts this prints — in particular
//! whether X-Trans honours the settings distinctly, since libraw routes X-Trans through
//! Markesteijn rather than the Bayer algorithms the numbers are usually quoted for.
//!
//! `cargo run --release -p raw-decode --example demosaic_probe <raw> <out-dir>`

use rsraw::{RawImage, BIT_DEPTH_16};
use std::time::Instant;

/// The settings worth comparing, with libraw's documented Bayer meaning.
const QUALS: [(i32, &str); 5] = [
    (0, "linear/bilinear"),
    (1, "VNG"),
    (2, "PPG"),
    (3, "AHD"),
    (4, "DCB"),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: demosaic_probe <raw> <out-dir>");
    let out = args.next().unwrap_or_else(|| ".".to_string());
    std::fs::create_dir_all(&out).unwrap();
    let bytes = std::fs::read(&path).expect("read raw");

    for (qual, name) in QUALS {
        let t0 = Instant::now();
        let mut raw = RawImage::open(&bytes).expect("open");
        raw.set_half_size(false);
        raw.unpack().expect("unpack");
        raw.set_use_camera_wb(true);
        {
            let params = &mut raw.as_mut().params;
            params.output_color = 1;
            params.no_auto_bright = 1;
            params.user_qual = qual;
        }
        let unpacked = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = Instant::now();
        let processed = raw.process::<{ BIT_DEPTH_16 }>().expect("process");
        let ms = t1.elapsed().as_secs_f64() * 1000.0;

        let (w, h) = (processed.width() as usize, processed.height() as usize);
        let data: &[u16] = &processed;

        // A 600x400 crop from the centre, where fine detail lives, written as 8-bit PNG.
        let (cw, ch) = (600usize.min(w), 400usize.min(h));
        let (x0, y0) = ((w - cw) / 2, (h - ch) / 2);
        let mut rgb = Vec::with_capacity(cw * ch * 3);
        for y in 0..ch {
            for x in 0..cw {
                let i = ((y0 + y) * w + (x0 + x)) * 3;
                for c in 0..3 {
                    // 16-bit linear -> 8-bit with a gamma so the crop is actually viewable.
                    let v = data[i + c] as f32 / 65535.0;
                    rgb.push((v.powf(1.0 / 2.2).clamp(0.0, 1.0) * 255.0) as u8);
                }
            }
        }
        let file = format!("{out}/qual{qual}_{}.png", name.replace('/', "-"));
        image::save_buffer(&file, &rgb, cw as u32, ch as u32, image::ColorType::Rgb8).unwrap();
        println!("user_qual={qual:2} {name:16} process {ms:8.0} ms  (open+unpack {unpacked:6.0} ms)  {w}x{h}  -> {file}");
    }
}
