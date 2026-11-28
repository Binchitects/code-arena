//! A snippet's body in VS Code's snippet syntax, read and filled in: tab stops (`$1`, `${1}`), placeholders
//! (`${1:text}`, nested), choices (`${1|one,two|}`), the final caret (`$0`), variables (`$TM_FILENAME`,
//! `${TM_SELECTED_TEXT:default}`) and transforms (`${TM_FILENAME/(.*)\\..+$/$1/}`, `${1/(.*)/${1:/upcase}/}`). What
//! does not read as one of them is its text (`$` alone, `${` not closed). Nothing here knows the window.

use std::collections::HashMap;
use std::ops::Range;

/// A piece of a body.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Text(String),
    /// A tab stop: its number, its placeholder (empty: none), its choices, and its transform (a mirror's).
    Stop {
        index: u32,
        inner: Vec<Node>,
        choices: Vec<String>,
        transform: Option<Transform>,
    },
    /// A variable: its name, its default (`${NAME:default}`), its transform.
    Var {
        name: String,
        inner: Option<Vec<Node>>,
        transform: Option<Transform>,
    },
}

/// `/regex/format/flags`.
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    pub regex: String,
    pub format: Vec<Fmt>,
    pub flags: String,
}

/// A piece of a transform's format.
#[derive(Clone, Debug, PartialEq)]
pub enum Fmt {
    Text(String),
    Group(usize, How),
}

