//! The example problems, as workspaces. These write the files in `examples/`, through a real log,
//! and `tests/samples.rs` checks that what's committed there still means what it should.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::{CellType, Entity, Name, On, Op, Stored};
use serde_json::{Value, json};

use crate::formulas::{AT, At, Called, IS_A, LABEL, OPTIONS, Options, REQUIRES, VALUE, ValueCell};
use crate::notes::{Body, DRAWING, Drawing, NOTES};
use crate::plan::{ALTERNATIVE, DECISION, DEPENDS_ON, EXCLUDES, Done, Estimate, GOAL, PART_OF, RISK, TASK, UNCERTAIN, UNKNOWN, WITHIN, Weight};

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
    Sample { name: "handbook", title: "Publishing a handbook", about: "A first plan to read: a team wants to publish a handbook and hasn't decided how. There are three options, and a gap for one nobody has thought of. Each node's note says what it's there to show. See which work is safe to start whatever is decided, which depends on the decision, and when each option would finish. Pin an option in the bar at the top to see the plan as it would be, with nothing saved.", group: "plan", systems: &["plan", "schedule"], build: handbook },
    Sample { name: "critical-path", title: "Critical path", about: "A plain schedule with no decisions: dates go forward over the dependencies, and slack comes from following them backward.", group: "plan", systems: &["plan", "schedule"], build: critical_path },
    Sample { name: "invoice", title: "Invoice", about: "Two lines, a subtotal, tax and a total. Edit a number and only what it reaches reruns.", group: "sheet", systems: &[], build: invoice },
    Sample { name: "trip-budget", title: "Trip budget", about: "A budget with decisions left open: the total is exact in every world, and so is what would put it over.", group: "sheet", systems: &["plan"], build: trip_budget },
    Sample { name: "choosing-a-car", title: "Choosing a car", about: "Petrol and electric cars as tables: what every car has, and what only petrol or electric cars have. What each costs a year over mileage you can only guess at, and which can get to Edinburgh without stopping.", group: "sheet", systems: &["plan"], build: choosing_a_car },
    Sample { name: "compound-interest", title: "Compound interest", about: "A table of years. One formula on Year works out each from the year before, and only Year[0] and Year[5] are written: the rest are worked out as far as they're asked for. A top-up you might make in year 5 changes only the years after it.", group: "sheet", systems: &["plan"], build: compound_interest },
    Sample { name: "fibonacci", title: "Fibonacci", about: "A table where each row adds the two before it: prev.f + prev.prev.f. Only the first two rows are written, and asking for F[80] works out the rest, each once: 79 formula runs.", group: "sheet", systems: &[], build: fibonacci },
    Sample { name: "australia", title: "Colouring Australia", about: "Each region is a choice and each border a constraint. There are 18 colourings. Pin a region and see what that forces.", group: "puzzle", systems: &[], build: australia },
    Sample { name: "eight-queens", title: "Eight queens", about: "A table with a row for each row of the board, choosing the column its queen is in. One formula on the table says no two share a column or a diagonal. There are 92 boards.", group: "puzzle", systems: &[], build: eight_queens },
    Sample { name: "sudoku", title: "4×4 sudoku", about: "A table of sixteen cells, each choosing a number, and a clue for each row, column and box: distinct(Cell[A1].chosen, Cell[A2].chosen, …). There are 288 grids.", group: "puzzle", systems: &[], build: sudoku },
    Sample { name: "knights-and-knaves", title: "Knights and knaves", about: "A says “we are both knaves”. Knights always tell the truth, and knaves always lie.", group: "puzzle", systems: &[], build: knights_and_knaves },
    Sample { name: "case-file", title: "The Vane watch", about: "A case file. A watch went from Rosalind Vane's study one evening, and there are three people it could have been, or someone nobody has thought of. Each account is a node saying who gave it, with how far it's believed, and when each suspect could have reached the house is worked out from those. The Worlds tab gives how likely each suspect is, and what's most worth finding out next. Pin what you learn.", group: "reasoning", systems: &["plan"], build: case_file },
    Sample { name: "monty-hall", title: "Monty Hall", about: "A game show has three doors, with a car behind one and goats behind the other two. You pick a door. The host, Monty, knows where the car is: he opens one of the other two doors to show a goat, then offers you the chance to switch to the door that's left. Should you? Most people say it makes no difference, but switching wins the car two times in three, and sticking only one time in three. Here it's four questions asked in order: where the car is, which door you pick, which door Monty opens, and whether you stick or switch. Pin your pick and the door Monty opens to see the probability that the car is behind each door, and compare sticking with switching on the Decisions tab.", group: "reasoning", systems: &["plan"], build: monty_hall },
    Sample { name: "medical-test", title: "The positive test", about: "A condition affects one person in a hundred. The test for it comes back positive for 90% of the people who have it, but also for 5% of the people who don't. Your test comes back positive. What is the probability that you have the condition? It feels as if it should be about 90%, but it's only about 15%. People without the condition outnumber those with it by so much that most positive results are false alarms: of 10,000 people tested, 90 positives come from the 100 who have it, and 495 from the 9,900 who don't.", group: "reasoning", systems: &["plan"], build: medical_test },
    Sample { name: "bertrands-box", title: "Bertrand's box", about: "There are three boxes. One holds two gold coins, one holds two silver coins, and one holds a gold coin and a silver coin. You pick a box at random and take a coin from it without looking, and the coin is gold. What is the probability that the other coin in the same box is also gold? It's tempting to say a half, since the box must be either the one with two gold coins or the mixed one. The answer is two thirds: there are three gold coins you could be holding, all equally likely, and two of them are in the box with two gold. Here it's three questions, each asked inside the one before: which box, which coin you take from it, and which coin is left.", group: "reasoning", systems: &["plan"], build: bertrands_box },
    Sample { name: "two-children", title: "Two children", about: "A family has two children, and you're told that at least one of them is a boy. What is the probability that both are boys? The intuitive answer is a half, but it's a third: there are three equally likely families with at least one boy (an elder boy and a younger girl, an elder girl and a younger boy, and two boys), and only one of them has two boys. The answer changes if you're told something more specific. Pin the elder child as a boy and the probability that both are boys becomes a half, because that also rules out the family where only the younger is a boy.", group: "reasoning", systems: &["plan"], build: two_children },
    Sample { name: "socks", title: "Socks in the wash", about: "Two, three or four pairs of socks went in the wash, and you don't know which. Every pair is different from the others. You pull out three socks without looking, and no two of them match. What does that tell you about how many pairs went in? Before you looked, each number of pairs was equally likely. Afterwards two pairs is ruled out, because any three socks from two pairs must include a match, and four pairs is more likely than three, at about 59% to 41%, because three socks are less likely to include a match when there are more pairs to draw from. Each sock is drawn from those in the wash that aren't out already.", group: "reasoning", systems: &["plan"], build: socks },
    Sample { name: "free-will", title: "Free will: three claims that can't all be true", about: "Three claims about free will each seem right on their own: everything that happens is settled by what came before, an act that was settled beforehand isn't free, and we do act freely. They can't all be true together, so at least one has to go, and each way of giving one up is a position philosophers hold. Each claim is something you accept or reject, and one requirement says they aren't all accepted. Pin the two you're surest of and see that the third is forced, and which position that makes you.", group: "philosophy", systems: &["plan"], build: free_will },
    Sample { name: "trolley", title: "The trolley problem", about: "A runaway trolley will kill five people. In one case you can pull a switch to send it down a side track, where it will kill one. In another you can only stop it by pushing a man off a footbridge into its path. Most people say pulling the switch is all right and pushing the man is not, though five are saved at the cost of one either way. Here three moral principles are each accepted or rejected, what they permit in each case follows by one rule, and those two common judgements go in as evidence. What's left are the principles that fit your judgements, and they settle a third case, the loop, in a way you may not like. That back and forth between principles and cases is what philosophers call reflective equilibrium.", group: "philosophy", systems: &["plan"], build: trolley },
    Sample { name: "reason-or-experience", title: "Reason or experience?", about: "Where does knowledge come from? One answer is reason alone, and the opposite answer is experience alone. Each explains something the other can't: reason explains why mathematics couldn't have been otherwise, and experience explains why we have to look to find out how the world is. Put in both as things any answer has to explain, and neither answer survives. What's left is the gap: an answer nobody has named yet. This is a dialectic, with a thesis, its antithesis, and the need for a synthesis that keeps what each got right. Name one, and say what it explains, to close the gap.", group: "philosophy", systems: &["plan"], build: reason_or_experience },
    Sample { name: "zebra", title: "The zebra puzzle", about: "Who owns the zebra, and who drinks water? A table for each kind of thing, whose rows choose a house, and the clues written as they read: Nationality[English].chosen = Colour[Red].chosen. About 3 × 10¹⁷ combinations, and the clues leave exactly one.", group: "puzzle", systems: &[], build: zebra },
];

