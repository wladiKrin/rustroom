//! A scripted camera for tests — this crate's and the app's (feature `testing`).

use crate::{Camera, CameraFile, Opener};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// What the fake does at its next wait.
#[derive(Debug, Clone)]
pub enum Step {
    /// A new file with these bytes.
    File(String, Vec<u8>),
    /// A new file whose download fails half-way, leaving bytes at the destination first.
    Broken(String),
    /// The wait fails — a pulled cable.
    WaitError(String),
}

/// A camera that plays `steps`, then waits quietly. `shots` counts the shutter presses; each press makes
/// one `SHOT<n>.JPG`.
pub struct Fake {
    pub name: String,
    steps: VecDeque<Step>,
    files: HashMap<String, Option<Vec<u8>>>,
    pub shots: Arc<AtomicUsize>,
}

impl Fake {
    pub fn new(name: &str, steps: Vec<Step>) -> Self {
        Self {
            name: name.to_string(),
            steps: steps.into(),
            files: HashMap::new(),
            shots: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Opens this fake on the session's thread.
    pub fn opener(self) -> Opener {
        Box::new(move || Ok(Box::new(self) as Box<dyn Camera + Send>))
    }

    fn file(&mut self, name: String, bytes: Option<Vec<u8>>) -> CameraFile {
        self.files.insert(name.clone(), bytes);
        CameraFile {
            folder: "/store_10000001/DCIM/100_FUJI".into(),
            name,
        }
    }
}

/// An opener that fails with `sentence`.
pub fn unopenable(sentence: &str) -> Opener {
    let s = sentence.to_string();
    Box::new(move || Err(s))
}

impl Camera for Fake {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn wait(&mut self, timeout: Duration) -> Result<Option<CameraFile>, String> {
        match self.steps.pop_front() {
            Some(Step::File(n, b)) => Ok(Some(self.file(n, Some(b)))),
            Some(Step::Broken(n)) => Ok(Some(self.file(n, None))),
            Some(Step::WaitError(e)) => Err(e),
            None => {
                std::thread::sleep(timeout.min(Duration::from_millis(5)));
                Ok(None)
            }
        }
    }

    fn fetch(&mut self, file: &CameraFile, to: &Path) -> Result<(), String> {
        match self.files.get(&file.name) {
            Some(Some(b)) => std::fs::write(to, b).map_err(|e| e.to_string()),
            Some(None) => {
                let _ = std::fs::write(to, b"half");
                Err("the camera stopped answering".into())
            }
            None => Err(format!("no file {}", file.name)),
        }
    }

    fn shoot(&mut self) -> Result<Vec<CameraFile>, String> {
        let n = self.shots.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(vec![self.file(format!("SHOT{n}.JPG"), Some(vec![n as u8]))])
    }
}
