//! The example problems, as workspaces. These write the files in `examples/`, through a real log,
//! and `tests/samples.rs` checks that what's committed there still means what it should.

use std::collections::BTreeMap;

use recollect_graph::{Component, Entity, Name, Op, Storage};
use serde_json::{Value, json};

use crate::formulas::{At, Domain, EXPR, IsA, Label, Requires, ValueCell};
use crate::notes::{Body, Drawing};
use crate::plan::{Alternative, Decision, DependsOn, Done, Estimate, Goal, PartOf, Task, Unknown};

pub struct Sample {
    pub name: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    /// What kind of thing it is, for finding it among the others.
    pub group: &'static str,
    pub systems: &'static [&'static str],
    pub build: fn(&mut Build),
}

pub const SAMPLES: &[Sample] = &[
    Sample { name: "calibration", title: "Calibration plan", about: "Three ways to calibrate, plus a gap for what nobody has thought of. The decision is held open, and every value is the exact set of worlds where it's true.", group: "plan", systems: &["plan", "schedule"], build: calibration },
    Sample { name: "critical-path", title: "Critical path", about: "A plain schedule with no decisions: dates go forward over the dependencies, and slack comes from following them backward.", group: "plan", systems: &["plan", "schedule"], build: critical_path },
    Sample { name: "invoice", title: "Invoice", about: "Two lines, a subtotal, tax and a total. Edit a number and only what it reaches reruns.", group: "sheet", systems: &[], build: invoice },
    Sample { name: "trip-budget", title: "Trip budget", about: "A budget with decisions left open: the total is exact in every world, and so is what would put it over.", group: "sheet", systems: &["plan"], build: trip_budget },
    Sample { name: "choosing-a-car", title: "Choosing a car", about: "Three cars of one kind, what each costs a year over mileage you can only guess at, and which can get to Edinburgh without stopping.", group: "sheet", systems: &["plan"], build: choosing_a_car },
    Sample { name: "compound-interest", title: "Compound interest", about: "Ten years chained together, sharing one rate cell.", group: "sheet", systems: &[], build: compound_interest },
    Sample { name: "fibonacci", title: "Fibonacci", about: "Each cell adds the two before it. Cells that are shared run once, so 80 terms take 79 formula runs.", group: "sheet", systems: &[], build: fibonacci },
    Sample { name: "australia", title: "Colouring Australia", about: "Each region is a choice and each border a constraint. There are 18 colourings. Pin a region and see what that forces.", group: "puzzle", systems: &[], build: australia },
    Sample { name: "eight-queens", title: "Eight queens", about: "One choice per row, and one constraint per pair of rows. There are 92 boards.", group: "puzzle", systems: &[], build: eight_queens },
    Sample { name: "sudoku", title: "4×4 sudoku", about: "Sixteen cells, and a constraint between each pair that can't match. There are 288 grids.", group: "puzzle", systems: &[], build: sudoku },
    Sample { name: "knights-and-knaves", title: "Knights and knaves", about: "A says “we are both knaves”. Knights always tell the truth, and knaves always lie.", group: "puzzle", systems: &[], build: knights_and_knaves },
    Sample { name: "zebra", title: "The zebra puzzle", about: "Who owns the zebra, and who drinks water? About 3 × 10¹⁷ combinations, and the rules leave exactly one.", group: "puzzle", systems: &[], build: zebra },
];

/// Changes when samples mean something different, so an old working copy gets replaced, not misread.
pub const FORMAT: &str = "3";

/// Ops for a sample, referring to nodes by name while they're being written.
#[derive(Default)]
pub struct Build {
    pub ops: Vec<Op>,
    ids: BTreeMap<String, Entity>,
}

impl Build {
    /// The node with this name, created if it's new.
    pub fn e(&mut self, name: &str) -> Entity {
        if let Some(e) = self.ids.get(name) {
            return *e;
        }
        let e = Entity::new();
        self.ids.insert(name.to_string(), e);
        self.ops.push(Op::set(e, Name(name.to_string())));
        e
    }

    /// A node known here by `key`, and called something else.
    pub fn named(&mut self, key: &str, name: &str) -> Entity {
        let e = Entity::new();
        self.ids.insert(key.to_string(), e);
        self.ops.push(Op::set(e, Name(name.to_string())));
        e
    }

