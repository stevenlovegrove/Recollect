//! A node named `Year[3]` is row 3 of the table `Year`, and takes the workspace's own cells it doesn't
//! say from the node named `Year`, as it would from a kind. Rows with the same key in related tables
//! are parts of one thing, and a numbered row can read the one before it. See `table`.

use std::collections::BTreeMap;
use std::sync::Arc;

use recollect_graph::{Cell, Component, Cx, Entity, Error, Graph, Inputs, Name, Place, Registry, Runtime};
use serde_json::{Value, json};

const IS_A: &str = "is_a";

/// Its own base, plus the amount of the row before it. Clamped, so different inputs often give the
/// same answer and early cut-off gets exercised.
fn running(_component: &str, _params: &Value) -> Result<(Inputs, Arc<dyn Fn(&Cx) -> Cell<Value> + Send + Sync>), String> {
    Ok((
        Inputs::default(),
        Arc::new(|cx: &Cx| {
            let number = |c: Option<Runtime>| c.map_or(0, |r| r.value.as_i64().expect("a number"));
            let mut total = number(cx.get_runtime(cx.id(), "base")?);
            if let Some(prev) = cx.row_along(-1) {
                total += number(cx.get_runtime(prev, "amount")?);
            }
            Ok(Some(json!(total.clamp(-3, 3))))
        }),
    ))
}

fn install(reg: &mut Registry) {
    reg.rule_on_unowned("tables/running", running).inherit_through(IS_A).declare("owned", Place::Node, None).declare(Name::NAME, Place::Node, None);
}

#[derive(Default)]
struct T {
    g: Graph,
    ids: BTreeMap<String, Entity>,
}

impl T {
    fn new() -> Self {
        T { g: Graph::with_registry(registry()), ..T::default() }
    }

    /// The node first made with this name, made now if it's new.
    fn e(&mut self, name: &str) -> Entity {
        if let Some(e) = self.ids.get(name) {
            return *e;
        }
        let e = self.g.spawn();
        self.g.set(e, Name(name.to_string()));
        self.ids.insert(name.to_string(), e);
        e
    }

    fn put(&mut self, name: &str, component: &str, v: Value) {
        let e = self.e(name);
        self.g.set_runtime(e, component, v);
    }

    fn kind(&mut self, of: &str, kind: &str) {
        let (a, b) = (self.e(of), self.e(kind));
        self.g.link_runtime(a, b, Runtime::new(IS_A, Value::Null));
    }

    fn read(&self, name: &str, component: &str) -> Option<Value> {
        self.g.get_runtime(self.ids[name], component).unwrap().map(|r| r.value)
    }

    fn rename(&mut self, name: &str, to: &str) {
        let e = self.ids[name];
        self.g.set(e, Name(to.to_string()));
    }
}

#[test]
fn a_row_takes_what_it_doesnt_say_from_its_table_and_its_own_word_wins() {
    let mut t = T::new();
    t.put("Year", "rate", json!(5));
    t.put("Year", "owned", json!(1));
    t.e("Year[1]");
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(5)));
    assert_eq!(t.g.kind_of(t.ids["Year[1]"], "rate"), Some(t.ids["Year"]));
    assert_eq!(t.read("Year[1]", "owned"), None, "what a system owns isn't the table's to say");
    t.put("Year[1]", "rate", json!(6));
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(6)));
    t.g.remove_named(t.ids["Year[1]"], "rate");
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(5)));

    // It's all in the name, so renaming moves it, whichever end is renamed.
    t.rename("Year[1]", "Month[1]");
    assert_eq!(t.read("Year[1]", "rate"), None);
    t.rename("Year", "Month");
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(5)), "the row followed its table");
    t.rename("Year", "Year");
    t.e("Month");
    assert_eq!(t.read("Year[1]", "rate"), None, "a table with that name that doesn't say it");
    t.put("Month", "rate", json!(7));
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(7)));
    assert!(t.g.components_of(t.ids["Year[1]"]).contains("rate"));
}

