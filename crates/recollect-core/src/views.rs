//! What a graph looks like from outside, shared by the web UI and Claude. All of it takes a world:
//! some open choices pinned to an option each. Every value then reads as it does among the worlds
//! consistent with those picks, worked out here by the same code, so a what-if and the real thing
//! can't disagree.

use std::cell::Cell as StdCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use recollect_graph::worlds::{Bool, Picks, Space, Var};
use recollect_graph::{Backed, CellDef, ComponentDef, ComponentType, Entity, Error, Event, Graph, Log, Named, On, Op, Origin, Shown, Source, Stored, Violation};
use recollect_systems::formulas::{CHOICE, LABEL, shown_free};
use recollect_systems::notes::Body;
use recollect_systems::plan::{self, Matrix, UNCERTAIN, WorkItem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::workspace::Workspace;

pub type World = BTreeMap<String, String>;

/// What a cell holds, for reading.
#[derive(Debug, Serialize)]
pub struct CellView {
    /// Where what's in it comes from: "own" if the entity says it, "kind" if it takes it from a kind,
    /// and "default" if it's what its component says when nothing else does.
    pub source: &'static str,
    /// The kind it's taken from, if it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Entity>,
    /// The kind that would say what it is, if this entity didn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beneath: Option<Entity>,
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
    /// Where it differs over choices that are someone's to make: how likely it is, at the least and the
    /// most, over the ways those could go. They aren't averaged over, since nobody rolls a die for them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub over: Option<Over>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Over {
    pub low: f64,
    pub high: f64,
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

/// Why an entity has a component.
#[derive(Debug, Serialize)]
pub struct HasView {
    pub name: String,
    /// "given", "everywhere", "kind", "requires" or "edge".
    pub why: &'static str,
    /// The kind or edge it comes from, or the component that requires it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

fn has_view(name: &str, origin: &Origin) -> HasView {
    let (why, from) = match origin {
        Origin::Given => ("given", None),
        Origin::Everywhere => ("everywhere", None),
        Origin::Kind(k) => ("kind", Some(k.to_string())),
        Origin::Requires(c) => ("requires", Some(c.clone())),
        Origin::Edge(e) => ("edge", Some(e.to_string())),
    };
    HasView { name: name.to_string(), why, from }
}

#[derive(Debug, Serialize)]
pub struct NodeView {
    pub id: Entity,
    pub name: String,
    pub components: Vec<HasView>,
    pub cells: BTreeMap<String, CellView>,
    /// Which row of which table its name makes it, if it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<RowView>,
    /// The rows of related tables with the same key: with this one, they're one thing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Entity>,
    /// The rows its cells read by where they are, as `prev` or `Year[3]` do, rather than through an edge.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<Entity>,
    /// A row nobody wrote, here because something that was written reads it. It's worked out from its
    /// table and isn't saved.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unwritten: bool,
}

#[derive(Debug, Serialize)]
pub struct EdgeView {
    pub id: Entity,
    pub from: Entity,
    pub to: Entity,
    pub components: Vec<String>,
    pub cells: BTreeMap<String, CellView>,
}

#[derive(Debug, Serialize)]
pub struct RowView {
    pub table: String,
    /// A number for a row in order, or text.
    pub key: Value,
}

#[derive(Debug, Serialize)]
pub struct OptionView {
    /// What the option is called to the engine: a handle, or plain text for a puzzle.
    pub id: String,
    pub name: String,
    /// How strongly it was believed before any evidence: its weight, out of all the options'.
    pub weight: f64,
    pub gap: bool,
    /// Some world still in play lands here.
    pub possible: bool,
    pub picked: bool,
    /// How strongly it's believed now, given what's pinned and whatever rules worlds out.
    pub chance: f64,
}

/// Another open question, and how sure of this one's answer knowing it would make you.
#[derive(Debug, Serialize)]
pub struct ClueView {
    pub var: String,
    /// The belief in this question's likeliest answer, on average over what that one turns out to be.
    pub sure: f64,
}

#[derive(Debug, Serialize)]
pub struct VarView {
    pub id: String,
    pub name: String,
    pub options: Vec<OptionView>,
    /// Not anyone's to choose, so what's expected averages over it.
    pub uncertain: bool,
    /// For something uncertain that's still open: the belief in its likeliest answer now.
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
    /// What the host knows it by.
    pub id: String,
    /// Where it is, for a person to read.
    pub location: String,
    pub systems: Vec<String>,
    pub sample: Option<String>,
    pub direct_writes: bool,
}

#[derive(Debug, Serialize)]
pub struct RanView {
    pub node: Entity,
    pub cell: String,
}

/// A cell's definition, with what follows from it.
#[derive(Debug, Serialize)]
pub struct CellInfo {
    #[serde(flatten)]
    pub def: CellDefView,
    /// Worked out, unless something says otherwise.
    pub computed: bool,
    /// It can hold a formula.
    pub formulas: bool,
    /// Anything can be written in it.
    pub settable: bool,
}

/// What's said about a cell, as it's sent.
#[derive(Debug, Serialize)]
pub struct CellDefView {
    pub name: String,
    pub component: String,
    pub about: String,
    pub internal: bool,
    pub summary: bool,
    pub quiet: bool,
    pub truth: bool,
    pub per_entity: bool,
    pub initial: Option<Value>,
}

fn cell_info(d: &CellDef) -> CellInfo {
    let def = CellDefView { name: d.name.clone(), component: d.component.clone(), about: d.about.clone(), internal: d.internal, summary: d.summary, quiet: d.quiet, truth: d.truth, per_entity: d.per_entity, initial: d.initial.clone() };
    CellInfo { def, computed: d.computed(), formulas: d.formulas(), settable: d.settable() }
}

/// Everything there is to see.
#[derive(Serialize)]
pub struct View {
    pub workspace: WorkspaceView,
    /// The newest event, which changes with every change.
    pub last_seq: Option<u64>,
    /// The event undo would take back.
    pub undo: Option<u64>,
    /// The undo redo would take back.
    pub redo: Option<u64>,
    pub nodes: Vec<NodeView>,
    /// Rows nobody wrote that a node here reads, so a map can show them: see `NodeView::unwritten`.
    pub unwritten: Vec<NodeView>,
    /// Nodes nobody wrote that are only what they say, such as the options of a puzzle's choices, so
    /// what lands on one can name it.
    pub constants: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
    pub components: Vec<ComponentDef>,
    pub cells: Vec<CellInfo>,
    pub worlds: WorldsView,
    pub checks: Vec<CheckView>,
    /// The cells that actually ran to answer this, since the last time anyone asked.
    pub ran: Vec<RanView>,
    pub proposals: usize,
}

struct Labels(HashMap<String, String>);

impl Labels {
    /// Every name there is. The options a choice only lists are nodes made as it's worked out, so the
    /// choices are worked out first, or the first look at a workspace would have no names for them.
    fn gather(g: &Graph) -> Labels {
        for choice in g.having(CHOICE) {
            let _ = g.get_value(choice, "chosen");
        }
        Labels(recollect_systems::labels(g))
    }

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

    /// Where something holds, and how likely that is among the worlds still in play: given the evidence
    /// and whatever's pinned, not before them.
    fn read(&self, b: &Bool, in_play: &Bool) -> Reading {
        let held = in_play.prob();
        let prob = if held > 0.0 { (b & in_play).prob() / held } else { 0.0 };
        Reading { when: b.show_with(&|s| self.of(s)), prob, always: b.is_always(), never: b.is_never() }
    }
}

pub fn error_text(e: &Error, g: &Graph) -> String {
    describe_error(e, &Labels::gather(g))
}

fn describe_error(e: &Error, labels: &Labels) -> String {
    match e {
        Error::Cycle(keys) => {
            let mut names: Vec<String> = keys.iter().map(|(n, _)| labels.of(&n.to_string())).collect();
            names.dedup();
            format!("depends on itself, through {}", names.join(", "))
        }
        Error::Missing((n, c)) => format!("needs {c} on {}, which isn't there", labels.of(&n.to_string())),
        Error::Malformed { cell: (n, c), detail } => format!("{c} on {} isn't valid: {detail}", labels.of(&n.to_string())),
        Error::Failed(s) => relabel(s, labels),
    }
}

fn show(s: &Shown, labels: &Labels, in_play: &Bool) -> ShownView {
    match s {
        Shown::Bool(b) => ShownView::Bool(labels.read(b, in_play)),
        Shown::Cond(c) => ShownView::Cond { cases: c.cases().into_iter().map(|(g, v)| CaseView { when: labels.read(&g, in_play), value: v.clone() }).collect() },
        Shown::Named(items) => ShownView::Named { items: items.iter().map(|(e, b)| NamedView { node: *e, when: labels.read(b, in_play) }).collect() },
    }
}

/// A world to look through: some choices pinned to an option each.
struct Lens {
    picks: Picks,
    /// What pinning a decision whose options depend on other choices leaves, beyond its variables: the
    /// worlds where it lands on what it's pinned to. Always, if nothing like that is pinned.
    found: Bool,
    /// The worlds that are left once these picks are made, and whatever rules worlds out has.
    in_play: Bool,
}

impl Lens {
    fn new(g: &Graph, world: &World) -> Lens {
        let picks = plan::picks(g, world);
        let found = plan::pinned(g, world).given(&picks);
        let ruled = g.registry().constrained(g).map_or_else(|| Space::global().always(), |c| c.given(&picks));
        Lens { in_play: &ruled & &found, picks, found }
    }

    /// A value as it reads through this.
    fn narrow(&self, s: &Shown) -> Shown {
        let narrowed = s.given(&self.picks);
        if self.found.is_always() { narrowed } else { narrowed.among(&self.found) }
    }
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

/// A cell as a world value, if that's what it is.
fn shown_of(g: &Graph, e: Entity, c: &str) -> Option<Result<Option<Shown>, Error>> {
    match g.registry().cell_def(c) {
        Some(def) => def.shown.as_ref().map(|f| f(g, e)),
        // The workspace's own cells read the way a value does, whether typed in or worked out.
        None => shown_free(g, e, c),
    }
}

fn cell_view(g: &Graph, e: Entity, c: &str, lens: &Lens, labels: &Labels, vars: &mut BTreeMap<String, Var>) -> Option<CellView> {
    let in_play = &lens.in_play;
    let (backed, source) = g.backed(e, c)?;
    let (source, kind) = match source {
        Source::Own => ("own", None),
        Source::Kind(k) => ("kind", Some(k)),
        Source::Default => ("default", None),
    };
    let formula = match backed {
        Backed::Formula(t) => Some(t),
        _ => None,
    };
    let beneath = g.kind_beneath(e, c);
    let over = StdCell::new(None);
    let cell = |value, shown, open: Vec<String>, varies, error| CellView { source, kind, beneath, formula: formula.clone(), value, shown, vars: open, varies, over: over.take(), error };
    if let Some(shown) = shown_of(g, e, c) {
        return match shown {
            Ok(None) => None,
            Err(err) => Some(cell(None, None, Vec::new(), false, Some(labels.error(&err, g)))),
            Ok(Some(s)) => {
                for v in s.vars() {
                    vars.insert(v.name(), v);
                }
                let narrowed = lens.narrow(&s);
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
                if !open.is_empty() {
                    over.set(over_choices(g, &narrowed, in_play));
                }
                Some(cell(value, Some(show(&narrowed, labels, in_play)), open, !s.vars().is_empty(), None))
            }
        };
    }
    match g.get_value(e, c) {
        Ok(None) => None,
        Err(err) => Some(cell(None, None, Vec::new(), false, Some(labels.error(&err, g)))),
        Ok(Some(v)) => Some(cell(Some(v), None, Vec::new(), false, None)),
    }
}

/// How likely something is for each way the open choices it differs over could go, where those are
/// someone's to make: the least and the most. Nothing, if it doesn't differ over any, or over too many.
fn over_choices(g: &Graph, s: &Shown, in_play: &Bool) -> Option<Over> {
    /// The most ways the choices could go that this looks at.
    const MOST: usize = 64;
    let Shown::Bool(b) = s else { return None };
    let own = |v: &Var| v.name().parse::<Entity>().is_ok_and(|e| g.has(e, plan::DECISION) && !g.has(e, UNCERTAIN) && !g.has(e, plan::NARROWED));
    let mut choices: Vec<Var> = b.vars().into_iter().filter(own).collect();
    choices.sort_by_key(Var::name);
    choices.dedup_by_key(|v| v.name());
    if choices.is_empty() || choices.iter().map(|v| v.options().len()).product::<usize>() > MOST {
        return None;
    }
    let mut ways = vec![in_play.clone()];
    for v in &choices {
        ways = ways.iter().flat_map(|w| v.options().into_iter().map(move |o| w & &v.is(&o))).collect();
    }
    let odds: Vec<f64> = ways.iter().filter(|w| w.prob() > 0.0).map(|w| (b & w).prob() / w.prob()).collect();
    let low = odds.iter().copied().fold(f64::INFINITY, f64::min);
    let high = odds.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (!odds.is_empty()).then_some(Over { low, high })
}

/// Whether a cell is plumbing, which only makes sense to what reads it.
fn internal(g: &Graph, c: &str) -> bool {
    g.registry().cell_def(c).is_some_and(|d| d.internal)
}

/// Every cell an entity has that's worth seeing.
fn cells(g: &Graph, e: Entity, lens: &Lens, labels: &Labels, vars: &mut BTreeMap<String, Var>) -> BTreeMap<String, CellView> {
    g.cells_of(e).into_iter().filter(|c| !internal(g, c)).filter_map(|c| cell_view(g, e, &c, lens, labels, vars).map(|v| (c, v))).collect()
}

fn node_view(g: &Graph, e: Entity, name: String, cells: BTreeMap<String, CellView>) -> NodeView {
    let row = g.row_of(e).map(|r| RowView {
        table: r.table.clone(),
        key: match &r.key {
            recollect_graph::table::Key::Num(n) => json!(n),
            recollect_graph::table::Key::Text(t) => json!(t),
        },
    });
    // What its cells read last time they ran, which they just did. A row reached through a reference,
    // as car.chosen.price reaches Car[Hatchback], isn't read by where it is: what it read the reference
    // from says so already, through its own edge.
    let deps: Vec<recollect_graph::Dep> = cells.keys().flat_map(|c| g.reads_of(e, c)).collect();
    let through: HashSet<Entity> = deps
        .iter()
        .filter_map(|d| match d {
            recollect_graph::Dep::Cell((y, c)) => match shown_of(g, *y, c) {
                Some(Ok(Some(Shown::Named(items)))) => Some(items.into_iter().map(|(x, _)| x)),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .collect();
    let mut reads: Vec<Entity> = deps
        .into_iter()
        .filter_map(|d| match d {
            recollect_graph::Dep::Cell((x, _)) if x != e && g.row_of(x).is_some() && !g.linked(e, x, LABEL) && !through.contains(&x) => Some(x),
            _ => None,
        })
        .collect();
    reads.sort();
    reads.dedup();
    let components = g.components_of(e).iter().map(|(c, o)| has_view(c, o)).collect();
    NodeView { id: e, name, components, cells, row, parts: g.parts_of(e), reads, unwritten: !g.contains(e) }
}

/// Something open, as the worlds see it: where it lands on each of its options.
struct Asked {
    id: String,
    /// Each option, with the worlds where this lands on it, among those these picks leave.
    lands: Vec<(String, Bool)>,
}

fn worlds(g: &Graph, world: &World, lens: &Lens, labels: &Labels, vars: BTreeMap<String, Var>) -> WorldsView {
    let constraint = g.registry().constrained(g);
    let in_play = &lens.in_play;
    // A decision whose options depend on other choices is several variables with its name, one for each
    // set of options that can be left, so it's asked where it lands rather than read off one of them.
    let narrowed = plan::narrowed(g).unwrap_or_default();
    let mut applied = World::new();
    let mut unpicked = Vec::new();
    let mut count = Some(1u128);
    let mut views = Vec::new();
    let mut asked = Vec::new();
    let held = in_play.prob();
    let chance = |lands: &Bool| if held > 0.0 { (lands & in_play).prob() / held } else { 0.0 };
    for (id, var) in &vars {
        if narrowed.iter().any(|n| n.id.to_string() == *id) {
            continue;
        }
        let options = var.options();
        let weights = var.weights();
        let gaps = var.gaps();
        let picked = world.get(id).filter(|p| options.contains(p));
        if let Some(p) = picked {
            applied.insert(id.clone(), p.clone());
        } else {
            unpicked.push(var.clone());
            count = count.and_then(|n| n.checked_mul(options.len() as u128));
            asked.push(Asked { id: id.clone(), lands: options.iter().map(|o| (o.clone(), var.is(o))).collect() });
        }
        let views_options = options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let possible = match picked {
                    Some(p) => p == o,
                    None => !(&var.is(o) & in_play).is_never(),
                };
                let chance = match picked {
                    Some(p) => f64::from(p == o),
                    None => chance(&var.is(o)),
                };
                OptionView { id: o.clone(), name: labels.of(o), weight: weights[i], gap: gaps[i], possible, picked: picked == Some(o), chance }
            })
            .collect();
        let uncertain = id.parse::<Entity>().is_ok_and(|e| g.has(e, UNCERTAIN));
        views.push(VarView { id: id.clone(), name: labels.of(id), options: views_options, uncertain, sure: None, clues: Vec::new() });
    }
    for n in &narrowed {
        let id = n.id.to_string();
        let picked = world.get(&id).filter(|p| n.options.iter().any(|o| o.0.to_string() == **p));
        let lands: Vec<(String, Bool)> = n.options.iter().map(|(o, _, lands)| (o.to_string(), lands.given(&lens.picks))).collect();
        let views_options = n
            .options
            .iter()
            .zip(&lands)
            .map(|((o, gap, before), (name, lands))| {
                let possible = match picked {
                    Some(p) => p == name,
                    None => !(lands & in_play).is_never(),
                };
                let chance = match picked {
                    Some(p) => f64::from(p == name),
                    None => chance(lands),
                };
                // Before any evidence, it's as likely as the choices before it make it.
                OptionView { id: name.clone(), name: labels.of(&o.to_string()), weight: before.prob(), gap: *gap, possible, picked: picked == Some(name), chance }
            })
            .collect();
        if let Some(p) = picked {
            applied.insert(id.clone(), p.clone());
        } else {
            asked.push(Asked { id: id.clone(), lands });
        }
        views.push(VarView { id: id.clone(), name: labels.of(&id), options: views_options, uncertain: g.has(n.id, UNCERTAIN), sure: None, clues: Vec::new() });
    }
    views.sort_by(|a, b| a.id.cmp(&b.id));
    clues(&mut views, &asked, in_play, held);
    let at_rest = plan::at_rest(g);
    let (count, solutions) = if at_rest.is_none() {
        if constraint.is_some() {
            let n = in_play.count(&unpicked);
            let found = in_play.solutions(&unpicked, 300).into_iter().map(|mut s| {
                s.extend(applied.clone());
                s
            });
            (n.to_string(), found.collect())
        } else {
            (count.map_or_else(|| "more than can be counted".to_string(), |n| n.to_string()), vec![])
        }
    } else {
        // Each way things could go, once: a variable that isn't in use doesn't make another.
        let n = (in_play & &at_rest.expect("just checked").given(&lens.picks)).count_with(&unpicked);
        let found = if constraint.is_some() { ways(&asked, in_play, &applied, 300) } else { vec![] };
        (n.to_string(), found)
    };
    WorldsView { picks: applied, vars: views, count, constrained: constraint.is_some(), solutions }
}

/// The ways everything open could land, among the worlds in play, up to `limit`.
fn ways(asked: &[Asked], in_play: &Bool, applied: &World, limit: usize) -> Vec<World> {
    fn go(asked: &[Asked], left: &Bool, at: &mut World, out: &mut Vec<World>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        let Some((first, rest)) = asked.split_first() else {
            out.push(at.clone());
            return;
        };
        for (option, lands) in &first.lands {
            let here = left & lands;
            if !here.is_never() {
                at.insert(first.id.clone(), option.clone());
                go(rest, &here, at, out, limit);
                at.remove(&first.id);
            }
        }
    }
    let mut out = Vec::new();
    if !in_play.is_never() {
        go(asked, in_play, &mut applied.clone(), &mut out, limit);
    }
    out
}

/// What's worth finding out. For each uncertain thing still open, how sure of it you'd be, on average,
/// once you knew each other uncertain thing that's open: the chance of its likeliest answer, given what
/// the other turns out to be, weighted by how likely that is. Only what would make you surer is listed,
/// best first. What's someone's to choose isn't something to find out, so it's neither.
fn clues(views: &mut [VarView], asked: &[Asked], in_play: &Bool, held: f64) {
    /// Past this many, it's a puzzle rather than a case, and this would take a while.
    const MOST: usize = 12;
    if held <= 0.0 {
        return;
    }
    let open: Vec<(String, Vec<&Bool>)> = views
        .iter()
        .filter(|v| v.uncertain && !v.options.iter().any(|o| o.picked) && v.options.iter().filter(|o| o.possible).count() > 1)
        .filter_map(|v| {
            let a = asked.iter().find(|a| a.id == v.id)?;
            let possible = v.options.iter().filter(|o| o.possible).filter_map(|o| a.lands.iter().find(|l| l.0 == o.id).map(|l| &l.1)).collect();
            Some((v.id.clone(), possible))
        })
        .take(MOST + 1)
        .collect();
    if open.len() > MOST {
        return;
    }
    // The worlds where each open thing lands on each of its options, and is consistent with what's known.
    let lands: Vec<Vec<Bool>> = open.iter().map(|(_, options)| options.iter().map(|o| *o & in_play).collect()).collect();
    for (t, (target, _)) in open.iter().enumerate() {
        let sure = lands[t].iter().map(|b| b.prob()).fold(0.0, f64::max) / held;
        let mut found: Vec<ClueView> = open
            .iter()
            .enumerate()
            .filter(|(c, _)| *c != t)
            .map(|(c, (id, _))| {
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

fn checks(log: &Log, labels: &Labels) -> Vec<CheckView> {
    let check = |severity: &'static str, v: Violation| CheckView { severity, rule: v.rule, on: v.on.nodes(), detail: relabel(&v.detail, labels) };
    let mut out: Vec<CheckView> = log.violations().into_iter().map(|v| check("broken", v)).collect();
    out.extend(log.notices().into_iter().map(|v| check("notice", v)));
    out.extend(log.graph().registry().all_advice(log.graph()).into_iter().map(|a| CheckView { severity: "notice", rule: a.rule, on: vec![a.on], detail: relabel(&a.detail, labels) }));
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
pub fn view(o: &Workspace, world: &World) -> View {
    let g: &Graph = o.log.graph();
    let labels = Labels::gather(g);
    let lens = Lens::new(g, world);
    let mut vars = BTreeMap::new();
    let mut node = |e: Entity, name: String| {
        let cells = cells(g, e, &lens, &labels, &mut vars);
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
    // These are made as choices are worked out, so only now are they all there.
    let constants = g.constants().into_iter().map(|(e, _)| node(e, recollect_systems::label_of(g, e))).collect();
    let edges = g.edges().map(|(e, from, to)| EdgeView { id: e, from, to, components: g.given(e).into_iter().collect(), cells: cells(g, e, &lens, &labels, &mut vars) }).collect();
    let worlds = worlds(g, world, &lens, &labels, vars);
    let ran = g.take_ran().into_iter().filter(|(_, c)| !internal(g, c)).map(|(node, cell)| RanView { node, cell }).collect();
    View {
        workspace: WorkspaceView { name: o.config.name.clone(), about: o.config.about.clone(), id: o.id.clone(), location: o.location.clone(), systems: o.built.names.clone(), sample: o.sample.clone(), direct_writes: o.config.llm_direct_writes },
        last_seq: o.log.last_seq(),
        undo: o.log.undo_next(),
        redo: o.log.redo_next(),
        nodes,
        unwritten,
        constants,
        edges,
        components: g.registry().components().cloned().collect(),
        cells: g.registry().cells().filter(|c| !c.internal).map(cell_info).collect(),
        worlds,
        checks: checks(&o.log, &labels),
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

pub fn try_formula(o: &Workspace, e: Entity, text: &str, world: &World) -> Tried {
    let g = o.log.graph();
    let labels = Labels::gather(g);
    match recollect_systems::formulas::try_formula(g, e, text) {
        Ok((value, expansion)) => {
            let lens = Lens::new(g, world);
            let narrowed = lens.narrow(&Shown::cond(&value));
            Tried { shown: Some(show(&narrowed, &labels, &lens.in_play)), expansion: Some(relabel(&expansion, &labels).replace(['“', '”'], "")), error: None }
        }
        Err(err) => Tried { shown: None, expansion: None, error: Some(describe_error(&err, &labels)) },
    }
}

/// A cell, by its node and name.
#[derive(Debug, Clone, Deserialize)]
pub struct CellRef {
    pub node: Entity,
    pub cell: String,
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
pub fn sweep(o: &Workspace, set: &CellRef, values: &[f64], read: &[CellRef], world: &World) -> Result<Vec<Swept>, String> {
    let g = o.log.graph();
    if !g.entities().any(|e| e == set.node) {
        return Err(format!("there's no node {}", set.node));
    }
    let labels = Labels::gather(g);
    let mut copy = g.fork();
    let mut vars = BTreeMap::new();
    Ok(values
        .iter()
        .map(|&at| {
            copy.set_value(set.node, &set.cell, json!(at));
            let lens = Lens::new(&copy, world);
            Swept { at, cells: read.iter().map(|r| cell_view(&copy, r.node, &r.cell, &lens, &labels, &mut vars)).collect() }
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
pub fn rows(o: &Workspace, table: &str, from: i64, to: i64, world: &World) -> Vec<RowCells> {
    let g = o.log.graph();
    let labels = Labels::gather(g);
    let lens = Lens::new(g, world);
    let mut vars = BTreeMap::new();
    (from..=to)
        .filter_map(|n| {
            let e = g.row_at(table, &recollect_graph::table::Key::Num(n))?;
            let cells = cells(g, e, &lens, &labels, &mut vars);
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

pub fn planning(o: &Workspace, world: &World) -> Result<PlanView, String> {
    if !o.built.names.iter().any(|n| n == "plan") {
        return Ok(PlanView { work: vec![], decisions: vec![] });
    }
    let g = o.log.graph();
    let labels = Labels::gather(g);
    let work = plan::work(g, world).map_err(|e| describe_error(&e, &labels))?;
    let decisions = plan::matrices(g, world).map_err(|e| describe_error(&e, &labels))?;
    Ok(PlanView { work, decisions })
}

/// An op, in words someone can read back.
#[derive(Debug, Serialize)]
pub struct OpView {
    /// What it does: "create", "delete", "add", "remove", "link", "unlink", "set" or "clear".
    pub verb: &'static str,
    pub node: Option<Entity>,
    /// For an edge, its target.
    pub other: Option<Entity>,
    /// The component or cell.
    pub what: Option<String>,
    pub value: Option<Value>,
    /// What's set is a formula.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub formula: bool,
}

pub fn op_view(op: &Op) -> OpView {
    let plain = |verb, node, other, what: &str| OpView { verb, node: Some(node), other, what: Some(what.to_string()), value: None, formula: false };
    match op {
        Op::Spawn { id } => OpView { verb: "create", node: Some(*id), other: None, what: None, value: None, formula: false },
        Op::Despawn { id } => OpView { verb: "delete", node: Some(*id), other: None, what: None, value: None, formula: false },
        Op::Add { id, component } => plain("add", *id, None, component),
        Op::Remove { id, component } => plain("remove", *id, None, component),
        Op::Link { from, to, component } => plain("link", *from, Some(*to), component),
        Op::Unlink { from, to, component } => plain("unlink", *from, Some(*to), component),
        Op::Set { on, cell, to } => {
            let (node, other) = match *on {
                On::Node { id } => (id, None),
                On::Edge { from, to } => (from, Some(to)),
            };
            let (verb, value, formula) = match to {
                None => ("clear", None, false),
                Some(Stored::Value(v)) => ("set", Some(v.clone()), false),
                Some(Stored::Formula(t)) => ("set", Some(json!(t)), true),
            };
            OpView { verb, node: Some(node), other, what: Some(cell.clone()), value, formula }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EventView {
    pub seq: u64,
    /// The event this one takes back, if it's an undo or a redo.
    pub reverts: Option<u64>,
    pub at: u64,
    pub author: String,
    pub why: String,
    pub ops: Vec<OpView>,
}

pub fn event_view(ev: &Event) -> EventView {
    EventView { seq: ev.seq, reverts: ev.reverts, at: ev.at, author: ev.author.clone(), why: ev.why.clone(), ops: ev.ops.iter().map(op_view).collect() }
}

/// What's happened since it was opened, newest first, or only what touched one node.
pub fn history(o: &Workspace, node: Option<Entity>) -> Vec<EventView> {
    o.log.events().iter().rev().filter(|ev| node.is_none_or(|n| ev.ops.iter().any(|op| op.target().nodes().contains(&n)))).map(event_view).collect()
}

/// A proposal as the inbox shows it.
#[derive(Debug, Serialize)]
pub struct ProposalBrief {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub author: String,
    pub created: u64,
    pub error: Option<String>,
    pub ops: Vec<OpView>,
}

pub fn proposals(o: &Workspace) -> Vec<ProposalBrief> {
    o.proposal_views()
        .into_iter()
        .map(|p| ProposalBrief { ops: p.proposal.ops.iter().map(op_view).collect(), id: p.proposal.id, title: p.proposal.title, rationale: p.proposal.rationale, author: p.proposal.author, created: p.proposal.created, error: p.error })
        .collect()
}

// ---------- for reading in smaller pieces, as Claude does ----------

#[derive(Debug, Serialize)]
pub struct Brief {
    pub id: Entity,
    pub name: String,
    pub components: Vec<String>,
}

fn brief(o: &Workspace, e: Entity) -> Brief {
    let g = o.log.graph();
    Brief { id: e, name: recollect_systems::label_of(g, e), components: g.components_of(e).keys().filter(|c| *c != Named::NAME).cloned().collect() }
}

pub fn list(o: &Workspace, component: Option<&str>) -> Vec<Brief> {
    let g = o.log.graph();
    g.entities().filter(|e| component.is_none_or(|c| g.has(*e, c))).map(|e| brief(o, e)).collect()
}

pub fn search(o: &Workspace, query: &str, limit: usize) -> Vec<Brief> {
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
    pub outgoing: Vec<EdgeBrief>,
    pub incoming: Vec<EdgeBrief>,
    pub history: Vec<EventView>,
}

#[derive(Debug, Serialize)]
pub struct EdgeBrief {
    pub components: Vec<String>,
    pub node: Entity,
    pub name: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, CellView>,
}

pub fn node(o: &Workspace, e: Entity, world: &World) -> NodeDetail {
    let g = o.log.graph();
    let labels = Labels::gather(g);
    let lens = Lens::new(g, world);
    let mut vars = BTreeMap::new();
    let mine = cells(g, e, &lens, &labels, &mut vars);
    let (mut outgoing, mut incoming) = (Vec::new(), Vec::new());
    for (edge, from, to) in g.edges().filter(|(_, f, t)| *f == e || *t == e) {
        let other = if from == e { to } else { from };
        let brief = EdgeBrief { components: g.given(edge).into_iter().collect(), node: other, name: labels.of(&other.to_string()), cells: cells(g, edge, &lens, &labels, &mut vars) };
        if from == e { outgoing.push(brief) } else { incoming.push(brief) }
    }
    NodeDetail { node: node_view(g, e, labels.of(&e.to_string()), mine), outgoing, incoming, history: history(o, Some(e)) }
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
pub fn tree(o: &Workspace, root: Entity, depth: usize) -> TreeNode {
    let kinds = o.log.graph().registry().components().filter(|c| c.arranges).map(|c| c.name.clone()).collect::<Vec<_>>();
    let labels = Labels::gather(o.log.graph());
    fn walk(o: &Workspace, kinds: &[String], labels: &Labels, e: Entity, depth: usize, seen: &mut Vec<Entity>) -> TreeNode {
        let g = o.log.graph();
        let shared = seen.contains(&e);
        seen.push(e);
        let mut children = Vec::new();
        if !shared && depth > 0 {
            let mut below: Vec<Entity> = kinds.iter().flat_map(|k| if g.registry().component_def(k).is_some_and(|c| c.reversed) { g.targets_of(e, k) } else { g.sources_of(e, k) }).collect();
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

#[derive(Serialize)]
pub struct Overview {
    pub workspace: WorkspaceView,
    pub nodes: usize,
    /// Every component there is, with the cells it brings.
    pub components: Vec<ComponentDef>,
    pub cells: Vec<CellInfo>,
    pub checks: Vec<CheckView>,
    pub open_choices: Vec<VarView>,
    pub pending_proposals: usize,
    pub events: usize,
}

pub fn overview(o: &Workspace) -> Overview {
    let v = view(o, &World::new());
    Overview {
        nodes: v.nodes.len(),
        workspace: v.workspace,
        components: v.components,
        cells: v.cells,
        checks: v.checks,
        open_choices: v.worlds.vars,
        pending_proposals: v.proposals,
        events: o.log.events().len(),
    }
}
