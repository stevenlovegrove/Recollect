//! What a graph looks like from outside, shared by the web UI and Claude. All of it takes a world:
//! some open choices pinned to an option each. Every value then reads as it does among the worlds
//! consistent with those picks, worked out here by the same rules, so a what-if and the real thing
//! can't disagree.

use std::collections::{BTreeMap, HashMap};

use recollect_graph::worlds::{Bool, Picks, Space, Var};
use recollect_graph::{Component, Entity, Error, Event, Graph, Log, Op, Stored, Target, Violation};
use recollect_systems::formulas::{EXPR, formula_text};
use recollect_systems::notes::Body;
use recollect_systems::plan::{self, Matrix, WorkItem};
use recollect_systems::{Catalog, ComponentInfo, Shown};
use serde::Serialize;
use serde_json::{Value, json};

use crate::app::Open;

pub type World = BTreeMap<String, String>;

/// What a cell holds, for reading.
#[derive(Debug, Serialize)]
pub struct CellView {
    /// "stored" if typed in, "rule" if worked out.
    pub source: &'static str,
    /// The rule, if one backs it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Whether this node says so itself, rather than taking the default for every node.
    pub own: bool,
    /// The formula, if one backs it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// The same value, if it differs between worlds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown: Option<ShownView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Reading {
    pub when: String,
    pub prob: f64,
    pub always: bool,
    pub never: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShownView {
    Bool(Reading),
    Cond { cases: Vec<CaseView> },
    Named { items: Vec<NamedView> },
}

#[derive(Debug, Serialize)]
pub struct CaseView {
    pub when: Reading,
    pub value: Value,
}

#[derive(Debug, Serialize)]
pub struct NamedView {
    pub node: Entity,
    pub when: Reading,
}

#[derive(Debug, Serialize)]
pub struct NodeView {
    pub id: Entity,
    pub name: String,
    pub cells: BTreeMap<String, CellView>,
}

#[derive(Debug, Serialize)]
pub struct LinkView {
    pub from: Entity,
    pub to: Entity,
    pub kind: String,
    pub value: Value,
}

#[derive(Debug, Serialize)]
pub struct OptionView {
    /// What the option is called to the engine: a handle, or plain text for a puzzle.
    pub id: String,
    pub name: String,
    pub weight: f64,
    pub gap: bool,
    /// Some world still in play lands here.
    pub possible: bool,
    pub picked: bool,
}

#[derive(Debug, Serialize)]
pub struct VarView {
    pub id: String,
    pub name: String,
    pub options: Vec<OptionView>,
}

#[derive(Debug, Serialize)]
pub struct WorldsView {
    pub picks: World,
    pub vars: Vec<VarView>,
    /// How many worlds are still in play, as text because it can be enormous.
    pub count: String,
    /// Whether anything rules worlds out, as a puzzle's constraints do.
    pub constrained: bool,
    /// The worlds in play, if that's what the workspace is about. Limited to the first few hundred.
    pub solutions: Vec<World>,
}

#[derive(Debug, Serialize)]
pub struct CheckView {
    pub severity: &'static str,
    pub rule: String,
    pub on: Vec<Entity>,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceView {
    pub name: String,
    pub about: Option<String>,
    pub dir: String,
    pub systems: Vec<String>,
    pub sample: Option<String>,
    pub direct_writes: bool,
}

#[derive(Debug, Serialize)]
pub struct RanView {
    pub node: Entity,
    pub component: String,
}

/// Everything there is to see.
#[derive(Debug, Serialize)]
pub struct View {
    pub workspace: WorkspaceView,
    /// Where the log is now.
    pub last_seq: Option<u64>,
    /// If it's gone back: the newest event that follows from here, to return to.
    pub latest: Option<u64>,
    pub nodes: Vec<NodeView>,
    pub links: Vec<LinkView>,
    pub components: Vec<ComponentInfo>,
    pub rules: Vec<recollect_graph::RuleInfo>,
    pub worlds: WorldsView,
    pub checks: Vec<CheckView>,
    /// The cells that actually ran to answer this, since the last time anyone asked.
    pub ran: Vec<RanView>,
    pub proposals: usize,
}

struct Labels(HashMap<String, String>);

impl Labels {
    fn of(&self, s: &str) -> String {
        self.0.get(s).cloned().unwrap_or_else(|| s.to_string())
    }
    fn read(&self, b: &Bool) -> Reading {
        Reading { when: b.show_with(&|s| self.of(s)), prob: b.prob(), always: b.is_always(), never: b.is_never() }
    }
}

pub fn error_text(e: &Error, g: &Graph) -> String {
    describe_error(e, &Labels(recollect_systems::labels(g)))
}

fn describe_error(e: &Error, labels: &Labels) -> String {
    match e {
        Error::Cycle(keys) => {
            let mut names: Vec<String> = keys.iter().map(|(n, _)| labels.of(&n.to_string())).collect();
            names.dedup();
            format!("depends on itself, through {}", names.join(", "))
        }
        Error::Undeclared { rule, read } => format!("{rule} read {read}, which it didn't declare"),
        Error::Missing((n, c)) => format!("needs {c} on {}, which isn't there", labels.of(&n.to_string())),
        Error::Malformed { cell: (n, c), detail } => format!("{c} on {} isn't valid: {detail}", labels.of(&n.to_string())),
        Error::Failed(s) => relabel(s, labels),
        Error::UnknownRule(r) => format!("no system provides the rule {r}"),
        Error::WrongRule { rule, component, provides } => format!("{rule} provides {provides}, not {component}"),
    }
}

fn show(s: &Shown, labels: &Labels) -> ShownView {
    match s {
        Shown::Bool(b) => ShownView::Bool(labels.read(b)),
        Shown::Cond(c) => ShownView::Cond { cases: c.cases().into_iter().map(|(g, v)| CaseView { when: labels.read(&g), value: v.clone() }).collect() },
        Shown::Named(items) => ShownView::Named { items: items.iter().map(|(e, b)| NamedView { node: *e, when: labels.read(b) }).collect() },
    }
}

fn cell_view(g: &Graph, catalog: &Catalog, e: Entity, c: &str, picks: &Picks, labels: &Labels, vars: &mut BTreeMap<String, Var>) -> Option<CellView> {
    let rule = g.rule_of(e, c).map(|(r, _)| r);
    let source = if rule.is_some() { "rule" } else { "stored" };
    let own = g.has_own(e, c);
    let formula = g.saved_rule(e, c).filter(|r| r.rule == EXPR).and_then(|r| formula_text(&r.params).ok().map(String::from));
    let cell = |value, shown, error| CellView { source, rule: rule.clone(), own, formula: formula.clone(), value, shown, error };
    if let Some(shown) = catalog.shown(g, e, c) {
        return match shown {
            Ok(None) => None,
            Err(err) => Some(cell(None, None, Some(describe_error(&err, labels)))),
            Ok(Some(s)) => {
                for v in s.vars() {
                    vars.insert(v.name(), v);
                }
                let narrowed = s.given(picks);
                // A value with only one case can be read, and edited, as what it is.
                let value = match &narrowed {
                    Shown::Cond(c) => match c.cases().as_slice() {
                        [(_, only)] => Some((*only).clone()),
                        _ => None,
                    },
                    _ => None,
                };
                Some(cell(value, Some(show(&narrowed, labels)), None))
            }
        };
    }
    match g.get_runtime(e, c) {
        Ok(None) => None,
        Err(err) => Some(cell(None, None, Some(describe_error(&err, labels)))),
        Ok(Some(r)) => Some(cell(Some(r.value), None, None)),
    }
}

fn worlds(g: &Graph, catalog: &Catalog, world: &World, picks: &Picks, labels: &Labels, vars: BTreeMap<String, Var>) -> WorldsView {
    let constraint = catalog.constrained(g);
    let space = Space::global();
    let in_play = constraint.as_ref().map_or_else(|| space.always(), |c| c.given(picks));
    let mut applied = World::new();
    let mut unpicked = Vec::new();
    let mut count = Some(1u128);
    let mut views = Vec::new();
    for (id, var) in &vars {
        let options = var.options();
        let weights = var.weights();
        let gaps = var.gaps();
        let picked = world.get(id).filter(|p| options.contains(p));
        if let Some(p) = picked {
            applied.insert(id.clone(), p.clone());
        } else {
            unpicked.push(var.clone());
            count = count.and_then(|n| n.checked_mul(options.len() as u128));
        }
        let views_options = options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let possible = match picked {
                    Some(p) => p == o,
                    None => !(&var.is(o) & &in_play).is_never(),
                };
                OptionView { id: o.clone(), name: labels.of(o), weight: weights[i], gap: gaps[i], possible, picked: picked == Some(o) }
            })
            .collect();
        views.push(VarView { id: id.clone(), name: labels.of(id), options: views_options });
    }
    views.sort_by(|a, b| a.id.cmp(&b.id));
    let (count, solutions) = if constraint.is_some() {
        let n = in_play.count(&unpicked);
        let found = in_play.solutions(&unpicked, 300).into_iter().map(|mut s| {
            s.extend(applied.clone());
            s
        });
        (n.to_string(), found.collect())
    } else {
        (count.map_or_else(|| "more than can be counted".to_string(), |n| n.to_string()), vec![])
    };
    WorldsView { picks: applied, vars: views, count, constrained: constraint.is_some(), solutions }
}

