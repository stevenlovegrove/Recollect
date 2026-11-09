//! Toy 3: combinatorial logic puzzles, as a graph of choices and constraints. Each answer here
//! is known independently: published counts, or checked by plain enumeration in the test.
//!
//! A choice is a node with `options`. A constraint is a node whose value is a formula over the
//! choices it has labelled, and the puzzle's value is `all(constraint)`. The worlds it's true in are
//! the solutions.

use std::collections::BTreeMap;
use std::time::Instant;

mod support;

use recollect_graph::worlds::{Bool, Space, Var};
use recollect_graph::{CellType, Entity, Registry};
use recollect_systems::formulas::{Called, LABEL, OPTIONS, Options, REQUIRES, VALUE, ValueCell, install, truth};
use support::N;

fn puzzle_systems(reg: &mut Registry) {
    install(reg);
}

struct Puzzle {
    g: N,
    puzzle: Entity,
    vars: Vec<Var>,
}

impl Puzzle {
    fn new() -> Self {
        let mut g = N::with(puzzle_systems);
        let puzzle = g.g.spawn();
        g.g.add(puzzle, VALUE).add(puzzle, REQUIRES);
        g.g.set_formula(puzzle, ValueCell::NAME, "all(constraint)");
        Puzzle { g, puzzle, vars: Vec::new() }
    }

    fn choice(&mut self, name: &str, options: &[&str]) {
        let e = options_on(&mut self.g, name, options.iter().map(|s| s.to_string()).collect());
        self.vars.push(Space::global().uniform(&e.to_string(), options));
    }

    /// How two choices must relate.
    fn pair(&mut self, a: &str, b: &str, text: &str) {
        self.constraint(&[("a", a), ("b", b)], text);
    }

    /// A clue: this choice has this value.
    fn is(&mut self, choice: &str, value: &str) {
        let value = if value.parse::<f64>().is_ok() { value.to_string() } else { format!("{value:?}") };
        self.constraint(&[("choice", choice)], &format!("choice = {value}"));
    }

    fn constraint(&mut self, about: &[(&str, &str)], text: &str) -> Entity {
        let id = self.g.g.spawn();
        self.g.g.add(id, VALUE).set_formula(id, ValueCell::NAME, text);
        for (name, a) in about {
            let a = self.g.id(a);
            label(&mut self.g, id, a, name);
        }
        label(&mut self.g, self.puzzle, id, "constraint");
        id
    }

    /// Write a new formula for a constraint's value.
    fn rewrite(&mut self, id: Entity, text: &str) {
        self.g.g.set_formula(id, ValueCell::NAME, text);
    }

    fn valid(&self) -> Bool {
        truth(&self.g.g.get::<ValueCell>(self.puzzle).unwrap().unwrap())
    }

    fn count(&self) -> u128 {
        self.valid().count(&self.vars)
    }

    /// Solutions, by name.
    fn solutions(&self, limit: usize) -> Vec<BTreeMap<String, String>> {
        self.valid().solutions(&self.vars, limit).into_iter().map(|s| s.into_iter().map(|(k, v)| (self.g.label(&k), v)).collect()).collect()
    }

    fn e(&self, name: &str) -> Entity {
        self.g.id(name)
    }
}

/// Give a node options, and so make it a choice.
fn options_on(g: &mut N, name: &str, options: Vec<String>) -> Entity {
    g.add(name, OPTIONS).set(name, Options(options));
    g.id(name)
}

/// What a formula on `from` calls `to`, by handle.
fn label(g: &mut N, from: Entity, to: Entity, name: &str) {
    let edge = g.g.link(from, to, LABEL);
    g.g.set(edge, Called(name.to_string()));
}

fn different(p: &mut Puzzle, a: &str, b: &str) {
    p.pair(a, b, "a != b");
}

// ---------- map colouring ----------

