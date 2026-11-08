//! What a graph looks like from outside, shared by the web UI and Claude. All of it takes a world:
//! some open choices pinned to an option each. Every value then reads as it does among the worlds
//! consistent with those picks, worked out here by the same rules, so a what-if and the real thing
//! can't disagree.

use std::collections::{BTreeMap, HashMap};

use recollect_graph::worlds::{Bool, Picks, Space, Var};
use recollect_graph::{Component, Entity, Error, Event, Graph, Log, Op, Stored, Target, Violation};
use recollect_systems::formulas::{EXPR, Uncertain, formula_text};
use recollect_systems::notes::Body;
use recollect_systems::plan::{self, Matrix, WorkItem};
use recollect_systems::{Catalog, ComponentInfo, Shown};
use serde::{Deserialize, Serialize};
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
    /// The kind that says what it is, if one does. It's taken from there unless `own`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Entity>,
    /// One of the workspace's own cells, such as a price, which no system says anything about.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
    /// The formula, if one backs it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// The same value, if it differs between worlds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown: Option<ShownView>,
    /// The open choices it differs over, by id. Empty if it's the same in every world that's left.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<String>,
    /// Whether it differs in any world at all, whatever's pinned or ruled out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub varies: bool,
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
    /// Which row of which table its name makes it, if it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<RowView>,
    /// The rows of related tables with the same key: with this one, they're one thing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Entity>,
    /// The rows its cells read by where they are, as `prev` or `Year[3]` do, rather than through a link.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<Entity>,
    /// A row nobody wrote, here because something that was written reads it. It's worked out from its
    /// table and isn't saved.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unwritten: bool,
}

#[derive(Debug, Serialize)]
pub struct RowView {
    pub table: String,
    /// A number for a row in order, or text.
    pub key: Value,
}

fn node_view(g: &Graph, e: Entity, name: String, cells: BTreeMap<String, CellView>) -> NodeView {
    let row = g.row_of(e).map(|r| RowView {
        table: r.table.clone(),
        key: match &r.key {
            recollect_graph::table::Key::Num(n) => json!(n),
            recollect_graph::table::Key::Text(t) => json!(t),
        },
    });
    // What its cells read last time they ran, which they just did.
    let mut reads: Vec<Entity> = cells
        .keys()
        .flat_map(|c| g.reads_of(e, c))
        .filter_map(|d| match d {
            recollect_graph::Dep::Cell((x, _)) if x != e && g.row_of(x).is_some() && g.link_value(e, x, "label").is_none() => Some(x),
            _ => None,
        })
        .collect();
    reads.sort();
    reads.dedup();
    NodeView { id: e, name, cells, row, parts: g.parts_of(e), reads, unwritten: !g.contains(e) }
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
    /// How likely it is now: its weight, given what's pinned and whatever rules worlds out.
    pub chance: f64,
}

/// Another open question, and how sure of this one's answer knowing it would make you.
#[derive(Debug, Serialize)]
pub struct ClueView {
    pub var: String,
    /// The chance of this question's likeliest answer, on average over what that one turns out to be.
    pub sure: f64,
}

