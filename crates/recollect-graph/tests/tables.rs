//! A node named `Year[3]` is row 3 of the table `Year`, and takes the workspace's own cells it doesn't
//! say from the node named `Year`, as it would from a kind. Rows with the same key in related tables
//! are parts of one thing, and a numbered row can read the one before it. See `table`.

use std::collections::BTreeMap;
use std::sync::Arc;

use recollect_graph::table::Key;
use recollect_graph::{CellType, ComponentType, Computed, Cx, Entity, Error, Graph, Name, Named, Registry, cell};
use serde_json::{Value, json};

const IS_A: &str = "is_a";
const OWNED: &str = "owned";

/// Something each node says about itself, so it can't come from a table.
#[cell]
struct Owned(i64);

/// Its own base, plus the amount of the row before it. Clamped, so different inputs often give the
/// same answer and early cut-off gets exercised.
fn running(cx: &Cx) -> Result<Option<Value>, Error> {
    let number = |c: Option<Value>| c.map_or(0, |v| v.as_i64().expect("a number"));
    let mut total = number(cx.get_value(cx.id(), "base")?);
    if let Some(prev) = cx.row_along(-1) {
        total += number(cx.get_value(prev, "amount")?);
    }
    Ok(Some(json!(total.clamp(-3, 3))))
}

fn install(reg: &mut Registry) {
    reg.free_formulas(Arc::new(|text| match text {
        "running" => Ok(Computed::json("running", running)),
        _ => Err(format!("there's no function called {text}")),
    }));
    reg.edge(IS_A, "What kind of thing this is.");
    reg.inherit_through(IS_A);
    reg.component(OWNED, "Something each node says about itself.");
    reg.cell::<Owned>(OWNED, "What this node says about itself.").per_entity();
    reg.component(Named::NAME, "What it's called.").everywhere();
    reg.cell::<Name>(Named::NAME, "What it's called.").per_entity();
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

    fn put(&mut self, name: &str, cell: &str, v: Value) {
        let e = self.e(name);
        self.g.set_value(e, cell, v);
    }

    fn formula(&mut self, name: &str) {
        let e = self.e(name);
        self.g.set_formula(e, "amount", "running");
    }

    fn kind(&mut self, of: &str, kind: &str) {
        let (a, b) = (self.e(of), self.e(kind));
        self.g.link(a, b, IS_A);
    }

    fn read(&self, name: &str, cell: &str) -> Option<Value> {
        self.g.get_value(self.ids[name], cell).unwrap()
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
    let year = t.e("Year");
    t.g.add(year, OWNED);
    t.put("Year", Owned::NAME, json!(1));
    t.e("Year[1]");
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(5)));
    assert_eq!(t.g.kind_beneath(t.ids["Year[1]"], "rate"), Some(t.ids["Year"]));
    assert_eq!(t.read("Year[1]", "owned"), None, "what each node says about itself isn't the table's to say");
    t.put("Year[1]", "rate", json!(6));
    assert_eq!(t.read("Year[1]", "rate"), Some(json!(6)));
    t.g.clear(t.ids["Year[1]"], "rate");
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
    assert!(t.g.cells_of(t.ids["Year[1]"]).contains("rate"));
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
    assert_eq!(t.g.kind_beneath(t.ids["Car[Hatchback]"], "per_mile"), Some(t.ids["Petrol car[Hatchback]"]));

    // A petrol car with no Car row is still a car.
    t.e("Petrol car[Hybrid]");
    assert_eq!(t.read("Petrol car[Hybrid]", "insurance"), Some(json!(500)));

    // And the tables stop being related when Petrol car stops being a car.
    let (petrol, car) = (t.ids["Petrol car"], t.ids["Car"]);
    t.g.unlink(petrol, car, IS_A);
    assert_eq!((t.read("Car[Hatchback]", "mpg"), t.read("Petrol car[Hybrid]", "insurance")), (None, None));
}

