//! The planning system: its stored facts, what's derived from them, and the rules they have to
//! obey. It's written with the same API a plugin uses, so the core knows nothing about any of
//! this. See `docs/graph-core.md`.
//!
//! Stored facts say what was decided and stated. Everything else is a rule, and every derived
//! value is exact: the set of worlds in which it holds, where a world is one way every open
//! decision could land.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;
use serde_json::{Value, json};

use recollect_graph::worlds::{Bool, Cond, Space, Worldly};
use recollect_graph::{Component, Compute, Cx, Entity, Error, Graph, Inputs, Op, Registry, RuleMeta, component, rule};

use crate::expr::{Type, Val};
use crate::formulas;
use crate::{Advice, Catalog, Shown, label_of, labels};

/// Work that can be picked up.
#[derive(Default)]
#[component]
pub struct Task {}

/// How long a task takes, in days. It can be typed in or worked out, so it can differ between
/// worlds. `None` where a formula says that isn't known, which makes its dates unknowable.
#[component]
pub struct Estimate(pub Worldly<Option<f64>>);

impl Estimate {
    pub fn days(d: f64) -> Self {
        Estimate(Worldly::plain(Some(d)))
    }
}

/// What a formula gives an estimate.
fn estimate_of(c: Cond<Val>) -> Result<Estimate, Error> {
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !matches!(v, Val::Num(_) | Val::Undefined)) {
        return Err(Error::Failed(format!("an estimate is a number of days, not {}", Type::of(bad).word())));
    }
    Ok(Estimate(Worldly(c.map(|v| if let Val::Num(n) = v { Some(*n) } else { None }))))
}

#[component]
pub struct Goal;

#[component]
pub struct Question;

#[component]
pub struct Risk;

/// A gap: something here isn't specified yet. It can't be done or chosen, so whatever requires it
/// stays incomplete until someone removes it, which says the gap is closed.
#[component]
pub struct Unknown;

/// Stated complete.
#[component]
pub struct Done;

/// Deliberately out of the picture.
#[component]
pub struct Dropped;

/// An open question with options, and which one was taken, if any.
#[derive(Default)]
#[component]
pub struct Decision {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Entity>,
}

/// Child → parent: AND.
#[component]
pub struct PartOf;

/// Option → decision: OR.
#[derive(Default)]
#[component]
pub struct Alternative {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruled_out: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

#[derive(Copy, Default)]
#[component]
#[serde(rename_all = "lowercase")]
pub enum WaitFor {
    #[default]
    Done,
    Decided,
}

/// From waits for to.
#[derive(Default)]
#[component]
pub struct DependsOn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<WaitFor>,
}

// Waiting, as two cells on every node, like the two ends of a bar on a Gantt chart. A node can't
// start until what it depends on has finished and its parents have started, and it can't finish
// until it's started and its parts have finished. So work that could never complete is exactly a
// cycle among these cells, and the core already reports those. They hold nothing else.

#[component]
pub struct CanStart;

#[component]
pub struct CanFinish;

#[rule("plan/can_start", outgoing(PartOf, Alternative, DependsOn))]
pub struct CanStartRule;

impl Compute for CanStartRule {
    type Output = CanStart;
    fn compute(&self, cx: &Cx) -> Result<CanStart, Error> {
        for parent in cx.targets::<PartOf>()?.into_iter().chain(cx.targets::<Alternative>()?) {
            cx.value::<CanStart>(parent)?;
        }
        for target in cx.targets::<DependsOn>()? {
            cx.value::<CanFinish>(target)?;
        }
        Ok(CanStart)
    }
}

#[rule("plan/can_finish", incoming(PartOf, Alternative))]
pub struct CanFinishRule;

impl Compute for CanFinishRule {
    type Output = CanFinish;
    fn compute(&self, cx: &Cx) -> Result<CanFinish, Error> {
        cx.own_value::<CanStart>()?;
        for child in cx.sources::<PartOf>()?.into_iter().chain(cx.sources::<Alternative>()?) {
            cx.value::<CanFinish>(child)?;
        }
        Ok(CanFinish)
    }
}

