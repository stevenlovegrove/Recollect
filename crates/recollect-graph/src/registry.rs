//! Rules that can be written down. A rule is JSON under a name, like a component: its key says
//! what code it is, its fields are its parameters, and the Rust struct is the typed shortcut.
//! A registry maps keys back to code, so a saved cell can say which rule backs it.
//!
//! Registration is explicit. A system is just a function that adds to a `Registry`, so what's
//! active can be read from one place.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Backing, Cell, Component, Cx, Entity, Error, Inputs, Key, Rule, RuleSpec, erase_runtime, erase_typed, typed_backing};
/// How a rule is written down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedRule {
    pub rule: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

/// What `#[rule]` provides.
pub trait RuleMeta {
    const KEY: &'static str;
    fn declared_inputs() -> Inputs;
}

/// What a rule returns: a component, or `Option` of one if it may decide a node has none.
pub trait CellResult {
    type C: Component;
    fn into_option(self) -> Option<Self::C>;
}

impl<C: Component> CellResult for C {
    type C = C;
    fn into_option(self) -> Option<C> {
        Some(self)
    }
}

impl<C: Component> CellResult for Option<C> {
    type C = C;
    fn into_option(self) -> Option<C> {
        self
    }
}

/// A rule as a type. Define one with `#[rule("key", incoming(..), outgoing(..), scans(..))]` on a
/// struct whose fields are its parameters.
pub trait Compute: RuleMeta + Serialize + DeserializeOwned + JsonSchema + Clone + Send + Sync + 'static {
    type Output: CellResult;
    fn compute(&self, cx: &Cx) -> Result<Self::Output, Error>;

    /// What it may follow. Override this if that depends on its parameters.
    fn inputs(&self) -> Inputs {
        Self::declared_inputs()
    }
}

/// The component a rule type provides.
pub type OutputOf<R> = <<R as Compute>::Output as CellResult>::C;

/// A rule type as a rule. `#[rule]` makes this happen through `.into()`.
pub fn into_rule<R: Compute>(r: R) -> Rule<OutputOf<R>> {
    let saved = SavedRule { rule: R::KEY.to_string(), params: serde_json::to_value(&r).expect("rule parameters serialise") };
    let inputs = r.inputs();
    Rule { name: R::KEY.to_string(), inputs, f: Arc::new(move |cx| Ok(r.compute(cx)?.into_option())), saved: Some(saved) }
}

/// A rule with no Rust type, such as one from a TypeScript plugin.
pub struct RuntimeRule {
    pub key: String,
    /// The component it provides.
    pub output: String,
    pub inputs: Inputs,
    pub compute: Arc<dyn Fn(&Value, &Cx) -> Cell<Value> + Send + Sync>,
}

/// What a registered rule is, for listing: enough for the LLM to know how to attach one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleInfo {
    pub key: String,
    pub output: String,
    /// Other components it can fill, for a rule that isn't tied to one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<String>,
    pub inputs: BTreeMap<String, Vec<String>>,
    pub params: Option<Value>,
}

/// The code a log's events ran under, as far as it can be written down: whatever the host says it
/// is, and every rule's signature. It can't say what a rule's body did, which is what witnesses
/// are for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Code {
    pub build: String,
    pub rules: Vec<RuleInfo>,
}

/// Where a component is allowed to be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    Node,
    Link,
}

/// Whether a value has the right shape, and if not, what's wrong with it.
pub type Validate = fn(&Value) -> Result<(), String>;

/// What's known about a component's shape. A component nothing declares is kept and unchecked.
#[derive(Clone)]
pub(crate) struct Declared {
    pub(crate) place: Place,
    pub(crate) validate: Option<Validate>,
}

/// How much a check matters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    /// A commit that newly breaks this is refused.
    Invariant,
    /// Worth a look, but nothing is refused.
    Notice,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckInfo {
    pub(crate) name: String,
    pub(crate) severity: Severity,
}

impl CheckInfo {
    pub(crate) fn column(&self) -> String {
        format!("check/{}", self.name)
    }
}

type Build = Arc<dyn Fn(&Value) -> Result<Backing, String> + Send + Sync>;

/// How a rule that can fill a workspace's own cells is made for one of them: what it may follow, and
/// what it works out.
pub type BuildUnowned = Arc<dyn Fn(&str, &Value) -> Result<(Inputs, Arc<dyn Fn(&Cx) -> Cell<Value> + Send + Sync>), String> + Send + Sync>;

struct Registered {
    info: RuleInfo,
    /// How to make it, for each component it can fill.
    builds: HashMap<String, Build>,
    /// How to make it for a component no system owns, if it can fill those.
    unowned: Option<BuildUnowned>,
}

/// The rules a graph can load, and the defaults they set.
#[derive(Default)]
pub struct Registry {
    rules: HashMap<String, Registered>,
    pub(crate) defaults: HashMap<String, Backing>,
    pub(crate) declared: HashMap<String, Declared>,
    pub(crate) checks: Vec<CheckInfo>,
    /// The kind of link from a node to its kind, if nodes can take cells from one.
    pub(crate) inherits_through: Option<String>,
    /// What systems say that a node still takes from its kind when it doesn't say it, as a row takes
    /// its table's options.
    pub(crate) passed_down: HashSet<String>,
    build: String,
}

