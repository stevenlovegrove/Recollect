//! A node takes the workspace's own cells it doesn't say for itself from its kind: what it links to
//! by `is_a`. A rule taken that way runs for the node taking it, and follows links from where it's
//! written as well. What the systems own is theirs alone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod support;

use recollect_graph::{Cell, Cx, Entity, Error, Graph, Inputs, Place, Registry, Runtime};
use serde_json::{Value, json};
use support::N;

const IS_A: &str = "is_a";

/// Its own base, plus the amounts of whatever the node it's written on feeds from. Clamped, so
/// different inputs often give the same answer and early cut-off gets exercised.
fn sum(_component: &str, _params: &Value) -> Result<(Inputs, Arc<dyn Fn(&Cx) -> Cell<Value> + Send + Sync>), String> {
    let inputs = Inputs { outgoing: vec!["feeds".to_string()], ..Inputs::default() };
    Ok((
        inputs,
        Arc::new(|cx: &Cx| {
            let number = |c: Option<Runtime>| c.map_or(0, |r| r.value.as_i64().expect("a number"));
            let mut total = number(cx.get_runtime(cx.id(), "base")?);
            for (t, _) in cx.outgoing_from_origin("feeds")? {
                total += number(cx.get_runtime(t, "amount")?);
            }
            Ok(Some(json!(total.clamp(-3, 3))))
        }),
    ))
}

fn install(reg: &mut Registry) {
    reg.rule_on_unowned("kinds/sum", sum).inherit_through(IS_A).declare("owned", Place::Node, None);
}

fn kind(g: &mut N, of: &str, kind: &str) {
    let (a, b) = (g.e(of), g.e(kind));
    g.g.link_runtime(a, b, Runtime::new(IS_A, Value::Null));
}

fn unkind(g: &mut N, of: &str, kind: &str) {
    let (a, b) = (g.id(of), g.id(kind));
    g.g.unlink_named(a, b, IS_A);
}

fn put(g: &mut N, name: &str, component: &str, v: Value) {
    let e = g.e(name);
    g.g.set_runtime(e, component, v);
}

fn rule(g: &mut N, name: &str) {
    let e = g.e(name);
    g.g.set_rule(e, "amount", "kinds/sum", Value::Null);
}

fn read(g: &N, name: &str, component: &str) -> Option<Value> {
    g.g.get_runtime(g.id(name), component).unwrap().map(|r| r.value)
}

#[test]
fn a_node_takes_what_it_doesnt_say_from_its_kind_and_its_own_word_wins() {
    let mut g = N::with(install);
    put(&mut g, "Car", "price", json!(5));
    put(&mut g, "Car", "owned", json!(1));
    g.e("Hatchback");
    assert_eq!(read(&g, "Hatchback", "price"), None);

    kind(&mut g, "Hatchback", "Car");
    let (car, hatch) = (g.id("Car"), g.id("Hatchback"));
    assert_eq!(read(&g, "Hatchback", "price"), Some(json!(5)));
    assert!(g.g.components_of(hatch).contains("price"));
    assert_eq!(g.g.kind_of(hatch, "price"), Some(car));
    assert!(!g.g.has_own(hatch, "price"), "it's taken, not copied");
    assert_eq!(read(&g, "Hatchback", "owned"), None, "what a system owns isn't the kind's to say");
    assert!(!g.g.components_of(hatch).contains("owned"));

    put(&mut g, "Hatchback", "price", json!(7));
    assert_eq!(read(&g, "Hatchback", "price"), Some(json!(7)));
    assert_eq!(g.g.kind_of(hatch, "price"), None);
    g.g.remove_named(hatch, "price");
    assert_eq!(read(&g, "Hatchback", "price"), Some(json!(5)), "and without its own, the kind's comes back");

    put(&mut g, "Car", "price", json!(6));
    assert_eq!(read(&g, "Hatchback", "price"), Some(json!(6)));
    unkind(&mut g, "Hatchback", "Car");
    assert_eq!(read(&g, "Hatchback", "price"), None);

    kind(&mut g, "Hatchback", "Car");
    let saved = g.g.save().unwrap();
    let mine = saved.nodes.iter().find(|n| n.id == hatch).unwrap();
    assert!(!mine.components.contains_key("price"), "what's taken from a kind isn't written down twice");
    g.assert_round_trips();
}