pub(crate) fn is_live(cx: &Cx, e: Entity) -> Result<bool, Error> {
    Ok(cx.get::<Dropped>(e)?.is_none())
}

// ---------- what's being attempted, per world ----------

/// How an open decision could land. A chosen one is a constant, and one with no live options
/// lands on nothing. Only decisions have this.
#[component]
pub struct Choice(pub Cond<Option<Entity>>);

/// The worlds in which this is part of what's being attempted.
#[component]
pub struct Need(pub Bool);

#[rule("plan/choice", incoming(Alternative))]
pub struct ChoiceRule;

impl Compute for ChoiceRule {
    type Output = Option<Choice>;
    fn compute(&self, cx: &Cx) -> Result<Option<Choice>, Error> {
        let Some(decision) = cx.own::<Decision>()? else { return Ok(None) };
        let space = Space::global();
        if let Some(chosen) = decision.chosen {
            return Ok(Some(Choice(space.pure(Some(chosen)))));
        }
        // Variables are ordered by creation. Whatever this is inside gets its variable first, so
        // formulas read from the outside in.
        cx.own_value::<Need>()?;
        let mut options = Vec::new();
        for (o, alt) in cx.incoming::<Alternative>()? {
            if alt.ruled_out.is_none() && is_live(cx, o)? {
                // A gap has no likelihood of its own unless someone gives it one.
                let gap = cx.get::<Unknown>(o)?.is_some();
                options.push((o.to_string(), alt.weight.unwrap_or(if gap { 0.0 } else { 1.0 }), gap));
            }
        }
        if options.is_empty() {
            return Ok(Some(Choice(space.pure(None))));
        }
        let var = space.var(&cx.id().to_string(), &options);
        Ok(Some(Choice(var.value().map(|o| Some(o.parse().expect("options are handles"))))))
    }
}

/// The worlds in which `decision` lands on `option`.
fn selects(cx: &Cx, decision: Entity, option: Entity) -> Result<Bool, Error> {
    Ok(match cx.get::<Choice>(decision)? {
        Some(choice) => choice.0.holds(|c| *c == Some(option)),
        None => Space::global().never(),
    })
}

/// Parents, each with the worlds in which this node is in play through it. Inheriting from a
/// parent only makes sense there: shared work doesn't wait on a phase this world isn't doing.
pub(crate) fn in_play_parents(cx: &Cx) -> Result<Vec<(Entity, Bool)>, Error> {
    let mut out = Vec::new();
    for p in cx.targets::<PartOf>()? {
        out.push((p, cx.value::<Need>(p)?.0));
    }
    for (d, alt) in cx.outgoing::<Alternative>()? {
        // Need before choice, so that an outer decision gets its variable first.
        let need = cx.value::<Need>(d)?.0;
        let via = if alt.ruled_out.is_some() { Space::global().never() } else { &need & &selects(cx, d, cx.id())? };
        out.push((d, via));
    }
    Ok(out)
}

#[rule("plan/need", outgoing(PartOf, Alternative))]
pub struct NeedRule;

impl Compute for NeedRule {
    type Output = Need;
    fn compute(&self, cx: &Cx) -> Result<Need, Error> {
        let space = Space::global();
        if cx.own::<Dropped>()?.is_some() {
            return Ok(Need(space.never()));
        }
        let parents = in_play_parents(cx)?;
        if parents.is_empty() {
            return Ok(Need(space.always()));
        }
        Ok(Need(space.any(parents.into_iter().map(|(_, via)| via))))
    }
}

// ---------- completion: real data at the leaves, and everything above is derived ----------

/// Per world: `Some(true)` finished, `Some(false)` still owed, and `None` where this isn't in the
/// completion system at all, because nothing about it can be finished. A node opts in by holding
/// an obligation, by being a decision, or by having parts that did.
#[component]
pub struct Completion(pub Cond<Option<bool>>);

