//! Writing, as the web UI and Claude do it. Everything comes down to the log's five ops, and this
//! takes the few things that make them easier to write:
//!
//! - A node can be named by handle or, if it's unambiguous, by its name.
//! - `"new:anything"` anywhere stands for a node that doesn't exist yet, and means the same node
//!   wherever it's used in one change-set.
//! - `{"op": "set", "id": .., "component": .., "value": ..}` or `"rule"` and `"params"` sets a cell,
//!   and with neither it removes it.
//! - `{"op": "add_node", "id": "new:x", "name": .., "body": .., "components": {..}, "rules": {..},
//!   "links": [{"to": .., "kind": .., "value": ..}]}` writes out to several ops.
//! - `{"op": "delete_node", "id": ..}` takes the node's links with it.
//! - `{"op": "set", "id": .., "component": .., "formula": "a + b", "labels": {"a": .., "b": ..}}` sets
//!   a cell to a formula and says what its names mean, as one change. Labels it no longer has go.
//!   `add_node` takes `formula` and `labels` too, for the node's `value`.

use std::collections::BTreeMap;

use recollect_graph::{Component, Entity, Graph, Name, Op};
use recollect_systems::formulas::{EXPR, Label, ValueCell};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Ops ready for the log, and what any `new:` names in them became.
#[derive(Debug, Serialize)]
pub struct Resolved {
    #[serde(skip)]
    pub ops: Vec<Op>,
    pub created: BTreeMap<String, Entity>,
}

struct Resolver<'a> {
    g: &'a Graph,
    created: BTreeMap<String, Entity>,
    names: BTreeMap<String, Vec<Entity>>,
}

