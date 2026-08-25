//! Print libraw's colour data at `output_color` 0 and 4.
//!
//! `cargo run --release -p raw-decode --example color_probe <raw>`
use rsraw::RawImage;

fn main() {
    let path = std::env::args().nth(1).expect("usage: color_probe <raw>");
    for oc in [0i32, 4] {
        let bytes = std::fs::read(&path).expect("read");
        let mut raw = RawImage::open(&bytes).expect("open");
        raw.unpack().expect("unpack");
        raw.set_use_camera_wb(true);
        {
            let params = &mut raw.as_mut().params;
            params.output_color = oc;
            params.no_auto_bright = 1;
            params.gamm[0] = 1.0;
            params.gamm[1] = 1.0;
        }
        let _ = raw.process::<16>().expect("process");
        let c = &raw.as_mut().color;
        println!("=== output_color = {oc} ===");
        println!("cam_xyz  = {:?}", &c.cam_xyz[..3]);
        println!("rgb_cam  = {:?}", c.rgb_cam);
        println!("pre_mul  = {:?}", c.pre_mul);
        println!("cam_mul  = {:?}", c.cam_mul);
    }
}
