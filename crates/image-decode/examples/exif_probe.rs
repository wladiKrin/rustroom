//! What does kamadak-exif actually give us for a real JPEG?
fn main() {
    let path = std::env::args().nth(1).expect("usage: exif_probe <jpeg>");
    let file = std::fs::File::open(&path).unwrap();
    let mut r = std::io::BufReader::new(&file);
    let reader = exif::Reader::new();
    let ex = match reader.read_from_container(&mut r) {
        Ok(e) => e,
        Err(e) => {
            println!("no exif: {e}");
            return;
        }
    };
    for tag in [
        exif::Tag::DateTimeOriginal,
        exif::Tag::Make,
        exif::Tag::Model,
        exif::Tag::LensModel,
        exif::Tag::PhotographicSensitivity,
        exif::Tag::ExposureTime,
        exif::Tag::FNumber,
        exif::Tag::FocalLength,
    ] {
        match ex.get_field(tag, exif::In::PRIMARY) {
            Some(f) => println!(
                "{:<26} = {}",
                format!("{tag}"),
                f.display_value().with_unit(&ex)
            ),
            None => println!("{:<26} = <absent>", format!("{tag}")),
        }
    }
}