impl Completion {
    /// The worlds in which something here is still owed. This is what a dependent waits for.
    pub fn open(&self) -> Bool {
        self.0.holds(|c| *c == Some(false))
    }

    /// The worlds in which it's been finished.
    pub fn finished(&self) -> Bool {
        self.0.holds(|c| *c == Some(true))
    }
}

fn both(a: &Option<bool>, b: &Option<bool>) -> Option<bool> {
    match (a, b) {
        (None, x) | (x, None) => *x,
        (Some(a), Some(b)) => Some(*a && *b),
    }
}

/// The parts that are still in the picture.
pub(crate) fn live_parts(cx: &Cx) -> Result<Vec<Entity>, Error> {
    let mut out = Vec::new();
    for p in cx.sources::<PartOf>()? {
        if is_live(cx, p)? {
            out.push(p);
        }
    }
    Ok(out)
}

/// `Done` only means something at a leaf. Above that it's the combination of what's beneath, so
/// there's one source of truth, and a stated `Done` that disagrees is a notice. To finish
/// something early, drop what's left. That's a fact with an author.
#[rule("plan/completion", incoming(PartOf, Alternative))]
pub struct CompletionRule {
    /// Components that are work of their own, which only `Done` discharges.
    pub obligations: Vec<String>,
}

impl CompletionRule {
    fn leaf(&self, cx: &Cx) -> Result<Option<bool>, Error> {
        if cx.own::<Unknown>()?.is_some() {
            return Ok(Some(false));
        }
        if cx.own::<Done>()?.is_some() {
            return Ok(Some(true));
        }
        for c in &self.obligations {
            if cx.get_runtime(cx.id(), c)?.is_some() {
                return Ok(Some(false));
            }
        }
        Ok(None)
    }
}

impl Compute for CompletionRule {
    type Output = Completion;
    fn compute(&self, cx: &Cx) -> Result<Completion, Error> {
        let space = Space::global();
        if cx.own::<Dropped>()?.is_some() {
            return Ok(Completion(space.pure(None)));
        }
        let parts = live_parts(cx)?;
        let choice = cx.own::<Choice>()?;
        if parts.is_empty() && choice.is_none() {
            return Ok(Completion(space.pure(self.leaf(cx)?)));
        }
        let mut acc = space.pure(None);
        for p in parts {
            acc = acc.zip(&cx.value::<Completion>(p)?.0, both);
        }
        if let Some(choice) = choice {
            // A decision that lands on nothing can't complete.
            let mut selected = space.pure(Some(false));
            for (o, alt) in cx.incoming::<Alternative>()? {
                if alt.ruled_out.is_none() && is_live(cx, o)? {
                    let is_this = choice.0.holds(|c| *c == Some(o));
                    selected = Cond::when(&is_this, &cx.value::<Completion>(o)?.0, &selected);
                }
            }
            acc = acc.zip(&selected, both);
        }
        Ok(Completion(acc))
    }
}

// ---------- blocking ----------

/// What this waits for, by name, with the worlds in which it does: its own dependencies, and those
/// of whatever it's in play through. Nothing listed is `never`.
#[component]
pub struct Blockers(pub Vec<(Entity, Bool)>);

/// The worlds in which anything holds this up.
#[component]
pub struct Blocked(pub Bool);

/// For a live task with no live parts, the worlds in which it could be started now.
#[component]
pub struct Actionable(pub Bool);

#[rule("plan/blockers", outgoing(DependsOn, PartOf, Alternative))]
pub struct BlockersRule;

