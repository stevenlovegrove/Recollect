//! The tools an agent uses, as it uses them: five of them, answering in YAML unless asked for JSON.

use serde_json::{Value, json};

use super::*;

fn server(sample: &str) -> (RecollectMcp, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let app = App::new(home.path().to_path_buf());
    app.open_sample(sample, true).unwrap();
    (RecollectMcp::new(app), home)
}

fn yaml(text: &str) -> Value {
    serde_yaml_ng::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Parameters<T> {
    Parameters(serde_json::from_value(v).unwrap())
}

#[tokio::test]
async fn there_are_five_tools_and_nothing_overlaps() {
    let (mcp, _home) = server("handbook");
    let mut names: Vec<String> = mcp.tool_router.list_all().into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, ["apply", "changes", "checks", "overview", "read"]);
}

#[tokio::test]
async fn the_overview_has_a_guide_for_each_system_the_workspace_uses() {
    let (mcp, _home) = server("handbook");
    let o = yaml(&mcp.overview(params(json!({}))).await.unwrap());
    let systems: Vec<&str> = o["systems"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(systems.contains(&"plan"), "{systems:?}");
    let plan = o["systems"].as_array().unwrap().iter().find(|s| s["name"] == "plan").unwrap();
    assert!(plan["guide"].as_str().unwrap().contains("part_of"), "{plan}");

    let (mcp, _home) = server("invoice");
    let o = yaml(&mcp.overview(params(json!({}))).await.unwrap());
    let available: Vec<&str> = o["available"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(available.contains(&"plan"), "a system not used yet is listed: {available:?}");
    let o = yaml(&mcp.overview(params(json!({ "also": ["plan"] }))).await.unwrap());
    assert!(o["systems"].as_array().unwrap().iter().any(|s| s["name"] == "plan"), "and its guide can be asked for");
}

#[tokio::test]
async fn a_read_answers_in_yaml_or_json_and_they_say_the_same() {
    let (mcp, _home) = server("handbook");
    let spec = json!({ "nodes": { "component": "decision" }, "worked_out": true });
    let as_yaml = yaml(&mcp.read(params(spec.clone())).await.unwrap());
    let mut json_spec = spec;
    json_spec["format"] = json!("json");
    let as_json: Value = serde_json::from_str(&mcp.read(params(json_spec)).await.unwrap()).unwrap();
    assert_eq!(as_yaml, as_json);
    assert_eq!(as_yaml["nodes"][0]["cells"]["name"], "How do we publish it?");
}

#[tokio::test]
async fn a_read_takes_pins_and_a_where_by_name() {
    let (mcp, _home) = server("handbook");
    let all = yaml(&mcp.read(params(json!({ "plan": true }))).await.unwrap());
    let decision = &all["plan"]["decisions"][0];
    let option = decision["options"].as_array().unwrap().iter().find(|o| !o["gap"].as_bool().unwrap()).unwrap()["name"].as_str().unwrap().to_string();
    let pinned = yaml(&mcp.read(params(json!({ "plan": true, "world": { "How do we publish it?": option } }))).await.unwrap());
    assert_ne!(pinned["plan"], all["plan"], "pinned by names, it reads differently");

    let w = json!({ "formula": "chose(d, o)", "labels": { "d": "How do we publish it?", "o": option } });
    let looked = yaml(&mcp.read(params(json!({ "nodes": { "ids": ["How do we publish it?"] }, "where": w }))).await.unwrap());
    assert!(looked["looking"]["kept"].as_f64().unwrap() < 1.0, "{looked}");

    let wrong = mcp.read(params(json!({ "world": { "Nothing": "x" } }))).await;
    assert!(wrong.unwrap_err().contains("isn't an open choice"));
}

#[tokio::test]
async fn changes_since_a_seq_are_what_the_user_did_in_between() {
    let (mcp, _home) = server("handbook");
    mcp.apply(params(json!({ "why": "a start", "ops": [{ "op": "add_node", "id": "new:s", "name": "Start" }] }))).await.unwrap();
    // The overview says where things stand, so there's always somewhere to look from.
    let seq = yaml(&mcp.overview(params(json!({}))).await.unwrap())["last_seq"].as_u64().unwrap();
    let ops = json!([{ "op": "add_node", "id": "new:x", "name": "A thought" }]);
    mcp.app.call("apply", json!({ "ops": ops, "why": "the user typed it" }), "user").unwrap();
    let after = yaml(&mcp.changes(params(json!({ "since": seq }))).await.unwrap());
    let events = after.as_array().unwrap();
    assert_eq!(events.len(), 1, "{after:?}");
    assert_eq!(events[0]["author"], "user");
    assert!(events[0]["did"].as_array().unwrap().iter().any(|d| d["node_name"] == "A thought"), "{after:?}");
}

#[tokio::test]
async fn apply_and_checks_work_as_before() {
    let (mcp, _home) = server("handbook");
    let done = yaml(&mcp.apply(params(json!({ "why": "an idea", "ops": [{ "op": "add_node", "id": "new:x", "name": "Idea" }] }))).await.unwrap());
    assert!(done["created"]["new:x"].is_string() || done.to_string().contains("new:x"), "{done}");
    let refused = mcp.apply(params(json!({ "why": "", "ops": [{ "op": "add_node", "id": "new:c", "name": "Header", "components": ["concept"] }] }))).await;
    assert!(refused.unwrap_err().contains("“Header”"), "a refusal names the node");
    let checks = yaml(&mcp.checks(params(json!({}))).await.unwrap());
    assert!(checks.is_array());
}
