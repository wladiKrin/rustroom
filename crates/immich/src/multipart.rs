//! `multipart/form-data` encoding, by hand.
//!
//! ⚠⚠ **`ureq` has no multipart helper**, so this is hand-rolled — and a server rejecting a
//! malformed body answers with a bare `400` and no hint about which part was wrong. That is why
//! this module is pure, separate, and pinned by a **byte-exact** test.

/// One form field: a plain text value, or a file with a filename and a content type.
pub enum Part<'a> {
    Text {
        name: &'a str,
        value: &'a str,
    },
    File {
        name: &'a str,
        filename: &'a str,
        content_type: &'a str,
        bytes: &'a [u8],
    },
}

/// The body for `parts` using `boundary`.
///
/// ⚠ CRLF line endings throughout, and the closing delimiter is `--<boundary>--`. Both are
/// required by RFC 7578; a bare `\n` is the classic silent failure.
pub fn body(boundary: &str, parts: &[Part<'_>]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match p {
            Part::Text { name, value } => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(value.as_bytes());
            }
            Part::File {
                name,
                filename,
                content_type,
                bytes,
            } => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// A boundary that is guaranteed not to occur in `data`.
///
/// ⚠⚠ **A boundary appearing inside the file would truncate the upload**, so this is checked
/// rather than assumed improbable: the suffix is bumped until the boundary is absent. Scanning a
/// 15 MB JPEG is microseconds against a multi-second upload.
pub fn boundary_absent_from(data: &[u8]) -> String {
    // ⚠ Long and dash-prefixed so it cannot collide with ordinary text, and derived from the
    // clock so two concurrent uploads (the export pool is `width`-wide) never share one.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for n in 0u32.. {
        let candidate = format!("----rustroom{seed:032x}{n:04x}");
        if !data
            .windows(candidate.len())
            .any(|w| w == candidate.as_bytes())
        {
            return candidate;
        }
    }
    unreachable!("u32 candidates cannot all collide")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ Byte-exact on purpose. Every structural rule of RFC 7578 that this encoder can get
    /// wrong — the leading `--`, CRLF after each header, the blank line before the body, the CRLF
    /// before the next delimiter, and the trailing `--` — is visible in this one literal.
    #[test]
    fn a_small_body_is_byte_exact() {
        let got = body(
            "XbounD",
            &[
                Part::Text {
                    name: "fileCreatedAt",
                    value: "2026-08-12T20:41:34.000Z",
                },
                Part::File {
                    name: "assetData",
                    filename: "a.jpg",
                    content_type: "image/jpeg",
                    bytes: b"\xff\xd8ok",
                },
            ],
        );
        let want: Vec<u8> = [
            b"--XbounD\r\n".as_ref(),
            b"Content-Disposition: form-data; name=\"fileCreatedAt\"\r\n\r\n",
            b"2026-08-12T20:41:34.000Z\r\n",
            b"--XbounD\r\n",
            b"Content-Disposition: form-data; name=\"assetData\"; filename=\"a.jpg\"\r\n",
            b"Content-Type: image/jpeg\r\n\r\n",
            b"\xff\xd8ok\r\n",
            b"--XbounD--\r\n",
        ]
        .concat();
        assert_eq!(
            got,
            want,
            "\ngot:  {:?}\nwant: {:?}",
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&want)
        );
    }

    /// ⚠ Binary data must survive untouched — this encoder must never treat bytes as text.
    #[test]
    fn binary_bytes_pass_through_unchanged() {
        let raw: Vec<u8> = (0u8..=255).collect();
        let out = body(
            "B",
            &[Part::File {
                name: "assetData",
                filename: "x.bin",
                content_type: "application/octet-stream",
                bytes: &raw,
            }],
        );
        let start = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(
            &out[start..start + 256],
            &raw[..],
            "the payload was altered"
        );
    }

    #[test]
    fn a_boundary_avoids_colliding_with_the_data() {
        // ⚠ The discriminating case: data that literally contains the first candidate.
        let first = boundary_absent_from(b"");
        let data = format!("junk {first} junk").into_bytes();
        let second = boundary_absent_from(&data);
        assert_ne!(
            first, second,
            "the boundary must change when it occurs in the data"
        );
        assert!(
            !data.windows(second.len()).any(|w| w == second.as_bytes()),
            "the chosen boundary still occurs in the data"
        );
    }
}
