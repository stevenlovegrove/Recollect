//! The action log. A graph held by a `Log` only changes through `commit`, so its state is the replay
//! of its events, and every fact can answer who set it, when and why.
//!
//! ```
//! # use std::sync::Arc;
//! # use recollect_graph::{Log, Op, On, Registry, Stored, Entity};
//! # use serde_json::json;
//! let mut log = Log::new(Arc::new(Registry::new()));
//! let a = Entity::new();
//! log.commit("steven", "first thoughts", vec![Op::Spawn { id: a }, Op::Set { on: On::Node { id: a }, cell: "note".into(), to: Some(Stored::Value(json!("hello"))) }]).unwrap();
//! assert_eq!(log.graph().get_value(a, "note"), Ok(Some(json!("hello"))));
//! log.undo_last("steven").unwrap();
//! assert!(!log.graph().contains(a));
//! log.redo("steven").unwrap();
//! assert!(log.graph().contains(a));
//! ```
//!
//! The log lives in memory, for as long as a workspace is open, and is what undo and redo work from.
//! It isn't saved: what's saved is the graph, as files, and the log says which of them each change
//! touched (`take_changes`), so whoever holds it can write just those. History is a line: undoing
//! appends the inverse, and so does redoing, so both are ordinary checked changes with an author.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Backed, Entity, Error, Graph};
use crate::registry::{Place, Registry, Severity};
use crate::saved::{LoadProblem, SavedEdge, SavedGraph, SavedNode};

/// What's written in a cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stored {
    Value(Value),
    Formula(String),
}

impl Stored {
    fn of(b: Backed) -> Option<Stored> {
        match b {
            Backed::Value(v) => Some(Stored::Value(v)),
            Backed::Formula(t) => Some(Stored::Formula(t)),
            Backed::Builtin => None,
        }
    }
}

/// What a cell is on: a node, or the edge from one node to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum On {
    Node { id: Entity },
    Edge { from: Entity, to: Entity },
}

impl On {
    pub fn entity(&self) -> Entity {
        match *self {
            On::Node { id } => id,
            On::Edge { from, to } => Entity::edge(from, to),
        }
    }

    pub fn target(&self) -> Target {
        match *self {
            On::Node { id } => Target::Node(id),
            On::Edge { from, to } => Target::Edge(from, to),
        }
    }
}

/// The only ways a logged graph changes. Each has an exact inverse, so undo is just more ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A node with nothing on it.
    Spawn { id: Entity },
    /// Remove a node and everything it says. Its edges have to go first.
    Despawn { id: Entity },
    /// Give a node a component.
    Add { id: Entity, component: String },
    /// Take a component from a node.
    Remove { id: Entity, component: String },
    /// Give the edge from one node to another a component, making the edge if it's new.
    Link { from: Entity, to: Entity, component: String },
    /// Take a component off an edge. Taking the last one takes the edge.
    Unlink { from: Entity, to: Entity, component: String },
    /// Write a value or a formula in a cell. `None` takes away what's written there. As JSON it reads
    /// `{"op": "set", "id": .., "cell": .., "value": ..}`, or `"formula"`, and an edge's cell has
    /// `"from"` and `"to"` in place of `"id"`.
    Set {
        #[serde(flatten)]
        on: On,
        cell: String,
        #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
        to: Option<Stored>,
    },
}

impl Op {
    pub fn set_value(id: Entity, cell: &str, value: Value) -> Op {
        Op::Set { on: On::Node { id }, cell: cell.to_string(), to: Some(Stored::Value(value)) }
    }

    pub fn set<C: crate::CellType>(id: Entity, value: C) -> Op {
        Op::set_value(id, C::NAME, serde_json::to_value(&value).expect("cells serialise"))
    }

    pub fn set_formula(id: Entity, cell: &str, text: &str) -> Op {
        Op::Set { on: On::Node { id }, cell: cell.to_string(), to: Some(Stored::Formula(text.to_string())) }
    }

