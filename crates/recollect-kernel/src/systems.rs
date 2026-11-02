//! The kernel's own cells. Each is a pure function of one node, written with the same API a
//! plugin uses. A derived value maps a node to the exact set of worlds in which it holds.

use std::collections::{BTreeMap, BTreeSet};

pub use crate::compute::*;
use crate::formula::{F, FALSE, TRUE};
use crate::schema::{Alternative, DependsOn, Done, PartOf, Task, WaitFor};
use crate::scope::{Scope, ScopeDepth};
use crate::store::*;

/// `need(n)`: the worlds in which `n` is part of what's being attempted.
///   dropped                                    → never
///   no decomposition parent                    → always
///   otherwise, OR over parents: part_of        → need(parent)
///                               alternative    → need(decision) ∧ decision selects n
pub struct Need;
impl Cell for Need {
    const NAME: &'static str = "need";
    type Value = F;
    fn eval(cx: &Cx, id: &str) -> F {
        if !cx.live(id) {
            return FALSE;
        }
        let parts = cx.links_from::<PartOf>(id);
        let options = cx.links_from::<Alternative>(id);
        if parts.is_empty() && options.is_empty() {
            return TRUE;
        }
        let mut acc = FALSE;
        for (parent, _) in parts {
            acc = cx.or(acc, cx.cell::<Need>(parent));
        }
        for (decision, alt) in options {
            if alt.ruled_out.is_none() {
                let via = cx.and(cx.cell::<Need>(decision), cx.selects(decision, id));
                acc = cx.or(acc, via);
            }
        }
        acc
    }
}

/// `all_required_done(n)`: the worlds in which everything `n` requires is complete.
///   every live part done
///   ∧ if a decision: OR over live options, selects(o) ∧ done(o)
///   ∧ if it has neither parts nor options: it has no work of its own
pub struct AllRequiredDone;
impl Cell for AllRequiredDone {
    const NAME: &'static str = "all_required_done";
    type Value = F;
    fn eval(cx: &Cx, id: &str) -> F {
        if !cx.live(id) {
            return FALSE;
        }
        let parts = cx.parts(id);
        let is_decision = cx.has::<crate::schema::Decision>(id);
        if parts.is_empty() && !is_decision {
            return if cx.has_obligation(id) { FALSE } else { TRUE };
        }
        let mut acc = TRUE;
        for p in parts {
            acc = cx.and(acc, cx.cell::<Completed>(p));
        }
        if is_decision {
            let mut any = FALSE;
            for o in cx.options(id) {
                let both = cx.and(cx.selects(id, o), cx.cell::<Completed>(o));
                any = cx.or(any, both);
            }
            acc = cx.and(acc, any);
        }
        acc
    }
}

/// `done(n)`: stated done ∨ all_required_done(n). Dropped nodes are neither.
pub struct Completed;
impl Cell for Completed {
    const NAME: &'static str = "done";
    type Value = F;
    fn eval(cx: &Cx, id: &str) -> F {
        if !cx.live(id) {
            return FALSE;
        }
        if cx.has::<Done>(id) { TRUE } else { cx.cell::<AllRequiredDone>(id) }
    }
}

/// What `n` waits for, by name: its own dependencies and those of everything it's inside, each
/// with the worlds in which it holds things up.
///   depends_on (default)        → need(target) ∧ ¬done(target)
///   depends_on {on: "decided"}  → target is an open decision
pub struct Blockers;
impl Cell for Blockers {
    const NAME: &'static str = "blockers";
    type Value = Vec<(Id, F)>;
    fn eval(cx: &Cx, id: &str) -> Vec<(Id, F)> {
        let mut out: BTreeMap<Id, F> = BTreeMap::new();
        for (target, dep) in cx.links_from::<DependsOn>(id) {
            if !cx.live(target) {
                continue;
            }
            let waits = match dep.on {
                Some(WaitFor::Decided) => {
                    let open = cx.get::<crate::schema::Decision>(target).is_some_and(|d| d.chosen.is_none());
                    if open { TRUE } else { FALSE }
                }
                _ => cx.and(cx.cell::<Need>(target), cx.not(cx.cell::<Completed>(target))),
            };
            out.insert(target.to_string(), waits);
        }
        let inherited = cx.links_from::<PartOf>(id).into_iter().map(|(p, _)| p).chain(cx.links_from::<Alternative>(id).into_iter().map(|(d, _)| d));
        for parent in inherited {
            for (target, waits) in cx.cell::<Blockers>(parent) {
                out.insert(target, waits);
            }
        }
        out.into_iter().filter(|(_, f)| *f != FALSE).collect()
    }
}

/// `blocked(n)`: the worlds in which anything holds `n` up.
pub struct Blocked;
impl Cell for Blocked {
    const NAME: &'static str = "blocked";
    type Value = F;
    fn eval(cx: &Cx, id: &str) -> F {
        cx.cell::<Blockers>(id).into_iter().fold(FALSE, |acc, (_, f)| cx.or(acc, f))
    }
}

