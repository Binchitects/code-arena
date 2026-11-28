//! Fill-in-the-middle: the code before the cursor and after it, put in a model's own tokens for the text between them
//! (the editor's code completion), and what it writes tidied before it is shown.

use serde_json::{Value, json};

/// The most of the code before the cursor and after it sent with each request, in characters.
pub const MOST_PREFIX: usize = 6_000;
pub const MOST_SUFFIX: usize = 2_000;

/// The longest a completion is, in tokens: a line or a few.
pub const MOST_TOKENS: u32 = 96;

/// A family of models' fill-in-the-middle tokens: before, after, middle, and what ends its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub prefix: &'static str,
    pub suffix: &'static str,
    pub middle: &'static str,
    pub stop: &'static [&'static str],
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "qwen",
        prefix: "<|fim_prefix|>",
        suffix: "<|fim_suffix|>",
        middle: "<|fim_middle|>",
        stop: &["<|fim_pad|>", "<|endoftext|>", "<|fim_prefix|>", "<|file_sep|>"],
    },
    Template {
        name: "deepseek",
        prefix: "<\u{ff5c}fim\u{2581}begin\u{ff5c}>",
        suffix: "<\u{ff5c}fim\u{2581}hole\u{ff5c}>",
        middle: "<\u{ff5c}fim\u{2581}end\u{ff5c}>",
        stop: &["<\u{ff5c}end\u{2581}of\u{2581}sentence\u{ff5c}>", "<\u{ff5c}fim\u{2581}begin\u{ff5c}>"],
    },
    Template { name: "codellama", prefix: "<PRE> ", suffix: " <SUF>", middle: " <MID>", stop: &["<EOT>", "</s>"] },
    Template {
        name: "starcoder",
        prefix: "<fim_prefix>",
        suffix: "<fim_suffix>",
        middle: "<fim_middle>",
        stop: &["<|endoftext|>", "<fim_prefix>", "<file_sep>"],
    },
];

/// The template of a model, by its family's name in the model's id (`deepseek-coder`, `starcoder2`); Qwen's for any
/// other, as Arena's own models are.
pub fn template(model: &str) -> &'static Template {
    let id = model.to_ascii_lowercase();
    let family = |t: &&Template| match t.name {
        "codellama" => id.contains("codellama") || id.contains("code-llama"),
        name => id.contains(name),
    };
    TEMPLATES.iter().find(family).unwrap_or(&TEMPLATES[0])
}

/// The last `most` characters of a text.
pub fn tail(text: &str, most: usize) -> &str {
    match text.char_indices().rev().nth(most.saturating_sub(1)) {
        Some((at, _)) if most > 0 => &text[at..],
        _ if most == 0 => "",
        _ => text,
    }
}

/// The first `most` characters of a text.
pub fn head(text: &str, most: usize) -> &str {
    match text.char_indices().nth(most) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// The body of a `/v1/completions` request for the code around the cursor of a file: the file's path first, as a
/// comment the model knows; the prefix and suffix cut to their most.
pub fn body(model: &str, path: &str, prefix: &str, suffix: &str) -> Value {
    let t = template(model);
    let (prefix, suffix) = (tail(prefix, MOST_PREFIX), head(suffix, MOST_SUFFIX));
    let mut stop: Vec<&str> = t.stop.to_vec();
    stop.push("\n\n\n");
    json!({
        "model": model,
        "prompt": format!("{}// {path}\n{prefix}{}{suffix}{}", t.prefix, t.suffix, t.middle),
        "max_tokens": MOST_TOKENS,
        "temperature": 0.1,
        "stop": stop,
        "stream": false,
    })
}

/// What goes at the cursor: the model's text cut at a stop it wrote anyway, without blank lines at its end, nor the
/// start of the code after the cursor written again at its end (only what the text closes more than it opens: a ")"
/// the code after the cursor has already, never a bracket of its own). Empty: nothing worth showing.
pub fn tidy(model: &str, text: &str, suffix: &str) -> String {
    let t = template(model);
    let cut = t.stop.iter().filter_map(|s| text.find(s)).min().unwrap_or(text.len());
    let text = text[..cut].trim_end();
    if text.trim().is_empty() {
        return String::new();
    }
    let next = suffix.trim_start_matches([' ', '\t']);
    let most = text.len().min(next.len()).min(40);
    for n in (1..=most).rev() {
        if !next.is_char_boundary(n) {
            continue;
        }
        let overlap = &next[..n];
        if text.ends_with(overlap) && !overlap.trim().is_empty() && unopened(text, overlap) {
            return text[..text.len() - n].trim_end().to_string();
        }
    }
    text.to_string()
}

/// Whether each bracket the overlap closes is one the text closes more often than it opens (written twice, not its
/// own).
fn unopened(text: &str, overlap: &str) -> bool {
    let count = |s: &str, c: char| s.chars().filter(|&x| x == c).count() as isize;
    [('(', ')'), ('[', ']'), ('{', '}')].into_iter().all(|(open, close)| {
        let closes = count(overlap, close);
        closes == 0 || count(text, close) - count(text, open) >= closes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_models_family_gives_its_tokens_and_any_other_is_qwens() {
        assert_eq!(template("deepseek-coder-6.7b").name, "deepseek");
        assert_eq!(template("bigcode/StarCoder2-15B").name, "starcoder");
        assert_eq!(template("CodeLlama-13b").name, "codellama");
        assert_eq!(template("qwen2.5-coder").name, "qwen");
        assert_eq!(template("gpt-oss").name, "qwen");
        assert!(template("deepseek").prefix.contains('\u{2581}'), "DeepSeek's bars and low line are its own");
    }

    #[test]
    fn the_prompt_has_the_path_and_the_code_cut_to_its_most() {
        let long: String = "é".repeat(MOST_PREFIX + 10);
        let b = body("qwen", "src/a.rs", &format!("Z{long}"), &"y".repeat(MOST_SUFFIX + 5));
        let prompt = b["prompt"].as_str().expect("a prompt");
        assert!(prompt.starts_with("<|fim_prefix|>// src/a.rs\n"));
        assert!(prompt.ends_with("<|fim_middle|>"));
        assert!(!prompt.contains('Z'), "only the last characters before the cursor");
        assert_eq!(prompt.matches('é').count(), MOST_PREFIX);
        assert_eq!(prompt.matches('y').count(), MOST_SUFFIX);
        assert_eq!(b["max_tokens"], 96);
        assert!(b["stop"].as_array().expect("stops").iter().any(|s| s == "\n\n\n"));
        assert_eq!(tail("abc", 0), "");
        assert_eq!(tail("abc", 5), "abc");
        assert_eq!(head("abc", 2), "ab");
    }

    #[test]
    fn the_answer_is_tidied() {
        assert_eq!(tidy("qwen", "a, b)\n\n", ")\n}"), "a, b");
        // A bracket the text opened itself stays.
        assert_eq!(tidy("qwen", "bar()", ")"), "bar()");
        assert_eq!(tidy("qwen", "x + 1<|endoftext|>junk", ""), "x + 1");
        assert_eq!(tidy("qwen", "  \n\t", ""), "");
        assert_eq!(tidy("qwen", "let a = 1;", "  let b"), "let a = 1;");
        // Cut on a letter's boundary.
        assert_eq!(tidy("qwen", "aé", "éb"), "a");
    }
}
