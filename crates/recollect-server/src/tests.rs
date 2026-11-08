//! The API from outside, on working copies of the samples.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use recollect_systems::samples::SAMPLES;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::app::App;
use crate::{api, source_tree};

struct Client {
    router: Router,
    app: Arc<App>,
    _home: tempfile::TempDir,
}

impl Client {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(home.path().to_path_buf(), source_tree("examples"));
        Client { router: api::router(app.clone()), app, _home: home }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path).header("content-type", "application/json");
        let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_string()))).unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let (status, v) = self.call("POST", path, Some(body)).await;
        assert!(status.is_success(), "{path}: {status} {v}");
        v
    }

    async fn open(&self, sample: &str) {
        self.post("/api/workspaces/open", json!({ "sample": sample })).await;
    }

    async fn view(&self, world: Value) -> Value {
        self.post("/api/view", json!({ "world": world })).await
    }
}

fn node<'a>(view: &'a Value, name: &str) -> &'a Value {
    view["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("no node called {name}"))
}

fn id(view: &Value, name: &str) -> String {
    node(view, name)["id"].as_str().unwrap().to_string()
}

fn cell<'a>(view: &'a Value, name: &str, component: &str) -> &'a Value {
    &node(view, name)["cells"][component]
}

fn when(view: &Value, name: &str, component: &str) -> String {
    cell(view, name, component)["shown"]["when"].as_str().unwrap_or_else(|| panic!("{name} {component}: {}", cell(view, name, component))).to_string()
}

#[tokio::test]
async fn with_nothing_open_it_says_so_and_offers_the_samples() {
    let c = Client::new();
    let (status, _) = c.call("POST", "/api/view", Some(json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, list) = c.call("GET", "/api/workspaces", None).await;
    assert_eq!(list["samples"].as_array().unwrap().len(), SAMPLES.len());
    assert_eq!(list["current"], Value::Null);
}

#[tokio::test]
async fn every_sample_opens_with_nothing_wrong_and_no_error_in_any_cell() {
    let c = Client::new();
    for s in SAMPLES {
        c.open(s.name).await;
        let v = c.view(json!({})).await;
        assert_eq!(v["workspace"]["name"], s.title);
        assert_eq!(v["workspace"]["sample"], s.name);
        let broken: Vec<&Value> = v["checks"].as_array().unwrap().iter().filter(|c| c["severity"] == "broken").collect();
        assert!(broken.is_empty(), "{}: {broken:?}", s.name);
        for n in v["nodes"].as_array().unwrap() {
            for (component, cell) in n["cells"].as_object().unwrap() {
                assert!(cell.get("error").is_none(), "{}: {component} of {}: {cell}", s.name, n["name"]);
            }
        }
    }
}

#[tokio::test]
async fn opening_a_sample_never_touches_the_original() {
    let c = Client::new();
    c.open("invoice").await;
    let before = std::fs::read_to_string(source_tree("examples/invoice/log.jsonl")).unwrap();
    let v = c.view(json!({})).await;
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": id(&v, "Tax rate"), "component": "value", "value": 0.5 }] })).await;
    assert_eq!(std::fs::read_to_string(source_tree("examples/invoice/log.jsonl")).unwrap(), before);
    assert_eq!(cell(&c.view(json!({})).await, "Tax", "value")["value"], 40.0);

    // And it can be started again.
    c.post("/api/workspaces/open", json!({ "sample": "invoice", "reset": true })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Tax", "value")["value"], 16.0);
}

#[tokio::test]
async fn an_edit_says_what_it_reran() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Total", "value")["value"], 96.0);
    assert_eq!(cell(&v, "Total", "value")["rule"], "expr@1");
    assert_eq!(cell(&v, "Total", "value")["formula"], "subtotal + tax");
    assert_eq!(cell(&v, "Tax rate", "value")["source"], "stored");

    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": "Widgets: price", "component": "value", "value": 11.0 }], "why": "dearer" })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Total", "value")["value"], 99.6);
    let ran: Vec<String> = v["ran"].as_array().unwrap().iter().map(|r| r["node"].as_str().unwrap().to_string()).collect();
    assert_eq!(ran.len(), 4, "{ran:?}");
    assert!(!ran.contains(&id(&v, "Gears")));
    assert!(c.view(json!({})).await["ran"].as_array().unwrap().is_empty(), "asking again runs nothing");
}

