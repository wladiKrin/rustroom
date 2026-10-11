//! The images an export's watermark draws, kept as copies (M10.10a).
//!
//! ⚠⚠ **A COPY, NOT THE CHOSEN FILE'S PATH — the author's choice.** A path breaks the moment the
//! logo is moved, renamed or on a disconnected drive, and an export preset would carry this
//! machine's layout to another. Choosing an image copies it into `<config>/rustroom/watermarks/`,
//! and the settings name the copy by its bare file name ([`export::Watermark::image`]).
//!
//! ⚠ A different file of a name already kept is numbered (`logo-1.png`) rather than replacing the
//! first, which another preset may still name; the same file chosen again is the copy it already
//! has, so choosing it twice does not pile up duplicates.

use export::{Watermark, WatermarkKind};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The file types the picker offers and the store takes — what `image` decodes in this build.
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "tif", "tiff", "webp"];

pub struct WatermarkStore {
    dir: PathBuf,
}

impl WatermarkStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<config>/rustroom/watermarks`, beside the looks and the presets.
    pub fn user_default(config_dir: &Path) -> Self {
        Self::new(config_dir.join("rustroom").join("watermarks"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Keep a copy of `file`, returning the name it is kept under.
    pub fn add(&self, file: &Path) -> io::Result<String> {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "not a PNG, JPEG, TIFF or WebP file",
                )
            })?;
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("watermark");
        let bytes = fs::read(file)?;
        fs::create_dir_all(&self.dir)?;
        let mut n = 0;
        loop {
            let name = if n == 0 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}-{n}.{ext}")
            };
            let at = self.dir.join(&name);
            match fs::read(&at) {
                Ok(there) if there == bytes => return Ok(name),
                Ok(_) => n += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::write(&at, &bytes)?;
                    return Ok(name);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Where the copy named `name` is — `None` unless `name` is one plain file name.
    ///
    /// ⚠⚠ **The only way a name becomes a path.** A settings file or a preset can be edited by
    /// hand, so `../../x.png` or an absolute path must find nothing rather than reach outside the
    /// store; the export then reports the image unreadable, as for a missing copy.
    pub fn path_of(&self, name: &str) -> Option<PathBuf> {
        if name.trim().is_empty() {
            return None;
        }
        let mut parts = Path::new(name).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(_)), None) => Some(self.dir.join(name)),
            _ => None,
        }
    }

    /// Keep a copy of `file` and make `mark` an image mark drawing it. The text is left alone,
    /// for switching back.
    pub fn adopt(&self, file: &Path, mark: &mut Watermark) -> io::Result<()> {
        mark.image = self.add(file)?;
        mark.kind = WatermarkKind::Image;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use export::{Watermark, WatermarkKind};
    use test_dir::TempDir;

    fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        fs::create_dir_all(dir).expect("source dir");
        let p = dir.join(name);
        fs::write(&p, bytes).expect("source file");
        p
    }

    fn kept(store: &WatermarkStore) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(store.dir())
            .expect("the store")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_chosen_file_is_kept_as_a_copy() {
        let d = TempDir::new("wm-copy");
        let src = file(d.path(), "logo.png", b"one");
        let store = WatermarkStore::new(d.path().join("store"));
        let name = store.add(&src).expect("add");
        assert_eq!(name, "logo.png");
        // ⚠ The point of a copy: the original can go and the mark does not.
        fs::remove_file(&src).expect("remove the original");
        let at = store.path_of(&name).expect("a plain name");
        assert_eq!(fs::read(at).expect("the copy"), b"one");
    }

    #[test]
    fn a_different_file_of_the_same_name_is_numbered() {
        let d = TempDir::new("wm-clash");
        let store = WatermarkStore::new(d.path().join("store"));
        let a = file(&d.path().join("a"), "logo.png", b"one");
        let b = file(&d.path().join("b"), "logo.png", b"two");
        let c = file(&d.path().join("c"), "logo.png", b"three");
        assert_eq!(store.add(&a).expect("a"), "logo.png");
        assert_eq!(store.add(&b).expect("b"), "logo-1.png");
        assert_eq!(store.add(&c).expect("c"), "logo-2.png");
        for (name, bytes) in [
            ("logo.png", &b"one"[..]),
            ("logo-1.png", b"two"),
            ("logo-2.png", b"three"),
        ] {
            let at = store.path_of(name).expect("a plain name");
            assert_eq!(fs::read(at).expect(name), bytes, "{name} was replaced");
        }
    }

    #[test]
    fn the_same_file_again_is_the_same_copy() {
        let d = TempDir::new("wm-same");
        let store = WatermarkStore::new(d.path().join("store"));
        let a = file(&d.path().join("a"), "logo.png", b"one");
        let b = file(&d.path().join("b"), "logo.png", b"two");
        assert_eq!(store.add(&a).expect("a"), "logo.png");
        assert_eq!(store.add(&a).expect("a again"), "logo.png");
        assert_eq!(store.add(&b).expect("b"), "logo-1.png");
        // ⚠ Found among the numbered ones too, not only under the bare name.
        assert_eq!(store.add(&b).expect("b again"), "logo-1.png");
        assert_eq!(kept(&store), ["logo-1.png", "logo.png"]);
    }

    #[test]
    fn only_image_files_are_taken() {
        let d = TempDir::new("wm-kind");
        let store = WatermarkStore::new(d.path().join("store"));
        let txt = file(d.path(), "notes.txt", b"words");
        let e = store.add(&txt).expect_err("a text file");
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        let upper = file(d.path(), "LOGO.PNG", b"one");
        assert_eq!(store.add(&upper).expect("upper case"), "LOGO.PNG");
    }

    #[test]
    fn a_name_that_is_not_one_plain_file_name_finds_nothing() {
        let d = TempDir::new("wm-names");
        let dir = d.path().join("store");
        let store = WatermarkStore::new(dir.clone());
        // ⚠ The positive control first: a plain name is found, in the store and nowhere else.
        assert_eq!(store.path_of("logo.png"), Some(dir.join("logo.png")));
        for bad in ["", "  ", "../x.png", "/etc/x.png", "a/b.png", "..", "."] {
            assert_eq!(store.path_of(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn adopting_sets_the_name_and_the_kind() {
        let d = TempDir::new("wm-adopt");
        let store = WatermarkStore::new(d.path().join("store"));
        let src = file(d.path(), "logo.png", b"one");
        let mut m = Watermark {
            text: "© me".into(),
            ..Watermark::default()
        };
        store.adopt(&src, &mut m).expect("adopt");
        assert_eq!(m.kind, WatermarkKind::Image);
        assert_eq!(m.image, "logo.png");
        assert_eq!(m.text, "© me", "kept, for switching back to Text");
    }
}