#[test]
fn rows_with_one_key_in_related_tables_are_one_thing() {
    let mut t = T::new();
    t.put("Car", "insurance", json!(500));
    t.put("Petrol car", "per_mile", json!(9));
    t.kind("Petrol car", "Car");
    t.put("Car[Hatchback]", "price", json!(18000));
    t.put("Petrol car[Hatchback]", "mpg", json!(52));
    t.put("Boat[Hatchback]", "hull", json!(1));

    // Each part reads what the other says, and what either table says.
    assert_eq!(t.read("Car[Hatchback]", "mpg"), Some(json!(52)));
    assert_eq!(t.read("Car[Hatchback]", "per_mile"), Some(json!(9)), "a petrol car's, though this part's table is Car");
    assert_eq!(t.read("Petrol car[Hatchback]", "price"), Some(json!(18000)));
    assert_eq!(t.read("Petrol car[Hatchback]", "insurance"), Some(json!(500)));
    assert_eq!(t.read("Car[Hatchback]", "hull"), None, "a boat isn't related to a car, however it's keyed");

    // What's more specific wins: a part's own word, then a petrol car's, then any car's.
    t.put("Car", "per_mile", json!(1));
    assert_eq!(t.read("Car[Hatchback]", "per_mile"), Some(json!(9)));
    t.put("Petrol car[Hatchback]", "per_mile", json!(3));
    assert_eq!(t.read("Car[Hatchback]", "per_mile"), Some(json!(3)));
    assert_eq!(t.g.kind_of(t.ids["Car[Hatchback]"], "per_mile"), Some(t.ids["Petrol car[Hatchback]"]));

    // A petrol car with no Car row is still a car.
    t.e("Petrol car[Hybrid]");
    assert_eq!(t.read("Petrol car[Hybrid]", "insurance"), Some(json!(500)));

    // And the tables stop being related when Petrol car stops being a car.
    let (petrol, car) = (t.ids["Petrol car"], t.ids["Car"]);
    t.g.unlink_named(petrol, car, IS_A);
    assert_eq!((t.read("Car[Hatchback]", "mpg"), t.read("Petrol car[Hybrid]", "insurance")), (None, None));
}

