//! What can go wrong talking to an Immich server.
//!
//! ⚠⚠ The variants exist so the UI can say **which** thing went wrong. A single opaque error
//! makes "your API key is wrong" indistinguishable from "your server is unreachable", which are
//! the two failures a self-hosted setup actually hits and which need opposite responses.

/// A failed call to the server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the server rejected the API key")]
    Unauthorized,
    #[error("the server has no such item")]
    NotFound,
    #[error("the server returned an error (HTTP {0})")]
    Server(u16),
    #[error("could not reach the server: {0}")]
    Network(String),
    #[error("the server's reply could not be understood: {0}")]
    Decode(String),
    #[error("writing the downloaded file failed: {0}")]
    Io(String),
    /// The server refused a request and (usually) said why — "Missing required permission:
    /// albumAsset.create". Made only by calls that read the refusal's reply ([`Error::from_reply`]); the
    /// rest still map a status alone ([`Error::from_status`]).
    #[error("the server refused the request (HTTP {status}){}", reason_suffix(.reason))]
    Refused { status: u16, reason: String },
}

/// `": <reason>"`, or nothing when the server gave none.
fn reason_suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(": {reason}")
    }
}

impl Error {
    /// Map an HTTP status to the variant the UI should show.
    pub fn from_status(code: u16) -> Self {
        match code {
            401 | 403 => Error::Unauthorized,
            404 => Error::NotFound,
            other => Error::Server(other),
        }
    }

    /// Map a failed reply — its status AND its body — keeping the server's own reason. 401 stays a rejected
    /// key and 404 a missing item; anything else is [`Error::Refused`] with Immich's `message` (a string, or
    /// NestJS's list of them, joined), or no reason when the body has none.
    pub fn from_reply(code: u16, body: &str) -> Self {
        match code {
            401 => Error::Unauthorized,
            404 => Error::NotFound,
            status => Error::Refused {
                status,
                reason: message_of(body),
            },
        }
    }
}

/// Immich's error reply's `message`: a string, or a list of strings joined by "; "; empty otherwise.
fn message_of(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    match v.get("message") {
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|m| m.as_str().map(str::trim))
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_401_is_unauthorized_not_a_generic_server_error() {
        assert!(matches!(Error::from_status(401), Error::Unauthorized));
        assert!(matches!(Error::from_status(403), Error::Unauthorized));
    }

    #[test]
    fn a_404_is_not_found() {
        assert!(matches!(Error::from_status(404), Error::NotFound));
    }

    #[test]
    fn any_other_failure_carries_its_code() {
        assert!(matches!(Error::from_status(503), Error::Server(503)));
    }

    /// ⚠ CONSTRUCTED, not captured: the shape of Immich's NestJS `ForbiddenException` reply as its source
    /// writes it (`message`, `error`, `statusCode`, `correlationId`). A captured reply would be better evidence.
    const FORBIDDEN: &str = r#"{"message":"Missing required permission: albumAsset.create","error":"Forbidden","statusCode":403,"correlationId":"c1"}"#;

    /// The reported bug: a 403 said "the Write key needs <a guessed permission>" — Immich's own reason was
    /// thrown away. A reply that says why keeps saying why.
    #[test]
    fn a_refusal_keeps_the_servers_reason() {
        match Error::from_reply(403, FORBIDDEN) {
            Error::Refused { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "Missing required permission: albumAsset.create");
            }
            other => panic!("{other:?}"),
        }
        assert!(Error::from_reply(403, FORBIDDEN)
            .to_string()
            .contains("Missing required permission: albumAsset.create"));
    }

    /// A validation failure answers a LIST of messages (NestJS); they are all kept.
    #[test]
    fn a_list_of_messages_is_kept_whole() {
        let body = r#"{"message":["ids must be an array","each value in ids must be a UUID"],"statusCode":400}"#;
        match Error::from_reply(400, body) {
            Error::Refused { reason, .. } => {
                assert_eq!(
                    reason,
                    "ids must be an array; each value in ids must be a UUID"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    /// A reply with no readable reason is still a refusal with its code — never the API-key message.
    #[test]
    fn a_refusal_without_a_reason_still_says_refused() {
        match Error::from_reply(403, "<html>proxy</html>") {
            Error::Refused { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "");
            }
            other => panic!("{other:?}"),
        }
        let s = Error::from_reply(403, "").to_string();
        assert!(s.contains("403") && !s.ends_with(": "), "{s}");
    }

    /// 401 is a rejected key and 404 a missing item, reason or not.
    #[test]
    fn a_reply_keeps_401_and_404_meaning() {
        assert!(matches!(
            Error::from_reply(401, FORBIDDEN),
            Error::Unauthorized
        ));
        assert!(matches!(Error::from_reply(404, FORBIDDEN), Error::NotFound));
    }

    /// ⚠ The UI has to tell "your key is wrong" from "your server is down", so these must not
    /// collapse into one message.
    #[test]
    fn the_three_kinds_read_differently() {
        let a = Error::Unauthorized.to_string();
        let b = Error::NotFound.to_string();
        let c = Error::Server(503).to_string();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