#[test]
fn colouring_australia_has_18_solutions() {
    let mut p = Puzzle::new();
    let regions = ["wa", "nt", "sa", "q", "nsw", "v", "t"];
    for r in regions {
        p.choice(r, &["red", "green", "blue"]);
    }
    let borders = [("wa", "nt"), ("wa", "sa"), ("nt", "sa"), ("nt", "q"), ("sa", "q"), ("sa", "nsw"), ("sa", "v"), ("q", "nsw"), ("nsw", "v")];
    for (a, b) in borders {
        different(&mut p, a, b);
    }
    assert_eq!(p.count(), 18);

    // Checked by plain enumeration too.
    let colours = ["red", "green", "blue"];
    let mut brute = 0;
    for code in 0..3usize.pow(7) {
        let of = |r: &str| colours[(code / 3usize.pow(regions.iter().position(|x| *x == r).unwrap() as u32)) % 3];
        if borders.iter().all(|(a, b)| of(a) != of(b)) {
            brute += 1;
        }
    }
    assert_eq!(brute, 18);

    // Tasmania touches nothing, so it's free: exactly what the formula's support says.
    assert!(!p.valid().support().contains(&p.e("t").to_string()));
    println!("australia, one solution: {:?}", p.solutions(1)[0]);
}

#[test]
fn adding_a_clue_reruns_only_that_clue_and_narrows_what_is_possible() {
    let mut p = Puzzle::new();
    for r in ["wa", "nt", "sa"] {
        p.choice(r, &["red", "green", "blue"]);
    }
    different(&mut p, "wa", "nt");
    different(&mut p, "wa", "sa");
    different(&mut p, "nt", "sa");
    assert_eq!(p.count(), 6);
    p.g.take_ran();

    // A clue: WA is red.
    let wa = p.vars[0].clone();
    assert_eq!(wa.possible(&p.valid()).len(), 3);
    p.g.take_ran();
    p.is("wa", "red");
    assert_eq!(p.count(), 2);
    let ran = p.g.ran_values();
    assert_eq!(ran.len(), 2, "the clue and the puzzle, since the choice it reads is already known: {ran:?}");

    // What's now forced, and what isn't.
    let sa = p.vars[2].clone();
    assert_eq!(wa.possible(&p.valid()), vec!["red"]);
    assert_eq!(sa.possible(&p.valid()), vec!["green", "blue"]);
}

// ---------- n queens ----------

fn queens(n: usize) -> Puzzle {
    let mut p = Puzzle::new();
    let cols: Vec<String> = (0..n).map(|c| c.to_string()).collect();
    let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
    for r in 0..n {
        p.choice(&format!("row{r}"), &cols);
    }
    for r1 in 0..n {
        for r2 in r1 + 1..n {
            p.pair(&format!("row{r1}"), &format!("row{r2}"), &format!("a != b && abs(a - b) != {}", r2 - r1));
        }
    }
    p
}

#[test]
fn n_queens_matches_the_known_counts() {
    for (n, want) in [(4, 2), (5, 10), (6, 4), (7, 40), (8, 92)] {
        let start = Instant::now();
        let p = queens(n);
        let got = p.count();
        println!("{n} queens: {got} solutions in {:?}, {} diagram nodes", start.elapsed(), Space::global().size());
        assert_eq!(got, want, "{n} queens");
        p.g.assert_round_trips();
    }
}

/// The same, as a table: each row of `Queen` takes the table's options, so it's a choice of column,
/// and the table says no two rows share a column or a diagonal.
fn queens_table(n: usize) -> Puzzle {
    let mut p = Puzzle::new();
    let cols: Vec<String> = (0..n).map(|c| c.to_string()).collect();
    let head = options_on(&mut p.g, "Queen", cols.clone());
    p.g.g.set_formula(head, ValueCell::NAME, "distinct(rows) && distinct(q in rows: q + q.row) && distinct(q in rows: q - q.row)");
    let puzzle = p.puzzle;
    label(&mut p.g, puzzle, head, "constraint");
    let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
    for r in 0..n {
        let e = p.g.e(&format!("Queen[{r}]"));
        p.vars.push(Space::global().uniform(&e.to_string(), &cols));
    }
    p
}

#[test]
fn n_queens_as_a_table_matches_the_known_counts() {
    for (n, want) in [(4, 2), (5, 10), (6, 4), (8, 92)] {
        let p = queens_table(n);
        assert_eq!(p.count(), want, "{n} queens");
        p.g.assert_round_trips();
    }
}