#[derive(Debug, Serialize)]
pub struct VarView {
    pub id: String,
    pub name: String,
    pub options: Vec<OptionView>,
    /// Not anyone's to choose, so what's expected averages over it.
    pub uncertain: bool,
    /// For something uncertain that's still open: the chance of its likeliest answer now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sure: Option<f64>,
    /// The other uncertain things that would help most to find out, best first, for working this out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clues: Vec<ClueView>,
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
    /// Rows nobody wrote that a node here reads, so a map can show them: see `NodeView::unwritten`.
    pub unwritten: Vec<NodeView>,
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
    /// What went wrong, in words. A row nobody wrote only exists once something has asked for it, which
    /// may be after these labels were gathered, so it's named here.
    fn error(&self, e: &Error, g: &Graph) -> String {
        let mut names = self.0.clone();
        names.extend(g.implicit_rows().into_iter().map(|(e, name)| (e.to_string(), name)));
        describe_error(e, &Labels(names))
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

/// The worlds that are left once these picks are made, and whatever rules worlds out has.
fn in_play(g: &Graph, catalog: &Catalog, picks: &Picks) -> Bool {
    catalog.constrained(g).map_or_else(|| Space::global().always(), |c| c.given(picks))
}

/// The same in every world that's left, though it could differ in worlds that are ruled out.
fn settled(s: &Shown, in_play: &Bool) -> bool {
    let fixed = |b: &Bool| b.holds_given(in_play) || (!b).holds_given(in_play);
    match s {
        Shown::Bool(b) => fixed(b),
        Shown::Cond(c) => c.cases().iter().any(|(when, _)| when.holds_given(in_play)),
        Shown::Named(items) => items.iter().all(|(_, b)| fixed(b)),
    }
}

fn cell_view(g: &Graph, catalog: &Catalog, e: Entity, c: &str, picks: &Picks, in_play: &Bool, labels: &Labels, vars: &mut BTreeMap<String, Var>) -> Option<CellView> {
    let rule = g.rule_of(e, c).map(|(r, _)| r);
    let source = if rule.is_some() { "rule" } else { "stored" };
    let own = g.has_own(e, c);
    let formula = g.saved_rule(e, c).filter(|r| r.rule == EXPR).and_then(|r| formula_text(&r.params).ok().map(String::from));
    let kind = g.kind_beneath(e, c);
    let workspace = catalog.is_own_cell(c) && c != recollect_graph::Name::NAME;
    let cell = |value, shown, open: Vec<String>, varies, error| CellView { source, rule: rule.clone(), own, kind, workspace, formula: formula.clone(), value, shown, vars: open, varies, error };
    if let Some(shown) = catalog.shown(g, e, c) {
        return match shown {
            Ok(None) => None,
            Err(err) => Some(cell(None, None, Vec::new(), false, Some(labels.error(&err, g)))),
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
                // A puzzle's constraint differs over its choices, but not between the worlds that are left.
                let mut open: Vec<String> = if settled(&narrowed, in_play) { Vec::new() } else { narrowed.vars().iter().map(Var::name).collect() };
                open.sort();
                open.dedup();
                Some(cell(value, Some(show(&narrowed, labels)), open, !s.vars().is_empty(), None))
            }
        };
    }
    match g.get_runtime(e, c) {
        Ok(None) => None,
        Err(err) => Some(cell(None, None, Vec::new(), false, Some(labels.error(&err, g)))),
        Ok(Some(r)) => Some(cell(Some(r.value), None, Vec::new(), false, None)),
    }
}

fn worlds(g: &Graph, catalog: &Catalog, world: &World, picks: &Picks, labels: &Labels, vars: BTreeMap<String, Var>) -> WorldsView {
    let constraint = catalog.constrained(g);
    let in_play = in_play(g, catalog, picks);
    let mut applied = World::new();
    let mut unpicked = Vec::new();
    let mut count = Some(1u128);
    let mut views = Vec::new();
    let held = in_play.prob();
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
                let chance = match picked {
                    Some(p) => f64::from(p == o),
                    None if held > 0.0 => (&var.is(o) & &in_play).prob() / held,
                    None => 0.0,
                };
                OptionView { id: o.clone(), name: labels.of(o), weight: weights[i], gap: gaps[i], possible, picked: picked == Some(o), chance }
            })
            .collect();
        let uncertain = id.parse::<Entity>().is_ok_and(|e| g.has_own(e, Uncertain::NAME));
        views.push(VarView { id: id.clone(), name: labels.of(id), options: views_options, uncertain, sure: None, clues: Vec::new() });
    }
    views.sort_by(|a, b| a.id.cmp(&b.id));
    clues(&mut views, &vars, &in_play, held);
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

