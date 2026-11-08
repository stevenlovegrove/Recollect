//! The example problems, as workspaces. These write the files in `examples/`, through a real log,
//! and `tests/samples.rs` checks that what's committed there still means what it should.

use std::collections::BTreeMap;

use recollect_graph::{Component, Entity, Name, Op, Storage};
use serde_json::{Value, json};

use crate::formulas::{At, Domain, EXPR, IsA, Label, Requires, Uncertain, ValueCell};
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
    Sample { name: "choosing-a-car", title: "Choosing a car", about: "Petrol and electric cars as tables: what every car has, and what only petrol or electric cars have. What each costs a year over mileage you can only guess at, and which can get to Edinburgh without stopping.", group: "sheet", systems: &["plan"], build: choosing_a_car },
    Sample { name: "compound-interest", title: "Compound interest", about: "A table of years. One formula on Year works out each from the year before, and only Year[0] and Year[5] are written: the rest are worked out as far as they're asked for. A top-up you might make in year 5 changes only the years after it.", group: "sheet", systems: &["plan"], build: compound_interest },
    Sample { name: "fibonacci", title: "Fibonacci", about: "A table where each row adds the two before it: prev.f + prev.prev.f. Only the first two rows are written, and asking for F[80] works out the rest, each once: 79 formula runs.", group: "sheet", systems: &[], build: fibonacci },
    Sample { name: "australia", title: "Colouring Australia", about: "Each region is a choice and each border a constraint. There are 18 colourings. Pin a region and see what that forces.", group: "puzzle", systems: &[], build: australia },
    Sample { name: "eight-queens", title: "Eight queens", about: "A table with a row for each row of the board, choosing the column its queen is in. One formula on the table says no two share a column or a diagonal. There are 92 boards.", group: "puzzle", systems: &[], build: eight_queens },
    Sample { name: "sudoku", title: "4×4 sudoku", about: "A table of sixteen cells, each choosing a number, and a clue for each row, column and box: distinct(Cell[A1], Cell[A2], …). There are 288 grids.", group: "puzzle", systems: &[], build: sudoku },
    Sample { name: "knights-and-knaves", title: "Knights and knaves", about: "A says “we are both knaves”. Knights always tell the truth, and knaves always lie.", group: "puzzle", systems: &[], build: knights_and_knaves },
    Sample { name: "zebra", title: "The zebra puzzle", about: "Who owns the zebra, and who drinks water? A table for each kind of thing, whose rows choose a house, and the clues written as they read: Nationality[English] = Colour[Red]. About 3 × 10¹⁷ combinations, and the clues leave exactly one.", group: "puzzle", systems: &[], build: zebra },
];

