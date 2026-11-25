//! What a tool call returned, read: its text, and the pictures, sounds and files that came with it.

use serde_json::Value;

use crate::CallResult;

/// A picture, a sound or a file a tool returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub mime: String,
    pub bytes: Vec<u8>,
    /// Its name or address when the server gave one.
    pub name: Option<String>,
}

pub(crate) fn read_result(r: &Value) -> CallResult {
    let mut out =
        CallResult { is_error: r.get("isError").and_then(Value::as_bool).unwrap_or(false), ..Default::default() };
    let mut texts = Vec::new();
    for c in r.get("content").and_then(Value::as_array).into_iter().flatten() {
        match c.get("type").and_then(Value::as_str) {
            Some("text") => texts.push(c.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
            Some("image") | Some("audio") => {
                if let Some(bytes) = c.get("data").and_then(Value::as_str).and_then(decode_base64) {
                    out.media.push(Media {
                        mime: c.get("mimeType").and_then(Value::as_str).unwrap_or("application/octet-stream").into(),
                        bytes,
                        name: None,
                    });
                }
            }
            Some("resource") => {
                let res = c.get("resource");
                let name = res.and_then(|r| r.get("uri")).and_then(Value::as_str).map(str::to_string);
                if let Some(t) = res.and_then(|r| r.get("text")).and_then(Value::as_str) {
                    texts.push(t.to_string());
                } else if let Some(bytes) =
                    res.and_then(|r| r.get("blob")).and_then(Value::as_str).and_then(decode_base64)
                {
                    let mime = res
                        .and_then(|r| r.get("mimeType"))
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream");
                    out.media.push(Media { mime: mime.into(), bytes, name });
                }
            }
            Some("resource_link") => {
                let uri = c.get("uri").and_then(Value::as_str).unwrap_or_default();
                texts.push(format!("(a file at {uri})"));
            }
            _ => {}
        }
    }
    if texts.is_empty()
        && let Some(s) = r.get("structuredContent")
    {
        texts.push(s.to_string());
    }
    out.text = texts.join("\n");
    out
}

/// Standard base64 (with or without padding, line breaks ignored).
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in text.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        assert_eq!(decode_base64("aGVsbG8=").expect("ok"), b"hello");
        assert_eq!(decode_base64("aGVsbG8").expect("ok"), b"hello");
        assert_eq!(decode_base64("aGVs\nbG8gd29ybGQ=").expect("ok"), b"hello world");
        assert!(decode_base64("@@").is_none());
    }

    #[test]
    fn text_pictures_and_links() {
        let r = serde_json::json!({"content": [
            {"type": "text", "text": "a"},
            {"type": "audio", "mimeType": "audio/wav", "data": "UklGRg=="},
            {"type": "resource_link", "uri": "https://arena/files/1"}
        ], "isError": true});
        let c = read_result(&r);
        assert_eq!(c.text, "a\n(a file at https://arena/files/1)");
        assert!(c.is_error);
        assert_eq!(c.media[0].bytes, b"RIFF");
    }
}