/// What's worth finding out. For each uncertain thing still open, how sure of it you'd be, on average,
/// once you knew each other uncertain thing that's open: the chance of its likeliest answer, given what
/// the other turns out to be, weighted by how likely that is. Only what would make you surer is listed,
/// best first. What's someone's to choose isn't something to find out, so it's neither.
fn clues(views: &mut [VarView], vars: &BTreeMap<String, Var>, in_play: &Bool, held: f64) {
    /// Past this many, it's a puzzle rather than a case, and this would take a while.
    const MOST: usize = 12;
    if held <= 0.0 {
        return;
    }
    let open: Vec<(String, Var, Vec<String>)> = views
        .iter()
        .filter(|v| v.uncertain && !v.options.iter().any(|o| o.picked) && v.options.iter().filter(|o| o.possible).count() > 1)
        .filter_map(|v| Some((v.id.clone(), vars.get(&v.id)?.clone(), v.options.iter().filter(|o| o.possible).map(|o| o.id.clone()).collect())))
        .take(MOST + 1)
        .collect();
    if open.len() > MOST {
        return;
    }
    // The worlds where each open thing lands on each of its options, and is consistent with what's known.
    let lands: Vec<Vec<Bool>> = open.iter().map(|(_, var, options)| options.iter().map(|o| &var.is(o) & in_play).collect()).collect();
    for (t, (target, _, _)) in open.iter().enumerate() {
        let sure = lands[t].iter().map(|b| b.prob()).fold(0.0, f64::max) / held;
        let mut found: Vec<ClueView> = open
            .iter()
            .enumerate()
            .filter(|(c, _)| *c != t)
            .map(|(c, (id, _, _))| {
                let after: f64 = lands[c].iter().map(|other| lands[t].iter().map(|b| (b & other).prob()).fold(0.0, f64::max)).sum::<f64>() / held;
                ClueView { var: id.clone(), sure: after }
            })
            .filter(|c| c.sure > sure + 1e-9)
            .collect();
        found.sort_by(|a, b| b.sure.total_cmp(&a.sure));
        let view = views.iter_mut().find(|v| v.id == *target).expect("an open one is in the views");
        view.sure = Some(sure);
        view.clues = found;
    }
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
    let playing = in_play(g, catalog, &picks);
    let mut vars = BTreeMap::new();
    let mut node = |e: Entity, name: String| {
        let cells = g.components_of(e).into_iter().filter(|c| !catalog.is_internal(c)).filter_map(|c| cell_view(g, catalog, e, &c, &picks, &playing, &labels, &mut vars).map(|v| (c, v))).collect();
        node_view(g, e, name, cells)
    };
    let nodes: Vec<NodeView> = g.entities().map(|e| node(e, recollect_systems::label_of(g, e))).collect();
    // Only what something written reads: a row nobody wrote that reads another doesn't bring that one too.
    let mut unwritten: Vec<(recollect_graph::table::Row, Entity)> = nodes.iter().flat_map(|n| n.reads.iter().copied()).filter(|e| !g.contains(*e)).filter_map(|e| Some((g.row_of(e)?, e))).collect();
    unwritten.sort_by(|a, b| (&a.0.table, &a.0.key).cmp(&(&b.0.table, &b.0.key)));
    unwritten.dedup_by_key(|(_, e)| *e);
    let unwritten = unwritten
        .into_iter()
        .map(|(row, e)| {
            let mut n = node(e, format!("{}[{}]", row.table, row.key));
            n.reads.clear();
            n
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
        unwritten,
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

/// A cell, by its node and component.
#[derive(Debug, Clone, Deserialize)]
pub struct CellRef {
    pub node: Entity,
    pub component: String,
}

/// Some cells, as they'd be with another cell typed in as one value.
#[derive(Debug, Serialize)]
pub struct Swept {
    pub at: f64,
    pub cells: Vec<Option<CellView>>,
}

/// What these cells would be with `set` typed in as each of these values in turn, with nothing changed.
/// It's the same as typing each one in: a copy of the graph is changed, and it keeps the memos, so each
/// value only reruns what it reaches.
pub fn sweep(o: &Open, set: &CellRef, values: &[f64], read: &[CellRef], world: &World) -> Result<Vec<Swept>, String> {
    let g = o.log.graph();
    if !g.entities().any(|e| e == set.node) {
        return Err(format!("there's no node {}", set.node));
    }
    let catalog = &o.built.catalog;
    let labels = Labels(recollect_systems::labels(g));
    let picks = Space::global().picks(world);
    let mut copy = g.fork();
    let mut vars = BTreeMap::new();
    Ok(values
        .iter()
        .map(|&at| {
            copy.set_runtime(set.node, &set.component, json!(at));
            let playing = in_play(&copy, catalog, &picks);
            Swept { at, cells: read.iter().map(|r| cell_view(&copy, catalog, r.node, &r.component, &picks, &playing, &labels, &mut vars)).collect() }
        })
        .collect())
}

/// One row of a numbered table, whether or not anyone wrote it.
#[derive(Debug, Serialize)]
pub struct RowCells {
    pub key: i64,
    pub id: Entity,
    /// Someone wrote it, so it's a node like any other. Otherwise it's worked out from the table, and
    /// isn't saved.
    pub written: bool,
    pub cells: BTreeMap<String, CellView>,
}

/// A numbered table's rows from one key to another, narrowed to `world`. Rows nobody wrote are there too,
/// as far as the table goes on: see `Graph::row_at`.
pub fn rows(o: &Open, table: &str, from: i64, to: i64, world: &World) -> Vec<RowCells> {
    let g = o.log.graph();
    let catalog = &o.built.catalog;
    let labels = Labels(recollect_systems::labels(g));
    let picks = Space::global().picks(world);
    let playing = in_play(g, catalog, &picks);
    let mut vars = BTreeMap::new();
    (from..=to)
        .filter_map(|n| {
            let e = g.row_at(table, &recollect_graph::table::Key::Num(n))?;
            let cells = g.components_of(e).into_iter().filter(|c| !catalog.is_internal(c)).filter_map(|c| cell_view(g, catalog, e, &c, &picks, &playing, &labels, &mut vars).map(|v| (c, v))).collect();
            Some(RowCells { key: n, id: e, written: g.contains(e), cells })
        })
        .collect()
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
    let playing = in_play(g, catalog, &picks);
    let mut vars = BTreeMap::new();
    let cells = g.components_of(e).into_iter().filter(|c| !catalog.is_internal(c)).filter_map(|c| cell_view(g, catalog, e, &c, &picks, &playing, &labels, &mut vars).map(|v| (c, v))).collect();
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
    NodeDetail { node: node_view(g, e, labels.of(&e.to_string()), cells), outgoing, incoming, history: history(o, Some(e)) }
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
