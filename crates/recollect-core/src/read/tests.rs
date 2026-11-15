//! A read gives what it's asked for, in the shape of the files, and nothing else.

use serde_json::{Value, json};

use super::*;
use crate::dispatch;
use crate::workspace::new_files;

/// A small design: a kind with two instances, a relation between them, and a decision a size hangs on.
fn design() -> Workspace {
    let (mut ws, _) = Workspace::load("test", "a test", None, new_files("Design").unwrap()).unwrap();
    let ops = json!([
        { "op": "add_node", "id": "new:concept", "name": "Concept", "cells": { "body": "A thing the design is about.", "layer": "data model" } },
        { "op": "add_node", "id": "new:delta", "name": "Delta", "cells": { "body": "An atomic change." }, "links": [{ "to": "new:concept", "component": "is_a" }] },
        { "op": "add_node", "id": "new:header", "name": "Header", "cells": { "body": "What a delta says about itself." },
          "links": [{ "to": "new:concept", "component": "is_a" }, { "to": "new:delta", "component": "relates", "cells": { "role": "names" } }] },
        { "op": "add_node", "id": "new:how", "name": "How is ancestry recorded?", "components": ["decision"] },
        { "op": "add_node", "id": "new:hash", "name": "By hash", "links": [{ "to": "new:how", "component": "alternative" }] },
        { "op": "add_node", "id": "new:count", "name": "By count", "links": [{ "to": "new:how", "component": "alternative" }] },
        { "op": "add_node", "id": "new:size", "name": "Parent size", "cells": { "value": { "formula": "if(chose(d, h), 32, 8)", "labels": { "d": "new:how", "h": "new:hash" } } } }
    ]);
    dispatch(&mut ws, "apply", json!({ "ops": ops, "why": "a start" }), "t").unwrap();
    ws
}

fn read_json(ws: &mut Workspace, spec: Value) -> Value {
    dispatch(ws, "read", spec, "t").unwrap_or_else(|e| panic!("{e:?}")).0
}

fn names(v: &Value) -> Vec<String> {
    v["nodes"].as_array().unwrap().iter().map(|n| n["cells"]["name"].as_str().unwrap_or("").to_string()).collect()
}

fn node<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["nodes"].as_array().unwrap().iter().find(|n| n["cells"]["name"] == name).unwrap_or_else(|| panic!("no {name} in {v}"))
}

fn handle(ws: &mut Workspace, name: &str) -> String {
    let v = read_json(ws, json!({ "nodes": { "ids": [name] } }));
    node(&v, name)["id"].as_str().unwrap().to_string()
}

