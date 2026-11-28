//! Each tool's own settings, as Configure Tools keeps them: on or off, how its calls ask, what the model is told of
//! it in place of its own words, arguments it always gets (the model is not offered them) and defaults for those
//! the model leaves out.
//!
//! They are kept at three levels: the person's for every project, the person's for this project only (on this
//! computer, never in the project), and the project's own (committed, in `.code-arena/settings.json`). The
//! person's for this project win over theirs for every project; the project's may only turn a tool off or make it
//! ask, never let one run unasked or change what the model is told.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ca_llm::ToolDef;
use ca_tools::{Context, Kind, Output, Tool};
use serde_json::{Map, Value, json};

/// How a tool's calls ask, set for it: every call, or none. Unset, they ask as the mode says of its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Ask,
    Allow,
}

impl Approval {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "ask" => Some(Approval::Ask),
            "allow" => Some(Approval::Allow),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Approval::Ask => "ask",
            Approval::Allow => "allow",
        }
    }
}

/// One tool's settings at one level. What is not set is left to the level below, and in the end to the tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choice {
    pub enabled: Option<bool>,
    pub approval: Option<Approval>,
    pub description: Option<String>,
    pub defaults: Map<String, Value>,
    pub fixed: Map<String, Value>,
}

impl Choice {
    /// From its JSON, `{"enabled": false, "approval": "ask", "description": "…", "defaults": {…}, "fixed": {…}}`;
    /// what is not of that shape is not read.
    pub fn from_value(v: &Value) -> Choice {
        let map = |k: &str| v.get(k).and_then(Value::as_object).cloned().unwrap_or_default();
        Choice {
            enabled: v.get("enabled").and_then(Value::as_bool),
            approval: v.get("approval").and_then(Value::as_str).and_then(Approval::parse),
            description: v.get("description").and_then(Value::as_str).filter(|d| !d.trim().is_empty()).map(String::from),
            defaults: map("defaults"),
            fixed: map("fixed"),
        }
    }

    /// As it is kept: only what is set.
    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        if let Some(e) = self.enabled {
            o.insert("enabled".into(), json!(e));
        }
        if let Some(a) = self.approval {
            o.insert("approval".into(), json!(a.name()));
        }
        if let Some(d) = &self.description {
            o.insert("description".into(), json!(d));
        }
        if !self.defaults.is_empty() {
            o.insert("defaults".into(), Value::Object(self.defaults.clone()));
        }
        if !self.fixed.is_empty() {
            o.insert("fixed".into(), Value::Object(self.fixed.clone()));
        }
        Value::Object(o)
    }

    pub fn is_empty(&self) -> bool {
        *self == Choice::default()
    }

    /// What a project's file may say of a tool: off, and asking before each call. The rest is not read, so a
    /// cloned repository cannot let a tool run unasked or tell the model something else of it.
    pub fn project_only(&self) -> Choice {
        Choice {
            enabled: (self.enabled == Some(false)).then_some(false),
            approval: (self.approval == Some(Approval::Ask)).then_some(Approval::Ask),
            ..Choice::default()
        }
    }
}

/// Every tool's settings at one level, by the tool's name.
pub type Choices = BTreeMap<String, Choice>;

/// The tools' settings of one level from its JSON: `{"read_file": {…}, …}`.
pub fn read(v: &Value) -> Choices {
    v.as_object()
        .into_iter()
        .flatten()
        .filter(|(_, c)| c.is_object())
        .map(|(k, c)| (k.clone(), Choice::from_value(c)))
        .collect()
}

/// A level's JSON with one tool's settings put in, or taken out when there are none.
pub fn with(all: &Value, name: &str, choice: &Choice) -> Value {
    let mut o = all.as_object().cloned().unwrap_or_default();
    o.remove(name);
    if !choice.is_empty() {
        o.insert(name.to_string(), choice.to_value());
    }
    Value::Object(o)
}

/// Why the person may not let `stop_command` run unasked.
pub const ALWAYS_ASKS: &str = "It asks every time: it stops what the person let run.";

/// Why the person may not let one of Arena's tools that asks first run unasked.
pub const ARENA_ASKS: &str = "Arena's admins set it to ask before each call.";

/// The tools' settings at the three levels.
#[derive(Debug, Clone, Default)]
pub struct Levels {
    pub person: Choices,
    pub here: Choices,
    /// Only what a project may say ([`Choice::project_only`]).
    pub project: Choices,
}

impl Levels {
    pub fn new(person: &Value, here: &Value, project: &Value) -> Levels {
        Levels {
            person: read(person),
            here: read(here),
            project: read(project).into_iter().map(|(k, c)| (k, c.project_only())).collect(),
        }
    }

    /// One tool's settings at each level: (the person's, theirs here, the project's).
    pub fn of(&self, name: &str) -> (Choice, Choice, Choice) {
        let at = |c: &Choices| c.get(name).cloned().unwrap_or_default();
        (at(&self.person), at(&self.here), at(&self.project))
    }

