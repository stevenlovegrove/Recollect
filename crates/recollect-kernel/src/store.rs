//! Stored facts: entities and links, each just a bag of components. Nothing derived lives
//! here, so there is nothing that can go stale.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

pub type Id = String;
pub type Components = BTreeMap<String, Value>;

// Names of the components the kernel's own systems give meaning to, whose types and rules are
// in `schema`. Anything else is kept and ignored.
pub const DONE: &str = "done"; // entity: stated complete.
pub const TASK: &str = "task"; // entity: work that can be picked up.
pub const DROPPED: &str = "dropped"; // entity: deliberately out of the picture.
pub const DECISION: &str = "decision"; // entity: {chosen?: option id}.
pub const PART_OF: &str = "part_of"; // link child → parent: AND.
pub const ALTERNATIVE: &str = "alternative"; // link option → decision: OR. {ruled_out?, weight?}.
pub const DEPENDS_ON: &str = "depends_on"; // link a → b: a waits for b. {on?: "done" | "decided"}.
pub const UNKNOWN: &str = "unknown"; // entity: a gap. Can't be done or chosen; removing it closes it.

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Entity(Id),
    Link(Id, Id),
}

pub fn e(id: &str) -> Target {
    Target::Entity(id.into())
}

pub fn l(from: &str, to: &str) -> Target {
    Target::Link(from.into(), to.into())
}

/// The only ways anything changes. Every op has an exact inverse, so undo is just more ops.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Create the target if needed and set one component. `None` removes the component; a
    /// link with no components left still exists as a plain association.
    Set(Target, String, Option<Value>),
    /// Remove a target and everything on it. Removing an entity needs its links gone first.
    Remove(Target),
    /// Put a removed target back exactly (the inverse of `Remove`).
    Restore(Target, Components),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Store {
    pub entities: BTreeMap<Id, Components>,
    pub links: BTreeMap<(Id, Id), Components>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub rule: &'static str,
    pub detail: String,
}

impl Store {
    pub fn has(&self, id: &str, c: &str) -> bool {
        self.entities.get(id).is_some_and(|e| e.contains_key(c))
    }

    pub fn live(&self, id: &str) -> bool {
        self.entities.contains_key(id) && !self.has(id, DROPPED)
    }

    pub fn chosen(&self, id: &str) -> Option<&str> {
        self.entities.get(id)?.get(DECISION)?.get("chosen")?.as_str()
    }

    /// Live children of `parent` over component `c`.
    pub fn children(&self, parent: &str, c: &str) -> Vec<(&str, &Value)> {
        self.links
            .iter()
            .filter(|((f, t), comps)| t == parent && comps.contains_key(c) && self.live(f))
            .map(|((f, _), comps)| (f.as_str(), &comps[c]))
            .collect()
    }

    /// Options still in play for a decision: alternatives that are live and not ruled out.
    pub fn options(&self, decision: &str) -> Vec<(&str, &Value)> {
        self.children(decision, ALTERNATIVE).into_iter().filter(|(_, v)| v.get("ruled_out").is_none()).collect()
    }

    /// Open decisions: the variables of the world space.
    pub fn open_decisions(&self) -> Vec<&str> {
        self.entities.keys().map(String::as_str).filter(|id| self.live(id) && self.has(id, DECISION) && self.chosen(id).is_none()).collect()
    }

    /// Parents of `id` through decomposition (part_of or alternative).
    pub fn parents(&self, id: &str) -> Vec<&str> {
        self.links
            .iter()
            .filter(|((f, _), comps)| f == id && (comps.contains_key(PART_OF) || comps.contains_key(ALTERNATIVE)))
            .map(|((_, t), _)| t.as_str())
            .collect()
    }

    /// Apply one op, returning its exact inverse.
    pub fn apply(&mut self, op: &Op) -> Result<Op, String> {
        match op {
            Op::Set(t, c, v) => {
                let existed = match t {
                    Target::Entity(id) => self.entities.contains_key(id),
                    Target::Link(f, to) => self.links.contains_key(&(f.clone(), to.clone())),
                };
                if !existed && v.is_none() {
                    return Err(format!("{t:?} doesn't exist"));
                }
                let comps = match t {
                    Target::Entity(id) => self.entities.entry(id.clone()).or_default(),
                    Target::Link(f, to) => {
                        if !self.entities.contains_key(f) || !self.entities.contains_key(to) {
                            return Err(format!("link {f} → {to} needs both ends to exist"));
                        }
                        self.links.entry((f.clone(), to.clone())).or_default()
                    }
                };
                let old = match v {
                    Some(v) => comps.insert(c.clone(), v.clone()),
                    None => comps.remove(c),
                };
                // Creating something is undone by removing it, not by leaving it empty.
                Ok(if existed { Op::Set(t.clone(), c.clone(), old) } else { Op::Remove(t.clone()) })
            }
            Op::Remove(t) => {
                let old = match t {
                    Target::Entity(id) => {
                        if self.links.keys().any(|(f, to)| f == id || to == id) {
                            return Err(format!("{id} still has links"));
                        }
                        self.entities.remove(id)
                    }
                    Target::Link(f, to) => self.links.remove(&(f.clone(), to.clone())),
                };
                let old = old.ok_or_else(|| format!("{t:?} doesn't exist"))?;
                Ok(Op::Restore(t.clone(), old))
            }
            Op::Restore(t, comps) => {
                match t {
                    Target::Entity(id) => {
                        self.entities.insert(id.clone(), comps.clone());
                    }
                    Target::Link(f, to) => {
                        self.links.insert((f.clone(), to.clone()), comps.clone());
                    }
                }
                Ok(Op::Remove(t.clone()))
            }
        }
    }