fn target_nodes(t: Target) -> Vec<Entity> {
    match t {
        Target::Node(e) => vec![e],
        Target::Link(a, b) => vec![a, b],
        Target::Defaults => vec![],
    }
}

fn checks(log: &Log, catalog: &Catalog, labels: &Labels) -> Vec<CheckView> {
    let check = |severity: &'static str, v: Violation| CheckView { severity, rule: v.rule, on: target_nodes(v.on), detail: relabel(&v.detail, labels) };
    let mut out: Vec<CheckView> = log.violations().into_iter().map(|v| check("broken", v)).collect();
    out.extend(log.notices().into_iter().map(|v| check("notice", v)));
    out.extend(catalog.all_advice(log.graph()).into_iter().map(|a| CheckView { severity: "notice", rule: a.rule, on: vec![a.on], detail: relabel(&a.detail, labels) }));
    out
}

/// Checks say what they found in handles, because that's what they have.
fn relabel(text: &str, labels: &Labels) -> String {
    let mut out = text.to_string();
    for (id, name) in &labels.0 {
        if out.contains(id.as_str()) {
            out = out.replace(id.as_str(), &format!("“{name}”"));
        }
    }
    out
}

/// The whole graph, narrowed to `world`.
pub fn view(o: &Open, world: &World) -> View {
    let g: &Graph = o.log.graph();
    let catalog = &o.built.catalog;
    let labels = Labels(recollect_systems::labels(g));
    let picks = Space::global().picks(world);
    let mut vars = BTreeMap::new();
    let nodes = g
        .entities()
        .map(|e| {
            let cells = g.components_of(e).into_iter().filter(|c| !catalog.is_internal(c)).filter_map(|c| cell_view(g, catalog, e, &c, &picks, &labels, &mut vars).map(|v| (c, v))).collect();
            NodeView { id: e, name: recollect_systems::label_of(g, e), cells }
        })
        .collect();
    let mut links = Vec::new();
    for (from, to, bag) in g.links() {
        for (kind, value) in bag {
            links.push(LinkView { from, to, kind: kind.clone(), value: value.clone() });
        }
    }
    let worlds = worlds(g, catalog, world, &picks, &labels, vars);
    let ran = g.take_ran().into_iter().filter(|(_, c)| !catalog.is_internal(c)).map(|(node, component)| RanView { node, component }).collect();
    View {
        workspace: WorkspaceView { name: o.config.name.clone(), about: o.config.about.clone(), dir: o.dir.display().to_string(), systems: o.built.names.clone(), sample: o.sample.clone(), direct_writes: o.config.llm_direct_writes },
        last_seq: o.log.last_seq(),
        latest: o.log.last_seq().and_then(|h| o.log.newest_after(h)),
        nodes,
        links,
        components: catalog.components().filter(|c| !c.internal).cloned().collect(),
        rules: g.registry().list().into_iter().filter(|r| !r.key.starts_with("check/")).collect(),
        worlds,
        checks: checks(&o.log, catalog, &labels),
        ran,
        proposals: o.proposals.len(),
    }
}