    pub fn set<C: Component>(&mut self, name: &str, storage: impl Into<Storage<C>>) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set(e, storage));
        self
    }

    pub fn link<L: Component>(&mut self, from: &str, to: &str, v: L) -> &mut Self {
        let (a, b) = (self.e(from), self.e(to));
        self.ops.push(Op::link(a, b, v));
        self
    }

    /// A task taking this many days.
    pub fn task(&mut self, name: &str, days: f64) -> &mut Self {
        self.set(name, Task {}).set(name, Estimate::days(days))
    }

    /// This node's value, as a formula.
    pub fn formula(&mut self, name: &str, text: &str) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set_rule(e, ValueCell::NAME, EXPR, json!({ "text": text })));
        self
    }

    /// One of the workspace's own cells, as a value.
    pub fn put(&mut self, name: &str, component: &str, v: Value) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set_runtime(e, component, v));
        self
    }

    /// Any cell that can hold a formula, as one.
    pub fn formula_on(&mut self, name: &str, component: &str, text: &str) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set_rule(e, component, EXPR, json!({ "text": text })));
        self
    }

    /// What a formula on `from` calls `to`.
    pub fn label(&mut self, from: &str, to: &str, label: &str) -> &mut Self {
        self.link(from, to, Label(label.to_string()))
    }

    fn link_named(&mut self, from: &str, to: &str, kind: &str, v: Value) -> &mut Self {
        let (a, b) = (self.e(from), self.e(to));
        self.ops.push(Op::link_runtime(a, b, recollect_graph::Runtime::new(kind, v)));
        self
    }
}

fn calibration(b: &mut Build) {
    b.set("Calibrate the lidar and camera", Goal);
    b.set("Calibrate the lidar and camera", Body("Done when the reprojection error stays under a pixel on held-out scenes.".to_string()));
    b.set("Which approach?", Decision::default()).link("Which approach?", "Calibrate the lidar and camera", PartOf);
    for o in ["From the CAD model", "Free-form search", "With a checkerboard"] {
        b.link(o, "Which approach?", Alternative::default());
    }
    b.set("Something else", Unknown).link("Something else", "Which approach?", Alternative::default());
    b.task("Fit the IMU time offset", 1.0).link("Fit the IMU time offset", "Which approach?", PartOf);
    b.task("Static captures", 2.0).link("Static captures", "From the CAD model", PartOf).link("Static captures", "Free-form search", PartOf);
    b.task("Export extrinsics from CAD", 3.0).link("Export extrinsics from CAD", "From the CAD model", PartOf).link("Export extrinsics from CAD", "Static captures", DependsOn::default());
    b.task("Global init search", 5.0).link("Global init search", "Free-form search", PartOf).link("Global init search", "Static captures", DependsOn::default());
    b.task("Board detector", 4.0).link("Board detector", "With a checkerboard", PartOf);
    b.set("Diverges on weak geometry", crate::plan::Risk);
    b.link_named("Diverges on weak geometry", "Global init search", "contradicts", Value::Null);
}

fn critical_path(b: &mut Build) {
    b.task("Lay foundations", 3.0).task("Wire the rooms", 2.0).task("Frame the walls", 4.0).task("Paint", 2.0);
    b.link("Wire the rooms", "Lay foundations", DependsOn::default());
    b.link("Frame the walls", "Lay foundations", DependsOn::default());
    b.link("Paint", "Wire the rooms", DependsOn::default());
    b.link("Paint", "Frame the walls", DependsOn::default());
    b.set("Lay foundations", Done::yes());
}

fn invoice(b: &mut Build) {
    for (line, qty, price) in [("Widgets", 3.0, 10.0), ("Gears", 2.0, 25.0)] {
        let (q, p) = (format!("{line}: quantity"), format!("{line}: price"));
        b.set(&q, ValueCell::num(qty)).set(&p, ValueCell::num(price));
        b.formula(line, "quantity * price").label(line, &q, "quantity").label(line, &p, "price");
        b.label("Subtotal", line, "line");
    }
    b.formula("Subtotal", "sum(line)");
    b.set("Tax rate", ValueCell::num(0.2));
    b.formula("Tax", "subtotal * rate").label("Tax", "Subtotal", "subtotal").label("Tax", "Tax rate", "rate");
    b.formula("Total", "subtotal + tax").label("Total", "Subtotal", "subtotal").label("Total", "Tax", "tax");
}

fn trip_budget(b: &mut Build) {
    // Options are ordinary nodes, the same as in a plan, so one could have parts and dependencies too.
    for (decision, options) in [("Travel", [("Train", 80.0), ("Plane", 120.0)]), ("Stay", [("Hostel", 60.0), ("Hotel", 150.0)])] {
        b.set(decision, Decision::default()).formula(decision, "pick(in.alternative)");
        for (option, price) in options {
            b.set(option, ValueCell::num(price)).link(option, decision, Alternative::default());
        }
    }
    b.formula("Trip", "travel + stay").label("Trip", "Travel", "travel").label("Trip", "Stay", "stay");
    b.formula("Over £200", "trip > 200").label("Over £200", "Trip", "trip");
}

