//! The model's window: how full the next request is, measured from the gateway's own counts; what fills it, and
//! how it is freed before anything is summarized: big results kept aside, used results condensed, old ones
//! cleared; then the older part of the conversation summarized.

use ca_llm::{Message, Role, ToolDef};

/// Where a session stands against its model's window.
#[derive(Debug, Clone)]
pub struct Window {
    /// The model's window, in tokens.
    pub size: u64,
    /// Kept free for the answer: the model's longest answer, at most an eighth of the window.
    pub reserve: u64,
    /// Compacts when the next request passes this share of the window (percent); `None`: only when it would not fit.
    pub compact_at: Option<u64>,
    /// What a compaction keeps whole, as a share of the window (percent).
    pub target: u64,
    /// Characters per token, learned from the gateway's counts.
    pub chars_per_token: f64,
}

impl Default for Window {
    fn default() -> Self {
        Window { size: 32_768, reserve: 4_096, compact_at: Some(80), target: 25, chars_per_token: 3.5 }
    }
}

/// A picture counts as this many characters (about a thousand tokens), as the web chat counts one.
pub const PICTURE_CHARS: usize = 3_500;

impl Window {
    pub fn new(size: Option<u64>, max_output: Option<u64>, compact_at: Option<u64>, target: u64) -> Self {
        let size = size.filter(|s| *s > 0).unwrap_or(32_768);
        let reserve = max_output.unwrap_or(4_096).min(size / 8).max(256);
        Window { size, reserve, compact_at, target: target.clamp(5, 85), chars_per_token: 3.5 }
    }

    /// The most the next request may hold: the window less the room for the answer.
    pub fn limit(&self) -> u64 {
        self.size.saturating_sub(self.reserve)
    }

    /// Where it compacts, in tokens.
    pub fn threshold(&self) -> u64 {
        match self.compact_at {
            Some(p) => (self.size * p / 100).min(self.limit()),
            None => self.limit(),
        }
    }

    pub fn tokens(&self, chars: usize) -> u64 {
        (chars as f64 / self.chars_per_token).ceil() as u64
    }

    /// The characters a token held in the last request, as the gateway counted it.
    pub fn learn(&mut self, chars: usize, prompt_tokens: u64) {
        if prompt_tokens > 100 && chars > 0 {
            self.chars_per_token = (chars as f64 / prompt_tokens as f64).clamp(1.2, 8.0);
        }
    }

    /// The window the engine itself holds, when it says it is smaller than the gateway does.
    pub fn engine_holds(&mut self, tokens: u64) {
        if tokens > 1000 && tokens < self.size {
            self.size = tokens;
            self.reserve = self.reserve.min(tokens / 8);
        }
    }
}

/// Tokens as people read them: 20k, 32.8k, 900.
pub fn short_tokens(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let k = n as f64 / 1000.0;
    if k >= 100.0 || (k - k.round()).abs() < 0.05 { format!("{k:.0}k") } else { format!("{k:.1}k") }
}

/// The characters one message brings to a request.
pub fn message_chars(m: &Message) -> usize {
    m.content.len() + m.tool_calls.iter().map(|c| c.name.len() + c.arguments.len() + 30).sum::<usize>() + 12
}

/// The characters a request holds: the system prompt, the messages and the tools' definitions.
pub fn request_chars(system: &str, history: &[Message], tools: &[ToolDef]) -> usize {
    system.len()
        + history.iter().map(message_chars).sum::<usize>()
        + tools.iter().map(|t| t.name.len() + t.description.len() + t.parameters.to_string().len() + 30).sum::<usize>()
}

/// What fills a request, by kind, in characters: for `/context` and the IDE's ring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakdown {
    pub system: usize,
    pub tools: usize,
    pub you: usize,
    pub answers: usize,
    pub tool_results: usize,
    pub notes: usize,
    /// The biggest messages: (place in the history, role, characters, its start).
    pub biggest: Vec<(usize, &'static str, usize, String)>,
}

/// What fills the request, by kind, as text: for a tokenizer to count (the same kinds as [`breakdown`]).
pub fn kinds_text(system: &str, history: &[Message], notes: &[bool], tools: &[ToolDef]) -> [String; 6] {
    let mut k: [String; 6] = Default::default();
    k[0] = system.to_string();
    k[1] = tools.iter().map(|t| t.to_wire().to_string()).collect::<Vec<_>>().join("\n");
    for (i, m) in history.iter().enumerate() {
        let at = match m.role {
            ca_llm::Role::User if notes.get(i).copied().unwrap_or(false) => 5,
            ca_llm::Role::User | ca_llm::Role::System => 2,
            ca_llm::Role::Assistant => 3,
            ca_llm::Role::Tool => 4,
        };
        k[at].push_str(&m.content);
        for c in &m.tool_calls {
            k[at].push_str(&c.name);
            k[at].push_str(&c.arguments);
        }
        k[at].push('\n');
    }
    k
}