#[test]
fn a_numbered_row_reads_the_one_before() {
    let mut t = T::new();
    t.formula("Year");
    for i in 0..4 {
        t.put(&format!("Year[{i}]"), "base", json!(1));
    }
    assert_eq!(t.read("Year[3]", "amount"), Some(json!(3)), "clamped at 3");
    assert_eq!(t.read("Year[1]", "amount"), Some(json!(2)));
    let order: Vec<Entity> = (0..4).map(|i| t.ids[&format!("Year[{i}]")]).collect();
    assert_eq!(t.g.rows("Year"), order);

    // A gap is a row nobody wrote, which takes everything from the table. Year says nothing about a
    // base, so that's an error, until it does.
    t.rename("Year[1]", "Spare");
    let year2 = t.ids["Year[2]"];
    assert!(matches!(t.g.get_value(year2, "amount"), Err(Error::Missing((_, c))) if c == "base"));
    t.put("Year", "base", json!(0));
    assert_eq!(t.read("Year[2]", "amount"), Some(json!(2)), "1 for Year[0], then 0 for the row nobody wrote, then its own 1");
    t.rename("Year[1]", "Year[1]");
    assert_eq!(t.read("Year[2]", "amount"), Some(json!(3)));
    // The first row says its own.
    t.put("Year[0]", "amount", json!(-3));
    assert_eq!((t.read("Year[1]", "amount"), t.read("Year[3]", "amount")), (Some(json!(-2)), Some(json!(0))));
}

#[test]
fn a_numbered_table_goes_on_as_far_as_its_asked_and_none_of_that_is_saved() {
    let mut t = T::new();
    t.formula("Year");
    t.put("Year", "base", json!(1));
    t.put("Year[0]", "amount", json!(-3));
    t.e("Year[1]");
    let saved = t.g.save();

    // Year[100] was never written: it's worked out from the table, and so is every row before it.
    let far = t.g.row_at("Year", &Key::Num(100)).expect("past the last row, it goes on");
    assert_eq!(t.g.get_value(far, "amount").unwrap().unwrap(), json!(3));
    assert_eq!(t.g.row_at("Year", &Key::Num(100)), Some(far), "the same row each time");
    assert_eq!(t.g.row_along(far, -1), t.g.row_at("Year", &Key::Num(99)));
    assert!(matches!(t.g.get_value(far, "colour"), Err(Error::Missing(_))), "what the table doesn't say, it can't have");
    assert_eq!(t.g.row_at("Year", &Key::Num(-1)), None, "before the first row, there's nothing");
    assert_eq!(t.g.row_at("Year", &Key::Num(1 + recollect_graph::graph::MOST_IMPLICIT + 1)), None);
    assert_eq!(t.g.row_at("Year", &Key::Text("x".to_string())), None, "only numbers go on by themselves");
    assert_eq!(t.g.save(), saved, "none of it is written down");
    assert!(!t.g.entities().any(|e| e == far));

    // It's remembered, and changes reach it.
    t.g.take_ran();
    t.g.get_value(far, "amount").unwrap();
    assert!(t.g.take_ran().is_empty());
    t.put("Year", "base", json!(-1));
    assert_eq!(t.g.get_value(far, "amount").unwrap().unwrap(), json!(-3));
    // Writing one of those rows makes it an ordinary row, which the ones after it read instead.
    t.put("Year[50]", "amount", json!(3));
    assert_eq!(t.g.get_value(far, "amount").unwrap().unwrap(), json!(-3), "clamped, fifty rows on");
    let near = t.g.row_at("Year", &Key::Num(51)).unwrap();
    assert_eq!(t.g.get_value(near, "amount").unwrap().unwrap(), json!(2));
}