/// What a formula would read as on a node, with nothing changed.
#[derive(Debug, Serialize)]
pub struct Tried {
    pub shown: Option<ShownView>,
    pub expansion: Option<String>,
    pub error: Option<String>,
}

pub fn try_formula(o: &Open, e: Entity, text: &str, world: &World) -> Tried {
    let g = o.log.graph();
    let labels = Labels(recollect_systems::labels(g));
    match recollect_systems::formulas::try_formula(g, e, text) {
        Ok((value, expansion)) => {
            let narrowed = Shown::cond(&value).given(&Space::global().picks(world));
            Tried { shown: Some(show(&narrowed, &labels)), expansion: Some(relabel(&expansion, &labels).replace(['“', '”'], "")), error: None }
        }
        Err(err) => Tried { shown: None, expansion: None, error: Some(describe_error(&err, &labels)) },
    }
}

/// What the planning system has to say, narrowed to `world`, or nothing if it isn't active.
#[derive(Debug, Serialize)]
pub struct PlanView {
    pub work: Vec<WorkItem>,
    pub decisions: Vec<Matrix>,
}

pub fn planning(o: &Open, world: &World) -> Result<PlanView, String> {
    if !o.built.names.iter().any(|n| n == "plan") {
        return Ok(PlanView { work: vec![], decisions: vec![] });
    }
    let g = o.log.graph();
    let labels = Labels(recollect_systems::labels(g));
    let work = plan::work(g, world).map_err(|e| describe_error(&e, &labels))?;
    let decisions = plan::matrices(g, world).map_err(|e| describe_error(&e, &labels))?;
    Ok(PlanView { work, decisions })
}