/// Changes when samples mean something different, so an old working copy gets replaced, not misread.
pub const FORMAT: &str = "7";

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
    // Not something to decide: it'll be whatever it is, and these are guesses at how likely each is.
    b.set("Miles a year", Decision::default()).set("Miles a year", Uncertain).formula("Miles a year", "pick(in.alternative)");
    for (option, miles, weight) in [("Light use", 6000.0, 1.0), ("Typical", 10000.0, 2.0), ("Heavy", 16000.0, 1.0)] {
        b.set(option, ValueCell::num(miles)).link(option, "Miles a year", Alternative { weight: Some(weight), ..Alternative::default() });
    }
    // What a mile costs follows from what a car runs on, so each kind says how, and each car only says
    // how far it goes on a litre or a kilowatt-hour.
    b.set("Fuel price", ValueCell::num(1.45)).set("Electricity price", ValueCell::num(0.28));
    b.set("Petrol car", Body("A mile costs the fuel it burns: a gallon is 4.546 litres. Each says its own mpg.".to_string()));
    b.formula_on("Petrol car", "cost_per_mile", "fuel * 4.546 / self.mpg").label("Petrol car", "Fuel price", "fuel");
    b.set("Electric car", Body("A mile costs the electricity it takes. Each says its own kWh a mile.".to_string()));
    b.formula_on("Electric car", "cost_per_mile", "power * self.kwh_per_mile").label("Electric car", "Electricity price", "power");
    // The decision gives whatever the car it lands on says, so anything can read it.
    b.set("Which car?", Decision::default());
    for cell in ["price", "cost_per_mile", "insurance", "range"] {
        b.formula_on("Which car?", cell, &format!("pick(o in in.alternative: o.{cell})"));
    }
    // Three tables. Every car has a price, insurance and a range, so those are Car's. Only petrol cars
    // have an mpg, and only electric ones a kWh a mile. Petrol car[Hatchback] and Car[Hatchback] are two
    // parts of one car, and each reads what the other says.
    b.link("Petrol car", "Car", IsA).link("Electric car", "Car", IsA);
    let cars: [(&str, &str, &str, f64, f64, f64, f64); 4] = [
        ("Hatchback", "Petrol car", "mpg", 52.0, 18000.0, 500.0, 550.0),
        ("Estate", "Petrol car", "mpg", 44.0, 24000.0, 550.0, 600.0),
        ("Electric", "Electric car", "kwh_per_mile", 0.28, 32000.0, 600.0, 250.0),
        // A petrol car whose maker gives its cost a mile, which says so for itself and isn't worked out.
        ("Hybrid", "Petrol car", "cost_per_mile", 0.09, 26000.0, 550.0, 650.0),
    ];
    for (car, kind, cell, v, price, insurance, range) in cars {
        let row = format!("Car[{car}]");
        b.put(&row, "price", json!(price)).put(&row, "insurance", json!(insurance)).put(&row, "range", json!(range));
        b.link(&row, "Which car?", Alternative::default());
        b.put(&format!("{kind}[{car}]"), cell, json!(v));
    }
    // Losing 15% of its value each year it's kept, running it, and insuring it.
    b.formula("Yearly cost", "car.price * (1 - 0.85 ^ years) / years + miles * car.cost_per_mile + car.insurance");
    b.label("Yearly cost", "Which car?", "car").label("Yearly cost", "Years kept", "years").label("Yearly cost", "Miles a year", "miles");
    b.set("Miles to Edinburgh", ValueCell::num(400.0));
    let trip = "Gets to Edinburgh without stopping";
    b.formula(trip, "car.range >= miles").label(trip, "Which car?", "car").label(trip, "Miles to Edinburgh", "miles");
}

fn compound_interest(b: &mut Build) {
    // One formula for every year, on the table. Year[0] says its own, so it's where the chain starts.
    b.set("Rate", ValueCell::num(0.05));
    b.formula_on("Year", "balance", "prev.balance * (1 + rate)").label("Year", "Rate", "rate");
    // Only rows with something of their own are written. The rest are there to be read, as far as asked.
    b.put("Year[0]", "balance", json!(1000.0));
    // A choice five years in: the years before it have one balance, and the years after have one for each answer.
    b.set("Top up in year 5?", Decision::default()).formula("Top up in year 5?", "pick(in.alternative)");
    for (option, amount) in [("No top-up", 0.0), ("Add 500", 500.0)] {
        b.set(option, ValueCell::num(amount)).link(option, "Top up in year 5?", Alternative::default());
    }
    b.formula_on("Year[5]", "balance", "prev.balance * (1 + rate) + top_up");
    b.label("Year[5]", "Rate", "rate").label("Year[5]", "Top up in year 5?", "top_up");
    b.formula("After 30 years", "Year[30].balance");
}

