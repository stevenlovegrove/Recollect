//! Uncertain numbers in formulas mean what `docs/distributions.md` says. The core of it is the same as
//! for worlds: pick a world, and a value for every draw, and a formula is what a plain evaluator gives
//! on ordinary numbers there. That evaluator is written out here, and shares only the parser.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use recollect_graph::chance::{self, Law, Num, Rand, Reader, Share, Term};
use recollect_graph::worlds::{Cond, Space};
use recollect_graph::{CellType, CommitError, Entity, Error, Log, Op, Registry};
use recollect_systems::expr::{BinOp, Expr, Val, parse};
use recollect_systems::formulas::{OPTIONS, Options, REQUIRES, VALUE, ValueCell, install};
use recollect_systems::plan::{self, ALTERNATIVE, DECISION, Estimate, PART_OF, TASK, Weight};
use recollect_systems::schedule::{Finish, PlanEnd};
use support::N;

fn sheet(reg: &mut Registry) {
    install(reg);
}

fn value(g: &N, name: &str) -> Cond<Val> {
    g.get::<ValueCell>(name).unwrap_or_else(|e| panic!("{name}: {e:?}")).unwrap().0.0
}

fn error(g: &N, name: &str) -> String {
    match g.get::<ValueCell>(name) {
        Err(Error::Failed(why)) => why,
        other => panic!("{name} should have failed, and is {other:?}"),
    }
}

/// The one thing a value is, where it's the same in every world of choices.
fn only(g: &N, name: &str) -> Val {
    let v = value(g, name);
    let cases = v.cases();
    assert_eq!(cases.len(), 1, "{name} is {v:?}");
    cases[0].1.clone()
}

fn rand(v: &Val) -> Rand {
    match v {
        Val::Rand(r) => *r,
        other => panic!("{other:?} isn't uncertain"),
    }
}

fn one(v: &Val) -> [Share; 1] {
    [Share { weight: 1.0, value: Term::Rand(rand(v)) }]
}

/// What a draw written on this node's value is called: the first, unless it says.
fn draw_of(g: &N, name: &str, which: u32) -> String {
    format!("~{}/value#{which}", g.id(name))
}

// ---------- the core property ----------

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }

    /// Strictly between 0 and 1.
    fn unit(&mut self) -> f64 {
        (self.next(999_983) as f64 + 0.5) / 999_983.0
    }
}

const INPUTS: [&str; 5] = ["a", "b", "c", "p", "d"];

fn number(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 {
        return match rng.next(8) {
            0 => "undefined".to_string(),
            1..=5 => INPUTS[rng.next(5)].to_string(),
            _ => ["0", "1", "2", "0.5"][rng.next(4)].to_string(),
        };
    }
    let d = depth - 1;
    match rng.next(17) {
        0 => format!("({} + {})", number(rng, d), number(rng, d)),
        1 => format!("({} - {})", number(rng, d), number(rng, d)),
        2 => format!("({} * {})", number(rng, d), number(rng, d)),
        3 => format!("({} / {})", number(rng, d), number(rng, d)),
        4 => format!("(-{})", number(rng, d)),
        5 => format!("if({}, {}, {})", truth_of(rng, d), number(rng, d), number(rng, d)),
        6 => format!("abs({})", number(rng, d)),
        7 => format!("min({}, {})", number(rng, d), number(rng, d)),
        8 => format!("max({}, {})", number(rng, d), number(rng, d)),
        9 => format!("or({}, {})", number(rng, d), number(rng, d)),
        10 => format!("floor({})", number(rng, d)),
        11 => format!("sqrt({})", number(rng, d)),
        12 => format!("round({}, 1)", number(rng, d)),
        13 => format!("ceil({})", number(rng, d)),
        14 => format!("exp({})", number(rng, d)),
        15 => format!("ln({})", number(rng, d)),
        _ => format!("({} ^ 2)", number(rng, d)),
    }
}