    pub fn add(id: Entity, component: &str) -> Op {
        Op::Add { id, component: component.to_string() }
    }

    pub fn link(from: Entity, to: Entity, component: &str) -> Op {
        Op::Link { from, to, component: component.to_string() }
    }

    pub fn target(&self) -> Target {
        match self {
            Op::Spawn { id } | Op::Despawn { id } | Op::Add { id, .. } | Op::Remove { id, .. } => Target::Node(*id),
            Op::Link { from, to, .. } | Op::Unlink { from, to, .. } => Target::Edge(*from, *to),
            Op::Set { on, .. } => on.target(),
        }
    }
}

/// What an op changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Node(Entity),
    Edge(Entity, Entity),
}

impl Target {
    /// The nodes it's about.
    pub fn nodes(&self) -> Vec<Entity> {
        match *self {
            Target::Node(e) => vec![e],
            Target::Edge(a, b) => vec![a, b],
        }
    }
}

/// Something that doesn't hold, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub rule: String,
    pub on: Target,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// Milliseconds since 1970.
    pub at: u64,
    pub author: String,
    /// Why, in the actor's words. Reasons live here, not in node text.
    pub why: String,
    pub ops: Vec<Op>,
    /// Exactly undoes `ops`, applied in order.
    pub inverse: Vec<Op>,
    /// The event this one takes back, if it's an undo or a redo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<u64>,
}

#[derive(Debug)]
pub enum CommitError {
    /// Nothing changed: these checks would be newly broken, or the ops don't apply.
    Refused(Vec<Violation>),
    /// There's no such event.
    NoSuchEvent(u64),
}

impl CommitError {
    /// What was refused, or nothing if this wasn't a refusal.
    pub fn violations(&self) -> &[Violation] {
        match self {
            CommitError::Refused(v) => v,
            _ => &[],
        }
    }
}

/// The node files a change needs written: these nodes as they are now, and these gone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    pub nodes: Vec<SavedNode>,
    pub gone: Vec<Entity>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.gone.is_empty()
    }
}

/// Milliseconds since 1970. `SystemTime` panics on the web, so there it's the browser's clock.
pub fn now_ms() -> u64 {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }
}

/// A graph whose only way to change is a commit.
pub struct Log {
    graph: Graph,
    /// What the graph was before the first event.
    base: SavedGraph,
    events: Vec<Event>,
    /// Events undo would take back, newest last.
    undoable: Vec<u64>,
    /// Undos redo would take back, newest last. Any other change clears it.
    redoable: Vec<u64>,
    /// Nodes whose files are out of date since `take_changes` was last called.
    dirty: BTreeSet<Entity>,
}

impl Log {
    /// An empty graph.
    pub fn new(registry: Arc<Registry>) -> Self {
        Log { graph: Graph::with_registry(registry), base: SavedGraph::default(), events: Vec::new(), undoable: Vec::new(), redoable: Vec::new(), dirty: BTreeSet::new() }
    }

    /// Start from a saved graph, with no history. A node that's only an edge's target gets an empty
    /// node, as a load gives it, and so does its file the next time changes are taken.
    pub fn from_saved(saved: SavedGraph, registry: Arc<Registry>) -> (Log, Vec<LoadProblem>) {
        let mut saved = saved;
        let mut problems = Vec::new();
        let dangling = fill_dangling(&mut saved, &mut problems);
        let (graph, more) = Graph::load(saved, registry);
        // `load` reports the same dangling links, so only its own problems are added.
        problems.extend(more.into_iter().filter(|p| !matches!(p, LoadProblem::DanglingLink { .. })));
        let base = graph.save();
        let log = Log { graph, base, events: Vec::new(), undoable: Vec::new(), redoable: Vec::new(), dirty: dangling };
        let standing = log.violations();
        if !standing.is_empty() {
            problems.push(LoadProblem::Standing(standing));
        }
        (log, problems)
    }

