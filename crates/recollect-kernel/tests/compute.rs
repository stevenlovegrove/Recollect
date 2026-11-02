//! The compute model and views, from outside the crate, using only what a plugin can use.

use std::collections::{BTreeMap, BTreeSet};

use recollect_kernel::compute::*;
use recollect_kernel::formula::{FALSE, TRUE};
use recollect_kernel::log::{Log, choose, close_gap, new_decision, set};
use recollect_kernel::schema::*;
use recollect_kernel::scope::{Scope, ScopeDepth, ViewGraph};
use recollect_kernel::store::*;
use recollect_kernel::systems::{Completed, Need, compute, kernel, pressure, Pressure};
use serde_json::json;

fn commit(ops: Vec<Op>) -> Log {
    let mut log = Log::default();
    log.commit("t", "t", ops).expect("valid");
    log
}

/// Two approaches with shared work, and a gap, with estimates.
fn estimated() -> Log {
    let mut ops = new_decision("calib");
    for o in ["cad", "free"] {
        ops.push(set(e(o), "title", json!(o)));
        ops.push(set(l(o, "calib"), ALTERNATIVE, json!({})));
    }
    for (t, est, parents) in [("imu", 1.0, vec!["calib"]), ("static", 2.0, vec!["cad", "free"]), ("export", 3.0, vec!["cad"]), ("search", 5.0, vec!["free"])] {
        ops.push(set(e(t), TASK, json!({ "estimate": est })));
        for p in parents {
            ops.push(set(l(t, p), PART_OF, json!({})));
        }
    }
    commit(ops)
}

// ---------- unknown: a gap in the graph ----------

#[test]
fn a_gap_keeps_a_decision_open_until_someone_closes_it() {
    let mut log = estimated();
    let d = compute(&log.store);
    assert_eq!(d.show("need", "static"), "calib∈{cad,free}", "safe only among the named options");
    assert!(!d.holds("need", "static") && d.holds_among_known("need", "static"));
    assert!(d.holds("need", "imu"), "needed even if the answer is something else entirely");
    assert!((d.prob("need", "static") - 1.0).abs() < 1e-9, "a gap has no likelihood until given one");
    assert!(d.cells["actionable"].get("calib.unknown").is_none(), "and isn't work");

    // Pressure treats named-option safety as safe, or a default gap would hide all shared work.
    assert_eq!(pressure(&log.store, &d, "calib"), Pressure::Free(vec!["imu".into(), "static".into()]));

    for t in ["imu", "static", "export", "search"] {
        log.commit("t", "t", vec![set(e(t), DONE, json!({}))]).unwrap();
    }
    assert_eq!(compute(&log.store).show("done", "calib"), "calib∈{cad,free}", "all known work finished, and it might not be all");

    let seq = log.commit("steven", "these are the only two approaches", close_gap("calib")).unwrap();
    assert!(compute(&log.store).holds("done", "calib"));
    let ev = &log.events[seq];
    assert_eq!((ev.author.as_str(), ev.why.as_str()), ("steven", "these are the only two approaches"), "closing it is an audited claim");

    log.undo(seq, "steven", "actually, maybe not").unwrap();
    assert_eq!(compute(&log.store).show("done", "calib"), "calib∈{cad,free}", "and reversible");
}

#[test]
fn a_gap_cannot_be_done_chosen_or_given_parts() {
    let log = estimated();
    let refused = |ops: Vec<Op>| log.clone().commit("t", "t", ops).unwrap_err()[0].rule;
    assert_eq!(refused(vec![set(e("calib.unknown"), DONE, json!({}))]), "unknown_cannot_be_done");
    assert_eq!(refused(vec![choose("calib", "calib.unknown")]), "unknown_cannot_be_chosen");
    assert_eq!(refused(vec![set(l("imu", "calib.unknown"), PART_OF, json!({}))]), "unknown_has_no_parts");
}

// ---------- typed reads ----------

