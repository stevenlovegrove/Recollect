//! Identity and cell forms, using the core directly: entities are handles, a name is just a cell,
//! and JSON under a name is what a cell really is. A Rust type is a shortcut.

use std::collections::BTreeMap;
use std::sync::Arc;

use recollect_graph::worlds::{Bool, Cond, Space};
use recollect_graph::{Cell, Computed, Cx, Entity, Error, Graph, Name, Registry, cell};
use serde_json::{Value, json};

#[derive(Copy)]
#[cell]
struct Amount(f64);

const ADDEND: &str = "addend";

fn sum(cx: &Cx) -> Cell<Amount> {
    let mut total = 0.0;
    for from in cx.sources(ADDEND) {
        total += cx.get::<Amount>(from)?.map_or(0.0, |a| a.0);
    }
    Ok(Some(Amount(total)))
}

/// Only strings and JSON cross this line, standing in for a TypeScript function.
fn runtime_sum(cx: &Cx) -> Cell<Value> {
    let mut total = 0.0;
    for (from, _) in cx.incoming(ADDEND) {
        total += cx.get_value(from, "amount")?.and_then(|v| v.as_f64()).unwrap_or(0.0);
    }
    Ok(Some(json!(total)))
}

fn count(cx: &Cx) -> Cell<Value> {
    Ok(Some(json!(cx.outgoing("ships_with").len())))
}

/// A graph whose formulas are names of functions here: `sum` gives a typed cell, and the others JSON.
fn graph() -> Graph {
    let mut reg = Registry::new();
    reg.free_formulas(Arc::new(|text| match text {
        "sum" => Ok(Computed::typed("sum", sum)),
        "runtime_sum" => Ok(Computed::json("runtime_sum", runtime_sum)),
        "count" => Ok(Computed::json("count", count)),
        _ => Err(format!("there's no function called {text}")),
    }));
    Graph::with_registry(Arc::new(reg))
}

#[test]
fn a_sheet_with_edges_built_from_handles() {
    let mut g = graph();
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.set_formula(total, "amount", "sum");
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(0.0)), "no edges yet, so nothing to add");
    g.link(a, total, ADDEND);
    g.link(b, total, ADDEND);
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(3.0)));
}

#[test]
fn a_chain_records_its_formulas_and_an_update_propagates_along_it() {
    let mut g = graph();
    let (a, b, c, d) = (g.spawn(), g.spawn(), g.spawn(), g.spawn());
    let name = move |e: Entity| ["a", "b", "c", "d"][[a, b, c, d].iter().position(|x| *x == e).unwrap()];

    // a → b → c → d. Recording formulas and edges runs nothing.
    g.set(a, Amount(1.0));
    g.set_formula(b, "amount", "sum").set_formula(c, "amount", "sum").set_formula(d, "amount", "sum");
    g.link(a, b, ADDEND);
    g.link(b, c, ADDEND);
    g.link(c, d, ADDEND);
    assert!(g.take_ran().is_empty(), "nothing has been computed yet");

    assert_eq!(g.get::<Amount>(d).unwrap(), Some(Amount(1.0)));
    let order: Vec<&str> = g.take_ran().into_iter().map(|(e, _)| name(e)).collect();
    assert_eq!(order, vec!["d", "c", "b"], "each started in turn as the one above asked for it");

    // Update a. Again nothing runs until something is read.
    g.set(a, Amount(5.0));
    assert!(g.take_ran().is_empty());

    // Reading the end finds what's stale, from the source forwards.
    assert_eq!(g.get::<Amount>(d).unwrap(), Some(Amount(5.0)));
    let order: Vec<&str> = g.take_ran().into_iter().map(|(e, _)| name(e)).collect();
    println!("after updating a, reading d ran: {order:?}");
    assert_eq!(order, vec!["b", "c", "d"], "the change travels down the chain");

    // Reading part way along only brings that part up to date.
    g.set(a, Amount(6.0));
    assert_eq!(g.get::<Amount>(b).unwrap(), Some(Amount(6.0)));
    let order: Vec<&str> = g.take_ran().into_iter().map(|(e, _)| name(e)).collect();
    assert_eq!(order, vec!["b"]);
    assert_eq!(g.get::<Amount>(d).unwrap(), Some(Amount(6.0)));
    let order: Vec<&str> = g.take_ran().into_iter().map(|(e, _)| name(e)).collect();
    assert_eq!(order, vec!["c", "d"], "and the rest catches up when asked");
}

#[test]
fn a_name_is_not_an_identity() {
    let mut g = graph();
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Name("Widgets".into())).set(b, Name("Widgets".into()));
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.set_formula(total, "amount", "sum");
    g.link(a, total, ADDEND);
    g.link(b, total, ADDEND);
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(3.0)), "two nodes can share a name");
    g.take_ran();

    g.set(a, Name("Sprockets".into()));
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(3.0)));
    assert!(g.take_ran().is_empty(), "and renaming one reruns nothing that doesn't read names");
}