pub fn breakdown(system: &str, history: &[Message], notes: &[bool], tools: &[ToolDef]) -> Breakdown {
    let mut b = Breakdown {
        system: system.len(),
        tools: tools.iter().map(|t| t.name.len() + t.description.len() + t.parameters.to_string().len()).sum(),
        ..Default::default()
    };
    let mut sizes = Vec::new();
    for (i, m) in history.iter().enumerate() {
        let n = message_chars(m);
        let role = match m.role {
            _ if notes.get(i).copied().unwrap_or(false) => {
                b.notes += n;
                "note"
            }
            Role::User => {
                b.you += n;
                "you"
            }
            Role::Assistant => {
                b.answers += n;
                "answer"
            }
            Role::Tool => {
                b.tool_results += n;
                "tool result"
            }
            Role::System => {
                b.system += n;
                "system"
            }
        };
        sizes.push((i, role, n, m.content.chars().take(60).collect::<String>().replace('\n', " ")));
    }
    sizes.sort_by_key(|s| std::cmp::Reverse(s.2));
    sizes.truncate(5);
    b.biggest = sizes;
    b
}

/// Words that mark a line worth keeping when a result is condensed.
const KEY_WORDS: &[&str] = &["error", "warning", "failed", "panicked", "exception", "traceback", "fatal", "fail:"];

/// A used result as the model reads it from now on: its first lines, the lines that look like errors, its last
/// lines, and the handle that reads it whole again.
pub fn condensed(text: &str, handle: Option<&str>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep: Vec<usize> = (0..lines.len().min(12)).collect();
    for (i, l) in lines.iter().enumerate() {
        let lower = l.to_ascii_lowercase();
        if KEY_WORDS.iter().any(|w| lower.contains(w)) && keep.len() < 60 {
            keep.push(i);
        }
    }
    keep.extend(lines.len().saturating_sub(8)..lines.len());
    keep.sort_unstable();
    keep.dedup();
    let mut out =
        String::from("(Summarised: the model used this result already. Its key lines, with their numbers:)\n");
    let mut last = None;
    for i in keep {
        if let Some(l) = last
            && i > l + 1
        {
            out.push_str("   …\n");
        }
        let line: String = lines[i].chars().take(240).collect();
        out.push_str(&format!("{:>5}: {line}\n", i + 1));
        last = Some(i);
    }
    match handle {
        Some(h) => out.push_str(&format!("(It is kept whole as {h}: read_result {{\"id\":\"{h}\"}} reads it again.)")),
        None => out.push_str("(The rest is not kept: run the call again to see it whole.)"),
    }
    out
}

/// The request that asks for a summary of the older part of the conversation.
pub const SUMMARY_ASK: &str = "Summarize the conversation so far for yourself, to carry on the work from your summary alone: the \
person's goals, the decisions made and why, the files read and changed, the commands run and what they gave, the \
errors met, and where the work stands. Write it in these sections: ## Goal, ## Constraints and preferences, \
## Decisions, ## Files, ## Commands, ## Errors met, ## State, ## Next steps. Be complete but brief; quote exact \
names, paths and commands. Do not call any tool.";

/// Whether a summary went round in a loop (the same few words again and again).
pub fn looped(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 200 {
        return false;
    }
    let tail: std::collections::HashSet<&str> = words[words.len() - 200..].iter().copied().collect();
    tail.len() < 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_keeps_room_for_the_answer() {
        let w = Window::new(Some(131_072), Some(32_768), Some(80), 25);
        assert_eq!(w.reserve, 16_384, "at most an eighth of the window");
        assert_eq!(w.limit(), 114_688);
        assert_eq!(w.threshold(), 104_857);
        let off = Window::new(Some(32_768), None, None, 25);
        assert_eq!(off.threshold(), off.limit());
    }

    #[test]
    fn it_learns_the_characters_a_token_holds() {
        let mut w = Window::default();
        w.learn(40_000, 10_000);
        assert!((w.chars_per_token - 4.0).abs() < 1e-9);
        assert_eq!(w.tokens(4_000), 1_000);
        w.learn(10, 0);
        assert!((w.chars_per_token - 4.0).abs() < 1e-9, "no count, nothing learned");
        w.engine_holds(20_000);
        assert_eq!(
            (short_tokens(20_480), short_tokens(32_768), short_tokens(20_000), short_tokens(900)),
            ("20.5k".into(), "32.8k".into(), "20k".into(), "900".into())
        );
        assert_eq!(w.size, 20_000);
    }

    #[test]
    fn a_condensed_result_keeps_its_key_lines() {
        let mut text = String::new();
        for i in 1..=300 {
            text.push_str(&format!("line {i}\n"));
            if i == 150 {
                text.push_str("error[E0425]: cannot find value `totl`\n");
            }
        }
        let c = condensed(&text, Some("r4"));
        assert!(c.contains("    1: line 1"));
        assert!(c.contains("  151: error[E0425]: cannot find value `totl`"));
        assert!(c.contains("  301: line 300"));
        assert!(c.ends_with("read_result {\"id\":\"r4\"} reads it again.)"));
        assert!(c.len() < text.len() / 3);
    }

    #[test]
    fn a_summary_in_a_loop_is_seen() {
        assert!(looped(&"and then ".repeat(150)));
        assert!(!looped(&(0..300).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ")));
    }
}