/// How a group is put in the format.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    Plain,
    Upcase,
    Downcase,
    Capitalize,
    Camel,
    Pascal,
    /// `${1:+if}`: the text when the group matched.
    If(String),
    /// `${1:?if:else}`.
    IfElse(String, String),
    /// `${1:-else}` or `${1:else}`: the text when it did not.
    Else(String),
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn int(&mut self) -> Option<u32> {
        let from = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
        }
        (self.at > from).then(|| self.chars[from..self.at].iter().collect::<String>().parse().ok()).flatten()
    }

    fn name(&mut self) -> Option<String> {
        if !self.peek().is_some_and(|c| c == '_' || c.is_ascii_alphabetic()) {
            return None;
        }
        let from = self.at;
        while self.peek().is_some_and(|c| c == '_' || c.is_ascii_alphanumeric()) {
            self.at += 1;
        }
        Some(self.chars[from..self.at].iter().collect())
    }

    /// Pieces up to (not taking) a `}` when `inside`, else to the end.
    fn any(&mut self, inside: bool) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            match c {
                '}' if inside => break,
                '\\' if matches!(self.chars.get(self.at + 1), Some('$' | '}' | '\\')) => {
                    text.push(self.chars[self.at + 1]);
                    self.at += 2;
                }
                '$' => {
                    let from = self.at;
                    self.at += 1;
                    match self.dollar() {
                        Some(node) => {
                            if !text.is_empty() {
                                out.push(Node::Text(std::mem::take(&mut text)));
                            }
                            out.push(node);
                        }
                        None => {
                            self.at = from + 1;
                            text.push('$');
                        }
                    }
                }
                _ => {
                    text.push(c);
                    self.at += 1;
                }
            }
        }
        if !text.is_empty() {
            out.push(Node::Text(text));
        }
        out
    }

    /// What follows a `$`; none: it is text.
    fn dollar(&mut self) -> Option<Node> {
        if let Some(index) = self.int() {
            return Some(Node::Stop { index, inner: Vec::new(), choices: Vec::new(), transform: None });
        }
        if let Some(name) = self.name() {
            return Some(Node::Var { name, inner: None, transform: None });
        }
        if !self.eat('{') {
            return None;
        }
        if let Some(index) = self.int() {
            let mut stop = Node::Stop { index, inner: Vec::new(), choices: Vec::new(), transform: None };
            if self.eat('}') {
                return Some(stop);
            }
            if self.eat(':') {
                let inner = self.any(true);
                self.eat('}').then_some(())?;
                if let Node::Stop { inner: i, .. } = &mut stop {
                    *i = inner;
                }
                return Some(stop);
            }
            if self.eat('|') {
                let choices = self.choices()?;
                if let Node::Stop { choices: c, .. } = &mut stop {
                    *c = choices;
                }
                return Some(stop);
            }
            if self.eat('/') {
                let t = self.transform()?;
                if let Node::Stop { transform, .. } = &mut stop {
                    *transform = Some(t);
                }
                return Some(stop);
            }
            return None;
        }
        let name = self.name()?;
        if self.eat('}') {
            return Some(Node::Var { name, inner: None, transform: None });
        }
        if self.eat(':') {
            let inner = self.any(true);
            self.eat('}').then_some(())?;
            return Some(Node::Var { name, inner: Some(inner), transform: None });
        }
        if self.eat('/') {
            let t = self.transform()?;
            return Some(Node::Var { name, inner: None, transform: Some(t) });
        }
        None
    }

    /// `a,b,c|}` after `${1|`.
    fn choices(&mut self) -> Option<Vec<String>> {
        let mut out = Vec::new();
        let mut one = String::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                '\\' if matches!(self.peek(), Some(',' | '|' | '\\' | '$' | '}')) => {
                    one.push(self.peek()?);
                    self.at += 1;
                }
                ',' => out.push(std::mem::take(&mut one)),
                '|' => {
                    out.push(one);
                    return self.eat('}').then_some(out);
                }
                _ => one.push(c),
            }
        }
    }

    /// Text up to an unescaped `/` (taken), with `\/` read as `/`; other escapes are kept for the regex.
    fn to_slash(&mut self, keep_escapes: bool) -> Option<String> {
        let mut out = String::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                '/' => return Some(out),
                '\\' => {
                    let n = self.peek()?;
                    self.at += 1;
                    if n == '/' || !keep_escapes {
                        out.push(n);
                    } else {
                        out.push('\\');
                        out.push(n);
                    }
                }
                _ => out.push(c),
            }
        }
    }

    /// `regex/format/flags}` after the first `/`, the `}` taken.
    fn transform(&mut self) -> Option<Transform> {
        let regex = self.to_slash(true)?;
        let mut format = Vec::new();
        let mut text = String::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                '/' => break,
                '\\' => {
                    let n = self.peek()?;
                    self.at += 1;
                    match n {
                        'n' => text.push('\n'),
                        't' => text.push('\t'),
                        _ => text.push(n),
                    }
                }
                '$' => {
                    let back = self.at;
                    match self.format_group() {
                        Some(g) => {
                            if !text.is_empty() {
                                format.push(Fmt::Text(std::mem::take(&mut text)));
                            }
                            format.push(g);
                        }
                        None => {
                            self.at = back;
                            text.push('$');
                        }
                    }
                }
                _ => text.push(c),
            }
        }
        if !text.is_empty() {
            format.push(Fmt::Text(text));
        }
        let mut flags = String::new();
        while let Some(c) = self.peek() {
            self.at += 1;
            if c == '}' {
                return Some(Transform { regex, format, flags });
            }
            flags.push(c);
        }
        None
    }

    /// `1`, `{1}`, `{1:/upcase}`, `{1:+if}`, `{1:?if:else}`, `{1:-else}`, `{1:else}` after a `$` of a format.
    fn format_group(&mut self) -> Option<Fmt> {
        if let Some(n) = self.int() {
            return Some(Fmt::Group(n as usize, How::Plain));
        }
        if !self.eat('{') {
            return None;
        }
        let n = self.int()? as usize;
        if self.eat('}') {
            return Some(Fmt::Group(n, How::Plain));
        }
        if !self.eat(':') {
            return None;
        }
        let till = |r: &mut Reader, stop: &[char]| -> Option<String> {
            let mut s = String::new();
            loop {
                let c = r.peek()?;
                if stop.contains(&c) {
                    return Some(s);
                }
                r.at += 1;
                if c == '\\' {
                    s.push(r.peek()?);
                    r.at += 1;
                } else {
                    s.push(c);
                }
            }
        };
        let how = if self.eat('/') {
            let word = self.name()?;
            match word.as_str() {
                "upcase" => How::Upcase,
                "downcase" => How::Downcase,
                "capitalize" => How::Capitalize,
                "camelcase" => How::Camel,
                "pascalcase" => How::Pascal,
                _ => return None,
            }
        } else if self.eat('+') {
            How::If(till(self, &['}'])?)
        } else if self.eat('?') {
            let yes = till(self, &[':'])?;
            self.eat(':');
            How::IfElse(yes, till(self, &['}'])?)
        } else {
            self.eat('-');
            How::Else(till(self, &['}'])?)
        };
        self.eat('}').then_some(Fmt::Group(n, how))
    }
}