impl Compute for BlockersRule {
    type Output = Blockers;
    fn compute(&self, cx: &Cx) -> Result<Blockers, Error> {
        let space = Space::global();
        let mut out: BTreeMap<Entity, Bool> = BTreeMap::new();
        let mut add = |target: Entity, waits: Bool| {
            let so_far = out.get(&target).cloned().unwrap_or_else(|| space.never());
            out.insert(target, &so_far | &waits);
        };
        for (target, dep) in cx.outgoing::<DependsOn>()? {
            if !is_live(cx, target)? {
                continue;
            }
            let waits = match dep.on {
                Some(WaitFor::Decided) => space.truth(cx.get::<Decision>(target)?.is_some_and(|d| d.chosen.is_none())),
                _ => &cx.value::<Need>(target)?.0 & &cx.value::<Completion>(target)?.open(),
            };
            add(target, waits);
        }
        for (parent, via) in in_play_parents(cx)? {
            for (target, waits) in cx.value::<Blockers>(parent)?.0 {
                add(target, &via & &waits);
            }
        }
        Ok(Blockers(out.into_iter().filter(|(_, waits)| !waits.is_never()).collect()))
    }
}

#[rule("plan/blocked")]
pub struct BlockedRule;

impl Compute for BlockedRule {
    type Output = Blocked;
    fn compute(&self, cx: &Cx) -> Result<Blocked, Error> {
        Ok(Blocked(Space::global().any(cx.own_value::<Blockers>()?.0.into_iter().map(|(_, waits)| waits))))
    }
}

#[rule("plan/actionable", incoming(PartOf))]
pub struct ActionableRule;

impl Compute for ActionableRule {
    type Output = Option<Actionable>;
    fn compute(&self, cx: &Cx) -> Result<Option<Actionable>, Error> {
        if cx.own::<Task>()?.is_none() || cx.own::<Dropped>()?.is_some() || !live_parts(cx)?.is_empty() {
            return Ok(None);
        }
        let wanted = &cx.own_value::<Need>()?.0 & &cx.own_value::<Completion>()?.open();
        Ok(Some(Actionable(&wanted & &!&cx.own_value::<Blocked>()?.0)))
    }
}

// ---------- scope: where a node belongs ----------

/// The nearest node every path from the top must pass through. It's derived, so adding a second
/// parent moves a node with nothing to maintain, and it ignores decisions and `Dropped`, so nothing
/// jumps around when you decide. Absent at the top level.
#[component]
pub struct Scope(pub Entity);

/// How many scopes a node is inside.
#[derive(Copy)]
#[component]
pub struct ScopeDepth(pub usize);

/// A scope can be any distance away, so these are allowed to read any node's scope.
#[rule("plan/scope", scans(Scope))]
pub struct ScopeRule {
    /// The kinds of link that place a node inside another.
    pub links: Vec<String>,
}

impl Compute for ScopeRule {
    type Output = Option<Scope>;
    fn inputs(&self) -> Inputs {
        Inputs { outgoing: self.links.clone(), ..Self::declared_inputs() }
    }
    fn compute(&self, cx: &Cx) -> Result<Option<Scope>, Error> {
        let mut parents = BTreeSet::new();
        for kind in &self.links {
            parents.extend(cx.outgoing_runtime(kind)?.into_iter().map(|(p, _)| p));
        }
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

#[rule("plan/scope_depth", scans(Scope))]
pub struct ScopeDepthRule;

impl Compute for ScopeDepthRule {
    type Output = ScopeDepth;
    fn compute(&self, cx: &Cx) -> Result<ScopeDepth, Error> {
        Ok(ScopeDepth(match cx.own::<Scope>()? {
            Some(Scope(up)) => cx.value::<ScopeDepth>(up)?.0 + 1,
            None => 0,
        }))
    }
}

/// Which option a decision has taken, if it's taken one.
fn chosen(cx: &Cx) -> Result<Option<Entity>, Error> {
    Ok(cx.own::<Decision>()?.and_then(|d| d.chosen))
}

/// What a plugin can add to the planning system without changing it.
#[derive(Debug, Clone)]
pub struct Config {
    /// Components that are work of their own, which only `Done` discharges.
    pub obligations: Vec<String>,
    /// Kinds of link that place a node inside another.
    pub scoping: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            obligations: [Task::NAME, Goal::NAME, Question::NAME, Risk::NAME].map(String::from).to_vec(),
            scoping: [PartOf::NAME, Alternative::NAME].map(String::from).to_vec(),
        }
    }
}