fn truth_of(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 {
        return ["true", "false", "(a < 1)", "(b > 2)", "(p = 1)", "(d >= 2)"][rng.next(6)].to_string();
    }
    let d = depth - 1;
    match rng.next(11) {
        0 => format!("({} && {})", truth_of(rng, d), truth_of(rng, d)),
        1 => format!("({} || {})", truth_of(rng, d), truth_of(rng, d)),
        2 => format!("(!{})", truth_of(rng, d)),
        3 => format!("({} < {})", number(rng, d), number(rng, d)),
        4 => format!("({} >= {})", number(rng, d), number(rng, d)),
        5 => format!("({} = {})", number(rng, d), number(rng, d)),
        6 => format!("({} = {})", truth_of(rng, d), truth_of(rng, d)),
        7 => format!("({} != {})", number(rng, d), number(rng, d)),
        8 => format!("({} <= {})", number(rng, d), number(rng, d)),
        9 => format!("({} > {})", number(rng, d), number(rng, d)),
        _ => format!("defined({})", number(rng, d)),
    }
}

/// What a formula means in one world and one draw of everything, with nothing in it that knows about
/// either.
fn plain(e: &Expr, vars: &BTreeMap<&str, f64>) -> Val {
    use Val::{Bool, Num, Undefined};
    let finite = |n: f64| if n.is_finite() { Num(n) } else { Undefined };
    let call = |name: &str, args: &[Expr]| -> Val {
        let arg = |i: usize| plain(&args[i], vars);
        let number = |f: &dyn Fn(f64) -> f64| match arg(0) {
            Num(n) => finite(f(n)),
            _ => Undefined,
        };
        match name {
            "if" => match arg(0) {
                Bool(true) => arg(1),
                Bool(false) => arg(2),
                _ => Undefined,
            },
            "defined" => Bool(arg(0) != Undefined),
            "abs" => number(&|n| if n < 0.0 { -n } else { n }),
            "floor" => number(&f64::floor),
            "ceil" => number(&f64::ceil),
            "sqrt" => number(&f64::sqrt),
            "exp" => number(&f64::exp),
            "ln" => number(&f64::ln),
            "round" => match (arg(0), arg(1)) {
                (Num(n), Num(places)) => finite((n * 10f64.powf(places)).round() / 10f64.powf(places)),
                _ => Undefined,
            },
            "min" | "max" => match (arg(0), arg(1)) {
                (Num(a), Num(b)) => Num(if name == "min" { a.min(b) } else { a.max(b) }),
                _ => Undefined,
            },
            "or" => match arg(0) {
                Undefined => arg(1),
                v => v,
            },
            _ => panic!("{name}"),
        }
    };
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Path(p) => Num(vars[p[0].as_str()]),
        Expr::Call(name, args) => call(name, args),
        Expr::Fold { .. } | Expr::Row { .. } => panic!("not generated"),
        Expr::Not(a) => match plain(a, vars) {
            Bool(b) => Bool(!b),
            _ => Undefined,
        },
        Expr::Neg(a) => match plain(a, vars) {
            Num(n) => Num(0.0 - n),
            _ => Undefined,
        },
        Expr::Bin(op, a, b) => {
            let (x, y) = (plain(a, vars), plain(b, vars));
            match op {
                BinOp::And => match (x, y) {
                    (Bool(false), _) | (_, Bool(false)) => Bool(false),
                    (Bool(true), Bool(true)) => Bool(true),
                    _ => Undefined,
                },
                BinOp::Or => match (x, y) {
                    (Bool(true), _) | (_, Bool(true)) => Bool(true),
                    (Bool(false), Bool(false)) => Bool(false),
                    _ => Undefined,
                },
                _ if x == Undefined || y == Undefined => Undefined,
                BinOp::Eq => Bool(x == y),
                BinOp::Ne => Bool(x != y),
                _ => {
                    let (Num(x), Num(y)) = (x, y) else { panic!("only numbers get here") };
                    match op {
                        BinOp::Add => finite(x + y),
                        BinOp::Sub => finite(x - y),
                        BinOp::Mul => finite(x * y),
                        BinOp::Div => finite(x / y),
                        BinOp::Pow => finite(x.powf(y)),
                        BinOp::Lt => Bool(x < y),
                        BinOp::Le => Bool(x <= y),
                        BinOp::Gt => Bool(x > y),
                        BinOp::Ge => Bool(x >= y),
                        _ => unreachable!(),
                    }
                }
            }
        }
    }
}

/// A bell curve, a spread, a wait, a choice and a die, for formulas to read.
fn with_inputs() -> N {
    let mut g = N::with(sheet);
    g.formula("a", "normal(1, 2)").formula("b", "uniform(0, 3)").formula("c", "exponential(2)").formula("d", "integers(1, 3)");
    g.add("p", OPTIONS).set("p", Options(["0", "1", "3"].iter().map(|s| s.to_string()).collect()));
    g.formula("p", "self.chosen.value");
    g
}