#[test]
fn a_tables_options_are_its_rows_choices_not_its_own() {
    let mut p = Puzzle::new();
    options_on(&mut p.g, "Pet", vec!["1".into(), "2".into()]);
    let (dog, cat) = (p.g.e("Pet[Dog]"), p.g.e("Pet[Cat]"));
    let value = |p: &Puzzle, e: Entity| p.g.g.get::<ValueCell>(e).unwrap().map(|v| v.0.0.cases().len());
    assert_eq!(value(&p, dog), Some(2), "a row chooses from its table's options");
    assert_eq!(value(&p, p.e("Pet")), None, "the table isn't a choice itself");

    // A row can say its own options, and the table's are taken again when it stops.
    options_on(&mut p.g, "Pet[Cat]", vec!["1".into(), "2".into(), "3".into()]);
    assert_eq!(value(&p, cat), Some(3));
    options_on(&mut p.g, "Pet", vec!["1".into(), "2".into(), "3".into(), "4".into()]);
    assert_eq!(value(&p, dog), Some(4), "changing the table's options changes its rows'");
    assert_eq!(value(&p, cat), Some(3));

    // With no rows left, it's a choice like any other node with options.
    let (d, c) = (p.e("Pet[Dog]"), p.e("Pet[Cat]"));
    p.g.g.set(d, recollect_graph::Name("Dog".to_string()));
    p.g.g.set(c, recollect_graph::Name("Cat".to_string()));
    assert_eq!(value(&p, p.e("Pet")), Some(4));
    assert_eq!(value(&p, d), None, "and what was a row isn't");
    p.g.assert_round_trips();
}

#[test]
fn distinct_is_every_pair_differing() {
    let mut p = Puzzle::new();
    for x in ["x", "y", "z"] {
        p.choice(x, &["1", "2", "3"]);
    }
    p.constraint(&[("v", "x"), ("v", "y"), ("v", "z")], "distinct(v)");
    assert_eq!(p.count(), 6);
    let mut q = Puzzle::new();
    for x in ["x", "y", "z"] {
        q.choice(x, &["1", "2", "3"]);
    }
    let all = q.constraint(&[("x", "x"), ("y", "y"), ("z", "z")], "distinct(x, y, z)");
    assert_eq!(q.count(), 6, "or the values listed out");
    q.rewrite(all, "distinct(x, y)");
    assert_eq!(q.count(), 18);
    q.rewrite(all, "true");
    q.constraint(&[("v", "x"), ("v", "y"), ("v", "z")], "distinct(n in v: n * n - 4 * n)");
    // n² - 4n is -3, -4 and -3, so there are only two different values for three choices.
    assert_eq!(q.count(), 0);
}

#[test]
fn a_clue_can_go_over_any_tables_rows_and_find_a_row_by_another_rows_key() {
    let mut p = Puzzle::new();
    let options = ["1", "2", "3"];
    for t in ["A", "B"] {
        options_on(&mut p.g, t, options.iter().map(|s| s.to_string()).collect());
        for k in ["x", "y"] {
            let e = p.g.e(&format!("{t}[{k}]"));
            p.vars.push(Space::global().uniform(&e.to_string(), &options));
        }
    }
    // For each key, A's row is below B's: three ways for x and three for y.
    let clue = p.constraint(&[], "all(a in A.rows: a < B[a.row])");
    assert_eq!(p.count(), 9);
    p.rewrite(clue, "count(A.rows) = 2 && sum(B.rows) = 6");
    assert_eq!(p.count(), 9, "short forms too: B's two rows add to 6 only as 3 and 3, and A's are free");

    // A new row is part of the group.
    let z = p.g.e("A[z]");
    p.vars.push(Space::global().uniform(&z.to_string(), &options));
    assert_eq!(p.count(), 0, "now A has three rows");

    p.rewrite(clue, "A.rows + 1");
    let err = format!("{:?}", p.g.g.get::<ValueCell>(clue).unwrap_err());
    assert!(err.contains("A.rows is a group of rows"), "{err}");
    p.rewrite(clue, "all(a in A.rows: B[a.row] > 0)");
    let err = format!("{:?}", p.g.g.get::<ValueCell>(clue).unwrap_err());
    assert!(err.contains("no row B[z]"), "{err}");
}

// ---------- sudoku ----------

fn sudoku4() -> Puzzle {
    let mut p = Puzzle::new();
    for r in 0..4 {
        for c in 0..4 {
            p.choice(&format!("r{r}c{c}"), &["1", "2", "3", "4"]);
        }
    }
    let id = |r: usize, c: usize| format!("r{r}c{c}");
    for a in 0..16usize {
        for b in a + 1..16 {
            let (ra, ca, rb, cb) = (a / 4, a % 4, b / 4, b % 4);
            if ra == rb || ca == cb || (ra / 2 == rb / 2 && ca / 2 == cb / 2) {
                different(&mut p, &id(ra, ca), &id(rb, cb));
            }
        }
    }
    p
}

