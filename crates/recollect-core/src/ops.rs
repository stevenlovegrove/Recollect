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
//!   A row can be just its key, and `"range": [0, 10]` adds numbered rows. A row of a grid has a key
//!   with a part for each way across, as `[2, 3]` or `"2, 3"`, and `"extent": "1..9, 1..9"` says how far
//!   the table goes, so every row inside it is there without being written: see `docs/expressions.md`. Nodes with those names
//!   already are used, not made again. `"alternative_of": "Which car?"` makes every row one of that
//!   decision's options. `"csv"` in place of `"rows"` reads them from the text of a CSV file, whose first
//!   line names the columns, numbered from 1 unless `"key"` names the column their keys are in: see
//!   `docs/data.md`.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::{Backed, CellType, Entity, Graph, Name, NodeKind, Op};
use recollect_systems::formulas::{Called, IS_A, LABEL};
use recollect_systems::notes::{Body, EdgeKind, NOTES, RELATES};
use recollect_systems::structure::ALTERNATIVE;
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

    fn nodes_in(&mut self, op: &mut Map<String, Value>, out: &mut Vec<Value>) -> Result<(), String> {
        for field in ["id", "from", "to"] {
            if let Some(v) = op.get_mut(field) {
                self.node(v)?;
                self.bring(v, out);
            }
        }
        Ok(())
    }

    /// A row nobody wrote is made a node, with the handle it already has and its name, before anything is
    /// written on it, so it's the same node after as before.
    fn bring(&mut self, id: &Value, out: &mut Vec<Value>) {
        let Some(e) = id.as_str().and_then(|s| s.parse::<Entity>().ok()) else { return };
        if self.g.contains(e) || self.g.kind_of(e) != Some(NodeKind::Row) || !self.spawned.insert(e.to_string()) {
            return;
        }
        let name = self.g.get_value(e, Name::NAME).ok().flatten().unwrap_or(Value::Null);
        out.push(json!({ "op": "spawn", "id": id }));
        out.push(json!({ "op": "set", "id": id, "cell": Name::NAME, "value": name }));
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
            self.labels(&id, cell, labels, out)?;
        }
        // A node named in a value, as {"ref": ..}, can be named by its name, as anywhere else in an op.
        if let Some(Value::Object(v)) = what.get_mut("value")
            && v.len() == 1
            && let Some(r) = v.get_mut("ref")
        {
            self.node(r)?;
        }
        for field in ["value", "formula"] {
            if let Some(v) = what.remove(field) {
                op.insert(field.to_string(), v);
            }
        }
        out.push(Value::Object(op));
        Ok(())
    }

    /// What a formula in `cell` on `id` calls which nodes. These replace what's there, so a name the
    /// formula no longer uses doesn't stay, unless another formula on the node still does.
    fn labels(&mut self, id: &Value, cell: &str, labels: Value, out: &mut Vec<Value>) -> Result<(), String> {
        let Value::Object(labels) = labels else { return Err("labels are {name: node}".to_string()) };
        if let Ok(known) = Self::entity(id)
            && self.g.contains(known)
        {
            let others: Vec<String> = self.g.written(known).into_iter().filter(|(c, _)| c != cell).filter_map(|(_, b)| if let Backed::Formula(t) = b { Some(t) } else { None }).collect();
            for (to, edge) in self.g.edges_from(known, LABEL) {
                let name = self.g.get::<Called>(edge).ok().flatten().map(|c| c.0).unwrap_or_default();
                if !labels.contains_key(&name) && !others.iter().any(|t| uses_name(t, &name)) {
                    out.push(json!({ "op": "unlink", "from": id, "to": to, "component": LABEL }));
                }
            }
        }
        for (name, nodes) in labels {
            let nodes = if let Value::Array(many) = nodes { many } else { vec![nodes] };
            for mut to in nodes {
                self.node(&mut to)?;
                self.bring(&to, out);
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
                // A component one of its own links gives it, as part_of gives `part`, isn't written as well: it
                // would only say again what the link says, and stay behind if the link went.
                let given: Vec<String> = match op.get("links") {
                    Some(Value::Array(links)) => links
                        .iter()
                        .filter_map(|l| l.get("component")?.as_str())
                        .filter_map(|c| recollect_systems::everything().component_def(c))
                        .flat_map(|d| d.source.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                if let Some(Value::Array(components)) = op.remove("components") {
                    for c in components {
                        let c = c.as_str().ok_or("a component is its name")?.to_string();
                        if !given.contains(&c) {
                            self.add(&id, &c, out);
                        }
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
                self.nodes_in(&mut op, out)?;
                let id = Self::entity(op.get("id").ok_or("delete_node needs an id")?)?;
                for (edge, from, to) in self.g.edges().filter(|(_, f, t)| *f == id || *t == id) {
                    for c in self.g.given(edge) {
                        out.push(json!({ "op": "unlink", "from": from, "to": to, "component": c }));
                    }
                }
                out.push(json!({ "op": "despawn", "id": id }));
            }
            "set" => {
                self.nodes_in(&mut op, out)?;
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
                self.nodes_in(&mut op, out)?;
                let (from, to) = (op.get("from").cloned().ok_or("link needs a from")?, op.get("to").cloned().ok_or("link needs a to")?);
                // A link of a kind the workspace defines is a `relates` edge naming that relation.
                let kind = op.remove("kind");
                let component = match (op.get("component").cloned(), &kind) {
                    (Some(c), _) => c,
                    (None, Some(_)) => json!(RELATES),
                    (None, None) => return Err("link needs a component, or a kind".to_string()),
                };
                out.push(json!({ "op": "link", "from": from, "to": to, "component": component }));
                if let Some(mut kind) = kind {
                    self.node(&mut kind).map_err(|e| format!("a link's kind is a relation: {e}"))?;
                    if component != json!(RELATES) {
                        out.push(json!({ "op": "link", "from": from, "to": to, "component": RELATES }));
                    }
                    let mut on = Map::new();
                    on.insert("from".to_string(), from.clone());
                    on.insert("to".to_string(), to.clone());
                    self.set(on, EdgeKind::NAME, written(json!({ "ref": kind })), out)?;
                }
                if let Some(Value::Object(cells)) = op.remove("cells") {
                    for (c, v) in cells {
                        let mut on = Map::new();
                        on.insert("from".to_string(), from.clone());
                        on.insert("to".to_string(), to.clone());
                        self.set(on, &c, written(v), out)?;
                    }
                }
            }
            "remove" => {
                self.nodes_in(&mut op, out)?;
                let id = op.get("id").cloned().ok_or("remove needs an id")?;
                let component = op.get("component").and_then(Value::as_str).ok_or("remove needs a component")?.to_string();
                // What it said in that component's cells goes with it, so nothing is left without its component.
                if let Ok(e) = Self::entity(&id) {
                    let reg = self.g.registry();
                    for cell in self.g.written(e).into_keys().filter(|c| reg.owner(c) == Some(component.as_str())) {
                        out.push(json!({ "op": "set", "id": id, "cell": cell }));
                    }
                }
                out.push(json!({ "op": "remove", "id": id, "component": component }));
            }
            "add" => {
                self.nodes_in(&mut op, out)?;
                let id = op.get("id").cloned().ok_or("add needs an id")?;
                let component = op.get("component").and_then(Value::as_str).ok_or("add needs a component")?.to_string();
                self.add(&id, &component, out);
            }
            _ => {
                self.nodes_in(&mut op, out)?;
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

/// Whether a formula might use this name. It can say so of a name that's only in some text, which keeps a
/// label that could have gone, and never the other way round.
fn uses_name(formula: &str, name: &str) -> bool {
    formula.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == name)
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
    // How far the table goes, such as "1..9, 1..9": every row inside it is there, written or not.
    if let Some(extent) = op.remove("extent") {
        out.push(set_cell(&at, "extent", extent));
    }
    if let Some(columns) = op.remove("columns") {
        let Value::Object(columns) = columns else { return Err("a table's columns are {cell: value or {formula, labels}}".to_string()) };
        for (c, v) in columns {
            out.push(set_cell(&at, &c, v));
        }
    }
    let decision = op.remove("alternative_of");
    let key_column = op.remove("key");
    let mut rows: Vec<Value> = match (op.remove("rows"), op.remove("csv")) {
        (Some(_), Some(_)) => return Err("a table's rows come from \"rows\" or from \"csv\", not both".to_string()),
        (None, Some(Value::String(csv))) => csv_rows(&csv, key_column.as_ref().and_then(Value::as_str))?,
        (None, Some(_)) => return Err("\"csv\" is the text of a CSV file".to_string()),
        (_, None) if key_column.is_some() => return Err("\"key\" names the CSV column the rows' keys are in, so it needs \"csv\"".to_string()),
        (Some(Value::Array(rows)), None) => rows,
        (None, None) => Vec::new(),
        (Some(_), None) => return Err("a table's rows are a list, each a key or {key, cells…}".to_string()),
    };
    if let Some(range) = op.remove("range") {
        let bounds: Option<(i64, i64)> = range.as_array().and_then(|r| Some((r.first()?.as_i64()?, r.get(1)?.as_i64()?)));
        let (from, to) = bounds.ok_or("a range is [first, last], such as [0, 10]")?;
        if to < from || to - from > 10_000 {
            return Err(format!("[{from}, {to}] isn't a range of up to 10,000 rows"));
        }
        let given: Vec<String> = rows.iter().filter_map(|r| r.get("key").or(Some(r)).map(key_text)).collect();
        // A range is of one part: a grid says how far it goes with an extent instead.
        rows.extend((from..=to).filter(|i| !given.contains(&i.to_string())).map(|i| json!(i)));
    }
    for row in rows {
        let (key, cells) = match row {
            Value::Object(mut cells) => (cells.remove("key").ok_or("a row needs a key")?, cells),
            key => (key, Map::new()),
        };
        // A row of a grid has a key with a part for each way across: [2, 3], or "2, 3". It's written as the
        // row would be named, so a row that's there already, written or not, is found by it.
        let said = match &key {
            Value::Array(parts) if parts.iter().any(|p| key_text(p).contains(',')) => return Err(format!("{key} has a part with a comma in it, which would read as two parts")),
            Value::Array(parts) => parts.iter().map(key_text).collect::<Vec<_>>().join(", "),
            k => key_text(k),
        };
        let key = match recollect_graph::table::Key::parse(&said) {
            Some(k) if !said.contains(['[', ']']) => k,
            _ => return Err(format!("“{said}” can't be a row's key")),
        };
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

/// The fields of each line of a CSV file. A field can be quoted, with `""` for a quote inside it, and a
/// quoted field can run over lines.
fn csv_fields(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut lines = Vec::new();
    let (mut line, mut field) = (Vec::new(), String::new());
    let mut chars = text.chars().peekable();
    let (mut quoted, mut was_quoted) = (false, false);
    let mut number = 1;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                c => {
                    if c == '\n' {
                        number += 1;
                    }
                    field.push(c);
                }
            }
            continue;
        }
        match c {
            '"' if field.trim().is_empty() && !was_quoted => {
                field.clear();
                quoted = true;
                was_quoted = true;
            }
            '"' => return Err(format!("line {number} of the CSV has a quote inside a field that isn't quoted")),
            ',' => {
                line.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                line.push(std::mem::take(&mut field));
                lines.push(std::mem::take(&mut line));
                was_quoted = false;
                number += 1;
            }
            c => field.push(c),
        }
    }
    if quoted {
        return Err("the CSV ends inside a quoted field".to_string());
    }
    if !field.is_empty() || !line.is_empty() {
        line.push(field);
        lines.push(line);
    }
    // Blank lines say nothing.
    lines.retain(|l| !(l.len() == 1 && l[0].trim().is_empty()));
    Ok(lines)
}

/// A table's rows, from a CSV file whose first line names the columns: see `docs/data.md`.
fn csv_rows(text: &str, key: Option<&str>) -> Result<Vec<Value>, String> {
    let mut lines = csv_fields(text)?.into_iter();
    let header = lines.next().ok_or("the CSV is empty: its first line names the columns")?;
    let mut names = Vec::with_capacity(header.len());
    for h in &header {
        let name: String = h.trim().chars().map(|c| if c == ' ' || c == '-' { '_' } else { c }).collect();
        if !recollect_systems::expr::is_name(&name) {
            return Err(format!("the CSV's column “{}” can't be what a formula calls a cell: a column is called by one name, such as t5k or finish_time", h.trim()));
        }
        if names.contains(&name) {
            return Err(format!("the CSV has two columns called {name}"));
        }
        names.push(name);
    }
    let key_at = match key {
        Some(k) => Some((0..names.len()).find(|i| names[*i] == k || header[*i].trim() == k).ok_or_else(|| format!("the CSV has no column called {k} for the rows' keys"))?),
        None => None,
    };
    let mut rows = Vec::new();
    for (i, fields) in lines.enumerate() {
        let line = i + 2;
        if fields.len() != names.len() {
            return Err(format!("line {line} of the CSV has {} fields, and its first line names {}", fields.len(), names.len()));
        }
        let mut row = Map::new();
        row.insert("key".to_string(), match key_at {
            Some(k) if fields[k].trim().is_empty() => return Err(format!("line {line} of the CSV has no key")),
            Some(k) => json!(fields[k].trim()),
            None => json!(i + 1),
        });
        for (at, (name, field)) in names.iter().zip(&fields).enumerate() {
            if Some(at) == key_at {
                continue;
            }
            let text = field.trim();
            let value = match text {
                "" => continue,
                "true" | "TRUE" | "True" => json!(true),
                "false" | "FALSE" | "False" => json!(false),
                _ => match text.parse::<f64>() {
                    Ok(n) if n.is_finite() => json!(n),
                    _ => json!(text),
                },
            };
            row.insert(name.clone(), value);
        }
        rows.push(Value::Object(row));
    }
    Ok(rows)
}

fn key_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    }
}

pub fn resolve_ops(ops: Vec<Value>, g: &Graph) -> Result<Resolved, String> {
    let mut names: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
    for e in g.nodes().writable() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_csv_is_the_rows_it_would_be_written_as_by_hand() {
        let csv = "id,finish time,name,fit\r\nann,21.5,\"Smith, \"\"Ann\"\"\",true\n\nbob,,Bob,false\n";
        let rows = csv_rows(csv, Some("id")).unwrap();
        assert_eq!(rows, vec![json!({"key": "ann", "finish_time": 21.5, "name": "Smith, \"Ann\"", "fit": true}), json!({"key": "bob", "name": "Bob", "fit": false})]);
        // Numbered from 1 without a key, and the last line needn't end.
        let rows = csv_rows("x,y\n1,2\n3,4", None).unwrap();
        assert_eq!(rows, vec![json!({"key": 1, "x": 1.0, "y": 2.0}), json!({"key": 2, "x": 3.0, "y": 4.0})]);
        // A quoted field can run over lines.
        let rows = csv_rows("note\n\"two\nlines\"\n", None).unwrap();
        assert_eq!(rows, vec![json!({"key": 1, "note": "two\nlines"})]);
    }

    #[test]
    fn a_csv_that_can_t_be_a_table_says_why() {
        let err = |csv: &str, key: Option<&str>| csv_rows(csv, key).unwrap_err();
        assert!(err("", None).contains("empty"));
        assert!(err("a.b,c\n1,2", None).contains("“a.b”"));
        assert!(err("5k time\n1", None).contains("“5k time”"), "a name starts with a letter");
        assert!(err("row,c\n1,2", None).contains("“row”"), "a word formulas use for something else");
        assert!(err("a,a\n1,2", None).contains("two columns called a"));
        assert!(err("a,b\n1,2,3", None).contains("line 2 of the CSV has 3 fields"));
        assert!(err("a,b\n1,2", Some("id")).contains("no column called id"));
        assert!(err("a\n\"open", None).contains("ends inside"));
        assert!(err("a\nx\"y", None).contains("quote inside"));
    }
}
