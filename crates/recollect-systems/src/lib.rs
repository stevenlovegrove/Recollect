//! What a workspace can be made of. A system is two functions: one adds rules and checks to a
//! `Registry`, which is all the core ever sees, and one says what they mean to someone looking at
//! them, in a `Catalog`. A workspace lists the systems it uses by name.

pub mod expr;
pub mod formulas;
pub mod notes;
pub mod plan;
pub mod samples;
pub mod schedule;
pub mod view;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use recollect_graph::worlds::{Bool, Cond, Picks, Var};
use recollect_graph::{Cell, Component, Entity, Graph, Registry};
use serde::Serialize;
use serde_json::Value;

/// A value that differs between worlds, in the forms a viewer knows how to read.
#[derive(Debug, Clone)]
pub enum Shown {
    /// The worlds where something holds.
    Bool(Bool),
    /// Any value, per world.
    Cond(Cond<Value>),
    /// Named nodes, each with the worlds it applies in.
    Named(Vec<(Entity, Bool)>),
}

impl Shown {
    pub fn cond<T: Serialize>(c: &Cond<T>) -> Shown {
        Shown::Cond(c.map(|v| serde_json::to_value(v).expect("values serialise")))
    }

    /// This value among the worlds consistent with `picks`.
    pub fn given(&self, picks: &Picks) -> Shown {
        match self {
            Shown::Bool(b) => Shown::Bool(b.given(picks)),
            Shown::Cond(c) => Shown::Cond(c.given(picks)),
            Shown::Named(items) => Shown::Named(items.iter().map(|(e, b)| (*e, b.given(picks))).filter(|(_, b)| !b.is_never()).collect()),
        }
    }

    /// The open choices this depends on.
    pub fn vars(&self) -> Vec<Var> {
        match self {
            Shown::Bool(b) => b.vars(),
            Shown::Cond(c) => c.vars(),
            Shown::Named(items) => items.iter().flat_map(|(_, b)| b.vars()).collect(),
        }
    }
}

/// What a component means to someone looking at it.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    pub name: String,
    pub system: String,
    /// "node" or "link".
    pub on: &'static str,
    pub about: String,
    /// Worked out by a rule, so there's nothing to type in.
    pub derived: bool,
    /// Plumbing, which only makes sense to the rules that read it.
    pub internal: bool,
    /// For a view to read, rather than a person.
    pub quiet: bool,
    /// A link that gives a graph its shape, rather than just saying something about two nodes.
    pub arranges: bool,
    /// For a link that gives a graph its shape: what it points at goes beneath, not above.
    pub reversed: bool,
    /// A link that says what comes first. It isn't containment, so it's no use to a tree, but a map
    /// can be laid out by it.
    pub orders: bool,
    /// Worth showing on a node's card.
    pub summary: bool,
    /// It's either true or false, so it can be read and set as a tick.
    pub truth: bool,
    /// What a new one starts as.
    pub initial: Option<Value>,
}

type ShownFn = Arc<dyn Fn(&Graph, Entity) -> Cell<Shown> + Send + Sync>;
type ConstraintFn = Arc<dyn Fn(&Graph, Entity) -> Cell<Bool> + Send + Sync>;

/// Something worth saying about a graph as a whole, which no single node's rule can see.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advice {
    pub rule: String,
    pub on: Entity,
    pub detail: String,
}

/// What the active systems mean to someone looking at a graph.
#[derive(Default)]
pub struct Catalog {
    current: String,
    components: BTreeMap<String, ComponentInfo>,
    shown: HashMap<String, ShownFn>,
    constraints: Vec<ConstraintFn>,
    advice: Vec<fn(&Graph) -> Vec<Advice>>,
}

impl Catalog {
    fn add(&mut self, name: &str, on: &'static str, about: &str) -> &mut ComponentInfo {
        let info = ComponentInfo { name: name.to_string(), system: self.current.clone(), on, about: about.to_string(), derived: false, internal: false, quiet: false, arranges: false, reversed: false, orders: false, summary: false, truth: formulas::component_type(name) == expr::Type::Bool, initial: None };
        self.components.entry(name.to_string()).or_insert(info)
    }

    /// A stored fact on a node, with what a new one starts as.
    pub fn fact<C: Component>(&mut self, about: &str, initial: Value) -> &mut ComponentInfo {
        let info = self.add(C::NAME, "node", about);
        info.initial = Some(initial);
        info
    }

    /// A link that says something about two nodes.
    pub fn link<L: Component>(&mut self, about: &str, initial: Value) -> &mut ComponentInfo {
        let info = self.add(L::NAME, "link", about);
        info.initial = Some(initial);
        info
    }

    /// The same for a kind of link with no Rust type.
    pub fn link_named(&mut self, name: &str, about: &str) -> &mut ComponentInfo {
        let info = self.add(name, "link", about);
        info.initial = Some(Value::Null);
        info
    }

    /// A link that gives a graph its shape.
    pub fn arranging<L: Component>(&mut self, about: &str, initial: Value) -> &mut ComponentInfo {
        let info = self.link::<L>(about, initial);
        info.arranges = true;
        info
    }

    /// A value worked out by a rule that doesn't vary between worlds.
    pub fn derived<C: Component>(&mut self, about: &str) -> &mut ComponentInfo {
        let info = self.add(C::NAME, "node", about);
        info.derived = true;
        info
    }

