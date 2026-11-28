//! Files attached to a message, kept in Arena: one sent up (`POST /api/chat/attachments`, a form with the one file),
//! and its content read back (`GET /api/chat/attachments/{id}/content`: a document's text as Arena read it, or the
//! picture, sound or video itself). Arena's web chat attaches files so, and Code Arena's IDE the same.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::client::{ArenaClient, ArenaError, Raw, refusal};

/// The most files one message carries.
pub const MOST_FILES: usize = 10;

/// The most an upload may hold when Arena has not said its limit (Arena's own default, 20 MB).
pub const DEFAULT_MOST_BYTES: u64 = 20 * 1024 * 1024;

/// The pieces an upload is sent in: how far it is is said after each.
const PIECE: usize = 64 * 1024;

/// A file Arena keeps for a message, as its upload answers: its id, name, size, kind ("text" for a document whose
/// text Arena read, "image", "audio", "video", "file"), its type, and whether its text was cut to fit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Uploaded {
    pub id: String,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub truncated: bool,
}

/// A name as a form's header holds it: no quotes, no line breaks, no backslashes.
fn header_name(name: &str) -> String {
    let clean: String = name.chars().map(|c| if c == '"' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    if clean.trim().is_empty() { "file".to_string() } else { clean }
}

/// The body of a form that holds one file (`multipart/form-data` with `boundary`), as a browser sends it.
pub fn form(boundary: &str, name: &str, mime: &str, bytes: &[u8]) -> Vec<u8> {
    let mime = if mime.is_empty() || mime.contains(['\r', '\n']) { "application/octet-stream" } else { mime };
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
        header_name(name)
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

impl ArenaClient {
    /// Sends one file up for a message. `sent` is told the bytes sent so far and of how many, as they go.
    pub async fn upload(
        &self,
        name: &str,
        mime: &str,
        bytes: &[u8],
        sent: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Result<Uploaded, ArenaError> {
        let boundary = format!("----code-arena-{:016x}", bytes.len() as u64 ^ 0x5eed_f11e_c0de_u64);
        let body = form(&boundary, name, mime, bytes);
        let total = body.len() as u64;
        let pieces: Vec<Vec<u8>> = body.chunks(PIECE).map(<[u8]>::to_vec).collect();
        let sent = Arc::new(sent);
        let mut so_far = 0u64;
        let stream = futures_util::stream::iter(pieces).map(move |piece| {
            so_far += piece.len() as u64;
            sent(so_far, total);
            Ok::<_, std::io::Error>(piece)
        });
        let res = self
            .untimed(Method::POST, "/api/chat/attachments")
            .header("Accept", "application/json")
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .header("Content-Length", total)
            .timeout(Duration::from_secs(600))
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        let content_type =
            res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let body = res.text().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let raw = Raw { status, content_type, body };
        if !status.is_success() {
            return Err(refusal(&raw));
        }
        serde_json::from_str(&raw.body)
            .map_err(|e| ArenaError::Parse { path: "/api/chat/attachments".into(), message: e.to_string() })
    }

    /// A file's content as Arena keeps it: a document's text, or the picture's bytes. More than `most` bytes is
    /// refused (`Status` 413), unread.
    pub async fn attachment_content(&self, id: &str, most: u64) -> Result<Vec<u8>, ArenaError> {
        let path = format!("/api/chat/attachments/{id}/content");
        let res = self
            .untimed(Method::GET, &path)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        let status = res.status();
        if !status.is_success() {
            let content_type =
                res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            let body = res.text().await.unwrap_or_default();
            return Err(refusal(&Raw { status, content_type, body }));
        }
        let too_large = || ArenaError::Status {
            status: 413,
            code: Some("too_large".into()),
            message: format!("more than {most} bytes"),
        };
        if res.content_length().is_some_and(|n| n > most) {
            return Err(too_large());
        }
        let bytes = res.bytes().await.map_err(|e| ArenaError::Net(ca_net::explain(&e)))?;
        if bytes.len() as u64 > most {
            return Err(too_large());
        }
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use wiremock::matchers::{header_regex, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    #[test]
    fn a_form_holds_the_one_file_with_its_name_made_safe() {
        let body = form("B", "a \"b\"\n.txt", "text/plain", b"hi");
        let text = String::from_utf8(body).unwrap();
        assert_eq!(
            text,
            "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a _b__.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--B--\r\n"
        );
        assert!(
            String::from_utf8(form("B", "", "", b""))
                .unwrap()
                .contains("filename=\"file\"\r\nContent-Type: application/octet-stream")
        );
    }

    #[tokio::test]
    async fn a_file_goes_up_in_pieces_said_as_they_go_and_its_content_comes_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat/attachments"))
            .and(header_regex("content-type", "^multipart/form-data; boundary="))
            .respond_with(|r: &Request| {
                let body = String::from_utf8_lossy(&r.body);
                assert!(body.contains("filename=\"notes.md\""), "{body}");
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "0b6f0a9e-1c1d-4d39-9f0e-2a4b6f1c9d11", "fileName": "notes.md", "size": 200000,
                    "kind": "text", "contentType": "text/markdown", "chars": 200000, "truncated": true
                }))
            })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/chat/attachments/0b6f0a9e-1c1d-4d39-9f0e-2a4b6f1c9d11/content"))
            .respond_with(ResponseTemplate::new(200).set_body_string("# Notes"))
            .mount(&server)
            .await;
        let arena = ArenaClient::new(&server.uri(), Some(ca_core::Secret::new("test-key")), None).unwrap();
        let said = Arc::new(Mutex::new(Vec::new()));
        let kept = said.clone();
        let bytes = vec![b'x'; 200_000];
        let up =
            arena.upload("notes.md", "text/markdown", &bytes, move |n, of| kept.lock().unwrap().push((n, of))).await;
        let up = up.unwrap();
        assert_eq!((up.file_name.as_str(), up.kind.as_str(), up.truncated), ("notes.md", "text", true));
        let said = said.lock().unwrap().clone();
        assert!(said.len() >= 4, "{said:?}");
        assert_eq!(said.last().map(|(n, of)| n == of), Some(true));
        assert!(said.windows(2).all(|w| w[0].0 < w[1].0));
        let text = arena.attachment_content(&up.id, 1_000).await.unwrap();
        assert_eq!(text, b"# Notes");
        assert!(matches!(arena.attachment_content(&up.id, 3).await, Err(ArenaError::Status { status: 413, .. })));
    }

    #[tokio::test]
    async fn arenas_refusal_of_an_upload_is_said_in_its_words() {
        let server = MockServer::start().await;
        Mock::given(path("/api/chat/attachments"))
            .respond_with(ResponseTemplate::new(413).set_body_json(serde_json::json!({
                "status": "too_large", "error": "big.bin is larger than 20 MB."
            })))
            .mount(&server)
            .await;
        let arena = ArenaClient::new(&server.uri(), Some(ca_core::Secret::new("test-key")), None).unwrap();
        let e = arena.upload("big.bin", "", b"1", |_, _| {}).await.unwrap_err();
        assert!(e.to_string().contains("larger than 20 MB"), "{e}");
    }
}
