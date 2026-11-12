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

use crate::api;
use crate::app::{App, AppError};

struct Client {
    router: Router,
    app: Arc<App>,
    _home: tempfile::TempDir,
}

impl Client {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let app = App::new(home.path().to_path_buf());
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
        self.post("/api/call/view", json!({ "world": world })).await
    }

    async fn history(&self, node: Option<&str>) -> Value {
        self.post("/api/call/history", json!({ "node": node })).await
    }

    fn propose(&self, title: &str, rationale: &str, ops: Vec<Value>) -> Result<Value, AppError> {
        self.app.call("propose", json!({ "title": title, "rationale": rationale, "ops": ops }), "claude")
    }

    /// Where the open workspace is on disk.
    async fn dir(&self) -> String {
        self.view(json!({})).await["workspace"]["location"].as_str().unwrap().to_string()
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
    let (status, _) = c.call("POST", "/api/call/view", Some(json!({}))).await;
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
    let dir = c.dir().await;
    assert!(dir.starts_with(c._home.path().to_str().unwrap()), "a working copy, in its own home: {dir}");
    let v = c.view(json!({})).await;
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": id(&v, "Tax rate"), "cell": "value", "value": 0.5 }] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Tax", "value")["value"], 40.0);
    // Opened again, it's as it was left.
    c.open("invoice").await;
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
    assert_eq!(cell(&v, "Total", "value")["formula"], "subtotal + tax");
    assert_eq!(cell(&v, "Tax rate", "value")["source"], "own");

    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": "Widgets: price", "cell": "value", "value": 11.0 }], "why": "dearer" })).await;
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
    c.post("/api/call/apply", json!({ "ops": [{ "op": "link", "from": "Subtotal", "to": "Total", "component": "label", "cells": { "called": "line" } }] })).await;
    let v = c.view(json!({})).await;
    assert!(cell(&v, "Total", "value")["error"].as_str().unwrap().contains("depends on itself"), "{}", cell(&v, "Total", "value"));
    assert_eq!(cell(&v, "Gears", "value")["value"], 50.0);
    c.post("/api/call/apply", json!({ "ops": [{ "op": "unlink", "from": "Subtotal", "to": "Total", "component": "label" }] })).await;
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
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": "Which approach?", "cell": "chosen", "value": { "ref": cad } }], "why": "it's accurate enough" })).await;
    let v = c.view(json!({})).await;
    assert!(cell(&v, "Export extrinsics from CAD", "need")["shown"]["always"].as_bool().unwrap());
    assert!(v["worlds"]["vars"].as_array().unwrap().is_empty(), "no open choices now");

    let (_, history) = (StatusCode::OK, c.history(None).await);
    assert_eq!(history[0]["why"], "it's accurate enough");
    assert_eq!(history[0]["author"], "you");
    let seq = history[0]["seq"].as_u64().unwrap();
    c.post("/api/call/undo", json!({ "seq": seq, "why": "changed my mind" })).await;
    assert_eq!(when(&c.view(json!({})).await, "Static captures", "need"), when(&open, "Static captures", "need"));
}

