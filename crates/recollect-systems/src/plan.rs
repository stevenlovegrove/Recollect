//! The planning system: work, decisions held open, and what's derived from them. It's written with the
//! same API a plugin uses, so the core knows nothing about any of this. See `docs/components.md`.
//!
//! What's written says what was decided and stated. Everything else is worked out, and every derived
//! value is exact: the set of worlds in which it holds, where a world is one way every open decision
//! could land.
//!
//! - Part is what's in a plan's structure: anything that's part of something, or has parts, or is an
//!   option. It knows whether it's needed, whether it's done, and what anything beneath waits for.
//! - Work is what's owed until it's done: a task, a goal, a question, a risk, or a gap.
//! - A decision is a choice whose options are linked to it by `alternative` edges, and only the one it
//!   lands on is needed. It's settled by writing that option in its `chosen` cell.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use serde_json::json;

use recollect_graph::chance::{self, Num, Reader, Share, Term};
use recollect_graph::worlds::{Bool, Cond, Picks, Space, Worldly};
use recollect_graph::{Cell, CellType, Cx, Entity, Error, Graph, Op, Registry, Severity, Shown, cell};

use crate::expr::{Type, Val};
use crate::formulas::{self, Chosen, compiler};
// Structure lives in `structure.rs`. What other crates know by these names is still here too.
use crate::structure::{ALTERNATIVE, DECISION, Need, PART, PART_OF, RuledOut, Weight, picks, pinned};
use crate::structure::{in_play_parents, is_dropped, is_live, is_open, listed_in, live_parts, selects, settled_on};
use crate::{label_of, labels};

/// What a workspace calls this system.
pub const NAME: &str = "plan";

pub const WORK: &str = "work";
pub const TASK: &str = "task";
pub const GOAL: &str = "goal";
pub const QUESTION: &str = "question";
pub const RISK: &str = "risk";
pub const UNKNOWN: &str = "unknown";
pub const DEPENDS_ON: &str = "depends_on";

/// How long a task takes, in days. It can be typed in or worked out, so it can differ between worlds.
/// `None` where a formula says that isn't known, which makes its dates unknowable.
#[cell]
pub struct Estimate(pub Worldly<Option<Num>>);

impl Estimate {
    pub fn days(d: f64) -> Self {
        Estimate(Worldly::plain(Some(Num::Plain(d))))
    }
}

/// Whether it's finished, per world. Undefined where nothing about it can be finished: something is
/// owed by being work, or by having parts that are. It's worked out from what's beneath, and whatever a
/// node says for itself is what counts instead.
#[cell]
pub struct Done(pub Worldly<Option<bool>>);

impl Done {
    pub fn yes() -> Self {
        Done(Worldly::plain(Some(true)))
    }

    /// The worlds in which something here is still owed. This is what a dependent waits for.
    pub fn open(&self) -> Bool {
        self.0.0.holds(|c| *c == Some(false))
    }

    /// The worlds in which it's been finished.
    pub fn finished(&self) -> Bool {
        self.0.0.holds(|c| *c == Some(true))
    }
}

/// What a dependency waits for.
#[derive(Copy, Default, Eq)]
#[cell]
#[serde(rename_all = "lowercase")]
pub enum WaitFor {
    #[default]
    Done,
    Decided,
}

// Waiting, as two cells on everything in a plan, like the two ends of a bar on a Gantt chart. A node
// can't start until what it depends on has finished and its parents have started, and it can't finish
// until it's started and its parts have finished. So work that could never complete is exactly a cycle
// among these cells, and the core already reports those. They hold nothing else.

#[cell]
pub struct CanStart;

#[cell]
pub struct CanFinish;

fn can_start(cx: &Cx) -> Cell<CanStart> {
    for parent in cx.targets(PART_OF).into_iter().chain(cx.targets(ALTERNATIVE)) {
        cx.value::<CanStart>(parent)?;
    }
    for target in cx.targets(DEPENDS_ON) {
        cx.value::<CanFinish>(target)?;
    }
    Ok(Some(CanStart))
}