#[test]
fn handles_sort_in_creation_order_and_round_trip() {
    let mut g = Graph::new();
    let made: Vec<Entity> = (0..10_000).map(|_| g.spawn()).collect();
    let listed: Vec<Entity> = g.entities().collect();
    assert_eq!(listed, made, "creation order is the order, so results are repeatable within a run");

    let text = made[0].to_string();
    assert_eq!(text.parse::<Entity>().unwrap(), made[0]);
    assert_eq!(serde_json::from_value::<Entity>(json!(text)).unwrap(), made[0]);
}

#[test]
fn an_edges_handle_comes_from_its_ends() {
    let mut g = Graph::new();
    let (a, b) = (g.spawn(), g.spawn());
    let edge = g.link(a, b, ADDEND);
    assert_eq!(edge, Entity::edge(a, b), "the same whenever those two are linked");
    assert_ne!(edge, Entity::edge(b, a), "and the other way is another edge");
    assert_eq!(g.ends(edge), Some((a, b)));
    assert!(g.is_edge(edge) && !g.contains(edge), "an edge isn't a node");
}

#[test]
fn a_node_with_an_existing_identity_can_be_adopted() {
    let e = Entity::new();
    let mut g = Graph::new();
    g.adopt(e).set(e, Amount(1.0));
    assert_eq!(g.get::<Amount>(e).unwrap(), Some(Amount(1.0)));
    let mut other = Graph::new();
    other.adopt(e);
    assert_eq!(other.get::<Amount>(e).unwrap(), None, "identity is global, and data is per graph");
}

#[test]
#[should_panic(expected = "isn't a node")]
fn a_handle_from_nowhere_is_a_mistake() {
    let mut g = Graph::new();
    g.set(Entity::new(), Amount(1.0));
}

// ---------- cells by name: a typed lookup is just a shortcut ----------

#[test]
fn typed_and_by_name_are_two_views_of_the_same_cell() {
    let mut g = Graph::new();
    let e = g.spawn();

    // Stored typed, read by name.
    g.set(e, Amount(2.5));
    assert_eq!(g.get_value(e, "amount").unwrap(), Some(json!(2.5)));

    // Stored by name, read typed: nothing distinguishes it from having been typed.
    g.set_value(e, "amount", json!(7.0));
    assert_eq!(g.get::<Amount>(e).unwrap(), Some(Amount(7.0)));
    g.take_ran();
    g.set(e, Amount(7.0));
    assert_eq!(g.get_value(e, "amount").unwrap(), Some(json!(7.0)), "the same value, whichever way it arrived");
}

#[test]
fn a_formula_with_no_rust_type_is_readable_typed_and_the_other_way_round() {
    let mut g = graph();
    let (a, b, c) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Amount(4.0)).set(b, Amount(5.0));
    g.set_value(a, "supplier", json!({ "days": 12 }));
    g.link(a, c, ADDEND);
    g.link(b, c, ADDEND);

    g.set_formula(c, "amount", "runtime_sum");
    assert_eq!(g.get::<Amount>(c).unwrap(), Some(Amount(9.0)), "a typed reader can read this as its own type");

    let typed_reader = g.spawn();
    g.link(c, typed_reader, ADDEND);
    g.set_formula(typed_reader, "amount", "sum");
    assert_eq!(g.get::<Amount>(typed_reader).unwrap(), Some(Amount(9.0)));
    assert_eq!(g.get_value(typed_reader, "amount").unwrap(), Some(json!(9.0)), "and one by name can read a typed one");

    // Memoised and invalidated like anything else.
    g.take_ran();
    g.get::<Amount>(typed_reader).unwrap();
    assert!(g.take_ran().is_empty());
    g.set(a, Amount(5.0));
    assert_eq!(g.get::<Amount>(typed_reader).unwrap(), Some(Amount(10.0)));

    // Cells nothing has a type for are kept, and their shape is nobody else's business.
    assert_eq!(g.get_value(a, "supplier").unwrap().unwrap()["days"], 12);
}

#[test]
fn a_value_of_the_wrong_shape_is_an_error_in_that_cell() {
    let mut g = Graph::new();
    let e = g.spawn();
    g.set_value(e, "amount", json!("not a number"));
    assert!(matches!(g.get::<Amount>(e), Err(Error::Malformed { .. })));
    assert_eq!(g.get_value(e, "amount").unwrap(), Some(json!("not a number")), "the data itself is untouched");
}

#[test]
fn edges_by_name_are_edges() {
    let mut g = graph();
    let (a, b) = (g.spawn(), g.spawn());
    let edge = g.link(a, b, "ships_with");
    g.set_value(edge, "crate", json!(3));
    g.set_formula(a, "n", "count");
    assert_eq!(g.get_value(a, "n").unwrap(), Some(json!(1)));
    assert_eq!(g.get_value(edge, "crate").unwrap(), Some(json!(3)), "and an edge holds cells as a node does");
}

