//! Reading what a specification asks for, in one go, in the shape of the files. This is how an agent
//! reads a workspace: it says which nodes, which of their cells and edges, and whether it wants what's
//! worked out or what happened to them, and gets that and nothing else.
//!
//! What was written comes back exactly as the node's file says it, so what someone wrote is never
//! confused with what was worked out, which is a section of its own when it's asked for. Every handle a
//! node refers to has the name of what it refers to beside it.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::worlds::Bool;
use recollect_graph::{Backed, CellType, Entity, Graph, Name, Op};
use recollect_systems::expr::Val;
use recollect_systems::formulas::{Called, IS_A, LABEL, is_chance, try_formula};
use recollect_systems::label_of;
use recollect_systems::notes::Body;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::views::{self, CellView, Labels, Lens, ShownView, World};
use crate::workspace::Workspace;

/// The most nodes one read gives, unless it says otherwise.
pub const MOST_NODES: usize = 100;

/// What to read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSpec {
    /// Which nodes. Everything, if nothing narrows it.
    #[serde(default)]
    pub nodes: Select,
    /// Which of each node's cells, by name or with `*` as a wildcard. What was written, if not given.
    #[serde(default)]
    pub cells: Option<Vec<String>>,
    /// Which edges come with each node.
    #[serde(default)]
    pub edges: EdgeSpec,
    /// Each cell's value too, in the worlds that are in play, and where it comes from.
    #[serde(default)]
    pub worked_out: bool,
    /// How many of the latest events that touched each node, newest first.
    #[serde(default)]
    pub history: usize,
    /// Choices pinned, as the app pins them.
    #[serde(default)]
    pub world: World,
    /// Only the worlds where this formula is true, for every value read.
    #[serde(default, rename = "where")]
    pub where_: Option<Where>,
    /// The most nodes to give.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A formula saying which worlds to look at: plain, when it names no nodes, as
/// `all(d in with.leaning: d.chosen = d.leaning)`, or with the nodes it names, as a cell's formula has.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Where {
    Plain(String),
    Named {
        formula: String,
        /// What each name in it means: a node's handle or name.
        #[serde(default)]
        labels: BTreeMap<String, String>,
    },
}

/// What looking through a `where` formula did.
#[derive(Debug, Serialize)]
pub struct Looking {
    /// How much of what was believed is in the worlds it leaves, from 0 to 1.
    pub kept: f64,
    /// How much is in worlds where it couldn't be told, which are kept rather than hidden.
    #[serde(skip_serializing_if = "is_zero")]
    pub undecided: f64,
    /// What's a matter of chance that it mentions: looking only where it holds is supposing those are
    /// found out, so what's believed about everything else changes with them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supposing: Vec<String>,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// Which nodes. Everything given here has to hold, so `{kind: Concept, name: "Delta*"}` is the concepts
/// whose names start with Delta.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    /// Handles or names.
    #[serde(default)]
    pub ids: Vec<String>,
    /// Instances of a kind, by its name or handle: what links to it by `is_a`, or to a kind of it.
    #[serde(default)]
    pub kind: Option<String>,
    /// Nodes with this component.
    #[serde(default)]
    pub component: Option<String>,
    /// A name, with `*` and `?` as wildcards.
    #[serde(default)]
    pub name: Option<String>,
    /// Words that all have to be in the name or the notes.
    #[serde(default)]
    pub text: Option<String>,
    /// What's beneath a node, nested, over the edges that arrange the graph.
    #[serde(default)]
    pub tree: Option<TreeSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSpec {
    /// The node at the top, by handle or name.
    pub root: String,
    /// How far down to go.
    #[serde(default = "three")]
    pub depth: usize,
}

fn three() -> usize {
    3
}

/// Which edges come with a node.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeSpec {
    /// Only edges with one of these components. Every edge, if not given.
    #[serde(default)]
    pub components: Option<Vec<String>>,
    /// Edges leaving the node, as its file lists them.
    #[serde(default = "yes")]
    pub outgoing: bool,
    /// Edges arriving, as the other nodes' files list them.
    #[serde(default)]
    pub incoming: bool,
    /// The cells written on each edge.
    #[serde(default = "yes")]
    pub cells: bool,
}

fn yes() -> bool {
    true
}

impl Default for EdgeSpec {
    fn default() -> Self {
        EdgeSpec { components: None, outgoing: true, incoming: false, cells: true }
    }
}

/// What a read gives.
#[derive(Debug, Serialize)]
pub struct ReadOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looking: Option<Looking>,
    pub nodes: Vec<NodeOut>,
    /// How many more matched than were given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more: Option<usize>,
}

