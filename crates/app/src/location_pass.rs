//! The one-time pass over photos catalogued before M10.18b: each file's GPS position, read in the
//! background so the Map has their pins. No egui.
//!
//! ⚠⚠ A file not on disk is SKIPPED, never answered: an Immich row not yet downloaded has no file, and a
//! "read, none" row for it would claim a reading that never happened — its server position still shows,
//! but the file's would never be read once it arrives.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// What one file says: its path and its position, `None` for a file with no GPS.
pub type Found = (PathBuf, Option<(f64, f64)>);

/// The pass's thread and what it has found so far.
pub struct LocationPass {
    rx: Receiver<Found>,
}

/// The EXIF block when the file has one (~0.5 ms), the full reader otherwise.
fn read_gps(path: &Path) -> Option<(f64, f64)> {
    match image_decode::quick_look(path) {
        Ok(Some(q)) => q.capture.gps,
        _ => crate::metadata_scanner::read_capture(path).gps,
    }
}

impl LocationPass {
    pub fn start(files: Vec<PathBuf>) -> Self {
        Self::with_reader(files, read_gps)
    }

    /// [`LocationPass::start`] with its reader handed in, for tests.
    pub fn with_reader(files: Vec<PathBuf>, read: fn(&Path) -> Option<(f64, f64)>) -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("location-pass".into())
            .spawn(move || {
                for f in files {
                    if !f.is_file() {
                        continue;
                    }
                    if tx.send((f.clone(), read(&f))).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn the location pass");
        Self { rx }
    }

    /// Everything found since the last call, and whether the pass is over (its thread gone and every
    /// answer read). ⚠ One call: a separate "finished?" check would swallow an answer arriving between.
    pub fn drain(&self) -> (Vec<Found>, bool) {
        let mut found = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(f) => found.push(f),
                Err(std::sync::mpsc::TryRecvError::Empty) => return (found, false),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return (found, true),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn all(p: &LocationPass) -> Vec<Found> {
        let t = Instant::now();
        let mut out = Vec::new();
        loop {
            let (found, done) = p.drain();
            out.extend(found);
            if done {
                return out;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "the pass hung");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Review focus 1: an Immich row not yet downloaded gets no answer at all.
    #[test]
    fn the_startup_pass_skips_a_file_not_on_disk() {
        let d = test_dir::TempDir::new("location-pass");
        let here = d.join("a.JPG");
        std::fs::write(&here, b"x").unwrap();
        let p = LocationPass::with_reader(vec![d.join("not-downloaded.RAF"), here.clone()], |_| {
            Some((38.7, -9.1))
        });
        assert_eq!(all(&p), vec![(here, Some((38.7, -9.1)))]);
    }

    #[test]
    fn a_file_without_gps_reads_as_none() {
        let d = test_dir::TempDir::new("location-pass-none");
        let here = d.join("a.JPG");
        std::fs::write(&here, b"not a jpeg").unwrap();
        let p = LocationPass::start(vec![here.clone()]);
        assert_eq!(all(&p), vec![(here, None)]);
    }
}