#[test]
fn in_every_world_and_every_draw_a_formula_is_what_a_plain_evaluator_says() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let (mut compared, mut undefined, mut uncertain, mut settled) = (0usize, 0usize, 0usize, 0usize);
    for round in 0..900 {
        let depth = 1 + rng.next(4);
        let text = if round % 2 == 0 { number(&mut rng, depth) } else { truth_of(&mut rng, depth) };
        let mut g = with_inputs();
        g.formula("f", &text);
        for input in INPUTS {
            g.input("f", input, input);
        }
        let got = value(&g, "f");
        let e = parse(&text).unwrap();
        let any_uncertain = got.cases().iter().any(|(_, v)| matches!(v, Val::Rand(_)));
        uncertain += usize::from(any_uncertain);
        settled += usize::from(!any_uncertain);
        let names = [draw_of(&g, "a", 0), draw_of(&g, "b", 0), draw_of(&g, "c", 0)];
        for p in ["0", "1", "3"] {
            for d in ["1", "2", "3"] {
                let world = BTreeMap::from([(g.id("p").to_string(), p.to_string()), (draw_of(&g, "d", 0), d.to_string())]);
                for _ in 0..4 {
                    let u = [rng.unit(), rng.unit(), rng.unit()];
                    let vars = BTreeMap::from([
                        ("a", 1.0 + 2.0 * chance::probit(u[0])),
                        ("b", 0.0 + (3.0 - 0.0) * u[1]),
                        ("c", 2.0 * -(-u[2]).ln_1p()),
                        ("p", p.parse().unwrap()),
                        ("d", d.parse().unwrap()),
                    ]);
                    let want = plain(&e, &vars);
                    let here = match got.at(&world) {
                        Val::Rand(r) => match r.at(&|name| u[names.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name} isn't one of its draws"))]) {
                            None => Val::Undefined,
                            Some(v) if r.is_truth() => Val::Bool(v == 1.0),
                            Some(v) => Val::Num(v),
                        },
                        plain => plain.clone(),
                    };
                    assert_eq!(here, want, "{text} with p={p} d={d} and draws {u:?}");
                    compared += 1;
                    undefined += usize::from(want == Val::Undefined);
                }
            }
        }
    }
    println!("{compared} comparisons, {undefined} undefined, {uncertain} formulas that were uncertain and {settled} that weren't");
    assert!(compared > 10_000 && undefined > 500 && uncertain > 200 && settled > 20, "the generator should actually exercise this");
}

// ---------- which draw is which ----------

#[test]
fn a_cell_is_one_draw_and_each_place_its_written_is_another() {
    let mut g = N::with(sheet);
    g.formula("rate", "normal(0.05, 0.01)");
    g.formula("same", "r - r").input("same", "rate", "r");
    g.formula("two", "normal(0, 1) - normal(0, 1)");
    g.formula("other", "normal(0.05, 0.01)");
    g.formula("apart", "r - o").input("apart", "rate", "r").input("apart", "other", "o");
    assert_eq!(only(&g, "same"), Val::Num(0.0), "one rate less itself is nothing");
    let mut reader = Reader::new();
    assert_eq!(reader.law(&rand(&only(&g, "two"))), Some(Law::Normal { mean: 0.0, sd: 2f64.sqrt() }), "two draws written in one formula are two");
    let apart = reader.law(&rand(&only(&g, "apart"))).unwrap();
    assert!(matches!(apart, Law::Normal { mean, sd } if mean == 0.0 && (sd - 0.01 * 2f64.sqrt()).abs() < 1e-12), "and the same formula in another cell is another draw: {apart:?}");
}