/// Changes when samples mean something different, so an old working copy gets replaced, not misread.
pub const FORMAT: &str = "21";

/// Ops for a sample, referring to nodes by name while they're being written.
#[derive(Default)]
pub struct Build {
    pub ops: Vec<Op>,
    ids: BTreeMap<String, Entity>,
    given: BTreeSet<(Entity, String)>,
}

impl Build {
    /// The node with this name, created if it's new.
    pub fn e(&mut self, name: &str) -> Entity {
        if let Some(e) = self.ids.get(name) {
            return *e;
        }
        self.named(name, name)
    }

    /// A node known here by `key`, and called something else.
    pub fn named(&mut self, key: &str, name: &str) -> Entity {
        let e = Entity::new();
        self.ids.insert(key.to_string(), e);
        self.ops.push(Op::Spawn { id: e });
        self.ops.push(Op::set(e, Name(name.to_string())));
        e
    }

    /// Give a node a component.
    pub fn add(&mut self, name: &str, component: &str) -> &mut Self {
        let e = self.e(name);
        if self.given.insert((e, component.to_string())) {
            self.ops.push(Op::add(e, component));
        }
        self
    }

    /// Write a cell of a component the node has, giving it the component if it hasn't.
    pub fn set<C: CellType>(&mut self, name: &str, component: &str, value: C) -> &mut Self {
        self.add(name, component);
        let e = self.e(name);
        self.ops.push(Op::set(e, value));
        self
    }

    /// A node's value, typed in.
    pub fn value(&mut self, name: &str, v: f64) -> &mut Self {
        self.set(name, VALUE, ValueCell::num(v))
    }

    /// A node's value, as a formula.
    pub fn formula(&mut self, name: &str, text: &str) -> &mut Self {
        self.add(name, VALUE);
        self.formula_on(name, ValueCell::NAME, text)
    }

    pub fn note(&mut self, name: &str, text: &str) -> &mut Self {
        self.set(name, NOTES, Body(text.to_string()))
    }

    /// A task taking this many days.
    pub fn task(&mut self, name: &str, days: f64) -> &mut Self {
        self.set(name, TASK, Estimate::days(days))
    }

