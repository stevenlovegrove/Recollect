//! A workspace given files and saying which to write, as a host uses it.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::dispatch;

fn open(files: Files) -> Workspace {
    let (ws, problems) = Workspace::load("test", "a test", None, files).unwrap();
    assert_eq!(problems, Vec::<String>::new());
    ws
}

fn invoice() -> Workspace {
    open(sample_files("invoice").unwrap())
}

fn view(ws: &mut Workspace) -> Value {
    dispatch(ws, "view", json!({}), "t").unwrap().0
}

fn id(v: &Value, name: &str) -> String {
    v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("no {name}"))["id"].as_str().unwrap().to_string()
}

fn value(v: &Value, name: &str) -> Value {
    v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap()["cells"]["value"]["value"].clone()
}

#[test]
fn every_sample_opens_with_nothing_to_write() {
    for s in SAMPLES {
        let mut ws = open(sample_files(s.name).unwrap());
        assert!(ws.take_writes().is_empty(), "{}", s.name);
        assert_eq!(ws.config.name, s.title);
    }
}

#[test]
fn a_change_writes_only_the_files_it_touched() {
    let mut ws = invoice();
    let v = view(&mut ws);
    let rate = id(&v, "Tax rate");
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "set", "id": rate, "cell": "value", "value": 0.5 }] }), "t").unwrap();
    let w = ws.take_writes();
    assert_eq!(w.write.keys().collect::<Vec<_>>(), vec![&format!("nodes/{rate}.yaml")]);
    assert!(w.write.values().next().unwrap().contains("0.5"));
    assert!(ws.take_writes().is_empty(), "once");
    assert_eq!(value(&view(&mut ws), "Tax"), json!(40.0));

    // A deleted node's file goes.
    let gears = id(&view(&mut ws), "Gears");
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "delete_node", "id": gears }] }), "t").unwrap();
    let w = ws.take_writes();
    assert!(w.delete.contains(&format!("nodes/{gears}.yaml")), "{w:?}");
}

#[test]
fn undo_and_redo_write_files_like_any_change() {
    let mut ws = invoice();
    let rate = id(&view(&mut ws), "Tax rate");
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "set", "id": rate, "cell": "value", "value": 0.5 }] }), "t").unwrap();
    let after = ws.take_writes();
    dispatch(&mut ws, "undo", json!({}), "t").unwrap();
    let undone = ws.take_writes();
    assert_eq!(undone.write.keys().collect::<Vec<_>>(), after.write.keys().collect::<Vec<_>>());
    assert_eq!(value(&view(&mut ws), "Tax"), json!(16.0));
    assert_eq!(view(&mut ws)["redo"], json!(1));
    dispatch(&mut ws, "redo", json!({}), "t").unwrap();
    assert_eq!(ws.take_writes(), after, "the same text as before");
    assert_eq!(value(&view(&mut ws), "Tax"), json!(40.0));
}

#[test]
fn a_file_changed_underneath_is_an_edit_and_its_own_writes_are_not() {
    let mut ws = invoice();
    let rate = id(&view(&mut ws), "Tax rate");
    let path = format!("nodes/{rate}.yaml");
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "set", "id": rate, "cell": "value", "value": 0.5 }] }), "t").unwrap();
    let w = ws.take_writes();
    let events = ws.log.events().len();

    // The host telling it about its own write is no change.
    let echo = ws.files_changed(w.write.iter().map(|(p, t)| (p.clone(), Some(t.clone()))).collect());
    assert_eq!(echo, Changed::default());
    assert_eq!(ws.log.events().len(), events);

    // Someone else's is.
    let text = ws.files()[&path].replace("0.5", "0.25");
    let changed = ws.files_changed(BTreeMap::from([(path.clone(), Some(text))]));
    assert!(changed.graph, "{changed:?}");
    assert_eq!(ws.log.events().last().unwrap().author, "disk");
    assert_eq!(value(&view(&mut ws), "Tax"), json!(20.0));
    assert!(ws.take_writes().is_empty(), "and it isn't written back, since it already says that");

    // And it can be undone like anything else.
    dispatch(&mut ws, "undo", json!({}), "t").unwrap();
    assert_eq!(value(&view(&mut ws), "Tax"), json!(40.0));
    assert!(ws.take_writes().write.contains_key(&path));
}

