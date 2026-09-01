//! What a RAW's as-shot neutral actually is, in numbers.
fn main() {
    let p = std::env::args().nth(1).expect("usage: wb_probe <raw>");
    let bytes = std::fs::read(&p).expect("read");
    let raw = rsraw::RawImage::open(&bytes).expect("open");
    let mut raw = raw;
    let _ = raw.unpack();
    let _ = raw.process::<{ rsraw::BIT_DEPTH_16 }>();
    let c = &raw.as_mut().color;
    let mut rgb_cam = [[0.0f32; 3]; 3];
    for (i, r) in rgb_cam.iter_mut().enumerate() {
        r.copy_from_slice(&c.rgb_cam[i][..3]);
    }
    let cam_mul = [c.cam_mul[0], c.cam_mul[1], c.cam_mul[2], c.cam_mul[3]];
    println!("cam_mul  {cam_mul:?}");
    println!(
        "pre_mul  {:?}",
        [c.pre_mul[0], c.pre_mul[1], c.pre_mul[2], c.pre_mul[3]]
    );
    for (i, r) in rgb_cam.iter().enumerate() {
        println!("rgb_cam[{i}] {r:?}  sum {}", r.iter().sum::<f32>());
    }
    let mut cam_xyz = [[0.0f32; 3]; 3];
    for (i, r) in cam_xyz.iter_mut().enumerate() {
        r.copy_from_slice(&c.cam_xyz[i][..3]);
    }
    for (i, r) in cam_xyz.iter().enumerate() {
        println!("cam_xyz[{i}] {r:?}  sum {}", r.iter().sum::<f32>());
    }
    let neutral = [1.0 / cam_mul[0], 1.0 / cam_mul[1], 1.0 / cam_mul[2]];
    if let Some(inv) = color::camera::invert(&cam_xyz) {
        let x2 = color::spaces::apply(&inv, neutral);
        let s2: f32 = x2.iter().sum();
        let (ax, ay) = (x2[0] / s2, x2[1] / s2);
        let ai = color::wb::solve_xy((ax, ay));
        let (lx2, ly2) = color::wb::locus_xy(ai.kelvin);
        println!(
            "VIA cam_xyz: xy ({ax:.5}, {ay:.5}) -> {:.0}K tint {:.1}  off-locus {:.5}",
            ai.kelvin,
            ai.tint,
            ((ax - lx2).powi(2) + (ay - ly2).powi(2)).sqrt()
        );
    }
    let srgb = color::spaces::apply(&rgb_cam, neutral);
    let xyz = color::spaces::apply(&color::spaces::SRGB_TO_XYZ, srgb);
    let s: f32 = xyz.iter().sum();
    let (x, y) = (xyz[0] / s, xyz[1] / s);
    println!("neutral(cam) {neutral:?}\n  -> sRGB {srgb:?}\n  -> xy ({x:.5}, {y:.5})");
    let i = color::wb::solve_xy((x, y));
    let (lx, ly) = color::wb::locus_xy(i.kelvin);
    println!(
        "solve: {:.0}K tint {:.1}   locus at that K = ({lx:.5}, {ly:.5})",
        i.kelvin, i.tint
    );
    println!(
        "distance off locus = {:.5}",
        ((x - lx).powi(2) + (y - ly).powi(2)).sqrt()
    );
}