    /// The same graph and history, read by a registry that knows more, as when a workspace takes on
    /// another system. Nothing it holds changes, and anything worked out is worked out again.
    pub fn widen(&mut self, registry: Arc<Registry>) {
        let (graph, _) = Graph::load(self.graph.save(), registry);
        self.graph = graph;
    }

    // ---------- reading ----------

    /// Read-only: writing goes through `commit`.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Every event since it was opened, oldest first.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The newest event, if there's been one.
    pub fn last_seq(&self) -> Option<u64> {
        self.events.last().map(|e| e.seq)
    }

    /// The event undo would take back.
    pub fn undo_next(&self) -> Option<u64> {
        self.undoable.last().copied()
    }

    /// The undo redo would take back.
    pub fn redo_next(&self) -> Option<u64> {
        self.redoable.last().copied()
    }

    /// The graph as it was just after event `seq`, worked out by the code running now.
    pub fn state_at(&self, seq: u64) -> Graph {
        let (mut g, _) = Graph::load(self.base.clone(), self.graph.registry.clone());
        for ev in &self.events[..=seq as usize] {
            for op in &ev.ops {
                apply(&mut g, op).expect("logged ops replay");
            }
        }
        g
    }

    /// Every event that touched a target, oldest first.
    pub fn history(&self, target: Target) -> Vec<&Event> {
        self.events.iter().filter(|ev| ev.ops.iter().any(|op| op.target() == target)).collect()
    }

    /// Who last wrote in a cell, and why, if anyone has since it was opened.
    pub fn blame(&self, on: On, cell: &str) -> Option<&Event> {
        self.events.iter().rev().find(|ev| ev.ops.iter().any(|op| matches!(op, Op::Set { on: o, cell: c, to: Some(_) } if *o == on && c == cell)))
    }

    /// Every invariant that doesn't hold now, such as after a hand edit.
    pub fn violations(&self) -> Vec<Violation> {
        let mut out = check_values(&self.graph, &all_targets(&self.graph));
        out.extend(check_rules(&self.graph, Severity::Invariant));
        out.sort();
        out
    }

    /// What's worth a look now.
    pub fn notices(&self) -> Vec<Violation> {
        check_rules(&self.graph, Severity::Notice)
    }

    /// The graph these ops would give, if they'd be accepted. It's the same code as a commit, and
    /// nothing is written or logged.
    pub fn what_if(&self, ops: &[Op]) -> Result<Graph, Vec<Violation>> {
        let mut hypothetical = self.graph.fork();
        try_apply(&mut hypothetical, ops)?;
        Ok(hypothetical)
    }

    /// The node files that need writing since this was last asked, and forgets them.
    pub fn take_changes(&mut self) -> Changes {
        let ids = std::mem::take(&mut self.dirty);
        let nodes = self.graph.save_only(ids.iter().copied().filter(|e| self.graph.contains(*e))).nodes;
        let gone = ids.into_iter().filter(|e| !self.graph.contains(*e)).collect();
        Changes { nodes, gone }
    }

    // ---------- writing ----------

    /// Apply `ops` atomically, or refuse them and change nothing. Checks already broken, such as by a
    /// hand edit, don't block a commit unless it breaks them in a new way.
    pub fn commit(&mut self, author: &str, why: &str, ops: Vec<Op>) -> Result<u64, CommitError> {
        let inverse = try_apply(&mut self.graph, &ops).map_err(CommitError::Refused)?;
        let seq = self.record(author, why, ops, inverse, None);
        self.undoable.push(seq);
        self.redoable.clear();
        Ok(seq)
    }

    /// Take back any one event, by appending its inverse. It's a change like any other, so it can be
    /// undone in turn.
    pub fn undo(&mut self, seq: u64, author: &str, why: &str) -> Result<u64, CommitError> {
        let inverse = self.events.get(seq as usize).ok_or(CommitError::NoSuchEvent(seq))?.inverse.clone();
        let applied = try_apply(&mut self.graph, &inverse).map_err(CommitError::Refused)?;
        let new = self.record(author, why, inverse, applied, Some(seq));
        self.undoable.retain(|s| *s != seq);
        self.undoable.push(new);
        self.redoable.clear();
        Ok(new)
    }