#[test]
fn a_column_is_a_draw_for_each_row_and_a_cell_they_all_read_is_one() {
    let mut g = N::with(sheet);
    g.formula("Shared", "normal(0, 1)");
    g.e("Year");
    g.formula_on("Year", "own", "normal(0, 1)");
    g.formula_on("Year", "both", "self.own + shared").input("Year", "Shared", "shared");
    for row in ["Year[1]", "Year[2]"] {
        g.e(row);
    }
    let cell = |g: &N, row: &str, cell: &str| -> Val {
        let w: recollect_graph::worlds::Worldly<Val> = serde_json::from_value(g.get_value(g.id(row), cell).unwrap().unwrap()).unwrap();
        w.0.cases()[0].1.clone()
    };
    assert_ne!(cell(&g, "Year[1]", "own"), cell(&g, "Year[2]", "own"), "each row has its own");
    g.formula("gap", "one.both - two.both").input("gap", "Year[1]", "one").input("gap", "Year[2]", "two");
    // What they share has gone, and what's left is the two rows' own draws.
    assert_eq!(Reader::new().law(&rand(&only(&g, "gap"))), Some(Law::Normal { mean: 0.0, sd: 2f64.sqrt() }));
}

#[test]
fn a_macro_draws_once_for_each_time_its_called_and_once_for_an_argument_it_uses_twice() {
    let mut g = N::with(sheet);
    g.add("noise", "macro").set("noise", recollect_systems::formulas::Macro { params: vec![], body: "normal(0, 1)".to_string() });
    g.add("twice", "macro").set("twice", recollect_systems::formulas::Macro { params: vec!["x".to_string()], body: "x + x".to_string() });
    g.formula("called", "noise() + noise()").input("called", "noise", "noise");
    g.formula("passed", "twice(normal(0, 1))").input("passed", "twice", "twice");
    let mut reader = Reader::new();
    assert_eq!(reader.law(&rand(&only(&g, "called"))), Some(Law::Normal { mean: 0.0, sd: 2f64.sqrt() }));
    assert_eq!(reader.law(&rand(&only(&g, "passed"))), Some(Law::Normal { mean: 0.0, sd: 2.0 }));
}

// ---------- short lists are exact ----------

#[test]
fn two_dice_are_eleven_totals_with_their_exact_chances() {
    let mut g = N::with(sheet);
    g.formula("total", "integers(1, 6) + integers(1, 6)");
    g.formula("seven", "t = 7").input("seven", "total", "t");
    g.formula("double", "integers(1, 6) * 2");
    let total = value(&g, "total");
    assert_eq!(total.len(), 11);
    for (when, v) in total.cases() {
        let Val::Num(n) = v else { panic!("{v:?}") };
        let ways = 6.0 - (n - 7.0).abs();
        assert!((when.prob() - ways / 36.0).abs() < 1e-12, "{n} comes up {} and should {}", when.prob(), ways / 36.0);
    }
    let seven = recollect_systems::formulas::truth(&g.get::<ValueCell>("seven").unwrap().unwrap());
    assert!((seven.prob() - 1.0 / 6.0).abs() < 1e-12);
    assert_eq!(value(&g, "double").len(), 6, "one die doubled is six things, not eleven");
}

#[test]
fn a_plain_chance_is_exact_and_an_uncertain_one_is_a_draw() {
    let mut g = N::with(sheet);
    g.formula("late", "bernoulli(0.2)");
    g.formula("cost", "if(l, 10, 4)").input("cost", "late", "l");
    g.formula("sure", "bernoulli(1)");
    g.formula("silly", "bernoulli(1.5)");
    g.formula("unsure", "bernoulli(uniform(0.2, 0.4))");
    let late = recollect_systems::formulas::truth(&g.get::<ValueCell>("late").unwrap().unwrap());
    assert!((late.prob() - 0.2).abs() < 1e-12);
    let cost: Cond<f64> = value(&g, "cost").map(|v| if let Val::Num(n) = v { *n } else { panic!("{v:?}") });
    assert!((cost.expected() - 5.2).abs() < 1e-12);
    assert_eq!((only(&g, "sure"), only(&g, "silly")), (Val::Bool(true), Val::Undefined));
    let unsure = rand(&only(&g, "unsure"));
    assert!(unsure.is_truth());
    let c = Reader::new().chance(&[Share { weight: 1.0, value: Term::Rand(unsure) }]);
    assert!(!c.exact && (c.chance - 0.3).abs() < 3.0 * c.error, "{c:?}");
}

