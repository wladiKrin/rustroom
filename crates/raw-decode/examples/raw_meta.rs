fn main() {
    let path = std::env::args().nth(1).unwrap();
    let buf = std::fs::read(&path).unwrap();
    let raw = rsraw::RawImage::open(&buf).unwrap();
    println!("datetime          = {:?}", raw.datetime());
    println!("make/model        = {:?} / {:?}", raw.make(), raw.model());
    println!(
        "normalized        = {:?} / {:?}",
        raw.normalized_make(),
        raw.normalized_model()
    );
    let l = raw.lens_info();
    println!("lens_make/name    = {:?} / {:?}", l.lens_make, l.lens_name);
    println!(
        "iso/shutter/f/mm  = {} / {} / {} / {}",
        raw.iso_speed(),
        raw.shutter(),
        raw.aperture(),
        raw.focal_len()
    );
}
