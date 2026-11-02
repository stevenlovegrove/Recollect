//! Try the kernel on a real workspace: converts the app's current node files into the kernel
//! model and prints what it derives. Reads only.
//!
//!     cargo run -p recollect-kernel --example demo -- .demo

use std::collections::BTreeMap;

use recollect_kernel::log::{Log, set, unknown_id};
use recollect_kernel::scope::{Scope, ViewGraph};
use recollect_kernel::store::*;
use recollect_kernel::systems::*;
use serde_json::{Value, json};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".demo".into());
    let mut nodes = BTreeMap::new();
    for entry in std::fs::read_dir(format!("{root}/nodes")).expect("workspace") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "md") {
            let text = std::fs::read_to_string(&path).unwrap();
            let front = text.strip_prefix("---\n").and_then(|t| t.split("\n---").next()).expect("frontmatter");
            let v: Value = serde_yaml_ng::from_str(front).unwrap();
            nodes.insert(v["id"].as_str().unwrap().to_string(), v);
        }
    }

    // Conversion: every option becomes a node, and scoped part_of edges point at options.
    let mut ops = Vec::new();
    let mut later = Vec::new();
    for (id, n) in &nodes {
        ops.push(set(e(id), "title", n["title"].clone()));
        if matches!(n["status"].as_str(), Some("dropped" | "superseded")) {
            ops.push(set(e(id), DROPPED, json!({})));
        }
        if let Some(a) = n.get("actionable") {
            ops.push(set(e(id), TASK, json!({ "estimate": a.get("estimate").filter(|v| v.is_number()) })));
            if a["state"] == "done" {
                ops.push(set(e(id), DONE, json!({})));
            }
        }
        if n.get("outcome").is_some() {
            ops.push(set(e(id), "goal", json!({})));
        }
        if let Some(u) = n.get("uncertain") {
            ops.push(set(e(id), "question", json!({})));
            if u["state"] != "open" {
                ops.push(set(e(id), DONE, json!({})));
            }
        }
        if n.get("risk").is_some() {
            ops.push(set(e(id), "risk", json!({})));
        }
        if n.get("principle").is_some() {
            ops.push(set(e(id), "principle", json!({})));
        }
        if n.get("artifact").is_some() {
            ops.push(set(e(id), "artifact", json!({})));
        }
        if let Some(c) = n.get("choice") {
            let chosen = c["options"].as_object().unwrap().iter().find(|(_, o)| o["state"] == "chosen").map(|(k, _)| format!("{id}.{k}"));
            for (k, o) in c["options"].as_object().unwrap() {
                let opt = format!("{id}.{k}");
                ops.push(set(e(&opt), "title", o["title"].clone()));
                let dropped_without_choice = o["state"] == "dropped";
                let alt = if dropped_without_choice { json!({ "ruled_out": {} }) } else { json!({ "weight": o.get("confidence").cloned().unwrap_or(json!(1)) }) };
                later.push(set(l(&opt, id), ALTERNATIVE, alt));
            }
            // An open decision has a gap until someone says its options are complete.
            if chosen.is_none() {
                let gap = unknown_id(id);
                ops.push(set(e(&gap), UNKNOWN, json!({})));
                later.push(set(l(&gap, id), ALTERNATIVE, json!({})));
            }
            ops.push(set(e(id), DECISION, chosen.map_or(json!({}), |c| json!({ "chosen": c }))));
        }
        for link in n["links"].as_array().into_iter().flatten() {
            let (rel, to) = (link["rel"].as_str().unwrap(), link["to"].as_str().unwrap());
            match link["options"].as_array() {
                Some(opts) if rel == "part_of" => {
                    for o in opts {
                        later.push(set(l(id, &format!("{to}.{}", o.as_str().unwrap())), PART_OF, json!({})));
                    }
                }
                _ => later.push(set(l(id, to), rel, json!({}))),
            }
        }
    }
    let mut log = Log::default();
    // One atomic commit: rules are checked on the result, so order within it doesn't matter.
    ops.extend(later);
    if let Err(v) = log.commit("import", "converted from the current app format", ops) {
        println!("REFUSED: {v:#?}");
        std::process::exit(1);
    }
    let s = &log.store;
    let start = std::time::Instant::now();
    let d = compute(s);
    let took = start.elapsed();

    println!("{} entities, {} links, {} open decisions, derived in {took:?}\n", s.entities.len(), s.links.len(), d.mdd.vars.len());
    for id in s.entities.keys().filter(|id| s.has(id, DECISION)) {
        println!("── {} [{}]", s.entities[id]["title"], s.chosen(id).map_or("open".to_string(), |c| format!("chose {c}")));
        println!("   done: {}   pressure: {:?}", d.show("done", id), pressure(s, &d, id));
        for ((f, t), c) in &s.links {
            if t == id && c.contains_key(ALTERNATIVE) {
                let out = if c[ALTERNATIVE].get("ruled_out").is_some() { " (ruled out)" } else { "" };
                println!("   option {f}{out}: done {}", d.show("done", f));
            }
        }
    }
    println!("\nsafe to start now (needed in every future, nothing in the way):");
    for id in d.ids_where("actionable", |f| f == recollect_kernel::formula::TRUE) {
        println!("   {id}");
    }
    println!("\nsafe among the options named so far (only a gap could change it):");
    for (id, f) in d.cells["actionable"].iter().filter(|(_, f)| **f > recollect_kernel::formula::TRUE && d.mdd.holds_among_known(**f)) {
        println!("   {id}: {}", d.mdd.show(*f));
    }
    println!("\ncontingent on open decisions:");
    for (id, f) in d.cells["actionable"].iter().filter(|(_, f)| **f > recollect_kernel::formula::TRUE) {
        println!("   {id}: {} ({:.0}%)", d.mdd.show(*f), d.mdd.prob(*f) * 100.0);
    }
    println!("\nblocked in every world:");
    let stuck: Vec<String> = d.cells["actionable"].iter().filter(|(id, f)| **f == recollect_kernel::formula::FALSE && d.possible("need", id) && !d.holds("done", id)).map(|(id, _)| id.clone()).collect();
    for id in stuck {
        let by = blockers(&d, &id);
        let names: Vec<String> = by.iter().map(|(b, f)| format!("{b} ({})", d.mdd.show(*f))).collect();
        println!("   {id}: waits for {}", names.join(", "));
    }
    println!("\nnotices:");
    for n in &d.notices {
        println!("   {} {}: {}", n.rule, n.id, n.detail);
    }
    println!("\nmalformed: {:?}\ncycles: {:?}", d.malformed, d.cycles);

    // Scope: everything folds under where it belongs.
    let live = ViewGraph::of(s).retain(|id| s.live(id));
    let top = live.collapse(&d, &Default::default());
    println!("\ntop level: {} of {} live nodes visible, {} of {} links", top.nodes.len(), live.nodes.len(), top.edges.len(), live.edges.len());
    for (id, n) in &top.nodes {
        println!("   {id} (+{} inside)", n.hidden);
    }
    let shared: Vec<(&String, &Option<String>)> = s.entities.keys().filter(|id| s.parents(id).len() > 1).map(|id| (id, d.of::<Scope>(id).unwrap())).collect();
    println!("\nnodes with several parents, and the scope they land in:");
    for (id, scope) in shared {
        println!("   {id}: {} parents → {}", s.parents(id).len(), scope.as_deref().unwrap_or("top level"));
    }
    println!("\nexample explanation:\n{}", explain(s, &d, "calib"));
}