fn inputs_info(i: &Inputs) -> BTreeMap<String, Vec<String>> {
    // `like` isn't listed: it says nothing beyond what the other rule's listing says.
    BTreeMap::from([("incoming".to_string(), i.incoming.clone()), ("outgoing".to_string(), i.outgoing.clone()), ("scans".to_string(), i.scans.clone())])
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, reg: Registered) {
        let key = reg.info.key.clone();
        assert!(self.rules.insert(key.clone(), reg).is_none(), "two rules are registered as {key}");
    }

    /// Make a rule type loadable by its key. Registering one twice is harmless.
    pub fn rule<R: Compute>(&mut self) -> &mut Self {
        if self.rules.contains_key(R::KEY) {
            return self;
        }
        self.insert(Registered {
            info: RuleInfo {
                key: R::KEY.to_string(),
                output: <OutputOf<R>>::NAME.to_string(),
                also: Vec::new(),
                inputs: inputs_info(&R::declared_inputs()),
                params: Some(schemars::schema_for!(R).to_value()),
            },
            builds: HashMap::from([(
                <OutputOf<R>>::NAME.to_string(),
                Arc::new(|params: &Value| {
                    let r: R = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
                    Ok(Backing::Computed(erase_typed(into_rule(r))))
                }) as Build,
            )]),
            unowned: None,
        });
        self
    }

    /// This rule type, on every node that doesn't say otherwise.
    pub fn set_default<R: Compute>(&mut self, r: R) -> &mut Self {
        self.rule::<R>();
        self.defaults.insert(<OutputOf<R>>::NAME.to_string(), typed_backing(crate::graph::Storage::Rule(into_rule(r))));
        self
    }

    /// A rule that's already loadable, as it's written down, on every node that doesn't say otherwise.
    pub fn set_default_rule(&mut self, component: &str, key: &str, params: Value) -> &mut Self {
        let saved = SavedRule { rule: key.to_string(), params };
        let backing = self.resolve(&Self::column_key(component), &saved);
        self.defaults.insert(component.to_string(), backing);
        self
    }

    /// Make a rule with no Rust type loadable by its key.
    pub fn runtime_rule(&mut self, r: RuntimeRule) -> &mut Self {
        let (key, inputs, compute) = (r.key.clone(), r.inputs.clone(), r.compute);
        let output = r.output.clone();
        let build: Build = Arc::new(move |params: &Value| {
            let (params, compute) = (params.clone(), compute.clone());
            let saved = SavedRule { rule: key.clone(), params: params.clone() };
            let f = Arc::new(move |cx: &Cx| compute(&params, cx));
            Ok(Backing::Computed(erase_runtime(Rule { name: key.clone(), inputs: inputs.clone(), f, saved: Some(saved) })))
        });
        self.insert(Registered { info: RuleInfo { key: r.key, output: r.output, also: Vec::new(), inputs: inputs_info(&r.inputs), params: None }, builds: HashMap::from([(output, build)]), unowned: None });
        self
    }

    /// Make this key able to fill any component no system owns: the workspace's own cells, such as a
    /// price or a range. A component a system owns still has to say it can hold this rule.
    pub fn rule_on_unowned(&mut self, key: &str, build: impl Fn(&str, &Value) -> Result<(Inputs, Arc<dyn Fn(&Cx) -> Cell<Value> + Send + Sync>), String> + Send + Sync + 'static) -> &mut Self {
        if !self.rules.contains_key(key) {
            let info = RuleInfo { key: key.to_string(), output: "a workspace's own cell".to_string(), also: Vec::new(), inputs: BTreeMap::new(), params: None };
            self.insert(Registered { info, builds: HashMap::new(), unowned: None });
        }
        self.rules.get_mut(key).expect("inserted above").unowned = Some(Arc::new(build));
        self
    }

    /// Nodes take what they don't say about the workspace's own cells from what they link to by this
    /// kind of link, as an instance does from its kind.
    pub fn inherit_through(&mut self, kind: &str) -> &mut Self {
        self.inherits_through = Some(kind.to_string());
        self
    }

    /// Nodes take this from their kind too, if they don't say it, though a system says what it is: a
    /// row of a table of choices has the options its table has.
    pub fn passed_down(&mut self, component: &str) -> &mut Self {
        self.passed_down.insert(component.to_string());
        self
    }

    /// The components systems say what they are: declared, filled by a rule, or given a default.
    pub fn owned(&self) -> HashSet<String> {
        let mut out: HashSet<String> = self.declared.keys().chain(self.defaults.keys()).cloned().collect();
        for r in self.rules.values() {
            out.extend(r.builds.keys().cloned());
        }
        out
    }

    /// Make this key able to fill `C`, built from its parameters. A key can fill several typed
    /// components this way, and it's still refused on any it hasn't said it can fill. What it may
    /// follow can depend on its parameters.
    pub fn rule_on<C: Component>(&mut self, key: &str, build: impl Fn(&Value) -> Result<Rule<C>, String> + Send + Sync + 'static) -> &mut Self {
        let saved_key = key.to_string();
        let built: Build = Arc::new(move |params: &Value| {
            let mut r = build(params)?;
            r.saved = Some(SavedRule { rule: saved_key.clone(), params: params.clone() });
            Ok(Backing::Computed(erase_typed(r)))
        });
        match self.rules.get_mut(key) {
            Some(reg) => {
                reg.info.also.push(C::NAME.to_string());
                reg.builds.insert(C::NAME.to_string(), built);
            }
            None => self.insert(Registered {
                info: RuleInfo { key: key.to_string(), output: C::NAME.to_string(), also: Vec::new(), inputs: BTreeMap::new(), params: None },
                builds: HashMap::from([(C::NAME.to_string(), built)]),
                unowned: None,
            }),
        }
        self
    }

    /// Which code this is, such as a commit hash. Recorded in the log.
    pub fn build(&mut self, id: &str) -> &mut Self {
        self.build = id.to_string();
        self
    }

    /// This code, as recorded in a log.
    pub fn code(&self) -> Code {
        Code { build: self.build.clone(), rules: self.list() }
    }

    /// Say that this component goes on nodes, and values must parse as this type.
    pub fn on_nodes<C: Component>(&mut self) -> &mut Self {
        self.declare(C::NAME, Place::Node, Some(|v| serde_json::from_value::<C>(v.clone()).map(drop).map_err(|e| e.to_string())))
    }

    /// Say that this component goes on links, and values must parse as this type.
    pub fn on_links<L: Component>(&mut self) -> &mut Self {
        self.declare(L::NAME, Place::Link, Some(|v| serde_json::from_value::<L>(v.clone()).map(drop).map_err(|e| e.to_string())))
    }

    /// The same for a component with no Rust type.
    pub fn declare(&mut self, name: &str, place: Place, validate: Option<Validate>) -> &mut Self {
        self.declared.insert(name.to_string(), Declared { place, validate });
        self
    }

    /// Something that must hold at every node: `f` says what's wrong with the node it's run for.
    /// It's an ordinary rule in its own column, so it's memoised, and may only read what `spec`
    /// declares.
    pub fn invariant(&mut self, spec: RuleSpec, f: impl Fn(&Cx) -> Result<Vec<String>, Error> + Send + Sync + 'static) -> &mut Self {
        self.check(spec, Severity::Invariant, f)
    }

    /// The same for something that's only worth a look.
    pub fn notice(&mut self, spec: RuleSpec, f: impl Fn(&Cx) -> Result<Vec<String>, Error> + Send + Sync + 'static) -> &mut Self {
        self.check(spec, Severity::Notice, f)
    }

    fn check(&mut self, spec: RuleSpec, severity: Severity, f: impl Fn(&Cx) -> Result<Vec<String>, Error> + Send + Sync + 'static) -> &mut Self {
        let info = CheckInfo { name: spec.name, severity };
        let column = info.column();
        let compute = move |_: &Value, cx: &Cx| Ok(Some(serde_json::to_value(f(cx)?).expect("strings serialise")));
        self.runtime_rule(RuntimeRule { key: column.clone(), output: column.clone(), inputs: spec.inputs, compute: Arc::new(compute) });
        let saved = SavedRule { rule: column.clone(), params: Value::Null };
        let backing = self.resolve(&Self::column_key(&column), &saved);
        self.defaults.insert(column, backing);
        self.checks.push(info);
        self
    }

    /// Everything registered, sorted by key.
    pub fn list(&self) -> Vec<RuleInfo> {
        let mut out: Vec<RuleInfo> = self.rules.values().map(|r| r.info.clone()).collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    pub fn contains(&self, key: &str) -> bool {
        self.rules.contains_key(key)
    }

    /// What a written-down rule becomes on this cell. If it can't run, the cell says why, and
    /// what was written is kept.
    pub(crate) fn resolve(&self, cell: &Key, saved: &SavedRule) -> Backing {
        let broken = |e: Error| Backing::Broken(saved.clone(), e);
        let Some(reg) = self.rules.get(&saved.rule) else {
            return broken(Error::UnknownRule(saved.rule.clone()));
        };
        let Some(build) = reg.builds.get(&cell.1) else {
            if let Some(unowned) = reg.unowned.as_ref().filter(|_| !self.owned().contains(&cell.1)) {
                return match unowned(&cell.1, &saved.params) {
                    Ok((inputs, f)) => Backing::Computed(erase_runtime(Rule { name: saved.rule.clone(), inputs, f, saved: Some(saved.clone()) })),
                    Err(detail) => broken(Error::Malformed { cell: cell.clone(), detail }),
                };
            }
            return broken(Error::WrongRule { rule: saved.rule.clone(), component: cell.1.clone(), provides: reg.info.output.clone() });
        };
        build(&saved.params).unwrap_or_else(|detail| broken(Error::Malformed { cell: cell.clone(), detail }))
    }

    pub(crate) fn column_key(component: &str) -> Key {
        (Entity::nil(), component.to_string())
    }
}