    /// One of the workspace's own cells, as a value.
    pub fn put(&mut self, name: &str, cell: &str, v: Value) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set_value(e, cell, v));
        self
    }

    /// A cell, as a formula.
    pub fn formula_on(&mut self, name: &str, cell: &str, text: &str) -> &mut Self {
        let e = self.e(name);
        self.ops.push(Op::set_formula(e, cell, text));
        self
    }

    /// An edge with this component.
    pub fn link(&mut self, from: &str, to: &str, component: &str) -> &mut Self {
        let (a, b) = (self.e(from), self.e(to));
        self.ops.push(Op::link(a, b, component));
        self
    }

    /// A cell on the edge from one node to another.
    pub fn on_edge(&mut self, from: &str, to: &str, cell: &str, v: Value) -> &mut Self {
        let (a, b) = (self.e(from), self.e(to));
        self.ops.push(Op::Set { on: On::Edge { from: a, to: b }, cell: cell.to_string(), to: Some(Stored::Value(v)) });
        self
    }

    /// What a formula on `from` calls `to`.
    pub fn label(&mut self, from: &str, to: &str, label: &str) -> &mut Self {
        self.link(from, to, LABEL).on_edge(from, to, Called::NAME, json!(label))
    }

    /// One of a decision's options, believed this far against the others before any evidence.
    pub fn option(&mut self, option: &str, decision: &str, weight: Option<f64>) -> &mut Self {
        self.link(option, decision, ALTERNATIVE);
        if let Some(w) = weight {
            self.on_edge(option, decision, Weight::NAME, json!(w));
        }
        self
    }
}

fn handbook(b: &mut Build) {
    // A plan small enough to read at once, with one of everything: a goal, a decision held open with a gap
    // among its options, work needed whichever way it goes, work two options share, work that waits on
    // other work, and a risk. Each says what it's there to show.
    let (goal, how) = ("Publish the handbook", "How do we publish it?");
    b.add(goal, GOAL).note(goal, "A goal: the result that's wanted. It's done when everyone on the team can open the current version. Everything beneath it is there to get it done.");
    b.add(how, DECISION).link(how, goal, PART_OF);
    b.note(how, "A decision held open. Nothing has been chosen, so the plan is every way it could go at once, and each piece of work says when it's needed. Pin an option in the bar at the top to see the plan as it would be then, with nothing saved. Choose one on the Decisions tab when you're sure.");
    for (option, about) in [
        ("As a printed booklet", "An option. Its work is only needed if this is the one taken: a day to agree approval, two to lay out and three to print, so it's finished after five."),
        ("As a PDF", "An option. It shares the layout with the booklet, then takes five days to tag, so it's finished after seven."),
        ("As a website", "An option. It shares nothing with the other two, and is finished after four days."),
    ] {
        b.option(option, how, None).note(option, about);
    }
    b.add("Something else", UNKNOWN).option("Something else", how, None);
    b.note("Something else", "A gap: an option nobody has named yet. While it's here the plan admits it can't say when it will finish if this is how it goes. Name the option, or close the gap to say these three are all there are.");
    b.task("Agree who approves it", 1.0).link("Agree who approves it", how, PART_OF);
    b.note("Agree who approves it", "Part of the decision itself, so it's needed however the handbook is published. That makes it safe to start now, before deciding.");
    b.task("Lay out the pages", 2.0).link("Lay out the pages", "As a printed booklet", PART_OF).link("Lay out the pages", "As a PDF", PART_OF);
    b.note("Lay out the pages", "Part of two options, so it's needed if either is taken and not if the website is. It reads as depending on the decision: likely to be wanted, but not certain.");
    b.task("Get it printed", 3.0).link("Get it printed", "As a printed booklet", PART_OF).link("Get it printed", "Lay out the pages", DEPENDS_ON);
    b.note("Get it printed", "Waits for the layout, so it can't start until that's done. Its dates follow from that.");
    b.task("Tag it for screen readers", 5.0).link("Tag it for screen readers", "As a PDF", PART_OF).link("Tag it for screen readers", "Lay out the pages", DEPENDS_ON);
    b.note("Tag it for screen readers", "Also waits for the layout. It's the longest piece of work here, which is what makes the PDF the slowest option.");
    b.task("Build the site", 4.0).link("Build the site", "As a website", PART_OF);
    b.note("Build the site", "Only needed for the website, and waits for nothing.");
    b.add("The tagging tool mangles tables", RISK).link("The tagging tool mangles tables", "Tag it for screen readers", "contradicts");
    b.note("The tagging tool mangles tables", "A risk: something that could go wrong, linked to the work it threatens. It only matters in the worlds where that work is needed.");
}

fn critical_path(b: &mut Build) {
    b.task("Lay foundations", 3.0).task("Wire the rooms", 2.0).task("Frame the walls", 4.0).task("Paint", 2.0);
    b.link("Wire the rooms", "Lay foundations", DEPENDS_ON);
    b.link("Frame the walls", "Lay foundations", DEPENDS_ON);
    b.link("Paint", "Wire the rooms", DEPENDS_ON);
    b.link("Paint", "Frame the walls", DEPENDS_ON);
    b.put("Lay foundations", Done::NAME, json!(true));
}