#[test]
fn a_long_list_is_drawn_and_only_whole_ends_mean_anything() {
    let mut g = N::with(sheet);
    g.formula("many", "integers(1, 100000)");
    g.formula("half", "integers(1, 2.5)");
    g.formula("backward", "integers(3, 1)");
    let many = rand(&only(&g, "many"));
    assert_eq!(many.bounds(), (1.0, 100_001.0));
    let column = Reader::new().column(&many);
    assert!(column.iter().all(|v| v.fract() == 0.0 && (1.0..=100_000.0).contains(v)));
    assert_eq!((only(&g, "half"), only(&g, "backward")), (Val::Undefined, Val::Undefined));
}

// ---------- the families ----------

#[test]
fn each_family_is_the_law_it_says() {
    let mut g = N::with(sheet);
    g.formula("n", "normal(3, 0.5) * 2 + 1");
    g.formula("u", "10 - uniform(2, 4)");
    g.formula("e", "exponential(4)");
    g.formula("l", "lognormal(0, 0.25)");
    g.formula("t", "triangular(2, 3, 6)");
    g.formula("b", "between(2, 4)");
    let mut reader = Reader::new();
    let mut law = |name: &str| reader.law(&rand(&only(&g, name))).unwrap_or_else(|| panic!("{name} has a law"));
    assert_eq!(law("n"), Law::Normal { mean: 7.0, sd: 1.0 });
    assert_eq!(law("u"), Law::Uniform { from: 6.0, to: 8.0 });
    assert_eq!(law("e"), Law::Exponential { from: 0.0, mean: 4.0 });
    assert_eq!(law("l"), Law::Lognormal { mu: 0.0, sigma: 0.25 });
    assert_eq!(law("t"), Law::Triangular { low: 2.0, likely: 3.0, high: 6.0 });
    let between = law("b");
    assert!((between.quantile(0.05) - 2.0).abs() < 1e-6 && (between.quantile(0.95) - 4.0).abs() < 1e-6, "nine times in ten it's between the two: {between:?}");
}

#[test]
fn what_makes_no_sense_is_undefined_and_the_wrong_type_is_an_error() {
    let mut g = N::with(sheet);
    g.formula("backward", "uniform(4, 2)").formula("narrow", "normal(1, -1)").formula("never", "exponential(0)").formula("jumbled", "triangular(3, 2, 6)");
    g.formula("text", r#"normal("a", 1)"#).formula("few", "normal(1)");
    assert_eq!((only(&g, "backward"), only(&g, "narrow"), only(&g, "never")), (Val::Undefined, Val::Undefined, Val::Undefined));
    let jumbled = Reader::new().summary(&one(&only(&g, "jumbled")));
    assert_eq!(jumbled.undefined, 1.0, "its likeliest isn't between its least and its most");
    assert!(error(&g, "text").contains("needs numbers"), "{}", error(&g, "text"));
    assert!(error(&g, "few").contains("normal takes 2"), "{}", error(&g, "few"));
}

#[test]
fn a_distributions_arguments_can_differ_between_worlds_and_be_uncertain() {
    let mut g = N::with(sheet);
    g.add("how", OPTIONS).set("how", Options(["3", "5"].iter().map(|s| s.to_string()).collect()));
    g.formula("days", "normal(how.chosen.value, 1)").input("days", "how", "how");
    g.formula("nested", "normal(normal(10, 3), 4)");
    let days = value(&g, "days");
    assert_eq!(days.len(), 2, "one for each answer, and each held as what it is");
    let mut reader = Reader::new();
    let means: Vec<f64> = days.cases().iter().map(|(_, v)| reader.law(&rand(v)).unwrap().mean()).collect();
    assert!(means.contains(&3.0) && means.contains(&5.0));
    // An uncertain middle and an uncertain spread about it add, as two unrelated bell curves do.
    assert_eq!(reader.law(&rand(&only(&g, "nested"))), Some(Law::Normal { mean: 10.0, sd: 5.0 }));
}

// ---------- truth ----------

#[test]
fn a_comparison_is_a_matter_of_chance_unless_it_cant_be() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(0, 1)").formula("spread", "uniform(2, 4)").formula("y", "normal(0, 1)");
    let questions = [("under", "s < 5"), ("over", "s > 5"), ("neither", "x > 5 && x < 3"), ("either", "x > 1 || x <= 1"), ("maybe", "x > 1"), ("within", "x > -1 && x < 1"), ("later", "max(x, y) > 1"), ("product", "x * y > 1"), ("abs", "abs(x) >= 0")];
    for (name, text) in questions {
        g.formula(name, text).input(name, "x", "x").input(name, "spread", "s").input(name, "y", "y");
    }
    // What can't be otherwise is settled, with nothing to read.
    assert_eq!((only(&g, "under"), only(&g, "over"), only(&g, "neither"), only(&g, "either")), (Val::Bool(true), Val::Bool(false), Val::Bool(false), Val::Bool(true)));
    let mut reader = Reader::new();
    let mut chance = |name: &str| {
        let truth = rand(&only(&g, name));
        assert!(truth.is_truth());
        reader.chance(&[Share { weight: 1.0, value: Term::Rand(truth) }])
    };
    // A question a law answers is answered exactly: a bell curve is over one spread above its middle
    // 15.87% of the time, and within one of it 68.27%.
    let maybe = chance("maybe");
    assert!(maybe.exact && (maybe.chance - 0.158_655_253_931_457).abs() < 1e-12, "{maybe:?}");
    let within = chance("within");
    assert!(within.exact && (within.chance - 0.682_689_492_137_086).abs() < 1e-12, "{within:?}");
    // The later of two that share nothing is over a point unless both are under it.
    let later = chance("later");
    assert!(later.exact && (later.chance - (1.0 - 0.841_344_746_068_543f64.powi(2))).abs() < 1e-12, "{later:?}");
    // One no law answers is read from draws, and says so.
    let product = chance("product");
    assert!(!product.exact && product.draws == chance::DRAWS && product.chance > 0.1 && product.chance < 0.2, "{product:?}");
    // What simplifying doesn't settle is still right in every draw: it just isn't shown to be.
    let abs = chance("abs");
    assert!(abs.chance == 1.0 && abs.hits == chance::DRAWS && !abs.exact, "{abs:?}");
}