fn can_finish(cx: &Cx) -> Cell<CanFinish> {
    cx.own_value::<CanStart>()?;
    for child in cx.sources(PART_OF).into_iter().chain(cx.sources(ALTERNATIVE)) {
        cx.value::<CanFinish>(child)?;
    }
    Ok(Some(CanFinish))
}

/// Whether this node is finished in every world.
fn all_done(cx: &Cx) -> Result<bool, Error> {
    Ok(cx.own::<Done>()?.is_some_and(|d| d.finished().is_always()))
}

/// Work is owed until it's done. Above a leaf, a decision that lands on nothing can't complete. Whatever
/// a node says for itself replaces all of this, and if that disagrees with what's beneath, that's a
/// notice.
const DONE: &str = "if(self.dropped, undefined, if(is_leaf(), if(has(self.work), false, undefined), done_beneath(if(has(self.decision), if(defined(self.chosen), self.chosen.done, false), undefined))))";

// ---------- blocking ----------

/// What this waits for, by name, with the worlds in which it does: its own dependencies, and those of
/// whatever it's in play through. Nothing listed is `never`.
#[cell]
pub struct Blockers(pub Vec<(Entity, Bool)>);

/// The worlds in which anything holds this up.
#[cell]
pub struct Blocked(pub Bool);

/// For a live task with no live parts, the worlds in which it could be started now.
#[cell]
pub struct Actionable(pub Bool);

fn blockers(cx: &Cx) -> Cell<Blockers> {
    let space = Space::global();
    let mut out: BTreeMap<Entity, Bool> = BTreeMap::new();
    let mut add = |target: Entity, waits: Bool| {
        let so_far = out.get(&target).cloned().unwrap_or_else(|| space.never());
        out.insert(target, &so_far | &waits);
    };
    for (target, edge) in cx.outgoing(DEPENDS_ON) {
        if !is_live(cx, target)? {
            continue;
        }
        let waits = match cx.get::<WaitFor>(edge)?.unwrap_or_default() {
            WaitFor::Decided => space.truth(cx.has(target, DECISION) && !cx.is_stored(target, Chosen::NAME)),
            WaitFor::Done => &cx.value::<Need>(target)?.0 & &cx.get::<Done>(target)?.map_or_else(|| space.never(), |d| d.open()),
        };
        add(target, waits);
    }
    for (parent, via) in in_play_parents(cx)? {
        for (target, waits) in cx.value::<Blockers>(parent)?.0 {
            add(target, &via & &waits);
        }
    }
    Ok(Some(Blockers(out.into_iter().filter(|(_, waits)| !waits.is_never()).collect())))
}

fn blocked(cx: &Cx) -> Cell<Blocked> {
    Ok(Some(Blocked(Space::global().any(cx.own_value::<Blockers>()?.0.into_iter().map(|(_, waits)| waits)))))
}

fn actionable(cx: &Cx) -> Cell<Actionable> {
    if !is_live(cx, cx.id())? || !live_parts(cx)?.is_empty() {
        return Ok(None);
    }
    let open = cx.own::<Done>()?.map_or_else(|| Space::global().always(), |d| d.open());
    let wanted = &cx.own_value::<Need>()?.0 & &open;
    Ok(Some(Actionable(&wanted & &!&cx.own_value::<Blocked>()?.0)))
}

// ---------- scope: where a node belongs ----------

/// The nearest node every path from the top must pass through. It's derived, so adding a second parent
/// moves a node with nothing to maintain, and it ignores decisions and dropping, so nothing jumps around
/// when you decide. Absent at the top level.
#[cell]
pub struct Scope(pub Entity);

/// How many scopes a node is inside.
#[derive(Copy)]
#[cell]
pub struct ScopeDepth(pub usize);

fn scope(cx: &Cx) -> Cell<Scope> {
    let parents: BTreeSet<Entity> = cx.targets(PART_OF).into_iter().chain(cx.targets(ALTERNATIVE)).collect();
    let mut parents = parents.into_iter();
    let Some(mut common) = parents.next() else { return Ok(None) };
    for p in parents {
        match meet(cx, common, p)? {
            Some(m) => common = m,
            None => return Ok(None),
        }
    }
    Ok(Some(Scope(common)))
}

