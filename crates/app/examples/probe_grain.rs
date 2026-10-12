//! Grain at a few amounts on a real photo (M10.11b), side by side at 1:1 — for judging `MAX_EV`.
//!
//! `cargo run --release -p app --example probe_grain -- <raw> <out.png> [cx cy]`
//! A 400 × 300 crop at full resolution, centred at `cx cy` (fractions, default the middle), rendered
//! at Amount 0, 25, 50 and 100 with the default Size and Roughness and the built-in rendering.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let decoded = raw_decode::decode_raw(
        std::path::Path::new(&args[0]),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
    let img = decoded.image;
    let (cx, cy) = match args.get(2..4) {
        Some([x, y]) => (x.parse::<f32>().unwrap(), y.parse::<f32>().unwrap()),
        _ => (0.5, 0.5),
    };
    let (cw, ch) = (400usize, 300usize);
    let x0 = ((img.width() as f32 * cx) as usize)
        .saturating_sub(cw / 2)
        .min(img.width() - cw);
    let y0 = ((img.height() as f32 * cy) as usize)
        .saturating_sub(ch / 2)
        .min(img.height() - ch);
    let crop = img.crop(x0, y0, cw, ch);
    let amounts = [0.0f32, 25.0, 50.0, 100.0];
    let mut strip = vec![0u8; cw * amounts.len() * ch * 3];
    for (i, a) in amounts.iter().enumerate() {
        let edits = edit_model::Edits {
            exposure: 0.7,
            rendering: Some(edit_model::Rendering::Camera),
            grain: edit_model::Grain {
                amount: *a,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = pipeline::build_pipeline(
            &edits,
            pipeline::RenderScale::FULL,
            pipeline::TailRendering::default(),
        )
        .run(crop.clone());
        let rgb = out.to_rgb8();
        for y in 0..ch {
            let dst = (y * cw * amounts.len() + i * cw) * 3;
            strip[dst..dst + cw * 3].copy_from_slice(&rgb[y * cw * 3..(y + 1) * cw * 3]);
        }
    }
    image::save_buffer(
        &args[1],
        &strip,
        (cw * amounts.len()) as u32,
        ch as u32,
        image::ColorType::Rgb8,
    )
    .unwrap();
    println!("crop at ({x0},{y0}); amounts {amounts:?}, left to right");
}