#[test]
fn undefined_is_per_draw_and_if_only_counts_the_side_taken() {
    let mut g = N::with(sheet);
    g.formula("x", "uniform(-1, 1)");
    g.formula("root", "sqrt(x)").input("root", "x", "x");
    g.formula("safe", "if(x >= 0, sqrt(x), 0)").input("safe", "x", "x");
    g.formula("known", "defined(sqrt(x))").input("known", "x", "x");
    g.formula("fallback", "or(sqrt(x), -1)").input("fallback", "x", "x");
    let mut reader = Reader::new();
    let root = reader.summary(&one(&only(&g, "root")));
    assert!((root.undefined - 0.5).abs() < 0.01, "{root:?}");
    let safe = reader.summary(&one(&only(&g, "safe")));
    assert_eq!(safe.undefined, 0.0);
    let known = reader.chance(&[Share { weight: 1.0, value: Term::Rand(rand(&only(&g, "known"))) }]);
    assert!(known.undefined == 0.0 && (known.chance - 0.5).abs() < 0.01, "{known:?}");
    let fallback = reader.summary(&one(&only(&g, "fallback")));
    assert!(fallback.undefined == 0.0 && (fallback.mean - (-0.5 + 1.0 / 3.0)).abs() < 0.01, "{fallback:?}");
}

#[test]
fn an_uncertain_condition_cant_yet_choose_between_things_that_arent_numbers() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(0, 1)");
    g.formula("word", r#"if(x > 0, "up", "down")"#).input("word", "x", "x");
    assert!(error(&g, "word").contains("an uncertain condition can only choose between numbers"), "{}", error(&g, "word"));
}

// ---------- memos, files and narrowing ----------

#[test]
fn a_value_that_comes_out_the_same_expression_stops_there() {
    let mut g = N::with(sheet);
    g.value("mean", ValueCell::num(3.0)).value("unused", ValueCell::num(1.0));
    g.formula("x", "normal(m, 1) + 0 * u").input("x", "mean", "m").input("x", "unused", "u");
    g.formula("y", "x * 2").input("y", "x", "x");
    let before = only(&g, "y");
    g.ran();
    g.value("unused", ValueCell::num(2.0));
    assert_eq!(only(&g, "y"), before);
    assert_eq!(g.ran_values(), ["x"], "x ran, came out the same expression, and y didn't");
    g.value("mean", ValueCell::num(4.0));
    assert_ne!(only(&g, "y"), before);
    let law = Reader::new().law(&rand(&only(&g, "y")));
    assert_eq!(law, Some(Law::Normal { mean: 8.0, sd: 2.0 }), "the same draw, about a new middle");
}

