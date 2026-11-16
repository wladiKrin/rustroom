//! The album calls against a one-request local HTTP server — what Rustroom makes of Immich's answer.
//!
//! ⚠ Every reply here is CONSTRUCTED from Immich's source, never captured from a server: it proves this
//! crate reads the shape it expects, not that a real server sends it.

use crate::{Client, Error};
use std::io::{Read, Write};
use std::net::TcpListener;

/// Serve ONE request on a free local port, answering `status` (e.g. `"403 Forbidden"`) with `body`; the base URL.
///
/// ⚠ The whole request — head and its `Content-Length` body — is read before answering, or the client may see a
/// reset instead of the reply.
fn answer_once(status: &'static str, body: &'static str) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let Ok((mut s, _)) = l.accept() else { return };
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got);
            if let Some(end) = text.find("\r\n\r\n") {
                let want = text[..end]
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if got.len() >= end + 4 + want {
                    break;
                }
            }
        }
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = s.write_all(reply.as_bytes());
    });
    format!("http://127.0.0.1:{port}")
}

const FORBIDDEN: &str = r#"{"message":"Missing required permission: albumAsset.create","error":"Forbidden","statusCode":403,"correlationId":"c1"}"#;

/// The positive control: the harness answers, and a success reads as success — so the refusal test below
/// cannot pass or fail because the server never spoke.
#[test]
fn a_successful_add_reads_each_ids_answer() {
    let c = Client::new(
        &answer_once("200 OK", r#"[{"id":"x","success":true}]"#),
        "k",
    )
    .unwrap();
    let r = c.add_to_album("a1", &["x".to_string()]).unwrap();
    assert_eq!(r.len(), 1);
    assert!(r[0].settled());
}

/// The reported bug: adding to an album was refused, and the status line named a permission Rustroom
/// guessed. The refusal must carry Immich's own reason.
#[test]
fn adding_to_an_album_keeps_immichs_reason() {
    let c = Client::new(&answer_once("403 Forbidden", FORBIDDEN), "k").unwrap();
    match c.add_to_album("a1", &["x".to_string()]) {
        Err(Error::Refused { status, reason }) => {
            assert_eq!(status, 403);
            assert_eq!(reason, "Missing required permission: albumAsset.create");
        }
        other => panic!("{other:?}"),
    }
}

/// Creating an album and taking assets out answer the same way.
#[test]
fn creating_and_removing_keep_immichs_reason_too() {
    let c = Client::new(&answer_once("403 Forbidden", FORBIDDEN), "k").unwrap();
    assert!(matches!(
        c.create_album("Web"),
        Err(Error::Refused { status: 403, .. })
    ));
    let c = Client::new(&answer_once("403 Forbidden", FORBIDDEN), "k").unwrap();
    assert!(matches!(
        c.remove_from_album("a1", &["x".to_string()]),
        Err(Error::Refused { status: 403, .. })
    ));
}

/// A rejected key is still a rejected key.
#[test]
fn a_401_from_an_album_call_is_a_rejected_key() {
    let c = Client::new(
        &answer_once("401 Unauthorized", r#"{"message":"Invalid API key"}"#),
        "k",
    )
    .unwrap();
    assert!(matches!(
        c.add_to_album("a1", &["x".to_string()]),
        Err(Error::Unauthorized)
    ));
}
