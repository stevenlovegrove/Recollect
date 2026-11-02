//! Identity and component forms, using the core directly: entities are handles, a name is just
//! a component, and JSON under a name is what a component really is. A Rust type is a shortcut.

use std::collections::BTreeMap;

use recollect_graph::worlds::{Bool, Cond, Space};
use recollect_graph::{Entity, Error, Graph, Name, Rule, Runtime, component, rule};
use serde_json::{Value, json};

#[derive(Copy)]
#[component]
struct Amount(f64);

#[component]
struct Addend;

fn sum() -> Rule<Amount> {
    rule("sum").incoming::<Addend>().compute(|cx| {
        let mut total = 0.0;
        for from in cx.sources::<Addend>()? {
            total += cx.get::<Amount>(from)?.map_or(0.0, |a| a.0);
        }
        Ok(Amount(total))
    })
}

#[test]
fn a_sheet_with_edges_built_from_handles() {
    let mut g = Graph::new();
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.set(total, sum());
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(0.0)), "no edges yet, so nothing to add");
    g.link(a, total, Addend).link(b, total, Addend);
    assert_eq!(g.get::<Amount>(total).unwrap(), Some(Amount(3.0)));
}

#[test]
fn a_chain_records_its_rules_and_an_update_propagates_along_it() {
    let mut g = Graph::new();
    let (a, b, c, d) = (g.spawn(), g.spawn(), g.spawn(), g.spawn());
    let name = move |e: Entity| ["a", "b", "c", "d"][[a, b, c, d].iter().position(|x| *x == e).unwrap()];

    // a → b → c → d. Recording rules and edges runs nothing.
    g.set(a, Amount(1.0));
    g.set(b, sum()).set(c, sum()).set(d, sum());
    g.link(a, b, Addend).link(b, c, Addend).link(c, d, Addend);
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
    let mut g = Graph::new();
    let (a, b, total) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Name("Widgets".into())).set(b, Name("Widgets".into()));
    g.set(a, Amount(1.0)).set(b, Amount(2.0));
    g.set(total, sum()).link(a, total, Addend).link(b, total, Addend);
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

// ---------- runtime components: a typed lookup is just a shortcut ----------

#[test]
fn typed_and_runtime_are_two_views_of_the_same_cell() {
    let mut g = Graph::new();
    let e = g.spawn();

    // Stored typed, read by name.
    g.set(e, Amount(2.5));
    assert_eq!(g.get_runtime(e, "amount").unwrap(), Some(Runtime::new("amount", json!(2.5))));

    // Stored by name, read typed: nothing distinguishes it from having been typed.
    g.set_runtime(e, "amount", json!(7.0));
    assert_eq!(g.get::<Amount>(e).unwrap(), Some(Amount(7.0)));
    g.take_ran();
    g.set(e, Amount(7.0));
    assert_eq!(g.get_runtime(e, "amount").unwrap().unwrap().value, json!(7.0), "the same value, whichever way it arrived");
}

#[test]
fn a_rule_with_no_rust_type_is_readable_typed_and_the_other_way_round() {
    let mut g = Graph::new();
    let (a, b, c) = (g.spawn(), g.spawn(), g.spawn());
    g.set(a, Amount(4.0)).set(b, Amount(5.0));
    g.set_runtime(a, "supplier", json!({ "days": 12 }));
    g.link(a, c, Addend).link(b, c, Addend);

    // Only strings and JSON cross this line, standing in for a TypeScript function.
    let runtime_sum = rule("runtime_sum").incoming_named("addend").compute::<Value>(|cx| {
        let mut total = 0.0;
        for (from, _) in cx.incoming_runtime("addend")? {
            total += cx.get_runtime(from, "amount")?.and_then(|r| r.value.as_f64()).unwrap_or(0.0);
        }
        Ok(json!(total))
    });
    g.set_runtime(c, "amount", runtime_sum);
    assert_eq!(g.get::<Amount>(c).unwrap(), Some(Amount(9.0)), "a typed rule can read this as its own type");

    let typed_reader = g.spawn();
    g.link(c, typed_reader, Addend);
    g.set(typed_reader, sum());
    assert_eq!(g.get::<Amount>(typed_reader).unwrap(), Some(Amount(9.0)));
    assert_eq!(g.get_runtime(typed_reader, "amount").unwrap().unwrap().value, json!(9.0), "and a runtime one can read a typed rule");

    // Memoised and invalidated like anything else.
    g.take_ran();
    g.get::<Amount>(typed_reader).unwrap();
    assert!(g.take_ran().is_empty());
    g.set(a, Amount(5.0));
    assert_eq!(g.get::<Amount>(typed_reader).unwrap(), Some(Amount(10.0)));

    // Components nothing has a type for are kept, and their shape is nobody else's business.
    assert_eq!(g.get_runtime(a, "supplier").unwrap().unwrap().value["days"], 12);
}

#[test]
fn a_runtime_value_of_the_wrong_shape_is_an_error_in_that_cell() {
    let mut g = Graph::new();
    let e = g.spawn();
    g.set_runtime(e, "amount", json!("not a number"));
    assert!(matches!(g.get::<Amount>(e), Err(Error::Malformed { .. })));
    assert_eq!(g.get_runtime(e, "amount").unwrap().unwrap().value, json!("not a number"), "the data itself is untouched");
}

#[test]
fn runtime_links_are_links() {
    let mut g = Graph::new();
    let (a, b) = (g.spawn(), g.spawn());
    g.link_runtime(a, b, Runtime::new("ships_with", json!({ "crate": 3 })));
    let count = rule("count").outgoing_named("ships_with").compute::<Value>(|cx| Ok(json!(cx.outgoing_runtime("ships_with")?.len())));
    g.set_runtime(a, "n", count);
    assert_eq!(g.get_runtime(a, "n").unwrap().unwrap().value, json!(1));
}

#[test]
fn a_runtime_rule_is_held_to_its_declared_links_too() {
    let mut g = Graph::new();
    let a = g.spawn();
    let sneaky = rule("sneaky").compute::<Value>(|cx| Ok(json!(cx.incoming_runtime("addend")?.len())));
    g.set_runtime(a, "n", sneaky);
    assert!(matches!(g.get_runtime(a, "n"), Err(Error::Undeclared { .. })));
}

// ---------- every component is JSON, including values that differ between worlds ----------

#[component]
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
fn a_world_value_can_be_read_by_name_and_returned_by_a_runtime_rule() {
    let mut g = Graph::new();
    let (typed, runtime) = (g.spawn(), g.spawn());
    let v = Space::global().uniform("json_c", &["cheap", "dear"]);
    let (v2, v3) = (v.clone(), v.clone());
    g.set(typed, rule("typed").compute(move |_| Ok(Budget(v2.value().map(|o| if o == "cheap" { 10.0 } else { 20.0 })))));

    // A runtime rule gets the world value as JSON, and hands one back: no shared memory.
    let produce = rule("produce").compute::<Value>(move |_| {
        Ok(serde_json::to_value(Budget(v3.value().map(|o| if o == "cheap" { 1.0 } else { 2.0 }))).unwrap())
    });
    g.set_runtime(runtime, "budget", produce);

    let as_json = g.get_runtime(typed, "budget").unwrap().unwrap();
    let decoded: Budget = as_json.decode().unwrap();
    assert!(decoded == g.get::<Budget>(typed).unwrap().unwrap(), "JSON decodes to the very same value");

    let typed_view = g.get::<Budget>(runtime).unwrap().unwrap().0;
    assert_eq!((typed_view.min(), typed_view.max()), (1.0, 2.0), "and a typed reader can use what a runtime rule returned");
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