fn choosing_a_car(b: &mut Build) {
    b.set("Years kept", ValueCell::num(5.0));
    b.set("Miles a year", Decision::default()).formula("Miles a year", "pick(in.alternative)");
    for (option, miles, weight) in [("Light use", 6000.0, 1.0), ("Typical", 10000.0, 2.0), ("Heavy", 16000.0, 1.0)] {
        b.set(option, ValueCell::num(miles)).link(option, "Miles a year", Alternative { weight: Some(weight), ..Alternative::default() });
    }
    // What every car has in common: how its price, running cost and insurance make a cost a year.
    b.set("Car", Body("What a car costs a year: losing 15% of its value each year it's kept, running it, and insuring it.".to_string()));
    b.formula_on("Car", "yearly", "self.price * (1 - 0.85 ^ years) / years + miles * self.per_mile + self.insurance");
    b.label("Car", "Years kept", "years").label("Car", "Miles a year", "miles");
    b.set("Which car?", Decision::default()).formula("Which car?", "pick(o in in.alternative: o.yearly)");
    b.formula_on("Which car?", "range", "pick(o in in.alternative: o.range)");
    for (car, price, per_mile, insurance, range) in [("Hatchback", 18000.0, 0.12, 500.0, 450.0), ("Estate", 24000.0, 0.13, 550.0, 500.0), ("Electric", 32000.0, 0.05, 600.0, 250.0)] {
        b.put(car, "price", json!(price)).put(car, "per_mile", json!(per_mile)).put(car, "insurance", json!(insurance)).put(car, "range", json!(range));
        b.link(car, "Car", IsA).link(car, "Which car?", Alternative::default());
    }
    b.set("To Edinburgh", ValueCell::num(400.0));
    b.formula("Gets to Edinburgh without stopping", "car.range >= trip").label("Gets to Edinburgh without stopping", "Which car?", "car").label("Gets to Edinburgh without stopping", "To Edinburgh", "trip");
}

fn compound_interest(b: &mut Build) {
    b.set("Year 0", ValueCell::num(1000.0)).set("Rate", ValueCell::num(0.05));
    for i in 1..=10 {
        let id = format!("Year {i}");
        b.formula(&id, "prev * (1 + rate)").label(&id, &format!("Year {}", i - 1), "prev").label(&id, "Rate", "rate");
    }
}

fn fibonacci(b: &mut Build) {
    b.set("f0", ValueCell::num(0.0)).set("f1", ValueCell::num(1.0));
    for i in 2..=80 {
        let id = format!("f{i}");
        b.formula(&id, "a + b").label(&id, &format!("f{}", i - 1), "a").label(&id, &format!("f{}", i - 2), "b");
    }
}

/// A puzzle's root, and what its constraints have in common.
struct Puzzle<'a> {
    b: &'a mut Build,
    root: String,
    next: usize,
}

impl<'a> Puzzle<'a> {
    fn new(b: &'a mut Build, root: &str) -> Self {
        b.formula(root, "all(constraint)").set(root, Requires);
        Puzzle { b, root: root.to_string(), next: 0 }
    }

    fn choice(&mut self, name: &str, options: &[&str]) {
        self.b.set(name, Domain(options.iter().map(|s| s.to_string()).collect()));
    }

    /// A formula over the named choices, which goes towards the puzzle being solved.
    fn constraint(&mut self, about: &[(&str, &str)], label: String, text: &str) {
        self.next += 1;
        // Two constraints can read the same, so what they say can't be what identifies one here.
        let key = format!("constraint {}", self.next);
        self.b.named(&key, &label);
        self.b.formula(&key, text);
        for (name, node) in about {
            self.b.label(&key, node, name);
        }
        let root = self.root.clone();
        self.b.label(&root, &key, "constraint");
    }

    fn same(&mut self, a: &str, b: &str) {
        self.constraint(&[("a", a), ("b", b)], format!("{a} = {b}"), "a = b");
    }

    fn different(&mut self, a: &str, b: &str) {
        self.constraint(&[("a", a), ("b", b)], format!("{a} ≠ {b}"), "a != b");
    }

    fn adjacent(&mut self, a: &str, b: &str) {
        self.constraint(&[("a", a), ("b", b)], format!("{a} next to {b}"), "abs(a - b) = 1");
    }

    fn is(&mut self, choice: &str, value: &str) {
        self.constraint(&[("choice", choice)], format!("{choice} is {value}"), &format!("choice = {value}"));
    }
}

