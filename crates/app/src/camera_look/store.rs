//! Where learned looks live: `<config>/rustroom/looks/camera/` — one `.cube` per body and film simulation,
//! and `manifest.json` saying which file is which, from how many photos, and when.
//!
//! ⚠ Plain files on purpose: a look the user dislikes can be deleted by hand, and Relearn empties the folder.
//! ⚠ In a SUBFOLDER of the looks folder, so the Look menu does not list them — applied twice (as Camera
//! matching and as a creative look) a film would be rendered twice.

use super::fit::Lattice;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// One learned look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Learned {
    /// The camera, as the catalog names it (`FUJIFILM X-T4`).
    pub body: String,
    /// The film simulation, as Fuji names it.
    pub film: String,
    /// The `.cube`'s file name in the store.
    pub file: String,
    /// How many photos it was fitted on, and how many plain photos the group had then.
    pub photos: usize,
    pub group: usize,
    /// Unix seconds.
    pub at: i64,
}

impl Learned {
    /// What the panel calls it: "Pro Neg. Std (X-T4)".
    pub fn label(&self) -> String {
        label(&self.body, &self.film)
    }
}

/// "Pro Neg. Std (X-T4)" — the film, then the body without its make.
pub fn label(body: &str, film: &str) -> String {
    let model = body.split_once(' ').map_or(body, |(_, m)| m);
    format!("{film} ({model})")
}

const MANIFEST: &str = "manifest.json";

pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The user's: `<config>/rustroom/looks/camera`.
    pub fn user_default(config_dir: &Path) -> Self {
        Self::new(config_dir.join("rustroom").join("looks").join("camera"))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every learned look — empty when none is, or the manifest cannot be read.
    pub fn manifest(&self) -> Vec<Learned> {
        std::fs::read(self.dir.join(MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// The look learned for `body` and `film`, when there is one and its file is here.
    pub fn learned(&self, body: &str, film: &str) -> Option<Learned> {
        self.manifest()
            .into_iter()
            .find(|l| l.body == body && l.film == film)
            .filter(|l| self.dir.join(&l.file).is_file())
    }

    /// Write `lattice` as the look for `body` and `film`, replacing any earlier one.
    pub fn save(
        &self,
        body: &str,
        film: &str,
        lattice: &Lattice,
        photos: usize,
        group: usize,
        at: i64,
    ) -> std::io::Result<Learned> {
        std::fs::create_dir_all(&self.dir)?;
        let label = label(body, film);
        let file = format!("{}.cube", crate::safe_name::sanitize(&label));
        write_atomically(&self.dir.join(&file), lattice.to_cube(&label).as_bytes())?;
        let learned = Learned {
            body: body.to_owned(),
            film: film.to_owned(),
            file,
            photos,
            group,
            at,
        };
        let mut all: Vec<Learned> = self
            .manifest()
            .into_iter()
            .filter(|l| !(l.body == body && l.film == film))
            .collect();
        all.push(learned.clone());
        all.sort_by(|a, b| (&a.body, &a.film).cmp(&(&b.body, &b.film)));
        let json = serde_json::to_vec_pretty(&all).map_err(std::io::Error::other)?;
        write_atomically(&self.dir.join(MANIFEST), &json)?;
        Ok(learned)
    }

    /// Forget every learned look — Settings' Relearn.
    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The parsed table of the look learned for `body` and `film`, read once per file and change.
    ///
    /// ⚠ Called while a photo is decoded, never per frame; the cache is keyed by path and modification time,
    /// so a relearned look is read again.
    pub fn cube(&self, body: &str, film: &str) -> Option<Arc<pipeline::lut::Cube>> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
        let path = self.dir.join(self.learned(body, film)?.file);
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        let cache = CACHE.get_or_init(Default::default);
        if let Some((t, c)) = cache.lock().ok()?.get(&path) {
            if *t == mtime {
                return Some(Arc::clone(c));
            }
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let cube = Arc::new(pipeline::lut::Cube::parse(&text).ok()?);
        cache.lock().ok()?.insert(path, (mtime, Arc::clone(&cube)));
        Some(cube)
    }
}

/// A parsed table and the modification time it was read at.
type Cached = (std::time::SystemTime, Arc<pipeline::lut::Cube>);

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_look_is_found_by_body_and_film_and_relearn_forgets_it() {
        let dir = test_dir::TempDir::new("camera-look-store");
        let s = Store::new(dir.path().join("camera"));
        assert!(s.manifest().is_empty());
        let lut = super::super::fit::fit(&[]);
        let l = s
            .save(
                "FUJIFILM X-T4",
                "Classic Chrome",
                &lut,
                30,
                52,
                1_700_000_000,
            )
            .unwrap();
        assert_eq!(l.label(), "Classic Chrome (X-T4)");
        assert_eq!(l.file, "Classic Chrome _X-T4_.cube");
        s.save("FUJIFILM X-T4", "Provia", &lut, 9, 9, 1).unwrap();
        // Saving again replaces, never duplicates.
        s.save("FUJIFILM X-T4", "Classic Chrome", &lut, 40, 80, 2)
            .unwrap();
        let m = s.manifest();
        assert_eq!(m.len(), 2);
        assert_eq!(
            s.learned("FUJIFILM X-T4", "Classic Chrome").unwrap().photos,
            40
        );
        assert!(s.learned("FUJIFILM X-T3", "Classic Chrome").is_none());
        assert_eq!(
            s.cube("FUJIFILM X-T4", "Provia").unwrap().size,
            super::super::fit::LATTICE
        );
        s.clear().unwrap();
        assert!(s.manifest().is_empty() && s.cube("FUJIFILM X-T4", "Provia").is_none());
        s.clear().unwrap();
    }

    #[test]
    fn a_deleted_cube_is_no_look() {
        let dir = test_dir::TempDir::new("camera-look-deleted");
        let s = Store::new(dir.path().to_path_buf());
        let l = s
            .save(
                "FUJIFILM X-T4",
                "Eterna",
                &super::super::fit::fit(&[]),
                8,
                8,
                1,
            )
            .unwrap();
        std::fs::remove_file(dir.path().join(l.file)).unwrap();
        assert!(s.learned("FUJIFILM X-T4", "Eterna").is_none());
    }
}