struct TaskCount;
impl Cell for TaskCount {
    const NAME: &'static str = "task_count";
    type Value = usize;
    fn eval(cx: &Cx, _: &str) -> usize {
        // Enumerating by type: `task` is a real Task, its estimate a real number.
        cx.with::<Task>().iter().filter(|(_, task)| task.estimate.is_some_and(|e| e > 1.0)).count()
    }
}

#[test]
fn enumerating_by_type_yields_the_type() {
    let log = estimated();
    let mut r = kernel();
    r.add::<TaskCount>();
    let d = run(&log.store, &r);
    assert_eq!(*d.of::<TaskCount>("imu").unwrap(), 3);
    assert_eq!(d.values["task_count"]["imu"].cases, vec![(TRUE, json!(3))], "typed cells are readable untyped too");
}

#[test]
fn malformed_values_are_refused_on_write_and_absent_on_read() {
    let mut log = estimated();
    let err = log.commit("t", "t", vec![set(e("imu"), TASK, json!({ "estimate": "soon" }))]).unwrap_err();
    assert_eq!(err[0].rule, "malformed_component");

    // A hand edit gets in anyway. It's recorded and reported, and typed reads never see it.
    let mut disk = log.store.clone();
    disk.entities.get_mut("imu").unwrap().insert(TASK.into(), json!({ "estimate": "soon" }));
    let issues = log.reconcile(&disk, "hand");
    assert_eq!(issues[0].rule, "malformed_component");
    assert_eq!(log.store.entities["imu"][TASK], json!({ "estimate": "soon" }), "the store keeps what's on disk");
    let d = compute(&log.store);
    assert_eq!(d.malformed.len(), 1);
    assert!(d.cells["actionable"].get("imu").is_none(), "as far as compute goes, it isn't a task");
}

#[test]
fn a_type_nobody_declared_still_cannot_panic() {
    #[derive(serde::Deserialize)]
    struct Mine {
        #[allow(dead_code)]
        n: i32,
    }
    impl Component for Mine {
        const NAME: &'static str = "mine";
    }
    struct Reader;
    impl Cell for Reader {
        const NAME: &'static str = "reader";
        type Value = usize;
        fn eval(cx: &Cx, _: &str) -> usize {
            cx.with::<Mine>().len()
        }
    }
    let log = commit(vec![set(e("a"), "mine", json!({ "n": 1 })), set(e("b"), "mine", json!("oops"))]);
    let mut r = kernel();
    r.add::<Reader>();
    let d = run(&log.store, &r);
    assert_eq!(*d.of::<Reader>("a").unwrap(), 1);
    assert_eq!(d.malformed.len(), 1, "{:?}", d.malformed);
}

// ---------- untyped: what a TypeScript system would use ----------

#[test]
fn components_rust_cannot_type_are_kept_readable_and_ignored() {
    let mut log = estimated();
    let before = compute(&log.store);
    log.commit("plugin", "t", vec![set(e("imu"), "review", json!({ "by": "ana", "pages": 12 })), set(l("imu", "search"), "informs", json!({ "strength": 0.5 }))]).unwrap();
    assert_eq!(log.store.entities["imu"]["review"]["by"], "ana", "stored as given");

    let d = compute(&log.store);
    for id in log.store.entities.keys() {
        for cell in ["need", "done", "blocked"] {
            assert_eq!(d.show(cell, id), before.show(cell, id), "the kernel ignores what it doesn't know");
        }
    }
}