/// The planning system: what its components look like, what's derived from them, and what must
/// hold.
pub fn install(reg: &mut Registry) {
    install_with(reg, Config::default());
}

pub fn install_with(reg: &mut Registry, config: Config) {
    formulas::install(reg);
    formulas::fills::<Estimate>(reg, Type::Number, estimate_of);
    reg.on_nodes::<Estimate>();
    reg.on_nodes::<Task>().on_nodes::<Goal>().on_nodes::<Question>().on_nodes::<Risk>().on_nodes::<Unknown>();
    reg.on_nodes::<Done>().on_nodes::<Dropped>().on_nodes::<Decision>();
    reg.on_links::<PartOf>().on_links::<Alternative>().on_links::<DependsOn>();
    reg.set_default(CanStartRule).set_default(CanFinishRule);
    reg.set_default(ChoiceRule).set_default(NeedRule).set_default(CompletionRule { obligations: config.obligations });
    reg.set_default(BlockersRule).set_default(BlockedRule).set_default(ActionableRule);
    reg.set_default(ScopeRule { links: config.scoping }).set_default(ScopeDepthRule);

    reg.invariant(rule("no_deadlock"), |cx| match cx.own::<CanFinish>() {
        Err(Error::Cycle(_)) => Ok(vec!["can never finish: it waits on itself, directly or through something else".to_string()]),
        Err(e) => Err(e),
        Ok(_) => Ok(vec![]),
    });

    reg.invariant(rule("one_decomposition_per_link").outgoing::<PartOf>().outgoing::<Alternative>(), |cx| {
        let parts = cx.targets::<PartOf>()?;
        Ok(cx.targets::<Alternative>()?.into_iter().filter(|t| parts.contains(t)).map(|t| format!("its link to {t} is both part_of and alternative")).collect())
    });

    reg.invariant(rule("unknown_has_no_parts").incoming::<PartOf>(), |cx| {
        if cx.own::<Unknown>()?.is_none() {
            return Ok(vec![]);
        }
        Ok(cx.sources::<PartOf>()?.into_iter().map(|c| format!("{c} is part of this gap: give it a real node instead")).collect())
    });

    reg.invariant(rule("unknown_cannot_be_done"), |cx| {
        let both = cx.own::<Unknown>()?.is_some() && cx.own::<Done>()?.is_some();
        Ok(if both { vec!["a gap can't be done: remove it to close it".to_string()] } else { vec![] })
    });

    reg.invariant(rule("unknown_cannot_be_chosen").scans::<Unknown>(), |cx| {
        let Some(c) = chosen(cx)? else { return Ok(vec![]) };
        let is_gap = cx.with::<Unknown>()?.iter().any(|(e, _)| *e == c);
        Ok(if is_gap { vec![format!("chose {c}, which is a gap: name the option instead")] } else { vec![] })
    });

    reg.invariant(rule("chosen_must_be_a_live_option").incoming::<Alternative>(), |cx| {
        let Some(c) = chosen(cx)? else { return Ok(vec![]) };
        let option = cx.incoming::<Alternative>()?.into_iter().find(|(o, _)| *o == c);
        let ok = match option {
            Some((o, alt)) => alt.ruled_out.is_none() && is_live(cx, o)?,
            None => false,
        };
        Ok(if ok { vec![] } else { vec![format!("chose {c}, which isn't a live option")] })
    });

    reg.invariant(rule("link_target_lacks_component").incoming::<Alternative>(), |cx| {
        if cx.own::<Decision>()?.is_some() {
            return Ok(vec![]);
        }
        Ok(cx.sources::<Alternative>()?.into_iter().map(|o| format!("{o} is an alternative of this, but it isn't a decision")).collect())
    });

    reg.notice(rule("decision_without_options").incoming::<Alternative>(), |cx| {
        let open = cx.own::<Decision>()?.is_some_and(|d| d.chosen.is_none()) && cx.own::<Dropped>()?.is_none();
        if !open {
            return Ok(vec![]);
        }
        for (o, alt) in cx.incoming::<Alternative>()? {
            if alt.ruled_out.is_none() && is_live(cx, o)? {
                return Ok(vec![]);
            }
        }
        Ok(vec!["open, with no live options".to_string()])
    });

    // A leaf's statement is what its completion comes from, so this can only be true above one.
    reg.notice(rule("stated_done_with_open_work"), |cx| {
        if cx.own::<Done>()?.is_none() {
            return Ok(vec![]);
        }
        let open = cx.own_value::<Completion>()?.open();
        Ok(if open.is_never() { vec![] } else { vec![format!("marked done, but it's derived from what's beneath it, which is still open: {}", open.show())] })
    });

    reg.notice(rule("undeveloped_goal").incoming::<PartOf>().incoming_named("serves"), |cx| {
        if cx.own::<Goal>()?.is_none() || cx.own::<Dropped>()?.is_some() || cx.own::<Done>()?.is_some() {
            return Ok(vec![]);
        }
        let means = live_parts(cx)?.len() + cx.incoming_runtime("serves")?.len();
        Ok(if means == 0 { vec!["nothing decomposes or serves this yet".to_string()] } else { vec![] })
    });

    reg.notice(rule("one_option_left").incoming::<Alternative>(), |cx| {
        if !cx.own::<Decision>()?.is_some_and(|d| d.chosen.is_none()) || cx.own::<Dropped>()?.is_some() {
            return Ok(vec![]);
        }
        let (mut live, mut gaps) = (0, 0);
        for (o, alt) in cx.incoming::<Alternative>()? {
            if alt.ruled_out.is_none() && is_live(cx, o)? {
                live += 1;
                gaps += usize::from(cx.get::<Unknown>(o)?.is_some());
            }
        }
        // A gap can't be chosen, and alongside one there may be more to come.
        Ok(if live == 1 && gaps == 0 { vec!["only one option is left: choose it, or add alternatives".to_string()] } else { vec![] })
    });

    reg.notice(rule("risk_without_target").outgoing_named("contradicts"), |cx| {
        if cx.own::<Risk>()?.is_none() || cx.own::<Dropped>()?.is_some() || cx.own::<Done>()?.is_some() {
            return Ok(vec![]);
        }
        Ok(if cx.outgoing_runtime("contradicts")?.is_empty() { vec!["it isn't linked to what it threatens".to_string()] } else { vec![] })
    });

    reg.notice(rule("oversized_task").incoming::<PartOf>(), |cx| {
        let big = cx.own::<Estimate>()?.is_some_and(|e| !e.0.0.holds(|d| d.is_some_and(|d| d > 5.0)).is_never());
        if !big || cx.own::<Done>()?.is_some() || !live_parts(cx)?.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec!["a big task with no breakdown".to_string()])
    });

    reg.notice(rule("dropped_part_of_live_work").outgoing::<PartOf>(), |cx| {
        if cx.own::<Dropped>()?.is_none() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for p in cx.targets::<PartOf>()? {
            if !cx.value::<Need>(p)?.0.is_never() {
                out.push(format!("dropped, but {p} is still in play and no longer requires it"));
            }
        }
        Ok(out)
    });
}