#[test]
fn a_kinds_rule_runs_for_each_of_its_kind_and_follows_links_from_the_kind() {
    let mut g = N::with(install);
    put(&mut g, "Miles", "amount", json!(2));
    rule(&mut g, "Car");
    let (car, miles) = (g.e("Car"), g.id("Miles"));
    g.g.link_runtime(car, miles, Runtime::new("feeds", Value::Null));
    put(&mut g, "Hatchback", "base", json!(1));
    put(&mut g, "Electric", "base", json!(-1));
    kind(&mut g, "Hatchback", "Car");
    kind(&mut g, "Electric", "Car");
    assert_eq!(read(&g, "Car", "amount"), Some(json!(2)));
    assert_eq!(read(&g, "Hatchback", "amount"), Some(json!(3)), "its own base, and the miles its kind reads");
    assert_eq!(read(&g, "Electric", "amount"), Some(json!(1)));
    assert_eq!(g.g.saved_rule(g.id("Hatchback"), "amount").unwrap().rule, "kinds/sum", "it says what it's worked out by");

    put(&mut g, "Miles", "amount", json!(1));
    assert_eq!((read(&g, "Hatchback", "amount"), read(&g, "Electric", "amount")), (Some(json!(2)), Some(json!(0))));
    put(&mut g, "Insurance", "amount", json!(1));
    let insurance = g.id("Insurance");
    g.g.link_runtime(car, insurance, Runtime::new("feeds", Value::Null));
    assert_eq!(read(&g, "Hatchback", "amount"), Some(json!(3)), "a new link on the kind reaches every one of its kind");

    // Nothing links the electric car to the miles, and it may read them all the same: the rule's
    // links are its kind's.
    let other = g.e("Other");
    assert_eq!(g.g.get_runtime(other, "amount"), Ok(None));
    g.assert_round_trips();
}

#[test]
fn a_kind_can_have_a_kind_and_a_loop_of_kinds_ends() {
    let mut g = N::with(install);
    put(&mut g, "Vehicle", "wheels", json!(4));
    put(&mut g, "Car", "doors", json!(5));
    kind(&mut g, "Car", "Vehicle");
    kind(&mut g, "Hatchback", "Car");
    assert_eq!((read(&g, "Hatchback", "wheels"), read(&g, "Hatchback", "doors")), (Some(json!(4)), Some(json!(5))));
    put(&mut g, "Car", "wheels", json!(3));
    assert_eq!(read(&g, "Hatchback", "wheels"), Some(json!(3)), "the nearest kind that says it");

    kind(&mut g, "Vehicle", "Hatchback");
    assert_eq!(read(&g, "Vehicle", "doors"), Some(json!(5)), "round the loop, and no further");
    assert_eq!(read(&g, "Hatchback", "nothing"), None);
}

// ---------- memoisation has to stay invisible ----------

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Amount {
    Literal(i64),
    Rule,
}

#[derive(Default, Debug)]
struct State {
    amount: BTreeMap<String, Amount>,
    base: BTreeMap<String, i64>,
    kinds: BTreeSet<(String, String)>,
    feeds: BTreeSet<(String, String)>,
}

fn fresh(s: &State, ids: &[String]) -> N {
    let mut g = N::with(install);
    for id in ids {
        g.e(id);
    }
    for (id, a) in &s.amount {
        match a {
            Amount::Literal(v) => put(&mut g, id, "amount", json!(v)),
            Amount::Rule => rule(&mut g, id),
        }
    }
    for (id, v) in &s.base {
        put(&mut g, id, "base", json!(v));
    }
    for (a, b) in &s.kinds {
        kind(&mut g, a, b);
    }
    for (a, b) in &s.feeds {
        let (x, y) = (g.id(a), g.id(b));
        g.g.link_runtime(x, y, Runtime::new("feeds", Value::Null));
    }
    g
}