#[tokio::test]
async fn a_change_that_breaks_a_rule_is_refused_and_says_why() {
    let c = Client::new();
    c.open("calibration").await;
    let v = c.view(json!({})).await;
    let imu = id(&v, "Fit the IMU time offset");
    let (status, body) = c.call("POST", "/api/call/apply", Some(json!({ "ops": [{ "op": "set", "id": "Which approach?", "cell": "chosen", "value": { "ref": imu } }] }))).await;
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
        .post("/api/call/apply", json!({ "ops": [
            { "op": "add_node", "id": "new:a", "name": "Held-out scenes", "components": ["task"], "cells": { "estimate": 2.0 }, "links": [{ "to": "Which approach?", "component": "part_of" }] },
            { "op": "add_node", "id": "new:b", "name": "Check them", "components": ["task"], "links": [{ "to": "new:a", "component": "depends_on" }] },
        ], "why": "we'll need these whichever way it goes" }))
        .await;
    let a = done["created"]["new:a"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(id(&v, "Held-out scenes"), a);
    assert_eq!(v["last_seq"].as_u64().unwrap(), before["last_seq"].as_u64().map_or(0, |s| s + 1));
    assert!(cell(&v, "Held-out scenes", "need")["shown"]["always"].as_bool().unwrap());
    assert_eq!(cell(&v, "Check them", "blocked")["shown"]["always"], true, "it waits for what it depends on");

    c.post("/api/call/apply", json!({ "ops": [{ "op": "delete_node", "id": "Check them" }] })).await;
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
    assert_eq!(s[id(&v, "Pet[Zebra]")], s[id(&v, "Nationality[Japanese]")]);
    assert_eq!(s[id(&v, "Drink[Water]")], s[id(&v, "Nationality[Norwegian]")]);
    // The tables are what the rows choose from, not choices themselves.
    assert!(v["worlds"]["vars"].as_array().unwrap().iter().all(|x| x["id"] != id(&v, "Pet")));
    assert_eq!(v["worlds"]["vars"].as_array().unwrap().len(), 25);
}

/// The case file: how likely each suspect is once the evidence has ruled worlds out, and which of the
/// things in doubt is most worth finding out. The numbers are worked out by hand in the comments.
#[tokio::test]
async fn the_case_file_says_who_it_probably_was_and_what_to_find_out() {
    let c = Client::new();
    c.open("case-file").await;
    let v = c.view(json!({})).await;
    let var = |v: &Value, name: &str| v["worlds"]["vars"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap_or_else(|| panic!("no {name}")).clone();
    let chance = |v: &Value, option: &str| {
        let who = var(v, "Who took it?");
        who["options"].as_array().unwrap().iter().find(|o| o["name"] == option).unwrap()["chance"].as_f64().unwrap()
    };
    // Of all the weight (9 for who, times the accounts'), the evidence leaves Tom half of his: he's ruled
    // out only if he left at 9:40 (3/4) and cycled (2/3). Pryce is left only if he drove, Clara only if she went out.
    // Tom 3·1/2, Pryce 3·1/4, Clara 2·1/3, someone else 1, over their total.
    let total = 1.5 + 0.75 + 2.0 / 3.0 + 1.0;
    for (who, w) in [("Tom", 1.5), ("Mr Pryce", 0.75), ("Clara", 2.0 / 3.0), ("Someone else", 1.0)] {
        assert!((chance(&v, who) - w / total).abs() < 1e-9, "{who}: {} not {}", chance(&v, who), w / total);
    }
    let who = var(&v, "Who took it?");
    assert!((who["sure"].as_f64().unwrap() - 1.5 / total).abs() < 1e-9, "Tom is likeliest");
    // How Mr Pryce got home settles most: if he drove he's likeliest, and if not he's out.
    let first = &who["clues"][0];
    assert_eq!(first["var"], var(&v, "How did Mr Pryce get home?")["id"]);
    assert!((first["sure"].as_f64().unwrap() - 0.479).abs() < 0.001, "{first}");

    // How likely anything is, is among the worlds still in play: the evidence holds in all of them.
    let evidence = node(&v, "They came in from the garden");
    assert_eq!(evidence["cells"]["value"]["shown"]["prob"], 1.0, "{}", evidence["cells"]["value"]);

    // Find out he was on the train, and he's out of it.
    let pryce = var(&v, "How did Mr Pryce get home?");
    let train = pryce["options"].as_array().unwrap().iter().find(|o| o["name"].as_str().unwrap().starts_with("On the 9:20")).unwrap()["id"].clone();
    let v = c.view(json!({ pryce["id"].as_str().unwrap(): train })).await;
    assert_eq!(chance(&v, "Mr Pryce"), 0.0);
    assert!((chance(&v, "Tom") - 0.47).abs() < 0.01);
    assert!(var(&v, "How did Mr Pryce get home?")["clues"].as_array().is_none_or(|c| c.is_empty()), "what's pinned isn't a question any more");
}

/// The classic problems, each with the answer it's known for.
#[tokio::test]
async fn the_classic_problems_have_their_known_answers() {
    let c = Client::new();
    let var = |v: &Value, name: &str| v["worlds"]["vars"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap_or_else(|| panic!("no {name}")).clone();
    let chance = |v: &Value, question: &str, option: &str| var(v, question)["options"].as_array().unwrap().iter().find(|o| o["name"] == option).unwrap_or_else(|| panic!("no {option}"))["chance"].as_f64().unwrap();
    let prob = |v: &Value, name: &str| cell(v, name, "value")["shown"]["prob"].as_f64().unwrap_or_else(|| panic!("{name}: {}", cell(v, name, "value")));
    let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} not {b}");

    // Monty Hall: the car is twice as likely behind door 2, so switching wins two times in three.
    c.open("monty-hall").await;
    let v = c.view(json!({})).await;
    near(chance(&v, "Where's the car?", "Behind door 1"), 1.0 / 3.0);
    near(chance(&v, "Where's the car?", "Behind door 2"), 2.0 / 3.0);
    near(chance(&v, "Where's the car?", "Behind door 3"), 0.0);
    let choice = var(&v, "Stick or switch?");
    for (option, wins) in [("Stick with door 1", 1.0 / 3.0), ("Switch to door 2", 2.0 / 3.0)] {
        let pinned = c.view(json!({ choice["id"].as_str().unwrap(): id(&v, option) })).await;
        near(prob(&pinned, "You win the car"), wins);
    }

    // The positive test: 90 true positives among 585.
    c.open("medical-test").await;
    let v = c.view(json!({})).await;
    near(chance(&v, "Do you have it?", "You have it"), 90.0 / 585.0);

    // Bertrand's box: two of the three gold coins are in the box with two.
    c.open("bertrands-box").await;
    let v = c.view(json!({})).await;
    near(chance(&v, "Which box?", "Two gold"), 2.0 / 3.0);
    near(chance(&v, "Which box?", "Two silver"), 0.0);
    near(prob(&v, "The other coin is gold too"), 2.0 / 3.0);

    // Two children: a third, and a half once it's the elder who's known to be a boy.
    c.open("two-children").await;
    let v = c.view(json!({})).await;
    near(prob(&v, "Both are boys"), 1.0 / 3.0);
    let pinned = c.view(json!({ var(&v, "The elder")["id"].as_str().unwrap(): id(&v, "The elder is a boy") })).await;
    near(prob(&pinned, "Both are boys"), 0.5);

    // Socks: each count of pairs by how likely three different socks were from it.
    c.open("socks").await;
    let v = c.view(json!({})).await;
    let likely = |n: f64| (2.0 * n - 4.0) / (2.0 * n - 1.0);
    let total: f64 = (3..=6).map(|n| likely(n as f64)).sum();
    for n in 3..=6 {
        near(chance(&v, "How many pairs went in?", &format!("{n} pairs")), likely(n as f64) / total);
    }
}

#[tokio::test]
async fn the_plan_answers_where_work_stands_and_what_decisions_need() {
    let c = Client::new();
    c.open("calibration").await;
    let p = c.post("/api/call/plan", json!({})).await;
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
    let p = c.post("/api/call/plan", json!({})).await;
    assert!(p["work"].as_array().unwrap().is_empty() && p["decisions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn proposals_wait_for_review_and_can_be_taken_in_part() {
    let c = Client::new();
    c.open("calibration").await;
    let before = c.view(json!({})).await;
    let p = c.propose("Add two", "both are needed", vec![
        json!({ "op": "add_node", "id": "new:a", "name": "One" }),
        json!({ "op": "add_node", "id": "new:b", "name": "Two" }),
    ]).unwrap();
    assert_eq!(c.view(json!({})).await["last_seq"], before["last_seq"], "nothing has happened yet");

    let (_, list) = (StatusCode::OK, c.post("/api/call/proposals", json!({})).await);
    assert_eq!(list[0]["id"], p["id"]);
    assert_eq!(list[0]["error"], Value::Null);
    assert_eq!(list[0]["ops"].as_array().unwrap().len(), 4, "each node is made, then named");

    // Only the first.
    c.post("/api/call/accept", json!({ "id": p["id"], "ops": [0, 1] })).await;
    let v = c.view(json!({})).await;
    assert!(v["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "One"));
    assert!(!v["nodes"].as_array().unwrap().iter().any(|n| n["name"] == "Two"));
    assert_eq!((StatusCode::OK, c.post("/api/call/proposals", json!({})).await).1.as_array().unwrap().len(), 0);
    let (_, history) = (StatusCode::OK, c.history(None).await);
    assert_eq!(history[0]["author"], "claude", "the log says who proposed it");
    assert!(history[0]["why"].as_str().unwrap().contains("both are needed"));

    // One that wouldn't apply is refused before it gets to anyone.
    let v = c.view(json!({})).await;
    let refused = c.propose("Bad", "", vec![json!({ "op": "set", "id": "Which approach?", "cell": "chosen", "value": { "ref": id(&v, "One") } })]);
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_hand_edit_on_disk_is_recorded_as_an_event() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let dir = v["workspace"]["location"].as_str().unwrap().to_string();
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
    let (_, history) = (StatusCode::OK, c.history(None).await);
    assert_eq!((history[0]["author"].as_str(), history[0]["why"].as_str()), (Some("disk"), Some("external edit")));
}

#[tokio::test]
async fn undo_and_redo_go_back_and_forward_and_the_files_follow() {
    let c = Client::new();
    c.open("invoice").await;
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": "Tax rate", "cell": "value", "value": 0.5 }], "why": "raised" })).await;
    let dir = c.dir().await;
    let rate = id(&c.view(json!({})).await, "Tax rate");
    let file = format!("{dir}/nodes/{rate}.yaml");
    assert!(std::fs::read_to_string(&file).unwrap().contains("value: 0.5"));

    let v = c.view(json!({})).await;
    assert_eq!((v["undo"].as_u64(), v["redo"].as_u64()), (Some(0), None));
    c.post("/api/call/undo", json!({})).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Tax", "value")["value"], 16.0);
    assert!(std::fs::read_to_string(&file).unwrap().contains("value: 0.2"), "and the file says so");
    assert_eq!((v["undo"].as_u64(), v["redo"].as_u64()), (None, Some(1)));

    c.post("/api/call/redo", json!({})).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Tax", "value")["value"], 40.0);
    assert!(std::fs::read_to_string(&file).unwrap().contains("value: 0.5"));
    let history = c.history(None).await;
    assert_eq!(history.as_array().unwrap().len(), 3, "every step is an event");
    assert_eq!(history[0]["reverts"], 1, "the redo takes back the undo");

    // A node's own history is only what touched it.
    let own = c.history(Some(&rate)).await;
    assert_eq!(own.as_array().unwrap().len(), 3);
    let gears = id(&v, "Gears");
    assert_eq!(c.history(Some(&gears)).await.as_array().unwrap().len(), 0);

    // With nothing to undo, it says so by giving nothing back.
    c.open("invoice").await;
    assert_eq!(c.post("/api/call/undo", json!({})).await["seq"], Value::Null);
    let (status, _) = c.call("POST", "/api/call/undo", Some(json!({ "seq": 99 }))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_worked_out_cell_can_be_typed_over_and_comes_back() {
    let c = Client::new();
    c.open("calibration").await;
    let done = c.post("/api/call/apply", json!({ "ops": [{ "op": "add_node", "id": "new:t", "name": "Mine", "components": ["task"] }] })).await;
    let t = done["created"]["new:t"].as_str().unwrap().to_string();
    let cell = |v: &Value| v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == t.as_str()).unwrap()["cells"]["done"].clone();

    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["source"], shown["value"].is_null()), (&json!("default"), false), "a task says it's owed before anyone has said anything: {shown}");
    assert_eq!(shown["value"], json!(false));
    assert!(shown["formula"].as_str().unwrap().contains("is_leaf"), "and what it's worked out by: {shown}");

    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": t, "cell": "done", "value": true }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["source"], &shown["value"]), (&json!("own"), &json!(true)));

    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": t, "cell": "done", "formula": "1 < 2" }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["source"], &shown["formula"], &shown["value"]), (&json!("own"), &json!("1 < 2"), &json!(true)));

    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": t, "cell": "done" }] })).await;
    let shown = cell(&c.view(json!({})).await);
    assert_eq!((&shown["source"], &shown["value"]), (&json!("default"), &json!(false)), "and without it, it's worked out again");
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

    let other = dir.path().join("not-one");
    std::fs::create_dir_all(&other).unwrap();
    let (status, body) = c.call("POST", "/api/workspaces/open", Some(json!({ "path": other }))).await;
    assert!(!status.is_success());
    assert!(body["error"].as_str().unwrap().contains("isn't a workspace"), "{body}");
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0, "and nothing was written there");
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
    assert!(paths.contains(&"recollect.toml"), "{paths:?}");
    assert!(!paths.contains(&".recollect.lock"), "{paths:?}");

    let v = c.view(json!({})).await;
    let path = format!("nodes/{}.yaml", id(&v, "Which approach?"));
    assert!(paths.contains(&path.as_str()), "{paths:?}");
    let (status, file) = c.call("GET", &format!("/api/file?path={path}"), None).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert!(file["text"].as_str().unwrap().contains("Which approach?"), "{file}");
    assert_eq!(file["truncated"], false);

    // Reading is all it does, and a change shows up.
    let before = c.call("GET", &format!("/api/file?path={path}"), None).await.1;
    assert_eq!(c.call("GET", &format!("/api/file?path={path}"), None).await.1, before);
    let done = c.post("/api/call/apply", json!({ "ops": [{ "op": "add_node", "id": "new:x", "name": "Fresh" }] })).await;
    let fresh = format!("nodes/{}.yaml", done["created"]["new:x"].as_str().unwrap());
    let after = c.call("GET", &format!("/api/file?path={fresh}"), None).await.1;
    assert!(after["text"].as_str().unwrap().contains("Fresh"), "{after}");
}

#[tokio::test]
async fn only_the_workspaces_own_files_can_be_read() {
    let c = Client::new();
    c.open("invoice").await;
    let dir = std::path::PathBuf::from(c.call("GET", "/api/workspaces", None).await.1["current"]["location"].as_str().unwrap());
    let secret = c._home.path().join("secret.txt");
    std::fs::write(&secret, "private").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("linked.txt")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/config"), "private").unwrap();

    let (_, list) = c.call("GET", "/api/files", None).await;
    let listed = paths(&list);
    assert!(!listed.iter().any(|p| p.contains("linked") || p.contains(".git/")), "{listed:?}");

    let absolute = secret.display().to_string();
    for path in ["../../secret.txt", "nodes/../../../secret.txt", "linked.txt", ".git/config", ".recollect.lock", "nope.txt", absolute.as_str()] {
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
    let (status, body) = c.call("POST", "/api/call/apply", Some(json!({ "ops": ops }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    body["error"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_formula_and_its_labels_are_one_change_and_a_bad_one_is_refused() {
    let c = Client::new();
    c.open("invoice").await;
    let before = c.view(json!({})).await;

    let done = c.post("/api/call/apply", json!({ "ops": [{ "op": "add_node", "id": "new:x", "name": "Doubled", "cells": { "value": { "formula": "total * 2", "labels": { "total": "Total" } } } }] })).await;
    let x = done["created"]["new:x"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Doubled", "value")["value"], 192.0);
    assert_eq!(v["last_seq"].as_u64().unwrap(), before["last_seq"].as_u64().map_or(0, |s| s + 1), "one event");

    // Changing it says what its names now mean, and a name it no longer has goes.
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": x, "cell": "value", "formula": "tax + 1", "labels": { "tax": "Tax" } }] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Doubled", "value")["value"], 17.0);
    let labels: Vec<&Value> = v["edges"].as_array().unwrap().iter().filter(|l| l["from"] == x.as_str() && l["components"] == json!(["label"])).collect();
    assert_eq!(labels.len(), 1, "{labels:?}");
    assert_eq!(labels[0]["cells"]["called"]["value"], "tax");

    // Several nodes under one name are a group.
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": x, "cell": "value", "formula": "sum(n)", "labels": { "n": ["Tax", "Total"] } }] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Doubled", "value")["value"], 112.0);

    // What's wrong with one is said before anything lands.
    let seq = c.view(json!({})).await["last_seq"].clone();
    let why = refused(&c, json!([{ "op": "set", "id": x, "cell": "value", "formula": "nope + 1" }])).await;
    assert!(why.contains("no input called nope"), "{why}");
    let why = refused(&c, json!([{ "op": "set", "id": x, "cell": "value", "formula": "1 +" }])).await;
    assert!(why.contains("stops short"), "{why}");
    let why = refused(&c, json!([{ "op": "set", "id": x, "cell": "value", "formula": "\"a\" + 1" }])).await;
    assert!(why.contains("+ needs numbers"), "{why}");
    let why = refused(&c, json!([{ "op": "link", "from": x, "to": "Tax", "component": "label", "cells": { "called": "two words" } }])).await;
    assert!(why.contains("a label has to be a name"), "{why}");
    assert_eq!(c.view(json!({})).await["last_seq"], seq, "and nothing changed");

    // A proposal is held to the same, before anyone is asked to look at it.
    assert!(c.propose("Bad", "", vec![json!({ "op": "add_node", "id": "new:y", "name": "Y", "cells": { "value": { "formula": "nope" } } })]).is_err());
}

#[tokio::test]
async fn a_hand_edit_to_a_formula_stands_and_its_cell_says_whats_wrong() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let dir = v["workspace"]["location"].as_str().unwrap().to_string();
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
    let dir = std::path::PathBuf::from(v["workspace"]["location"].as_str().unwrap());
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
    let p = c.propose("Cap the effort at six days", "the review is in a week", vec![
        json!({ "op": "set", "id": "Static captures", "cell": "estimate", "formula": "if(chose(approach, free), 3, 2)", "labels": { "approach": approach, "free": free } }),
        json!({ "op": "add_node", "id": "new:effort", "name": "Effort", "cells": { "value": { "formula": "effort()" } } }),
        json!({ "op": "add_node", "id": "new:cap", "name": "Within six days", "components": ["requires"], "cells": { "value": { "formula": "effort <= 6", "labels": { "effort": "new:effort" } } } }),
    ]).unwrap();
    let (_, list) = (StatusCode::OK, c.post("/api/call/proposals", json!({})).await);
    assert!(list[0]["ops"].as_array().unwrap().iter().any(|o| o["value"] == "effort <= 6"), "the formula is what gets reviewed: {list}");
    c.post("/api/call/accept", json!({ "id": p["id"] })).await;

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
    let person = |id: &str, name: &str| json!({ "op": "add_node", "id": id, "name": name, "cells": { "options": ["truth", "lie"] } });
    c.post("/api/call/apply", json!({ "ops": [
        person("new:a", "Alice"), person("new:b", "Bob"), person("new:c", "Carol"),
        { "op": "add_node", "id": "new:most", "name": "At most one lies", "components": ["requires"], "cells": { "value": { "formula": "count(x in people: x.chosen.value = \"lie\") <= 1", "labels": { "people": ["new:a", "new:b", "new:c"] } } } },
        { "op": "add_node", "id": "new:clue", "name": "Alice and Bob differ", "components": ["requires"], "cells": { "value": { "formula": "a.chosen != b.chosen", "labels": { "a": "new:a", "b": "new:b" } } } },
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
    c.post("/api/call/apply", json!({ "ops": [
        { "op": "add_node", "id": "new:tier", "name": "Tier", "cells": { "macro": { "params": ["units", "from", "to", "price"], "body": "max(0, min(units, to) - from) * price" } } },
        { "op": "add_node", "id": "new:units", "name": "Units", "cells": { "value": 250 } },
        { "op": "add_node", "id": "new:bill", "name": "Bill", "cells": { "value": { "formula": "tier(units, 0, 100, 10) + tier(units, 100, 200, 8) + tier(units, 200, 1000, 5)", "labels": { "tier": "new:tier", "units": "new:units" } } } },
    ] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Bill", "value")["value"], 2050.0, "1000 + 800 + 250");

    // Monthly repayment on 10000 at 1% a month over 12 months, from the standard formula, with the
    // power written out by chaining a macro that squares.
    c.post("/api/call/apply", json!({ "ops": [
        { "op": "add_node", "id": "new:sq", "name": "Squared", "cells": { "macro": { "params": ["x"], "body": "x * x" } } },
        { "op": "add_node", "id": "new:rate", "name": "Rate", "cells": { "value": 0.01 } },
        { "op": "add_node", "id": "new:g", "name": "Growth", "cells": { "value": { "formula": "sq(sq(1 + r)) * sq(sq(1 + r)) * sq(sq(1 + r))", "labels": { "sq": "new:sq", "r": "new:rate" } } } },
        { "op": "add_node", "id": "new:pay", "name": "Payment", "cells": { "value": { "formula": "round(10000 * r * g / (g - 1), 2)", "labels": { "r": "new:rate", "g": "new:g" } } } },
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
    let option = |id: &str, name: &str, of: &str, value: f64, weight: f64| json!({ "op": "add_node", "id": id, "name": name, "cells": { "value": value }, "links": [{ "to": of, "component": "alternative", "cells": { "weight": weight } }] });
    // A car says what it is, and its kind says what follows from that.
    let car = |id: &str, name: &str, kind: &str, cell: &str, v: f64, price: f64, insurance: f64, range: f64| {
        json!({ "op": "add_node", "id": id, "name": name, "cells": { cell: v, "price": price, "insurance": insurance, "range": range },
            "links": [{ "to": "new:which", "component": "alternative" }, { "to": kind, "component": "is_a" }] })
    };
    let petrol = "fuel * 4.546 / self.mpg";
    let done = c.post("/api/call/apply", json!({ "ops": [
        { "op": "add_node", "id": "new:years", "name": "Years kept", "cells": { "value": 5 } },
        { "op": "add_node", "id": "new:miles", "name": "Miles a year", "components": ["decision", "uncertain"] },
        option("new:light", "Light use", "new:miles", 6000.0, 1.0), option("new:typical", "Typical", "new:miles", 10000.0, 2.0), option("new:heavy", "Heavy", "new:miles", 16000.0, 1.0),
        { "op": "add_node", "id": "new:fuel", "name": "Fuel price", "cells": { "value": 1.45 } },
        { "op": "add_node", "id": "new:power", "name": "Electricity price", "cells": { "value": 0.28 } },
        { "op": "add_node", "id": "new:petrol", "name": "Petrol car", "components": ["task"], "cells": { "value": 1 }, "links": [{ "to": "new:fuel", "component": "label", "cells": { "called": "fuel" } }] },
        { "op": "set", "id": "new:petrol", "cell": "cost_per_mile", "formula": petrol },
        { "op": "add_node", "id": "new:electric", "name": "Electric car", "links": [{ "to": "new:power", "component": "label", "cells": { "called": "power" } }] },
        { "op": "set", "id": "new:electric", "cell": "cost_per_mile", "formula": "power * self.kwh_per_mile" },
        { "op": "add_node", "id": "new:which", "name": "Which car?", "components": ["decision"] },
        car("new:hatch", "Hatchback", "new:petrol", "mpg", 52.0, 18000.0, 500.0, 550.0),
        car("new:estate", "Estate", "new:petrol", "mpg", 44.0, 24000.0, 550.0, 600.0),
        car("new:ev", "Electric", "new:electric", "kwh_per_mile", 0.28, 32000.0, 600.0, 250.0),
        // Its maker gives its cost a mile, so it says that for itself rather than an mpg.
        car("new:hybrid", "Hybrid", "new:petrol", "cost_per_mile", 0.09, 26000.0, 550.0, 650.0),
        { "op": "add_node", "id": "new:cost", "name": "Yearly cost", "cells": { "value": { "formula": "car.chosen.price * (1 - 0.85 ^ years) / years + miles.chosen.value * car.chosen.cost_per_mile + car.chosen.insurance",
            "labels": { "car": "new:which", "years": "new:years", "miles": "new:miles" } } } },
        { "op": "add_node", "id": "new:trip", "name": "To Edinburgh", "cells": { "value": 400 } },
        { "op": "add_node", "id": "new:reaches", "name": "Gets to Edinburgh without stopping", "cells": { "value": { "formula": "car.chosen.range >= trip", "labels": { "car": "new:which", "trip": "new:trip" } } } },
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
    assert_eq!((&hatch["source"], &hatch["kind"], &hatch["formula"]), (&json!("kind"), &json!(ids("new:petrol")), &json!(petrol)), "it's the kind's, and says so: {hatch}");
    assert_eq!((&cell(&v, "Hybrid", "cost_per_mile")["source"], &cell(&v, "Hybrid", "cost_per_mile")["beneath"]), (&json!("own"), &json!(ids("new:petrol"))), "and one of its own says which kind it'd go back to");
    // A kind's components are its instances': a petrol car is a task because Petrol car is. Its value
    // is its own, though: a kind's answer isn't each instance's.
    let hatchback = node(&v, "Hatchback");
    assert!(hatchback["components"].as_array().unwrap().iter().any(|c| c["name"] == "task" && c["why"] == "kind"), "{hatchback}");
    assert!(!hatchback["cells"].as_object().unwrap().contains_key("value"), "{hatchback}");

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

    // What's read through the decision is whichever car it lands on says.
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
    let read = json!([{ "node": ids("new:hatch"), "cell": "cost_per_mile" }, { "node": ids("new:ev"), "cell": "cost_per_mile" }]);
    let swept = c.post("/api/call/sweep", json!({ "set": { "node": ids("new:fuel"), "cell": "value" }, "values": [1.45, 3.0], "read": read })).await;
    let at = |i: usize, j: usize| swept[i]["cells"][j]["value"].as_f64().unwrap_or_else(|| panic!("{swept}"));
    for (i, fuel) in [1.45, 3.0].into_iter().enumerate() {
        assert!((at(i, 0) - by_mpg(fuel, 52.0)).abs() < 1e-9 && (at(i, 1) - 0.28 * 0.28).abs() < 1e-9, "{swept}");
    }
    // Heavy use, swept up to 20,000 with both decisions pinned.
    let pinned = c.post("/api/call/sweep", json!({ "set": { "node": ids("new:heavy"), "cell": "value" }, "values": [20000], "read": [{ "node": ids("new:cost"), "cell": "value" }], "world": { &which: ids("new:ev"), ids("new:miles"): ids("new:heavy") } })).await;
    assert!((pinned[0]["cells"][0]["value"].as_f64().unwrap() - by_hand(32000.0, 0.28 * 0.28, 600.0, 20_000.0)).abs() < 1e-6, "{pinned}");
    assert!((per_mile(&c.view(json!({})).await, "Hatchback") - by_mpg(1.45, 52.0)).abs() < 1e-9, "and nothing was changed");

    // A car can say otherwise for itself, and goes back to its kind's when that's taken away.
    let hatch = ids("new:hatch");
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": hatch, "cell": "cost_per_mile", "value": 0.2 }] })).await;
    assert_eq!(per_mile(&c.view(json!({})).await, "Hatchback"), 0.2);
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": hatch, "cell": "cost_per_mile", "labels": {} }] })).await;
    assert!((per_mile(&c.view(json!({})).await, "Hatchback") - by_mpg(1.45, 52.0)).abs() < 1e-9);

    // And the kind changes every car of that kind at once, but not one that says its own.
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": ids("new:petrol"), "cell": "cost_per_mile", "formula": "fuel * 4.5 / self.mpg" }] })).await;
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
    c.post("/api/call/apply", json!({ "ops": [
        { "op": "add_node", "id": "new:fuel", "name": "Fuel price", "cells": { "value": 1.45 } },
        { "op": "add_node", "id": "new:which", "name": "Which car?", "components": ["decision"] },
        { "op": "add_node", "id": "new:per_mile", "name": "Per mile", "cells": { "value": { "formula": "car.chosen.cost_per_mile", "labels": { "car": "new:which" } } } },
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
    let which = cell(&v, "Which car?", "chosen")["shown"]["items"].as_array().unwrap().len();
    assert_eq!(which, 3, "the decision's options are Car's rows");
    let per_mile_cases: Vec<f64> = cell(&v, "Per mile", "value")["shown"]["cases"].as_array().unwrap().iter().map(|c| c["value"].as_f64().unwrap()).collect();
    assert_eq!(per_mile_cases.len(), 3, "and what's read through it is each row's: {per_mile_cases:?}");

    // Writing it again changes nothing that's there: rows with those names are used, not made twice.
    c.post("/api/call/apply", json!({ "ops": [{ "op": "table", "name": "Car", "rows": [{ "key": "Hatchback", "price": 17000 }, "Estate"] }] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["nodes"].as_array().unwrap().iter().filter(|n| n["name"] == "Car[Hatchback]").count(), 1);
    assert_eq!(cell(&v, "Car[Hatchback]", "price")["value"], 17000.0);
    assert!(refused(&c, json!([{ "op": "table", "name": "Car[x]" }])).await.contains("row's name"));

    // Years, with a top-up chosen at year 5 that only the years after it see.
    let done = c.post("/api/call/apply", json!({ "ops": [
        { "op": "add_node", "id": "new:rate", "name": "Rate", "cells": { "value": 0.05 } },
        { "op": "add_node", "id": "new:top", "name": "Top up?", "components": ["decision"] },
        { "op": "add_node", "name": "No", "id": "new:no", "cells": { "value": 0 }, "links": [{ "to": "new:top", "component": "alternative" }] },
        { "op": "add_node", "name": "Yes", "id": "new:yes", "cells": { "value": 500 }, "links": [{ "to": "new:top", "component": "alternative" }] },
        { "op": "table", "name": "Year", "columns": { "balance": { "formula": "prev.balance * (1 + rate)", "labels": { "rate": "new:rate" } } }, "range": [0, 10], "rows": [
            { "key": 0, "balance": 1000 },
            { "key": 5, "balance": { "formula": "prev.balance * (1 + rate) + top_up.chosen.value", "labels": { "rate": "new:rate", "top_up": "new:top" } } },
        ] },
    ] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(v["nodes"].as_array().unwrap().iter().filter(|n| n["row"]["table"] == "Year").count(), 11);
    assert!(cell(&v, "Year[4]", "balance")["vars"].is_null());
    assert_eq!(cell(&v, "Year[10]", "balance")["vars"], json!([ids_of(&done, "new:top")]));
}

/// A row read through a reference, as car.chosen.price reads Car[Hatchback], isn't read by where it is:
/// the label to what holds the reference already shows it, so the map draws no line of its own for it.
#[tokio::test]
async fn rows_read_through_a_reference_are_not_read_by_where_they_are() {
    let c = Client::new();
    c.open("choosing-a-car").await;
    let v = c.view(json!({})).await;
    for n in ["Yearly cost", "Gets to Edinburgh without stopping", "Which car?"] {
        assert!(node(&v, n)["reads"].is_null(), "{n} reads {}", node(&v, n)["reads"]);
    }
}

/// A numbered table goes on past its last row, worked out and never saved, and a row nobody wrote says
/// what it's missing by name.
#[tokio::test]
async fn rows_nobody_wrote_can_be_looked_at() {
    let c = Client::new();
    c.open("compound-interest").await;
    let v = c.view(json!({})).await;
    let rows = c.post("/api/call/rows", json!({ "table": "Year", "from": 4, "to": 12, "world": { id(&v, "Top up in year 5?"): id(&v, "No top-up") } })).await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.iter().take(3).map(|r| (r["key"].as_i64().unwrap(), r["written"].as_bool().unwrap())).collect::<Vec<_>>(), vec![(4, false), (5, true), (6, false)]);
    let twelve = rows[8]["cells"]["balance"]["value"].as_f64().unwrap();
    assert!((twelve - 1000.0 * 1.05f64.powi(12)).abs() < 1e-6, "{twelve}");
    let after = c.view(json!({})).await;
    assert_eq!(after["nodes"].as_array().unwrap().len(), v["nodes"].as_array().unwrap().len(), "and they aren't nodes");

    // The view carries the rows nobody wrote that something written reads, and says who reads what.
    let names = |v: &Value| v.as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&v["unwritten"]), vec!["Year[4]", "Year[30]"], "what Year[5] reads as prev, and what After 30 years asks for");
    let year4 = v["unwritten"][0]["id"].clone();
    assert_eq!(node(&v, "Year[5]")["reads"], json!([year4]));
    assert!(v["unwritten"][0]["reads"].is_null(), "and a row nobody wrote doesn't bring the one before it too");
    let cells: Vec<&String> = v["unwritten"][0]["cells"].as_object().unwrap().keys().collect();
    assert_eq!(cells, vec!["balance"], "its cells are its table's, not every node's");

    // A formula reading a cell the table doesn't say is refused, naming the row it's on.
    let why = refused(&c, json!([{ "op": "add_node", "id": "new:x", "name": "Far", "cells": { "value": { "formula": "Year[40].colour" } } }])).await;
    assert!(why.contains("colour on Year[40]"), "{why}");
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
    let tried = c.post("/api/call/try", json!({ "node": id(&v, "Trip"), "text": "travel.chosen.value + 1" })).await;
    assert_eq!(tried["shown"]["cases"].as_array().unwrap().len(), 2);
    assert_eq!(tried["expansion"], "(Travel.chosen.value + 1)");
    let pinned = c.post("/api/call/try", json!({ "node": id(&v, "Trip"), "text": "travel.chosen.value + 1", "world": { id(&v, "Travel"): id(&v, "Plane") } })).await;
    assert_eq!(pinned["shown"]["cases"][0]["value"], 121.0);
    let bad = c.post("/api/call/try", json!({ "node": id(&v, "Trip"), "text": "nope" })).await;
    assert!(bad["error"].as_str().unwrap().contains("no input called nope"), "{bad}");
    assert_eq!(c.view(json!({})).await["last_seq"], v["last_seq"]);
}

#[tokio::test]
async fn a_drawing_is_added_undone_and_refused_like_anything_else() {
    let c = Client::new();
    c.open("australia").await;
    let done = c.post("/api/call/apply", json!({ "ops": [{ "op": "add_node", "id": "new:d", "name": "Mine", "cells": { "drawing": "export function draw() { return \"\"; }" } }], "why": "a second way to see it" })).await;
    let d = done["created"]["new:d"].as_str().unwrap().to_string();
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Mine", "drawing")["value"], "export function draw() { return \"\"; }");
    assert_eq!(cell(&v, "Map of Australia", "drawing")["source"], "own", "and there can be several");
    assert_eq!(v["worlds"]["count"], "18");

    let why = refused(&c, json!([{ "op": "set", "id": d, "cell": "drawing", "value": 3 }])).await;
    assert!(why.contains("malformed_cell"), "a drawing is text: {why}");

    let (_, history) = (StatusCode::OK, c.history(None).await);
    c.post("/api/call/undo", json!({ "seq": history[0]["seq"] })).await;
    assert!(c.view(json!({})).await["nodes"].as_array().unwrap().iter().all(|n| n["name"] != "Mine"));
}

#[tokio::test]
async fn a_hand_edit_nobody_has_noticed_yet_is_brought_in_before_a_change() {
    let c = Client::new();
    c.open("invoice").await;
    let v = c.view(json!({})).await;
    let path = format!("{}/nodes/{}.yaml", c.dir().await, id(&v, "Tax rate"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("value: 0.2", "value: 0.7")).unwrap();

    // Straight away, before anything watching the files could have looked.
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": "Widgets: price", "cell": "value", "value": 11.0 }] })).await;
    let history = c.history(None).await;
    assert_eq!(history[1]["why"], "external edit", "it was recorded first, not lost: {history}");
    assert_eq!(cell(&c.view(json!({})).await, "Tax rate", "value")["value"], 0.7);
    assert!(std::fs::read_to_string(&path).unwrap().contains("value: 0.7"));
}

/// Taking a component away takes what was written in its cells, so nothing is left without its
/// component, and undoing it brings both back.
#[tokio::test]
async fn taking_a_component_away_takes_its_cells() {
    let c = Client::new();
    c.open("calibration").await;
    c.post("/api/call/apply", json!({ "ops": [{ "op": "remove", "id": "Board detector", "component": "task" }], "why": "not a task" })).await;
    let v = c.view(json!({})).await;
    assert!(cell(&v, "Board detector", "estimate").is_null(), "{}", node(&v, "Board detector"));
    let (_, history) = (StatusCode::OK, c.history(None).await);
    c.post("/api/call/undo", json!({ "seq": history[0]["seq"] })).await;
    assert_eq!(cell(&c.view(json!({})).await, "Board detector", "estimate")["value"], 4.0);
}

/// Labels said with one formula replace that formula's names, and leave alone what another formula on
/// the same node uses.
#[tokio::test]
async fn a_formulas_labels_leave_another_formulas_alone() {
    let c = Client::new();
    c.open("case-file").await;
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "id": "Clara", "cell": "arrives", "formula": "1 + 1", "labels": {} }] })).await;
    let v = c.view(json!({})).await;
    assert_eq!(cell(&v, "Clara", "arrives")["value"], 2.0);
    assert!(cell(&v, "Clara", "from_garden")["error"].is_null(), "from_garden still reads garden: {}", cell(&v, "Clara", "from_garden"));
}

/// How far each answer is believed is what makes the worlds, so a weight that differs between them is
/// an error in what the decision lands on, rather than quietly one of its values.
#[tokio::test]
async fn a_weight_that_differs_between_worlds_is_an_error() {
    let c = Client::new();
    c.open("case-file").await;
    c.post("/api/call/apply", json!({ "ops": [{ "op": "set", "from": "Tom", "to": "Who took it?", "cell": "weight", "formula": "if(from.arrives > 40, 3, 1)" }] })).await;
    let why = cell(&c.view(json!({})).await, "Who took it?", "chosen")["error"].as_str().unwrap_or_default().to_string();
    assert!(why.contains("differs between worlds"), "{why}");
}