/// A new decision, with a gap for the options nobody has named yet.
pub fn new_decision(id: Entity, gap: Entity) -> Vec<Op> {
    vec![Op::set(id, Decision::default()), Op::set(gap, Unknown), Op::link(gap, id, Alternative::default())]
}

/// Say a node isn't fully specified yet.
pub fn mark_incomplete(id: Entity, gap: Entity) -> Vec<Op> {
    vec![Op::set(gap, Unknown), Op::link(gap, id, PartOf)]
}

/// Say a gap is closed: for a decision (`L` is `Alternative`), these are all the options there are.
pub fn close_gap<L: Component>(gap: Entity, under: Entity) -> Vec<Op> {
    vec![Op::unlink::<L>(gap, under), Op::Despawn { id: gap }]
}

/// Take an option.
pub fn choose(decision: Entity, option: Entity) -> Op {
    Op::set(decision, Decision { chosen: Some(option) })
}

/// Take a decision back. Nothing beneath it was ever touched, so it all comes back on its own.
pub fn reopen(decision: Entity) -> Op {
    Op::set(decision, Decision::default())
}

pub fn describe(c: &mut Catalog) {
    c.fact::<Task>("Work of its own, which only Done discharges.", json!({}));
    let estimate = c.worldly::<Estimate>("How long it takes, in days: typed in, or a formula.", |e| Shown::cond(&e.0.0));
    estimate.derived = false;
    estimate.initial = Some(json!(1.0));
    c.fact::<Goal>("A result you want. Break it into parts, or let other things serve it.", Value::Null);
    c.fact::<Question>("Something not yet known. Done means it's been answered.", Value::Null);
    c.fact::<Risk>("Something that could go wrong. Link it to what it threatens.", Value::Null);
    c.fact::<Unknown>("A gap: something here isn't specified yet. Removing it says the gap is closed.", Value::Null);
    c.fact::<Done>("Stated complete. Only counts at a leaf: above that, it's derived.", Value::Null);
    c.fact::<Dropped>("Deliberately out of the picture.", Value::Null);
    c.fact::<Decision>("An open question with options, and which one was taken, if any.", json!({}));
    c.arranging::<PartOf>("Part of that: all of a node's parts are needed.", Value::Null);
    c.arranging::<Alternative>("One way of doing that decision: only the option taken is needed.", json!({}));
    c.link::<DependsOn>("Waits for that.", json!({}));
    c.worldly::<Need>("The worlds where this is part of what's being attempted.", |n| Shown::Bool(n.0.clone())).summary = true;
    c.worldly::<Completion>("Per world: finished, still owed, or nothing to finish.", |v| Shown::cond(&v.0));
    c.worldly::<Choice>("What this decision could land on.", |v| Shown::cond(&v.0));
    c.worldly::<Blockers>("What this waits for, and in which worlds.", |b| Shown::Named(b.0.clone()));
    c.worldly::<Blocked>("The worlds where something holds this up.", |b| Shown::Bool(b.0.clone()));
    c.worldly::<Actionable>("The worlds where this could be started now.", |a| Shown::Bool(a.0.clone())).summary = true;
    c.derived::<Scope>("The nearest node every path from the top passes through.");
    c.internal::<ScopeDepth>();
    c.internal::<CanStart>();
    c.internal::<CanFinish>();
    c.advice(blocking_decisions);
}