/// A body read into its pieces.
pub fn parse(body: &str) -> Vec<Node> {
    Reader { chars: body.chars().collect(), at: 0 }.any(false)
}

/// A word's letters with the first made capital and the rest left.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect()
}

/// A transform applied to a text (none matched: the text as it is).
pub fn apply(t: &Transform, text: &str) -> String {
    let flags: String = t.flags.chars().filter(|c| "imsx".contains(*c)).collect();
    let pattern = if flags.is_empty() { t.regex.clone() } else { format!("(?{flags}){}", t.regex) };
    let Ok(re) = regex::Regex::new(&pattern) else { return text.to_string() };
    let one = |caps: &regex::Captures| -> String {
        let mut out = String::new();
        for f in &t.format {
            match f {
                Fmt::Text(s) => out.push_str(s),
                Fmt::Group(n, how) => {
                    let g = caps.get(*n).map(|m| m.as_str()).filter(|g| !g.is_empty());
                    match (how, g) {
                        (How::Plain, Some(g)) => out.push_str(g),
                        (How::Upcase, Some(g)) => out.push_str(&g.to_uppercase()),
                        (How::Downcase, Some(g)) => out.push_str(&g.to_lowercase()),
                        (How::Capitalize, Some(g)) => out.push_str(&capital(g)),
                        (How::Pascal, Some(g)) => out.extend(words(g).into_iter().map(capital)),
                        (How::Camel, Some(g)) => {
                            for (i, w) in words(g).into_iter().enumerate() {
                                out.push_str(&if i == 0 { w.to_lowercase() } else { capital(w) });
                            }
                        }
                        (How::If(s) | How::IfElse(s, _), Some(_)) => out.push_str(s),
                        (How::IfElse(_, s) | How::Else(s), None) => out.push_str(s),
                        _ => {}
                    }
                }
            }
        }
        out
    };
    if t.flags.contains('g') {
        re.replace_all(text, |c: &regex::Captures| one(c)).into_owned()
    } else {
        re.replace(text, |c: &regex::Captures| one(c)).into_owned()
    }
}

/// A tab stop of a snippet put in: every place it is (the first is the one typed in, the others its mirrors), its
/// choices, and the places it is shown transformed (filled in when Tab leaves it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stop {
    pub index: u32,
    pub ranges: Vec<Range<usize>>,
    pub choices: Vec<String>,
    pub derived: Vec<(Range<usize>, Transform)>,
}

/// A body filled in: its text and its tab stops in the order Tab goes (1, 2, … then 0, the end).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expanded {
    pub text: String,
    pub stops: Vec<Stop>,
}

/// How a body is filled in: the variables' values (none: not a variable), the indent put after each new line, and
/// what a tab of the body becomes.
pub struct Fill<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub indent: &'a str,
    pub tab: &'a str,
}

struct Out<'a> {
    fill: &'a Fill<'a>,
    text: String,
    stops: HashMap<u32, Stop>,
    /// Each stop's placeholder: the first with text or choices.
    defaults: &'a HashMap<u32, Vec<Node>>,
    /// The number given to the next unknown variable (made a placeholder of its name).
    next: u32,
}

