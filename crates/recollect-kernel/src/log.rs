//! The append-only action log. A store is the replay of its log: there is no other way for
//! state to change, so every fact can answer who set it, when and why.

use serde_json::Value;

use crate::schema::Schema;
use crate::store::{ALTERNATIVE, Components, DECISION, Op, PART_OF, Store, Target, UNKNOWN, Violation};

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: usize,
    pub author: String,
    /// Why, in the actor's words. Reasons live here, not in node text.
    pub why: String,
    pub ops: Vec<Op>,
    /// Exactly undoes `ops`, applied in order.
    pub inverse: Vec<Op>,
}

#[derive(Debug, Clone, Default)]
pub struct Log {
    pub events: Vec<Event>,
    pub store: Store,
    pub schema: Schema,
}

impl Log {
    /// Apply `ops` atomically, or refuse them and change nothing.
    pub fn commit(&mut self, author: &str, why: &str, ops: Vec<Op>) -> Result<usize, Vec<Violation>> {
        let mut next = self.store.clone();
        let mut inverse = Vec::with_capacity(ops.len());
        for op in &ops {
            match next.apply(op) {
                Ok(inv) => inverse.push(inv),
                Err(detail) => return Err(vec![Violation { rule: "op_failed", detail }]),
            }
        }
        inverse.reverse();
        let before = self.schema.check(&self.store);
        let new: Vec<Violation> = self.schema.check(&next).into_iter().filter(|v| !before.contains(v)).collect();
        if !new.is_empty() {
            return Err(new);
        }
        let seq = self.events.len();
        self.store = next;
        self.events.push(Event { seq, author: author.into(), why: why.into(), ops, inverse });
        Ok(seq)
    }

    /// Bring in a store edited outside the log (a hand edit, `git pull`). The files are what
    /// they are, so this always records, as ordinary audited ops computed by diffing, and
    /// reports any rules the edit now breaks rather than refusing it.
    pub fn reconcile(&mut self, disk: &Store, author: &str) -> Vec<Violation> {
        let mut ops = Vec::new();
        for (id, comps) in &disk.entities {
            let target = Target::Entity(id.clone());
            match self.store.entities.get(id) {
                None => ops.push(Op::Restore(target, comps.clone())),
                old => diff(target, old, comps, &mut ops),
            }
        }
        for ((f, t), comps) in &disk.links {
            let target = Target::Link(f.clone(), t.clone());
            match self.store.links.get(&(f.clone(), t.clone())) {
                // A link with no components is still a link, so it's restored rather than set.
                None => ops.push(Op::Restore(target, comps.clone())),
                old => diff(target, old, comps, &mut ops),
            }
        }
        for (f, t) in self.store.links.keys() {
            if !disk.links.contains_key(&(f.clone(), t.clone())) {
                ops.push(Op::Remove(Target::Link(f.clone(), t.clone())));
            }
        }
        for id in self.store.entities.keys() {
            if !disk.entities.contains_key(id) {
                ops.push(Op::Remove(Target::Entity(id.clone())));
            }
        }
        if ops.is_empty() {
            return vec![];
        }
        let mut next = self.store.clone();
        let mut inverse = Vec::new();
        // Links are removed before entities above, so these all apply.
        for op in &ops {
            inverse.push(next.apply(op).expect("diff applies"));
        }
        inverse.reverse();
        let seq = self.events.len();
        self.store = next;
        self.events.push(Event { seq, author: author.into(), why: "external edit".into(), ops, inverse });
        self.schema.check(&self.store)
    }

    /// Undo by appending the inverse. History is never rewritten, so the undo is itself audited.
    pub fn undo(&mut self, seq: usize, author: &str, why: &str) -> Result<usize, Vec<Violation>> {
        let inverse = self.events[seq].inverse.clone();
        self.commit(author, why, inverse)
    }

    /// The store as it was just after event `seq`.
    pub fn state_at(&self, seq: usize) -> Store {
        let mut s = Store::default();
        for ev in &self.events[..=seq] {
            for op in &ev.ops {
                s.apply(op).expect("logged ops replay");
            }
        }
        s
    }

    /// Every event that touched a target, oldest first.
    pub fn history(&self, target: &Target) -> Vec<&Event> {
        self.events.iter().filter(|ev| ev.ops.iter().any(|op| op_target(op) == target)).collect()
    }

    /// Who last set a component and why, if anyone has.
    pub fn blame(&self, target: &Target, component: &str) -> Option<&Event> {
        self.events.iter().rev().find(|ev| ev.ops.iter().any(|op| match op {
            Op::Set(t, c, _) => t == target && c == component,
            Op::Restore(t, comps) => t == target && comps.contains_key(component),
            Op::Remove(_) => false,
        }))
    }
}

fn diff(target: Target, old: Option<&Components>, new: &Components, ops: &mut Vec<Op>) {
    for (c, v) in new {
        if old.and_then(|o| o.get(c)) != Some(v) {
            ops.push(Op::Set(target.clone(), c.clone(), Some(v.clone())));
        }
    }
    for c in old.into_iter().flat_map(|o| o.keys()) {
        if !new.contains_key(c) {
            ops.push(Op::Set(target.clone(), c.clone(), None));
        }
    }
}

fn op_target(op: &Op) -> &Target {
    match op {
        Op::Set(t, _, _) | Op::Remove(t) | Op::Restore(t, _) => t,
    }
}

// Small constructors, so change-sets read clearly.
pub fn set(t: Target, c: &str, v: Value) -> Op {
    Op::Set(t, c.into(), Some(v))
}

pub fn unset(t: Target, c: &str) -> Op {
    Op::Set(t, c.into(), None)
}

pub fn choose(decision: &str, option: &str) -> Op {
    set(Target::Entity(decision.into()), crate::store::DECISION, serde_json::json!({ "chosen": option }))
}

pub fn reopen(decision: &str) -> Op {
    set(Target::Entity(decision.into()), DECISION, serde_json::json!({}))
}

/// The gap under a node, by convention.
pub fn unknown_id(parent: &str) -> String {
    format!("{parent}.unknown")
}

fn gap(parent: &str, link: &str) -> Vec<Op> {
    let id = unknown_id(parent);
    vec![
        set(Target::Entity(id.clone()), UNKNOWN, serde_json::json!({})),
        set(Target::Link(id, parent.into()), link, serde_json::json!({})),
    ]
}

/// A new decision, with a gap for the options nobody has named yet.
pub fn new_decision(id: &str) -> Vec<Op> {
    let mut ops = vec![set(Target::Entity(id.into()), DECISION, serde_json::json!({}))];
    ops.extend(gap(id, ALTERNATIVE));
    ops
}

/// Say a node isn't fully specified yet.
pub fn mark_incomplete(id: &str) -> Vec<Op> {
    gap(id, PART_OF)
}

/// Say a node's gap is closed: for a decision, these are all the options there are.
pub fn close_gap(id: &str) -> Vec<Op> {
    let gap = unknown_id(id);
    vec![Op::Remove(Target::Link(gap.clone(), id.into())), Op::Remove(Target::Entity(gap))]
}