/// Open decisions where safe work has run out.
fn blocking_decisions(g: &Graph) -> Vec<Advice> {
    let mut out = Vec::new();
    for e in g.entities() {
        if let Ok(Pressure::Blocking(work)) = pressure(g, e) {
            let plural = if work.len() == 1 { "task" } else { "tasks" };
            out.push(Advice { rule: "blocking_decision".to_string(), on: e, detail: format!("only work specific to an option is left: {} {plural} depend on deciding this", work.len()) });
        }
    }
    out
}

// ---------- questions about the graph as a whole ----------

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
    if !g.get::<Decision>(decision)?.is_some_and(|d| d.chosen.is_none()) {
        return Ok(Pressure::Decided);
    }
    let mut under = BTreeSet::new();
    let mut stack = vec![decision];
    while let Some(cur) = stack.pop() {
        for child in g.sources_of(cur, PartOf::NAME).into_iter().chain(g.sources_of(cur, Alternative::NAME)) {
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
    let open = |e: Entity| Ok::<_, Error>(g.get::<Completion>(e)?.map_or_else(|| "never".to_string(), |c| show(&c.open())));
    let need = g.get::<Need>(e)?.map_or_else(|| "never".to_string(), |n| show(&n.0));
    let blocked = g.get::<Blocked>(e)?.map_or_else(|| "never".to_string(), |b| show(&b.0));
    let mut out = format!("{}\n  need: {need}\n  open: {}\n  blocked: {blocked}\n", name(e), open(e)?);
    if g.get::<Done>(e)?.is_some() {
        out += "  stated done\n";
    }
    for c in g.sources_of(e, PartOf::NAME) {
        out += &format!("  requires {}: open {}\n", name(c), open(c)?);
    }
    for o in g.sources_of(e, Alternative::NAME) {
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
            for child in g.sources_of(cur, PartOf::NAME).into_iter().chain(g.sources_of(cur, Alternative::NAME)) {
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

/// Every decision, open ones first. `world` narrows everything, as a what-if does.
pub fn matrices(g: &Graph, world: &BTreeMap<String, String>) -> Result<Vec<Matrix>, Error> {
    let names = labels(g);
    let label = |s: &str| names.get(s).cloned().unwrap_or_else(|| s.to_string());
    let space = Space::global();
    let picks = space.picks(world);
    let mut out = Vec::new();
    for d in g.entities() {
        let Some(decision) = g.get::<Decision>(d)? else { continue };
        let mut options = Vec::new();
        let mut live = Vec::new();
        for o in g.sources_of(d, Alternative::NAME) {
            let alt: Alternative = g.link_value(o, d, Alternative::NAME).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
            let dropped = g.get::<Dropped>(o)?.is_some();
            let gap = g.get::<Unknown>(o)?.is_some();
            let is_live = alt.ruled_out.is_none() && !dropped;
            if is_live {
                live.push((o, alt.weight.unwrap_or(if gap { 0.0 } else { 1.0 })));
            }
            options.push(MatrixOption { id: o, name: label_of(g, o), weight: 0.0, gap, ruled_out: alt.ruled_out.is_some(), dropped, chosen: decision.chosen == Some(o) });
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
            if direct.contains(&e) || g.get::<Dropped>(e)?.is_some() {
                continue;
            }
            let need = g.get::<Need>(e)?.map_or(space.never(), |n| n.0);
            let mut per_option = BTreeMap::new();
            for o in &options {
                let mut w = world.clone();
                w.insert(d.to_string(), o.id.to_string());
                per_option.insert(o.id, need.given(&space.picks(&w)).prob());
            }
            let completion = g.get::<Completion>(e)?;
            rows.push(MatrixRow {
                id: e,
                name: label_of(g, e),
                depth: row_depth,
                per_option,
                need: read(&need.given(&picks), &label),
                finished: completion.is_some_and(|c| c.finished().given(&picks).is_always()),
                is_work: g.get::<Task>(e)?.is_some() && g.sources_of(e, PartOf::NAME).is_empty(),
                actionable: g.get::<Actionable>(e)?.map(|a| read(&a.0.given(&picks), &label)),
                blockers: blockers_of(g, e, &picks, &label)?,
            });
        }
        rows.sort_by(|a, b| b.need.prob.total_cmp(&a.need.prob).then(a.depth.cmp(&b.depth)).then(a.name.cmp(&b.name)));
        out.push(Matrix { id: d, name: label_of(g, d), chosen: decision.chosen, options, rows, pressure: pressure(g, d)? });
    }
    out.sort_by_key(|m| m.chosen.is_some());
    Ok(out)
}

fn blockers_of(g: &Graph, e: Entity, picks: &recollect_graph::worlds::Picks, label: &dyn Fn(&str) -> String) -> Result<Vec<Blocker>, Error> {
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

fn estimate_text(c: &Cond<Option<f64>>) -> String {
    let known: Vec<f64> = c.cases().iter().filter_map(|(_, d)| **d).collect();
    let (low, high) = (known.iter().copied().fold(f64::INFINITY, f64::min), known.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    if known.is_empty() {
        "?".to_string()
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
    let picks = Space::global().picks(world);
    let mut out = Vec::new();
    for e in g.entities() {
        let Some(actionable) = g.get::<Actionable>(e)? else { continue };
        let actionable = actionable.0.given(&picks);
        let need = g.get::<Need>(e)?.map_or(Space::global().never(), |n| n.0).given(&picks);
        let finished = g.get::<Completion>(e)?.is_some_and(|c| c.finished().given(&picks).holds_given(&need));
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