    /// A value worked out by a rule that does.
    pub fn worldly<C: Component>(&mut self, about: &str, f: fn(&C) -> Shown) -> &mut ComponentInfo {
        self.shown.insert(C::NAME.to_string(), Arc::new(move |g, e| Ok(g.get::<C>(e)?.map(|c| f(&c)))));
        self.derived::<C>(about)
    }

    /// Plumbing, which only makes sense to the rules that read it.
    pub fn internal<C: Component>(&mut self) {
        self.add(C::NAME, "node", "").internal = true;
    }

    /// Only the worlds where this holds, for each node that has something to say, count. A puzzle's
    /// solutions are what's left.
    pub fn constraint_on(&mut self, f: fn(&Graph, Entity) -> Cell<Bool>) {
        self.constraints.push(Arc::new(f));
    }

    pub fn advice(&mut self, f: fn(&Graph) -> Vec<Advice>) {
        self.advice.push(f);
    }

    pub fn components(&self) -> impl Iterator<Item = &ComponentInfo> {
        self.components.values()
    }

    pub fn info(&self, name: &str) -> Option<&ComponentInfo> {
        self.components.get(name)
    }

    pub fn is_internal(&self, name: &str) -> bool {
        name.starts_with("check/") || self.info(name).is_some_and(|c| c.internal)
    }

    /// The kinds of link that give a graph its shape.
    pub fn arranging_kinds(&self) -> Vec<&str> {
        self.components.values().filter(|c| c.arranges).map(|c| c.name.as_str()).collect()
    }

    /// This cell as a world value, if that's what it is.
    pub fn shown(&self, g: &Graph, e: Entity, name: &str) -> Option<Cell<Shown>> {
        if let Some(f) = self.shown.get(name) {
            return Some(f(g, e));
        }
        // The workspace's own cells read the way a value does, whether typed in or worked out.
        if self.components.contains_key(name) || self.is_internal(name) {
            return None;
        }
        formulas::shown_unowned(g, e, name)
    }

    /// Whether this is one of the workspace's own cells, which no system says anything about.
    pub fn is_own_cell(&self, name: &str) -> bool {
        !self.components.contains_key(name) && !self.is_internal(name)
    }

    /// The worlds that count: those where every constraint holds. `None` if nothing narrows them.
    pub fn constrained(&self, g: &Graph) -> Option<Bool> {
        let mut acc: Option<Bool> = None;
        for f in &self.constraints {
            for e in g.entities() {
                if let Ok(Some(b)) = f(g, e) {
                    acc = Some(match acc {
                        Some(a) => &a & &b,
                        None => b,
                    });
                }
            }
        }
        acc
    }

    pub fn all_advice(&self, g: &Graph) -> Vec<Advice> {
        self.advice.iter().flat_map(|f| f(g)).collect()
    }
}

/// A system, as a workspace names it.
#[derive(Clone, Copy)]
pub struct System {
    pub name: &'static str,
    pub about: &'static str,
    pub install: fn(&mut Registry),
    pub describe: fn(&mut Catalog),
}

/// Every system there is. A plugin would add to this.
pub const SYSTEMS: &[System] = &[
    System { name: "notes", about: "Names, notes and sources, and links that say how ideas relate.", install: notes::install, describe: notes::describe },
    System { name: "formulas", about: "Values typed in or worked out, choices, and requirements on the worlds that count.", install: formulas::install, describe: formulas::describe },
    System { name: "plan", about: "Work, decisions held open, and what can safely start.", install: plan::install, describe: plan::describe },
    System { name: "schedule", about: "Dates and slack in every world. Needs plan.", install: schedule::install, describe: schedule::describe },
];

/// The systems every workspace has, so they aren't anything to choose.
pub const ALWAYS: &[&str] = &["notes", "formulas"];

/// A registry and catalog for these systems. Notes are always there, because a node needs a name,
/// and so are formulas, because any cell may be worked out.
pub struct Built {
    pub names: Vec<String>,
    pub registry: Arc<Registry>,
    pub catalog: Arc<Catalog>,
}

pub fn build(names: &[String]) -> Result<Built, String> {
    let mut wanted: Vec<String> = ALWAYS.iter().map(|s| s.to_string()).collect();
    for n in names {
        if !wanted.contains(n) {
            wanted.push(n.clone());
        }
    }
    if wanted.iter().any(|n| n == "schedule") && !wanted.iter().any(|n| n == "plan") {
        return Err("the schedule system needs the plan system".to_string());
    }
    let mut reg = Registry::new();
    let mut catalog = Catalog::default();
    reg.build(concat!("recollect ", env!("CARGO_PKG_VERSION")));
    for n in &wanted {
        let s = SYSTEMS.iter().find(|s| s.name == n).ok_or_else(|| format!("there's no system called {n}"))?;
        (s.install)(&mut reg);
        catalog.current = s.name.to_string();
        (s.describe)(&mut catalog);
    }
    Ok(Built { names: wanted, registry: Arc::new(reg), catalog: Arc::new(catalog) })
}

/// Handles to names, for reading a formula. A node with no name reads as the start of its handle.
pub fn labels(g: &Graph) -> HashMap<String, String> {
    let rows = g.implicit_rows().into_iter().map(|(e, name)| (e.to_string(), name));
    rows.chain(g.entities().map(|e| (e.to_string(), label_of(g, e)))).collect()
}

pub fn label_of(g: &Graph, e: Entity) -> String {
    match g.get::<recollect_graph::Name>(e) {
        Ok(Some(n)) => n.0,
        _ => e.to_string().chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect(),
    }
}
