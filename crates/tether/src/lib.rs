//! Tethered capture (M10.18a): a camera on a USB cable, each shot downloaded into a folder as it is
//! taken, and the shutter fired from the computer. No GUI.
//!
//! ⚠⚠ **One thread owns the camera for its whole life** ([`Session`]): libgphoto2 blocks for seconds
//! and is not thread-safe, so nothing else ever touches a [`Camera`].
//!
//! ⚠ The camera's card is only ever READ — nothing here deletes or writes a file on it (the import's rule).

#[cfg(any(test, feature = "testing"))]
pub mod fake;
#[cfg(all(feature = "gphoto2", not(windows)))]
pub mod gphoto;
pub mod hold;
pub mod name;
pub mod session;
pub mod trouble;

pub use hold::Hold;
pub use name::free_name;
pub use session::{Event, Session};

use std::path::Path;
use std::time::Duration;

/// A file the camera has, by its folder on the camera and its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraFile {
    pub folder: String,
    pub name: String,
}

/// A connected camera. Errors are sentences for the status row.
pub trait Camera {
    /// The model, e.g. "Fujifilm X-T4".
    fn name(&self) -> String;
    /// The next new file, or `None` when nothing arrived within `timeout`.
    fn wait(&mut self, timeout: Duration) -> Result<Option<CameraFile>, String>;
    /// Download `file` to `to`.
    fn fetch(&mut self, file: &CameraFile, to: &Path) -> Result<(), String>;
    /// Fire the shutter; the files the shot made.
    fn shoot(&mut self) -> Result<Vec<CameraFile>, String>;
}

/// Opens the camera — run ON the session's thread, which then owns it.
pub type Opener = Box<dyn FnOnce() -> Result<Box<dyn Camera + Send>, String> + Send>;