#[test]
fn only_the_formula_is_saved_and_it_reads_the_same_when_loaded() {
    let mut g = N::with(sheet);
    g.formula("x", "normal(3, 0.5)");
    g.formula("y", "max(x, 3) * uniform(1, 2)").input("y", "x", "x");
    g.formula_on("y", "own", "x + 1");
    let before = only(&g, "y");
    let loaded = g.assert_round_trips();
    let saved = serde_json::to_string(&g.save()).unwrap();
    assert!(saved.contains("normal(3, 0.5)") && !saved.contains("rand"), "{saved}");
    // The draws are named for the nodes, which are the same nodes, so it's the same expression.
    assert_eq!(loaded.get::<ValueCell>(g.id("y")).unwrap().unwrap().0.0.cases()[0].1, &before);
}

#[test]
fn narrowing_agrees_with_really_deciding() {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    let mut log = Log::new(Arc::new(reg));
    let ids: BTreeMap<&str, Entity> = ["how", "fast", "slow", "days", "twice"].into_iter().map(|n| (n, Entity::new())).collect();
    let mut ops: Vec<Op> = ids.values().map(|e| Op::Spawn { id: *e }).collect();
    ops.push(Op::add(ids["how"], DECISION));
    for (option, days) in [("fast", "normal(3, 0.5)"), ("slow", "uniform(4, 8)")] {
        ops.extend([Op::link(ids[option], ids["how"], ALTERNATIVE), Op::add(ids[option], VALUE), Op::set_formula(ids[option], ValueCell::NAME, days)]);
    }
    ops.extend([Op::add(ids["days"], VALUE), Op::set_formula(ids["days"], ValueCell::NAME, "how.chosen.value + 1"), Op::link(ids["days"], ids["how"], "label")]);
    ops.push(Op::Set { on: recollect_graph::On::Edge { from: ids["days"], to: ids["how"] }, cell: "called".to_string(), to: Some(recollect_graph::Stored::Value(serde_json::json!("how"))) });
    log.commit("t", "set up", ops).unwrap();
    let open = log.graph().get::<ValueCell>(ids["days"]).unwrap().unwrap().0.0;
    assert_eq!(open.len(), 2, "a bell curve one way and a spread the other: {open:?}");
    for option in ["fast", "slow"] {
        let picks = Space::global().picks(&BTreeMap::from([(ids["how"].to_string(), ids[option].to_string())]));
        let narrowed = open.given(&picks);
        let seq = log.commit("t", "decided", vec![plan::choose(ids["how"], ids[option])]).unwrap();
        let decided = log.graph().get::<ValueCell>(ids["days"]).unwrap().unwrap().0.0;
        assert_eq!(narrowed, decided, "{option}");
        log.undo(seq, "t", "and back").unwrap();
    }
}

// ---------- plans ----------

fn planned() -> N {
    N::systems(&["plan", "schedule"])
}

#[test]
fn uncertain_estimates_add_up_exactly_and_the_later_of_two_is_drawn() {
    let mut g = planned();
    g.e("whole");
    for (task, days) in [("design", "normal(3, 1)"), ("build", "normal(5, 2)"), ("test", "normal(2, 2)")] {
        g.add(task, TASK).formula_on(task, Estimate::NAME, days).link(task, "whole", PART_OF);
    }
    g.formula("effort", "sum(t in task: t.estimate)").input("effort", "design", "task").input("effort", "build", "task").input("effort", "test", "task");
    let mut reader = Reader::new();
    assert_eq!(reader.law(&rand(&only(&g, "effort"))), Some(Law::Normal { mean: 10.0, sd: 3.0 }));

    // Side by side, the plan ends when the last of them does, and the later of three uncertain ends is
    // later, on average, than any one of them: adding averages would say 5. They share nothing, so that
    // has a law, though no name of its own, and it agrees with drawing it.
    let end = g.g.get::<PlanEnd>(Entity::whole()).unwrap().unwrap().0.0;
    let Some(Num::Rand(end)) = end.cases()[0].1.clone() else { panic!("{end:?}") };
    assert_eq!(reader.law(&end).and_then(|l| l.text()).as_deref(), Some("max(normal(2, 2), normal(3, 1), normal(5, 2), 0)"));
    let s = reader.summary(&[Share { weight: 1.0, value: Term::Rand(end) }]);
    assert!(s.exact && s.mean > 5.2 && s.mean < 5.5, "{s:?}");
    let drawn = reader.column(&end);
    let mean = drawn.iter().sum::<f64>() / drawn.len() as f64;
    assert!((mean - s.mean).abs() < 0.08, "drawn, it's {mean}, and the law says {}", s.mean);

    // One after another, the end is their sum, and is held as exactly that. It takes estimates that can't
    // go below nothing to see it: a bell curve can, and a task can't finish before it starts.
    for (task, days) in [("design", "triangular(2, 3, 5)"), ("build", "triangular(3, 5, 9)"), ("test", "uniform(1, 3)")] {
        g.formula_on(task, Estimate::NAME, days);
    }
    g.link("build", "design", plan::DEPENDS_ON).link("test", "build", plan::DEPENDS_ON);
    g.formula("sum", "d.estimate + b.estimate + t.estimate").input("sum", "design", "d").input("sum", "build", "b").input("sum", "test", "t");
    let finish = g.get::<Finish>("test").unwrap().unwrap().0.0;
    let Some(Num::Rand(finish)) = finish.cases()[0].1.clone() else { panic!("{finish:?}") };
    assert_eq!(Val::Rand(finish), only(&g, "sum"));
}

