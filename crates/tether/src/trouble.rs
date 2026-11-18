//! What a camera error tells the person — one sentence, no full stop (the status row's).

/// The kinds of camera trouble that read differently; everything else is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// Another program holds the USB device (GNOME's gvfs, macOS's PTPCamera).
    Claimed,
    /// The device went away — a pulled cable, a camera switched off.
    Gone,
    /// The camera refused: busy, or in a mode that cannot do this.
    Refused,
    Other,
    /// Linux does not let this user open the camera's USB device — no udev rule grants it (v0.1, piece 3c;
    /// `usb_access`). libgphoto2 reports it only as an I/O problem.
    NoPermission,
}

/// What `t` really is: libgphoto2 reports a camera it may not open as an I/O loss — at the first operation, not at
/// opening (captured, `usb_access`) — so a [`Trouble::Gone`] while `node_blocked` says the camera's USB node will not
/// open is [`Trouble::NoPermission`]. `node_blocked` is asked only then.
pub fn cause(t: Trouble, node_blocked: impl FnOnce() -> bool) -> Trouble {
    match t {
        Trouble::Gone if node_blocked() => Trouble::NoPermission,
        t => t,
    }
}

/// The status row's sentence for `t`; `raw` is the library's own text, kept for `Other`.
pub fn sentence(t: Trouble, raw: &str) -> String {
    match t {
        // ⚠ On macOS the system's own camera service is pushed off by the session (`hold`), so what is left
        // is an app the person can quit.
        Trouble::Claimed if cfg!(target_os = "macos") => {
            "Another program is using the camera — quit Image Capture, Photos or any other camera app".into()
        }
        Trouble::Claimed => "Another program is using the camera".into(),
        Trouble::Gone => "The camera was disconnected".into(),
        Trouble::Refused => "Set the camera to USB tether shooting".into(),
        Trouble::NoPermission => {
            "Linux does not allow Rustroom to use this camera — install libgphoto2's udev rules".into()
        }
        Trouble::Other => format!(
            "The camera reported an error: {}",
            raw.trim_end_matches('.')
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_claim_error_reads_as_another_program() {
        assert!(sentence(Trouble::Claimed, "Could not claim the USB device")
            .starts_with("Another program is using the camera"));
        let s = sentence(Trouble::Other, "Unspecified error.");
        assert!(s.ends_with("Unspecified error"), "{s}");
    }

    #[test]
    fn a_lost_camera_whose_node_will_not_open_is_a_permission_not_a_disconnect() {
        assert_eq!(cause(Trouble::Gone, || true), Trouble::NoPermission);
        assert_eq!(cause(Trouble::Gone, || false), Trouble::Gone);
        // Only an I/O loss is re-read: another program's claim stays a claim, with or without the node.
        assert_eq!(cause(Trouble::Claimed, || true), Trouble::Claimed);
        assert_eq!(cause(Trouble::Other, || true), Trouble::Other);
    }

    #[test]
    fn the_permission_sentence_names_the_udev_rules() {
        let s = sentence(Trouble::NoPermission, "");
        assert!(s.contains("udev rules"), "{s}");
        assert!(!s.ends_with('.'), "{s}");
    }
}
