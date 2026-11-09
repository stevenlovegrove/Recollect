//! Writing, as the web UI and Claude do it. Everything comes down to the log's ops, and this takes the
//! few things that make them easier to write:
//!
//! - A node can be named by handle or, if it's unambiguous, by its name.
//! - `"new:anything"` anywhere stands for a node that doesn't exist yet, and means the same node
//!   wherever it's used in one change-set.
//! - `{"op": "set", "id": .., "cell": .., "value": ..}` or `"formula": ..` writes in a cell, and with
//!   neither it takes away what's written there. With `"from"` and `"to"` instead of `"id"`, it's a cell
//!   of that edge. A formula can come with `"labels": {"a": .., "b": ..}`, which say what its names
//!   mean, as one change: labels it no longer has go. Writing in a cell of a component a node doesn't
//!   have gives it the component.
//! - `{"op": "add", "id": .., "component": ..}` and `"remove"` give a node a component or take it away.
//! - `{"op": "link", "from": .., "to": .., "component": .., "cells": {..}}` gives the edge from one node
//!   to another a component, and `"unlink"` takes it away.
//! - `{"op": "add_node", "id": "new:x", "name": .., "body": .., "components": [..], "cells": {..},
//!   "links": [{"to": .., "component": .., "cells": {..}}]}` writes out to several ops. A cell is a
//!   value, or `{"formula": .., "labels": {..}}`.
//! - `{"op": "delete_node", "id": ..}` takes the node's edges with it.
//! - `{"op": "table", "name": "Car", "is_a": "Vehicle", "columns": {..}, "rows": [{"key": "Hatchback",
//!   ..}, ..]}` writes a table: the node named `Car`, with `columns` its cells, which every row takes
//!   unless it says otherwise, and a node named `Car[Hatchback]` for each row, with its own cells.
//!   A row can be just its key, and `"range": [0, 10]` adds numbered rows. Nodes with those names
//!   already are used, not made again. `"alternative_of": "Which car?"` makes every row one of that
//!   decision's options.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::{CellType, Entity, Graph, Name, Op};
use recollect_systems::formulas::{Called, IS_A, LABEL};
use recollect_systems::notes::{Body, NOTES};
use recollect_systems::plan::ALTERNATIVE;
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
    /// The components this change-set gives each node, so far.
    adding: BTreeMap<String, BTreeSet<String>>,
    /// The nodes this change-set makes.
    spawned: BTreeSet<String>,
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

    fn entity(v: &Value) -> Result<Entity, String> {
        serde_json::from_value(v.clone()).map_err(|e| e.to_string())
    }

    /// Give a node a component, unless it has it, or this change-set has already given it something that
    /// brings it.
    fn ensure(&mut self, id: &Value, component: &str, out: &mut Vec<Value>) {
        let key = id.as_str().unwrap_or_default().to_string();
        let reg = self.g.registry();
        let mut has: BTreeSet<String> = self.adding.get(&key).cloned().unwrap_or_default();
        let mut todo: Vec<String> = has.iter().cloned().collect();
        while let Some(c) = todo.pop() {
            for r in reg.component_def(&c).map(|d| d.requires.clone()).unwrap_or_default() {
                if has.insert(r.clone()) {
                    todo.push(r);
                }
            }
        }
        let existing = key.parse::<Entity>().ok().is_some_and(|e| self.g.has(e, component));
        if !existing && !has.contains(component) {
            self.add(id, component, out);
        }
    }

    fn add(&mut self, id: &Value, component: &str, out: &mut Vec<Value>) {
        self.adding.entry(id.as_str().unwrap_or_default().to_string()).or_default().insert(component.to_string());
        out.push(json!({ "op": "add", "id": id, "component": component }));
    }

    /// Writing in a cell: `what` is `{"value": ..}`, `{"formula": .., "labels": {..}}`, or nothing, to
    /// take away what's there.
    fn set(&mut self, on: Map<String, Value>, cell: &str, mut what: Map<String, Value>, out: &mut Vec<Value>) -> Result<(), String> {
        let mut op = on.clone();
        op.insert("op".to_string(), json!("set"));
        op.insert("cell".to_string(), json!(cell));
        // On a node, writing in a component's cell gives the node that component.
        if let Some(id) = on.get("id").cloned()
            && let Some(owner) = self.g.registry().owner(cell).map(String::from)
            && !what.is_empty()
        {
            self.ensure(&id, &owner, out);
        }
        if let Some(labels) = what.remove("labels") {
            let id = on.get("id").cloned().ok_or("only a node's formula can have labels")?;
            self.labels(&id, labels, out)?;
        }
        for field in ["value", "formula"] {
            if let Some(v) = what.remove(field) {
                op.insert(field.to_string(), v);
            }
        }
        out.push(Value::Object(op));
        Ok(())
    }

    /// What a formula on `id` calls which nodes. These replace what's there, so a name the formula no
    /// longer uses doesn't stay.
    fn labels(&self, id: &Value, labels: Value, out: &mut Vec<Value>) -> Result<(), String> {
        let Value::Object(labels) = labels else { return Err("labels are {name: node}".to_string()) };
        if let Ok(known) = Self::entity(id)
            && self.g.contains(known)
        {
            for (to, edge) in self.g.edges_from(known, LABEL) {
                let name = self.g.get::<Called>(edge).ok().flatten().map(|c| c.0).unwrap_or_default();
                if !labels.contains_key(&name) {
                    out.push(json!({ "op": "unlink", "from": id, "to": to, "component": LABEL }));
                }
            }
        }
        for (name, nodes) in labels {
            let nodes = if let Value::Array(many) = nodes { many } else { vec![nodes] };
            for mut to in nodes {
                self.node(&mut to)?;
                out.push(json!({ "op": "link", "from": id, "to": to, "component": LABEL }));
                out.push(json!({ "op": "set", "from": id, "to": to, "cell": Called::NAME, "value": name }));
            }
        }
        Ok(())
    }

    fn write(&mut self, mut op: Map<String, Value>, out: &mut Vec<Value>) -> Result<(), String> {
        let kind = op.get("op").and_then(Value::as_str).unwrap_or("").to_string();
        match kind.as_str() {
            "add_node" => {
                let id = op.get("id").cloned().ok_or("add_node needs an id, such as \"new:x\"")?;
                // The same node, said twice in one change-set, is made once.
                if self.spawned.insert(id.as_str().unwrap_or_default().to_string()) {
                    out.push(json!({ "op": "spawn", "id": id }));
                }
                if let Some(name) = op.remove("name") {
                    out.push(json!({ "op": "set", "id": id, "cell": Name::NAME, "value": name }));
                }
                if let Some(Value::Array(components)) = op.remove("components") {
                    for c in components {
                        let c = c.as_str().ok_or("a component is its name")?.to_string();
                        self.add(&id, &c, out);
                    }
                }
                if let Some(body) = op.remove("body") {
                    self.ensure(&id, NOTES, out);
                    out.push(json!({ "op": "set", "id": id, "cell": Body::NAME, "value": body }));
                }
                if let Some(Value::Object(cells)) = op.remove("cells") {
                    for (c, v) in cells {
                        self.set(on_node(&id), &c, written(v), out)?;
                    }
                }
                if let Some(Value::Array(links)) = op.remove("links") {
                    for l in links {
                        let mut l = l.as_object().cloned().ok_or("a link has to be an object")?;
                        l.insert("from".to_string(), id.clone());
                        l.insert("op".to_string(), json!("link"));
                        self.write(l, out)?;
                    }
                }
            }
            "delete_node" => {
                self.nodes_in(&mut op)?;
                let id = Self::entity(op.get("id").ok_or("delete_node needs an id")?)?;
                for (edge, from, to) in self.g.edges().filter(|(_, f, t)| *f == id || *t == id) {
                    for c in self.g.given(edge) {
                        out.push(json!({ "op": "unlink", "from": from, "to": to, "component": c }));
                    }
                }
                out.push(json!({ "op": "despawn", "id": id }));
            }
            "set" => {
                self.nodes_in(&mut op)?;
                let cell = op.remove("cell").and_then(|c| c.as_str().map(String::from)).ok_or("set needs a cell")?;
                let mut what = Map::new();
                for field in ["value", "formula", "labels"] {
                    if let Some(v) = op.remove(field) {
                        what.insert(field.to_string(), v);
                    }
                }
                op.remove("op");
                self.set(op, &cell, what, out)?;
            }
            "link" => {
                self.nodes_in(&mut op)?;
                let (from, to) = (op.get("from").cloned().ok_or("link needs a from")?, op.get("to").cloned().ok_or("link needs a to")?);
                let component = op.get("component").cloned().ok_or("link needs a component")?;
                out.push(json!({ "op": "link", "from": from, "to": to, "component": component }));
                if let Some(Value::Object(cells)) = op.remove("cells") {
                    for (c, v) in cells {
                        let mut on = Map::new();
                        on.insert("from".to_string(), from.clone());
                        on.insert("to".to_string(), to.clone());
                        self.set(on, &c, written(v), out)?;
                    }
                }
            }
            "add" => {
                self.nodes_in(&mut op)?;
                let id = op.get("id").cloned().ok_or("add needs an id")?;
                let component = op.get("component").and_then(Value::as_str).ok_or("add needs a component")?.to_string();
                self.add(&id, &component, out);
            }
            _ => {
                self.nodes_in(&mut op)?;
                out.push(Value::Object(op));
            }
        }
        Ok(())
    }
}