#[test]
fn a_big_task_is_one_thats_likely_to_be_and_says_how_likely() {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    let mut log = Log::new(Arc::new(reg));
    let (small, big, sure) = (Entity::new(), Entity::new(), Entity::new());
    let mut ops = Vec::new();
    for (task, days) in [(small, "normal(3, 1)"), (big, "normal(6, 2)"), (sure, "uniform(6, 7)")] {
        ops.extend([Op::Spawn { id: task }, Op::add(task, TASK), Op::set_formula(task, Estimate::NAME, days)]);
    }
    log.commit("t", "three tasks", ops).unwrap();
    let notices: BTreeMap<Entity, String> = log.notices().into_iter().filter(|v| v.rule == "oversized_task").map(|v| (v.on.nodes()[0], v.detail)).collect();
    assert_eq!(notices.keys().collect::<Vec<_>>(), {
        let mut both = vec![&big, &sure];
        both.sort();
        both
    });
    assert!(notices[&big].contains("big 69% of the time"), "a bell curve has a law, so how likely is exact: {}", notices[&big]);
    assert_eq!(notices[&sure], "a big task with no breakdown", "between 6 and 7 is over 5 for certain");
}

#[test]
fn what_makes_the_worlds_cant_be_uncertain_and_requiring_something_uncertain_is_refused() {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    let mut log = Log::new(Arc::new(reg));
    let (decision, option, cost, cap) = (Entity::new(), Entity::new(), Entity::new(), Entity::new());
    let mut ops: Vec<Op> = [decision, option, cost, cap].into_iter().map(|id| Op::Spawn { id }).collect();
    ops.extend([Op::add(decision, DECISION), Op::link(option, decision, ALTERNATIVE)]);
    ops.extend([Op::add(cost, VALUE), Op::set_formula(cost, ValueCell::NAME, "normal(3000, 500)")]);
    ops.extend([Op::add(cap, VALUE), Op::set_formula(cap, ValueCell::NAME, "cost < 4000"), Op::link(cap, cost, "label")]);
    ops.push(Op::Set { on: recollect_graph::On::Edge { from: cap, to: cost }, cell: "called".to_string(), to: Some(recollect_graph::Stored::Value(serde_json::json!("cost"))) });
    log.commit("t", "a cost and a cap on it", ops).unwrap();

    let weight = Op::Set { on: recollect_graph::On::Edge { from: option, to: decision }, cell: Weight::NAME.to_string(), to: Some(recollect_graph::Stored::Formula("uniform(1, 2)".to_string())) };
    log.commit("t", "an uncertain weight", vec![weight]).unwrap();
    let Err(Error::Failed(why)) = log.graph().get::<Weight>(Entity::edge(option, decision)) else { panic!("an uncertain weight should be an error") };
    assert!(why.contains("a weight can't be uncertain"), "{why}");

    let Err(CommitError::Refused(broken)) = log.commit("t", "require it", vec![Op::add(cap, REQUIRES)]) else { panic!("requiring something uncertain should be refused") };
    assert!(broken.iter().any(|v| v.rule == "requires_something_uncertain"), "{broken:?}");
}
