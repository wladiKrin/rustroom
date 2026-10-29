//! Probe (M10.18c, found in use): the real face search on the files given — how many faces each has.
//! `cargo run --release -p app --example probe_faces_files -- <files…>`
use rustroom_app::face_pass::{real_finder, FacePass};
use std::time::Duration;

fn main() {
    let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let models = rustroom_app::ai_cache::models_dir().expect("a models folder");
    let p = FacePass::start(files.clone(), real_finder(models));
    let mut found = Vec::new();
    loop {
        let (f, done) = p.drain();
        found.extend(f);
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for (path, faces) in &found {
        println!(
            "{}: {:?} faces",
            path.display(),
            faces.as_ref().map(Vec::len)
        );
    }
    println!(
        "answered {} of {}, failures {:?}",
        found.len(),
        files.len(),
        p.failures()
    );
    // With PROBE_OUT set: each file's picture — the one the search sees — saved small, to look at.
    if let Ok(out) = std::env::var("PROBE_OUT") {
        for f in &files {
            if let Some(t) = rustroom_app::preview_loader::extract_camera(f) {
                let t = rustroom_app::preview_loader::cap_to(t, 900);
                let img =
                    image::RgbaImage::from_raw(t.width as u32, t.height as u32, t.rgba).unwrap();
                let name = f.file_stem().unwrap().to_string_lossy().to_string();
                image::DynamicImage::ImageRgba8(img)
                    .to_rgb8()
                    .save(format!("{out}/{name}.jpg"))
                    .unwrap();
            }
        }
    }
}