fn on_node(id: &Value) -> Map<String, Value> {
    let mut on = Map::new();
    on.insert("id".to_string(), id.clone());
    on
}

/// A cell as it's given: a value, or `{"formula": .., "labels": {..}}`.
fn written(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(f) if f.contains_key("formula") => f,
        v => {
            let mut m = Map::new();
            m.insert("value".to_string(), v);
            m
        }
    }
}

/// A cell as `set` writes it: a value, or a formula with what its names mean.
fn set_cell(id: &str, cell: &str, v: Value) -> Value {
    match v {
        Value::Object(mut f) if f.contains_key("formula") => {
            let mut op = json!({ "op": "set", "id": id, "cell": cell, "formula": f.remove("formula") });
            if let Some(labels) = f.remove("labels") {
                op["labels"] = labels;
            }
            op
        }
        v => json!({ "op": "set", "id": id, "cell": cell, "value": v }),
    }
}

/// A table, as the ops that make it: see the top of this file.
fn table(mut op: Map<String, Value>, names: &BTreeMap<String, Vec<Entity>>) -> Result<Vec<Value>, String> {
    let name = op.remove("name").and_then(|n| n.as_str().map(str::to_string)).ok_or("a table needs a name, such as \"Year\"")?;
    if recollect_graph::table::row_of(&name).is_some() {
        return Err(format!("“{name}” is a row's name, so it can't be a table's"));
    }
    let mut out = Vec::new();
    // The node with that name, or a new one.
    let node = |n: &str, out: &mut Vec<Value>| -> Result<String, String> {
        match names.get(n).map(Vec::as_slice) {
            Some([one]) => Ok(one.to_string()),
            Some(many) if many.len() > 1 => Err(format!("{} nodes are called “{n}”", many.len())),
            _ => {
                let id = format!("new:table:{n}");
                if !out.iter().any(|o: &Value| o["id"] == id.as_str() && o["op"] == "add_node") {
                    out.push(json!({ "op": "add_node", "id": id, "name": n }));
                }
                Ok(id)
            }
        }
    };
    let at = node(&name, &mut out)?;
    if let Some(body) = op.remove("body") {
        out.push(json!({ "op": "set", "id": at, "cell": Body::NAME, "value": body }));
    }
    if let Some(kind) = op.remove("is_a") {
        // Another table this batch makes is new too, so it's found the same way.
        let kind = kind.as_str().ok_or("is_a names a table")?;
        let to = if kind.starts_with("new:") || kind.parse::<Entity>().is_ok() { kind.to_string() } else { node(kind, &mut out)? };
        out.push(json!({ "op": "link", "from": at, "to": to, "component": IS_A }));
    }
    if let Some(Value::Array(components)) = op.remove("components") {
        for c in components {
            out.push(json!({ "op": "add", "id": at, "component": c }));
        }
    }
    if let Some(columns) = op.remove("columns") {
        let Value::Object(columns) = columns else { return Err("a table's columns are {cell: value or {formula, labels}}".to_string()) };
        for (c, v) in columns {
            out.push(set_cell(&at, &c, v));
        }
    }
    let decision = op.remove("alternative_of");
    let mut rows: Vec<Value> = match op.remove("rows") {
        Some(Value::Array(rows)) => rows,
        None => Vec::new(),
        Some(_) => return Err("a table's rows are a list, each a key or {key, cells…}".to_string()),
    };
    if let Some(range) = op.remove("range") {
        let bounds: Option<(i64, i64)> = range.as_array().and_then(|r| Some((r.first()?.as_i64()?, r.get(1)?.as_i64()?)));
        let (from, to) = bounds.ok_or("a range is [first, last], such as [0, 10]")?;
        if to < from || to - from > 10_000 {
            return Err(format!("[{from}, {to}] isn't a range of up to 10,000 rows"));
        }
        let given: Vec<String> = rows.iter().filter_map(|r| r.get("key").or(Some(r)).map(key_text)).collect();
        rows.extend((from..=to).filter(|i| !given.contains(&i.to_string())).map(|i| json!(i)));
    }
    for row in rows {
        let (key, cells) = match row {
            Value::Object(mut cells) => (cells.remove("key").ok_or("a row needs a key")?, cells),
            key => (key, Map::new()),
        };
        let key = key_text(&key);
        if key.is_empty() || key.contains(['[', ']']) {
            return Err(format!("“{key}” can't be a row's key"));
        }
        let id = node(&format!("{name}[{key}]"), &mut out)?;
        for (c, v) in cells {
            out.push(set_cell(&id, &c, v));
        }
        if let Some(d) = &decision {
            out.push(json!({ "op": "link", "from": id, "to": d, "component": ALTERNATIVE }));
        }
    }
    Ok(out)
}

fn key_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    }
}

pub fn resolve_ops(ops: Vec<Value>, g: &Graph) -> Result<Resolved, String> {
    let mut names: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
    for e in g.entities() {
        if let Ok(Some(n)) = g.get::<Name>(e) {
            names.entry(n.0).or_default().push(e);
        }
    }
    let mut r = Resolver { g, created: BTreeMap::new(), names, adding: BTreeMap::new(), spawned: BTreeSet::new() };
    let mut written = Vec::new();
    let mut expanded = Vec::new();
    for op in ops {
        match op.as_object() {
            Some(fields) if fields.get("op").and_then(Value::as_str) == Some("table") => expanded.extend(table(fields.clone(), &r.names)?),
            _ => expanded.push(op),
        }
    }
    for mut op in expanded {
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