#[test]
fn a_second_table_node_reaches_rows_nobody_wrote() {
    // Found by the random test below: the rows nobody wrote were left out when a name moved what they take.
    let mut t = T::new();
    t.put("Other", "base", json!(-1));
    t.formula("First");
    t.put("Year[0]", "base", json!(1));
    t.rename("First", "Year");
    let one = t.g.row_at("Year", &Key::Num(1)).unwrap();
    assert!(matches!(t.g.get_value(one, "amount"), Err(Error::Missing(_))));
    t.rename("Other", "Year");
    assert_eq!(t.g.get_value(one, "amount").unwrap().unwrap(), json!(0), "the other node named Year says the base: -1, and 1 from Year[0]");
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
    Formula,
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
            Amount::Literal(v) => g.set_value(ids[*i], "amount", json!(v)),
            Amount::Formula => g.set_formula(ids[*i], "amount", "running"),
        };
    }
    for (i, v) in &s.base {
        g.set_value(ids[*i], "base", json!(v));
    }
    for (a, b) in &s.kinds {
        g.link(ids[*a], ids[*b], IS_A);
    }
    (g, ids)
}

fn outcome(g: &Graph, e: Entity) -> Result<Option<Value>, ()> {
    match g.get_value(e, "amount") {
        Ok(v) => Ok(v),
        // Going round in a loop, or reading what a row nobody wrote can't have.
        Err(Error::Cycle(_) | Error::Missing(_)) => Err(()),
        Err(e) => panic!("unexpected {e:?}"),
    }
}

#[test]
fn a_long_lived_graph_with_tables_always_agrees_with_a_new_one() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let (mut edits, mut taken, mut with_prev, mut implicit) = (0usize, 0usize, 0usize, 0usize);
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
                    g.set_value(ids[a], "amount", json!(v));
                }
                4 | 5 => {
                    state.amount.insert(a, Amount::Formula);
                    g.set_formula(ids[a], "amount", "running");
                }
                6 => {
                    state.amount.remove(&a);
                    g.clear(ids[a], "amount");
                }
                7 => {
                    let v = rng.next(3) as i64 - 1;
                    state.base.insert(a, v);
                    g.set_value(ids[a], "base", json!(v));
                }
                8 if a != b => {
                    // Kinds between tables, mostly, which is what relates their rows.
                    if state.kinds.remove(&(a, b)) {
                        g.unlink(ids[a], ids[b], IS_A);
                    } else {
                        state.kinds.insert((a, b));
                        g.link(ids[a], ids[b], IS_A);
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
                assert_eq!(g.cells_of(ids[i]), new.cells_of(new_ids[i]), "slot {i}, with {state:?}");
                assert_eq!(g.components_of(ids[i]), new.components_of(new_ids[i]), "slot {i}, with {state:?}");
                taken += usize::from(new.kind_beneath(new_ids[i], "amount").is_some());
                with_prev += usize::from(new.row_along(new_ids[i], -1).is_some());
            }
            // And rows nobody wrote, read the same way.
            for (table, n) in [("T", 4), ("U", 3), ("T", 1)] {
                let (a, b) = (g.row_at(table, &Key::Num(n)), new.row_at(table, &Key::Num(n)));
                assert_eq!(a.is_some(), b.is_some());
                if let (Some(a), Some(b)) = (a, b) {
                    assert_eq!(outcome(&g, a), outcome(&new, b), "{table}[{n}], with {state:?}");
                    implicit += usize::from(!g.entities().any(|e| e == a));
                }
            }
            if edits % 10 == 0 {
                let (reloaded, problems) = Graph::load(g.save(), registry());
                assert_eq!(problems, vec![]);
                for i in 0..SLOTS {
                    assert_eq!(outcome(&reloaded, ids[i]), outcome(&new, new_ids[i]), "slot {i} after a round trip");
                }
            }
        }
    }
    println!("{edits} edits, {taken} answers taken from elsewhere, {with_prev} rows with one before, {implicit} reads of rows nobody wrote");
    assert!(implicit > 300, "and rows nobody wrote");
    assert!(taken > 300, "the edits should actually exercise tables");
    assert!(with_prev > 300, "and rows reading the one before");
}