/// The closest node containing both, each counting as containing itself.
fn meet(cx: &Cx, mut a: Entity, mut b: Entity) -> Result<Option<Entity>, Error> {
    let (mut da, mut db) = (cx.value::<ScopeDepth>(a)?.0, cx.value::<ScopeDepth>(b)?.0);
    while a != b {
        let (deeper, depth) = if da >= db { (&mut a, &mut da) } else { (&mut b, &mut db) };
        let Some(Scope(up)) = cx.get::<Scope>(*deeper)? else { return Ok(None) };
        *deeper = up;
        *depth -= 1;
    }
    Ok(Some(a))
}

fn scope_depth(cx: &Cx) -> Cell<ScopeDepth> {
    Ok(Some(ScopeDepth(match cx.own::<Scope>()? {
        Some(Scope(up)) => cx.value::<ScopeDepth>(up)?.0 + 1,
        None => 0,
    })))
}

/// The planning system: its components, what's worked out for them, and what must hold.
pub fn install(reg: &mut Registry) {
    crate::structure::install(reg);

    reg.cell::<Done>(PART, "Whether it's finished, in each world: worked out from what's beneath it, or typed in.")
        .compiles(compiler::<Done>(Type::Bool, |c| formulas::flag_cases(c, "done").map(Done)))
        .default_formula(DONE)
        .shown(|d| Shown::cond(&d.0.0))
        .truth();
    reg.cell::<Blockers>(PART, "What anything here waits for, and in which worlds.").builtin(blockers).internal();
    reg.cell::<Scope>(PART, "The nearest node every path from the top passes through.").builtin(scope).internal();
    reg.cell::<ScopeDepth>(PART, "").builtin(scope_depth).internal();
    reg.cell::<CanStart>(PART, "").builtin(can_start).internal();
    reg.cell::<CanFinish>(PART, "").builtin(can_finish).internal();

    reg.component(WORK, "Something owed until it's done.").requires(PART);
    reg.cell::<Blocked>(WORK, "The worlds where something holds this up.").builtin(blocked).shown(|b| Shown::Bool(b.0.clone()));

    reg.component(TASK, "Work of its own, which done discharges.").icon("▸").requires(WORK);
    reg.cell::<Estimate>(TASK, "How long it takes, in days: typed in, or a formula.").compiles(compiler::<Estimate>(Type::Number, |c| formulas::number_cases(c, "an estimate").map(Estimate))).shown(|e| Shown::cond(&e.0.0)).initial(json!(1.0));
    reg.cell::<Actionable>(TASK, "The worlds where this could be started now.").builtin(actionable).shown(|a| Shown::Bool(a.0.clone())).summary();
    reg.component(GOAL, "A result you want. Break it into parts, or let other things serve it.").icon("◎").requires(WORK);
    reg.component(QUESTION, "Something not yet known. Done means it's been answered.").icon("?").requires(WORK);
    reg.component(RISK, "Something that could go wrong. Link it to what it threatens.").icon("!").requires(WORK);
    reg.component(UNKNOWN, "A gap: something here isn't specified yet. Removing it says the gap is closed.").icon("…").requires(WORK);

    reg.edge(DEPENDS_ON, "Waits for that.").orders().source_has(PART).target_has(PART);
    reg.cell::<WaitFor>(DEPENDS_ON, "What it waits for: that to be done, or decided.").default_value(WaitFor::Done);

    checks(reg);
    reg.advice(blocking_decisions);
}