fn invoice(b: &mut Build) {
    for (line, qty, price) in [("Widgets", 3.0, 10.0), ("Gears", 2.0, 25.0)] {
        let (q, p) = (format!("{line}: quantity"), format!("{line}: price"));
        b.value(&q, qty).value(&p, price);
        b.formula(line, "quantity * price").label(line, &q, "quantity").label(line, &p, "price");
        b.label("Subtotal", line, "line");
    }
    b.formula("Subtotal", "sum(line)");
    b.value("Tax rate", 0.2);
    b.formula("Tax", "subtotal * rate").label("Tax", "Subtotal", "subtotal").label("Tax", "Tax rate", "rate");
    b.formula("Total", "subtotal + tax").label("Total", "Subtotal", "subtotal").label("Total", "Tax", "tax");
}

fn trip_budget(b: &mut Build) {
    // Options are ordinary nodes, the same as in a plan, so one could have parts and dependencies too.
    for (decision, options) in [("Travel", [("Train", 80.0), ("Plane", 120.0)]), ("Stay", [("Hostel", 60.0), ("Hotel", 150.0)])] {
        b.add(decision, DECISION);
        for (option, price) in options {
            b.value(option, price).option(option, decision, None);
        }
    }
    b.formula("Trip", "travel.chosen.value + stay.chosen.value").label("Trip", "Travel", "travel").label("Trip", "Stay", "stay");
    b.formula("Over £200", "trip > 200").label("Over £200", "Trip", "trip");
}

fn choosing_a_car(b: &mut Build) {
    b.value("Years kept", 5.0);
    // Not something to decide: it'll be whatever it is, and these are guesses at how likely each is.
    b.add("Miles a year", DECISION).add("Miles a year", UNCERTAIN);
    for (option, miles, weight) in [("Light use", 6000.0, 1.0), ("Typical", 10000.0, 2.0), ("Heavy", 16000.0, 1.0)] {
        b.value(option, miles).option(option, "Miles a year", Some(weight));
    }
    // What a mile costs follows from what a car runs on, so each kind says how, and each car only says
    // how far it goes on a litre or a kilowatt-hour.
    b.value("Fuel price", 1.45).value("Electricity price", 0.28);
    b.note("Petrol car", "A mile costs the fuel it burns: a gallon is 4.546 litres. Each says its own mpg.");
    b.formula_on("Petrol car", "cost_per_mile", "fuel * 4.546 / self.mpg").label("Petrol car", "Fuel price", "fuel");
    b.note("Electric car", "A mile costs the electricity it takes. Each says its own kWh a mile.");
    b.formula_on("Electric car", "cost_per_mile", "power * self.kwh_per_mile").label("Electric car", "Electricity price", "power");
    // Whatever the car it lands on says is read through it, as car.chosen.price.
    b.add("Which car?", DECISION);
    // Three tables. Every car has a price, insurance and a range, so those are Car's. Only petrol cars
    // have an mpg, and only electric ones a kWh a mile. Petrol car[Hatchback] and Car[Hatchback] are two
    // parts of one car, and each reads what the other says.
    b.link("Petrol car", "Car", IS_A).link("Electric car", "Car", IS_A);
    // Car says what every car has. It can't say what they come to, so each row says its own.
    for cell in ["price", "insurance", "range"] {
        b.formula_on("Car", cell, "undefined");
    }
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
        b.option(&row, "Which car?", None);
        b.put(&format!("{kind}[{car}]"), cell, json!(v));
    }
    // Losing 15% of its value each year it's kept, running it, and insuring it.
    b.formula("Yearly cost", "car.chosen.price * (1 - 0.85 ^ years) / years + miles.chosen.value * car.chosen.cost_per_mile + car.chosen.insurance");
    b.label("Yearly cost", "Which car?", "car").label("Yearly cost", "Years kept", "years").label("Yearly cost", "Miles a year", "miles");
    b.value("Miles to Edinburgh", 400.0);
    let trip = "Gets to Edinburgh without stopping";
    b.formula(trip, "car.chosen.range >= miles").label(trip, "Which car?", "car").label(trip, "Miles to Edinburgh", "miles");
}

fn compound_interest(b: &mut Build) {
    // One formula for every year, on the table. Year[0] says its own, so it's where the chain starts.
    b.value("Rate", 0.05);
    b.formula_on("Year", "balance", "prev.balance * (1 + rate)").label("Year", "Rate", "rate");
    // Only rows with something of their own are written. The rest are there to be read, as far as asked.
    b.put("Year[0]", "balance", json!(1000.0));
    // A choice five years in: the years before it have one balance, and the years after have one for each answer.
    b.add("Top up in year 5?", DECISION);
    for (option, amount) in [("No top-up", 0.0), ("Add 500", 500.0)] {
        b.value(option, amount).option(option, "Top up in year 5?", None);
    }
    b.formula_on("Year[5]", "balance", "prev.balance * (1 + rate) + top_up.chosen.value");
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
        b.formula(root, "all(constraint)").add(root, REQUIRES);
        Puzzle { b, root: root.to_string(), next: 0 }
    }

    fn choice(&mut self, name: &str, options: &[&str]) {
        self.b.set(name, OPTIONS, Options(options.iter().map(|s| s.to_string()).collect()));
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
        self.constraint(&[("a", a), ("b", b)], format!("{a} ≠ {b}"), "a.chosen != b.chosen");
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
        self.b.formula(table, text).note(table, said);
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
    p.b.set("Map of Australia", DRAWING, Drawing(include_str!("drawings/australia.ts").to_string()));
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
    p.rule("Queen", "No two share a column or a diagonal", "distinct(q in rows: q.chosen) && distinct(q in rows: q.chosen.value + q.row) && distinct(q in rows: q.chosen.value - q.row)");
}