impl Out<'_> {
    fn text(&mut self, s: &str) {
        for (i, line) in s.split('\n').enumerate() {
            if i > 0 {
                self.text.push('\n');
                self.text.push_str(self.fill.indent);
            }
            self.text.push_str(&line.replace('\t', self.fill.tab));
        }
    }

    fn nodes(&mut self, nodes: &[Node], live: bool) {
        for n in nodes {
            self.node(n, live);
        }
    }

    /// One piece put out; `live`: its stops are stops (not inside a mirror's copy).
    fn node(&mut self, n: &Node, live: bool) {
        match n {
            Node::Text(s) => self.text(s),
            Node::Stop { index, inner, choices, transform } => {
                let from = self.text.len();
                let defaults = self.defaults;
                if !choices.is_empty() {
                    let first = choices[0].clone();
                    self.text(&first);
                } else if !inner.is_empty() {
                    self.nodes(inner, live);
                } else if let Some(d) = defaults.get(index) {
                    self.nodes(d, false);
                }
                // A mirror with a transform shows its placeholder transformed.
                if let Some(t) = transform {
                    let raw = self.text.split_off(from);
                    self.text.push_str(&apply(t, &raw));
                }
                if !live {
                    return;
                }
                let range = from..self.text.len();
                let stop = self.stops.entry(*index).or_insert_with(|| Stop { index: *index, ..Stop::default() });
                if stop.choices.is_empty() {
                    stop.choices = choices.clone();
                }
                match transform {
                    Some(t) => stop.derived.push((range, t.clone())),
                    None => stop.ranges.push(range),
                }
            }
            Node::Var { name, inner, transform } => {
                let value = (self.fill.var)(name);
                match (value, inner) {
                    (Some(v), _) if !v.is_empty() => {
                        let v = transform.as_ref().map_or(v.clone(), |t| apply(t, &v));
                        self.text.push_str(&v);
                    }
                    (_, Some(d)) => self.nodes(d, live),
                    (Some(_), None) => {}
                    (None, None) => {
                        // VS Code: an unknown variable's name is put in, as a placeholder.
                        let from = self.text.len();
                        self.text.push_str(name);
                        if live {
                            let index = self.next;
                            self.next += 1;
                            let range = from..self.text.len();
                            self.stops.insert(index, Stop { index, ranges: vec![range], ..Stop::default() });
                        }
                    }
                }
            }
        }
    }
}

fn find_defaults(nodes: &[Node], into: &mut HashMap<u32, Vec<Node>>, most: &mut u32) {
    for n in nodes {
        match n {
            Node::Stop { index, inner, choices, .. } => {
                *most = (*most).max(*index);
                if !choices.is_empty() && !into.contains_key(index) {
                    into.insert(*index, vec![Node::Text(choices[0].clone())]);
                } else if !inner.is_empty() {
                    into.entry(*index).or_insert_with(|| inner.clone());
                }
                find_defaults(inner, into, most);
            }
            Node::Var { inner: Some(inner), .. } => find_defaults(inner, into, most),
            _ => {}
        }
    }
}

/// A body filled in: the text to put in and where its tab stops are in it.
pub fn expand(body: &str, fill: &Fill) -> Expanded {
    let nodes = parse(body);
    let mut defaults = HashMap::new();
    let mut most = 0;
    find_defaults(&nodes, &mut defaults, &mut most);
    let mut out = Out { fill, text: String::new(), stops: HashMap::new(), defaults: &defaults, next: most + 1 };
    out.nodes(&nodes, true);
    let text = out.text;
    let mut stops: Vec<Stop> = out.stops.into_values().collect();
    // A stop only ever shown transformed is typed in nowhere: its first transformed place is its own.
    for s in &mut stops {
        if s.ranges.is_empty()
            && let Some((r, _)) = s.derived.first().cloned()
        {
            s.ranges.push(r);
            s.derived.remove(0);
        }
    }
    stops.sort_by_key(|s| if s.index == 0 { u32::MAX } else { s.index });
    if !stops.last().is_some_and(|s| s.index == 0) {
        stops.push(Stop { index: 0, ranges: vec![text.len()..text.len()], ..Stop::default() });
    }
    Expanded { text, stops }
}

/// A body as plain text, its stops' placeholders and variables' names in (the details pane's preview).
pub fn plain(body: &str) -> String {
    let var = |name: &str| Some(name.to_string());
    expand(body, &Fill { var: &var, indent: "", tab: "\t" }).text
}

