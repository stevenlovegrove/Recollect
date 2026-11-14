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
fn proposals_are_files_and_land_in_the_inbox_from_either_side() {
    let mut ws = invoice();
    let rate = id(&view(&mut ws), "Tax rate");
    let (p, effect) = dispatch(&mut ws, "propose", json!({ "title": "Higher tax", "rationale": "because", "ops": [{ "op": "set", "id": rate, "cell": "value", "value": 0.3 }] }), "claude").unwrap();
    assert!(effect.proposals && !effect.graph);
    let path = format!("proposals/{}.json", p["id"].as_str().unwrap());
    let w = ws.take_writes();
    assert!(w.write.contains_key(&path));

    // Written by someone else, it's picked up.
    let mut other = invoice();
    let changed = other.files_changed(BTreeMap::from([(path.clone(), Some(w.write[&path].clone()))]));
    assert!(changed.proposals);
    assert_eq!(dispatch(&mut other, "proposals", json!({}), "t").unwrap().0.as_array().unwrap().len(), 1);

    dispatch(&mut ws, "accept", json!({ "id": p["id"] }), "t").unwrap();
    let w = ws.take_writes();
    assert!(w.delete.contains(&path));
    assert_eq!(value(&view(&mut ws), "Tax"), json!(24.0));
}

#[test]
fn a_new_workspace_is_just_its_settings() {
    let files = new_files("Mine", vec!["plan".into()]).unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), vec![CONFIG]);
    let mut ws = open(files);
    assert_eq!(view(&mut ws)["workspace"]["name"], "Mine");
    assert!(new_files("Bad", vec!["nonsense".into()]).is_err());
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
