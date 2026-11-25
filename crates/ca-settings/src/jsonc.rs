//! JSON with comments and trailing commas, as VS Code's settings files are written.

/// The text as plain JSON: `//` and `/* */` comments blanked (line breaks kept, so a parse error's line is the
/// file's), and a comma before `}` or `]` dropped. Strings are left as they are.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    // Where a comma was written that may yet turn out to trail.
    let mut pending_comma: Option<usize> = None;
    let chars: Vec<char> = text.chars().collect();
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                pending_comma = None;
                in_string = true;
                out.push(c);
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i += 2;
                continue;
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(c);
            }
            '}' | ']' => {
                if let Some(at) = pending_comma.take() {
                    out.replace_range(at..at + 1, " ");
                }
                out.push(c);
            }
            c if c.is_whitespace() => out.push(c),
            _ => {
                pending_comma = None;
                out.push(c);
            }
        }
        i += 1;
    }
    out
}

/// Parses JSONC into a value.
pub fn parse(text: &str) -> Result<serde_json::Value, serde_json::Error> {
    let plain = strip(text);
    if plain.trim().is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    serde_json::from_str(&plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_and_trailing_commas() {
        let text = r#"{
  // the model
  "agent.model": "qwen", /* inline */
  "list": [1, 2,],
  "url": "https://a//b /* not a comment */",
}"#;
        assert_eq!(
            parse(text).expect("parses"),
            json!({"agent.model": "qwen", "list": [1, 2], "url": "https://a//b /* not a comment */"})
        );
    }

    #[test]
    fn escaped_quotes_stay_in_the_string() {
        assert_eq!(parse(r#"{"a": "x\"// y"}"#).expect("parses"), json!({"a": "x\"// y"}));
    }

    #[test]
    fn empty_is_an_empty_object() {
        assert_eq!(parse("  // nothing\n").expect("parses"), json!({}));
    }

    #[test]
    fn a_parse_error_keeps_its_line() {
        let err = parse("{\n// one\n/* two\nthree */\n\"a\": }").expect_err("fails");
        assert_eq!(err.line(), 5);
    }
}