#[tokio::test]
async fn a_circular_reference_is_an_error_in_those_cells_and_heals() {
    let c = Client::new();
    c.open("invoice").await;
    c.post("/api/ops", json!({ "ops": [{ "op": "link", "from": "Subtotal", "to": "Total", "kind": "label", "value": "line" }] })).await;
    let v = c.view(json!({})).await;
    assert!(cell(&v, "Total", "value")["error"].as_str().unwrap().contains("depends on itself"), "{}", cell(&v, "Total", "value"));
    assert_eq!(cell(&v, "Gears", "value")["value"], 50.0);
    c.post("/api/ops", json!({ "ops": [{ "op": "link", "from": "Subtotal", "to": "Total", "kind": "label" }] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Total", "value")["value"], 96.0);
}

#[tokio::test]
async fn a_world_narrows_every_value_exactly() {
    let c = Client::new();
    c.open("calibration").await;
    let open = c.view(json!({})).await;
    assert_eq!(when(&open, "Static captures", "need"), "Which approach?∈{From the CAD model,Free-form search}");
    assert_eq!(open["worlds"]["count"], "4");
    let var = &open["worlds"]["vars"][0];
    assert_eq!(var["name"], "Which approach?");
    assert_eq!(var["options"].as_array().unwrap().len(), 4);

    let picked = json!({ id(&open, "Which approach?"): id(&open, "With a checkerboard") });
    let v = c.view(picked).await;
    assert!(cell(&v, "Static captures", "need")["shown"]["never"].as_bool().unwrap());
    assert!(cell(&v, "Board detector", "need")["shown"]["always"].as_bool().unwrap());
    assert_eq!(v["worlds"]["count"], "1");
    let cases = cell(&v, "Which approach?", "finish")["shown"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["value"], 4.0);

    // Nothing was committed by looking.
    assert_eq!(v["last_seq"], open["last_seq"]);
}

#[tokio::test]
async fn deciding_commits_and_reopening_brings_it_all_back() {
    let c = Client::new();
    c.open("calibration").await;
    let open = c.view(json!({})).await;
    let cad = id(&open, "From the CAD model");
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": "Which approach?", "component": "decision", "value": { "chosen": cad } }], "why": "it's accurate enough" })).await;
    let v = c.view(json!({})).await;
    assert!(cell(&v, "Export extrinsics from CAD", "need")["shown"]["always"].as_bool().unwrap());
    assert!(v["worlds"]["vars"].as_array().unwrap().is_empty(), "no open choices now");

    let (_, history) = c.call("GET", "/api/history", None).await;
    assert_eq!(history[0]["why"], "it's accurate enough");
    assert_eq!(history[0]["author"], "you");
    let seq = history[0]["seq"].as_u64().unwrap();
    c.post("/api/undo", json!({ "seq": seq, "why": "changed my mind" })).await;
    assert_eq!(when(&c.view(json!({})).await, "Static captures", "need"), when(&open, "Static captures", "need"));
}

#[tokio::test]
async fn a_change_that_breaks_a_rule_is_refused_and_says_why() {
    let c = Client::new();
    c.open("calibration").await;
    let v = c.view(json!({})).await;
    let imu = id(&v, "Fit the IMU time offset");
    let (status, body) = c.call("POST", "/api/ops", Some(json!({ "ops": [{ "op": "set", "id": "Which approach?", "component": "decision", "value": { "chosen": imu } }] }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("chosen_must_be_a_live_option"), "{body}");
    assert_eq!(c.view(json!({})).await["last_seq"], v["last_seq"], "and nothing changed");
}

#[tokio::test]
async fn adding_a_node_and_linking_it_is_one_event_and_says_what_it_made() {
    let c = Client::new();
    c.open("calibration").await;
    let before = c.view(json!({})).await;
    let done = c
        .post("/api/ops", json!({ "ops": [
            { "op": "add_node", "id": "new:a", "name": "Held-out scenes", "components": { "task": {}, "estimate": 2.0 }, "links": [{ "to": "Which approach?", "kind": "part_of" }] },
            { "op": "add_node", "id": "new:b", "name": "Check them", "components": { "task": {} }, "links": [{ "to": "new:a", "kind": "depends_on", "value": {} }] },
        ], "why": "we'll need these whichever way it goes" }))
        .await;
    let a = done["created"]["new:a"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(id(&v, "Held-out scenes"), a);
    assert_eq!(v["last_seq"].as_u64().unwrap(), before["last_seq"].as_u64().unwrap() + 1);
    assert!(cell(&v, "Held-out scenes", "need")["shown"]["always"].as_bool().unwrap());
    assert_eq!(cell(&v, "Check them", "blockers")["shown"]["items"][0]["node"], a.as_str());

    c.post("/api/ops", json!({ "ops": [{ "op": "delete_node", "id": "Check them" }] })).await;
    assert!(c.view(json!({})).await["nodes"].as_array().unwrap().iter().all(|n| n["name"] != "Check them"));
}

#[tokio::test]
async fn pinning_a_puzzle_narrows_what_is_possible() {
    let c = Client::new();
    c.open("australia").await;
    let v = c.view(json!({})).await;
    assert_eq!(v["worlds"]["count"], "18");
    assert_eq!(v["worlds"]["solutions"].as_array().unwrap().len(), 18);
    assert!(v["worlds"]["constrained"].as_bool().unwrap());

    let picked = json!({ id(&v, "South Australia"): "red", id(&v, "Western Australia"): "green" });
    let v = c.view(picked).await;
    assert_eq!(v["worlds"]["count"], "3", "the rest of the mainland is forced, and Tasmania is free");
    let var = |name: &str| v["worlds"]["vars"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap().clone();
    let possible = |name: &str| -> Vec<String> { var(name)["options"].as_array().unwrap().iter().filter(|o| o["possible"].as_bool().unwrap()).map(|o| o["id"].as_str().unwrap().to_string()).collect() };
    assert_eq!(possible("Northern Territory"), vec!["blue"]);
    assert_eq!(possible("Queensland"), vec!["green"]);
    assert_eq!(possible("Tasmania").len(), 3);

    // An impossible pin leaves no worlds, and says so.
    let v0 = c.view(json!({})).await;
    let none = c.view(json!({ id(&v0, "South Australia"): "red", id(&v0, "Northern Territory"): "red" })).await;
    assert_eq!(none["worlds"]["count"], "0");
}

#[tokio::test]
async fn the_zebra_puzzle_comes_back_solved() {
    let c = Client::new();
    c.open("zebra").await;
    let v = c.view(json!({})).await;
    assert_eq!(v["worlds"]["count"], "1");
    let s = &v["worlds"]["solutions"][0];
    assert_eq!(s[id(&v, "Zebra")], s[id(&v, "The Japanese")]);
    assert_eq!(s[id(&v, "Water")], s[id(&v, "The Norwegian")]);
}

#[tokio::test]
async fn the_plan_answers_where_work_stands_and_what_decisions_need() {
    let c = Client::new();
    c.open("calibration").await;
    let p = c.post("/api/plan", json!({})).await;
    let standing = |p: &Value, name: &str| p["work"].as_array().unwrap().iter().find(|w| w["name"] == name).unwrap()["standing"].clone();
    assert_eq!(standing(&p, "Fit the IMU time offset"), "now");
    assert_eq!(standing(&p, "Static captures"), "depends", "two of the three named options need it");
    let d = &p["decisions"][0];
    assert_eq!(d["name"], "Which approach?");
    assert_eq!(d["pressure"]["state"], "free");
    assert_eq!(d["options"].as_array().unwrap().iter().filter(|o| o["gap"].as_bool().unwrap()).count(), 1);
}

#[tokio::test]
async fn a_sheet_workspace_has_no_plan_to_show() {
    let c = Client::new();
    c.open("invoice").await;
    let p = c.post("/api/plan", json!({})).await;
    assert!(p["work"].as_array().unwrap().is_empty() && p["decisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn proposals_wait_for_review_and_can_be_taken_in_part() {
    let c = Client::new();
    c.open("calibration").await;
    let before = c.view(json!({})).await;
    let p = c.app.propose("Add two".to_string(), "both are needed".to_string(), vec![
        json!({ "op": "add_node", "id": "new:a", "name": "One" }),
        json!({ "op": "add_node", "id": "new:b", "name": "Two" }),
    ], "claude").unwrap();
    assert_eq!(c.view(json!({})).await["last_seq"], before["last_seq"], "nothing has happened yet");

    let (_, list) = c.call("GET", "/api/proposals", None).await;
    assert_eq!(list[0]["id"], p.id.as_str());
    assert_eq!(list[0]["error"], Value::Null);
    assert_eq!(list[0]["ops"].as_array().unwrap().len(), 2);

    // Only the first.
    c.post(&format!("/api/proposals/{}/accept", p.id), json!({ "ops": [0] })).await;
    let v = c.view(json!({})).await;
    assert!(v["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "One"));
    assert!(!v["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "Two"));
    assert_eq!(c.call("GET", "/api/proposals", None).await.1.as_array().unwrap().len(), 0);
    let (_, history) = c.call("GET", "/api/history", None).await;
    assert_eq!(history[0]["author"], "claude", "the log says who proposed it");
    assert!(history[0]["why"].as_str().unwrap().contains("both are needed"));

    // One that wouldn't apply is refused before it gets to anyone.
    let v = c.view(json!({})).await;
    let refused = c.app.propose("Bad".to_string(), String::new(), vec![json!({ "op": "set", "id": "Which approach?", "component": "decision", "value": { "chosen": id(&v, "One") } })], "claude");
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_hand_edit_on_disk_is_recorded_as_an_event() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let dir = v["workspace"]["dir"].as_str().unwrap().to_string();
    let rate = id(&v, "Tax rate");
    let path = format!("{dir}/nodes/{rate}.yaml");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("value: 0.2"), "{text}");
    std::fs::write(&path, text.replace("value: 0.2", "value: 0.5")).unwrap();

    let mut tries = 0;
    loop {
        let v = c.view(json!({})).await;
        if cell(&v, "Tax", "value")["value"] == 40.0 {
            break;
        }
        tries += 1;
        assert!(tries < 100, "the edit never arrived");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, history) = c.call("GET", "/api/history", None).await;
    assert_eq!((history[0]["author"].as_str(), history[0]["why"].as_str()), (Some("disk"), Some("external edit")));
}

#[tokio::test]
async fn going_back_and_changing_something_starts_a_branch_and_loses_nothing() {
    let c = Client::new();
    c.open("invoice").await;
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": "Tax rate", "component": "value", "value": 0.5 }], "why": "raised" })).await;
    let dir = c.view(json!({})).await["workspace"]["dir"].as_str().unwrap().to_string();
    let rate = id(&c.view(json!({})).await, "Tax rate");
    let file = format!("{dir}/nodes/{rate}.yaml");
    assert!(std::fs::read_to_string(&file).unwrap().contains("value: 0.5"));

    // Going back is the whole app being there, including the files.
    c.post("/api/checkout", json!({ "seq": 0 })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Tax", "value")["value"], 16.0);
    assert_eq!((v["last_seq"].as_u64(), v["latest"].as_u64()), (Some(0), Some(1)), "and it says there's something to return to");
    assert!(std::fs::read_to_string(&file).unwrap().contains("value: 0.2"));

    // A change from here is a fork.
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": "Tax rate", "component": "value", "value": 0.1 }], "why": "cut instead" })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Tax", "value")["value"], 8.0);
    assert_eq!(v["latest"], Value::Null, "this is the end of a branch");
    let (_, history) = c.call("GET", "/api/history", None).await;
    let events = history.as_array().unwrap();
    assert_eq!(events.len(), 3, "nothing is lost");
    let parents: Vec<(u64, Value)> = events.iter().map(|e| (e["seq"].as_u64().unwrap(), e["parent"].clone())).collect();
    assert_eq!(parents, vec![(2, json!(0)), (1, json!(0)), (0, Value::Null)]);
    assert_eq!(events.iter().map(|e| e["here"].as_bool().unwrap()).collect::<Vec<_>>(), vec![true, false, true]);

    // A node's own history is only what led here.
    let (_, own) = c.call("GET", &format!("/api/history?node={rate}"), None).await;
    assert_eq!(own.as_array().unwrap().iter().map(|e| e["why"].as_str().unwrap().to_string()).filter(|w| w == "raised").count(), 0);

    // What isn't on this branch can't be undone from here.
    let (status, body) = c.call("POST", "/api/undo", Some(json!({ "seq": 1 }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("isn't on this branch"), "{body}");

    // And the other branch is still there to go back to, through a reopen too.
    c.post("/api/checkout", json!({ "seq": 1 })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Tax", "value")["value"], 40.0);
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Tax", "value")["value"], 40.0);
    assert_eq!(v["last_seq"], 1);

    let (status, _) = c.call("POST", "/api/checkout", Some(json!({ "seq": 99 }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_worked_out_cell_can_be_typed_over_and_comes_back() {
    let c = Client::new();
    c.open("calibration").await;
    let done = c.post("/api/ops", json!({ "ops": [{ "op": "add_node", "id": "new:t", "name": "Mine", "components": { "task": {} } }] })).await;
    let t = done["created"]["new:t"].as_str().unwrap().to_string();
    let cell = |v: &Value| v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == t.as_str()).unwrap()["cells"]["done"].clone();

    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["own"], shown["value"].is_null()), (&json!(false), false), "a task says it's owed before anyone has said anything: {shown}");
    assert_eq!(shown["value"], json!(false));
    assert!(shown["formula"].as_str().unwrap().contains("is_leaf"), "and what it's worked out by: {shown}");

    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": t, "component": "done", "value": true }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["own"], &shown["value"]), (&json!(true), &json!(true)));

    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": t, "component": "done", "formula": "1 < 2" }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["own"], &shown["formula"], &shown["value"]), (&json!(true), &json!("1 < 2"), &json!(true)));

    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": t, "component": "done" }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["own"], &shown["value"]), (&json!(false), &json!(false)), "and without it, it's worked out again");
}

#[tokio::test]
async fn a_new_workspace_starts_empty_and_an_old_one_is_not_touched() {
    let c = Client::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mine");
    c.post("/api/workspaces/new", json!({ "path": path, "name": "Mine", "systems": ["plan"] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["workspace"]["name"], "Mine");
    assert!(v["nodes"].as_array().unwrap().is_empty());
    assert_eq!(v["worlds"]["count"], "1");

    let old = dir.path().join("old");
    std::fs::create_dir_all(old.join("nodes")).unwrap();
    std::fs::write(old.join("recollect.toml"), "name = \"Old\"\n").unwrap();
    std::fs::write(old.join("nodes/a.md"), "---\nid: a\n---\n").unwrap();
    let (status, body) = c.call("POST", "/api/workspaces/open", Some(json!({ "path": old }))).await;
    assert!(!status.is_success());
    assert!(body["error"].as_str().unwrap().contains("older version"), "{body}");
    assert!(!old.join("log.jsonl").exists(), "and nothing was written there");
}

fn paths(list: &Value) -> Vec<&str> {
    list.as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn the_files_can_be_read_and_show_what_the_app_is_working_from() {
    let c = Client::new();
    c.open("calibration").await;
    let (_, list) = c.call("GET", "/api/files", None).await;
    let paths = paths(&list);
    assert!(paths.contains(&"recollect.toml") && paths.contains(&"log.jsonl"), "{paths:?}");
    assert!(!paths.contains(&"log.lock"), "{paths:?}");

    let v = c.view(json!({})).await;
    let path = format!("nodes/{}.yaml", id(&v, "Which approach?"));
    assert!(paths.contains(&path.as_str()), "{paths:?}");
    let (status, file) = c.call("GET", &format!("/api/file?path={path}"), None).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert!(file["text"].as_str().unwrap().contains("Which approach?"), "{file}");
    assert_eq!(file["truncated"], false);

    // Reading is all it does, and a change shows up.
    let before = c.call("GET", "/api/file?path=log.jsonl", None).await.1;
    assert_eq!(c.call("GET", "/api/file?path=log.jsonl", None).await.1, before);
    c.post("/api/ops", json!({ "ops": [{ "op": "add_node", "id": "new:x", "name": "Fresh" }] })).await;
    let after = c.call("GET", "/api/file?path=log.jsonl", None).await.1;
    assert!(after["size"].as_u64() > before["size"].as_u64());
    assert!(after["text"].as_str().unwrap().contains("Fresh"));
}

#[tokio::test]
async fn only_the_workspaces_own_files_can_be_read() {
    let c = Client::new();
    c.open("invoice").await;
    let dir = std::path::PathBuf::from(c.call("GET", "/api/workspaces", None).await.1["current"]["dir"].as_str().unwrap());
    let secret = c._home.path().join("secret.txt");
    std::fs::write(&secret, "private").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("linked.txt")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/config"), "private").unwrap();

    let (_, list) = c.call("GET", "/api/files", None).await;
    let listed = paths(&list);
    assert!(!listed.iter().any(|p| p.contains("linked") || p.contains(".git/")), "{listed:?}");

    let absolute = secret.display().to_string();
    for path in ["../../secret.txt", "nodes/../../../secret.txt", "linked.txt", ".git/config", "log.lock", "nope.txt", absolute.as_str()] {
        let (status, body) = c.call("GET", &format!("/api/file?path={}", path.replace('/', "%2F")), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
}

#[tokio::test]
async fn with_nothing_open_there_are_no_files() {
    let c = Client::new();
    assert_eq!(c.call("GET", "/api/files", None).await.0, StatusCode::CONFLICT);
    assert_eq!(c.call("GET", "/api/file?path=recollect.toml", None).await.0, StatusCode::CONFLICT);
}

async fn refused(c: &Client, ops: Value) -> String {
    let (status, body) = c.call("POST", "/api/ops", Some(json!({ "ops": ops }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    body["error"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_formula_and_its_labels_are_one_change_and_a_bad_one_is_refused() {
    let c = Client::new();
    c.open("invoice").await;
    let before = c.view(json!({})).await;

    let done = c.post("/api/ops", json!({ "ops": [{ "op": "add_node", "id": "new:x", "name": "Doubled", "formula": "total * 2", "labels": { "total": "Total" } }] })).await;
    let x = done["created"]["new:x"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Doubled", "value")["value"], 192.0);
    assert_eq!(v["last_seq"].as_u64().unwrap(), before["last_seq"].as_u64().unwrap() + 1, "one event");

    // Changing it says what its names now mean, and a name it no longer has goes.
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": x, "component": "value", "formula": "tax + 1", "labels": { "tax": "Tax" } }] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Doubled", "value")["value"], 17.0);
    let labels: Vec<&Value> = v["links"].as_array().unwrap().iter().filter(|l| l["from"] == x.as_str()).collect();
    assert_eq!(labels.len(), 1, "{labels:?}");
    assert_eq!(labels[0]["value"], "tax");

    // Several nodes under one name are a group.
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": x, "component": "value", "formula": "sum(n)", "labels": { "n": ["Tax", "Total"] } }] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Doubled", "value")["value"], 112.0);

    // What's wrong with one is said before anything lands.
    let seq = c.view(json!({})).await["last_seq"].clone();
    let why = refused(&c, json!([{ "op": "set", "id": x, "component": "value", "formula": "nope + 1" }])).await;
    assert!(why.contains("no input called nope"), "{why}");
    let why = refused(&c, json!([{ "op": "set", "id": x, "component": "value", "formula": "1 +" }])).await;
    assert!(why.contains("stops short"), "{why}");
    let why = refused(&c, json!([{ "op": "set", "id": x, "component": "value", "formula": "\"a\" + 1" }])).await;
    assert!(why.contains("+ needs numbers"), "{why}");
    let why = refused(&c, json!([{ "op": "link", "from": x, "to": "Tax", "kind": "label", "value": "two words" }])).await;
    assert!(why.contains("a label has to be a name"), "{why}");
    assert_eq!(c.view(json!({})).await["last_seq"], seq, "and nothing changed");

    // A proposal is held to the same, before anyone is asked to look at it.
    assert!(c.app.propose("Bad".to_string(), String::new(), vec![json!({ "op": "add_node", "id": "new:y", "name": "Y", "formula": "nope" })], "claude").is_err());
}

#[tokio::test]
async fn a_hand_edit_to_a_formula_stands_and_its_cell_says_whats_wrong() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let dir = v["workspace"]["dir"].as_str().unwrap().to_string();
    let path = format!("{dir}/nodes/{}.yaml", id(&v, "Total"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("subtotal + tax"), "{text}");
    std::fs::write(&path, text.replace("subtotal + tax", "subtotal + nope")).unwrap();
    let mut tries = 0;
    loop {
        let v = c.view(json!({})).await;
        if let Some(why) = cell(&v, "Total", "value")["error"].as_str() {
            assert!(why.contains("no input called nope"), "{why}");
            assert_eq!(cell(&v, "Tax", "value")["value"], 16.0, "and other cells are fine");
            break;
        }
        tries += 1;
        assert!(tries < 100, "the edit never arrived");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_working_copy_from_before_samples_changed_is_put_aside_not_misread() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let dir = std::path::PathBuf::from(v["workspace"]["dir"].as_str().unwrap());
    std::fs::remove_file(dir.join("sample-format")).unwrap();
    std::fs::write(dir.join("recollect.toml"), "name = \"Old\"\nsystems = [\"sheet\"]\n").unwrap();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    assert_eq!(v["workspace"]["name"], "Invoice");
    assert_eq!(cell(&v, "Total", "value")["value"], 96.0);
    let aside: Vec<_> = std::fs::read_dir(c._home.path().join("samples-superseded")).unwrap().collect();
    assert_eq!(aside.len(), 1, "and it's still there");
}

// ---------- problems nobody wrote the rules for, solved with data alone, the way Claude would ----------

/// A budget cap on a plan whose estimates depend on a decision.
#[tokio::test]
async fn held_out_a_budget_cap_on_a_plan() {
    let c = Client::new();
    c.open("calibration").await;
    let v = c.view(json!({})).await;
    let (approach, cad, free, board) = (id(&v, "Which approach?"), id(&v, "From the CAD model"), id(&v, "Free-form search"), id(&v, "With a checkerboard"));

    // Free-form needs a second pass of captures, so what's shared costs more on that path.
    let p = c.app.propose("Cap the effort at six days".to_string(), "the review is in a week".to_string(), vec![
        json!({ "op": "set", "id": "Static captures", "component": "estimate", "formula": "if(chose(approach, free), 3, 2)", "labels": { "approach": approach, "free": free } }),
        json!({ "op": "add_node", "id": "new:effort", "name": "Effort", "formula": "effort()" }),
        json!({ "op": "add_node", "id": "new:cap", "name": "Within six days", "formula": "effort <= 6", "labels": { "effort": "new:effort" }, "components": { "requires": null } }),
    ], "claude").unwrap();
    let (_, list) = c.call("GET", "/api/proposals", None).await;
    assert!(list[0]["ops"].as_array().unwrap().iter().any(|o| o["value"] == "effort <= 6"), "the formula is what gets reviewed: {list}");
    c.post(&format!("/api/proposals/{}/accept", p.id), json!({})).await;

    let v = c.view(json!({})).await;
    let at = |o: &str| json!({ approach.as_str(): o });
    assert!(cell(&v, "Effort", "value")["error"].is_null(), "{}", cell(&v, "Effort", "value"));
    let read = |v: &Value| cell(v, "Effort", "value")["value"].clone();
    assert_eq!(read(&c.view(at(&cad)).await), 6.0);
    assert_eq!(read(&c.view(at(&free)).await), 9.0, "1 + 3 + 5, with the estimate that depends on the decision");
    assert_eq!(read(&c.view(at(&board)).await), 5.0);

    let cases = cell(&v, "Within six days", "value")["shown"]["cases"].as_array().unwrap().clone();
    let case = |value: Value| cases.iter().find(|c| c["value"] == value).unwrap()["when"]["when"].as_str().unwrap().to_string();
    assert_eq!(case(json!(false)), "Which approach?=Free-form search");
    assert_eq!(case(Value::Null), "Which approach?=Something else", "it says what isn't known, and where");
    assert_eq!(v["worlds"]["constrained"], true);
    let options = v["worlds"]["vars"][0]["options"].as_array().unwrap();
    let possible: Vec<&str> = options.iter().filter(|o| o["possible"].as_bool().unwrap()).map(|o| o["name"].as_str().unwrap()).collect();
    assert_eq!(possible, vec!["From the CAD model", "With a checkerboard"], "the cap rules out the others, with a reason you can read");

    // And the dates follow the estimate, in each world.
    let free_view = c.view(at(&free)).await;
    assert_eq!(cell(&free_view, "Static captures", "finish")["value"], 3.0);
    let cad_view = c.view(at(&cad)).await;
    assert_eq!(cell(&cad_view, "Static captures", "finish")["value"], 2.0);
}

/// A puzzle clause that isn't about a pair: at most one of three people is lying.
#[tokio::test]
async fn held_out_a_clause_that_isnt_a_pair() {
    let c = Client::new();
    let dir = tempfile::tempdir().unwrap();
    c.post("/api/workspaces/new", json!({ "path": dir.path().join("logic"), "name": "Logic", "systems": [] })).await;
    let person = |id: &str, name: &str| json!({ "op": "add_node", "id": id, "name": name, "components": { "domain": ["truth", "lie"] } });
    c.post("/api/ops", json!({ "ops": [
        person("new:a", "Alice"), person("new:b", "Bob"), person("new:c", "Carol"),
        { "op": "add_node", "id": "new:most", "name": "At most one lies", "formula": "count(x in people: x = \"lie\") <= 1", "labels": { "people": ["new:a", "new:b", "new:c"] }, "components": { "requires": null } },
        { "op": "add_node", "id": "new:clue", "name": "Alice and Bob differ", "formula": "a != b", "labels": { "a": "new:a", "b": "new:b" }, "components": { "requires": null } },
    ] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["worlds"]["count"], "2", "either Alice or Bob lies, and Carol tells the truth");
    let carol = id(&v, "Carol");
    for s in v["worlds"]["solutions"].as_array().unwrap() {
        assert_eq!(s[carol.as_str()], "truth");
    }
}

/// A tiered price and a loan repayment, using a macro of its own.
#[tokio::test]
async fn held_out_tiers_and_a_loan_with_a_macro() {
    let c = Client::new();
    let dir = tempfile::tempdir().unwrap();
    c.post("/api/workspaces/new", json!({ "path": dir.path().join("money"), "name": "Money", "systems": [] })).await;
    c.post("/api/ops", json!({ "ops": [
        { "op": "add_node", "id": "new:tier", "name": "Tier", "components": { "macro": { "params": ["units", "from", "to", "price"], "body": "max(0, min(units, to) - from) * price" } } },
        { "op": "add_node", "id": "new:units", "name": "Units", "components": { "value": 250 } },
        { "op": "add_node", "id": "new:bill", "name": "Bill", "formula": "tier(units, 0, 100, 10) + tier(units, 100, 200, 8) + tier(units, 200, 1000, 5)", "labels": { "tier": "new:tier", "units": "new:units" } },
    ] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Bill", "value")["value"], 2050.0, "1000 + 800 + 250");

    // Monthly repayment on 10000 at 1% a month over 12 months, from the standard formula, with the
    // power written out by chaining a macro that squares.
    c.post("/api/ops", json!({ "ops": [
        { "op": "add_node", "id": "new:sq", "name": "Squared", "components": { "macro": { "params": ["x"], "body": "x * x" } } },
        { "op": "add_node", "id": "new:rate", "name": "Rate", "components": { "value": 0.01 } },
        { "op": "add_node", "id": "new:g", "name": "Growth", "formula": "sq(sq(1 + r)) * sq(sq(1 + r)) * sq(sq(1 + r))", "labels": { "sq": "new:sq", "r": "new:rate" } },
        { "op": "add_node", "id": "new:pay", "name": "Payment", "formula": "round(10000 * r * g / (g - 1), 2)", "labels": { "r": "new:rate", "g": "new:g" } },
    ] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Payment", "value")["value"], 888.49);
}

/// Choosing a car: what each costs a year with mileage you can only guess at, and which can make a trip
/// you'd like to take without stopping. What a mile costs follows from what a car runs on, so a kind says
/// how, and each car only says its mpg.
#[tokio::test]
async fn held_out_choosing_a_car() {
    let c = Client::new();
    let dir = tempfile::tempdir().unwrap();
    c.post("/api/workspaces/new", json!({ "path": dir.path().join("car"), "name": "Car", "systems": ["plan"] })).await;
    let option = |id: &str, name: &str, of: &str, value: f64, weight: f64| json!({ "op": "add_node", "id": id, "name": name, "components": { "value": value }, "links": [{ "to": of, "kind": "alternative", "value": { "weight": weight } }] });
    // A car says what it is, and its kind says what follows from that.
    let car = |id: &str, name: &str, kind: &str, cell: &str, v: f64, price: f64, insurance: f64, range: f64| {
        json!({ "op": "add_node", "id": id, "name": name, "components": { cell: v, "price": price, "insurance": insurance, "range": range },
            "links": [{ "to": "new:which", "kind": "alternative", "value": {} }, { "to": kind, "kind": "is_a" }] })
    };
    let pick = |cell: &str| json!({ "op": "set", "id": "new:which", "component": cell, "formula": format!("pick(o in in.alternative: o.{cell})") });
    let petrol = "fuel * 4.546 / self.mpg";
    let done = c.post("/api/ops", json!({ "ops": [
        { "op": "add_node", "id": "new:years", "name": "Years kept", "components": { "value": 5 } },
        { "op": "add_node", "id": "new:miles", "name": "Miles a year", "components": { "decision": {}, "uncertain": null }, "formula": "pick(in.alternative)" },
        option("new:light", "Light use", "new:miles", 6000.0, 1.0), option("new:typical", "Typical", "new:miles", 10000.0, 2.0), option("new:heavy", "Heavy", "new:miles", 16000.0, 1.0),
        { "op": "add_node", "id": "new:fuel", "name": "Fuel price", "components": { "value": 1.45 } },
        { "op": "add_node", "id": "new:power", "name": "Electricity price", "components": { "value": 0.28 } },
        { "op": "add_node", "id": "new:petrol", "name": "Petrol car", "components": { "value": 1, "task": {} }, "links": [{ "to": "new:fuel", "kind": "label", "value": "fuel" }] },
        { "op": "set", "id": "new:petrol", "component": "cost_per_mile", "formula": petrol },
        { "op": "add_node", "id": "new:electric", "name": "Electric car", "links": [{ "to": "new:power", "kind": "label", "value": "power" }] },
        { "op": "set", "id": "new:electric", "component": "cost_per_mile", "formula": "power * self.kwh_per_mile" },
        { "op": "add_node", "id": "new:which", "name": "Which car?", "components": { "decision": {} } },
        pick("price"), pick("cost_per_mile"), pick("insurance"), pick("range"),
        car("new:hatch", "Hatchback", "new:petrol", "mpg", 52.0, 18000.0, 500.0, 550.0),
        car("new:estate", "Estate", "new:petrol", "mpg", 44.0, 24000.0, 550.0, 600.0),
        car("new:ev", "Electric", "new:electric", "kwh_per_mile", 0.28, 32000.0, 600.0, 250.0),
        // Its maker gives its cost a mile, so it says that for itself rather than an mpg.
        car("new:hybrid", "Hybrid", "new:petrol", "cost_per_mile", 0.09, 26000.0, 550.0, 650.0),
        { "op": "add_node", "id": "new:cost", "name": "Yearly cost", "formula": "car.price * (1 - 0.85 ^ years) / years + miles * car.cost_per_mile + car.insurance",
            "labels": { "car": "new:which", "years": "new:years", "miles": "new:miles" } },
        { "op": "add_node", "id": "new:trip", "name": "To Edinburgh", "components": { "value": 400 } },
        { "op": "add_node", "id": "new:reaches", "name": "Gets to Edinburgh without stopping", "formula": "car.range >= trip", "labels": { "car": "new:which", "trip": "new:trip" } },
    ] })).await;
    let ids = |k: &str| done["created"][k].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    for n in v["nodes"].as_array().unwrap() {
        for (k, cell) in n["cells"].as_object().unwrap() {
            assert!(cell["error"].is_null(), "{} {k}: {cell}", n["name"]);
        }
    }

    // Each car's cost a mile is its kind's, worked out from its own mpg, unless it says otherwise.
    let per_mile = |v: &Value, name: &str| cell(v, name, "cost_per_mile")["value"].as_f64().unwrap_or_else(|| panic!("{name}: {}", cell(v, name, "cost_per_mile")));
    let by_mpg = |fuel: f64, mpg: f64| fuel * 4.546 / mpg;
    assert!((per_mile(&v, "Hatchback") - by_mpg(1.45, 52.0)).abs() < 1e-9 && (per_mile(&v, "Estate") - by_mpg(1.45, 44.0)).abs() < 1e-9);
    assert!((per_mile(&v, "Electric") - 0.28 * 0.28).abs() < 1e-9 && per_mile(&v, "Hybrid") == 0.09);
    let hatch = cell(&v, "Hatchback", "cost_per_mile");
    assert_eq!((&hatch["own"], &hatch["kind"], &hatch["formula"]), (&json!(false), &json!(ids("new:petrol")), &json!(petrol)), "it's the kind's, and says so: {hatch}");
    assert_eq!((&cell(&v, "Hybrid", "cost_per_mile")["own"], &cell(&v, "Hybrid", "cost_per_mile")["kind"]), (&json!(true), &json!(ids("new:petrol"))), "and one of its own says which kind it'd go back to");
    let cells = node(&v, "Hatchback")["cells"].as_object().unwrap();
    assert!(!cells.contains_key("value") && !cells.contains_key("task"), "and what a system says is the system's, not the kind's: {cells:?}");

    // What each car would cost a year, over the mileage you might do.
    let which = ids("new:which");
    let expected = |v: &Value| -> f64 {
        let cases = cell(v, "Yearly cost", "value")["shown"]["cases"].as_array().unwrap_or_else(|| panic!("{}", cell(v, "Yearly cost", "value"))).clone();
        cases.iter().map(|c| c["value"].as_f64().unwrap() * c["when"]["prob"].as_f64().unwrap()).sum()
    };
    // Mileage is 10,500 on average: 6,000, 10,000 twice as likely, and 16,000.
    let by_hand = |price: f64, per_mile: f64, insurance: f64, miles: f64| price * (1.0 - 0.85f64.powf(5.0)) / 5.0 + miles * per_mile + insurance;
    let cars = [("new:hatch", by_hand(18000.0, by_mpg(1.45, 52.0), 500.0, 10_500.0)), ("new:estate", by_hand(24000.0, by_mpg(1.45, 44.0), 550.0, 10_500.0)), ("new:ev", by_hand(32000.0, 0.28 * 0.28, 600.0, 10_500.0)), ("new:hybrid", by_hand(26000.0, 0.09, 550.0, 10_500.0))];
    for (car, want) in cars {
        let got = expected(&c.view(json!({ &which: ids(car) })).await);
        assert!((got - want).abs() < 1e-6, "{car}: {got} and {want}");
    }

    // The decision's range is whichever car it lands on, so anything can read it.
    assert_eq!(when(&v, "Gets to Edinburgh without stopping", "value"), "Which car?∈{Hatchback,Estate,Hybrid}");

    // How far you drive isn't yours to choose, and which car is. A view says which each cell differs
    // over, so what's expected can average over one and never the other.
    let mut uncertain: Vec<(String, bool)> = v["worlds"]["vars"].as_array().unwrap().iter().map(|x| (x["name"].as_str().unwrap().to_string(), x["uncertain"].as_bool().unwrap())).collect();
    uncertain.sort();
    assert_eq!(uncertain, vec![("Miles a year".to_string(), true), ("Which car?".to_string(), false)]);
    assert_eq!(cell(&v, "Hatchback", "cost_per_mile")["vars"], Value::Null, "a car's own cost a mile differs over nothing");
    let mut both = vec![ids("new:miles"), which.clone()];
    both.sort();
    assert_eq!(cell(&v, "Yearly cost", "value")["vars"], json!(both), "the chosen car's cost a year differs with both");
    assert_eq!(cell(&c.view(json!({ &which: ids("new:hatch") })).await, "Yearly cost", "value")["vars"], json!([ids("new:miles")]), "until the car is pinned");

    // What the hatchback and the electric car cost as fuel gets dearer, which is the same as typing each
    // price in, with nothing changed. Every petrol car reads the one price through its kind.
    let read = json!([{ "node": ids("new:hatch"), "component": "cost_per_mile" }, { "node": ids("new:ev"), "component": "cost_per_mile" }]);
    let swept = c.post("/api/sweep", json!({ "set": { "node": ids("new:fuel"), "component": "value" }, "values": [1.45, 3.0], "read": read })).await;
    let at = |i: usize, j: usize| swept[i]["cells"][j]["value"].as_f64().unwrap_or_else(|| panic!("{swept}"));
    for (i, fuel) in [1.45, 3.0].into_iter().enumerate() {
        assert!((at(i, 0) - by_mpg(fuel, 52.0)).abs() < 1e-9 && (at(i, 1) - 0.28 * 0.28).abs() < 1e-9, "{swept}");
    }
    let pinned = c.post("/api/sweep", json!({ "set": { "node": ids("new:miles"), "component": "value" }, "values": [20000], "read": [{ "node": ids("new:cost"), "component": "value" }], "world": { &which: ids("new:ev") } })).await;
    assert!((pinned[0]["cells"][0]["value"].as_f64().unwrap() - by_hand(32000.0, 0.28 * 0.28, 600.0, 20_000.0)).abs() < 1e-6, "{pinned}");
    assert!((per_mile(&c.view(json!({})).await, "Hatchback") - by_mpg(1.45, 52.0)).abs() < 1e-9, "and nothing was changed");

    // A car can say otherwise for itself, and goes back to its kind's when that's taken away.
    let hatch = ids("new:hatch");
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": hatch, "component": "cost_per_mile", "value": 0.2 }] })).await;
    assert_eq!(per_mile(&c.view(json!({})).await, "Hatchback"), 0.2);
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": hatch, "component": "cost_per_mile", "labels": {} }] })).await;
    assert!((per_mile(&c.view(json!({})).await, "Hatchback") - by_mpg(1.45, 52.0)).abs() < 1e-9);

    // And the kind changes every car of that kind at once, but not one that says its own.
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": ids("new:petrol"), "component": "cost_per_mile", "formula": "fuel * 4.5 / self.mpg" }] })).await;
    let v = c.view(json!({})).await;
    assert!((per_mile(&v, "Hatchback") - 1.45 * 4.5 / 52.0).abs() < 1e-9 && (per_mile(&v, "Estate") - 1.45 * 4.5 / 44.0).abs() < 1e-9);
    assert_eq!(per_mile(&v, "Hybrid"), 0.09);
}

/// The same cars as tables: every car has a price, only petrol cars have an mpg, and each row of Car is
/// an option of the decision. Then a recurrence with a choice part way down it.
#[tokio::test]
async fn held_out_tables_of_cars_and_years() {
    let c = Client::new();
    let dir = tempfile::tempdir().unwrap();
    c.post("/api/workspaces/new", json!({ "path": dir.path().join("tables"), "name": "Tables", "systems": ["plan"] })).await;
    c.post("/api/ops", json!({ "ops": [
        { "op": "add_node", "id": "new:fuel", "name": "Fuel price", "components": { "value": 1.45 } },
        { "op": "add_node", "id": "new:which", "name": "Which car?", "components": { "decision": {} } },
        { "op": "set", "id": "new:which", "component": "cost_per_mile", "formula": "pick(o in in.alternative: o.cost_per_mile)" },
        { "op": "table", "name": "Car", "alternative_of": "new:which", "rows": [
            { "key": "Hatchback", "price": 18000 }, { "key": "Electric", "price": 32000 }, { "key": "Hybrid", "price": 26000 },
        ] },
        { "op": "table", "name": "Petrol car", "is_a": "Car", "columns": { "cost_per_mile": { "formula": "fuel * 4.546 / self.mpg", "labels": { "fuel": "new:fuel" } } }, "rows": [
            { "key": "Hatchback", "mpg": 52 }, { "key": "Hybrid", "cost_per_mile": 0.09 },
        ] },
        { "op": "table", "name": "Electric car", "is_a": "Car", "columns": { "cost_per_mile": 0.08 }, "rows": ["Electric"] },
    ] })).await;
    let v = c.view(json!({})).await;
    for n in v["nodes"].as_array().unwrap() {
        for (k, cell) in n["cells"].as_object().unwrap() {
            assert!(cell["error"].is_null(), "{} {k}: {cell}", n["name"]);
        }
    }
    let per_mile = |name: &str| cell(&v, name, "cost_per_mile")["value"].as_f64().unwrap_or_else(|| panic!("{name}: {}", cell(&v, name, "cost_per_mile")));
    assert!((per_mile("Car[Hatchback]") - 1.45 * 4.546 / 52.0).abs() < 1e-9, "the Car row reads the petrol part's mpg through Petrol car's formula");
    assert_eq!((per_mile("Car[Hybrid]"), per_mile("Car[Electric]")), (0.09, 0.08));
    assert_eq!(node(&v, "Car[Hatchback]")["row"], json!({ "table": "Car", "key": "Hatchback" }));
    assert_eq!(node(&v, "Car[Hatchback]")["parts"], json!([id(&v, "Petrol car[Hatchback]")]));
    assert!(cell(&v, "Car[Electric]", "mpg").is_null(), "an electric car has no mpg at all");
    let which = cell(&v, "Which car?", "cost_per_mile")["shown"]["cases"].as_array().unwrap().len();
    assert_eq!(which, 3, "the decision's options are Car's rows");

    // Writing it again changes nothing that's there: rows with those names are used, not made twice.
    c.post("/api/ops", json!({ "ops": [{ "op": "table", "name": "Car", "rows": [{ "key": "Hatchback", "price": 17000 }, "Estate"] }] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["nodes"].as_array().unwrap().iter().filter(|n| n["name"] == "Car[Hatchback]").count(), 1);
    assert_eq!(cell(&v, "Car[Hatchback]", "price")["value"], 17000.0);
    assert!(refused(&c, json!([{ "op": "table", "name": "Car[x]" }])).await.contains("row's name"));

    // Years, with a top-up chosen at year 5 that only the years after it see.
    let done = c.post("/api/ops", json!({ "ops": [
        { "op": "add_node", "id": "new:rate", "name": "Rate", "components": { "value": 0.05 } },
        { "op": "add_node", "id": "new:top", "name": "Top up?", "components": { "decision": {} }, "formula": "pick(in.alternative)" },
        { "op": "add_node", "name": "No", "id": "new:no", "components": { "value": 0 }, "links": [{ "to": "new:top", "kind": "alternative", "value": {} }] },
        { "op": "add_node", "name": "Yes", "id": "new:yes", "components": { "value": 500 }, "links": [{ "to": "new:top", "kind": "alternative", "value": {} }] },
        { "op": "table", "name": "Year", "columns": { "balance": { "formula": "prev.balance * (1 + rate)", "labels": { "rate": "new:rate" } } }, "range": [0, 10], "rows": [
            { "key": 0, "balance": 1000 },
            { "key": 5, "balance": { "formula": "prev.balance * (1 + rate) + top_up", "labels": { "rate": "new:rate", "top_up": "new:top" } } },
        ] },
    ] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["nodes"].as_array().unwrap().iter().filter(|n| n["row"]["table"] == "Year").count(), 11);
    assert!(cell(&v, "Year[4]", "balance")["vars"].is_null());
    assert_eq!(cell(&v, "Year[10]", "balance")["vars"], json!([ids_of(&done, "new:top")]));
}

fn ids_of(done: &Value, k: &str) -> String {
    done["created"][k].as_str().unwrap().to_string()
}

/// A puzzle's constraints differ over its choices, but they hold in every world that's left, so there's
/// nothing to say about which they depend on. Something that differs between the worlds left still does.
#[tokio::test]
async fn what_holds_in_every_world_left_differs_over_nothing() {
    let c = Client::new();
    c.open("australia").await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Western Australia ≠ Northern Territory", "value")["vars"], Value::Null);
    assert_eq!(cell(&v, "A colouring", "value")["vars"], Value::Null);
    c.open("trip-budget").await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Over £200", "value")["vars"], json!([id(&v, "Stay")]));
    // Pinned, it's settled, but it still differs in some world, which is worth knowing.
    let pinned = c.view(json!({ id(&v, "Stay"): id(&v, "Hotel") })).await;
    assert_eq!((&cell(&pinned, "Over £200", "value")["vars"], &cell(&pinned, "Over £200", "value")["varies"]), (&Value::Null, &json!(true)));
    assert_eq!(cell(&v, "Train", "value")["varies"], Value::Null, "and what's the same everywhere doesn't");
}

#[tokio::test]
async fn part_of_a_formula_can_be_tried_with_nothing_changed() {
    let c = Client::new();
    c.open("trip-budget").await;
    let v = c.view(json!({})).await;
    let tried = c.post("/api/try", json!({ "node": id(&v, "Trip"), "text": "travel + 1" })).await;
    assert_eq!(tried["shown"]["cases"].as_array().unwrap().len(), 2);
    assert_eq!(tried["expansion"], "(Travel + 1)");
    let pinned = c.post("/api/try", json!({ "node": id(&v, "Trip"), "text": "travel + 1", "world": { id(&v, "Travel"): id(&v, "Plane") } })).await;
    assert_eq!(pinned["shown"]["cases"][0]["value"], 121.0);
    let bad = c.post("/api/try", json!({ "node": id(&v, "Trip"), "text": "nope" })).await;
    assert!(bad["error"].as_str().unwrap().contains("no input called nope"), "{bad}");
    assert_eq!(c.view(json!({})).await["last_seq"], v["last_seq"]);
}

#[tokio::test]
async fn a_drawing_is_added_undone_and_refused_like_anything_else() {
    let c = Client::new();
    c.open("australia").await;
    let done = c.post("/api/ops", json!({ "ops": [{ "op": "add_node", "id": "new:d", "name": "Mine", "components": { "drawing": "export function draw() { return \"\"; }" } }], "why": "a second way to see it" })).await;
    let d = done["created"]["new:d"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Mine", "drawing")["value"], "export function draw() { return \"\"; }");
    assert_eq!(cell(&v, "Map of Australia", "drawing")["source"], "stored", "and there can be several");
    assert_eq!(v["worlds"]["count"], "18");

    let why = refused(&c, json!([{ "op": "set", "id": d, "component": "drawing", "value": 3 }])).await;
    assert!(why.contains("malformed_component"), "a drawing is text: {why}");

    let (_, history) = c.call("GET", "/api/history", None).await;
    c.post("/api/undo", json!({ "seq": history[0]["seq"] })).await;
    assert!(c.view(json!({})).await["nodes"].as_array().unwrap().iter().all(|n| n["name"] != "Mine"));
}

#[tokio::test]
async fn a_hand_edit_nobody_has_noticed_yet_stays_on_the_branch_it_was_made_on() {
    let c = Client::new();
    c.open("invoice").await;
    c.post("/api/ops", json!({ "ops": [{ "op": "set", "id": "Tax rate", "component": "value", "value": 0.5 }] })).await;
    let v = c.view(json!({})).await;
    let path = format!("{}/nodes/{}.yaml", v["workspace"]["dir"].as_str().unwrap(), id(&v, "Tax rate"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("value: 0.5", "value: 0.7")).unwrap();

    // Straight away, before anything watching the files could have looked.
    c.app.checkout(0, "you", "").unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("value: 0.2"));
    let (_, history) = c.call("GET", "/api/history", None).await;
    let edit = history.as_array().unwrap().iter().find(|e| e["why"] == "external edit").expect("it was recorded, not lost");
    assert_eq!(edit["parent"], 1, "on the branch it was made on");
    let seq = edit["seq"].as_u64().unwrap();
    c.app.checkout(seq, "you", "").unwrap();
    assert_eq!(cell(&c.view(json!({})).await, "Tax rate", "value")["value"], 0.7);
}