#[test]
fn a_file_that_cant_be_read_is_left_as_it_was_understood() {
    let mut ws = invoice();
    let rate = id(&view(&mut ws), "Tax rate");
    let changed = ws.files_changed(BTreeMap::from([(format!("nodes/{rate}.yaml"), Some("id: [half, written".to_string()))]));
    assert_eq!(changed.problems.len(), 1, "{changed:?}");
    assert!(!changed.graph);
    assert_eq!(value(&view(&mut ws), "Tax"), json!(16.0));
}

#[test]
fn a_node_stays_in_whatever_file_it_was_made_in() {
    let mut ws = invoice();
    let new = recollect_graph::Entity::new();
    let changed = ws.files_changed(BTreeMap::from([("nodes/an-idea.yaml".to_string(), Some(format!("id: {new}\ncells:\n  name: An idea\n")))]));
    assert!(changed.graph);
    assert!(view(&mut ws)["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "An idea"));
    assert!(ws.take_writes().is_empty(), "it isn't copied to a file named for it: {:?}", ws.files().keys().collect::<Vec<_>>());

    // A change to it goes back where it lives, so whoever made the file sees it there.
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "set", "id": new.to_string(), "cell": "name", "value": "A better idea" }] }), "t").unwrap();
    let w = ws.take_writes();
    assert_eq!(w.write.keys().collect::<Vec<_>>(), vec!["nodes/an-idea.yaml"]);
    assert!(w.write["nodes/an-idea.yaml"].contains("A better idea"));

    // And opened again, it's still there.
    let (mut again, _) = Workspace::load("t", "t", None, ws.files().clone()).unwrap();
    assert!(again.take_writes().is_empty());

    // Its file going is the node going.
    let changed = ws.files_changed(BTreeMap::from([("nodes/an-idea.yaml".to_string(), None)]));
    assert!(changed.graph);
    assert!(!view(&mut ws)["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "A better idea"));
    let gone = id(&view(&mut ws), "Gears");
    ws.files_changed(BTreeMap::from([(format!("nodes/{gone}.yaml"), None)]));
    assert!(!view(&mut ws)["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "Gears"));
}

#[test]
fn a_proposal_left_from_when_there_was_an_inbox_is_ignored() {
    let path = "proposals/1-higher-tax.json".to_string();
    let mut files = sample_files("invoice").unwrap();
    files.insert(path.clone(), "{ not even what a proposal was }".to_string());
    let mut ws = open(files);
    assert!(ws.take_writes().is_empty());
    assert_eq!(ws.files_changed(BTreeMap::from([(path, Some("{}".to_string()))])), Changed::default());
}

#[test]
fn a_new_workspace_is_just_its_settings() {
    let files = new_files("Mine").unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), vec![CONFIG]);
    let mut ws = open(files);
    assert_eq!(view(&mut ws)["workspace"]["name"], "Mine");
    assert_eq!(ws.config.systems, Vec::<String>::new());
}

fn systems(ws: &Workspace) -> Vec<&str> {
    ws.config.systems.iter().map(String::as_str).collect()
}

#[test]
fn a_workspace_takes_on_a_system_when_it_first_uses_one_and_keeps_its_history() {
    let mut ws = open(new_files("Mine").unwrap());
    ws.take_writes();
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:a", "name": "An idea" }] }), "t").unwrap();
    assert_eq!(systems(&ws), Vec::<&str>::new(), "a note needs nothing more");

    // A decision is the plan system's.
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:d", "name": "Which way?", "components": ["decision"] }] }), "t").unwrap();
    assert_eq!(systems(&ws), vec!["plan"]);
    let written = ws.take_writes();
    assert!(written.write[CONFIG].contains("plan"), "{}", written.write[CONFIG]);

    // A task is dates as well.
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:t", "name": "Do it", "components": ["task"] }] }), "t").unwrap();
    assert_eq!(systems(&ws), vec!["plan", "schedule"]);
    let v = view(&mut ws);
    let task = v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "Do it").unwrap();
    assert!(task["cells"]["finish"].is_object(), "it has dates: {task}");

    // Nothing that happened before is lost, and taking the task back doesn't take the system with it.
    assert_eq!(v["last_seq"], 2);
    dispatch(&mut ws, "undo", json!({}), "t").unwrap();
    assert_eq!(systems(&ws), vec!["plan", "schedule"]);
    dispatch(&mut ws, "undo", json!({ "seq": 0 }), "t").unwrap();
    assert!(!view(&mut ws)["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "An idea"));
}

#[test]
fn writing_in_a_cell_of_a_system_not_yet_used_takes_it_on() {
    let mut ws = open(new_files("Mine").unwrap());
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:t", "name": "Do it", "cells": { "estimate": 3 } }] }), "t").unwrap();
    assert_eq!(systems(&ws), vec!["plan", "schedule"]);
    let v = view(&mut ws);
    let task = v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "Do it").unwrap();
    assert!(task["components"].as_array().unwrap().iter().any(|c| c["name"] == "task"), "{task}");
}

#[test]
fn a_refused_change_takes_on_no_system() {
    let mut ws = open(new_files("Mine").unwrap());
    ws.take_writes();
    let bad = dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:t", "name": "Do it", "components": ["task"], "cells": { "estimate": { "formula": "nope" } } }] }), "t");
    assert!(bad.is_err());
    assert_eq!(systems(&ws), Vec::<&str>::new());
    assert!(ws.take_writes().is_empty());
    // And it still works afterwards, without them.
    dispatch(&mut ws, "apply", json!({ "ops": [{ "op": "add_node", "id": "new:a", "name": "An idea" }] }), "t").unwrap();
}

#[test]
fn nodes_written_by_hand_bring_their_systems_with_them() {
    let mut files = new_files("Mine").unwrap();
    let id = recollect_graph::Entity::new();
    let node = format!("id: {id}\ncomponents:\n- task\ncells:\n  name: By hand\n");
    files.insert(format!("nodes/{id}.yaml"), node.clone());
    let mut ws = open(files);
    assert_eq!(systems(&ws), vec!["plan", "schedule"]);
    assert!(ws.take_writes().write[CONFIG].contains("schedule"));

    // Arriving while it's open, it needs opening again, with them.
    let mut ws = open(new_files("Mine").unwrap());
    assert!(ws.files_changed(BTreeMap::from([(format!("nodes/{id}.yaml"), Some(node))])).reopen);
}

#[test]
fn files_from_an_older_version_are_ignored() {
    let mut files = sample_files("invoice").unwrap();
    files.insert("log.jsonl".into(), "not even json".into());
    files.insert("log.applied".into(), "3\n".into());
    let mut ws = open(files);
    assert!(ws.take_writes().is_empty());
}

#[test]
fn settings_changed_on_disk_come_in_and_new_systems_need_reopening() {
    let mut ws = invoice();
    let text = ws.files()[CONFIG].replace("Invoice", "Bill");
    assert!(ws.files_changed(BTreeMap::from([(CONFIG.to_string(), Some(text))])).workspace);
    assert_eq!(ws.config.name, "Bill");
    let text = format!("{}\nsystems = [\"plan\"]\n", ws.files()[CONFIG].lines().filter(|l| !l.starts_with("systems")).collect::<Vec<_>>().join("\n"));
    assert!(ws.files_changed(BTreeMap::from([(CONFIG.to_string(), Some(text))])).reopen);
}

#[test]
fn an_unknown_method_or_bad_arguments_say_so() {
    let mut ws = invoice();
    assert!(matches!(dispatch(&mut ws, "nonsense", json!({}), "t"), Err(Error::NotFound(_))));
    assert!(matches!(dispatch(&mut ws, "apply", json!({ "ops": 3 }), "t"), Err(Error::Refused(_))));
    assert!(matches!(dispatch(&mut ws, "sweep", json!({ "set": { "node": recollect_graph::Entity::new(), "cell": "value" }, "values": vec![0.0; 201], "read": [] }), "t"), Err(Error::Refused(_))));
}

#[test]
fn a_workspace_from_when_claude_had_to_propose_still_opens() {
    let config = read_config("name = \"Old\"\nsystems = [\"plan\"]\nllm_direct_writes = false\n").unwrap();
    assert_eq!(config.name, "Old");
    assert_eq!(config.systems, vec!["plan".to_string()]);
}

fn var<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["worlds"]["vars"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap_or_else(|| panic!("no {name}"))
}

fn option<'a>(var: &'a Value, name: &str) -> &'a Value {
    var["options"].as_array().unwrap().iter().find(|o| o["name"] == name).unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn an_option_ruled_out_only_for_looking_is_left_out_of_every_world_and_saved_nowhere() {
    let mut ws = open(sample_files("monty-hall").unwrap());
    ws.take_writes();
    let v = view(&mut ws);
    let car = var(&v, "Where's the car?");
    let (car_id, door2) = (car["id"].as_str().unwrap().to_string(), option(car, "Door[2]")["id"].as_str().unwrap().to_string());
    let stick = var(&v, "Stick or switch?");
    let (stick_id, sticking) = (stick["id"].as_str().unwrap().to_string(), option(stick, "Stick")["id"].as_str().unwrap().to_string());
    let all: u64 = v["worlds"]["count"].as_str().unwrap().parse().unwrap();
    let checks = v["checks"].clone();

    // Something uncertain: it's known not to be there, so the other two are believed more.
    let world = json!({ format!("!{car_id}/{door2}"): "out" });
    let v = dispatch(&mut ws, "view", json!({ "world": world }), "t").unwrap().0;
    let car = var(&v, "Where's the car?");
    assert_eq!(option(car, "Door[2]")["chance"], 0.0);
    assert!((option(car, "Door[1]")["chance"].as_f64().unwrap() - 0.5).abs() < 1e-9, "{car}");
    assert!(v["worlds"]["count"].as_str().unwrap().parse::<u64>().unwrap() < all);
    assert_eq!(v["checks"], checks, "what rules it out is read without complaint");
    assert_eq!(v["worlds"]["constrained"], false, "looking isn't a puzzle's requirement");
    let winning = |v: &Value| v["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "Chance of winning").unwrap()["cells"]["value"].clone();
    let before = winning(&dispatch(&mut ws, "view", json!({}), "t").unwrap().0);
    assert_ne!(winning(&v), before, "and what's worked out from chances takes it in: {before}");

    // A choice: the worlds where it's taken are only left out of what's looked at.
    let world = json!({ format!("!{stick_id}/{sticking}"): "out" });
    let v = dispatch(&mut ws, "view", json!({ "world": world }), "t").unwrap().0;
    assert_eq!(v["worlds"]["count"].as_str().unwrap().parse::<u64>().unwrap() * 2, all);
    assert!(!option(var(&v, "Stick or switch?"), "Stick")["possible"].as_bool().unwrap());
    assert!(v["worlds"]["picks"].as_object().unwrap().is_empty(), "nothing is pinned");
    assert!(!v["nodes"].as_array().unwrap().iter().any(|n| n["name"].is_null() && n["components"].as_array().unwrap().iter().any(|c| c["name"] == "found")), "what does it is never shown");

    assert!(ws.take_writes().is_empty(), "and nothing is written");
}