    /// Take back the newest change that hasn't been, if there is one.
    pub fn undo_last(&mut self, author: &str) -> Result<Option<u64>, CommitError> {
        let Some(seq) = self.undoable.last().copied() else { return Ok(None) };
        let inverse = self.events[seq as usize].inverse.clone();
        let applied = try_apply(&mut self.graph, &inverse).map_err(CommitError::Refused)?;
        self.undoable.pop();
        let new = self.record(author, &format!("undo #{seq}"), inverse, applied, Some(seq));
        self.redoable.push(new);
        Ok(Some(new))
    }

    /// Take back the newest undo, if nothing else has changed since.
    pub fn redo(&mut self, author: &str) -> Result<Option<u64>, CommitError> {
        let Some(seq) = self.redoable.last().copied() else { return Ok(None) };
        let inverse = self.events[seq as usize].inverse.clone();
        let applied = try_apply(&mut self.graph, &inverse).map_err(CommitError::Refused)?;
        self.redoable.pop();
        let new = self.record(author, &format!("redo #{}", self.events[seq as usize].reverts.unwrap_or(seq)), inverse, applied, Some(seq));
        self.undoable.push(new);
        Ok(Some(new))
    }

    /// Bring in a graph edited outside the log, such as by hand, by another program or `git pull`.
    /// The files are what they are, so this always records, as ordinary ops worked out by diffing,
    /// and reports any checks the edit now breaks rather than refusing it. Returns the event if there
    /// was a difference.
    pub fn reconcile(&mut self, disk: &SavedGraph, author: &str) -> (Option<u64>, Vec<Violation>) {
        let mut disk = disk.clone();
        let filled = fill_dangling(&mut disk, &mut Vec::new());
        let ops = diff(&self.graph.save(), &disk);
        let seq = (!ops.is_empty()).then(|| {
            let mut inverse = Vec::new();
            for op in &ops {
                let mut undo = apply(&mut self.graph, op).expect("a diff applies");
                undo.extend(std::mem::take(&mut inverse));
                inverse = undo;
            }
            let seq = self.record(author, "external edit", ops, inverse, None);
            self.undoable.push(seq);
            self.redoable.clear();
            seq
        });
        // A node made for a dangling edge has no file yet.
        self.dirty.extend(filled);
        (seq, self.violations())
    }

    /// The graph already holds these ops. Make them an event.
    fn record(&mut self, author: &str, why: &str, ops: Vec<Op>, inverse: Vec<Op>, reverts: Option<u64>) -> u64 {
        let seq = self.events.len() as u64;
        // An edge lives in its source's file.
        self.dirty.extend(ops.iter().map(|op| match op.target() {
            Target::Node(e) | Target::Edge(e, _) => e,
        }));
        self.events.push(Event { seq, at: now_ms(), author: author.to_string(), why: why.to_string(), ops, inverse, reverts });
        seq
    }
}

/// Give every node that's only an edge's target an empty node, saying so. Returns which.
fn fill_dangling(saved: &mut SavedGraph, problems: &mut Vec<LoadProblem>) -> BTreeSet<Entity> {
    let known: BTreeSet<Entity> = saved.nodes.iter().map(|n| n.id).collect();
    let mut dangling = BTreeSet::new();
    for n in &saved.nodes {
        for link in n.links.iter().filter(|l| !known.contains(&l.to)) {
            problems.push(LoadProblem::DanglingLink { from: n.id, to: link.to });
            dangling.insert(link.to);
        }
    }
    saved.nodes.extend(dangling.iter().copied().map(SavedNode::empty));
    saved.nodes.sort_by_key(|n| n.id);
    dangling
}

