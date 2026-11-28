//! A key rule's `when`: its expression read once, when the rule is, and tested at each key pressed against the
//! context keys as they are then (`mod.rs` says which there are).
//!
//! The grammar is VS Code's: a key alone (true when its value is), `!`, `&&` (before `||`), `||`, brackets,
//! `true` and `false`, `key == value` and `!=` (also `===`, `!==`), `<`, `<=`, `>`, `>=` (numbers),
//! `key =~ /regex/flags`, `key in other` and `key not in other` (the other's value is a list or an object).
//! A value is a word or a quoted text ('a b'). What cannot be read is said, with where, and the rule is not taken.
//! What is here is pure: it is tested without a window.

use regex::Regex;
use serde_json::Value;

use crate::state::{Send, Ui};

/// An expression read.
#[derive(Clone, Debug)]
pub enum Expr {
    True,
    False,
    /// A key alone: its value is true.
    Has(String),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Equals(String, String),
    NotEquals(String, String),
    /// A number compared: `<` `<=` `>` `>=`.
    Compare(String, Cmp, f64),
    Matches(String, Regex),
    In(String, String),
    NotIn(String, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// Whether a context key's value counts as true: true, a number not 0, a text not empty, a list or an object.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A value as a `==` compares it: a text as it is, true and false and numbers as written.
fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

impl Expr {
    /// Whether it holds, the context keys read by `key`.
    pub fn holds(&self, key: &dyn Fn(&str) -> Value) -> bool {
        match self {
            Expr::True => true,
            Expr::False => false,
            Expr::Has(k) => truthy(&key(k)),
            Expr::Not(e) => !e.holds(key),
            Expr::And(all) => all.iter().all(|e| e.holds(key)),
            Expr::Or(any) => any.iter().any(|e| e.holds(key)),
            Expr::Equals(k, v) => equals(&key(k), v),
            Expr::NotEquals(k, v) => !equals(&key(k), v),
            Expr::Compare(k, cmp, n) => {
                let v = key(k);
                let Some(x) = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) else {
                    return false;
                };
                match cmp {
                    Cmp::Less => x < *n,
                    Cmp::LessOrEqual => x <= *n,
                    Cmp::Greater => x > *n,
                    Cmp::GreaterOrEqual => x >= *n,
                }
            }
            Expr::Matches(k, re) => re.is_match(&text_of(&key(k))),
            Expr::In(k, other) => is_in(&key(k), &key(other)),
            Expr::NotIn(k, other) => !is_in(&key(k), &key(other)),
        }
    }
}

/// `key == value`: `true` and `false` as the key's truth, else the texts the same.
fn equals(v: &Value, word: &str) -> bool {
    match word {
        "true" => truthy(v),
        "false" => !truthy(v),
        _ => text_of(v) == word,
    }
}

fn is_in(v: &Value, list: &Value) -> bool {
    let item = text_of(v);
    match list {
        Value::Array(a) => a.iter().any(|x| text_of(x) == item),
        Value::Object(o) => o.contains_key(&item),
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Word(String),
    Quoted(String),
    Regex(String, String),
    Not,
    And,
    Or,
    Open,
    Close,
    Op(&'static str),
}

/// The expression's words and signs; an error says what could not be read, and where.
fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        let two = |a: char, b: char| c == a && next == Some(b);
        if c.is_whitespace() {
            at += 1;
        } else if two('&', '&') {
            out.push(Token::And);
            at += 2;
        } else if two('|', '|') {
            out.push(Token::Or);
            at += 2;
        } else if two('=', '~') {
            out.push(Token::Op("=~"));
            at += 2;
            // The regular expression after it, between slashes, then its flags.
            while chars.get(at).is_some_and(|c| c.is_whitespace()) {
                at += 1;
            }
            if chars.get(at) != Some(&'/') {
                return Err(format!("a regular expression between slashes was expected at {}", at + 1));
            }
            let mut body = String::new();
            at += 1;
            loop {
                match chars.get(at) {
                    None => return Err("the regular expression is not closed with '/'".into()),
                    Some('\\') => {
                        body.push('\\');
                        if let Some(c) = chars.get(at + 1) {
                            body.push(*c);
                        }
                        at += 2;
                    }
                    Some('/') => {
                        at += 1;
                        break;
                    }
                    Some(c) => {
                        body.push(*c);
                        at += 1;
                    }
                }
            }
            let mut flags = String::new();
            while let Some(c) = chars.get(at).filter(|c| c.is_ascii_alphabetic()) {
                flags.push(*c);
                at += 1;
            }
            out.push(Token::Regex(body, flags));
        } else if c == '=' || c == '!' && next == Some('=') {
            // ==, ===, !=, !==.
            let not = c == '!';
            let mut n = 1;
            while chars.get(at + n) == Some(&'=') {
                n += 1;
            }
            let equals = if not { n - 1 } else { n };
            if !(1..=3).contains(&n) || (!not && equals < 2) {
                return Err(format!("'{}' cannot be read at {}", chars[at..at + n].iter().collect::<String>(), at + 1));
            }
            out.push(Token::Op(if not { "!=" } else { "==" }));
            at += n;
        } else if c == '!' {
            out.push(Token::Not);
            at += 1;
        } else if c == '<' || c == '>' {
            let eq = next == Some('=');
            out.push(Token::Op(match (c, eq) {
                ('<', false) => "<",
                ('<', true) => "<=",
                (_, false) => ">",
                _ => ">=",
            }));
            at += 1 + eq as usize;
        } else if c == '(' {
            out.push(Token::Open);
            at += 1;
        } else if c == ')' {
            out.push(Token::Close);
            at += 1;
        } else if c == '\'' || c == '"' {
            let end = chars[at + 1..].iter().position(|x| *x == c).ok_or("a quoted text is not closed")?;
            out.push(Token::Quoted(chars[at + 1..at + 1 + end].iter().collect()));
            at += end + 2;
        } else {
            let start = at;
            while at < chars.len() && !chars[at].is_whitespace() && !"!=<>()&|'\"~".contains(chars[at]) {
                at += 1;
            }
            if at == start {
                return Err(format!("'{c}' cannot be read at {}", at + 1));
            }
            out.push(Token::Word(chars[start..at].iter().collect()));
        }
    }
    Ok(out)
}

struct Reader {
    tokens: Vec<Token>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut any = vec![self.and()?];
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            any.push(self.and()?);
        }
        Ok(if any.len() == 1 { any.remove(0) } else { Expr::Or(any) })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut all = vec![self.not()?];
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            all.push(self.not()?);
        }
        Ok(if all.len() == 1 { all.remove(0) } else { Expr::And(all) })
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Token::Not) {
            self.at += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }

    /// A value after an operator: a word or a quoted text.
    fn value(&mut self, op: &str) -> Result<String, String> {
        match self.next() {
            Some(Token::Word(w) | Token::Quoted(w)) => Ok(w),
            _ => Err(format!("a value was expected after '{op}'")),
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::Open) => {
                let e = self.or()?;
                match self.next() {
                    Some(Token::Close) => Ok(e),
                    _ => Err("')' was expected".into()),
                }
            }
            Some(Token::Word(w)) if w == "true" => Ok(Expr::True),
            Some(Token::Word(w)) if w == "false" => Ok(Expr::False),
            Some(Token::Word(key)) => match self.peek().cloned() {
                Some(Token::Op(op)) => {
                    self.at += 1;
                    match op {
                        "==" => Ok(Expr::Equals(key, self.value(op)?)),
                        "!=" => Ok(Expr::NotEquals(key, self.value(op)?)),
                        "=~" => match self.next() {
                            Some(Token::Regex(body, flags)) => {
                                let mut pattern = String::new();
                                // VS Code's flags: i, s, m are the regex crate's too; g, u, y change nothing here.
                                let known: String = flags.chars().filter(|f| "ism".contains(*f)).collect();
                                if !known.is_empty() {
                                    pattern.push_str(&format!("(?{known})"));
                                }
                                pattern.push_str(&body);
                                Regex::new(&pattern)
                                    .map(|re| Expr::Matches(key, re))
                                    .map_err(|e| format!("/{body}/ is not a regular expression: {e}"))
                            }
                            _ => Err("a regular expression was expected after '=~'".into()),
                        },
                        cmp => {
                            let v = self.value(cmp)?;
                            let n: f64 = v.parse().map_err(|_| format!("'{v}' after '{cmp}' is not a number"))?;
                            let cmp = match cmp {
                                "<" => Cmp::Less,
                                "<=" => Cmp::LessOrEqual,
                                ">" => Cmp::Greater,
                                _ => Cmp::GreaterOrEqual,
                            };
                            Ok(Expr::Compare(key, cmp, n))
                        }
                    }
                }
                Some(Token::Word(w)) if w == "in" => {
                    self.at += 1;
                    Ok(Expr::In(key, self.value("in")?))
                }
                Some(Token::Word(w)) if w == "not" => {
                    self.at += 1;
                    match self.next() {
                        Some(Token::Word(w)) if w == "in" => Ok(Expr::NotIn(key, self.value("not in")?)),
                        _ => Err("'in' was expected after 'not'".into()),
                    }
                }
                _ => Ok(Expr::Has(key)),
            },
            Some(Token::Quoted(q)) => Err(format!("'{q}' is a value where a context key was expected")),
            Some(t) => Err(format!("{} was not expected", shown(&t))),
            None => Err("the expression ends too soon".into()),
        }
    }
}

