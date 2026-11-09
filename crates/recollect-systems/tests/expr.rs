//! Formulas mean what `docs/expressions.md` says they mean. The core of it: in every world, a formula
//! has the value a plain evaluator gives on that world's values. That evaluator is written out here
//! with no world values in it, and shares only the parser.

mod support;

use std::collections::BTreeMap;

use recollect_graph::worlds::Space;
use recollect_graph::{CellType, Error, Registry};
use recollect_systems::expr::{BinOp, Expr, Val, parse};
use recollect_systems::formulas::{Called, Chosen, LABEL, MACRO, Macro, OPTIONS, Options, REQUIRES, VALUE, ValueCell, install, option_node, truth, try_formula};
use recollect_systems::plan::{self, ALTERNATIVE, DECISION, Estimate, PART_OF, TASK, UNKNOWN};
use recollect_systems::schedule::{self, Finish};
use serde_json::json;
use support::N;

fn sheet(reg: &mut Registry) {
    install(reg);
}

fn value(g: &N, name: &str) -> ValueCell {
    g.get::<ValueCell>(name).unwrap_or_else(|e| panic!("{name}: {e:?}")).unwrap()
}

fn error(g: &N, name: &str) -> String {
    match g.get::<ValueCell>(name) {
        Err(Error::Failed(why)) => why,
        other => panic!("{name} should have failed, and is {other:?}"),
    }
}

/// A formula with no inputs, in a graph of its own.
fn just(text: &str) -> Val {
    let mut g = N::with(sheet);
    g.formula("x", text);
    let v = value(&g, "x");
    let cases = v.0.0.cases();
    assert_eq!(cases.len(), 1, "{text}");
    cases[0].1.clone()
}

/// Give a node options, and so make it a choice.
fn options(g: &mut N, name: &str, options: &[&str]) {
    g.add(name, OPTIONS).set(name, Options(options.iter().map(|s| s.to_string()).collect()));
}

/// A choice that also reads as its own value: whatever the option it lands on says, so a bare `name`
/// in a formula is that option's value.
fn valued_options(g: &mut N, name: &str, opts: &[&str]) {
    options(g, name, opts);
    g.formula(name, "self.chosen.value");
}

fn just_error(text: &str) -> String {
    let mut g = N::with(sheet);
    g.formula("x", text);
    error(&g, "x")
}