/// `actionable(t)`: for a live task with no live parts, the worlds where it's needed, not done
/// and not blocked. Equal to `always` means it's safe to start now.
pub struct Actionable;
impl Cell for Actionable {
    const NAME: &'static str = "actionable";
    type Value = F;
    fn eval(cx: &Cx, id: &str) -> F {
        let free = cx.and(cx.not(cx.cell::<Completed>(id)), cx.not(cx.cell::<Blocked>(id)));
        cx.and(cx.cell::<Need>(id), free)
    }
    fn domain(cx: &Cx) -> Vec<Id> {
        cx.with::<Task>().into_iter().filter(|(id, _)| cx.live(id) && cx.parts(id).is_empty()).map(|(id, _)| id.to_string()).collect()
    }
}

/// Situations worth a look, each a precise statement about the graph.
fn notices(cx: &Cx) -> Vec<Notice> {
    let mut out = Vec::new();
    for id in cx.entities() {
        if !cx.live(id) {
            continue;
        }
        let is_decision = cx.has::<crate::schema::Decision>(id);
        if cx.has::<Done>(id) && (is_decision || !cx.parts(id).is_empty()) {
            let required = cx.cell::<AllRequiredDone>(id);
            if required != TRUE {
                out.push(Notice { rule: "stated_done_with_open_work", id: id.to_string(), detail: format!("marked done, but what it requires is done: {}", cx.show(required)) });
            }
        }
        if is_decision && cx.get::<crate::schema::Decision>(id).is_some_and(|d| d.chosen.is_none()) && cx.options(id).is_empty() {
            out.push(Notice { rule: "decision_without_options", id: id.to_string(), detail: "open, with no live options".into() });
        }
    }
    // Deliberately dropped work on a path that's still in play.
    for (from, _) in cx.with::<crate::schema::Dropped>() {
        for (to, _) in cx.links_from::<PartOf>(from) {
            if cx.cell::<Need>(to) != FALSE {
                out.push(Notice { rule: "dropped_part_of_live_work", id: from.to_string(), detail: format!("dropped, but {to} is still in play and no longer requires it") });
            }
        }
    }
    out
}

/// The kernel's systems, as a library a plugin can add to.
pub fn kernel() -> Registry {
    let mut r = Registry::default();
    r.add::<Need>().add::<AllRequiredDone>().add::<Completed>().add::<Blockers>().add::<Blocked>().add::<Actionable>().add::<Scope>().add::<ScopeDepth>().add_check(notices);
    r
}

pub fn compute(s: &Store) -> Derived {
    run(s, &kernel())
}

/// Open decisions and whether work is waiting on them. Derived from `actionable` alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Pressure {
    /// Some work under it is safe whichever named option is taken: no need to decide yet.
    Free(Vec<Id>),
    /// Only option-specific work remains: this decision is what's in the way.
    Blocking(Vec<Id>),
    /// Open, with no actionable work under it either way.
    Idle,
    /// Not open: pressure doesn't apply.
    Decided,
}

pub fn pressure(s: &Store, d: &Derived, decision: &str) -> Pressure {
    if s.chosen(decision).is_some() {
        return Pressure::Decided;
    }
    let mut under = BTreeSet::new();
    let mut stack = vec![decision.to_string()];
    while let Some(cur) = stack.pop() {
        for ((f, t), comps) in &s.links {
            if t == &cur && (comps.contains_key(PART_OF) || comps.contains_key(ALTERNATIVE)) && under.insert(f.clone()) {
                stack.push(f.clone());
            }
        }
    }
    let mut safe = Vec::new();
    let mut specific = Vec::new();
    for id in &under {
        let Some(&f) = d.cells.get("actionable").and_then(|c| c.get(id)) else { continue };
        if f != FALSE && d.mdd.holds_among_known(f) {
            safe.push(id.clone());
        } else if f != FALSE && d.mdd.support(f).contains(decision) {
            specific.push(id.clone());
        }
    }
    if !safe.is_empty() {
        Pressure::Free(safe)
    } else if !specific.is_empty() {
        Pressure::Blocking(specific)
    } else {
        Pressure::Idle
    }
}

/// What `id` is waiting for, by name. Empty means nothing is in the way.
pub fn blockers(d: &Derived, id: &str) -> Vec<(Id, F)> {
    d.of::<Blockers>(id).cloned().unwrap_or_default()
}

/// What-if: the derived values of a hypothetical store. Nothing is written or logged; a
/// preview and the real thing run exactly the same code.
pub fn what_if(s: &Store, ops: &[Op]) -> Result<Derived, String> {
    let mut hypothetical = s.clone();
    for op in ops {
        hypothetical.apply(op)?;
    }
    if let Some(v) = hypothetical.check().first() {
        return Err(format!("{}: {}", v.rule, v.detail));
    }
    Ok(compute(&hypothetical))
}

/// Why a cell has its value: one level of inputs, each with its own exact formula.
pub fn explain(s: &Store, d: &Derived, id: &str) -> String {
    let mut out = format!("{id}\n  need: {}\n  done: {}\n  blocked: {}\n", d.show("need", id), d.show("done", id), d.show("blocked", id));
    if s.has(id, DONE) {
        out += "  stated done\n";
    }
    for (c, _) in s.children(id, PART_OF) {
        out += &format!("  requires {c}: done {}\n", d.show("done", c));
    }
    for (o, _) in s.options(id) {
        out += &format!("  option {o}: done {}\n", d.show("done", o));
    }
    for (b, f) in blockers(d, id) {
        out += &format!("  waits for {b}: {}\n", d.mdd.show(f));
    }
    out
}