fn checks(reg: &mut Registry) {
    use Severity::{Invariant, Notice};

    reg.check(PART, "no_deadlock", Invariant, |cx| match cx.own::<CanFinish>() {
        Err(Error::Cycle(_)) => Ok(vec!["can never finish: it waits on itself, directly or through something else".to_string()]),
        Err(e) => Err(e),
        Ok(_) => Ok(vec![]),
    });

    reg.check(UNKNOWN, "unknown_has_no_parts", Invariant, |cx| Ok(cx.sources(PART_OF).into_iter().map(|c| format!("{c} is part of this gap: give it a real node instead")).collect()));

    reg.check(UNKNOWN, "unknown_cannot_be_done", Invariant, |cx| {
        // What's beneath a gap can finish. It's saying so of the gap itself that isn't allowed.
        let done = cx.own_is_stored(Done::NAME) && cx.own::<Done>()?.is_some_and(|d| !d.finished().is_never());
        Ok(if done { vec!["a gap can't be done: remove it to close it".to_string()] } else { vec![] })
    });

    reg.check(DECISION, "unknown_cannot_be_chosen", Invariant, |cx| {
        let Some(c) = settled_on(cx, cx.id())? else { return Ok(vec![]) };
        Ok(if cx.has(c, UNKNOWN) { vec![format!("chose {c}, which is a gap: name the option instead")] } else { vec![] })
    });

    // What's worked out can't disagree with what's beneath it, so this is only ever what a node said for
    // itself.
    reg.check(PART, "stated_done_with_open_work", Notice, |cx| {
        if !cx.own_is_stored(Done::NAME) || !is_live(cx, cx.id())? {
            return Ok(vec![]);
        }
        let Some(done) = cx.own::<Done>()? else { return Ok(vec![]) };
        let space = Space::global();
        let mut beneath = space.never();
        for p in cx.sources(PART_OF) {
            beneath = &beneath | &cx.get::<Done>(p)?.map_or_else(|| space.never(), |d| d.open());
        }
        if cx.own_has(DECISION)
            && let Some(chosen) = cx.own::<Chosen>()?
        {
            // A decision that lands on nothing can't complete.
            beneath = &beneath | &chosen.0.0.holds(|v| *v == Val::Undefined);
            for o in cx.sources(ALTERNATIVE) {
                let open = cx.get::<Done>(o)?.map_or_else(|| space.never(), |d| d.open());
                beneath = &beneath | &(&selects(cx, cx.id(), o)? & &open);
            }
        }
        let open = &done.finished() & &beneath;
        Ok(if open.is_never() { vec![] } else { vec![format!("marked done, with what's beneath it still open: {}", open.show())] })
    });

    reg.check(GOAL, "undeveloped_goal", Notice, |cx| {
        if !is_live(cx, cx.id())? || all_done(cx)? {
            return Ok(vec![]);
        }
        let means = live_parts(cx)?.len() + cx.incoming("serves").len();
        Ok(if means == 0 { vec!["nothing decomposes or serves this yet".to_string()] } else { vec![] })
    });

    reg.check(RISK, "risk_without_target", Notice, |cx| {
        if !is_live(cx, cx.id())? || all_done(cx)? {
            return Ok(vec![]);
        }
        Ok(if cx.outgoing("contradicts").is_empty() { vec!["it isn't linked to what it threatens".to_string()] } else { vec![] })
    });

    reg.check(TASK, "oversized_task", Notice, |cx| {
        let Some(estimate) = cx.own::<Estimate>()? else { return Ok(vec![]) };
        let mut how_often: Option<chance::Chance> = None;
        let mut big = false;
        for (_, days) in estimate.0.0.cases() {
            match days {
                Some(Num::Plain(d)) => big |= *d > BIG_TASK,
                // Whether an uncertain estimate is over is a matter of chance, unless it can't be.
                Some(Num::Rand(r)) => match chance::bin(chance::Op::Lt, &Term::Num(BIG_TASK), &Term::Rand(*r)) {
                    Term::Flag(over) => big |= over,
                    over => {
                        let c = Reader::new().chance(&[Share { weight: 1.0, value: over }]);
                        if c.chance >= LIKELY_BIG && how_often.as_ref().is_none_or(|h| c.chance > h.chance) {
                            how_often = Some(c);
                        }
                    }
                },
                None => {}
            }
        }
        if !(big || how_often.is_some()) || all_done(cx)? || !live_parts(cx)?.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![match how_often.filter(|_| !big) {
            Some(c) if c.exact => format!("a task with no breakdown that's big {:.0}% of the time", c.chance * 100.0),
            Some(c) => format!("a task with no breakdown that's big in about {:.0}% of {} draws", c.chance * 100.0, c.draws),
            None => "a big task with no breakdown".to_string(),
        }])
    });

}