#[test]
fn a_numbered_row_reads_the_one_before() {
    let mut t = T::new();
    let year = t.e("Year");
    t.g.set_rule(year, "amount", "tables/running", Value::Null);
    for i in 0..4 {
        t.put(&format!("Year[{i}]"), "base", json!(1));
    }
    assert_eq!(t.read("Year[3]", "amount"), Some(json!(3)), "clamped at 3");
    assert_eq!(t.read("Year[1]", "amount"), Some(json!(2)));
    let order: Vec<Entity> = (0..4).map(|i| t.ids[&format!("Year[{i}]")]).collect();
    assert_eq!(t.g.rows("Year"), order);

    // A gap means there's nothing before, and filling it brings it back.
    t.rename("Year[1]", "Spare");
    assert_eq!(t.read("Year[2]", "amount"), Some(json!(1)));
    t.rename("Year[1]", "Year[1]");
    assert_eq!(t.read("Year[2]", "amount"), Some(json!(3)));
    // The first row says its own.
    t.put("Year[0]", "amount", json!(-3));
    assert_eq!((t.read("Year[1]", "amount"), t.read("Year[3]", "amount")), (Some(json!(-2)), Some(json!(0))));
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

/// Names that make tables, rows of them, parts of one thing, and gaps.
const NAMES: &[&str] = &["T", "U", "T[0]", "T[1]", "T[2]", "U[1]", "U[2]", "T[x]", "U[x]", "V[x]", "plain"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Amount {
    Literal(i64),
    Rule,
}

#[derive(Default, Debug)]
struct State {
    names: BTreeMap<usize, &'static str>,
    amount: BTreeMap<usize, Amount>,
    base: BTreeMap<usize, i64>,
    kinds: std::collections::BTreeSet<(usize, usize)>,
}

const SLOTS: usize = 8;

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    install(&mut reg);
    Arc::new(reg)
}

fn fresh(s: &State) -> (Graph, Vec<Entity>) {
    let mut g = Graph::with_registry(registry());
    let ids: Vec<Entity> = (0..SLOTS).map(|_| g.spawn()).collect();
    for (i, n) in &s.names {
        g.set(ids[*i], Name(n.to_string()));
    }
    for (i, a) in &s.amount {
        match a {
            Amount::Literal(v) => g.set_runtime(ids[*i], "amount", json!(v)),
            Amount::Rule => g.set_rule(ids[*i], "amount", "tables/running", Value::Null),
        };
    }
    for (i, v) in &s.base {
        g.set_runtime(ids[*i], "base", json!(v));
    }
    for (a, b) in &s.kinds {
        g.link_runtime(ids[*a], ids[*b], Runtime::new(IS_A, Value::Null));
    }
    (g, ids)
}

fn outcome(g: &Graph, e: Entity) -> Result<Option<Value>, ()> {
    match g.get_runtime(e, "amount") {
        Ok(v) => Ok(v.map(|r| r.value)),
        Err(Error::Cycle(_)) => Err(()),
        Err(e) => panic!("unexpected {e:?}"),
    }
}

#[test]
fn a_long_lived_graph_with_tables_always_agrees_with_a_new_one() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let (mut edits, mut taken, mut with_prev) = (0usize, 0usize, 0usize);
    for _ in 0..300 {
        let mut state = State::default();
        let (mut g, ids) = fresh(&state);
        for _ in 0..60 {
            let (a, b) = (rng.next(SLOTS), rng.next(SLOTS));
            match rng.next(9) {
                0..=2 => {
                    let name = NAMES[rng.next(NAMES.len())];
                    state.names.insert(a, name);
                    g.set(ids[a], Name(name.to_string()));
                }
                3 => {
                    let v = rng.next(5) as i64 - 2;
                    state.amount.insert(a, Amount::Literal(v));
                    g.set_runtime(ids[a], "amount", json!(v));
                }
                4 | 5 => {
                    state.amount.insert(a, Amount::Rule);
                    g.set_rule(ids[a], "amount", "tables/running", Value::Null);
                }
                6 => {
                    state.amount.remove(&a);
                    g.remove_named(ids[a], "amount");
                }
                7 => {
                    let v = rng.next(3) as i64 - 1;
                    state.base.insert(a, v);
                    g.set_runtime(ids[a], "base", json!(v));
                }
                8 if a != b => {
                    // Kinds between tables, mostly, which is what relates their rows.
                    if state.kinds.remove(&(a, b)) {
                        g.unlink_named(ids[a], ids[b], IS_A);
                    } else {
                        state.kinds.insert((a, b));
                        g.link_runtime(ids[a], ids[b], Runtime::new(IS_A, Value::Null));
                    }
                }
                _ => continue,
            }
            edits += 1;
            if rng.next(100) < 50 {
                continue;
            }
            let (new, new_ids) = fresh(&state);
            for i in (0..SLOTS).filter(|_| rng.next(100) < 50) {
                let (got, want) = (outcome(&g, ids[i]), outcome(&new, new_ids[i]));
                assert_eq!(got, want, "slot {i}, with {state:?}");
                assert_eq!(g.components_of(ids[i]), new.components_of(new_ids[i]), "slot {i}, with {state:?}");
                taken += usize::from(new.kind_of(new_ids[i], "amount").is_some());
                with_prev += usize::from(new.row_along(new_ids[i], -1).is_some());
            }
            if edits % 10 == 0 {
                let (reloaded, problems) = Graph::load(g.save().unwrap(), registry());
                assert_eq!(problems, vec![]);
                for i in 0..SLOTS {
                    assert_eq!(outcome(&reloaded, ids[i]), outcome(&new, new_ids[i]), "slot {i} after a round trip");
                }
            }
        }
    }
    println!("{edits} edits, {taken} answers taken from elsewhere, {with_prev} rows with one before");
    assert!(taken > 300, "the edits should actually exercise tables");
    assert!(with_prev > 300, "and rows reading the one before");
}
