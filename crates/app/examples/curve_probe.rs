//! Measure the camera's own rendering: scene-linear luminance in, camera JPEG luminance out.
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let path = std::path::Path::new(&path);
    let thumb = raw_decode::extract_thumbnail(path).expect("thumb");
    let d = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene()).expect("decode");
    let long = thumb.width.max(thumb.height);
    let small = d.image.downscale_to(long);

    // Bin by scene luminance; report the median camera-encoded luma in each bin.
    const NB: usize = 12;
    let edges: Vec<f32> = (0..=NB).map(|i| 0.002 * 2f32.powf(i as f32 * 0.85)).collect();
    let mut bins: Vec<Vec<f32>> = vec![Vec::new(); NB];
    let (tw, th) = (thumb.width, thumb.height);
    for y in 0..small.height().min(th) {
        for x in 0..small.width().min(tw) {
            let s = color::luminance(small.pixel(x, y));
            let i = (y * tw + x) * 4;
            let p = &thumb.rgba[i..i + 3];
            let c = color::luminance([
                color::srgb_to_linear(p[0] as f32 / 255.0),
                color::srgb_to_linear(p[1] as f32 / 255.0),
                color::srgb_to_linear(p[2] as f32 / 255.0),
            ]);
            if let Some(b) = edges.windows(2).position(|w| s >= w[0] && s < w[1]) {
                bins[b].push(c);
            }
        }
    }
    println!("{:>10}  {:>10}  {:>8}  {:>7}", "scene", "camera", "ratio", "n");
    for (b, v) in bins.iter_mut().enumerate() {
        if v.len() < 50 { continue; }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mid = (edges[b] * edges[b + 1]).sqrt();
        let cam = v[v.len() / 2];
        println!("{mid:10.4}  {cam:10.4}  {:8.2}x  {:7}", cam / mid, v.len());
    }
}