/// A new decision, with a gap for the options nobody has named yet.
pub fn new_decision(id: Entity, gap: Entity) -> Vec<Op> {
    vec![Op::add(id, DECISION), Op::add(gap, UNKNOWN), Op::link(gap, id, ALTERNATIVE)]
}

/// Say a node isn't fully specified yet.
pub fn mark_incomplete(id: Entity, gap: Entity) -> Vec<Op> {
    vec![Op::add(gap, UNKNOWN), Op::link(gap, id, PART_OF)]
}

/// Say a gap is closed: for a decision (`edge` is `alternative`), these are all the options there are.
pub fn close_gap(gap: Entity, under: Entity, edge: &str) -> Vec<Op> {
    vec![Op::Unlink { from: gap, to: under, component: edge.to_string() }, Op::Despawn { id: gap }]
}

/// Open decisions where safe work has run out.
fn blocking_decisions(g: &Graph) -> Vec<recollect_graph::Advice> {
    let mut out = Vec::new();
    for e in g.having(DECISION) {
        if let Ok(Pressure::Blocking(work)) = pressure(g, e) {
            let plural = if work.len() == 1 { "task" } else { "tasks" };
            out.push(recollect_graph::Advice { rule: "blocking_decision".to_string(), on: e, detail: format!("only work specific to an option is left: {} {plural} depend on deciding this", work.len()) });
        }
    }
    out
}

/// Whether work is waiting on an open decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Pressure {
    /// Some work under it is safe whichever named option is taken: no need to decide yet.
    Free(Vec<Entity>),
    /// Only work specific to an option remains: this decision is what's in the way.
    Blocking(Vec<Entity>),
    /// Open, with no actionable work under it either way.
    Idle,
    /// Not an open decision, so pressure doesn't apply.
    Decided,
}

pub fn pressure(g: &Graph, decision: Entity) -> Result<Pressure, Error> {
    if !is_open(g, decision)? {
        return Ok(Pressure::Decided);
    }
    let mut under = BTreeSet::new();
    let mut stack = vec![decision];
    while let Some(cur) = stack.pop() {
        for child in g.sources_of(cur, PART_OF).into_iter().chain(g.sources_of(cur, ALTERNATIVE)) {
            if under.insert(child) {
                stack.push(child);
            }
        }
    }
    let (mut safe, mut specific) = (Vec::new(), Vec::new());
    for e in under {
        let Some(Actionable(f)) = g.get::<Actionable>(e)? else { continue };
        if f.holds_among_known() {
            safe.push(e);
        } else if !f.is_never() && f.support().contains(&decision.to_string()) {
            specific.push(e);
        }
    }
    Ok(if !safe.is_empty() {
        Pressure::Free(safe)
    } else if !specific.is_empty() {
        Pressure::Blocking(specific)
    } else {
        Pressure::Idle
    })
}

/// Why a node reads as it does: one level of inputs, each with its own exact formula. `label`
/// translates handles for reading.
pub fn explain(g: &Graph, e: Entity, label: &dyn Fn(&str) -> String) -> Result<String, Error> {
    let name = |e: Entity| label(&e.to_string());
    let show = |b: &Bool| b.show_with(label);
    let open = |e: Entity| Ok::<_, Error>(g.get::<Done>(e)?.map_or_else(|| "never".to_string(), |c| show(&c.open())));
    let need = g.get::<Need>(e)?.map_or_else(|| "never".to_string(), |n| show(&n.0));
    let blocked = g.get::<Blockers>(e)?.map_or_else(|| "never".to_string(), |b| show(&Space::global().any(b.0.into_iter().map(|(_, w)| w))));
    let mut out = format!("{}\n  need: {need}\n  open: {}\n  blocked: {blocked}\n", name(e), open(e)?);
    if g.has_own(e, Done::NAME) {
        out += "  stated done\n";
    }
    for c in g.sources_of(e, PART_OF) {
        out += &format!("  requires {}: open {}\n", name(c), open(c)?);
    }
    for o in g.sources_of(e, ALTERNATIVE) {
        out += &format!("  option {}: open {}\n", name(o), open(o)?);
    }
    for (b, waits) in g.get::<Blockers>(e)?.map_or_else(Vec::new, |b| b.0) {
        out += &format!("  waits for {}: {}\n", name(b), show(&waits));
    }
    Ok(out)
}