fn fibonacci(b: &mut Build) {
    b.formula_on("F", "f", "prev.f + prev.prev.f");
    b.put("F[0]", "f", json!(0)).put("F[1]", "f", json!(1));
    b.formula("The 80th", "F[80].f");
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

    fn different(&mut self, a: &str, b: &str) {
        self.constraint(&[("a", a), ("b", b)], format!("{a} ≠ {b}"), "a != b");
    }

    /// A table of choices: each row takes the table's options.
    fn table(&mut self, table: &str, options: &[&str], keys: &[&str]) {
        self.choice(table, options);
        for k in keys {
            self.b.e(&format!("{table}[{k}]"));
        }
    }

    /// A clue, as the puzzle says it, and as a formula that names the rows it's about.
    fn clue(&mut self, said: &str, text: &str) {
        self.constraint(&[], said.to_string(), text);
    }

    /// What a table says about its rows, which goes towards the puzzle being solved.
    fn rule(&mut self, table: &str, said: &str, text: &str) {
        self.b.formula(table, text).set(table, Body(said.to_string()));
        let root = self.root.clone();
        self.b.label(&root, table, "constraint");
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
    // Each row of the board is a row of the table, which says which column its queen is in.
    let cols: Vec<String> = (1..=8).map(|c| c.to_string()).collect();
    let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
    let keys: Vec<String> = (1..=8).map(|r| r.to_string()).collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    p.table("Queen", &cols, &keys);
    p.rule("Queen", "No two share a column or a diagonal", "distinct(rows) && distinct(q in rows: q + q.row) && distinct(q in rows: q - q.row)");
}

fn sudoku(b: &mut Build) {
    let mut p = Puzzle::new(b, "A grid");
    p.b.set("Board with its boxes", Drawing(include_str!("drawings/sudoku.ts").to_string()));
    let rows = ["A", "B", "C", "D"];
    let keys: Vec<String> = rows.iter().flat_map(|r| (1..=4).map(move |c| format!("{r}{c}"))).collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    p.table("Cell", &["1", "2", "3", "4"], &keys);
    for (i, k) in keys.iter().enumerate() {
        p.b.set(&format!("Cell[{k}]"), At { row: (i / 4) as u32, col: (i % 4) as u32 });
    }
    let cells = |pick: &dyn Fn(usize, usize) -> bool| {
        let named: Vec<String> = (0..16).filter(|i| pick(i / 4, i % 4)).map(|i| format!("Cell[{}]", keys[i])).collect();
        format!("distinct({})", named.join(", "))
    };
    for (r, name) in rows.iter().enumerate() {
        p.clue(&format!("Row {name} has each number once"), &cells(&|rr, _| rr == r));
    }
    for c in 0..4 {
        p.clue(&format!("Column {} has each number once", c + 1), &cells(&|_, cc| cc == c));
    }
    for (r, c, name) in [(0, 0, "top left"), (0, 1, "top right"), (1, 0, "bottom left"), (1, 1, "bottom right")] {
        p.clue(&format!("The {name} box has each number once"), &cells(&|rr, cc| rr / 2 == r && cc / 2 == c));
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
    // A table for each kind of thing, whose rows say which house each is in.
    let tables: [(&str, &[&str]); 5] = [
        ("Nationality", &["English", "Spanish", "Ukrainian", "Norwegian", "Japanese"]),
        ("Colour", &["Red", "Green", "Ivory", "Yellow", "Blue"]),
        ("Pet", &["Dog", "Snails", "Fox", "Horse", "Zebra"]),
        ("Drink", &["Coffee", "Tea", "Milk", "Juice", "Water"]),
        ("Smoke", &["Old Gold", "Kools", "Chesterfield", "Lucky Strike", "Parliament"]),
    ];
    let mut p = Puzzle::new(b, "The street");
    for (table, rows) in tables {
        p.table(table, &houses, rows);
    }
    // The clues go first, so the search is narrow before the rest apply.
    p.clue("The Englishman lives in the red house", "Nationality[English] = Colour[Red]");
    p.clue("The Spaniard owns the dog", "Nationality[Spanish] = Pet[Dog]");
    p.clue("Coffee is drunk in the green house", "Drink[Coffee] = Colour[Green]");
    p.clue("The Ukrainian drinks tea", "Nationality[Ukrainian] = Drink[Tea]");
    p.clue("The green house is just right of the ivory house", "Colour[Green] = Colour[Ivory] + 1");
    p.clue("The Old Gold smoker owns snails", r#"Smoke["Old Gold"] = Pet[Snails]"#);
    p.clue("Kools are smoked in the yellow house", "Smoke[Kools] = Colour[Yellow]");
    p.clue("Milk is drunk in the middle house", "Drink[Milk] = 3");
    p.clue("The Norwegian lives in the first house", "Nationality[Norwegian] = 1");
    p.clue("The Chesterfield smoker lives next to the man with the fox", "abs(Smoke[Chesterfield] - Pet[Fox]) = 1");
    p.clue("Kools are smoked next to the house with the horse", "abs(Smoke[Kools] - Pet[Horse]) = 1");
    p.clue("The Lucky Strike smoker drinks juice", r#"Smoke["Lucky Strike"] = Drink[Juice]"#);
    p.clue("The Japanese smokes Parliament", "Nationality[Japanese] = Smoke[Parliament]");
    p.clue("The Norwegian lives next to the blue house", "abs(Nationality[Norwegian] - Colour[Blue]) = 1");
    for (table, _) in tables {
        p.rule(table, "Each in a different house", "distinct(rows)");
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