fn shown(t: &Token) -> String {
    match t {
        Token::Word(w) => format!("'{w}'"),
        Token::Quoted(q) => format!("'{q}'"),
        Token::Regex(b, _) => format!("/{b}/"),
        Token::Not => "'!'".into(),
        Token::And => "'&&'".into(),
        Token::Or => "'||'".into(),
        Token::Open => "'('".into(),
        Token::Close => "')'".into(),
        Token::Op(o) => format!("'{o}'"),
    }
}

/// A `when` read; an empty one always holds.
pub fn parse(text: &str) -> Result<Expr, String> {
    let tokens = tokens(text)?;
    if tokens.is_empty() {
        return Ok(Expr::True);
    }
    let mut r = Reader { tokens, at: 0 };
    let e = r.or()?;
    match r.peek() {
        None => Ok(e),
        Some(t) => Err(format!("{} was not expected", shown(t))),
    }
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: Ui, _send: &Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn holds(when: &str, ctx: Value) -> bool {
        let e = parse(when).unwrap_or_else(|e| panic!("{when}: {e}"));
        e.holds(&|k| ctx.get(k).cloned().unwrap_or(Value::Null))
    }

    #[test]
    fn keys_alone_not_and_or_and_brackets_are_read_as_vs_code_reads_them() {
        let ctx = json!({"editorTextFocus": true, "inDebugMode": false, "name": "", "n": 0});
        assert!(holds("editorTextFocus", ctx.clone()));
        assert!(!holds("inDebugMode", ctx.clone()));
        assert!(!holds("missing", ctx.clone()));
        assert!(!holds("name", ctx.clone()), "an empty text is false");
        assert!(!holds("n", ctx.clone()));
        assert!(holds("!inDebugMode", ctx.clone()));
        assert!(holds("!!editorTextFocus", ctx.clone()));
        assert!(holds("", ctx.clone()));
        assert!(holds("true", ctx.clone()));
        assert!(!holds("false || inDebugMode", ctx.clone()));
        // && binds tighter than ||.
        assert!(holds("inDebugMode && false || editorTextFocus", ctx.clone()));
        assert!(!holds("inDebugMode && (false || editorTextFocus)", ctx.clone()));
        assert!(holds("editorTextFocus && !inDebugMode && !missing", ctx));
    }

    #[test]
    fn values_are_compared_matched_and_looked_up() {
        let ctx = json!({
            "resourceLangId": "python",
            "debugState": "stopped",
            "count": 3,
            "resourceFilename": "Main.RS",
            "list": ["a", "b c"],
            "map": {"x": 1},
            "item": "b c",
            "other": "x",
            "on": true,
        });
        assert!(holds("resourceLangId == python", ctx.clone()));
        assert!(holds("resourceLangId == 'python'", ctx.clone()));
        assert!(holds("resourceLangId === python", ctx.clone()));
        assert!(holds("resourceLangId != rust", ctx.clone()));
        assert!(holds("resourceLangId !== rust", ctx.clone()));
        assert!(holds("debugState == 'stopped' && resourceLangId != markdown", ctx.clone()));
        assert!(holds("on == true", ctx.clone()));
        assert!(holds("missing == false", ctx.clone()));
        assert!(holds("count > 2 && count >= 3 && count < 4 && count <= 3", ctx.clone()));
        assert!(!holds("count > 3", ctx.clone()));
        assert!(!holds("resourceLangId > 3", ctx.clone()), "a text is no number");
        assert!(holds("resourceFilename =~ /\\.rs$/i", ctx.clone()));
        assert!(!holds("resourceFilename =~ /\\.rs$/", ctx.clone()));
        assert!(holds("resourceFilename =~ /^Main\\./", ctx.clone()));
        assert!(holds("item in list", ctx.clone()));
        assert!(holds("other in map", ctx.clone()));
        assert!(!holds("resourceLangId in list", ctx.clone()));
        assert!(holds("resourceLangId not in list", ctx.clone()));
        assert!(!holds("item not in list", ctx));
    }

    #[test]
    fn what_cannot_be_read_is_said() {
        for bad in [
            "a &&",
            "(a",
            "a)",
            "a == ",
            "a =~ b",
            "a =~ /(/",
            "a < x",
            "a not b",
            "&& a",
            "'quoted'",
            "a = b",
            "a 'b'",
        ] {
            assert!(parse(bad).is_err(), "{bad} was read");
        }
        assert!(parse("a =~ /x/").is_ok());
        assert!(parse("config.editor.wordWrap == 'on' && view == workbench.panel.markers").is_ok());
        assert!(parse("activeViewlet == 'workbench.view.explorer'").is_ok());
    }
}