#[test]
fn four_by_four_sudoku_has_288_grids() {
    let start = Instant::now();
    let p = sudoku4();
    assert_eq!(p.count(), 288);
    println!("4×4 sudoku: 288 grids in {:?}", start.elapsed());
    p.g.assert_round_trips();
}

/// A plain backtracking solver, sharing nothing with the diagrams.
fn count_grids_by_search(clues: &[(usize, usize, u8)]) -> usize {
    fn ok(grid: &[[u8; 4]; 4], r: usize, c: usize, d: u8) -> bool {
        (0..4).all(|i| grid[r][i] != d && grid[i][c] != d) && (0..2).all(|i| (0..2).all(|j| grid[r / 2 * 2 + i][c / 2 * 2 + j] != d))
    }
    fn go(grid: &mut [[u8; 4]; 4], at: usize, clues: &[(usize, usize, u8)]) -> usize {
        if at == 16 {
            return 1;
        }
        let (r, c) = (at / 4, at % 4);
        let clue = clues.iter().find(|(x, y, _)| *x == r && *y == c).map(|t| t.2);
        let mut n = 0;
        for d in 1..=4u8 {
            if clue.is_none_or(|k| k == d) && ok(grid, r, c, d) {
                grid[r][c] = d;
                n += go(grid, at + 1, clues);
                grid[r][c] = 0;
            }
        }
        n
    }
    go(&mut [[0; 4]; 4], 0, clues)
}

fn with_clues(clues: &[(usize, usize, u8)]) -> Puzzle {
    let mut p = sudoku4();
    p.count();
    for (r, c, d) in clues {
        p.is(&format!("r{r}c{c}"), &d.to_string());
    }
    p
}

#[test]
fn clues_narrow_a_grid_exactly_as_a_plain_search_says() {
    assert_eq!(count_grids_by_search(&[]), 288);

    // Clues from a real grid: 1234 / 3412 / 2143 / 4321.
    let few = [(0, 0, 1), (1, 2, 1), (2, 1, 1), (3, 3, 1)];
    let enough = [(0, 0, 1), (0, 1, 2), (1, 0, 3), (1, 2, 1), (2, 0, 2), (2, 3, 3), (3, 2, 2), (3, 3, 1)];
    // Contradictory, as a solver should be able to say.
    let impossible = [(0, 0, 1), (0, 3, 4), (1, 1, 3), (2, 2, 2), (3, 0, 3), (3, 3, 1)];
    for clues in [&few[..], &enough[..], &impossible[..]] {
        let p = with_clues(clues);
        let want = count_grids_by_search(clues);
        println!("{} clues: {want} grids", clues.len());
        assert_eq!(p.count(), want as u128);
    }
    assert_eq!(count_grids_by_search(&enough), 1, "this set should pin down one grid");
    assert_eq!(count_grids_by_search(&impossible), 0);

    let p = with_clues(&enough);
    assert_eq!(p.solutions(10)[0]["r2c1"], "1", "and the grid can be read back");
    p.g.assert_round_trips();
}

// ---------- knights and knaves ----------