fn outcome(g: &Graph, e: Entity) -> Result<Option<Value>, ()> {
    match g.get_runtime(e, "amount") {
        Ok(v) => Ok(v.map(|r| r.value)),
        Err(Error::Cycle(_)) => Err(()),
        Err(e) => panic!("unexpected {e:?}"),
    }
}

#[test]
fn a_long_lived_graph_with_kinds_always_agrees_with_a_new_one() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let (mut edits, mut inherited, mut cycles) = (0usize, 0usize, 0usize);
    for _ in 0..300 {
        let ids: Vec<String> = (0..8).map(|i| format!("n{i}")).collect();
        let mut state = State::default();
        let mut g = fresh(&state, &ids);
        for _ in 0..60 {
            let (a, b) = (ids[rng.next(8)].clone(), ids[rng.next(8)].clone());
            let ea = g.id(&a);
            match rng.next(9) {
                0 => {
                    let v = rng.next(5) as i64 - 2;
                    state.amount.insert(a.clone(), Amount::Literal(v));
                    put(&mut g, &a, "amount", json!(v));
                }
                1 => {
                    state.amount.insert(a.clone(), Amount::Rule);
                    rule(&mut g, &a);
                }
                2 => {
                    state.amount.remove(&a);
                    g.g.remove_named(ea, "amount");
                }
                3 => {
                    let v = rng.next(3) as i64 - 1;
                    state.base.insert(a.clone(), v);
                    put(&mut g, &a, "base", json!(v));
                }
                4 | 5 if a != b => {
                    // Mostly kinds made before what's of that kind, with the odd loop.
                    let (x, y) = if a > b || rng.next(10) == 0 { (a, b) } else { (b, a) };
                    state.kinds.insert((x.clone(), y.clone()));
                    kind(&mut g, &x, &y);
                }
                6 => {
                    if let Some(l) = state.kinds.iter().nth(rng.next(state.kinds.len().max(1))).cloned() {
                        state.kinds.remove(&l);
                        unkind(&mut g, &l.0, &l.1);
                    }
                }
                7 if a != b => {
                    state.feeds.insert((a.clone(), b.clone()));
                    let eb = g.id(&b);
                    g.g.link_runtime(ea, eb, Runtime::new("feeds", Value::Null));
                }
                8 => {
                    if let Some(l) = state.feeds.iter().nth(rng.next(state.feeds.len().max(1))).cloned() {
                        state.feeds.remove(&l);
                        let (x, y) = (g.id(&l.0), g.id(&l.1));
                        g.g.unlink_named(x, y, "feeds");
                    }
                }
                _ => continue,
            }
            edits += 1;
            if rng.next(100) < 50 {
                continue;
            }
            let new = fresh(&state, &ids);
            for id in ids.iter().filter(|_| rng.next(100) < 50) {
                let (got, want) = (outcome(&g.g, g.id(id)), outcome(&new.g, new.id(id)));
                assert_eq!(got, want, "{id}, with {state:?}");
                cycles += usize::from(want.is_err());
                inherited += usize::from(new.g.kind_of(new.id(id), "amount").is_some());
                assert_eq!(g.g.components_of(g.id(id)), new.g.components_of(new.id(id)), "{id}, with {state:?}");
            }
            if edits % 10 == 0 {
                let (reloaded, problems) = Graph::load(g.g.save().unwrap(), g.registry.clone());
                assert_eq!(problems, vec![]);
                for id in &ids {
                    assert_eq!(outcome(&reloaded, g.id(id)), outcome(&new.g, new.id(id)), "{id} after a round trip");
                }
            }
        }
    }
    println!("{edits} edits, {inherited} answers taken from a kind, {cycles} cycles");
    assert!(inherited > 300, "the edits should actually exercise kinds");
    assert!(cycles > 20, "and loops through them");
}