#[test]
fn an_untyped_system_can_do_what_a_typed_one_can() {
    let mut log = estimated();
    log.commit("plugin", "t", vec![set(e("export"), "review", json!({ "pages": 12 })), set(e("search"), "review", json!({ "pages": 30 }))]).unwrap();
    let mut r = kernel();
    // Standing in for a TypeScript function: only strings and JSON cross this line.
    r.add_untyped(
        "review_pages",
        |cx| cx.with_raw("review").into_iter().map(|(id, _)| id.to_string()).collect(),
        |cx, id| {
            let pages = cx.raw(id, "review").and_then(|v| v["pages"].as_i64()).unwrap_or(0);
            let need = cx.cell_untyped("need", id).expect("kernel cell");
            cx.map(&need, |needed| if needed == &json!(true) { json!(pages) } else { json!(0) })
        },
    );
    let d = run(&log.store, &r);
    let c = &d.values["review_pages"]["export"];
    let at = |o: &str| c.at(&d.mdd, &BTreeMap::from([("calib", o)])).clone();
    assert_eq!((at("cad"), at("free")), (json!(12), json!(0)));
    assert!(d.values["review_pages"].get("imu").is_none(), "only nodes with a review");
    assert!(d.trace[&("review_pages".to_string(), "export".to_string())].contains(&Dep::Cell("need".into(), "export".into())), "reads are traced the same way");
}

#[test]
fn a_runtime_declaration_gives_an_untyped_component_real_meaning() {
    let log = commit(vec![set(e("doc"), "review", json!({})), set(e("note"), "title", json!("note"))]);
    assert!(compute(&log.store).holds("done", "doc"), "undeclared, it means nothing");
    let mut r = kernel();
    r.declare(ComponentDef::entity("review").obligation());
    let d = run(&log.store, &r);
    assert_eq!(d.get("done", "doc"), FALSE, "declared as work, it has to be finished");
    assert!(d.holds("done", "note"));
}

#[test]
fn a_runtime_declaration_can_add_to_scope() {
    let log = commit(vec![set(e("idea"), "title", json!("idea")), set(e("thought"), "title", json!("thought")), set(l("thought", "idea"), "annotates", json!({}))]);
    assert_eq!(compute(&log.store).of::<Scope>("thought").unwrap(), &None);
    let mut r = kernel();
    r.declare(ComponentDef::link("annotates").scoping());
    assert_eq!(run(&log.store, &r).of::<Scope>("thought").unwrap().as_deref(), Some("idea"));
}

#[test]
fn every_link_on_a_node_is_available() {
    struct Degree;
    impl Cell for Degree {
        const NAME: &'static str = "degree";
        type Value = usize;
        fn eval(cx: &Cx, id: &str) -> usize {
            cx.links_of(id).len()
        }
    }
    let log = commit(vec![set(e("a"), "title", json!("a")), set(e("b"), "title", json!("b")), set(e("c"), "title", json!("c")), set(l("a", "b"), "whatever", json!(1)), set(l("c", "a"), PART_OF, json!({}))]);
    let mut r = kernel();
    r.add::<Degree>();
    assert_eq!(*run(&log.store, &r).of::<Degree>("a").unwrap(), 2, "in and out, known or not");
}

// ---------- values that differ between worlds ----------

/// Remaining effort under a node, with shared work counted once.
struct Remaining;
impl Cell for Remaining {
    const NAME: &'static str = "remaining";
    type Value = Cond<f64>;
    fn eval(cx: &Cx, id: &str) -> Cond<f64> {
        let mut under = BTreeSet::from([id.to_string()]);
        let mut stack = vec![id.to_string()];
        while let Some(cur) = stack.pop() {
            for (child, _) in cx.links_into::<PartOf>(&cur).into_iter().map(|(c, p)| (c, Ok::<_, Alternative>(p))).chain(cx.links_into::<Alternative>(&cur).into_iter().map(|(c, a)| (c, Err(a)))) {
                if under.insert(child.to_string()) {
                    stack.push(child.to_string());
                }
            }
        }
        let terms = cx.with::<Task>().into_iter().filter(|(t, _)| under.contains(*t)).map(|(t, task)| {
            let open = cx.and(cx.cell::<Need>(t), cx.not(cx.cell::<Completed>(t)));
            cx.when(open, &cx.pure(task.estimate.unwrap_or(0.0)), &cx.pure(0.0))
        });
        let terms: Vec<_> = terms.collect();
        cx.sum(terms)
    }
}