#[test]
fn arithmetic_and_logic_with_no_worlds_are_ordinary() {
    assert_eq!(just("1 + 2 * 3"), Val::Num(7.0));
    assert_eq!(just("(1 + 2) * 3"), Val::Num(9.0));
    assert_eq!(just("-2 - -3"), Val::Num(1.0));
    assert_eq!(just("7 / 2"), Val::Num(3.5));
    assert_eq!(just("1 < 2 && 2 <= 2"), Val::Bool(true));
    assert_eq!(just(r#""a" = "a""#), Val::Bool(true));
    assert_eq!(just(r#""a" != "a""#), Val::Bool(false));
    assert_eq!(just("true = false"), Val::Bool(false));
    assert_eq!(just("round(2.345, 2)"), Val::Num(2.35));
    assert_eq!((just("floor(2.5)"), just("ceil(2.5)"), just("round(2.5)")), (Val::Num(2.0), Val::Num(3.0), Val::Num(3.0)));
    assert_eq!((just("abs(-3)"), just("min(1, 2)"), just("max(1, 2)"), just("or(undefined, 4)"), just("or(2, 4)")), (Val::Num(3.0), Val::Num(1.0), Val::Num(2.0), Val::Num(4.0), Val::Num(2.0)));
}

#[test]
fn has_says_whether_a_node_has_a_cell_and_folds_begin_where_they_are_told() {
    let mut g = N::with(sheet);
    g.value("a", ValueCell::num(3.0)).value("b", ValueCell::num(7.0)).value("gap", ValueCell::plain(Val::Undefined));
    g.formula("has", "has(x.value)").input("has", "a", "x");
    g.formula("hasnt", "has(x.nothing_like_this)").input("hasnt", "a", "x");
    options(&mut g, "own", &["only"]);
    g.formula("own", "has(self.options)");
    g.formula("top", "max(x in line: x, 0)").input("top", "a", "line").input("top", "b", "line");
    g.formula("low", "min(x in line: x, 5)").input("low", "a", "line").input("low", "b", "line");
    g.formula("nothing", "max(x in line: x, 9)");
    g.formula("unknown", "max(x in line: x, 0)").input("unknown", "a", "line").input("unknown", "gap", "line");
    let plain = |n: &str| value(&g, n).0.0.cases()[0].1.clone();
    assert_eq!((plain("has"), plain("hasnt"), plain("own")), (Val::Bool(true), Val::Bool(false), Val::Bool(true)));
    assert_eq!((plain("top"), plain("low"), plain("nothing")), (Val::Num(7.0), Val::Num(3.0), Val::Num(9.0)));
    assert_eq!(plain("unknown"), Val::Undefined, "an unknown could be the biggest");
    assert!(just_error("has(1)").contains("has needs a node and a cell"));
    assert!(just_error("whole").contains("whole needs to say which cell"));
}

#[test]
fn undefined_spreads_and_logic_is_three_valued() {
    for undefined in ["1 / 0", "undefined + 1", "0 / 0", "1 < undefined", "undefined = 1", "!undefined", "if(undefined, 1, 2)", "true && undefined", "false || undefined", "(1 / 0) = (1 / 0)"] {
        assert_eq!(just(undefined), Val::Undefined, "{undefined}");
    }
    assert_eq!(just("false && undefined"), Val::Bool(false));
    assert_eq!(just("true || undefined"), Val::Bool(true));
    assert_eq!(just("defined(undefined)"), Val::Bool(false));
    assert_eq!(just("defined(1 / 0)"), Val::Bool(false));
    assert_eq!(just("defined(1)"), Val::Bool(true));
    assert_eq!(just("defined(1 < undefined)"), Val::Bool(false));
    assert_eq!(just("if(false, 1 / 0, 2)"), Val::Num(2.0), "only the side taken counts");
}

#[test]
fn an_undefined_world_is_only_undefined_in_that_world() {
    let mut g = N::with(sheet);
    options(&mut g, "x", &["0", "2"]);
    g.formula("half", "1 / x.chosen.value").input("half", "x", "x");
    g.formula("safe", "if(x.chosen.value != 0, 1 / x.chosen.value, 0)").input("safe", "x", "x");
    let at = |o: &str| BTreeMap::from([(g.id("x").to_string(), o.to_string())]);
    let half = value(&g, "half").0.0;
    assert_eq!((half.at(&at("0")), half.at(&at("2"))), (&Val::Undefined, &Val::Num(0.5)));
    let safe = value(&g, "safe").0.0;
    assert_eq!((safe.at(&at("0")), safe.at(&at("2"))), (&Val::Num(0.0), &Val::Num(0.5)));
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
}

const NUMBER_CHOICES: [&str; 2] = ["p", "q"];
const NUMBER_OPTIONS: [&str; 3] = ["0", "1", "3"];
const TEXT_OPTIONS: [&str; 2] = ["red", "blue"];

fn number(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 {
        return match rng.next(5) {
            0 => "undefined".to_string(),
            1 | 2 => NUMBER_CHOICES[rng.next(2)].to_string(),
            _ => ["0", "1", "2", "5"][rng.next(4)].to_string(),
        };
    }
    let d = depth - 1;
    match rng.next(10) {
        8 => format!("({} ^ {})", number(rng, d), number(rng, d)),
        0 => format!("({} + {})", number(rng, d), number(rng, d)),
        1 => format!("({} - {})", number(rng, d), number(rng, d)),
        2 => format!("({} * {})", number(rng, d), number(rng, d)),
        3 => format!("({} / {})", number(rng, d), number(rng, d)),
        4 => format!("(-{})", number(rng, d)),
        5 => format!("if({}, {}, {})", truth_of(rng, d), number(rng, d), number(rng, d)),
        6 => format!("abs({})", number(rng, d)),
        7 => format!("min({}, {})", number(rng, d), number(rng, d)),
        _ => format!("or({}, {})", number(rng, d), number(rng, d)),
    }
}

fn truth_of(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 {
        return match rng.next(4) {
            0 => r#"(colour = "red")"#.to_string(),
            1 => "true".to_string(),
            2 => "false".to_string(),
            _ => r#"(colour != "blue")"#.to_string(),
        };
    }
    let d = depth - 1;
    match rng.next(7) {
        0 => format!("({} && {})", truth_of(rng, d), truth_of(rng, d)),
        1 => format!("({} || {})", truth_of(rng, d), truth_of(rng, d)),
        2 => format!("(!{})", truth_of(rng, d)),
        3 => format!("({} < {})", number(rng, d), number(rng, d)),
        4 => format!("({} = {})", number(rng, d), number(rng, d)),
        5 => format!("({} = {})", truth_of(rng, d), truth_of(rng, d)),
        _ => format!("defined({})", number(rng, d)),
    }
}

/// What a formula means in one world, with nothing in it that knows about worlds.
fn plain(e: &Expr, vars: &BTreeMap<&str, Val>) -> Val {
    use Val::{Bool, Num, Undefined};
    let call = |name: &str, args: &[Expr]| -> Val {
        let arg = |i: usize| plain(&args[i], vars);
        match name {
            "if" => match arg(0) {
                Bool(true) => arg(1),
                Bool(false) => arg(2),
                _ => Undefined,
            },
            "defined" => Bool(arg(0) != Undefined),
            "abs" => match arg(0) {
                Num(n) => Num(n.abs()),
                _ => Undefined,
            },
            "min" => match (arg(0), arg(1)) {
                (Num(a), Num(b)) => Num(if a < b { a } else { b }),
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
        Expr::Path(p) => vars[p[0].as_str()].clone(),
        Expr::Call(name, args) => call(name, args),
        Expr::Fold { .. } | Expr::Row { .. } => panic!("not generated"),
        Expr::Not(a) => match plain(a, vars) {
            Bool(b) => Bool(!b),
            _ => Undefined,
        },
        Expr::Neg(a) => match plain(a, vars) {
            Num(n) => Num(-n),
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
                    let finite = |n: f64| if n.is_finite() { Num(n) } else { Undefined };
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

/// Every way p, q and colour could land.
fn worlds() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for p in NUMBER_OPTIONS {
        for q in NUMBER_OPTIONS {
            for c in TEXT_OPTIONS {
                out.push((p.to_string(), q.to_string(), c.to_string()));
            }
        }
    }
    out
}

fn with_choices() -> N {
    let mut g = N::with(sheet);
    for name in NUMBER_CHOICES {
        valued_options(&mut g, name, &NUMBER_OPTIONS);
    }
    valued_options(&mut g, "colour", &TEXT_OPTIONS);
    g
}

#[test]
fn in_every_world_a_formula_is_what_a_plain_evaluator_says() {
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let (mut compared, mut undefined, mut varied) = (0usize, 0usize, 0usize);
    for round in 0..600 {
        let depth = 1 + rng.next(4);
        let text = if round % 2 == 0 { number(&mut rng, depth) } else { truth_of(&mut rng, depth) };
        let mut g = with_choices();
        g.formula("f", &text).input("f", "p", "p").input("f", "q", "q").input("f", "colour", "colour");
        let got = value(&g, "f").0.0;
        varied += usize::from(got.len() > 1);
        let e = parse(&text).unwrap();
        for (p, q, c) in worlds() {
            let world = BTreeMap::from([(g.id("p").to_string(), p.clone()), (g.id("q").to_string(), q.clone()), (g.id("colour").to_string(), c.clone())]);
            let vars = BTreeMap::from([("p", Val::Num(p.parse().unwrap())), ("q", Val::Num(q.parse().unwrap())), ("colour", Val::Text(c.clone()))]);
            let want = plain(&e, &vars);
            assert_eq!(got.at(&world), &want, "{text} with p={p} q={q} colour={c}");
            compared += 1;
            undefined += usize::from(want == Val::Undefined);
        }
    }
    println!("{compared} comparisons, {undefined} undefined, {varied} formulas that varied");
    assert!(compared > 10_000 && undefined > 1000 && varied > 150, "the generator should actually exercise this");
}

/// A decision taken through a real commit, with narrowing asked the same question beforehand.
#[test]
fn narrowing_a_formula_agrees_with_really_deciding() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut compared = 0;
    for _ in 0..200 {
        let mut text = number(&mut rng, 3);
        // Over decisions rather than domains: a decision is what really gets decided.
        text = text.replace('p', "pp").replace("colour", "cc").replace('q', "qq");
        let (mut log, ids) = decided_plan(&text);
        let (d, options) = (ids["pp"], [ids["a"], ids["b"]]);
        for o in options {
            let open = log.graph().get::<ValueCell>(ids["f"]).unwrap().unwrap().0.0;
            let picks = Space::global().picks(&BTreeMap::from([(d.to_string(), o.to_string())]));
            let narrowed = open.given(&picks);

            let seq = log.commit("t", "decided", vec![plan::choose(d, o)]).unwrap();
            let decided = log.graph().get::<ValueCell>(ids["f"]).unwrap().unwrap().0.0;
            assert_eq!(narrowed, decided, "{text}");
            compared += 1;
            log.undo(seq, "t", "and back").unwrap();
        }
    }
    assert!(compared >= 400);
}

fn decided_plan(text: &str) -> (recollect_graph::Log, BTreeMap<&'static str, recollect_graph::Entity>) {
    use recollect_graph::{Entity, Log, On, Op, Stored};
    use std::sync::Arc;
    let mut reg = Registry::new();
    plan::install(&mut reg);
    let mut log = Log::new(Arc::new(reg));
    let ids: BTreeMap<&str, Entity> = ["pp", "a", "b", "qq", "c", "d", "cc", "e", "f2", "f"].into_iter().map(|n| (n, Entity::new())).collect();
    let mut ops: Vec<Op> = ids.values().map(|e| Op::Spawn { id: *e }).collect();
    // `pp` and `qq` read as numbers from their options, and `cc` is true or false in the same way.
    for (d, options) in [("pp", [("a", 0.0), ("b", 3.0)]), ("qq", [("c", 1.0), ("d", 5.0)])] {
        ops.extend([Op::add(ids[d], DECISION), Op::add(ids[d], VALUE), Op::set_formula(ids[d], ValueCell::NAME, "self.chosen.value")]);
        for (o, price) in options {
            ops.extend([Op::add(ids[o], VALUE), Op::set(ids[o], ValueCell::num(price))]);
            ops.push(Op::link(ids[o], ids[d], ALTERNATIVE));
        }
    }
    ops.extend([Op::add(ids["cc"], DECISION), Op::add(ids["cc"], VALUE), Op::set_formula(ids["cc"], ValueCell::NAME, "self.chosen.value")]);
    for (o, v) in [("e", "red"), ("f2", "blue")] {
        ops.extend([Op::add(ids[o], VALUE), Op::set(ids[o], ValueCell::plain(Val::Text(v.to_string())))]);
        ops.push(Op::link(ids[o], ids["cc"], ALTERNATIVE));
    }
    ops.extend([Op::add(ids["f"], VALUE), Op::set_formula(ids["f"], ValueCell::NAME, text)]);
    for name in ["pp", "qq", "cc"] {
        ops.push(Op::link(ids["f"], ids[name], LABEL));
        ops.push(Op::Set { on: On::Edge { from: ids["f"], to: ids[name] }, cell: Called::NAME.to_string(), to: Some(Stored::Value(json!(name))) });
    }
    log.commit("t", "made", ops).unwrap();
    (log, ids)
}

// ---------- groups ----------

#[test]
fn a_group_is_however_many_links_have_that_label_including_none() {
    let mut g = N::with(sheet);
    g.formula("total", "sum(line)");
    assert_eq!(value(&g, "total"), ValueCell::num(0.0), "no lines yet is fine");
    g.formula("how_many", "count(line)").formula("every", "all(x in line: x > 0)").formula("some", "any(x in line: x > 5)").formula("times", "product(line)");
    g.value("a", ValueCell::num(2.0)).input("total", "a", "line").input("how_many", "a", "line").input("every", "a", "line").input("some", "a", "line").input("times", "a", "line");
    g.value("b", ValueCell::num(3.0)).input("total", "b", "line").input("how_many", "b", "line").input("every", "b", "line").input("some", "b", "line").input("times", "b", "line");
    assert_eq!(value(&g, "total"), ValueCell::num(5.0));
    assert_eq!(value(&g, "how_many"), ValueCell::num(2.0));
    assert_eq!(value(&g, "every"), ValueCell::plain(Val::Bool(true)));
    assert_eq!(value(&g, "some"), ValueCell::plain(Val::Bool(false)));
    assert_eq!(value(&g, "times"), ValueCell::num(6.0));

    g.formula("plain", "a").input("plain", "a", "a").input("plain", "b", "a");
    assert!(error(&g, "plain").contains("a is 2 nodes"), "{}", error(&g, "plain"));
}

#[test]
fn groups_can_follow_any_kind_of_link_and_scan_a_component() {
    let mut g = N::with(plan::install);
    for (t, d) in [("a", 1.0), ("b", 2.0)] {
        g.add(t, TASK).set(t, Estimate::days(d)).link(t, "whole", PART_OF);
    }
    g.formula("whole", "sum(in.part_of.estimate)");
    assert_eq!(value(&g, "whole"), ValueCell::num(3.0));
    g.formula("everything", "sum(t in with.task: t.estimate)");
    assert_eq!(value(&g, "everything"), ValueCell::num(3.0));
    g.formula("up", "count(x in out.part_of: true)");
    g.link("up", "elsewhere", PART_OF);
    assert_eq!(value(&g, "up"), ValueCell::num(1.0));
    g.add("c", TASK).set("c", Estimate::days(4.0)).link("c", "whole", PART_OF);
    assert_eq!(value(&g, "whole"), ValueCell::num(7.0), "a new one is noticed");
}

#[test]
fn a_cell_can_read_other_cells_and_their_fields() {
    let mut g = N::with(plan::install);
    g.add("gate", DECISION);
    g.link("yes", "gate", ALTERNATIVE).link("no", "gate", ALTERNATIVE);
    g.add("t", TASK).set("t", Estimate::days(2.0)).link("t", "yes", PART_OF);
    g.formula("t", "if(self.need, self.estimate * 100, 0)");
    let v = value(&g, "t").0.0;
    let at = |o: &str| BTreeMap::from([(g.id("gate").to_string(), g.id(o).to_string())]);
    assert_eq!((v.at(&at("yes")), v.at(&at("no"))), (&Val::Num(200.0), &Val::Num(0.0)));

    g.g.set_value(g.id("yes"), "supplier", json!({ "lead": { "days": 12 } }));
    g.formula("lead", "s.supplier.lead.days + 1").input("lead", "yes", "s");
    assert_eq!(value(&g, "lead"), ValueCell::num(13.0));
    g.formula("nothing", "s.supplier.nope").input("nothing", "yes", "s");
    assert_eq!(value(&g, "nothing"), ValueCell::plain(Val::Undefined), "a field that isn't there is undefined, like a cell that isn't");
}

// ---------- what can go wrong ----------

#[test]
fn a_missing_input_says_so_and_heals() {
    let mut g = N::with(sheet);
    g.formula("x", "rate * 2");
    assert!(error(&g, "x").contains("no input called rate"), "{}", error(&g, "x"));
    g.value("r", ValueCell::num(3.0)).input("x", "r", "rate");
    assert_eq!(value(&g, "x"), ValueCell::num(6.0));
    g.unlink("x", "r", LABEL);
    assert!(error(&g, "x").contains("no input called rate"));
    g.assert_round_trips();
}

#[test]
fn text_that_doesnt_parse_is_kept_says_why_and_heals() {
    let mut g = N::with(sheet);
    g.formula("x", "1 +");
    assert!(matches!(g.get::<ValueCell>("x"), Err(Error::Failed(detail)) if detail.contains("stops short")));
    g.formula("x", "1 + 1");
    assert_eq!(value(&g, "x"), ValueCell::num(2.0));
}

#[test]
fn types_are_checked_without_looking_at_a_world() {
    for (text, says) in [
        (r#"1 + "a""#, "+ needs numbers"),
        (r#"1 < "a""#, "< needs numbers"),
        (r#"1 = "a""#, "= can't compare"),
        (r#"if(1, 2, 3)"#, "if needs true or false"),
        (r#"if(true, 1, "a")"#, "the two sides of if"),
        (r#"!1"#, "! needs true or false"),
        (r#"-"a""#, "- needs a number"),
        (r#"1 && true"#, "&& needs true or false"),
        (r#"round("a")"#, "round needs numbers"),
        (r#"abs("a")"#, "needs"),
        (r#"sum(x in in.nothing: "a")"#, ""),
    ] {
        if says.is_empty() {
            continue;
        }
        let why = just_error(text);
        assert!(why.contains(says), "{text}: {why}");
        assert!(why.contains(text), "and it says which formula: {why}");
    }

    // It's a type error even in worlds the bad branch never runs in.
    let mut g = N::with(sheet);
    options(&mut g, "x", &["a", "b"]);
    g.formula("f", r#"if(x.chosen.value = "a", 1, "text")"#).input("f", "x", "x");
    assert!(error(&g, "f").contains("the two sides of if"), "{}", error(&g, "f"));
}

#[test]
fn a_choice_lands_on_a_shared_node_for_each_option_whose_value_is_a_number_if_they_all_are() {
    let mut g = N::with(sheet);
    options(&mut g, "colour", &["red", "blue"]);
    options(&mut g, "door", &["blue", "green"]);
    options(&mut g, "house", &["1", "2"]);
    let (colour_red, colour_blue) = (g.world(&[("colour", "red")]), g.world(&[("colour", "blue")]));
    let (house_1, house_2) = (g.world(&[("house", "1")]), g.world(&[("house", "2")]));
    let plain = |v: ValueCell, world: &BTreeMap<String, String>| v.0.0.at(world).clone();

    // What it lands on is a node nobody wrote, one for each option, named for it.
    let (red, blue) = (option_node(&g.g, "red", false), option_node(&g.g, "blue", false));
    let chosen = g.get::<Chosen>("colour").unwrap().unwrap().0.0;
    assert_eq!((chosen.at(&colour_red), chosen.at(&colour_blue)), (&Val::Ref(red), &Val::Ref(blue)));
    g.formula("named", "c.chosen.name").input("named", "colour", "c");
    assert_eq!(plain(value(&g, "named"), &colour_red), Val::Text("red".into()));

    // Two choices with the same option land on the same node, so they can be compared as nodes.
    g.formula("same", "c.chosen = d.chosen").input("same", "colour", "c").input("same", "door", "d");
    let both_blue = BTreeMap::from([(g.id("colour").to_string(), "blue".to_string()), (g.id("door").to_string(), "blue".to_string())]);
    let not_both = BTreeMap::from([(g.id("colour").to_string(), "red".to_string()), (g.id("door").to_string(), "blue".to_string())]);
    let same = value(&g, "same");
    assert_eq!((plain(same.clone(), &both_blue), plain(same, &not_both)), (Val::Bool(true), Val::Bool(false)));

    g.formula("fine", r#"c.chosen.value = "red""#).input("fine", "colour", "c");
    let fine = value(&g, "fine");
    assert_eq!((plain(fine.clone(), &colour_red), plain(fine, &colour_blue)), (Val::Bool(true), Val::Bool(false)));
    // There's no list of options in a type any more, so a misspelt option isn't an error: it's just
    // never what the choice lands on.
    g.formula("typo", r#"c.chosen.value = "rde""#).input("typo", "colour", "c");
    assert!(truth(&value(&g, "typo")).is_never());

    // Options that are all numbers read as numbers.
    g.formula("as_text", r#"h.chosen.value = "1""#).input("as_text", "house", "h");
    assert!(truth(&value(&g, "as_text")).is_never(), "options that are all numbers read as numbers");
    g.formula("as_number", "h.chosen.value = 1").input("as_number", "house", "h");
    let as_number = value(&g, "as_number");
    assert_eq!((plain(as_number.clone(), &house_1), plain(as_number, &house_2)), (Val::Bool(true), Val::Bool(false)));
    g.formula("sums", "h.chosen.value + 1").input("sums", "house", "h");
    let sums = value(&g, "sums");
    assert_eq!((plain(sums.clone(), &house_1), plain(sums, &house_2)), (Val::Num(2.0), Val::Num(3.0)));

    // And a choice has no value of its own: it's what it lands on that has one.
    g.formula("bare", "h + 1").input("bare", "house", "h");
    assert_eq!(value(&g, "bare"), ValueCell::plain(Val::Undefined));
}

#[test]
fn a_types_error_reaches_whatever_reads_it_and_a_stored_value_has_a_type_too() {
    let mut g = N::with(sheet);
    g.value("words", ValueCell::plain(Val::Text("hello".into())));
    g.formula("bad", "w + 1").input("bad", "words", "w");
    assert!(error(&g, "bad").contains("+ needs numbers, not text and a number"), "{}", error(&g, "bad"));
    g.formula("reads_it", "b + 1").input("reads_it", "bad", "b");
    assert!(error(&g, "reads_it").contains("+ needs numbers"));
    g.value("words", ValueCell::num(1.0));
    assert_eq!(value(&g, "reads_it"), ValueCell::num(3.0), "and it heals");
}

#[test]
fn changing_a_number_doesnt_rework_any_types_downstream() {
    let mut g = N::with(sheet);
    g.value("a", ValueCell::num(1.0));
    g.formula("b", "a + 1").input("b", "a", "a");
    g.formula("c", "b + 1").input("c", "b", "b");
    value(&g, "c");
    g.take_ran();
    g.value("a", ValueCell::num(5.0));
    assert_eq!(value(&g, "c"), ValueCell::num(7.0));
    let ran = g.ran_cells();
    assert_eq!(ran.iter().filter(|c| c.starts_with("value_type(")).collect::<Vec<_>>(), vec!["value_type(a)"], "{ran:?}");
    assert!(!ran.contains(&"value_type(c)".to_string()));
}

#[test]
fn a_loop_is_found_by_its_types_and_heals() {
    let mut g = N::with(sheet);
    g.formula("a", "b + 1").input("a", "b", "b");
    g.formula("b", "a + 1").input("b", "a", "a");
    assert!(matches!(g.get::<ValueCell>("a"), Err(Error::Cycle(_))));
    g.value("b", ValueCell::num(1.0));
    assert_eq!(value(&g, "a"), ValueCell::num(2.0));
    g.assert_round_trips();
}

#[test]
fn a_formula_can_only_go_on_a_cell_that_says_it_can_hold_one() {
    let mut g = N::with(plan::install);
    // Which option a decision took is only ever typed in.
    g.add("x", DECISION).add("x", TASK).formula_on("x", "chosen", "true");
    let e = g.id("x");
    assert!(matches!(g.g.get_value(e, "chosen"), Err(Error::Failed(why)) if why.contains("chosen can't hold a formula")));
    g.formula_on("x", Estimate::NAME, r#""soon""#);
    let Err(Error::Failed(why)) = g.get::<Estimate>("x") else { panic!() };
    assert!(why.contains("this cell holds a number, and this formula gives text"), "{why}");
}

#[test]
fn too_many_different_values_is_a_clear_error_and_not_a_hang() {
    let mut g = N::with(sheet);
    let mut text = Vec::new();
    for i in 0..24 {
        let name = format!("c{i}");
        options(&mut g, &name, &["0", &(1u64 << i).to_string()]);
        g.input("total", &name, &name);
        text.push(format!("{name}.chosen.value"));
    }
    g.formula("total", &text.join(" + "));
    let why = error(&g, "total");
    assert!(why.contains("more than 10000 different values"), "{why}");
    assert!(why.contains("and 1") || why.contains(", and"), "it says what it depends on: {why}");

    // True or false stays a diagram, so the same size of thing is fine that way.
    let mut g = N::with(sheet);
    let mut text = Vec::new();
    for i in 0..24 {
        let name = format!("c{i}");
        options(&mut g, &name, &["0", "1"]);
        g.input("all_on", &name, &name);
        text.push(format!("{name}.chosen.value = 1"));
    }
    g.formula("all_on", &text.join(" && "));
    assert_eq!(value(&g, "all_on").0.0.len(), 2);
}

// ---------- macros ----------

fn macro_node(g: &mut N, name: &str, params: &[&str], body: &str) {
    g.add(name, MACRO).set(name, Macro { params: params.iter().map(|s| s.to_string()).collect(), body: body.to_string() });
}

#[test]
fn a_macro_means_what_it_expands_to() {
    let mut g = with_choices();
    macro_node(&mut g, "cost", &["days", "rate"], "days * rate + 1");
    g.formula("with", "cost(p, q)").input("with", "cost", "cost").input("with", "p", "p").input("with", "q", "q");
    g.formula("by_hand", "p * q + 1").input("by_hand", "p", "p").input("by_hand", "q", "q");
    assert_eq!(value(&g, "with"), value(&g, "by_hand"));
    g.assert_round_trips();
}

#[test]
fn an_argument_can_be_a_node_a_group_or_a_value() {
    let mut g = N::with(sheet);
    g.value("a", ValueCell::num(2.0)).value("b", ValueCell::num(3.0));
    macro_node(&mut g, "double", &["x"], "x * 2");
    macro_node(&mut g, "total", &["things"], "sum(t in things: t)");
    g.formula("of_a_value", "double(1 + 1)").input("of_a_value", "double", "double");
    assert_eq!(value(&g, "of_a_value"), ValueCell::num(4.0));
    g.formula("of_a_node", "double(a)").input("of_a_node", "double", "double").input("of_a_node", "a", "a");
    assert_eq!(value(&g, "of_a_node"), ValueCell::num(4.0));
    g.formula("of_a_group", "total(line)").input("of_a_group", "total", "total").input("of_a_group", "a", "line").input("of_a_group", "b", "line");
    assert_eq!(value(&g, "of_a_group"), ValueCell::num(5.0));
}

#[test]
fn a_macro_can_use_macros_it_links_to_and_changing_one_reruns_only_what_uses_it() {
    let mut g = N::with(sheet);
    macro_node(&mut g, "double", &["x"], "x * 2");
    macro_node(&mut g, "quadruple", &["x"], "double(double(x))");
    g.input("quadruple", "double", "double");
    g.formula("uses", "quadruple(3)").input("uses", "quadruple", "quadruple");
    g.formula("other", "1 + 1");
    assert_eq!(value(&g, "uses"), ValueCell::num(12.0));
    value(&g, "other");
    g.take_ran();
    macro_node(&mut g, "double", &["x"], "x * 3");
    assert_eq!(value(&g, "uses"), ValueCell::num(27.0));
    value(&g, "other");
    assert_eq!(g.ran_values(), vec!["uses"]);
}

#[test]
fn a_macro_calling_itself_is_an_ordinary_loop() {
    let mut g = N::with(sheet);
    macro_node(&mut g, "a", &["x"], "b(x)");
    macro_node(&mut g, "b", &["x"], "a(x)");
    g.input("a", "b", "b").input("b", "a", "a");
    g.formula("uses", "a(1)").input("uses", "a", "a");
    assert!(matches!(g.get::<ValueCell>("uses"), Err(Error::Cycle(_))));
}

#[test]
fn a_macro_is_closed() {
    let mut g = N::with(sheet);
    g.value("rate", ValueCell::num(2.0));
    macro_node(&mut g, "leaks", &["x"], "x * rate");
    g.input("leaks", "rate", "rate");
    g.formula("f", "leaks(1)").input("f", "leaks", "leaks").input("f", "rate", "rate");
    assert!(error(&g, "f").contains("a macro can only use its parameters, not rate"), "{}", error(&g, "f"));

    macro_node(&mut g, "peeks", &[], "self.need");
    g.formula("h", "peeks()").input("h", "peeks", "peeks");
    assert!(error(&g, "h").contains("a macro can only use its parameters"), "{}", error(&g, "h"));
}

#[test]
fn names_inside_a_macro_cant_clash_with_names_at_the_call() {
    let mut g = N::with(sheet);
    g.value("y", ValueCell::num(10.0));
    // The body's `x` and the caller's `x` are different things.
    macro_node(&mut g, "add_one", &["x"], "x + 1");
    g.formula("f", "add_one(x)").input("f", "add_one", "add_one").input("f", "y", "x");
    assert_eq!(value(&g, "f"), ValueCell::num(11.0));
    // And a binder inside one doesn't capture an argument with the same name.
    g.value("a", ValueCell::num(1.0));
    macro_node(&mut g, "scaled", &["things", "by"], "sum(x in things: x * by)");
    g.formula("h", "sum(x in line: scaled(line, x))").input("h", "scaled", "scaled").input("h", "a", "line");
    assert_eq!(value(&g, "h"), ValueCell::num(1.0));
}

#[test]
fn an_enormous_expansion_is_an_error_and_not_a_hang() {
    let mut g = N::with(sheet);
    macro_node(&mut g, "f0", &["x"], "x + x");
    for i in 1..40 {
        let (this, prev) = (format!("f{i}"), format!("f{}", i - 1));
        macro_node(&mut g, &this, &["x"], &format!("{prev}({prev}(x))"));
        g.input(&this, &prev, &prev);
    }
    g.formula("big", "f39(1)").input("big", "f39", "f39");
    let why = error(&g, "big");
    assert!(why.contains("too big"), "{why}");
}

#[test]
fn wrong_numbers_of_arguments_are_errors() {
    assert!(just_error("abs(1, 2)").contains("abs takes 1 and was given 2"));
    assert!(just_error("if(true, 1)").contains("if takes 3 and was given 2"));
    assert!(just_error("nope(1)").contains("no function called nope"));
    let mut g = N::with(sheet);
    g.value("a", ValueCell::num(1.0));
    g.formula("f", "a(1)").input("f", "a", "a");
    assert!(error(&g, "f").contains("a isn't a macro"));
}

// ---------- decisions and plans ----------

#[test]
fn a_formula_can_ask_which_way_a_decision_went_and_set_an_estimate() {
    let mut g = N::with(|reg| {
        plan::install(reg);
        schedule::install(reg);
    });
    g.add("how", DECISION);
    g.link("gpu", "how", ALTERNATIVE).link("cpu", "how", ALTERNATIVE);
    g.add("train", TASK).link("train", "how", PART_OF);
    g.formula_on("train", Estimate::NAME, "if(chose(how, gpu), 3, 5)");
    g.input("train", "how", "how").input("train", "gpu", "gpu");

    let (how, gpu, cpu) = (g.id("how").to_string(), g.id("gpu").to_string(), g.id("cpu").to_string());
    let at = |o: &String| BTreeMap::from([(how.clone(), o.clone())]);
    let finish = g.get::<Finish>("train").unwrap().unwrap().0.0;
    assert_eq!((finish.at(&at(&gpu)), finish.at(&at(&cpu))), (&Some(3.0), &Some(5.0)));

    // A date that depends on itself through its own estimate is a loop.
    g.formula_on("train", Estimate::NAME, "self.finish");
    assert!(matches!(g.get::<Finish>("train"), Err(Error::Cycle(_))));

    // And one a formula can't work out makes the date unknowable, not an error.
    g.formula_on("train", Estimate::NAME, "if(chose(how, gpu), undefined, 5)");
    let finish = g.get::<Finish>("train").unwrap().unwrap().0.0;
    assert_eq!((finish.at(&at(&gpu)), finish.at(&at(&cpu))), (&None, &Some(5.0)));
    g.assert_round_trips();
}

#[test]
fn chose_needs_a_choice_and_what_is_read_through_a_gap_is_undefined() {
    let mut g = N::with(plan::install);
    g.value("not_a_decision", ValueCell::num(1.0)).value("o", ValueCell::num(2.0));
    g.formula("f", "chose(d, o)").input("f", "not_a_decision", "d").input("f", "o", "o");
    assert!(error(&g, "f").contains("chose needs a choice"), "{}", error(&g, "f"));

    g.add("d", DECISION);
    g.formula("d", "self.chosen.value");
    g.value("known", ValueCell::num(5.0)).link("known", "d", ALTERNATIVE);
    g.add("gap", UNKNOWN).link("gap", "d", ALTERNATIVE);
    let v = value(&g, "d").0.0;
    let at = |o: &str| BTreeMap::from([(g.id("d").to_string(), g.id(o).to_string())]);
    assert_eq!((v.at(&at("known")), v.at(&at("gap"))), (&Val::Num(5.0), &Val::Undefined));
}

/// The sample's facts, with what the design worked out by hand.
#[test]
fn effort_is_worked_out_across_a_plan_with_shared_work_counted_once() {
    let mut g = N::with(plan::install);
    g.add("calib", DECISION);
    for o in ["cad", "free", "board"] {
        g.link(o, "calib", ALTERNATIVE);
    }
    g.add("else", UNKNOWN).link("else", "calib", ALTERNATIVE);
    for (t, d, parents) in [("imu", 1.0, vec!["calib"]), ("static", 2.0, vec!["cad", "free"]), ("export", 3.0, vec!["cad"]), ("search", 5.0, vec!["free"]), ("detector", 4.0, vec!["board"])] {
        g.add(t, TASK).set(t, Estimate::days(d));
        for p in parents {
            g.link(t, p, PART_OF);
        }
    }
    g.formula("effort", "effort()");
    let v = value(&g, "effort").0.0;
    let at = |o: &str| BTreeMap::from([(g.id("calib").to_string(), g.id(o).to_string())]);
    assert_eq!((v.at(&at("cad")), v.at(&at("free")), v.at(&at("board")), v.at(&at("else"))), (&Val::Num(6.0), &Val::Num(8.0), &Val::Num(5.0), &Val::Undefined));

    // A requirement rules worlds out, with a reason you can read.
    g.formula("short", "e <= 6").input("short", "effort", "e").add("short", REQUIRES);
    assert_eq!(g.show(&truth(&value(&g, "short"))), "calib∈{cad,board}");
}

#[test]
fn any_formula_can_be_tried_on_a_node_with_nothing_changed() {
    let mut g = with_choices();
    g.formula("f", "(p + 1) * abs(q)").input("f", "p", "p").input("f", "q", "q");
    let whole = value(&g, "f");
    let e = g.id("f");

    let (part, _) = try_formula(&g.g, e, "p + 1").unwrap();
    let world = BTreeMap::from([(g.id("p").to_string(), "3".to_string())]);
    assert_eq!(part.at(&world), &Val::Num(4.0));

    let (_, expansion) = try_formula(&g.g, e, "abs(q)").unwrap();
    let q = g.id("q").to_string();
    assert_eq!(expansion, format!("if(({q} < 0), -{q}, {q})"), "and it says what that really means");

    assert!(matches!(try_formula(&g.g, e, "nope"), Err(Error::Failed(why)) if why.contains("no input called nope")));
    assert!(matches!(try_formula(&g.g, e, "1 +"), Err(Error::Failed(why)) if why.contains("stops short")));
    assert_eq!(value(&g, "f"), whole);
    assert!(!g.g.has_own(e, "formula_try"), "the graph itself never heard about any of this");
}