// ---------- every cell is JSON, including values that differ between worlds ----------

#[cell]
struct Budget(Cond<f64>);

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(v: &T) -> T {
    // Through text, as it would be on the way to another language.
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

#[test]
fn world_values_survive_json_exactly() {
    let s = Space::global();
    let a = s.uniform("json_a", &["x", "y", "z"]);
    let b = s.uniform("json_b", &["p", "q"]);

    let cases = [s.always(), s.never(), a.is("x"), &a.is("x") | &a.is("z"), &(&a.is("x") & &b.is("p")) | &b.is("q"), !&a.is("y")];
    for f in &cases {
        let back: Bool = round_trip(f);
        assert!(back == *f, "{} became {}", f.show(), back.show());
    }

    let c = a.value().zip(&b.value(), |x, y| (x.len() + y.len()) as f64 + if x == "x" { 1.5 } else { 0.0 });
    let back: Cond<f64> = round_trip(&c);
    assert!(back == c);
    assert_eq!(*back.at(&BTreeMap::from([("json_a".to_string(), "x".to_string()), ("json_b".to_string(), "p".to_string())])), 3.5);
}

#[test]
fn a_world_value_can_be_read_by_name_and_returned_by_a_formula_with_no_rust_type() {
    let v = Space::global().uniform("json_c", &["cheap", "dear"]);
    let mut reg = Registry::new();
    reg.free_formulas(Arc::new(move |text| {
        let v = v.clone();
        match text {
            "typed" => Ok(Computed::typed("typed", move |_| Ok(Some(Budget(v.value().map(|o| if o == "cheap" { 10.0 } else { 20.0 })))))),
            // This gets the world value as JSON, and hands one back: no shared memory.
            "produce" => Ok(Computed::json("produce", move |_| Ok(Some(serde_json::to_value(Budget(v.value().map(|o| if o == "cheap" { 1.0 } else { 2.0 }))).unwrap())))),
            _ => Err(format!("there's no function called {text}")),
        }
    }));
    let mut g = Graph::with_registry(Arc::new(reg));
    let (typed, runtime) = (g.spawn(), g.spawn());
    g.set_formula(typed, "budget", "typed");
    g.set_formula(runtime, "budget", "produce");

    let as_json = g.get_value(typed, "budget").unwrap().unwrap();
    let decoded: Budget = serde_json::from_value(as_json).unwrap();
    assert!(decoded == g.get::<Budget>(typed).unwrap().unwrap(), "JSON decodes to the very same value");

    let typed_view = g.get::<Budget>(runtime).unwrap().unwrap().0;
    assert_eq!((typed_view.min(), typed_view.max()), (1.0, 2.0), "and a typed reader can use what a JSON formula returned");
}

#[test]
fn a_world_value_means_the_same_in_a_space_with_a_different_variable_order() {
    let s = Space::global();
    let (a, b) = (s.uniform("order_a", &["x", "y", "z"]), s.uniform("order_b", &["p", "q"]));
    let f = &(&a.is("x") & &b.is("p")) | &(&a.is("z") & &b.is("q"));
    let text = serde_json::to_string(&f).unwrap();

    let mut truth = Vec::new();
    for x in ["x", "y", "z"] {
        for y in ["p", "q"] {
            let w = BTreeMap::from([("order_a".to_string(), x.to_string()), ("order_b".to_string(), y.to_string())]);
            truth.push((w.clone(), f.at(&w)));
        }
    }

    // A separate space stands in for another process. Declaring the variables in the opposite
    // order there means a copied table would be wrong.
    let other = Space::new();
    other.uniform("order_b", &["p", "q"]);
    other.uniform("order_a", &["x", "y", "z"]);
    let back = Bool::from_json_in(&other, &serde_json::from_str(&text).unwrap()).unwrap();
    for (w, want) in truth {
        assert_eq!(back.at(&w), want, "{w:?}");
    }
}

/// Writing out where something holds lists the ways it does, and an odd number of eight switches being
/// on holds 128 ways, which nobody reads. Past a couple of dozen, it says how many more there are.
#[test]
fn a_condition_that_holds_many_ways_is_written_out_only_so_far() {
    let s = Space::global();
    let vars: Vec<_> = (0..8).map(|i| s.uniform(&format!("many_{i}"), &["off", "on"])).collect();
    let odd = vars.iter().fold(s.never(), |acc, v| &(&acc & &v.is("off")) | &(&!&acc & &v.is("on")));
    let text = odd.show();
    assert_eq!(text.matches(" ∨ ").count(), 24, "24 ways, and the rest counted: {text}");
    assert!(text.ends_with(" ∨ … 104 more ways"), "{text}");
    let few = &vars[0].is("on") | &vars[1].is("on");
    assert_eq!(few.show(), "(many_0=off ∧ many_1=on) ∨ many_0=on", "and a short one is written out whole");
}