impl Resolver<'_> {
    /// Replace every `new:` string, wherever it is.
    fn fresh(&mut self, v: &mut Value) {
        match v {
            Value::String(s) if s.starts_with("new:") => *s = self.created.entry(s.clone()).or_default().to_string(),
            Value::Array(items) => items.iter_mut().for_each(|i| self.fresh(i)),
            Value::Object(fields) => fields.values_mut().for_each(|i| self.fresh(i)),
            _ => {}
        }
    }

    /// A node reference that may be a name, as a handle.
    fn node(&self, v: &mut Value) -> Result<(), String> {
        let Value::String(s) = v else { return Err(format!("{v} isn't a node")) };
        if s.parse::<Entity>().is_ok() {
            return Ok(());
        }
        match self.names.get(s.as_str()).map(Vec::as_slice) {
            Some([one]) => {
                *s = one.to_string();
                Ok(())
            }
            Some(many) => Err(format!("{} nodes are called “{s}”: use a handle", many.len())),
            None => Err(format!("there's no node called “{s}”")),
        }
    }

    fn nodes_in(&self, op: &mut Map<String, Value>) -> Result<(), String> {
        for field in ["id", "from", "to"] {
            if let Some(v) = op.get_mut(field) {
                self.node(v)?;
            }
        }
        Ok(())
    }

    /// What a formula on `id` calls which nodes.
    fn labels(&self, id: &Value, labels: Value, out: &mut Vec<Value>) -> Result<(), String> {
        let Value::Object(labels) = labels else { return Err("labels are {name: node}".to_string()) };
        for (name, nodes) in labels {
            let nodes = if let Value::Array(many) = nodes { many } else { vec![nodes] };
            for mut to in nodes {
                self.node(&mut to)?;
                out.push(json!({ "op": "link", "from": id, "to": to, "kind": Label::NAME, "value": name }));
            }
        }
        Ok(())
    }

    fn write(&self, mut op: Map<String, Value>, out: &mut Vec<Value>) -> Result<(), String> {
        let kind = op.get("op").and_then(Value::as_str).unwrap_or("").to_string();
        match kind.as_str() {
            "add_node" => {
                let id = op.get("id").cloned().ok_or("add_node needs an id, such as \"new:x\"")?;
                let mut set = |component: &str, to: Value| out.push(json!({ "op": "set", "id": id, "component": component, "to": to }));
                if let Some(name) = op.remove("name") {
                    set(Name::NAME, json!({ "value": name }));
                }
                if let Some(body) = op.remove("body") {
                    set("body", json!({ "value": body }));
                }
                if let Some(Value::Object(components)) = op.remove("components") {
                    for (c, v) in components {
                        set(&c, json!({ "value": v }));
                    }
                }
                if let Some(Value::Object(rules)) = op.remove("rules") {
                    for (c, r) in rules {
                        set(&c, json!({ "rule": r }));
                    }
                }
                if let Some(formula) = op.remove("formula") {
                    set(ValueCell::NAME, json!({ "rule": { "rule": EXPR, "params": { "text": formula } } }));
                }
                if !out.iter().any(|o| o["id"] == id) {
                    out.push(json!({ "op": "spawn", "id": id }));
                }
                if let Some(labels) = op.remove("labels") {
                    self.labels(&id, labels, out)?;
                }
                if let Some(Value::Array(links)) = op.remove("links") {
                    for l in links {
                        let mut l = l.as_object().cloned().ok_or("a link has to be an object")?;
                        self.nodes_in(&mut l)?;
                        let to = l.get("to").cloned().ok_or("a link needs a `to`")?;
                        let kind = l.get("kind").cloned().ok_or("a link needs a `kind`")?;
                        out.push(json!({ "op": "link", "from": id, "to": to, "kind": kind, "value": l.get("value").cloned().unwrap_or(Value::Null) }));
                    }
                }
            }
            "delete_node" => {
                self.nodes_in(&mut op)?;
                let id: Entity = serde_json::from_value(op.get("id").cloned().ok_or("delete_node needs an id")?).map_err(|e| e.to_string())?;
                for (from, to, bag) in self.g.links().filter(|(f, t, _)| *f == id || *t == id) {
                    for k in bag.keys() {
                        out.push(json!({ "op": "link", "from": from, "to": to, "kind": k }));
                    }
                }
                out.push(json!({ "op": "despawn", "id": id }));
            }
            "set" if !op.contains_key("to") => {
                self.nodes_in(&mut op)?;
                if let Some(labels) = op.remove("labels") {
                    let id = op.get("id").cloned().ok_or("set needs an id")?;
                    let known: Entity = serde_json::from_value(id.clone()).map_err(|e| e.to_string())?;
                    // These replace what's there, so a name the formula no longer uses doesn't stay.
                    for (from, to, bag) in self.g.links().filter(|(f, _, b)| *f == known && b.contains_key(Label::NAME)) {
                        let kept = labels.as_object().is_some_and(|l| l.contains_key(bag[Label::NAME].as_str().unwrap_or("")));
                        if !kept {
                            out.push(json!({ "op": "link", "from": from, "to": to, "kind": Label::NAME }));
                        }
                    }
                    self.labels(&id, labels, out)?;
                }
                let to = if let Some(formula) = op.remove("formula") {
                    json!({ "rule": { "rule": EXPR, "params": { "text": formula } } })
                } else if let Some(v) = op.remove("value") {
                    json!({ "value": v })
                } else if let Some(rule) = op.remove("rule") {
                    json!({ "rule": { "rule": rule, "params": op.remove("params").unwrap_or(Value::Null) } })
                } else {
                    Value::Null
                };
                op.insert("to".to_string(), to);
                out.push(Value::Object(op));
            }
            _ => {
                self.nodes_in(&mut op)?;
                out.push(Value::Object(op));
            }
        }
        Ok(())
    }
}

pub fn resolve_ops(ops: Vec<Value>, g: &Graph) -> Result<Resolved, String> {
    let mut names: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
    for e in g.entities() {
        if let Ok(Some(n)) = g.get::<Name>(e) {
            names.entry(n.0).or_default().push(e);
        }
    }
    let mut r = Resolver { g, created: BTreeMap::new(), names };
    let mut written = Vec::new();
    for mut op in ops {
        r.fresh(&mut op);
        let fields = op.as_object().cloned().ok_or_else(|| format!("{op} isn't an op"))?;
        r.write(fields, &mut written)?;
    }
    let mut out = Vec::new();
    for w in written {
        out.push(serde_json::from_value(w.clone()).map_err(|e| format!("{w} isn't an op: {e}"))?);
    }
    Ok(Resolved { ops: out, created: r.created })
}