    /// The settings in force: the person's here over theirs everywhere, the project's off and ask over both. A
    /// tool that may not run unasked (`locked`) keeps asking as the mode says.
    pub fn effective(&self, name: &str, locked: bool) -> Choice {
        let (person, here, project) = self.of(name);
        let enabled = if project.enabled == Some(false) { Some(false) } else { here.enabled.or(person.enabled) };
        let approval =
            project.approval.or(here.approval).or(person.approval).filter(|a| !(locked && *a == Approval::Allow));
        let merge = |a: &Map<String, Value>, b: &Map<String, Value>| {
            let mut m = a.clone();
            m.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
            m
        };
        Choice {
            enabled,
            approval,
            description: here.description.or(person.description),
            defaults: merge(&person.defaults, &here.defaults),
            fixed: merge(&person.fixed, &here.fixed),
        }
    }
}

/// A tool as the person's settings make it: told of in their words, its fixed arguments not offered and given, its
/// defaults filling what the model leaves out.
pub struct Configured {
    inner: Arc<dyn Tool>,
    description: Option<String>,
    defaults: Map<String, Value>,
    fixed: Map<String, Value>,
}

impl Configured {
    /// The model's arguments with the defaults filled in and the fixed ones put over them.
    fn args(&self, args: Value) -> Value {
        let mut given = match args {
            Value::Object(o) => o,
            Value::Null => Map::new(),
            other => return other,
        };
        for (k, v) in &self.defaults {
            if given.get(k).is_none_or(Value::is_null) {
                given.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in &self.fixed {
            given.insert(k.clone(), v.clone());
        }
        Value::Object(given)
    }
}

#[async_trait::async_trait]
impl Tool for Configured {
    fn def(&self) -> ToolDef {
        let mut d = self.inner.def();
        if let Some(text) = &self.description {
            d.description = text.clone();
        }
        if let Some(props) = d.parameters.get_mut("properties").and_then(Value::as_object_mut) {
            for k in self.fixed.keys() {
                props.remove(k);
            }
            for (k, v) in &self.defaults {
                if let Some(p) = props.get_mut(k).and_then(Value::as_object_mut) {
                    let said = p.get("description").and_then(Value::as_str).unwrap_or("").trim_end_matches('.');
                    let text = format!("{said} (default: {v})").trim().to_string();
                    p.insert("description".into(), json!(text));
                }
            }
        }
        if let Some(req) = d.parameters.get_mut("required").and_then(Value::as_array_mut) {
            // Nor is one with a default required of the model: left out, it is filled.
            req.retain(|r| r.as_str().is_none_or(|n| !self.fixed.contains_key(n) && !self.defaults.contains_key(n)));
        }
        d
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn subject(&self, args: &Value) -> String {
        self.inner.subject(&self.args(args.clone()))
    }

    async fn run(&self, args: Value, ctx: &Context) -> Output {
        self.inner.run(self.args(args), ctx).await
    }

    fn group(&self) -> String {
        self.inner.group()
    }

    fn arena_tool(&self) -> Option<String> {
        self.inner.arena_tool()
    }

    fn own(&self) -> Option<Arc<dyn Tool>> {
        Some(self.inner.clone())
    }

    fn given(&self, args: Value) -> Value {
        self.args(args)
    }
}

/// The tool as it is defined, whatever settings were applied to it before.
pub fn base(t: &Arc<dyn Tool>) -> Arc<dyn Tool> {
    t.own().unwrap_or_else(|| t.clone())
}

/// The tool as its settings in force make it; itself when they change nothing of its definition. Arguments it
/// does not have are left out.
pub fn configure(tool: &Arc<dyn Tool>, c: &Choice) -> Arc<dyn Tool> {
    let tool = base(tool);
    let def = tool.def();
    let has = |k: &String| def.parameters.get("properties").and_then(|p| p.get(k)).is_some();
    let fixed: Map<String, Value> = c.fixed.iter().filter(|(k, _)| has(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let defaults: Map<String, Value> = c
        .defaults
        .iter()
        .filter(|(k, _)| has(k) && !fixed.contains_key(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if c.description.is_none() && fixed.is_empty() && defaults.is_empty() {
        return tool;
    }
    Arc::new(Configured { inner: tool, description: c.description.clone(), defaults, fixed })
}

/// How each tool's calls ask, as its settings in force say, for [`crate::Policy`].
pub type Approvals = HashMap<String, Approval>;

/// Why a saved choice is not taken, or none: "allow" for a tool that may not run unasked, an argument the tool
/// does not have, a description past 4,000 characters.
pub fn refusal(name: &str, choice: &Choice, def: &ToolDef, locked: Option<&str>) -> Option<String> {
    if choice.approval == Some(Approval::Allow)
        && let Some(why) = locked
    {
        return Some(format!("{name} cannot run unasked: {why}"));
    }
    let props = def.parameters.get("properties");
    if let Some(k) = choice.fixed.keys().chain(choice.defaults.keys()).find(|k| props.and_then(|p| p.get(*k)).is_none())
    {
        return Some(format!("{name} has no argument {k}."));
    }
    if choice.description.as_ref().is_some_and(|d| d.chars().count() > 4000) {
        return Some("A tool's description is at most 4,000 characters.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn def(&self) -> ToolDef {
            ToolDef {
                name: "echo".into(),
                description: "Says it back.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "What to say."},
                        "times": {"type": "integer", "description": "How often."},
                        "loud": {"type": "boolean"}
                    },
                    "required": ["text", "times"]
                }),
            }
        }
        fn kind(&self) -> Kind {
            Kind::Edit
        }
        fn subject(&self, args: &Value) -> String {
            args.to_string()
        }
        async fn run(&self, args: Value, _ctx: &Context) -> Output {
            Output::ok(args.to_string())
        }
    }

    fn choice(v: Value) -> Choice {
        Choice::from_value(&v)
    }

    #[test]
    fn a_fixed_argument_is_not_offered_and_a_default_is_not_required() {
        let t: Arc<dyn Tool> = Arc::new(Echo);
        let c = choice(json!({"description": "Mine.", "fixed": {"loud": true, "nope": 1}, "defaults": {"times": 2}}));
        let t = configure(&t, &c);
        let d = t.def();
        assert_eq!(d.description, "Mine.");
        assert!(d.parameters["properties"].get("loud").is_none());
        assert_eq!(d.parameters["required"], json!(["text"]));
        assert_eq!(d.parameters["properties"]["times"]["description"], "How often (default: 2)");
        assert_eq!(base(&t).def().description, "Says it back.", "the tool as defined is kept under it");
    }

    #[test]
    fn fixed_beats_the_models_value_and_a_default_fills_only_what_is_missing() {
        let t = configure(&(Arc::new(Echo) as Arc<dyn Tool>), &choice(json!({"fixed": {"loud": true}, "defaults": {"times": 2}})));
        assert_eq!(t.given(json!({"text": "hi", "loud": false})), json!({"text": "hi", "loud": true, "times": 2}));
        assert_eq!(t.given(json!({"text": "hi", "times": 5})), json!({"text": "hi", "loud": true, "times": 5}));
        assert!(t.subject(&json!({"text": "hi"})).contains("\"loud\":true"));
    }

    #[test]
    fn settings_that_change_nothing_leave_the_tool_as_it_is() {
        let t: Arc<dyn Tool> = Arc::new(Echo);
        let same = configure(&t, &choice(json!({"enabled": true, "approval": "ask"})));
        assert!(same.own().is_none());
        let again = configure(&configure(&t, &choice(json!({"description": "A."}))), &Choice::default());
        assert!(again.own().is_none(), "applied anew from the tool as defined");
    }

    #[test]
    fn the_project_can_only_turn_off_or_make_ask() {
        let l = Levels::new(
            &json!({}),
            &json!({}),
            &json!({"echo": {"enabled": true, "approval": "allow", "description": "Run me.", "fixed": {"loud": true}},
                    "other": {"enabled": false, "approval": "ask"}}),
        );
        assert!(l.of("echo").2.is_empty());
        assert_eq!(l.of("other").2, Choice { enabled: Some(false), approval: Some(Approval::Ask), ..Choice::default() });
    }

    #[test]
    fn here_wins_over_everywhere_and_the_project_over_both() {
        let l = Levels::new(
            &json!({"echo": {"enabled": false, "approval": "allow", "description": "P.", "fixed": {"a": 1, "b": 1}}}),
            &json!({"echo": {"enabled": true, "description": "H.", "fixed": {"b": 2}}}),
            &json!({}),
        );
        let e = l.effective("echo", false);
        assert_eq!(e.enabled, Some(true));
        assert_eq!(e.approval, Some(Approval::Allow));
        assert_eq!(e.description.as_deref(), Some("H."));
        assert_eq!(Value::Object(e.fixed), json!({"a": 1, "b": 2}));
        assert_eq!(l.effective("echo", true).approval, None, "a locked tool never runs unasked");
        let off = Levels::new(&json!({}), &json!({"echo": {"enabled": true}}), &json!({"echo": {"enabled": false}}));
        assert_eq!(off.effective("echo", false).enabled, Some(false));
    }

    #[test]
    fn a_choice_is_kept_with_only_what_is_set() {
        let all = json!({"x": {"enabled": false}});
        let c = choice(json!({"approval": "ask", "defaults": {}, "description": "  "}));
        assert_eq!(with(&all, "echo", &c), json!({"x": {"enabled": false}, "echo": {"approval": "ask"}}));
        assert_eq!(with(&all, "x", &Choice::default()), json!({}));
    }

    #[test]
    fn a_choice_that_cannot_be_kept_says_why() {
        let d = Echo.def();
        let allow = choice(json!({"approval": "allow"}));
        assert_eq!(refusal("stop", &allow, &d, Some(ALWAYS_ASKS)).as_deref(), Some(&*format!("stop cannot run unasked: {ALWAYS_ASKS}")));
        assert_eq!(refusal("echo", &allow, &d, None), None);
        assert_eq!(refusal("echo", &choice(json!({"fixed": {"nope": 1}})), &d, None).as_deref(), Some("echo has no argument nope."));
    }
}