    /// Every rule that must hold, under the kernel's own schema.
    pub fn check(&self) -> Vec<Violation> {
        crate::schema::Schema::default().check(self)
    }

    /// Rules about structure, whatever the schema. These are refusals, not warnings.
    pub fn check_structure(&self) -> Vec<Violation> {
        let mut out = Vec::new();
        let mut v = |rule, detail: String| out.push(Violation { rule, detail });

        for ((f, t), comps) in &self.links {
            if f == t {
                v("no_self_links", format!("{f} links to itself"));
            }
            if comps.contains_key(ALTERNATIVE) && comps.contains_key(PART_OF) {
                v("one_decomposition_per_link", format!("{f} → {t} is both part_of and alternative"));
            }
            if comps.contains_key(PART_OF) && self.has(t, UNKNOWN) {
                v("unknown_has_no_parts", format!("{f} is part of {t}, which is a gap: give it a real node instead"));
            }
        }
        for id in self.entities.keys() {
            if self.has(id, UNKNOWN) && self.has(id, DONE) {
                v("unknown_cannot_be_done", format!("{id} is a gap: remove it to close it"));
            }
            if let Some(c) = self.chosen(id) {
                if self.has(c, UNKNOWN) {
                    v("unknown_cannot_be_chosen", format!("{id} chose {c}, which is a gap: name the option instead"));
                }
                let ok = self.links.get(&(c.to_string(), id.clone())).and_then(|l| l.get(ALTERNATIVE)).is_some_and(|a| a.get("ruled_out").is_none());
                if !ok || !self.live(c) {
                    v("chosen_must_be_a_live_option", format!("{id} chose {c}"));
                }
            }
        }
        if let Some(path) = find_cycle(&self.entities.keys().cloned().collect(), &self.wait_edges()) {
            v("no_deadlock", format!("waits form a cycle: {}", path.join(" → ")));
        }
        out
    }

    /// Who can't finish before whom. A parent waits for its children; anything waits for what
    /// it depends on; and a child can't start until its ancestors' dependencies are met.
    /// A cycle here means work that can never complete.
    fn wait_edges(&self) -> BTreeMap<Id, BTreeSet<Id>> {
        let mut edges: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
        for ((f, t), comps) in &self.links {
            if comps.contains_key(PART_OF) || comps.contains_key(ALTERNATIVE) {
                edges.entry(t.clone()).or_default().insert(f.clone());
            }
            if comps.contains_key(DEPENDS_ON) {
                edges.entry(f.clone()).or_default().insert(t.clone());
                let mut stack = vec![f.as_str()];
                let mut seen = BTreeSet::new();
                while let Some(cur) = stack.pop() {
                    for (child, c) in self.links.iter().filter(|((_, to), _)| to == cur).filter_map(|((ch, _), c)| Some((ch, c))) {
                        if (c.contains_key(PART_OF) || c.contains_key(ALTERNATIVE)) && seen.insert(child.clone()) {
                            edges.entry(child.clone()).or_default().insert(t.clone());
                            stack.push(child);
                        }
                    }
                }
            }
        }
        edges
    }
}

fn find_cycle(nodes: &BTreeSet<Id>, edges: &BTreeMap<Id, BTreeSet<Id>>) -> Option<Vec<Id>> {
    fn go<'a>(n: &'a str, edges: &'a BTreeMap<Id, BTreeSet<Id>>, state: &mut BTreeMap<&'a str, u8>, path: &mut Vec<&'a str>) -> Option<Vec<Id>> {
        state.insert(n, 1);
        path.push(n);
        for m in edges.get(n).into_iter().flatten() {
            match state.get(m.as_str()) {
                Some(1) => {
                    let at = path.iter().position(|p| *p == m).unwrap();
                    let mut cycle: Vec<Id> = path[at..].iter().map(|s| s.to_string()).collect();
                    cycle.push(m.clone());
                    return Some(cycle);
                }
                Some(_) => {}
                None => {
                    if let Some(c) = stacker::maybe_grow(64 * 1024, 1024 * 1024, || go(m, edges, state, path)) {
                        return Some(c);
                    }
                }
            }
        }
        path.pop();
        state.insert(n, 2);
        None
    }
    let mut state = BTreeMap::new();
    for n in nodes {
        if !state.contains_key(n.as_str()) {
            let mut path = Vec::new();
            if let Some(c) = go(n, edges, &mut state, &mut path) {
                return Some(c);
            }
        }
    }
    None
}