#[test]
fn numeric_cells_differ_exactly_between_worlds() {
    let mut log = estimated();
    let mut r = kernel();
    r.add::<Remaining>();
    let d = run(&log.store, &r);
    let c = d.of::<Remaining>("calib").unwrap();
    let at = |o: &str| *c.at(&d.mdd, &BTreeMap::from([("calib", o)]));
    assert_eq!((at("cad"), at("free"), at("calib.unknown")), (6.0, 8.0, 1.0));

    // Guards are mutually exclusive and cover every world.
    let total: f64 = c.cases.iter().map(|(g, _)| d.mdd.prob(*g)).sum();
    assert!((total - 1.0).abs() < 1e-9);

    // Probability-weighted, because that's what weights are for.
    let expected: f64 = c.cases.iter().map(|(g, v)| d.mdd.prob(*g) * v).sum();
    assert!((expected - 7.0).abs() < 1e-9, "{expected}");

    log.commit("t", "t", vec![choose("calib", "free")]).unwrap();
    let d = run(&log.store, &r);
    assert_eq!(d.of::<Remaining>("calib").unwrap().cases, vec![(TRUE, 8.0)], "once decided, one value");
}

// ---------- audit of compute ----------

#[test]
fn the_trace_says_what_each_value_was_computed_from() {
    let log = estimated();
    let d = compute(&log.store);
    let deps = &d.trace[&("done".to_string(), "calib".to_string())];
    assert!(deps.contains(&Dep::Cell("all_required_done".into(), "calib".into())), "{deps:?}");
    let deps = &d.trace[&("all_required_done".to_string(), "calib".to_string())];
    for child in ["imu", "cad", "free", "calib.unknown"] {
        assert!(deps.contains(&Dep::Cell("done".into(), child.into())), "{child}: {deps:?}");
    }
}

#[test]
fn a_cycle_from_a_hand_edit_is_reported_and_nothing_crashes() {
    let mut disk = Store::default();
    disk.apply(&set(e("a"), TASK, json!({}))).unwrap();
    disk.apply(&set(e("b"), TASK, json!({}))).unwrap();
    disk.apply(&set(e("c"), TASK, json!({}))).unwrap();
    disk.apply(&set(l("a", "b"), PART_OF, json!({}))).unwrap();
    disk.apply(&set(l("b", "a"), PART_OF, json!({}))).unwrap();
    let mut log = Log::default();
    let issues = log.reconcile(&disk, "hand");
    assert_eq!(issues[0].rule, "no_deadlock");
    let d = compute(&log.store);
    assert!(!d.cycles.is_empty(), "reported");
    assert!(d.holds("need", "c"), "and the rest of the graph is fine");
    ViewGraph::of(&log.store).collapse(&d, &BTreeSet::new());
}

#[test]
fn a_long_chain_computes() {
    let mut ops = vec![set(e("n0"), "title", json!("top"))];
    for i in 1..20_000 {
        ops.push(set(e(&format!("n{i}")), "title", json!(i)));
        ops.push(set(l(&format!("n{i}"), &format!("n{}", i - 1)), PART_OF, json!({})));
    }
    let log = commit(ops);
    let d = compute(&log.store);
    assert!(d.holds("need", "n19999"));
    assert_eq!(*d.of::<ScopeDepth>("n19999").unwrap(), 19_999);
}

// ---------- scope: where a node belongs ----------

fn scope<'a>(d: &'a Derived, id: &str) -> Option<&'a str> {
    d.of::<Scope>(id).unwrap().as_deref()
}