#[test]
fn a_node_reads_as_its_file_says_it_with_names_beside_handles() {
    let mut ws = design();
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Header"] } }));
    assert_eq!(names(&v), ["Header"]);
    let header = node(&v, "Header");
    assert_eq!(header["cells"], json!({ "name": "Header", "body": "What a delta says about itself." }));
    let links = header["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    let to_delta = links.iter().find(|l| l["to_name"] == "Delta").unwrap();
    assert_eq!(to_delta["components"], json!(["relates"]));
    assert_eq!(to_delta["cells"], json!({ "role": "names" }));
    for nothing in ["worked_out", "history", "incoming", "beneath"] {
        assert!(header.get(nothing).is_none(), "{nothing} only comes when asked for: {header}");
    }

    // About the size of its file: the file, and the name of what each edge goes to.
    let id: Entity = header["id"].as_str().unwrap().parse().unwrap();
    let saved = ws.log.graph().save().nodes.into_iter().find(|n| n.id == id).unwrap();
    let file = recollect_graph::node_text(&saved).len();
    let read = serde_json::to_string(header).unwrap().len();
    assert!(read < file + 200, "read {read} bytes, file {file}");
}

#[test]
fn nodes_can_be_picked_by_kind_name_text_and_component() {
    let mut ws = design();
    let by_kind = read_json(&mut ws, json!({ "nodes": { "kind": "Concept" } }));
    assert_eq!(names(&by_kind), ["Delta", "Header"], "instances, and not the kind");
    let by_name = read_json(&mut ws, json!({ "nodes": { "name": "by *" } }));
    assert_eq!(names(&by_name), ["By hash", "By count"], "a wildcard, whatever the case");
    let by_text = read_json(&mut ws, json!({ "nodes": { "text": "atomic" } }));
    assert_eq!(names(&by_text), ["Delta"], "the notes are searched too");
    let decisions = read_json(&mut ws, json!({ "nodes": { "component": "decision" } }));
    assert_eq!(names(&decisions), ["How is ancestry recorded?"]);
    let both = read_json(&mut ws, json!({ "nodes": { "kind": "Concept", "name": "D*" } }));
    assert_eq!(names(&both), ["Delta"], "everything given has to hold");

    let none = dispatch(&mut ws, "read", json!({ "nodes": { "ids": ["Nothing"] } }), "t");
    assert!(matches!(none, Err(crate::Error::Refused(ref why)) if why.contains("no node Nothing")), "{none:?}");
    let wrong = dispatch(&mut ws, "read", json!({ "nodez": {} }), "t");
    assert!(matches!(wrong, Err(crate::Error::Refused(_))), "a misspelt field is refused, not ignored: {wrong:?}");
}

#[test]
fn cells_and_edges_are_only_what_was_asked_for() {
    let mut ws = design();
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Concept"] }, "cells": ["lay*"], "edges": { "incoming": true, "outgoing": false } }));
    let concept = node(&v, "Concept");
    assert_eq!(concept["cells"], json!({ "name": "Concept", "layer": "data model" }), "the name always comes");
    let from: Vec<&str> = concept["incoming"].as_array().unwrap().iter().map(|l| l["from_name"].as_str().unwrap()).collect();
    assert_eq!(from, ["Delta", "Header"]);

    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Header"] }, "edges": { "components": ["relates"], "cells": false } }));
    let links = node(&v, "Header")["links"].as_array().unwrap().clone();
    assert_eq!(links.len(), 1);
    assert!(links[0].get("cells").is_none());
}

#[test]
fn whats_worked_out_comes_in_a_section_of_its_own_once_where_its_the_same() {
    let mut ws = design();
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Parent size", "Delta"] }, "worked_out": true }));
    let size = node(&v, "Parent size");
    assert!(size["cells"].get("value").is_none(), "a formula is written, not a value: {size}");
    assert_eq!(size["formulas"]["value"], "if(chose(d, h), 32, 8)");
    let cases = size["worked_out"]["value"]["cases"].as_array().unwrap();
    let values: Vec<&Value> = cases.iter().map(|c| &c["value"]).collect();
    assert_eq!(values.len(), 2, "{size}");
    assert!(values.contains(&&json!(32.0)) && values.contains(&&json!(8.0)), "{size}");
    assert!(cases.iter().all(|c| c["when"].as_str().unwrap().contains("By hash") || c["when"].as_str().unwrap().contains("By count")), "worlds are said with names: {cases:?}");

    let delta = node(&v, "Delta");
    assert_eq!(delta["worked_out"]["body"], json!({ "value": "An atomic change." }), "the same in every world is one value");
    assert_eq!(delta["worked_out"]["layer"], json!({ "value": "data model", "from": "kind Concept" }), "and says where it came from");

    // Pinned, it's one value.
    let (how, hash) = (handle(&mut ws, "How is ancestry recorded?"), handle(&mut ws, "By hash"));
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Parent size"] }, "worked_out": true, "cells": ["value"], "world": { how: hash } }));
    assert_eq!(node(&v, "Parent size")["worked_out"]["value"], json!({ "value": 32.0 }));
}

#[test]
fn history_is_what_happened_not_every_op() {
    let mut ws = design();
    let delta = handle(&mut ws, "Delta");
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "set", "id": delta, "cell": "body", "value": "A change." }], "why": "shorter" }), "t").unwrap();
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Delta"] }, "history": 1 }));
    let history = node(&v, "Delta")["history"].as_array().unwrap().clone();
    assert_eq!(history.len(), 1, "only as many as asked for");
    assert_eq!(history[0]["why"], "shorter");
    assert_eq!(history[0]["ops"], 1);
    let v = read_json(&mut ws, json!({ "nodes": { "ids": ["Delta"] }, "history": 5 }));
    assert_eq!(node(&v, "Delta")["history"].as_array().unwrap().len(), 2);
}

#[test]
fn a_tree_is_whats_beneath_a_node_nested() {
    let mut ws = design();
    let v = read_json(&mut ws, json!({ "nodes": { "tree": { "root": "How is ancestry recorded?", "depth": 1 } }, "edges": { "outgoing": false } }));
    let top = &v["nodes"][0];
    assert_eq!(top["cells"]["name"], "How is ancestry recorded?");
    let beneath: Vec<&str> = top["beneath"].as_array().unwrap().iter().map(|n| n["cells"]["name"].as_str().unwrap()).collect();
    assert_eq!(beneath, ["By hash", "By count"]);
}

#[test]
fn a_big_read_says_how_many_more_there_are() {
    let mut ws = design();
    let v = read_json(&mut ws, json!({ "limit": 2 }));
    assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(v["more"], 5);
}

#[test]
fn wildcards_match_as_expected() {
    assert!(matches("Delta*", "delta store"));
    assert!(matches("*store", "Delta store"));
    assert!(matches("d?lta", "Delta"));
    assert!(!matches("d?lta", "Dlta"));
    assert!(matches("*", ""));
}
