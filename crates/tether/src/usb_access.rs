//! Whether Linux lets Rustroom open the camera at all (v0.1, piece 3c). A camera's `/dev/bus/usb/BBB/DDD` is the
//! user's only through udev rules — systemd's `70-uaccess.rules` grants any PTP camera to the seat's user, a
//! distribution's libgphoto2 may install its own, an AppImage can install neither. Without them libgphoto2 still LISTS
//! the camera but fails to open it with a bare "I/O problem", read as "disconnected" — so the cause is read from the
//! device node, never from libgphoto2's text, which does not carry it (captured on Arch:
//! `grep -n 'libgphoto2 without USB permission' docs/JOURNAL.md`).

use std::path::Path;

/// The USB interface class of a still-image camera (PTP) — a Fujifilm's, and most cameras'.
const STILL_IMAGE: &str = "06";

/// The first still-image camera under `sys_usb` (sysfs' `/sys/bus/usb`) whose node under `dev_usb` (`/dev/bus/usb`)
/// cannot be opened for reading and writing, by its product name (or "camera") — `None` when every camera's node
/// opens, or there is none. A node that does not exist is not blocked: the device went away. Off Linux, `None`.
pub fn blocked_camera(sys_usb: &Path, dev_usb: &Path) -> Option<String> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (sys_usb, dev_usb, STILL_IMAGE);
        None
    }
    #[cfg(target_os = "linux")]
    {
        let devices = sys_usb.join("devices");
        let read = |p: &Path| {
            std::fs::read_to_string(p)
                .ok()
                .map(|s| s.trim().to_string())
        };
        let mut names: Vec<String> = std::fs::read_dir(&devices)
            .ok()?
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        names.sort();
        // A device is an entry without a ':'; its interfaces are `<device>:<config>.<interface>`.
        let cameras = names.iter().filter(|n| !n.contains(':')).filter(|n| {
            let prefix = format!("{n}:");
            names.iter().any(|i| {
                i.starts_with(&prefix)
                    && read(&devices.join(i).join("bInterfaceClass")).as_deref()
                        == Some(STILL_IMAGE)
            })
        });
        for name in cameras {
            let dir = devices.join(name);
            let number = |f: &str| read(&dir.join(f))?.parse::<u32>().ok();
            let (Some(bus), Some(dev)) = (number("busnum"), number("devnum")) else {
                continue;
            };
            let node = dev_usb.join(format!("{bus:03}")).join(format!("{dev:03}"));
            if !node.exists() {
                continue;
            }
            let opens = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&node)
                .is_ok();
            if !opens {
                return Some(read(&dir.join("product")).unwrap_or_else(|| "camera".into()));
            }
        }
        None
    }
}

/// [`blocked_camera`] on this computer's own sysfs and device tree.
pub fn blocked_camera_here() -> Option<String> {
    blocked_camera(Path::new("/sys/bus/usb"), Path::new("/dev/bus/usb"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A device as sysfs lays it out: `devices/<name>` with its bus and device numbers and product, and one
    /// interface `devices/<name>:1.0` of `class`; its node under `dev`, writable or not (`None`: no node).
    fn device(
        root: &Path,
        name: &str,
        bus: u32,
        dev: u32,
        class: &str,
        writable: Option<bool>,
    ) -> PathBuf {
        let d = root.join("sys").join("devices").join(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("busnum"), format!("{bus}\n")).unwrap();
        fs::write(d.join("devnum"), format!("{dev}\n")).unwrap();
        fs::write(d.join("product"), "X-T4\n").unwrap();
        let i = root.join("sys").join("devices").join(format!("{name}:1.0"));
        fs::create_dir_all(&i).unwrap();
        fs::write(i.join("bInterfaceClass"), format!("{class}\n")).unwrap();
        let node = root
            .join("dev")
            .join(format!("{bus:03}"))
            .join(format!("{dev:03}"));
        if let Some(w) = writable {
            fs::create_dir_all(node.parent().unwrap()).unwrap();
            fs::write(&node, b"").unwrap();
            let mode = if w { 0o666 } else { 0o444 };
            fs::set_permissions(&node, fs::Permissions::from_mode(mode)).unwrap();
        }
        node
    }

    fn trees(root: &Path) -> (PathBuf, PathBuf) {
        (root.join("sys"), root.join("dev"))
    }

    /// The premise every "blocked" case rests on: a 0o444 file cannot be opened read-write. Root opens anything, so
    /// there the tests say so and stop.
    fn unwritable_really_is(node: &Path) -> bool {
        let opens = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .is_ok();
        if opens {
            eprintln!("SKIPPED: running as root, which opens a 0o444 file");
        }
        !opens
    }

    #[test]
    fn an_unwritable_camera_is_named() {
        let d = test_dir::TempDir::new("usb-access-blocked");
        let node = device(d.path(), "2-10.2", 2, 7, "06", Some(false));
        if !unwritable_really_is(&node) {
            return;
        }
        let (sys, dev) = trees(d.path());
        assert_eq!(blocked_camera(&sys, &dev), Some("X-T4".into()));
    }

    #[test]
    fn a_writable_camera_is_not_blocked() {
        let d = test_dir::TempDir::new("usb-access-writable");
        device(d.path(), "2-10.2", 2, 7, "06", Some(true));
        let (sys, dev) = trees(d.path());
        assert_eq!(blocked_camera(&sys, &dev), None);
    }

    #[test]
    fn an_unwritable_non_camera_beside_a_writable_camera_is_not_blocked() {
        let d = test_dir::TempDir::new("usb-access-webcam");
        let webcam = device(d.path(), "1-3", 1, 4, "0e", Some(false));
        if !unwritable_really_is(&webcam) {
            return;
        }
        device(d.path(), "2-10.2", 2, 7, "06", Some(true));
        let (sys, dev) = trees(d.path());
        assert_eq!(blocked_camera(&sys, &dev), None);
    }

    #[test]
    fn no_camera_at_all_is_not_blocked() {
        let d = test_dir::TempDir::new("usb-access-none");
        let webcam = device(d.path(), "1-3", 1, 4, "0e", Some(false));
        if !unwritable_really_is(&webcam) {
            return;
        }
        let (sys, dev) = trees(d.path());
        assert_eq!(blocked_camera(&sys, &dev), None);
    }

    #[test]
    fn a_missing_node_is_not_blocked() {
        let d = test_dir::TempDir::new("usb-access-gone");
        device(d.path(), "2-10.2", 2, 7, "06", None);
        let (sys, dev) = trees(d.path());
        assert_eq!(blocked_camera(&sys, &dev), None);
    }
}
