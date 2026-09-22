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