fn australia(b: &mut Build) {
    let mut p = Puzzle::new(b, "A colouring");
    let regions = ["Western Australia", "Northern Territory", "South Australia", "Queensland", "New South Wales", "Victoria", "Tasmania"];
    for r in regions {
        p.choice(r, &["red", "green", "blue"]);
    }
    p.b.set("Map of Australia", Drawing(include_str!("drawings/australia.ts").to_string()));
    let borders = [(0, 1), (0, 2), (1, 2), (1, 3), (2, 3), (2, 4), (2, 5), (3, 4), (4, 5)];
    for (x, y) in borders {
        p.different(regions[x], regions[y]);
    }
}

fn eight_queens(b: &mut Build) {
    let mut p = Puzzle::new(b, "A board");
    let cols: Vec<String> = (1..=8).map(|c| c.to_string()).collect();
    let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
    for r in 1..=8 {
        p.choice(&format!("Row {r}"), &cols);
    }
    for r1 in 1..=8 {
        for r2 in r1 + 1..=8 {
            let (a, b) = (format!("Row {r1}"), format!("Row {r2}"));
            p.constraint(&[("a", &a), ("b", &b)], format!("{a} clear of {b}"), &format!("a != b && abs(a - b) != {}", r2 - r1));
        }
    }
}

fn sudoku(b: &mut Build) {
    let mut p = Puzzle::new(b, "A grid");
    p.b.set("Board with its boxes", Drawing(include_str!("drawings/sudoku.ts").to_string()));
    let id = |r: usize, c: usize| format!("Cell {}{}", (b'A' + r as u8) as char, c + 1);
    for r in 0..4 {
        for c in 0..4 {
            p.choice(&id(r, c), &["1", "2", "3", "4"]);
            p.b.set(&id(r, c), At { row: r as u32, col: c as u32 });
        }
    }
    for a in 0..16usize {
        for b in a + 1..16 {
            let (ra, ca, rb, cb) = (a / 4, a % 4, b / 4, b % 4);
            if ra == rb || ca == cb || (ra / 2 == rb / 2 && ca / 2 == cb / 2) {
                p.different(&id(ra, ca), &id(rb, cb));
            }
        }
    }
}

fn knights_and_knaves(b: &mut Build) {
    let mut p = Puzzle::new(b, "The island");
    p.choice("A", &["knight", "knave"]);
    p.choice("B", &["knight", "knave"]);
    let said = "“We are both knaves”";
    p.b.formula(said, r#"a = "knave" && b = "knave""#).label(said, "A", "a").label(said, "B", "b");
    p.constraint(&[("speaker", "A"), ("said", said)], "A is a knight exactly when that's true".to_string(), r#"(speaker = "knight") = said"#);
}

fn zebra(b: &mut Build) {
    let houses = ["1", "2", "3", "4", "5"];
    let categories: [&[&str]; 5] = [
        &["The Englishman", "The Spaniard", "The Ukrainian", "The Norwegian", "The Japanese"],
        &["Red house", "Green house", "Ivory house", "Yellow house", "Blue house"],
        &["Dog", "Snails", "Fox", "Horse", "Zebra"],
        &["Coffee", "Tea", "Milk", "Juice", "Water"],
        &["Old Gold", "Kools", "Chesterfield", "Lucky Strike", "Parliament"],
    ];
    let mut p = Puzzle::new(b, "The street");
    for cat in categories {
        for v in cat {
            p.choice(v, &houses);
        }
    }
    // The clues go first, so the search is narrow before the all-different constraints apply.
    p.same("The Englishman", "Red house");
    p.same("The Spaniard", "Dog");
    p.same("Coffee", "Green house");
    p.same("The Ukrainian", "Tea");
    p.constraint(&[("a", "Green house"), ("b", "Ivory house")], "Green house = Ivory house + 1".to_string(), "a = b + 1");
    p.same("Old Gold", "Snails");
    p.same("Kools", "Yellow house");
    p.is("Milk", "3");
    p.is("The Norwegian", "1");
    p.adjacent("Chesterfield", "Fox");
    p.adjacent("Kools", "Horse");
    p.same("Lucky Strike", "Juice");
    p.same("The Japanese", "Parliament");
    p.adjacent("The Norwegian", "Blue house");
    for cat in categories {
        for i in 0..5 {
            for j in i + 1..5 {
                p.different(cat[i], cat[j]);
            }
        }
    }
}

/// The `recollect.toml` for a sample.
pub fn config_text(s: &Sample) -> String {
    let systems: Vec<String> = s.systems.iter().map(|n| format!("{n:?}")).collect();
    format!("name = {:?}\nabout = {:?}\nsystems = [{}]\n", s.title, s.about, systems.join(", "))
}

/// What samples are generated from, for anything that wants to check one.
pub fn ops_of(s: &Sample) -> Vec<Op> {
    let mut b = Build::default();
    (s.build)(&mut b);
    b.ops
}
