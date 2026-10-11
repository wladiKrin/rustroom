//! A scanned negative through the app's own film-base pick (M10.11a): what a click on the film base
//! measures, and the positive it gives.
//!
//! `PROBE_OUT=positive.png cargo run --release -p app --example probe_negative -- <scan> <x> <y>`
//! — `x y` in the file's own pixels, on the film base. Prints the base and the measured densities;
//! with `PROBE_OUT`, writes the positive (the negative stage alone, sRGB-encoded).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let img = image_decode::decode(std::path::Path::new(&args[0]))
        .expect("decode")
        .image;
    let (w, h) = (img.width(), img.height());
    let at = [
        args[1].parse::<f32>().unwrap() / w as f32,
        args[2].parse::<f32>().unwrap() / h as f32,
    ];
    let mut edits = edit_model::Edits::default();
    let picked = rustroom_app::negative_tool::pick(&mut edits, &img, at);
    let n = edits.negative;
    println!("{w}x{h} picked {picked}: base {:?}", n.base);
    println!("  low {:?}\n  high {:?}", n.low, n.high);
    if let Some(dest) = std::env::var_os("PROBE_OUT") {
        let out = img.map_pixels(|p| pipeline::negative::positive(p, &n));
        let rgb: Vec<u8> = out
            .data()
            .iter()
            .map(|&v| (color::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8)
            .collect();
        image::save_buffer(dest, &rgb, w as u32, h as u32, image::ColorType::Rgb8).unwrap();
    }
}