/// How likely, among the worlds consistent with `picks`, and the formula for reading.
fn read(b: &Bool, label: &dyn Fn(&str) -> String) -> Reading {
    Reading { when: b.show_with(label), prob: b.prob(), always: b.is_always(), never: b.is_never() }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub when: String,
    pub prob: f64,
    pub always: bool,
    pub never: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatrixOption {
    pub id: Entity,
    pub name: String,
    /// Share of the live options.
    pub weight: f64,
    pub gap: bool,
    pub ruled_out: bool,
    pub dropped: bool,
    pub chosen: bool,
    /// One the decision lists by name, with no node or link of its own, so there's nothing to rule out.
    pub listed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatrixRow {
    pub id: Entity,
    pub name: String,
    pub depth: usize,
    /// How much of each option's remaining worlds need this.
    pub per_option: BTreeMap<Entity, f64>,
    pub need: Reading,
    pub finished: bool,
    pub is_work: bool,
    pub actionable: Option<Reading>,
    pub blockers: Vec<Blocker>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blocker {
    pub id: Entity,
    pub name: String,
    pub when: String,
}

/// What a decision means for the work beneath it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Matrix {
    pub id: Entity,
    pub name: String,
    pub chosen: Option<Entity>,
    pub options: Vec<MatrixOption>,
    pub rows: Vec<MatrixRow>,
    pub pressure: Pressure,
}

impl Serialize for Pressure {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let (state, work): (&str, &[Entity]) = match self {
            Pressure::Free(w) => ("free", w),
            Pressure::Blocking(w) => ("blocking", w),
            Pressure::Idle => ("idle", &[]),
            Pressure::Decided => ("decided", &[]),
        };
        json!({ "state": state, "work": work }).serialize(ser)
    }
}

fn beneath(g: &Graph, root: Entity) -> BTreeMap<Entity, usize> {
    let mut depth = BTreeMap::new();
    let mut frontier = vec![root];
    let mut d = 0;
    while !frontier.is_empty() {
        d += 1;
        let mut next = Vec::new();
        for cur in frontier {
            for child in g.sources_of(cur, PART_OF).into_iter().chain(g.sources_of(cur, ALTERNATIVE)) {
                if child != root && !depth.contains_key(&child) {
                    depth.insert(child, d);
                    next.push(child);
                }
            }
        }
        frontier = next;
    }
    depth
}

/// An option's weight, as one number: the first case of what its edge says.
fn weight_of(g: &Graph, edge: Entity) -> f64 {
    g.get::<Weight>(edge).ok().flatten().and_then(|w| w.0.0.cases().first().and_then(|(_, w)| **w)).unwrap_or(0.0)
}

/// Every decision, open ones first. `world` narrows everything, as a what-if does.
pub fn matrices(g: &Graph, world: &BTreeMap<String, String>) -> Result<Vec<Matrix>, Error> {
    let names = labels(g);
    let label = |s: &str| names.get(s).cloned().unwrap_or_else(|| s.to_string());
    let space = Space::global();
    let picks = picks(g, world);
    let found = pinned(g, world).given(&picks);
    let mut out = Vec::new();
    for d in g.having(DECISION) {
        let chosen = if g.stored(d, Chosen::NAME).is_some() { g.get::<Chosen>(d)?.and_then(|c| c.settled()) } else { None };
        let mut options = Vec::new();
        let mut live = Vec::new();
        for (o, edge) in g.edges_into(d, ALTERNATIVE) {
            let dropped = is_dropped(g, o)?;
            let ruled_out = g.get::<RuledOut>(edge)?.is_some();
            if !ruled_out && !dropped {
                live.push((o, weight_of(g, edge)));
            }
            options.push(MatrixOption { id: o, name: label_of(g, o), weight: 0.0, gap: g.has(o, UNKNOWN), ruled_out, dropped, chosen: chosen == Some(o), listed: false });
        }
        for o in listed_in(g, d) {
            live.push((o, 1.0));
            options.push(MatrixOption { id: o, name: label_of(g, o), weight: 0.0, gap: false, ruled_out: false, dropped: false, chosen: chosen == Some(o), listed: true });
        }
        let total: f64 = live.iter().map(|l| l.1).sum();
        for o in &mut options {
            if let Some((_, w)) = live.iter().find(|l| l.0 == o.id) {
                o.weight = if total > 0.0 { w / total } else { 1.0 / live.len() as f64 };
            }
        }

        let depth = beneath(g, d);
        let direct: BTreeSet<Entity> = options.iter().map(|o| o.id).collect();
        let mut rows = Vec::new();
        for (&e, &row_depth) in &depth {
            if direct.contains(&e) || is_dropped(g, e)? {
                continue;
            }
            let need = g.get::<Need>(e)?.map_or(space.never(), |n| n.0);
            let mut per_option = BTreeMap::new();
            for o in &options {
                let mut w = world.clone();
                w.insert(d.to_string(), o.id.to_string());
                let (picks, found) = (self::picks(g, &w), pinned(g, &w));
                let found = found.given(&picks);
                let held = found.prob();
                per_option.insert(o.id, if held > 0.0 { (&need.given(&picks) & &found).prob() / held } else { 0.0 });
            }
            let done = g.get::<Done>(e)?;
            rows.push(MatrixRow {
                id: e,
                name: label_of(g, e),
                depth: row_depth,
                per_option,
                need: read(&need.given(&picks).among(&found), &label),
                finished: done.is_some_and(|c| c.finished().given(&picks).holds_given(&found)),
                is_work: g.has(e, TASK) && g.sources_of(e, PART_OF).is_empty(),
                actionable: g.get::<Actionable>(e)?.map(|a| read(&a.0.given(&picks), &label)),
                blockers: blockers_of(g, e, &picks, &label)?,
            });
        }
        rows.sort_by(|a, b| b.need.prob.total_cmp(&a.need.prob).then(a.depth.cmp(&b.depth)).then(a.name.cmp(&b.name)));
        out.push(Matrix { id: d, name: label_of(g, d), chosen, options, rows, pressure: pressure(g, d)? });
    }
    out.sort_by_key(|m| m.chosen.is_some());
    Ok(out)
}

fn blockers_of(g: &Graph, e: Entity, picks: &Picks, label: &dyn Fn(&str) -> String) -> Result<Vec<Blocker>, Error> {
    let items = g.get::<Blockers>(e)?.map_or_else(Vec::new, |b| b.0);
    Ok(items.into_iter().map(|(b, f)| (b, f.given(picks))).filter(|(_, f)| !f.is_never()).map(|(b, f)| Blocker { id: b, name: label_of(g, b), when: f.show_with(label) }).collect())
}

/// Where a piece of work stands among the worlds still in play.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Could start now, whichever named option is taken.
    Now,
    /// Could start now in some worlds only: it depends on a decision.
    Depends,
    /// Needed, and held up.
    Waiting,
    Finished,
    /// No world still in play needs it.
    NotNeeded,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkItem {
    pub id: Entity,
    pub name: String,
    pub standing: Standing,
    /// As it reads among the worlds in play: one number, or a range.
    pub estimate: Option<String>,
    pub need: Reading,
    pub actionable: Reading,
    pub blockers: Vec<Blocker>,
    /// What it's inside.
    pub scope: Option<Blocker>,
}

/// How many days make a task big enough to want breaking down.
const BIG_TASK: f64 = 5.0;
/// How likely an uncertain estimate has to be to go over that before it's worth saying. A view should
/// be able to say this, and can't yet: see `docs/distributions.md`.
const LIKELY_BIG: f64 = 0.5;

fn estimate_text(c: &Cond<Option<Num>>) -> String {
    let mut reader = Reader::new();
    let mut uncertain = false;
    // An uncertain one reads as where it falls nine times in ten.
    let known: Vec<(f64, f64)> = c
        .cases()
        .iter()
        .filter_map(|(_, d)| match d {
            None => None,
            Some(Num::Plain(d)) => Some((*d, *d)),
            Some(Num::Rand(r)) => {
                uncertain = true;
                let s = reader.summary(&[Share { weight: 1.0, value: Term::Rand(*r) }]);
                Some((s.low, s.high))
            }
        })
        .collect();
    let (low, high) = (known.iter().map(|k| k.0).fold(f64::INFINITY, f64::min), known.iter().map(|k| k.1).fold(f64::NEG_INFINITY, f64::max));
    if known.is_empty() {
        "?".to_string()
    } else if uncertain {
        format!("about {} to {}", chance::figures(low), chance::figures(high))
    } else if low == high {
        low.to_string()
    } else {
        format!("{low} to {high}")
    }
}

/// All work that can be picked up, as things stand among the worlds in play.
pub fn work(g: &Graph, world: &BTreeMap<String, String>) -> Result<Vec<WorkItem>, Error> {
    let names: HashMap<String, String> = labels(g);
    let label = |s: &str| names.get(s).cloned().unwrap_or_else(|| s.to_string());
    let picks = picks(g, world);
    let found = pinned(g, world).given(&picks);
    let mut out = Vec::new();
    for e in g.entities() {
        let Some(actionable) = g.get::<Actionable>(e)? else { continue };
        let actionable = actionable.0.given(&picks).among(&found);
        let need = g.get::<Need>(e)?.map_or(Space::global().never(), |n| n.0).given(&picks).among(&found);
        let finished = g.get::<Done>(e)?.is_some_and(|c| c.finished().given(&picks).holds_given(&need));
        let standing = if need.is_never() {
            Standing::NotNeeded
        } else if finished {
            Standing::Finished
        } else if actionable.holds_among_known() {
            Standing::Now
        } else if !actionable.is_never() {
            Standing::Depends
        } else {
            Standing::Waiting
        };
        out.push(WorkItem {
            id: e,
            name: label_of(g, e),
            standing,
            estimate: g.get::<Estimate>(e)?.map(|d| estimate_text(&d.0.0.given(&picks))),
            need: read(&need, &label),
            actionable: read(&actionable, &label),
            blockers: blockers_of(g, e, &picks, &label)?,
            scope: g.get::<Scope>(e)?.map(|Scope(s)| Blocker { id: s, name: label_of(g, s), when: String::new() }),
        });
    }
    Ok(out)
}

/// How to use what this system gives, for an agent: see `mcp.rs` in the server.
pub const GUIDE: &str = "\
Structure is coarse to fine. `part_of` edges point child to parent, and all of a node's parts are needed. A `decision` node's options are \
ordinary nodes with an `alternative` edge to it, and only the option taken is needed, so work several options share is simply part_of \
each of them, and is safe to do before deciding. Finding that overlap is worth effort. Decide with {op: set, id: <decision>, cell: \
chosen, value: {ref: <option's handle>}}, and rule an option out with {op: set, from: <option>, to: <decision>, cell: ruled_out, value: \
\"why\"}. Give a decision `uncertain` when it isn't anyone's to choose: its options' weights then say how far each is believed, and \
what's expected averages over it. A `knowing` edge from a decision to something says it's made knowing how that turns out.\n\
\n\
`depends_on` is ordering, inherited by what's inside. An `unknown` node is a gap: it says something here isn't specified yet, and \
removing it is the claim that it's closed. `done` is true or false, and is worked out from what's beneath unless a node says otherwise. \
A stated `done` that disagrees with what's beneath gets a notice, so to finish something early, say so and drop what's left. `dropped` \
means deliberately out of the picture: prefer dropping to deleting.\n\
\n\
Reading a plan: `read {plan: true}` says where each piece of work stands (could start now, depends on a decision, waiting and on what, \
finished, or not needed) and what each decision is holding up (pressure: free, blocking, idle, decided). `read {nodes: {tree: {root: \
<node>}}, cells: [need]}` is what's beneath a node: read it before decomposing further, and reuse nodes wherever the same work appears. \
Stop when a leaf could be started today. Surface risks with a `contradicts` edge to what they threaten, and suggest what would \
discriminate between a decision's options.";