#[test]
fn scope_follows_the_structure() {
    let log = estimated();
    let d = compute(&log.store);
    assert_eq!(scope(&d, "calib"), None);
    assert_eq!(scope(&d, "export"), Some("cad"), "only one parent: it lives inside it");
    assert_eq!(scope(&d, "cad"), Some("calib"));
    assert_eq!(scope(&d, "static"), Some("calib"), "two options share it: it lives in what they share");
    assert_eq!(*d.of::<ScopeDepth>("export").unwrap(), 2);

    // Add a parent elsewhere and it moves out, with nothing to maintain.
    let mut log = log;
    log.commit("t", "t", vec![set(e("elsewhere"), "title", json!("elsewhere")), set(l("export", "elsewhere"), PART_OF, json!({}))]).unwrap();
    assert_eq!(scope(&compute(&log.store), "export"), None, "two unrelated roots share nothing");
}

#[test]
fn deciding_does_not_move_anything() {
    let mut log = estimated();
    let before: Vec<_> = log.store.entities.keys().map(|id| scope(&compute(&log.store), id).map(String::from)).collect();
    log.commit("t", "t", vec![choose("calib", "cad")]).unwrap();
    let d = compute(&log.store);
    let after: Vec<_> = log.store.entities.keys().map(|id| scope(&d, id).map(String::from)).collect();
    assert_eq!(before, after, "the layout of the graph doesn't jump when you decide");
}

// ---------- views are transforms of a graph ----------

#[test]
fn collapse_folds_scopes_and_redraws_links_to_what_is_visible() {
    let mut log = estimated();
    log.commit("t", "t", vec![set(e("other"), TASK, json!({})), set(l("other", "export"), DEPENDS_ON, json!({}))]).unwrap();
    let d = compute(&log.store);
    let full = ViewGraph::of(&log.store);

    let top = full.collapse(&d, &BTreeSet::new());
    assert_eq!(top.nodes.keys().map(String::as_str).collect::<Vec<_>>(), vec!["calib", "other"]);
    assert_eq!(top.nodes["calib"].hidden, 7, "cad, free, the gap and four tasks");
    let edge = &top.edges[&("other".to_string(), "calib".to_string())];
    assert!(edge.components.contains(DEPENDS_ON), "a link into the fold points at the fold");

    let one = full.collapse(&d, &BTreeSet::from(["calib".to_string()]));
    assert!(one.nodes.contains_key("cad") && one.nodes.contains_key("static") && !one.nodes.contains_key("export"));
    assert_eq!(one.nodes["cad"].hidden, 1);
    assert!(one.edges.contains_key(&("other".to_string(), "cad".to_string())), "it follows the fold in");
}

#[test]
fn within_shows_a_scope_and_its_boundary() {
    let mut log = estimated();
    log.commit("t", "t", vec![set(e("other"), TASK, json!({})), set(l("other", "export"), DEPENDS_ON, json!({}))]).unwrap();
    let d = compute(&log.store);
    let v = ViewGraph::of(&log.store).within(&d, "cad");
    assert_eq!(v.nodes.iter().filter(|(_, n)| !n.external).map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["cad", "export"]);
    assert!(v.nodes["other"].external, "a link reaches in from outside, so it shows, marked");
    assert!(v.nodes["calib"].external);
    assert!(v.nodes["static"].external, "shared work lives above cad, and is visible from it");
}

#[test]
fn transforms_compose_including_with_what_the_decisions_rule_out() {
    let mut log = estimated();
    log.commit("t", "t", vec![choose("calib", "cad")]).unwrap();
    let d = compute(&log.store);
    // Only what's still in play, then folded.
    let v = ViewGraph::of(&log.store).retain(|id| d.possible("need", id));
    assert!(!v.nodes.contains_key("search") && !v.nodes.contains_key("free"));
    let folded = v.collapse(&d, &BTreeSet::new());
    assert_eq!(folded.nodes["calib"].hidden, v.nodes.len() - 1);
    assert_eq!(log.store, estimated_then_cad(), "viewing changes nothing");
}

fn estimated_then_cad() -> Store {
    let mut log = estimated();
    log.commit("t", "t", vec![choose("calib", "cad")]).unwrap();
    log.store
}
