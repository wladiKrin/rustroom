//! The app's icon in its window — the title bar, the taskbar, Alt-Tab (v0.1): the master's committed 256 px raster
//! (`packaging/icons/`), decoded for eframe's viewport. ⚠ No egui here: `main` wraps the pixels in `IconData`.

/// `packaging/icons/png/rustroom-256.png`, made from `rustroom.svg` by `packaging/icons/render.py`.
const PNG: &[u8] = include_bytes!("../../../packaging/icons/png/rustroom-256.png");

/// The icon as RGBA8 rows and its width and height.
pub fn rgba() -> (Vec<u8>, u32, u32) {
    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the committed icon is a PNG")
        .to_rgba8();
    let (w, h) = img.dimensions();
    (img.into_raw(), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed raster decodes at its size, and it is a picture: its corners are transparent (the rounded
    /// tile) and its centre is not.
    #[test]
    fn the_window_icon_is_the_256_pixel_raster() {
        let (px, w, h) = rgba();
        assert_eq!((w, h), (256, 256));
        assert_eq!(px.len(), 256 * 256 * 4);
        let alpha = |x: usize, y: usize| px[(y * 256 + x) * 4 + 3];
        assert_eq!(
            alpha(0, 0),
            0,
            "a rounded tile leaves its corner transparent"
        );
        assert_eq!(alpha(128, 128), 255, "the tile itself is opaque");
    }
}