/// An op, in words someone can read back.
#[derive(Debug, Serialize)]
pub struct OpView {
    pub verb: &'static str,
    pub node: Option<Entity>,
    pub other: Option<Entity>,
    pub what: Option<String>,
    pub value: Option<Value>,
}

pub fn op_view(op: &Op) -> OpView {
    match op {
        Op::Spawn { id } => OpView { verb: "create", node: Some(*id), other: None, what: None, value: None },
        Op::Despawn { id } => OpView { verb: "delete", node: Some(*id), other: None, what: None, value: None },
        Op::Set { id, component, to } => {
            let (verb, value) = match to {
                None => ("remove", None),
                Some(Stored::Value(v)) => ("set", Some(v.clone())),
                // A formula is what you'd want to review, not which rule runs it.
                Some(Stored::Rule(r)) => ("set_rule", Some(json!(formula_text(&r.params).ok().filter(|_| r.rule == EXPR).unwrap_or(&r.rule)))),
            };
            OpView { verb, node: Some(*id), other: None, what: Some(component.clone()), value }
        }
        Op::Link { from, to, kind, value } => OpView { verb: if value.is_some() { "link" } else { "unlink" }, node: Some(*from), other: Some(*to), what: Some(kind.clone()), value: value.clone() },
        Op::Default { component, to } => OpView { verb: if to.is_some() { "set_default" } else { "remove_default" }, node: None, other: None, what: Some(component.clone()), value: None },
    }
}

#[derive(Debug, Serialize)]
pub struct EventView {
    pub seq: u64,
    /// The event this one follows.
    pub parent: Option<u64>,
    /// Whether it's part of what led to where the log is now.
    pub here: bool,
    pub at: u64,
    pub author: String,
    pub why: String,
    pub ops: Vec<OpView>,
}

pub fn event_view(ev: &Event, here: bool) -> EventView {
    EventView { seq: ev.seq, parent: ev.parent, here, at: ev.at, author: ev.author.clone(), why: ev.why.clone(), ops: ev.ops.iter().map(op_view).collect() }
}

/// Everything on every branch, newest first. For one node, only what led to how it is now.
pub fn history(o: &Open, node: Option<Entity>) -> Vec<EventView> {
    let path: Vec<u64> = o.log.path().iter().map(|ev| ev.seq).collect();
    let here = |ev: &&Event| path.contains(&ev.seq);
    match node {
        None => o.log.events().iter().rev().map(|ev| event_view(ev, here(&ev))).collect(),
        Some(n) => o.log.path().into_iter().rev().filter(|ev| ev.ops.iter().any(|op| target_nodes(op.target()).contains(&n))).map(|ev| event_view(ev, true)).collect(),
    }
}

// ---------- for reading in smaller pieces, as Claude does ----------

#[derive(Debug, Serialize)]
pub struct Brief {
    pub id: Entity,
    pub name: String,
    pub components: Vec<String>,
}

fn brief(o: &Open, e: Entity) -> Brief {
    let g = o.log.graph();
    let components = g.components_of(e).into_iter().filter(|c| g.has_own(e, c) && !catalog_hides(c)).collect();
    Brief { id: e, name: recollect_systems::label_of(g, e), components }
}

fn catalog_hides(c: &str) -> bool {
    c == recollect_graph::Name::NAME
}

pub fn list(o: &Open, component: Option<&str>) -> Vec<Brief> {
    let g = o.log.graph();
    g.entities().filter(|e| component.is_none_or(|c| g.has_own(*e, c) || matches!(g.get_runtime(*e, c), Ok(Some(_))))).map(|e| brief(o, e)).collect()
}

