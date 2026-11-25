//! Text as files hold it: UTF-8, or UTF-16 and UTF-32 by their byte-order mark (Windows PowerShell's `>` writes
//! UTF-16), and CRLF kept where a file has it.

/// How a file's text was stored, so it is written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}

/// Why bytes are not text.
#[derive(Debug, PartialEq, Eq)]
pub enum NotText {
    Binary,
}

/// The bytes as text, and how they were stored.
pub fn decode(bytes: &[u8]) -> Result<(String, Encoding), NotText> {
    let (enc, body) = match bytes {
        [0xFF, 0xFE, 0, 0, rest @ ..] => (Encoding::Utf32Le, rest),
        [0, 0, 0xFE, 0xFF, rest @ ..] => (Encoding::Utf32Be, rest),
        [0xEF, 0xBB, 0xBF, rest @ ..] => (Encoding::Utf8Bom, rest),
        [0xFF, 0xFE, rest @ ..] => (Encoding::Utf16Le, rest),
        [0xFE, 0xFF, rest @ ..] => (Encoding::Utf16Be, rest),
        _ => (Encoding::Utf8, bytes),
    };
    let text = match enc {
        Encoding::Utf8 | Encoding::Utf8Bom => {
            if body.contains(&0) {
                return Err(NotText::Binary);
            }
            String::from_utf8(body.to_vec()).map_err(|_| NotText::Binary)?
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let units: Vec<u16> = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if enc == Encoding::Utf16Le {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16(&units).map_err(|_| NotText::Binary)?
        }
        Encoding::Utf32Le | Encoding::Utf32Be => body
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                let n = if enc == Encoding::Utf32Le {
                    u32::from_le_bytes([c[0], c[1], c[2], c[3]])
                } else {
                    u32::from_be_bytes([c[0], c[1], c[2], c[3]])
                };
                char::from_u32(n).ok_or(NotText::Binary)
            })
            .collect::<Result<String, _>>()?,
    };
    Ok((text, enc))
}

/// The text in its encoding, with its mark.
pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
        Encoding::Utf16Be => [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect(),
        Encoding::Utf32Le => {
            [0xFF, 0xFE, 0, 0].into_iter().chain(text.chars().flat_map(|c| (c as u32).to_le_bytes())).collect()
        }
        Encoding::Utf32Be => {
            [0, 0, 0xFE, 0xFF].into_iter().chain(text.chars().flat_map(|c| (c as u32).to_be_bytes())).collect()
        }
    }
}

/// Whether the text's lines end with CRLF (most of them do).
pub fn uses_crlf(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    crlf > 0 && crlf * 2 >= text.matches('\n').count()
}

/// `\n` as `\r\n`, for a file that uses CRLF; one that has it already stays.
pub fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Words that mark a line worth showing when the middle of an output is cut.
const ERROR_WORDS: &[&str] = &["error", "failed", "failure", "exception", "traceback", "panicked", "fatal"];

/// An output past `max` characters, cut in the middle: its start and its end, and the lines of the cut part that
/// look like errors, with their numbers.
pub fn cut_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let head_n = max * 2 / 5;
    let tail_n = max * 2 / 5;
    let head: String = text.chars().take(head_n).collect();
    let tail: String = text.chars().skip(count - tail_n).collect();
    let head_lines = head.matches('\n').count();
    let middle: String = text.chars().skip(head_n).take(count - head_n - tail_n).collect();
    let mut flagged = Vec::new();
    let budget = max / 5;
    let mut used = 0;
    for (i, line) in middle.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        if ERROR_WORDS.iter().any(|w| lower.contains(w)) {
            let shown: String = line.chars().take(300).collect();
            used += shown.len() + 10;
            if used > budget {
                break;
            }
            flagged.push(format!("{:>6}: {shown}", head_lines + i + 1));
        }
    }
    let mut out = head;
    out.push_str(&format!("\n… {} characters cut from the middle …\n", count - head_n - tail_n));
    if !flagged.is_empty() {
        out.push_str("Lines in the cut part that look like errors:\n");
        out.push_str(&flagged.join("\n"));
        out.push_str("\n…\n");
    }
    out.push_str(&tail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_encoding() {
        let text = "héllo\r\nwörld ✓\r\n";
        for enc in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Utf32Le,
            Encoding::Utf32Be,
        ] {
            let bytes = encode(text, enc);
            assert_eq!(decode(&bytes).expect("text"), (text.to_string(), enc), "{enc:?}");
        }
    }

    #[test]
    fn binary_is_not_text() {
        assert_eq!(decode(b"\x7fELF\0\0\x01").expect_err("binary"), NotText::Binary);
        assert_eq!(decode(&[0xC3, 0x28]).expect_err("bad utf-8"), NotText::Binary);
    }

    #[test]
    fn line_endings() {
        assert!(uses_crlf("a\r\nb\r\n"));
        assert!(!uses_crlf("a\nb\n"));
        assert_eq!(to_crlf("a\nb\r\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn the_middle_is_cut_and_its_errors_listed() {
        let mut text = String::new();
        for i in 0..2000 {
            text.push_str(&format!("line {i}\n"));
            if i == 1000 {
                text.push_str("error[E0308]: mismatched types\n");
            }
        }
        let cut = cut_middle(&text, 2000);
        assert!(cut.len() < 2600, "{}", cut.len());
        assert!(cut.starts_with("line 0\n"));
        assert!(cut.trim_end().ends_with("line 1999"));
        assert!(cut.contains("error[E0308]: mismatched types"));
        assert!(cut.contains("characters cut from the middle"));
    }
}