/// A says "we are both knaves". Knights tell the truth and knaves lie. The classic answer is
/// that A is a knave and B a knight, and here that comes from a statement being a node.
#[test]
fn knights_and_knaves() {
    let mut p = Puzzle::new();
    for who in ["a", "b"] {
        p.choice(who, &["knight", "knave"]);
    }
    p.g.formula("said", r#"a = "knave" && b = "knave""#).input("said", "a", "a").input("said", "b", "b");
    p.constraint(&[("speaker", "a"), ("said", "said")], r#"(speaker = "knight") = said"#);

    let solved = p.solutions(10);
    assert_eq!(solved.len(), 1, "{solved:?}");
    assert_eq!((solved[0]["a"].as_str(), solved[0]["b"].as_str()), ("knave", "knight"));
    println!("knights and knaves: {:?}", solved[0]);
    p.g.assert_round_trips();
}

// ---------- the zebra puzzle ----------

fn zebra() -> Puzzle {
    let houses = ["1", "2", "3", "4", "5"];
    let categories: [&[&str]; 5] = [
        &["english", "spanish", "ukrainian", "norwegian", "japanese"],
        &["red", "green", "ivory", "yellow", "blue"],
        &["dog", "snails", "fox", "horse", "zebra"],
        &["coffee", "tea", "milk", "juice", "water"],
        &["oldgold", "kools", "chesterfield", "luckystrike", "parliament"],
    ];
    let mut p = Puzzle::new();
    for cat in categories {
        for v in cat {
            p.choice(v, &houses);
        }
    }
    // The clues go first, so the search is narrow before the all-different constraints apply.
    p.pair("english", "red", "a = b");
    p.pair("spanish", "dog", "a = b");
    p.pair("coffee", "green", "a = b");
    p.pair("ukrainian", "tea", "a = b");
    p.pair("green", "ivory", "a = b + 1");
    p.pair("oldgold", "snails", "a = b");
    p.pair("kools", "yellow", "a = b");
    p.is("milk", "3");
    p.is("norwegian", "1");
    p.pair("chesterfield", "fox", "abs(a - b) = 1");
    p.pair("kools", "horse", "abs(a - b) = 1");
    p.pair("luckystrike", "juice", "a = b");
    p.pair("japanese", "parliament", "a = b");
    p.pair("norwegian", "blue", "abs(a - b) = 1");

    for cat in categories {
        for i in 0..5 {
            for j in i + 1..5 {
                different(&mut p, cat[i], cat[j]);
            }
        }
    }
    p
}

#[test]
fn the_zebra_puzzle_has_its_known_unique_answer() {
    let start = Instant::now();
    let p = zebra();
    let n = p.count();
    println!("zebra: {n} solution(s) in {:?}, {} diagram nodes", start.elapsed(), Space::global().size());
    assert_eq!(n, 1);
    let s = &p.solutions(1)[0];
    assert_eq!(s["zebra"], s["japanese"], "the Japanese owns the zebra");
    assert_eq!(s["water"], s["norwegian"], "the Norwegian drinks water");

    // Saved, reloaded, and still the same puzzle with the same answer.
    let start = Instant::now();
    let loaded = p.g.assert_round_trips();
    println!("zebra round trip, including every cell: {:?}", start.elapsed());
    assert!(truth(&loaded.get::<ValueCell>(p.puzzle).unwrap().unwrap()) == p.valid());
}

/// The point of saving: a new process with no memory of this one solves the same puzzle from
/// the files alone. Nothing is shared, not even the diagram.
#[test]
fn a_new_process_solves_the_zebra_puzzle_from_its_files() {
    let p = zebra();
    let dir = tempfile::tempdir().unwrap();
    p.g.g.save_dir(dir.path()).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "solve_from_files_in_this_process", "--nocapture"])
        .env("ZEBRA_DIR", dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    println!("{text}");
    assert!(output.status.success(), "{text}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(text.contains("1 passed"), "the other process should have run the check: {text}");
}

/// Run by the test above, in its own process.
#[test]
#[ignore]
fn solve_from_files_in_this_process() {
    let mut reg = Registry::new();
    puzzle_systems(&mut reg);
    let dir = std::env::var("ZEBRA_DIR").expect("run by a_new_process_solves_the_zebra_puzzle_from_its_files");
    let (g, problems) = recollect_graph::Graph::load_dir(std::path::Path::new(&dir), std::sync::Arc::new(reg)).unwrap();
    assert_eq!(problems, vec![]);
    assert_eq!(Space::global().size(), 2, "nothing has been computed in this process yet");

    // Everything needed comes from the graph: which node is the puzzle, what the choices are, and
    // what they're called.
    let puzzle = g.entities().find(|e| g.has(*e, REQUIRES)).expect("a puzzle");
    let mut vars = Vec::new();
    let mut names = BTreeMap::new();
    for e in g.entities() {
        if let Some(Options(options)) = g.get::<Options>(e).unwrap() {
            let options: Vec<&str> = options.iter().map(String::as_str).collect();
            vars.push(Space::global().uniform(&e.to_string(), &options));
            names.insert(e.to_string(), g.get::<recollect_graph::Name>(e).unwrap().unwrap().0);
        }
    }
    let valid = truth(&g.get::<ValueCell>(puzzle).unwrap().unwrap());
    assert_eq!(valid.count(&vars), 1);
    let solution = &valid.solutions(&vars, 1)[0];
    let house = |who: &str| solution.iter().find(|(k, _)| names[*k] == who).unwrap().1;
    assert_eq!(house("zebra"), house("japanese"));
    assert_eq!(house("water"), house("norwegian"));
}
