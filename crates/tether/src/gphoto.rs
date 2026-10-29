//! Real cameras, through libgphoto2 (feature `gphoto2`).
//!
//! ⚠ A Fujifilm body sends its shots only in **USB TETHER SHOOTING AUTO** (Set up → Connection
//! setting → PC Connection Mode); as a card reader it lists its files but raises no events.

use crate::trouble::{sentence, Trouble};
use crate::{Camera, CameraFile, Opener};
use gphoto2::camera::CameraEvent;
use gphoto2::error::ErrorKind;
use gphoto2::Context;
use std::path::Path;
use std::time::Duration;

fn say(e: gphoto2::Error) -> String {
    let t = match e.kind() {
        ErrorKind::IoUsbClaim | ErrorKind::IoLock => Trouble::Claimed,
        ErrorKind::Io
        | ErrorKind::IoRead
        | ErrorKind::IoWrite
        | ErrorKind::IoUsbFind
        | ErrorKind::IoUsbClearHalt
        | ErrorKind::UnknownPort => Trouble::Gone,
        ErrorKind::CameraBusy | ErrorKind::NotSupported => Trouble::Refused,
        _ => Trouble::Other,
    };
    sentence(t, &e.to_string())
}

/// The first camera on the computer's ports, by its model, or `None`.
pub fn detect() -> Result<Option<String>, String> {
    let ctx = Context::new().map_err(say)?;
    let mut cams = ctx.list_cameras().wait().map_err(say)?;
    Ok(cams.next().map(|d| d.model))
}

/// Opens the first camera found — on the session's thread.
pub fn opener() -> Opener {
    Box::new(|| {
        let ctx = Context::new().map_err(say)?;
        let desc = ctx
            .list_cameras()
            .wait()
            .map_err(say)?
            .next()
            .ok_or_else(|| "No camera found".to_string())?;
        let camera = ctx.get_camera(&desc).wait().map_err(say)?;
        Ok(Box::new(Gphoto {
            name: desc.model,
            camera,
        }) as Box<dyn Camera + Send>)
    })
}

struct Gphoto {
    name: String,
    camera: gphoto2::Camera,
}

impl Camera for Gphoto {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn wait(&mut self, timeout: Duration) -> Result<Option<CameraFile>, String> {
        match self.camera.wait_event(timeout).wait() {
            Ok(CameraEvent::NewFile(p)) => Ok(Some(CameraFile {
                folder: p.folder().into_owned(),
                name: p.name().into_owned(),
            })),
            Ok(_) => Ok(None),
            // ⚠ A quiet camera may answer a wait with a timeout ERROR rather than the event.
            Err(e) if matches!(e.kind(), ErrorKind::Timeout) => Ok(None),
            Err(e) => Err(say(e)),
        }
    }

    fn fetch(&mut self, file: &CameraFile, to: &Path) -> Result<(), String> {
        self.camera
            .fs()
            .download_to(&file.folder, &file.name, to)
            .wait()
            .map(|_| ())
            .map_err(say)
    }

    fn shoot(&mut self) -> Result<Vec<CameraFile>, String> {
        let p = self.camera.capture_image().wait().map_err(say)?;
        Ok(vec![CameraFile {
            folder: p.folder().into_owned(),
            name: p.name().into_owned(),
        }])
    }
}
