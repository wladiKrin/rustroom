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
}

/// The status row's sentence for `t`; `raw` is the library's own text, kept for `Other`.
pub fn sentence(t: Trouble, raw: &str) -> String {
    match t {
        Trouble::Claimed => "Another program is using the camera".into(),
        Trouble::Gone => "The camera was disconnected".into(),
        Trouble::Refused => "Set the camera to USB tether shooting".into(),
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
        assert_eq!(
            sentence(Trouble::Claimed, "Could not claim the USB device"),
            "Another program is using the camera"
        );
        let s = sentence(Trouble::Other, "Unspecified error.");
        assert!(s.ends_with("Unspecified error"), "{s}");
    }
}