// ---------- applying ----------

/// What's written on an entity, as the ops that would write it back.
fn rewrite(g: &Graph, on: On) -> Vec<Op> {
    g.written(on.entity()).into_iter().filter_map(|(cell, b)| Some(Op::Set { on, cell, to: Some(Stored::of(b)?) })).collect()
}

/// Apply one op, returning what exactly undoes it, in order.
pub(crate) fn apply(g: &mut Graph, op: &Op) -> Result<Vec<Op>, String> {
    let node = |g: &Graph, id: &Entity| if g.contains(*id) { Ok(()) } else { Err(format!("{id} doesn't exist")) };
    match op {
        Op::Spawn { id } => {
            if g.contains(*id) {
                return Err(format!("{id} already exists"));
            }
            g.adopt(*id);
            Ok(vec![Op::Despawn { id: *id }])
        }
        Op::Despawn { id } => {
            node(g, id)?;
            let mut undo = vec![Op::Spawn { id: *id }];
            undo.extend(g.given(*id).into_iter().map(|component| Op::Add { id: *id, component }));
            undo.extend(rewrite(g, On::Node { id: *id }));
            if !g.despawn(*id) {
                return Err(format!("{id} still has edges"));
            }
            Ok(undo)
        }
        Op::Add { id, component } => {
            node(g, id)?;
            if g.given(*id).contains(component) {
                return Ok(vec![]);
            }
            g.add(*id, component);
            Ok(vec![Op::Remove { id: *id, component: component.clone() }])
        }
        Op::Remove { id, component } => {
            node(g, id)?;
            if !g.given(*id).contains(component) {
                return Err(format!("{id} doesn't have {component}"));
            }
            g.remove(*id, component);
            Ok(vec![Op::Add { id: *id, component: component.clone() }])
        }
        Op::Link { from, to, component } => {
            node(g, from)?;
            node(g, to)?;
            if g.linked(*from, *to, component) {
                return Ok(vec![]);
            }
            g.link(*from, *to, component);
            Ok(vec![Op::Unlink { from: *from, to: *to, component: component.clone() }])
        }
        Op::Unlink { from, to, component } => {
            if !g.linked(*from, *to, component) {
                return Err(format!("the edge {from} → {to} doesn't have {component}"));
            }
            let edge = Entity::edge(*from, *to);
            let last = g.given(edge).len() == 1;
            let mut undo = vec![Op::Link { from: *from, to: *to, component: component.clone() }];
            if last {
                undo.extend(rewrite(g, On::Edge { from: *from, to: *to }));
            }
            g.unlink(*from, *to, component);
            Ok(undo)
        }
        Op::Set { on, cell, to } => {
            let e = on.entity();
            match on {
                On::Node { id } => node(g, id)?,
                On::Edge { from, to } if !g.is_edge(e) => return Err(format!("there's no edge {from} → {to}")),
                On::Edge { .. } => {}
            }
            let old = g.written(e).remove(cell).and_then(Stored::of);
            match to {
                Some(Stored::Value(v)) => g.set_value(e, cell, v.clone()),
                Some(Stored::Formula(text)) => g.set_formula(e, cell, text),
                None => g.clear(e, cell),
            };
            Ok(vec![Op::Set { on: *on, cell: cell.clone(), to: old }])
        }
    }
}