fn sudoku(b: &mut Build) {
    let mut p = Puzzle::new(b, "A grid");
    p.b.set("Board with its boxes", DRAWING, Drawing(include_str!("drawings/sudoku.ts").to_string()));
    let rows = ["A", "B", "C", "D"];
    let keys: Vec<String> = rows.iter().flat_map(|r| (1..=4).map(move |c| format!("{r}{c}"))).collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    p.table("Cell", &["1", "2", "3", "4"], &keys);
    for (i, k) in keys.iter().enumerate() {
        p.b.set(&format!("Cell[{k}]"), AT, At { row: (i / 4) as u32, col: (i % 4) as u32 });
    }
    let cells = |pick: &dyn Fn(usize, usize) -> bool| {
        let named: Vec<String> = (0..16).filter(|i| pick(i / 4, i % 4)).map(|i| format!("Cell[{}].chosen", keys[i])).collect();
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
    p.b.formula(said, r#"a.chosen.value = "knave" && b.chosen.value = "knave""#).label(said, "A", "a").label(said, "B", "b");
    p.constraint(&[("speaker", "A"), ("said", said)], "A is a knight exactly when that's true".to_string(), r#"(speaker.chosen.value = "knight") = said"#);
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
    p.clue("The Englishman lives in the red house", "Nationality[English].chosen = Colour[Red].chosen");
    p.clue("The Spaniard owns the dog", "Nationality[Spanish].chosen = Pet[Dog].chosen");
    p.clue("Coffee is drunk in the green house", "Drink[Coffee].chosen = Colour[Green].chosen");
    p.clue("The Ukrainian drinks tea", "Nationality[Ukrainian].chosen = Drink[Tea].chosen");
    p.clue("The green house is just right of the ivory house", "Colour[Green].chosen.value = Colour[Ivory].chosen.value + 1");
    p.clue("The Old Gold smoker owns snails", r#"Smoke["Old Gold"].chosen = Pet[Snails].chosen"#);
    p.clue("Kools are smoked in the yellow house", "Smoke[Kools].chosen = Colour[Yellow].chosen");
    p.clue("Milk is drunk in the middle house", "Drink[Milk].chosen.value = 3");
    p.clue("The Norwegian lives in the first house", "Nationality[Norwegian].chosen.value = 1");
    p.clue("The Chesterfield smoker lives next to the man with the fox", "abs(Smoke[Chesterfield].chosen.value - Pet[Fox].chosen.value) = 1");
    p.clue("Kools are smoked next to the house with the horse", "abs(Smoke[Kools].chosen.value - Pet[Horse].chosen.value) = 1");
    p.clue("The Lucky Strike smoker drinks juice", r#"Smoke["Lucky Strike"].chosen = Drink[Juice].chosen"#);
    p.clue("The Japanese smokes Parliament", "Nationality[Japanese].chosen = Smoke[Parliament].chosen");
    p.clue("The Norwegian lives next to the blue house", "abs(Nationality[Norwegian].chosen.value - Colour[Blue].chosen.value) = 1");
    for (table, _) in tables {
        p.rule(table, "Each in a different house", "distinct(r in rows: r.chosen)");
    }
}

fn case_file(b: &mut Build) {
    b.note(
        "The case",
        "Saturday evening at Rosalind Vane's house. Her late husband's gold watch was in a locked case in the study. Clara saw it there at 9:00. Rosalind found the case open and the watch gone at 10:15. Times here are minutes past nine, so 9:40 is 40.",
    );

    // Something nobody chose: it happened one way, and these are how far each was believed before the evidence.
    let who = "Who took it?";
    b.add(who, DECISION).add(who, UNCERTAIN);
    let suspects: [(&str, f64, &str); 4] = [
        ("Tom", 3.0, "Rosalind's nephew. He owes money, and he knew where the key to the case was kept."),
        ("Mr Pryce", 3.0, "The neighbour. He collects watches, and offered to buy this one in May. She said no."),
        ("Clara", 2.0, "The housekeeper. She has a key to the study, and was in the house all evening."),
        ("Someone else", 1.0, "Someone nobody has thought of yet."),
    ];
    for (name, weight, about) in suspects {
        b.note(name, about).option(name, who, Some(weight));
    }
    b.add("Someone else", UNKNOWN);

    // What's in doubt, each a decision nobody made, whose options are the accounts.
    // The question's answer is which account is true. What follows from it is a cell named for what it is,
    // which each account says, and which is read through the question for whichever is true.
    let account = |b: &mut Build, question: &str, cell: &str, options: &[(&str, Value, f64, &str)]| {
        b.add(question, DECISION).add(question, UNCERTAIN);
        for (option, value, weight, said) in options {
            b.put(option, cell, value.clone()).note(option, said).option(option, question, Some(*weight));
        }
    };
    account(
        b,
        "When did Tom leave the Red Lion?",
        "left_at",
        &[
            ("At 9:40, as the barman says", json!(40), 3.0, "The barman remembers calling time on Tom's tab at twenty to ten."),
            ("At about 9:10, as Mrs Dale says", json!(10), 1.0, "Mrs Dale was at the next table, and thinks he went just after nine. She'd had a few."),
        ],
    );
    account(
        b,
        "How did Tom get to the house?",
        "minutes",
        &[
            ("By bike, 25 minutes", json!(25), 2.0, "Tom says his car was at the garage, so he cycled."),
            ("By car, 10 minutes", json!(10), 1.0, "The garage says he collected the car on Friday."),
        ],
    );
    account(
        b,
        "How did Mr Pryce get home?",
        "at_house_by",
        &[
            ("On the 9:20 train, at the house by 10:10", json!(70), 3.0, "He has a ticket for the 9:20 from town. It gets in at 9:55, and the walk from the station is a quarter of an hour."),
            ("He drove, home by 9:05", json!(5), 1.0, "His neighbour on the other side heard a car on the drive just after nine."),
        ],
    );
    account(
        b,
        "Did Clara go out into the garden?",
        "went_out",
        &[
            ("No, as she says", json!(false), 2.0, "Clara says she was in the kitchen with the radio on all evening."),
            ("Yes, as the cook thinks", json!(true), 1.0, "The cook found the back door unbolted at half past nine."),
        ],
    );

    // When each could have been at the house, worked out from the accounts.
    b.formula_on("Tom", "arrives", "pub.chosen.left_at + journey.chosen.minutes").label("Tom", "When did Tom leave the Red Lion?", "pub").label("Tom", "How did Tom get to the house?", "journey");
    b.formula_on("Mr Pryce", "arrives", "trip.chosen.at_house_by").label("Mr Pryce", "How did Mr Pryce get home?", "trip");
    b.put("Clara", "arrives", json!(0)).formula_on("Clara", "from_garden", "garden.chosen.went_out").label("Clara", "Did Clara go out into the garden?", "garden");
    b.put("Someone else", "arrives", json!(0));
    for name in ["Tom", "Mr Pryce", "Someone else"] {
        b.put(name, "from_garden", json!(true));
    }

    // The evidence, which rules out the worlds it doesn't fit.
    let evidence = |b: &mut Build, name: &str, formula: &str, about: &str| {
        b.formula(name, formula).add(name, REQUIRES).note(name, about).label(name, who, "who");
    };
    evidence(b, "Whoever took it was in the house by 10:00", "who.chosen.arrives <= 60", "The case was locked at 9:00 and empty at 10:15, and it would take a while to find the key, open it, and get away.");
    evidence(b, "They came in from the garden", "who.chosen.from_garden", "Fresh mud on the study carpet, from the garden door to the case.");
}

/// Something in doubt: a decision nobody makes, whose options each say `cell`, believed this far.
fn doubt(b: &mut Build, question: &str, cell: &str, options: &[(&str, Value, f64)]) {
    b.add(question, DECISION).add(question, UNCERTAIN);
    for (option, value, weight) in options {
        b.put(option, cell, value.clone()).option(option, question, Some(*weight));
    }
}

/// What was seen, which rules out the worlds it doesn't fit.
fn seen(b: &mut Build, name: &str, formula: &str, about: &str) {
    b.formula(name, formula).add(name, REQUIRES).note(name, about);
}

fn monty_hall(b: &mut Build) {
    b.note("The game", "Three doors, with a car behind one and goats behind the other two. You pick a door. Monty knows where the car is: he opens another door, and always shows a goat. Then you can stick with your door, or switch to the one that's left. Nothing here is fixed: pin your pick and the door Monty opens to see where the car probably is, and compare sticking with switching.");
    // Four questions, each landing on a door, and each after the ones it leaves out. The doors are the
    // same three nodes every time, so "you win" is two questions landing on one.
    let doors: Vec<String> = (1..=3).map(|d| format!("Door[{d}]")).collect();
    let (car, pick, monty, left) = ("Where's the car?", "Which door do you pick?", "Which door does Monty open?", "Which door is left?");
    b.add(car, DECISION).add(car, UNCERTAIN);
    b.add(pick, DECISION);
    for door in &doors {
        b.option(door, car, Some(1.0)).option(door, pick, None);
    }
    // Any door but the car's and yours, each as likely as the other when there are two. With one left,
    // that's the one: nothing has to say so.
    b.add(monty, DECISION).add(monty, UNCERTAIN).link(monty, car, EXCLUDES).link(monty, pick, EXCLUDES);
    b.note(monty, "Never yours, and never the car's. When the car is behind yours he has two doors to choose from, and is as likely to open either. Otherwise he has one.");
    // Not yours and not the one he opened, which is always exactly one door.
    b.add(left, DECISION).add(left, UNCERTAIN).link(left, pick, EXCLUDES).link(left, monty, EXCLUDES);
    for door in &doors {
        b.option(door, monty, Some(1.0)).option(door, left, Some(1.0));
    }
    // Yours to choose, so it isn't believed: each option is compared by what follows from it. The two
    // options have nothing to say but their names, so the decision lists them.
    let choice = "Stick or switch?";
    b.add(choice, DECISION).set(choice, OPTIONS, Options(vec!["Stick".to_string(), "Switch".to_string()]));
    // The door you open in the end is one of two other answers, and which is up to you.
    let ends = "The door you end on";
    b.formula(ends, r#"if(choice.chosen.value = "Switch", left.chosen, pick.chosen)"#);
    b.label(ends, choice, "choice").label(ends, left, "left").label(ends, pick, "pick");
    b.formula("You win the car", "ends = car.chosen").label("You win the car", ends, "ends").label("You win the car", car, "car");
}

fn medical_test(b: &mut Build) {
    let have = "Do you have it?";
    b.add(have, DECISION).add(have, UNCERTAIN).note(have, "Before the test, you're anyone: one in a hundred has it.");
    b.option("You have it", have, Some(1.0)).option("You don't", have, Some(99.0));
    // How the test behaves is a separate doubt for each case, and only the one that applies is read.
    doubt(b, "The test, if you have it", "positive", &[("It catches it", json!(true), 90.0), ("It misses it", json!(false), 10.0)]);
    doubt(b, "The test, if you don't", "positive", &[("A false alarm", json!(true), 5.0), ("It's clear", json!(false), 95.0)]);
    b.formula_on("You have it", "positive", "test.chosen.positive").label("You have it", "The test, if you have it", "test");
    b.formula_on("You don't", "positive", "test.chosen.positive").label("You don't", "The test, if you don't", "test");
    seen(b, "The test came back positive", "have.chosen.positive", "Of 10,000 people, 100 have it and 90 of those test positive. 9,900 don't, and 495 of those test positive too. So 90 of the 585 positives have it.");
    b.label("The test came back positive", have, "have");
}

fn bertrands_box(b: &mut Build) {
    // Three questions, one inside another: a box, a coin from that box, and the coin that's then left in it.
    let (which, took, left) = ("Which box?", "Which coin do you take?", "Which coin is left in the box?");
    b.add(which, DECISION).add(which, UNCERTAIN);
    b.add(took, DECISION).add(took, UNCERTAIN).link(took, which, WITHIN);
    b.add(left, DECISION).add(left, UNCERTAIN).link(left, which, WITHIN).link(left, took, EXCLUDES);
    b.note(took, "One of the two coins in whichever box it is, each as likely as the other.");
    let boxes: [(&str, [(&str, bool); 2]); 3] = [
        ("Two gold", [("Gold coin 1", true), ("Gold coin 2", true)]),
        ("One of each", [("Gold coin 3", true), ("Silver coin 1", false)]),
        ("Two silver", [("Silver coin 2", false), ("Silver coin 3", false)]),
    ];
    for (name, coins) in boxes {
        b.option(name, which, Some(1.0));
        for (coin, gold) in coins {
            b.put(coin, "gold", json!(gold)).link(coin, name, PART_OF).option(coin, took, Some(1.0)).option(coin, left, Some(1.0));
        }
    }
    seen(b, "The coin you took is gold", "took.chosen.gold", "There are three gold coins you could be holding, each as likely as the others, and two of them are in the box with two gold.");
    b.label("The coin you took is gold", took, "took");
    b.formula("The other coin is gold too", "left.chosen.gold").label("The other coin is gold too", left, "left");
}

fn two_children(b: &mut Build) {
    // Each child is one or the other, as likely as not, and neither answer has anything more to say, so
    // each question lists them.
    for child in ["The elder", "The younger"] {
        b.add(child, DECISION).add(child, UNCERTAIN).set(child, OPTIONS, Options(vec!["boy".to_string(), "girl".to_string()]));
    }
    let both = [("elder", "The elder"), ("younger", "The younger")];
    seen(b, "At least one is a boy", r#"elder.chosen.value = "boy" || younger.chosen.value = "boy""#, "This rules out only two girls. Of the three families left, boy and boy, boy and girl, girl and boy, one has two boys.");
    b.formula("Both are boys", r#"elder.chosen.value = "boy" && younger.chosen.value = "boy""#);
    for (label, child) in both {
        b.label("At least one is a boy", child, label).label("Both are boys", child, label);
    }
}

fn socks(b: &mut Build) {
    b.note("The wash", "Every pair is different from the others. You pull three socks out without looking, and no two of them match.");
    let pairs = "How many pairs went in?";
    b.add(pairs, DECISION).add(pairs, UNCERTAIN);
    for n in 2..=4 {
        b.option(&format!("{n} pairs"), pairs, Some(1.0));
    }
    // Each sock out is one of those in the wash, which depends on how many pairs went in, and isn't one
    // that's already out.
    let draws = ["The first sock out", "The second sock out", "The third sock out"];
    for (i, draw) in draws.iter().enumerate() {
        b.add(draw, DECISION).add(draw, UNCERTAIN).link(draw, pairs, WITHIN);
        for earlier in &draws[..i] {
            b.link(draw, earlier, EXCLUDES);
        }
    }
    for sock in 1..=8 {
        let name = format!("Sock[{sock}]");
        let pair = (sock + 1) / 2;
        b.put(&name, "pair", json!(pair));
        // A sock is in every load with at least as many pairs as its own.
        for n in pair.max(2)..=4 {
            b.link(&name, &format!("{n} pairs"), PART_OF);
        }
        for draw in draws {
            b.option(&name, draw, Some(1.0));
        }
    }
    seen(b, "The first three are all different", "distinct(first.chosen.pair, second.chosen.pair, third.chosen.pair)", "With two pairs it can't happen: the third sock has to match one of the first two. It's likelier with four pairs than with three, so it counts for the bigger load.");
    for (label, draw) in ["first", "second", "third"].into_iter().zip(draws) {
        b.label("The first three are all different", draw, label);
    }
}

/// Something that's either accepted or rejected, as likely as not before anything's said about it.
fn claim(b: &mut Build, name: &str, about: &str) {
    b.add(name, DECISION).add(name, UNCERTAIN).set(name, OPTIONS, Options(vec!["accept".to_string(), "reject".to_string()])).note(name, about);
}

fn free_will(b: &mut Build) {
    let (settled, unfree, free) = ("Everything that happens is settled by what came before", "An act that was settled beforehand isn't free", "We act freely");
    claim(b, settled, "Determinism. Every event, including everything you do, follows from the state of the world before it and the laws of nature.");
    claim(b, unfree, "Incompatibilism. If what you'd do was already settled before you were born, you couldn't have done otherwise, and so it wasn't up to you.");
    claim(b, free, "What it feels like from the inside, and what blame and praise seem to assume: at least some of what we do is up to us.");
    let all = [("settled", settled), ("unfree", unfree), ("free", free)];
    seen(b, "They can't all be true", r#"!(settled.chosen.value = "accept" && unfree.chosen.value = "accept" && free.chosen.value = "accept")"#, "If everything is settled beforehand, and what's settled beforehand isn't free, then nothing we do is free. So accepting any two of them means rejecting the third.");
    // Each way of giving up exactly one is a position with a name.
    for (name, formula, about) in [
        ("Hard determinism", r#"settled.chosen.value = "accept" && unfree.chosen.value = "accept" && free.chosen.value = "reject""#, "Gives up free will: everything is settled, that rules freedom out, so we aren't free."),
        ("Libertarianism", r#"settled.chosen.value = "reject" && unfree.chosen.value = "accept" && free.chosen.value = "accept""#, "Gives up determinism: we're free, freedom needs an open future, so not everything is settled."),
        ("Compatibilism", r#"settled.chosen.value = "accept" && unfree.chosen.value = "reject" && free.chosen.value = "accept""#, "Gives up the idea that the two conflict: an act can be settled beforehand and still be yours, if it comes from what you want."),
    ] {
        b.formula(name, formula).note(name, about);
        for (label, node) in all {
            b.label(name, node, label);
        }
    }
    for (label, node) in all {
        b.label("They can't all be true", node, label);
    }
}

fn trolley(b: &mut Build) {
    let (save, kill, means) = ("Do what saves the most lives", "Never kill, whatever it prevents", "Never use a person merely as a means");
    claim(b, save, "What matters is how things turn out, and five deaths are worse than one.");
    claim(b, kill, "Some things you may not do, and killing someone is one of them, even to stop more deaths that you wouldn't be the one causing.");
    claim(b, means, "You may not treat a person as a tool: it's one thing for someone to die as a side effect of what you do, and another for their death to be how you do it.");
    // One rule for every case: you may act if saving more is what counts, killing isn't ruled out
    // altogether, and it doesn't use someone as a means in a way that's ruled out.
    b.note("Case", "Each case says whether stopping the trolley uses the one who dies as the means of stopping it. Whether you may act follows from that and from the three principles, by the same rule for every case.");
    b.formula_on("Case", "permitted", r#"save.chosen.value = "accept" && kill.chosen.value = "reject" && !(means.chosen.value = "accept" && self.uses_someone)"#);
    b.label("Case", save, "save").label("Case", kill, "kill").label("Case", means, "means");
    for (case, uses, about) in [
        ("Switch", false, "You can pull a switch to send the trolley down a side track, where one person is standing. They die because they happen to be there: if the side track were empty, the five would be saved just the same."),
        ("Footbridge", true, "You're on a footbridge beside a large man. Pushing him onto the track would stop the trolley. His body is what stops it: without him there, the five would die."),
        ("Loop", true, "As the switch, except the side track loops back to the main one. The trolley would come round and kill the five, but the one person on the loop is heavy enough to stop it. So this time their body is what saves the five."),
    ] {
        let row = format!("Case[{case}]");
        b.put(&row, "uses_someone", json!(uses)).note(&row, about);
    }
    seen(b, "You may pull the switch", "Case[Switch].permitted", "What most people say. It rules out any set of principles that forbids it.");
    seen(b, "You may not push the man", "!Case[Footbridge].permitted", "Also what most people say, and it's what makes this a problem: the numbers are the same as for the switch.");
    b.formula("You may send it round the loop", "Case[Loop].permitted");
    b.note("You may send it round the loop", "Not something put in, but what follows from the principles that fit the other two judgements. If it comes out a way you can't accept, something has to give: one of the judgements, or the principles, or the rule that connects them.");
}

fn reason_or_experience(b: &mut Build) {
    let answer = "Where does knowledge come from?";
    b.add(answer, DECISION).add(answer, UNCERTAIN);
    b.note(answer, "Not yours to choose: one answer is right. Each answer says what it can explain, and what any answer has to explain is put in as evidence.");
    let answers: [(&str, bool, bool, &str); 2] = [
        ("From reason alone", true, false, "The thesis. Real knowledge is worked out by thinking, as geometry is, and the senses only mislead. This explains why mathematics is certain. It can't explain why thinking alone never tells you what's in the next room."),
        ("From experience alone", false, true, "The antithesis. The mind starts empty, and everything in it comes through the senses. This explains why we have to look. It can't explain why two and two couldn't have made five: experience only says what has happened so far."),
    ];
    for (name, necessity, observation, about) in answers {
        b.put(name, "explains_necessity", json!(necessity)).put(name, "explains_observation", json!(observation)).note(name, about).option(name, answer, Some(2.0));
    }
    b.add("Something else", UNKNOWN).option("Something else", answer, Some(1.0));
    b.note("Something else", "A gap: an answer nobody has named. Once both things to explain are in, it's the only one left, which is the plan's way of saying a synthesis is needed. To name one, add an option that says it explains both, such as that experience supplies what we know and the mind supplies the form it has to take, and then close this gap.");
    // An answer nobody has named hasn't said it can't, so only an answer that says it can't is ruled out.
    seen(b, "It has to explain why mathematics couldn't be otherwise", "or(answer.chosen.explains_necessity, true)", "Two and two don't just happen to make four. An answer has to say where that kind of certainty comes from.");
    seen(b, "It has to explain why we have to look", "or(answer.chosen.explains_observation, true)", "Nobody can work out from an armchair what the weather is. An answer has to say why finding out about the world takes observation.");
    for evidence in ["It has to explain why mathematics couldn't be otherwise", "It has to explain why we have to look"] {
        b.label(evidence, answer, "answer");
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