/// Whether a body puts in the selection (`$TM_SELECTED_TEXT`, `$SELECTION`): Surround With Snippet lists it.
pub fn takes_selection(body: &str) -> bool {
    fn any(nodes: &[Node]) -> bool {
        nodes.iter().any(|n| match n {
            Node::Var { name, inner, .. } => {
                name == "TM_SELECTED_TEXT" || name == "SELECTION" || inner.as_deref().is_some_and(any)
            }
            Node::Stop { inner, .. } => any(inner),
            Node::Text(_) => false,
        })
    }
    any(&parse(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn run(body: &str) -> Expanded {
        let var = |n: &str| match n {
            "TM_FILENAME" => Some("main.rs".to_string()),
            "TM_SELECTED_TEXT" => Some(String::new()),
            _ => None,
        };
        expand(body, &Fill { var: &var, indent: "    ", tab: "  " })
    }

    fn stops(e: &Expanded) -> Vec<(u32, Vec<&str>)> {
        e.stops.iter().map(|s| (s.index, s.ranges.iter().map(|r| &e.text[r.clone()]).collect())).collect()
    }

    #[test]
    fn stops_placeholders_and_the_end_are_found_in_tab_order() {
        let e = run("for ${1:i} in ${2:0..n} {\n\t$0\n}");
        assert_eq!(e.text, "for i in 0..n {\n      \n    }");
        assert_eq!(stops(&e), [(1, vec!["i"]), (2, vec!["0..n"]), (0, vec![""])]);
        assert_eq!(e.stops[2].ranges[0].start, "for i in 0..n {\n      ".len());
    }

    #[test]
    fn a_body_without_its_end_ends_after_its_text() {
        let e = run("$2 then $1");
        assert_eq!(e.text, " then ");
        assert_eq!(e.stops.iter().map(|s| s.index).collect::<Vec<_>>(), [1, 2, 0]);
        assert_eq!(e.stops[2].ranges, [6..6]);
    }

    #[test]
    fn the_same_number_twice_is_a_mirror_of_its_placeholder() {
        let e = run("let ${1:name} = $1; ${1}");
        assert_eq!(e.text, "let name = name; name");
        assert_eq!(stops(&e)[0], (1, vec!["name", "name", "name"]));
    }

    #[test]
    fn placeholders_nest_and_choices_put_their_first() {
        let e = run("${1:a ${2:b}} ${3|one,two\\,x,three|}");
        assert_eq!(e.text, "a b one");
        assert_eq!(stops(&e), [(1, vec!["a b"]), (2, vec!["b"]), (3, vec!["one"]), (0, vec![""])]);
        assert_eq!(e.stops[2].choices, ["one", "two,x", "three"]);
    }

    #[test]
    fn variables_are_filled_in_defaulted_or_made_placeholders() {
        let e = run("$TM_FILENAME ${TM_SELECTED_TEXT:none} ${NOPE} ${TM_SELECTED_TEXT}.");
        assert_eq!(e.text, "main.rs none NOPE .");
        assert_eq!(stops(&e)[0], (1, vec!["NOPE"]));
    }

    #[test]
    fn what_is_not_syntax_stays_text() {
        let var = none;
        let e = expand("$ ${ \\$1 a}b ${1:x", &Fill { var: &var, indent: "", tab: "\t" });
        assert_eq!(e.text, "$ ${ $1 a}b ${1:x");
        assert_eq!(e.stops.len(), 1);
    }

    #[test]
    fn transforms_change_a_variable_and_a_mirror() {
        let e = run("${TM_FILENAME/(.*)\\..+$/$1/} ${TM_FILENAME/(\\w+)/${1:/upcase}/}");
        assert_eq!(e.text, "main MAIN.rs");
        let e = run("${1:my name} ${1/(.*)/${1:/pascalcase}/}");
        assert_eq!(e.text, "my name MyName");
        assert_eq!(e.stops[0].derived.len(), 1);
        let t = &e.stops[0].derived[0].1;
        assert_eq!(apply(t, "my name"), "MyName");
    }

    #[test]
    fn format_groups_upcase_camel_and_conditions() {
        let t = |body: &str| match &parse(body)[0] {
            Node::Var { transform: Some(t), .. } => t.clone(),
            n => panic!("{n:?}"),
        };
        assert_eq!(apply(&t("${X/(a)|(b)/${1:+A}${2:?B:-}/g}"), "ab"), "A-B");
        assert_eq!(apply(&t("${X/(.*)/${1:/camelcase}/}"), "some_long name"), "someLongName");
        assert_eq!(apply(&t("${X/(.*)/${1:/capitalize}/}"), "word"), "Word");
        assert_eq!(apply(&t("${X/(x)?.*/${1:-none}/}"), "abc"), "none");
        assert_eq!(apply(&t("${X/A/b/i}"), "aaa"), "baa");
        assert_eq!(apply(&t("${X/a/\\//g}"), "aa"), "//");
    }

    #[test]
    fn surround_lists_only_the_bodies_that_take_the_selection() {
        assert!(takes_selection("<b>$TM_SELECTED_TEXT</b>"));
        assert!(takes_selection("${1:${SELECTION}}"));
        assert!(!takes_selection("\\$TM_SELECTED_TEXT $1"));
        assert_eq!(plain("fn ${1:name}($2) -> $TM_FILENAME"), "fn name() -> TM_FILENAME");
    }
}