/// Apply `ops` if nothing new breaks, returning what undoes them. Otherwise leave `g` as it was.
fn try_apply(g: &mut Graph, ops: &[Op]) -> Result<Vec<Op>, Vec<Violation>> {
    let touched: BTreeSet<Target> = ops.iter().map(Op::target).collect();
    let mut before: BTreeSet<Violation> = check_values(g, &touched).into_iter().collect();
    before.extend(check_rules(g, Severity::Invariant));

    let mut inverse: Vec<Op> = Vec::new();
    let rollback = |g: &mut Graph, inverse: &[Op]| {
        for op in inverse {
            apply(g, op).expect("an inverse applies");
        }
    };
    for op in ops {
        match apply(g, op) {
            Ok(mut undo) => {
                undo.append(&mut inverse);
                inverse = undo;
            }
            Err(detail) => {
                rollback(g, &inverse);
                return Err(vec![Violation { rule: "op_failed".to_string(), on: op.target(), detail }]);
            }
        }
    }
    let mut new = check_values(g, &touched);
    new.extend(check_rules(g, Severity::Invariant));
    new.retain(|v| !before.contains(v));
    if !new.is_empty() {
        rollback(g, &inverse);
        new.sort();
        new.dedup();
        return Err(new);
    }
    Ok(inverse)
}

// ---------- checking ----------

/// What the definitions say about these targets, which doesn't depend on anything else, so a commit
/// only has to look at what it touched.
fn check_values(g: &Graph, targets: &BTreeSet<Target>) -> Vec<Violation> {
    let reg = g.registry();
    let mut out = Vec::new();
    for t in targets {
        let (e, place, what) = match *t {
            Target::Node(e) if g.contains(e) => (e, Place::Node, e.to_string()),
            Target::Edge(from, to) if g.is_edge(Entity::edge(from, to)) => {
                if from == to {
                    out.push(Violation { rule: "no_self_links".to_string(), on: *t, detail: format!("{from} links to itself") });
                }
                (Entity::edge(from, to), Place::Edge, format!("{from} → {to}"))
            }
            _ => continue,
        };
        for c in g.given(e) {
            match reg.component_def(&c) {
                None => out.push(Violation { rule: "unknown_component".to_string(), on: *t, detail: format!("{what} has {c}, which no system provides") }),
                Some(def) if def.place != place => out.push(Violation { rule: "component_on_wrong_target".to_string(), on: *t, detail: format!("{what} has {c}, which goes on {}", if def.place == Place::Node { "a node" } else { "an edge" }) }),
                Some(_) => {}
            }
        }
        for (cell, b) in g.written(e) {
            let def = reg.cell_def(&cell);
            if let Some(def) = def
                && !g.has(e, &def.component)
            {
                out.push(Violation { rule: "cell_without_component".to_string(), on: *t, detail: format!("{what}.{cell} belongs to {}, which it doesn't have", def.component) });
            }
            match b {
                Backed::Value(v) => {
                    if let Some(Err(why)) = def.and_then(|d| d.validate).map(|f| f(&v)) {
                        out.push(Violation { rule: "malformed_cell".to_string(), on: *t, detail: format!("{what}.{cell}: {why}") });
                    }
                }
                Backed::Formula(_) => {
                    if let Some(error) = g.formula_error(e, &cell) {
                        out.push(Violation { rule: "formula_cannot_be_read".to_string(), on: *t, detail: format!("{what}.{cell}: {error:?}") });
                    }
                }
                Backed::Builtin => {}
            }
        }
    }
    out
}

fn all_targets(g: &Graph) -> BTreeSet<Target> {
    let nodes = g.entities().map(Target::Node);
    let edges = g.edges().map(|(_, f, t)| Target::Edge(f, t));
    nodes.chain(edges).collect()
}

/// Every check of this severity, on everything with its component. They're memoised, so this only
/// reruns the ones an edit reached.
fn check_rules(g: &Graph, severity: Severity) -> Vec<Violation> {
    let mut out = Vec::new();
    let entities: Vec<(Entity, Target)> = g.entities().map(|e| (e, Target::Node(e))).chain(g.edges().map(|(e, f, t)| (e, Target::Edge(f, t)))).collect();
    for (component, check) in g.registry().checks().filter(|(_, c)| c.severity == severity) {
        let cell = check.cell();
        for (e, target) in entities.iter().filter(|(e, _)| g.has(*e, component)) {
            let details = match g.get_value(*e, &cell) {
                Ok(Some(v)) => serde_json::from_value::<Vec<String>>(v).unwrap_or_default(),
                Ok(None) => Vec::new(),
                Err(Error::Cycle(_)) => vec!["couldn't be checked: it reads itself".to_string()],
                Err(error) => vec![format!("couldn't be checked: {error:?}")],
            };
            for detail in details {
                out.push(Violation { rule: check.name.clone(), on: *target, detail });
            }
        }
    }
    out
}

