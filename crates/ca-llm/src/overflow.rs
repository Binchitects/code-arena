//! A refusal because the prompt is bigger than the engine's window, read from its words as llama.cpp, LiteLLM, vLLM,
//! SGLang and OpenAI put them (the same reading as Arena's own `ContextOverflow`). llama.cpp's "Context size has been
//! exceeded." is another thing: the cache its requests share filled up (others filled it), not this prompt alone.

use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overflow {
    /// The engine's shared cache was full: asked again shortly, it may fit.
    pub shared_cache: bool,
    /// The prompt's tokens as the engine counted them, when it said.
    pub prompt: Option<u64>,
    /// The engine's window, when it said (llama.cpp's n_ctx).
    pub context: Option<u64>,
}

fn re(p: &str) -> Regex {
    Regex::new(p).expect("a regex of ours")
}

static WORDS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)ContextWindowExceeded|exceed_context|exceeds? (the )?(available |maximum )?context|context (size|length|window)|n_ctx|maximum context|prompt is too long|too many (input |prompt )?tokens",
    )
});
static SHARED: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)Context size has been exceeded"));
static LLAMA: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)request \((\d+) tokens\) exceeds the available context size \((\d+) tokens\)"));
static PROMPT_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"n_prompt_tokens\W{0,4}(\d+)"));
static CONTEXT_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"n_ctx\W{0,4}(\d+)"));
static STATED_PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    re(r"prompt contains (?:at least )?(\d+) input tokens|(\d+) tokens from the input messages|\((\d+) in the messages")
});
static STATED_CONTEXT: LazyLock<Regex> = LazyLock::new(|| re(r"maximum context length (?:is|of) (\d+) tokens"));

fn number(c: Option<regex::Captures<'_>>) -> Option<u64> {
    let c = c?;
    c.iter().skip(1).flatten().find_map(|m| m.as_str().parse().ok()).filter(|n: &u64| *n > 0)
}

/// What a refusal's text (its message or whole body) says of the window; `None` when it says nothing of it.
pub fn parse(text: &str) -> Option<Overflow> {
    if !WORDS.is_match(text) {
        return None;
    }
    let (mut prompt, mut context) = (None, None);
    if let Some(c) = LLAMA.captures(text) {
        prompt = c[1].parse().ok();
        context = c[2].parse().ok();
    }
    let prompt = prompt.or_else(|| number(PROMPT_KEY.captures(text))).or_else(|| number(STATED_PROMPT.captures(text)));
    let context =
        context.or_else(|| number(CONTEXT_KEY.captures(text))).or_else(|| number(STATED_CONTEXT.captures(text)));
    Some(Overflow { shared_cache: SHARED.is_match(text), prompt, context })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engines_words_read() {
        let l = parse(r#"{"error":{"code":400,"message":"request (40000 tokens) exceeds the available context size (20480 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":40000,"n_ctx":20480}}"#)
            .expect("llama.cpp");
        assert_eq!(l, Overflow { shared_cache: false, prompt: Some(40_000), context: Some(20_480) });
        let s = parse("Context size has been exceeded.").expect("shared");
        assert!(s.shared_cache && s.context.is_none());
        let o = parse("This model's maximum context length is 32768 tokens. However, your messages resulted in 40000 tokens (39000 in the messages, 1000 in the functions).")
            .expect("openai");
        assert_eq!((o.prompt, o.context), (Some(39_000), Some(32_768)));
        assert!(parse("litellm.ContextWindowExceededError: too long").is_some());
        assert_eq!(parse("rate limited"), None);
    }
}