pub fn search(o: &Open, query: &str, limit: usize) -> Vec<Brief> {
    let g = o.log.graph();
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<(usize, Entity)> = g
        .entities()
        .filter_map(|e| {
            let name = recollect_systems::label_of(g, e).to_lowercase();
            let body = g.get::<Body>(e).ok().flatten().map_or(String::new(), |b| b.0.to_lowercase());
            let all = terms.iter().all(|t| name.contains(t) || body.contains(t));
            all.then(|| (terms.iter().filter(|t| name.contains(*t)).count(), e))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().take(limit).map(|(_, e)| brief(o, e)).collect()
}

#[derive(Debug, Serialize)]
pub struct NodeDetail {
    #[serde(flatten)]
    pub node: NodeView,
    pub outgoing: Vec<LinkBrief>,
    pub incoming: Vec<LinkBrief>,
    pub history: Vec<EventView>,
}

#[derive(Debug, Serialize)]
pub struct LinkBrief {
    pub kind: String,
    pub node: Entity,
    pub name: String,
    pub value: Value,
}

pub fn node(o: &Open, e: Entity, world: &World) -> NodeDetail {
    let g = o.log.graph();
    let catalog = &o.built.catalog;
    let labels = Labels(recollect_systems::labels(g));
    let picks = Space::global().picks(world);
    let mut vars = BTreeMap::new();
    let cells = g.components_of(e).into_iter().filter(|c| !catalog.is_internal(c)).filter_map(|c| cell_view(g, catalog, e, &c, &picks, &labels, &mut vars).map(|v| (c, v))).collect();
    let mut outgoing = Vec::new();
    let mut incoming = Vec::new();
    for (from, to, bag) in g.links() {
        for (kind, value) in bag {
            if from == e {
                outgoing.push(LinkBrief { kind: kind.clone(), node: to, name: labels.of(&to.to_string()), value: value.clone() });
            }
            if to == e {
                incoming.push(LinkBrief { kind: kind.clone(), node: from, name: labels.of(&from.to_string()), value: value.clone() });
            }
        }
    }
    NodeDetail { node: NodeView { id: e, name: labels.of(&e.to_string()), cells }, outgoing, incoming, history: history(o, Some(e)) }
}

#[derive(Debug, Serialize)]
pub struct TreeNode {
    #[serde(flatten)]
    pub node: Brief,
    pub need: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
    /// Shown elsewhere in this tree, so what's beneath it isn't repeated.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
}

/// What's beneath a node, over the kinds of link that give the graph its shape.
pub fn tree(o: &Open, root: Entity, depth: usize) -> TreeNode {
    let kinds = o.built.catalog.arranging_kinds().into_iter().map(String::from).collect::<Vec<_>>();
    let labels = Labels(recollect_systems::labels(o.log.graph()));
    fn walk(o: &Open, kinds: &[String], labels: &Labels, e: Entity, depth: usize, seen: &mut Vec<Entity>) -> TreeNode {
        let g = o.log.graph();
        let shared = seen.contains(&e);
        seen.push(e);
        let mut children = Vec::new();
        if !shared && depth > 0 {
            let mut below: Vec<Entity> = kinds.iter().flat_map(|k| if o.built.catalog.info(k).is_some_and(|c| c.reversed) { g.targets_of(e, k) } else { g.sources_of(e, k) }).collect();
            below.dedup();
            for c in below {
                children.push(walk(o, kinds, labels, c, depth - 1, seen));
            }
        }
        let need = g.get::<plan::Need>(e).ok().flatten().map(|n| n.0.show_with(&|s| labels.of(s)));
        TreeNode { node: brief(o, e), need, children, shared }
    }
    walk(o, &kinds, &labels, root, depth, &mut Vec::new())
}

#[derive(Debug, Serialize)]
pub struct Overview {
    pub workspace: WorkspaceView,
    pub nodes: usize,
    pub components: Vec<ComponentInfo>,
    pub rules: Vec<String>,
    pub checks: Vec<CheckView>,
    pub open_choices: Vec<VarView>,
    pub pending_proposals: usize,
    pub events: usize,
}

pub fn overview(o: &Open) -> Overview {
    let v = view(o, &World::new());
    Overview {
        nodes: v.nodes.len(),
        workspace: v.workspace,
        components: v.components,
        rules: v.rules.into_iter().map(|r| r.key).collect(),
        checks: v.checks,
        open_choices: v.worlds.vars,
        pending_proposals: v.proposals,
        events: o.log.events().len(),
    }
}