// ---------- files and the log ----------

fn by_id(g: &SavedGraph) -> BTreeMap<Entity, &SavedNode> {
    g.nodes.iter().map(|n| (n.id, n)).collect()
}

fn cells_of(cells: &BTreeMap<String, Value>, formulas: &BTreeMap<String, String>) -> BTreeMap<String, Stored> {
    let values = cells.iter().map(|(c, v)| (c.clone(), Stored::Value(v.clone())));
    values.chain(formulas.iter().map(|(c, f)| (c.clone(), Stored::Formula(f.clone())))).collect()
}

/// The ops that turn one cell list into another.
fn set_ops(on: On, was: BTreeMap<String, Stored>, is: BTreeMap<String, Stored>, ops: &mut Vec<Op>) {
    for (c, v) in &is {
        if was.get(c) != Some(v) {
            ops.push(Op::Set { on, cell: c.clone(), to: Some(v.clone()) });
        }
    }
    ops.extend(was.keys().filter(|c| !is.contains_key(*c)).map(|c| Op::Set { on, cell: c.clone(), to: None }));
}

/// The ops that turn one written-down graph into another: new nodes first and gone nodes last, so
/// they all apply.
fn diff(from: &SavedGraph, to: &SavedGraph) -> Vec<Op> {
    let (old, new) = (by_id(from), by_id(to));
    let empty = SavedNode::empty(Entity::whole());
    let mut ops: Vec<Op> = new.keys().filter(|id| !old.contains_key(id)).map(|id| Op::Spawn { id: *id }).collect();
    let (mut edge_ops, mut removals) = (Vec::new(), Vec::new());
    for id in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (old.get(id).copied().unwrap_or(&empty), new.get(id).copied().unwrap_or(&empty));
        if new.contains_key(id) {
            ops.extend(b.components.iter().filter(|c| !a.components.contains(c)).map(|c| Op::Add { id: *id, component: c.clone() }));
            set_ops(On::Node { id: *id }, cells_of(&a.cells, &a.formulas), cells_of(&b.cells, &b.formulas), &mut ops);
            removals.extend(a.components.iter().filter(|c| !b.components.contains(c)).map(|c| Op::Remove { id: *id, component: c.clone() }));
        }
        let edges = |n: &SavedNode| -> BTreeMap<Entity, SavedEdge> { n.links.iter().map(|l| (l.to, l.clone())).collect() };
        let (was, is) = (edges(a), edges(b));
        for t in was.keys().chain(is.keys()).collect::<BTreeSet<_>>() {
            let blank = SavedEdge { to: *t, components: Vec::new(), cells: BTreeMap::new(), formulas: BTreeMap::new() };
            let (x, y) = (was.get(t).unwrap_or(&blank), is.get(t).unwrap_or(&blank));
            edge_ops.extend(y.components.iter().filter(|c| !x.components.contains(c)).map(|c| Op::Link { from: *id, to: *t, component: c.clone() }));
            if !y.components.is_empty() {
                set_ops(On::Edge { from: *id, to: *t }, cells_of(&x.cells, &x.formulas), cells_of(&y.cells, &y.formulas), &mut edge_ops);
            }
            edge_ops.extend(x.components.iter().filter(|c| !y.components.contains(c)).map(|c| Op::Unlink { from: *id, to: *t, component: c.clone() }));
        }
    }
    ops.append(&mut edge_ops);
    ops.append(&mut removals);
    ops.extend(old.keys().filter(|id| !new.contains_key(id)).map(|id| Op::Despawn { id: *id }));
    ops
}
