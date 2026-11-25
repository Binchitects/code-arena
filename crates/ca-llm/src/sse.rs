//! Server-sent events, as the gateway streams them: `data:` lines, an empty line between events.

/// Splits a byte stream into events' data, keeping what is not finished yet for the next chunk.
#[derive(Default)]
pub struct SseReader {
    buf: String,
    data: Vec<String>,
}

impl SseReader {
    /// Feeds a chunk; returns each event's data that it completed.
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(i) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=i).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // Comments (`:`), `event:`, `id:` and `retry:` say nothing the gateway uses.
        }
        out
    }

    /// The stream ended: an event without its blank line still counts.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        if let Some(d) = rest.trim_end().strip_prefix("data:") {
            self.data.push(d.trim_start().to_string());
        }
        (!self.data.is_empty()).then(|| std::mem::take(&mut self.data).join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_across_chunks() {
        let mut r = SseReader::default();
        assert!(r.feed("data: {\"a\"").is_empty());
        assert_eq!(r.feed(":1}\r\n\r\n: keep-alive\n\ndata: [DONE]\n\n"), vec!["{\"a\":1}", "[DONE]"]);
    }

    #[test]
    fn a_last_event_without_its_blank_line() {
        let mut r = SseReader::default();
        assert!(r.feed("data: x\n").is_empty());
        assert_eq!(r.finish().as_deref(), Some("x"));
    }
}
