//! Translating the old kernel's names, ops and stores into this core's, for the tests that compare
//! the two.
#![allow(dead_code)]

use std::collections::BTreeMap;

use recollect_graph::{Entity, Op, Runtime, SavedGraph};
use recollect_kernel::store::{Op as KOp, Store, Target as KTarget};
use serde_json::{Value, json};

const UNITS: [&str; 7] = ["goal", "question", "risk", "unknown", "done", "dropped", "part_of"];

pub struct Ids(pub BTreeMap<String, Entity>);

impl Ids {
    pub fn of(&mut self, id: &str) -> Entity {
        *self.0.entry(id.to_string()).or_default()
    }

    /// The same value in this core: unit components hold null rather than {}, and ids are handles.
    pub fn value(&mut self, component: &str, v: &Value) -> Value {
        if UNITS.contains(&component) && *v == json!({}) {
            return Value::Null;
        }
        if component == "decision"
            && let Some(c) = v.get("chosen").and_then(Value::as_str)
        {
            return json!({ "chosen": self.of(c) });
        }
        v.clone()
    }

    /// `state` is the kernel's store up to this op, since removing a link here is one op per kind.
    pub fn op(&mut self, op: &KOp, state: &Store) -> Vec<Op> {
        match op {
            KOp::Set(KTarget::Entity(id), c, v) => {
                let id = self.of(id);
                match v {
                    Some(v) => vec![Op::set_runtime(id, c, self.value(c, v))],
                    None => vec![Op::Set { id, component: c.clone(), to: None }],
                }
            }
            KOp::Set(KTarget::Link(f, t), c, v) => {
                let (from, to) = (self.of(f), self.of(t));
                match v {
                    Some(v) => vec![Op::link_runtime(from, to, Runtime::new(c, self.value(c, v)))],
                    None => vec![Op::Link { from, to, kind: c.clone(), value: None }],
                }
            }
            KOp::Remove(KTarget::Entity(id)) => vec![Op::Despawn { id: self.of(id) }],
            KOp::Remove(KTarget::Link(f, t)) => {
                let (from, to) = (self.of(f), self.of(t));
                match state.links.get(&(f.clone(), t.clone())) {
                    Some(kinds) => kinds.keys().map(|k| Op::Link { from, to, kind: k.clone(), value: None }).collect(),
                    None => vec![Op::Link { from, to, kind: "any".into(), value: None }],
                }
            }
            KOp::Restore(..) => unreachable!("only ever an inverse"),
        }
    }

    pub fn store(&mut self, s: &Store) -> SavedGraph {
        let mut g = SavedGraph::default();
        let mut links: BTreeMap<Entity, BTreeMap<Entity, BTreeMap<String, Value>>> = BTreeMap::new();
        for ((f, t), comps) in &s.links {
            let (from, to) = (self.of(f), self.of(t));
            for (c, v) in comps {
                let v = self.value(c, v);
                links.entry(from).or_default().entry(to).or_default().insert(c.clone(), v);
            }
        }
        for (id, comps) in &s.entities {
            let id = self.of(id);
            let components = comps.iter().map(|(c, v)| (c.clone(), self.value(c, v))).collect();
            let links = links.remove(&id).unwrap_or_default().into_iter().map(|(to, components)| recollect_graph::saved::SavedLink { to, components }).collect();
            g.nodes.push(recollect_graph::SavedNode { id, components, rules: BTreeMap::new(), links });
        }
        g.nodes.sort_by_key(|n| n.id);
        g
    }
}