/// A node as its file says it, with names beside handles, and whatever else was asked for.
#[derive(Debug, Serialize)]
pub struct NodeOut {
    pub id: Entity,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub formulas: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incoming: Vec<LinkIn>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub worked_out: BTreeMap<String, Worked>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Happened>,
    /// In a tree, what's beneath it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub beneath: Vec<NodeOut>,
    /// In a tree, already given higher up, so what's beneath it isn't given again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
}

/// An edge leaving a node, as its file says it.
#[derive(Debug, Serialize)]
pub struct LinkOut {
    pub to: Entity,
    pub to_name: String,
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub formulas: BTreeMap<String, String>,
}

/// An edge arriving at a node, as the file of the node it comes from says it.
#[derive(Debug, Serialize)]
pub struct LinkIn {
    pub from: Entity,
    pub from_name: String,
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub formulas: BTreeMap<String, String>,
}

/// What a cell comes to, in the worlds in play. One value, if it's the same in all of them. Otherwise
/// each way it can be, with the worlds where it is and how likely those are.
#[derive(Debug, Serialize)]
pub struct Worked {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<Case>,
    /// What it comes to with whatever's a matter of chance taken together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub together: Option<Value>,
    /// Where it comes from, if not from what this node says: `kind <name>`, or `default`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Case {
    pub when: String,
    pub prob: f64,
    /// What it is then. A truth has none: it's true when `when` says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// An event that touched a node.
#[derive(Debug, Serialize)]
pub struct Happened {
    pub seq: u64,
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// How many of its ops touched this node.
    pub ops: usize,
}

/// Read what the specification asks for.
pub fn read(o: &Workspace, spec: &ReadSpec) -> Result<ReadOut, String> {
    let (copy, _) = views::looked_at(o.log.graph(), &spec.world);
    let g: &Graph = copy.as_ref().unwrap_or_else(|| o.log.graph());
    let narrowing = spec.where_.as_ref().map(|w| narrowing(g, w)).transpose()?;
    let reader = Reader::new(o, g, spec, narrowing.as_ref());
    let looking = narrowing.as_ref().map(|n| looking(g, &spec.world, n, reader.lens.as_ref().expect("a where makes a lens")));
    let limit = spec.limit.unwrap_or(MOST_NODES);
    if let Some(tree) = &spec.nodes.tree {
        let root = one(g, &tree.root)?;
        let mut seen = BTreeSet::new();
        let mut given = 0;
        let top = reader.tree(root, tree.depth, &mut seen, &mut given, limit);
        let more = given.saturating_sub(limit);
        return Ok(ReadOut { looking, nodes: vec![top], more: (more > 0).then_some(more) });
    }
    let chosen = select(g, &spec.nodes)?;
    let more = chosen.len().saturating_sub(limit);
    Ok(ReadOut { looking, nodes: chosen.into_iter().take(limit).map(|e| reader.node(e)).collect(), more: (more > 0).then_some(more) })
}

/// The worlds a `where` formula keeps: where it's true, and where it can't be told, so a gap in what's
/// written never hides a world.
pub struct Narrowing {
    pub keeps: Bool,
    pub undefined: Bool,
    pub truth: Bool,
}

pub fn narrowing(g: &Graph, w: &Where) -> Result<Narrowing, String> {
    let (text, labels) = match w {
        Where::Plain(t) => (t.as_str(), BTreeMap::new()),
        Where::Named { formula, labels } => (formula.as_str(), labels.clone()),
    };
    // A formula's names are its node's labels, so one that names nodes is tried on a node of its own,
    // on a copy that's thrown away.
    let tried = if labels.is_empty() {
        try_formula(g, Entity::whole(), text)
    } else {
        let mut copy = g.fork();
        let at = copy.spawn();
        for (called, said) in &labels {
            let to = one(g, said)?;
            let edge = copy.link(at, to, LABEL);
            copy.set(edge, Called(called.clone()));
        }
        try_formula(&copy, at, text)
    };
    let (value, _) = tried.map_err(|e| format!("where: {}", views::error_text(&e, g)))?;
    let truth = value.holds(|v| *v == Val::Bool(true));
    let undefined = value.holds(|v| *v == Val::Undefined);
    if value.holds(|v| !matches!(v, Val::Bool(_) | Val::Undefined)).prob() > 0.0 {
        return Err(format!("where: {text} has to be true or false"));
    }
    Ok(Narrowing { keeps: &truth | &undefined, undefined, truth })
}

fn looking(g: &Graph, world: &World, n: &Narrowing, lens: &Lens) -> Looking {
    let before = Lens::new(g, world).in_play.prob();
    let after = lens.in_play.prob();
    let kept = if before > 0.0 { after / before } else { 0.0 };
    let undecided = if after > 0.0 { (&n.undefined & &lens.in_play).prob() / after } else { 0.0 };
    let labels = Labels::gather(g);
    let mut supposing: Vec<String> = n.truth.vars().iter().map(|v| v.name()).filter(|v| is_chance(g, v)).map(|v| labels.of(&v)).collect();
    supposing.sort();
    supposing.dedup();
    Looking { kept, undecided, supposing }
}

/// The node a handle or a name means, which has to be exactly one.
fn one(g: &Graph, said: &str) -> Result<Entity, String> {
    match by_id_or_name(g, said).as_slice() {
        [e] => Ok(*e),
        [] => Err(format!("there's no node {said}")),
        many => Err(format!("{} nodes are called {said}, so give one by its handle: {}", many.len(), many.iter().map(Entity::to_string).collect::<Vec<_>>().join(", "))),
    }
}

fn by_id_or_name(g: &Graph, said: &str) -> Vec<Entity> {
    if let Ok(e) = said.parse::<Entity>()
        && g.contains(e)
    {
        return vec![e];
    }
    g.entities().filter(|e| name_of(g, *e).as_deref() == Some(said)).collect()
}

fn name_of(g: &Graph, e: Entity) -> Option<String> {
    match g.get_value(e, Name::NAME) {
        Ok(Some(Value::String(n))) => Some(n),
        _ => None,
    }
}

/// Every node the selection picks out, in the order they were made.
fn select(g: &Graph, s: &Select) -> Result<Vec<Entity>, String> {
    let mut nodes: Vec<Entity> = g.entities().filter(|e| !g.is_edge(*e)).collect();
    if !s.ids.is_empty() {
        let mut wanted = BTreeSet::new();
        for said in &s.ids {
            let found = by_id_or_name(g, said);
            if found.is_empty() {
                return Err(format!("there's no node {said}"));
            }
            wanted.extend(found);
        }
        nodes.retain(|e| wanted.contains(e));
    }
    if let Some(kind) = &s.kind {
        let kind = one(g, kind)?;
        let instances = instances(g, kind);
        nodes.retain(|e| instances.contains(e));
    }
    if let Some(c) = &s.component {
        nodes.retain(|e| g.has(*e, c));
    }
    if let Some(pattern) = &s.name {
        nodes.retain(|e| name_of(g, *e).is_some_and(|n| matches(pattern, &n)));
    }
    if let Some(text) = &s.text {
        let terms: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        nodes.retain(|e| {
            let name = label_of(g, *e).to_lowercase();
            let body = g.get::<Body>(*e).ok().flatten().map_or(String::new(), |b| b.0.to_lowercase());
            terms.iter().all(|t| name.contains(t) || body.contains(t))
        });
    }
    Ok(nodes)
}

/// What's an instance of a kind: what links to it by `is_a`, and to whatever is, all the way down.
fn instances(g: &Graph, kind: Entity) -> BTreeSet<Entity> {
    let mut out = BTreeSet::new();
    let mut level = vec![kind];
    while let Some(k) = level.pop() {
        for i in g.sources_of(k, IS_A) {
            if out.insert(i) {
                level.push(i);
            }
        }
    }
    out
}

/// A wildcard match: `*` is anything, `?` is any one character. Case doesn't matter.
pub fn matches(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) => a == b && go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    let lower = |s: &str| s.to_lowercase().chars().collect::<Vec<_>>();
    go(&lower(pattern), &lower(text))
}

struct Reader<'a> {
    o: &'a Workspace,
    g: &'a Graph,
    spec: &'a ReadSpec,
    labels: Labels,
    lens: Option<Lens>,
}

impl<'a> Reader<'a> {
    fn new(o: &'a Workspace, g: &'a Graph, spec: &'a ReadSpec, narrowing: Option<&Narrowing>) -> Reader<'a> {
        let lens = (spec.worked_out || narrowing.is_some()).then(|| Lens::looking(g, &spec.world, narrowing.map(|n| &n.keeps)));
        Reader { o, g, spec, labels: Labels::gather(g), lens }
    }

    fn wanted(&self, cell: &str) -> bool {
        // A node's name always comes, so it can always be told which node it is.
        cell == Name::NAME || self.spec.cells.as_ref().is_none_or(|cs| cs.iter().any(|p| matches(p, cell)))
    }

    fn edge_wanted(&self, edge: Entity) -> bool {
        self.spec.edges.components.as_ref().is_none_or(|cs| self.g.given(edge).iter().any(|c| cs.contains(c)))
    }

    /// What's written on an entity, split as its file splits it, keeping only the cells asked for.
    fn written(&self, e: Entity, all: bool) -> (BTreeMap<String, Value>, BTreeMap<String, String>) {
        let (mut cells, mut formulas) = (BTreeMap::new(), BTreeMap::new());
        for (c, b) in self.g.written(e) {
            if !all && !self.wanted(&c) {
                continue;
            }
            match b {
                Backed::Value(v) => {
                    cells.insert(c, v);
                }
                Backed::Formula(t) => {
                    formulas.insert(c, t);
                }
                Backed::Builtin => {}
            }
        }
        (cells, formulas)
    }

    fn node(&self, e: Entity) -> NodeOut {
        let g = self.g;
        let (cells, formulas) = self.written(e, false);
        let edge_cells = |edge: Entity| if self.spec.edges.cells { self.written(edge, true) } else { Default::default() };
        let mut links = Vec::new();
        let mut incoming = Vec::new();
        for (edge, from, to) in g.edges().filter(|(_, f, t)| *f == e || *t == e) {
            if !self.edge_wanted(edge) {
                continue;
            }
            let components: Vec<String> = g.given(edge).into_iter().collect();
            if from == e && self.spec.edges.outgoing {
                let (cells, formulas) = edge_cells(edge);
                links.push(LinkOut { to, to_name: label_of(g, to), components, cells, formulas });
            } else if to == e && from != e && self.spec.edges.incoming {
                let (cells, formulas) = edge_cells(edge);
                incoming.push(LinkIn { from, from_name: label_of(g, from), components, cells, formulas });
            }
        }
        links.sort_by_key(|l| l.to);
        incoming.sort_by_key(|l| l.from);
        NodeOut {
            id: e,
            components: g.given(e).into_iter().collect(),
            cells,
            formulas,
            links,
            incoming,
            worked_out: self.worked_out(e),
            history: self.history(e),
            beneath: Vec::new(),
            shared: false,
        }
    }

    fn worked_out(&self, e: Entity) -> BTreeMap<String, Worked> {
        let Some(lens) = self.lens.as_ref().filter(|_| self.spec.worked_out) else { return BTreeMap::new() };
        let mut vars = BTreeMap::new();
        self.g
            .cells_of(e)
            .into_iter()
            .filter(|c| !views::internal(self.g, c) && self.wanted(c))
            .filter_map(|c| views::cell_view(self.g, e, &c, lens, &self.labels, &mut vars).map(|v| (c, self.worked(v))))
            .collect()
    }

    fn worked(&self, v: CellView) -> Worked {
        let from = match v.source {
            "kind" => Some(format!("kind {}", v.kind.map_or(String::new(), |k| self.labels.of(&k.to_string())))),
            "default" => Some("default".to_string()),
            _ => None,
        };
        if let Some(error) = v.error {
            return Worked { value: None, cases: Vec::new(), together: None, from, error: Some(error) };
        }
        // The same in every world left is one value.
        if v.value.is_some() && v.vars.is_empty() {
            return Worked { value: v.value, cases: Vec::new(), together: None, from, error: None };
        }
        let cases = match v.shown {
            Some(ShownView::Bool(r)) => vec![Case { when: r.when, prob: r.prob, value: None }],
            Some(ShownView::Cond { cases }) => cases.into_iter().map(|c| Case { when: c.when.when, prob: c.when.prob, value: Some(c.value) }).collect(),
            Some(ShownView::Named { items }) => items.into_iter().map(|i| Case { when: i.when.when, prob: i.when.prob, value: Some(Value::String(self.labels.of(&i.node.to_string()))) }).collect(),
            None => Vec::new(),
        };
        let value = if cases.is_empty() { v.value } else { None };
        Worked { value, cases, together: v.mix, from, error: None }
    }

    fn history(&self, e: Entity) -> Vec<Happened> {
        if self.spec.history == 0 {
            return Vec::new();
        }
        let touches = |op: &Op| op.target().nodes().contains(&e);
        self.o
            .log
            .events()
            .iter()
            .rev()
            .filter_map(|ev| {
                let ops = ev.ops.iter().filter(|op| touches(op)).count();
                (ops > 0).then(|| Happened { seq: ev.seq, author: ev.author.clone(), why: ev.why.clone(), ops })
            })
            .take(self.spec.history)
            .collect()
    }

    /// A node and what's beneath it, over every edge that arranges the graph.
    fn tree(&self, e: Entity, depth: usize, seen: &mut BTreeSet<Entity>, given: &mut usize, limit: usize) -> NodeOut {
        *given += 1;
        let mut out = self.node(e);
        if !seen.insert(e) {
            out.shared = true;
            return out;
        }
        if depth == 0 {
            return out;
        }
        let reg = self.g.registry();
        let mut below: Vec<Entity> = Vec::new();
        for c in reg.components().filter(|c| c.arranges) {
            let next = if c.reversed { self.g.targets_of(e, &c.name) } else { self.g.sources_of(e, &c.name) };
            for n in next {
                if !below.contains(&n) {
                    below.push(n);
                }
            }
        }
        for n in below {
            if *given >= limit {
                *given += 1;
                continue;
            }
            out.beneath.push(self.tree(n, depth - 1, seen, given, limit));
        }
        out
    }
}

#[cfg(test)]
mod tests;
