//! Formulas, as a system. Any cell can hold a value typed in or a formula over what its node has
//! labelled. A choice lands on one node in each world, and what's read through it, as `car.chosen.price`,
//! is that node's. See `docs/expressions.md`.
//!
//! A formula's meaning doesn't mention worlds: in any world, its value is what ordinary evaluation
//! gives on its inputs' values in that world. World values are how that's worked out exactly, and
//! `tests/expr.rs` checks the two agree. That's also why nothing here can collapse worlds into one
//! number: narrowing that number couldn't then agree with really deciding.
//!
//! A formula goes through three steps. It's expanded, which replaces every name with the node it
//! means and every function with its definition. It's type-checked, without looking at any world.
//! Then it's evaluated.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use recollect_graph::table::Key;
use recollect_graph::chance::{self, Given, Known, Num, Rand, Reader, Share, Std, Term};
use recollect_graph::worlds::{Bool, Cond, Picks, Space, Var, Worldly};
use recollect_graph::{Advice, Backed, Cell, CellType, Compile, Computed, Cx, Entity, Error, Graph, Registry, Severity, Shown, cell};
use serde_json::{Value, json};

use crate::expr::{self, BinOp, CORE, Estimate, Expr, Type, Val};
use crate::plan::Done;
use crate::structure::{Dropped, Need};
use crate::schedule::{Finish, LatestFinish, LatestStart, PlanEnd, Start};

/// The most distinct values a formula can have across worlds. Beyond this, it says so.
const MOST_CASES: usize = 10_000;
/// The most pairs of cases one step can look at.
const MOST_PAIRS: usize = 4_000_000;
/// The most steps in expanding one formula. Macros calling macros can get very big very quickly.
pub(crate) const MOST_STEPS: usize = 200_000;

pub const VALUE: &str = "value";
pub const CHOICE: &str = "choice";
pub const OPTIONS: &str = "options";
pub const AT: &str = "at";
pub const REQUIRES: &str = "requires";
pub const EVIDENCE: &str = "evidence";
/// What a view has found out by pinning, on a copy of the graph it looks through. Never saved.
pub const FOUND: &str = "found";
pub const MACRO: &str = "macro";
/// What should be true, said so a notice can say where it isn't: it never rules any world out.
pub const SHOULD: &str = "should";

/// A truth that should hold, as a formula. Written on a kind, each instance is held to it, with `self`
/// meaning that instance.
#[cell("should")]
pub struct Should(pub Worldly<Option<bool>>);
pub const LABEL: &str = "label";
pub const IS_A: &str = "is_a";

/// A node's answer. Typed in, or a choice from its options, or a formula.
#[cell("value")]
pub struct ValueCell(pub Worldly<Val>);

impl ValueCell {
    pub fn plain(v: Val) -> Self {
        ValueCell(Worldly::plain(v))
    }

    pub fn num(n: f64) -> Self {
        Self::plain(Val::Num(n))
    }
}

/// The worlds where a value is true. Undefined isn't true.
pub fn truth(v: &ValueCell) -> Bool {
    v.0.0.holds(|v| *v == Val::Bool(true))
}

/// How surely a requirement has to be met, from 0 to 1. Always, unless it says.
#[cell("at_least")]
pub struct AtLeast(pub f64);

// ---------- what's known, and what's demanded ----------
//
// Evidence is known to be true: it leaves only the worlds, and the draws, where it holds, and never
// restricts a choice. A requirement is demanded of choices: it leaves only the ways they could go that
// make it true, as surely as it says, given the evidence. With nothing a matter of chance, each leaves
// exactly the worlds where its value is true. See `docs/distributions.md`.

/// Whether an open choice is a matter of chance rather than someone's to make: a draw from a short list,
/// or a decision that says it's uncertain.
pub fn is_chance(g: &Graph, var: &str) -> bool {
    crate::is_draw(var) || var.parse::<Entity>().is_ok_and(|e| g.has(e, crate::structure::UNCERTAIN))
}

/// The worlds a piece of evidence leaves: those where it's true. Where it's a matter of draws it says
/// nothing about which world this is, and what it says about the draws is `known_draws`.
fn evidence_worlds(v: &ValueCell) -> Bool {
    v.0.0.holds(|v| matches!(v, Val::Bool(true) | Val::Rand(_) | Val::Likely(_)))
}

/// Everything known that's a matter of draws, together: the uncertain truths, `Term::Flag(false)` if they
/// can't all hold, and the log of how likely what was seen is, where that depends on draws.
fn known_terms(values: impl Iterator<Item = ValueCell>) -> (Option<Term>, Option<Term>) {
    let (mut truth, mut weight): (Option<Term>, Option<Term>) = (None, None);
    let join = |so_far: Option<Term>, op: chance::Op, r: Rand| Some(match so_far {
        None => Term::Rand(r),
        Some(so_far) => chance::bin(op, &so_far, &Term::Rand(r)),
    });
    for v in values {
        match v.0.0.cases().as_slice() {
            [(_, Val::Rand(r))] if r.is_truth() => truth = join(truth, chance::Op::And, *r),
            [(_, Val::Likely(r))] => weight = join(weight, chance::Op::Add, *r),
            _ => {}
        }
    }
    (truth, weight)
}

fn known_term(g: &Graph) -> Option<Term> {
    known_terms(g.having(EVIDENCE).into_iter().filter_map(|e| g.get::<ValueCell>(e).ok().flatten())).0
}

/// A reader's `Known`, from the truth and the weight `known_terms` gives. A truth that can't hold is
/// left out here: what can't all hold is said where it's found.
fn known_of((truth, weight): (Option<Term>, Option<Term>)) -> Known {
    let rand = |t: Option<Term>| match t {
        Some(Term::Rand(r)) => Some(r),
        _ => None,
    };
    Known { truth: rand(truth), log_weight: rand(weight) }
}

/// What's known as a matter of draws, for a `Reader` to be given.
pub fn known_draws(g: &Graph) -> Known {
    known_of(known_terms(g.having(EVIDENCE).into_iter().filter_map(|e| g.get::<ValueCell>(e).ok().flatten())))
}

/// What a view has found out about matters of chance by pinning them: each, and the option it landed on.
/// It's only ever on a copy of the graph a view looks through, so `chance` and `mean` read it as they read
/// anything else, and a what-if can't disagree with finding it out.
#[cell("found")]
pub struct Found(pub BTreeMap<String, String>);

/// Whether something read from draws comes out within this many standard errors of what it's held against
/// is too close to call.
const CLOSE: f64 = 3.0;

/// For each way the choices could go, how likely `what` is to be true, or what it comes to on average,
/// over every matter of chance, given what's known. Where nothing known is possible, or what it comes
/// to is undefined some of the time, it's nothing.
struct Over {
    value: Cond<Option<f64>>,
    /// How far out that could be, as a standard error, where it was read from draws.
    error: Cond<Option<f64>>,
    /// How many draws the most read was read from, if any were.
    draws: usize,
    /// How what was seen was taken into account in reading them, if it was.
    given: Option<Given>,
}

fn over_chance(what: &Cond<Val>, known: &Bool, reader: &mut Reader, chancy: &dyn Fn(&str) -> bool, mean: bool) -> Result<Over, Error> {
    let too_many = || Error::Failed("this differs with too many different chances, over the ways the choices could go, to work out".to_string());
    let space = Space::global();
    // Summed over the cases: the value, how far out it could be, and where it's undefined, each weighted
    // by how likely that case is for each way the choices could go.
    let (mut sum, mut err, mut lost) = (space.pure(0.0), space.pure(Some(0.0)), space.pure(0.0));
    let (mut draws, mut given) = (0, None);
    for (when, v) in what.cases() {
        // What this case comes to, how far out that could be, and how much of it is undefined.
        let (value, error, undefined): (f64, Option<f64>, f64) = match (v, mean) {
            (Val::Undefined, _) => (0.0, Some(0.0), 1.0),
            (Val::Bool(b), false) => (f64::from(u8::from(*b)), Some(0.0), 0.0),
            (Val::Num(n), true) => (*n, Some(0.0), 0.0),
            (Val::Rand(r), false) if r.is_truth() => {
                let c = reader.chance(&[Share { weight: 1.0, value: Term::Rand(*r) }]);
                draws = draws.max(c.draws);
                given = given.or(c.given);
                // Undefined isn't true.
                (c.chance * (1.0 - c.undefined), Some(c.error), 0.0)
            }
            (Val::Rand(r), true) if !r.is_truth() => {
                let s = reader.summary(&[Share { weight: 1.0, value: Term::Rand(*r) }]);
                draws = draws.max(s.draws);
                given = given.or(s.given);
                (s.mean, Some(s.error), s.undefined)
            }
            (Val::Estimate(e), true) => {
                draws = draws.max(e.draws);
                given = given.or(e.given);
                (e.value, e.error, 0.0)
            }
            (other, false) => return failed(format!("chance needs true or false, not {}", Type::of(other).word())),
            (other, true) => return failed(format!("mean needs a number, not {}", Type::of(other).word())),
        };
        let here = (&when & known).weigh(chancy).ok_or_else(too_many)?;
        sum = sum.zip(&here, |so_far, p| so_far + value * p);
        err = err.zip(&here, |so_far, p| so_far.zip(error).map(|(s, e)| s + e * p));
        lost = lost.zip(&here, |so_far, p| so_far + undefined * p);
    }
    let possible = known.weigh(chancy).ok_or_else(too_many)?;
    // What isn't true isn't undefined, so for a chance only the mean can be lost.
    let value = sum.zip(&lost, |s, l| (*s, *l)).zip(&possible, |(s, l), p| if *p <= 0.0 || (mean && *l > 1e-12) { None } else { Some(s / p) });
    let error = err.zip(&possible, |e, p| if *p > 0.0 { e.map(|e| e / p) } else { Some(0.0) });
    Ok(Over { value, error, draws, given })
}

/// A value from `Over`: a number where it's exact, an estimate where it was read from draws, and undefined
/// where there's nothing to say.
fn over_values(over: &Over) -> Cond<Val> {
    over.value.zip(&over.error, |v, e| match (v, e) {
        (None, _) => Val::Undefined,
        (Some(v), Some(e)) if *e == 0.0 => Val::num(*v),
        (Some(v), e) => Val::Estimate(Estimate { value: *v, error: *e, draws: over.draws, given: over.given }),
    })
}

/// What's known, as anything a cell reads: the worlds evidence leaves, what it says about draws, and what's
/// been found out by pinning, which only a view's copy of the graph has.
fn known_by(cx: &Cx) -> Result<(Bool, (Option<Term>, Option<Term>), Picks, BTreeMap<String, String>), Error> {
    let mut known = Space::global().always();
    let mut values = Vec::new();
    for e in cx.scan(EVIDENCE) {
        let Some(v) = cx.get::<ValueCell>(e)? else { continue };
        known = &known & &evidence_worlds(&v);
        values.push(v);
    }
    let draws = known_terms(values.into_iter());
    let mut found = BTreeMap::new();
    for e in cx.scan(FOUND) {
        if let Some(Found(f)) = cx.get::<Found>(e)? {
            found.extend(f);
        }
    }
    let picks = Space::global().picks(&found);
    Ok((known, draws, picks, found))
}

/// What finding out these draws leaves, for those that are several variables: the worlds where whichever
/// is in use lands on what each was found to be.
pub fn draws_found(found: &BTreeMap<String, String>) -> Bool {
    let mut out = Space::global().always();
    for (name, option) in found.iter().filter(|(k, _)| crate::is_draw(k)) {
        if let Some(group) = Space::global().draw_group(name) {
            out = &out & &group.lands(option);
        }
    }
    out
}

/// `chance(x)` or `mean(x)`: over every matter of chance, given what's known, for each way the choices
/// that are someone's to make could go. Never over one of those.
fn expect(cx: &Cx, mean: bool, of: &Ev) -> Result<Cond<Val>, Error> {
    let (known, draws, picks, found) = known_by(cx)?;
    // What's known can't all be true, so nothing can be said given it.
    if matches!(draws.0, Some(Term::Flag(false))) {
        return Ok(pure(Val::Undefined));
    }
    let reader_known = known_of(draws);
    let whole = of.data();
    // A draw that's several variables is found out where whichever is in use lands on what it was found to be.
    let known = &known & &draws_found(&found);
    let what = whole.given(&picks);
    let known = known.given(&picks);
    let chancy = |name: &str| crate::is_draw(name) || name.parse::<Entity>().is_ok_and(|e| cx.has(e, crate::structure::UNCERTAIN));
    let mut reader = Reader::given(reader_known);
    if reader_known.is_some() && reader.draws_kept() == 0 {
        return Ok(pure(Val::Undefined));
    }
    Ok(over_values(&over_chance(&what, &known, &mut reader, &chancy, mean)?))
}

/// What a requirement leaves, and how that was judged.
pub struct Judged {
    /// The ways the choices could go that meet it as surely as it says.
    pub allowed: Bool,
    /// The matters of chance it depends on that no choice knows. It's judged as if every choice is made
    /// before those are known.
    pub before: Vec<String>,
    /// The choices made knowing how something turns out, each with what it knows.
    pub knowing: Vec<(String, Vec<String>)>,
    /// It depends on how uncertain numbers fall, which is also not known when the choices are made.
    pub uncertain: bool,
    /// How likely it's met was read from draws for some way the choices could go.
    pub drawn: bool,
    /// And for some way they could go, that's too close to how sure it has to be to call.
    pub close: bool,
    /// Why it couldn't be judged, if it couldn't. Then it rules nothing out.
    pub unjudged: Option<String>,
}

/// The ways something a choice knows could turn out: the worlds where it lands on each.
fn outcomes(g: &Graph, known: Entity, picks: &Picks) -> Result<Vec<Bool>, String> {
    let name = || crate::label_of(g, known);
    let value = if g.has(known, CHOICE) {
        g.get::<Chosen>(known).map_err(|e| format!("what {} lands on can't be worked out: {e:?}", name()))?.map(|c| c.0.0)
    } else {
        g.get::<ValueCell>(known).map_err(|e| format!("{} can't be worked out: {e:?}", name()))?.map(|v| v.0.0)
    };
    let Some(value) = value else { return Err(format!("{} has nothing to know", name())) };
    if any_rand(&value) {
        return Err(format!("a choice knows {}, which is an uncertain number, and what's best to do for each way that could fall isn't worked out yet", name()));
    }
    let value = value.given(picks);
    let mut seen: Vec<&Val> = Vec::new();
    let cases = value.cases();
    for (_, v) in &cases {
        if !seen.contains(v) {
            seen.push(v);
        }
    }
    Ok(seen.into_iter().map(|v| value.holds(|x| x == v)).filter(|b| !b.is_never()).collect())
}

/// The most ways the things choices know could turn out, together, that a requirement is judged for.
const MOST_KNOWN: usize = 256;

/// Which ways the choices could go meet a requirement. For each, how likely its value is to be true over
/// everything that's a matter of chance, given what's known, is held against how sure it has to be. A
/// choice made knowing how something turns out is judged for each way it could: an option is allowed where
/// its chance of meeting it, given that, is enough. An earlier choice is allowed only where, however that
/// turns out, a later choice is. `known` is the worlds the evidence leaves, and `reader` is given whatever's
/// known about draws.
pub fn judge(g: &Graph, e: Entity, picks: &Picks, known: &Bool, reader: &mut Reader) -> Result<Option<Judged>, Error> {
    let Some(value) = g.get::<ValueCell>(e)? else { return Ok(None) };
    let at_least = g.get::<AtLeast>(e)?.map_or(1.0, |a| a.0);
    let value = value.0.0.given(picks);
    let chancy = |name: &str| is_chance(g, name);
    let uncertain = any_rand(&value);
    let vars = value.vars();
    // The choices in it that are someone's to make, and what each knows when it's made.
    let own: Vec<&Var> = vars.iter().filter(|v| !chancy(&v.name())).collect();
    let mut knowing: BTreeMap<String, BTreeSet<Entity>> = BTreeMap::new();
    for v in &own {
        if let Ok(d) = v.name().parse::<Entity>() {
            let knows: BTreeSet<Entity> = g.targets_of(d, crate::structure::KNOWING).into_iter().collect();
            if !knows.is_empty() {
                knowing.insert(v.name(), knows);
            }
        }
    }
    let label = |e: &Entity| crate::label_of(g, *e);
    let knowing_view: Vec<(String, Vec<String>)> = knowing.iter().map(|(d, k)| (crate::label_of(g, d.parse().expect("a decision")), k.iter().map(label).collect())).collect();
    let known_names: BTreeSet<String> = knowing.values().flatten().map(|e| e.to_string()).collect();
    let mut before: Vec<String> = vars.iter().map(Var::name).filter(|n| chancy(n) && !known_names.contains(n) && !crate::draw_site(n).is_some_and(|(node, _, _)| known_names.contains(node))).collect();
    before.sort();
    before.dedup();
    let unjudged = |why: String| Ok(Some(Judged { allowed: Space::global().always(), before: before.clone(), knowing: knowing_view.clone(), uncertain, drawn: false, close: false, unjudged: Some(why) }));

    let sets: BTreeSet<&BTreeSet<Entity>> = knowing.values().collect();
    if sets.len() > 1 {
        return unjudged("choices here know different things, and working out several in a row like that isn't built yet".to_string());
    }
    // Every way what's known when the later choices are made could turn out, together.
    let mut events = vec![Space::global().always()];
    if let Some(knows) = sets.into_iter().next() {
        for k in knows {
            let each = match outcomes(g, *k, picks) {
                Ok(each) => each,
                Err(why) => return unjudged(why),
            };
            events = events.iter().flat_map(|e| each.iter().map(move |o| e & o)).filter(|b| !b.is_never()).collect();
            if events.len() > MOST_KNOWN {
                return unjudged(format!("what the choices know could turn out more than {MOST_KNOWN} ways"));
            }
        }
    }
    let later: Vec<Var> = own.iter().filter(|v| knowing.contains_key(&v.name())).map(|v| (*v).clone()).collect();
    let (mut per_state, mut each_later, mut drawn, mut close) = (Space::global().never(), Space::global().always(), false, false);
    for event in &events {
        let here = known & event;
        let over = over_chance(&value, &here, reader, &chancy, false)?;
        drawn |= over.draws > 0;
        let allowed = over.value.holds(|r| r.is_none_or(|r| r >= at_least - 1e-9));
        per_state = &per_state | &(event & &allowed);
        let near = over.value.zip(&over.error, |r, e| r.zip(*e).is_some_and(|(r, e)| e > 0.0 && (r - at_least).abs() < CLOSE * e));
        close |= !near.holds(|n| *n).is_never();
        if !later.is_empty() {
            // However this turns out, among the ways it can, a later choice has to have something allowed.
            let possible = here.weigh(&chancy).ok_or_else(|| Error::Failed("this is met with too many different chances to judge".to_string()))?.holds(|p| *p > 0.0);
            each_later = &each_later & &(&!&possible | &allowed.exists(&later));
        }
    }
    Ok(Some(Judged { allowed: &per_state & &each_later, before, knowing: knowing_view, uncertain, drawn, close, unjudged: None }))
}

/// The worlds left once everything known has ruled out what it does, and every requirement has ruled out
/// the choices that don't meet it, among those consistent with `picks`. `found` is anything else found
/// out by pinning.
pub fn worlds_left(g: &Graph, picks: &Picks, found: &Bool, reader: &mut Reader) -> Bool {
    let known = g.registry().constrained(g).map_or_else(|| Space::global().always(), |c| c.given(picks));
    let known = &known & found;
    let mut left = known.clone();
    for e in g.having(REQUIRES) {
        // One that can't be judged says so in its own cell, or in a notice, and rules nothing out.
        if let Ok(Some(judged)) = judge(g, e, picks, &known, reader) {
            left = &left & &judged.allowed;
        }
    }
    left
}

/// Whether anything here rules worlds out.
pub fn narrows(g: &Graph) -> bool {
    g.registry().constrained(g).is_some() || !g.having(REQUIRES).is_empty()
}

/// What's worth saying about how the requirements and the evidence were judged.
fn judging(g: &Graph) -> Vec<Advice> {
    let mut out = Vec::new();
    let known_as_draws = known_term(g);
    if known_as_draws == Some(Term::Flag(false)) {
        if let Some(e) = g.having(EVIDENCE).first() {
            out.push(Advice { rule: "evidence_cant_all_hold".to_string(), on: *e, detail: "what's known about uncertain numbers can't all be true at once".to_string() });
        }
    }
    let mut reader = Reader::given(known_draws(g));
    if known_draws(g).is_some() && reader.draws_kept() == 0 {
        if let Some(e) = g.having(EVIDENCE).first() {
            out.push(Advice { rule: "evidence_never_drawn".to_string(), on: *e, detail: format!("what's known held in none of {} draws, so nothing that depends on it can be read", chance::DRAWS) });
        }
    }
    if let Some(moved) = reader.moved()
        && moved.distinct > 0
        && let Some(e) = g.having(EVIDENCE).into_iter().find(|e| matches!(g.get::<ValueCell>(*e), Ok(Some(v)) if v.0.0.cases().iter().any(|(_, v)| matches!(v, Val::Likely(_)))))
    {
        let rarely = moved.accepted < 0.1;
        if moved.reached < 1.0 {
            out.push(Advice {
                rule: "evidence_moving_stopped_short".to_string(),
                on: e,
                detail: format!("moving the draws to fit what's been seen stopped after {} steps with only {:.0}% of its weight brought in, so what's read through them is given only that much of it, and is wrong", moved.steps, moved.reached * 100.0),
            });
        }
        out.push(Advice {
            rule: if rarely { "evidence_moves_rarely_taken" } else { "evidence_moved_draws" }.to_string(),
            on: e,
            detail: format!(
                "weighing the draws by what's been seen would have left too few of them counting, so the draws it depends on were moved to fit it instead, in {} steps. Of the {} draws, {} are distinct, and they're taken to count as {:.0}% of them, since moved draws are related to where they came from. The last moves were taken {:.0}% of the time{}",
                moved.steps,
                chance::DRAWS,
                moved.distinct,
                100.0 * moved.worth as f64 / chance::DRAWS as f64,
                moved.accepted * 100.0,
                if rarely { ". That's rarely, which is when what's read can be wrong without the numbers showing it" } else { "" }
            ),
        });
    }
    let known = g.registry().constrained(g).unwrap_or_else(|| Space::global().always());
    let picks = Space::global().picks(&BTreeMap::new());
    for e in g.having(REQUIRES) {
        let Ok(Some(judged)) = judge(g, e, &picks, &known, &mut reader) else { continue };
        if (!judged.before.is_empty() || judged.uncertain) && judged.unjudged.is_none() {
            // A draw is called by where it's written, so it reads as that node.
            let mut names: Vec<String> = judged.before.iter().map(|n| crate::draw_site(n).map_or(n.clone(), |(node, _, _)| node.to_string())).collect();
            names.dedup();
            if judged.uncertain {
                names.push("how its uncertain numbers fall".to_string());
            }
            let nothing = if judged.allowed.is_never() { ". No choice makes it sure enough: if it's something that's known, rather than demanded, it's evidence" } else { "" };
            out.push(Advice { rule: "requirement_judged_before".to_string(), on: e, detail: format!("it's judged as if every choice is made before these are known: {}{nothing}", names.join(", ")) });
        }
        for (choice, knows) in &judged.knowing {
            out.push(Advice { rule: "requirement_judged_knowing".to_string(), on: e, detail: format!("it's judged with {choice} made knowing {}, for each way that could turn out, and each choice before it allowed only where {choice} can then meet it", knows.join(" and ")) });
        }
        if let Some(why) = &judged.unjudged {
            out.push(Advice { rule: "requirement_not_judged".to_string(), on: e, detail: format!("it rules nothing out, because it can't be judged: {why}") });
        }
        if judged.close {
            out.push(Advice { rule: "requirement_too_close_to_call".to_string(), on: e, detail: format!("how likely it's met was read from {} draws, and for some choice that's too close to how sure it has to be to call", chance::DRAWS) });
        }
    }
    out
}

/// What a choice lands on, in each world: a node, or undefined where there's nothing to land on.
/// Writing one in settles it.
#[cell]
pub struct Chosen(pub Worldly<Val>);

impl Chosen {
    /// Settled on this node.
    pub fn on(e: Entity) -> Self {
        Chosen(Worldly::plain(Val::Ref(e)))
    }

    /// The worlds where it lands on this node.
    pub fn lands_on(&self, e: Entity) -> Bool {
        self.0.0.holds(|v| *v == Val::Ref(e))
    }

    /// The one node it's settled on, if it's the same in every world.
    pub fn settled(&self) -> Option<Entity> {
        match self.0.0.cases().as_slice() {
            [(_, Val::Ref(e))] => Some(*e),
            _ => None,
        }
    }
}

/// A plain list of what a choice can land on, such as a puzzle's colours. Each is a node nobody wrote,
/// whose name and value are the option, and which every choice with that option shares.
#[cell("options")]
pub struct Options(pub Vec<String>);

/// Where a choice goes on a board, if the puzzle has one.
#[cell("at")]
pub struct At {
    pub row: u32,
    pub col: u32,
}

/// On a label edge: the name a formula on its source uses for its target. Several edges with one name
/// make a group.
#[cell]
pub struct Called(pub String);

/// A function of your own, written as a formula over its parameters.
#[cell("macro")]
pub struct Macro {
    pub params: Vec<String>,
    pub body: String,
}

/// A macro, with the macros it uses found. It's a cell so a macro calling itself is a loop in these
/// cells, which needs nothing special, and so a caller only ever reads one hop.
#[cell("macro_body")]
pub struct MacroBody {
    pub params: Vec<String>,
    pub body: Expr,
    pub uses: BTreeMap<String, MacroBody>,
}

/// What type a node's value is. A type doesn't depend on a world, or on any value that isn't typed
/// in, so this reruns far less often than the value does, and stops there when it comes out the same.
#[cell("value_type")]
pub struct ValueType(pub Type);

/// What the cells of compiled systems hold, for a formula reading them. Anything else is checked as
/// it runs.
pub(crate) fn component_type(cell: &str) -> Type {
    match cell {
        "need" | "actionable" | "blocked" | "done" | "dropped" => Type::Bool,
        "chosen" => Type::Ref,
        "estimate" | "start" | "finish" | "latest_start" | "latest_finish" | "slack" | "plan_end" | "weight" => Type::Number,
        "name" | "body" => Type::Text,
        _ => Type::Any,
    }
}

/// How far along its table `prev` and `next` are from a row.
fn along(name: &str) -> Option<i64> {
    match name {
        "prev" => Some(-1),
        "next" => Some(1),
        _ => None,
    }
}

/// How far along a path starting `prev.prev…` goes, and what it reads there.
fn steps_along(path: &[String]) -> (i64, &[String]) {
    let n = path.iter().take_while(|p| along(p).is_some()).count();
    (path[..n].iter().filter_map(|p| along(p)).sum(), &path[n..])
}

fn failed<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::Failed(message.into()))
}

// ---------- expanding: names to nodes, and functions to what they're defined as ----------

/// A part of an expanded formula, as an index into `Closed`.
type Id = usize;

/// A part of a formula with nothing left to look up. Its parts come earlier in the list, so a
/// formula can be as long as its groups are, and going through it needs no recursion.
#[derive(Debug)]
enum Part {
    Lit(Val),
    Read { node: Entity, component: String, fields: Vec<String> },
    Field(Id, Vec<String>),
    If(Id, Id, Id),
    Bin(BinOp, Id, Id),
    Not(Id),
    Neg(Id),
    Defined(Id),
    Has(Entity, String),
    Chose(Entity, Entity),
    Round(Id, Id),
    Floor(Id),
    Ceil(Id),
    /// `exp`, `ln` or `sqrt`.
    Math(chance::Un, Id),
    /// A standard draw: which of this formula's draws it is. No two parts are the same draw, which is
    /// what makes each place one is written a quantity of its own.
    Draw(Std, u32),
    /// A three-point estimate: a uniform draw, then the least, the likeliest and the most.
    Tri(Id, Id, Id, Id),
    /// Each whole number from one to another, equally.
    Integers(Id, Id, u32),
    /// True this often.
    Bernoulli(Id, u32),
    /// `chance(x)`, or `mean(x)`: over what's a matter of chance, given what's known.
    Expect { mean: bool, of: Id },
    /// One of these nodes, each as likely.
    OneOf(Vec<Entity>, u32),
    /// `what ~ family(args…)`: what was seen, and how likely each way things could be makes it. Where
    /// that's the same in every draw it's a chance of its own, written at `site`. See `docs/inference.md`.
    Seen { family: String, what: Id, args: Vec<Id>, site: u32 },
    /// `what ~ one_of(group)`.
    SeenOneOf { what: Id, nodes: Vec<Entity>, site: u32 },
}

impl Part {
    fn renumbered(self, to: &[Id]) -> Part {
        match self {
            Part::Field(a, f) => Part::Field(to[a], f),
            Part::If(a, b, c) => Part::If(to[a], to[b], to[c]),
            Part::Bin(op, a, b) => Part::Bin(op, to[a], to[b]),
            Part::Not(a) => Part::Not(to[a]),
            Part::Neg(a) => Part::Neg(to[a]),
            Part::Defined(a) => Part::Defined(to[a]),
            Part::Round(a, b) => Part::Round(to[a], to[b]),
            Part::Floor(a) => Part::Floor(to[a]),
            Part::Ceil(a) => Part::Ceil(to[a]),
            Part::Math(op, a) => Part::Math(op, to[a]),
            Part::Tri(u, a, b, c) => Part::Tri(to[u], to[a], to[b], to[c]),
            Part::Integers(a, b, site) => Part::Integers(to[a], to[b], site),
            Part::Bernoulli(p, site) => Part::Bernoulli(to[p], site),
            Part::Expect { mean, of } => Part::Expect { mean, of: to[of] },
            Part::Seen { family, what, args, site } => Part::Seen { family, what: to[what], args: args.into_iter().map(|a| to[a]).collect(), site },
            Part::SeenOneOf { what, nodes, site } => Part::SeenOneOf { what: to[what], nodes, site },
            other => other,
        }
    }

    fn parts(&self) -> Vec<Id> {
        match self {
            Part::Seen { what, args, .. } => std::iter::once(*what).chain(args.iter().copied()).collect(),
            Part::SeenOneOf { what, .. } => vec![*what],
            Part::Lit(_) | Part::Read { .. } | Part::Has(..) | Part::Chose(..) | Part::Draw(..) | Part::OneOf(..) => vec![],
            Part::Field(a, _) | Part::Not(a) | Part::Neg(a) | Part::Defined(a) | Part::Floor(a) | Part::Ceil(a) | Part::Math(_, a) | Part::Bernoulli(a, _) | Part::Expect { of: a, .. } => vec![*a],
            Part::Bin(_, a, b) | Part::Round(a, b) | Part::Integers(a, b, _) => vec![*a, *b],
            Part::If(a, b, c) => vec![*a, *b, *c],
            Part::Tri(u, a, b, c) => vec![*u, *a, *b, *c],
        }
    }
}

/// An expanded formula: only the parts it really uses, in order, so what each uses comes first. The
/// same part can be used many times, which is how an argument a macro uses twice is worked out once.
struct Closed(Vec<Part>);

#[derive(Clone)]
enum Binding {
    Node(Entity),
    Group(Vec<Entity>),
    Value(Id),
}

#[derive(Clone, Default)]
struct Scope<'m> {
    vars: HashMap<String, Binding>,
    /// For a node being gone over in an edge's direction, the edge it was reached by: `o.via`.
    via: HashMap<String, Entity>,
    /// In a macro: what it can call. It can't see anything else.
    macro_uses: Option<&'m BTreeMap<String, MacroBody>>,
}

impl Scope<'_> {
    fn is_macro(&self) -> bool {
        self.macro_uses.is_some()
    }
}

struct Expander<'c, 'a> {
    cx: &'c Cx<'a>,
    labels: BTreeMap<String, Vec<Entity>>,
    parts: RefCell<Vec<Part>>,
    steps: StdCell<usize>,
    /// How many draws have been written so far.
    draws: StdCell<u32>,
}

impl<'c, 'a> Expander<'c, 'a> {
    fn new(cx: &'c Cx<'a>) -> Result<Self, Error> {
        let mut labels: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
        // Names are what the node the formula's written on has labelled, which is a kind's for a
        // formula taken from one.
        for (to, edge) in cx.outgoing_from_origin(LABEL) {
            if let Some(Called(name)) = cx.get::<Called>(edge)? {
                labels.entry(name).or_default().push(to);
            }
        }
        Ok(Expander { cx, labels, parts: RefCell::new(Vec::new()), steps: StdCell::new(0), draws: StdCell::new(0) })
    }

    fn add(&self, p: Part) -> Id {
        let mut parts = self.parts.borrow_mut();
        parts.push(p);
        parts.len() - 1
    }

    fn lit(&self, v: Val) -> Id {
        self.add(Part::Lit(v))
    }

    /// The next draw this formula writes. Expanding is the same every time, so it's the same draw every
    /// time, and a formula that's written out once for each node of a group has one for each.
    fn site(&self) -> u32 {
        self.draws.set(self.draws.get() + 1);
        self.draws.get() - 1
    }

    fn bin(&self, op: BinOp, a: Id, b: Id) -> Id {
        self.add(Part::Bin(op, a, b))
    }

    /// `value` where `ok` holds, and undefined where it doesn't, as a spread below 0 is.
    fn only_if(&self, ok: Id, value: Id) -> Id {
        let undefined = self.lit(Val::Undefined);
        self.add(Part::If(ok, value, undefined))
    }

    /// `mean + sd * z`, for a standard draw `z`.
    fn normal(&self, mean: Id, sd: Id) -> Id {
        let z = self.add(Part::Draw(Std::Normal, self.site()));
        let value = self.bin(BinOp::Add, mean, self.bin(BinOp::Mul, sd, z));
        let zero = self.lit(Val::Num(0.0));
        self.only_if(self.bin(BinOp::Ge, sd, zero), value)
    }

    fn read(&self, node: Entity, path: &[String]) -> Id {
        // A row's key isn't a cell, but it's read the same way, as `q.row` or `prev.row`.
        if let [only] = path
            && only == "row"
        {
            return self.lit(match self.cx.row_key_of(node) {
                Some(Key::Num(n)) => Val::Num(n as f64),
                Some(Key::Text(t)) => Val::Text(t),
                None => Val::Undefined,
            });
        }
        self.add(match path {
            [] => Part::Read { node, component: ValueCell::NAME.to_string(), fields: vec![] },
            [component, fields @ ..] => Part::Read { node, component: component.clone(), fields: fields.to_vec() },
        })
    }

    /// The whole formula, with anything nothing ended up using left out.
    fn expand_all(self, e: &Expr) -> Result<Closed, Error> {
        let root = self.expand(e, &Scope::default())?;
        let parts = self.parts.into_inner();
        let mut used = vec![false; parts.len()];
        used[root] = true;
        for i in (0..parts.len()).rev() {
            if used[i] {
                parts[i].parts().into_iter().for_each(|p| used[p] = true);
            }
        }
        // Parts move down the list, so what refers to them has to follow.
        let mut moved = vec![0; parts.len()];
        let mut out = Vec::with_capacity(parts.len());
        for (i, part) in parts.into_iter().enumerate() {
            if used[i] {
                moved[i] = out.len();
                out.push(part.renumbered(&moved));
            }
        }
        Ok(Closed(out))
    }

    fn group(&self, path: &[String], scope: &Scope) -> Result<Vec<Entity>, Error> {
        Ok(self.members(path, scope)?.into_iter().map(|(n, _)| n).collect())
    }

    /// A group's nodes, each with the edge it was reached by, if it was reached along one.
    fn members(&self, path: &[String], scope: &Scope) -> Result<Vec<(Entity, Option<Entity>)>, Error> {
        let (root, rest) = path.split_first().expect("a path has a first name");
        let plain = |nodes: Vec<Entity>| nodes.into_iter().map(|n| (n, None)).collect::<Vec<_>>();
        if let Some(b) = scope.vars.get(root) {
            let start = match b {
                Binding::Node(n) => vec![(*n, None)],
                Binding::Group(g) => plain(g.clone()),
                Binding::Value(_) => return failed(format!("{root} isn't a group of nodes")),
            };
            return self.go_on(root, start, rest);
        }
        if matches!(root.as_str(), "in" | "out" | "with") {
            if scope.is_macro() {
                return failed(format!("a macro can only use its parameters, not {root}.…"));
            }
            let Some(component) = rest.first() else { return failed(format!("{root} needs to say which component, such as {root}.part_of")) };
            let along = |edges: Vec<(Entity, Entity)>| edges.into_iter().map(|(n, e)| (n, Some(e))).collect();
            let start = match root.as_str() {
                "in" => along(self.cx.incoming(component)),
                "out" => along(self.cx.outgoing(component)),
                _ => plain(self.cx.scan(component)),
            };
            return self.go_on(&format!("{root}.{component}"), start, &rest[1..]);
        }
        if scope.is_macro() {
            return failed(format!("a macro can only use its parameters, not {root}"));
        }
        if root == "rows" {
            return self.go_on(root, plain(self.cx.rows()), rest);
        }
        // From this node, as `self.in.realises`, which on a kind is each instance in turn.
        if root == "self" {
            return self.go_on(root, vec![(self.cx.id(), None)], rest);
        }
        if rest.first().is_some_and(|r| r == "rows") && !self.labels.contains_key(root) {
            // Another table's rows, by its name, as `Arrived.rows`.
            return self.go_on(&format!("{root}.rows"), plain(self.cx.rows_of(root)), &rest[1..]);
        }
        // No edges yet is fine for a group: a total with no lines is 0.
        self.go_on(root, plain(self.labels.get(root).cloned().unwrap_or_default()), rest)
    }

    /// Going on from a group along links, as `x.in.part_of` or `hub.out.relates.in.is_a` do: each step
    /// is `in` or `out` and a component, from every node reached so far. `in*` and `out*` keep going
    /// along it as far as it goes, so `kind.in*.is_a` is its instances, theirs, and so on, each once.
    /// Anything else after a group is a cell, which a group hasn't got.
    fn go_on(&self, said: &str, mut nodes: Vec<(Entity, Option<Entity>)>, mut rest: &[String]) -> Result<Vec<(Entity, Option<Entity>)>, Error> {
        let mut said = said.to_string();
        while let Some(step) = rest.first() {
            let way = step.as_str();
            if !matches!(way, "in" | "out" | "in*" | "out*") {
                return failed(format!("{said}.{step}: a group of nodes goes on only by in.… or out.…, so read a cell inside, as sum(x in {said}: x.{step})"));
            }
            let Some(component) = rest.get(1) else { return failed(format!("{said}.{way} needs to say which component, such as {said}.{way}.part_of")) };
            let along = |n: Entity| if way.starts_with("in") { self.cx.incoming_at(n, component) } else { self.cx.outgoing_at(n, component) };
            let mut next = Vec::new();
            if way.ends_with('*') {
                // Each node once, however many ways it's reached, and a loop ends.
                let mut seen = std::collections::BTreeSet::new();
                let mut level: Vec<Entity> = nodes.iter().map(|(n, _)| *n).collect();
                while let Some(n) = level.pop() {
                    for (m, e) in along(n) {
                        if seen.insert(m) {
                            next.push((m, Some(e)));
                            level.push(m);
                        }
                    }
                }
            } else {
                for (n, _) in &nodes {
                    next.extend(along(*n).into_iter().map(|(m, e)| (m, Some(e))));
                }
            }
            nodes = next;
            said = format!("{said}.{way}.{component}");
            rest = &rest[2..];
        }
        Ok(nodes)
    }

    /// On an edge, its source as `from` and its target as `to`.
    fn end(&self, root: &str) -> Option<Entity> {
        let (from, to) = self.cx.ends()?;
        match root {
            "from" => Some(from),
            "to" => Some(to),
            _ => None,
        }
    }

    fn node_arg(&self, e: &Expr, scope: &Scope) -> Result<Entity, Error> {
        match self.binding(e, scope)? {
            Binding::Node(n) => Ok(n),
            _ => failed("chose needs one decision and one of its options"),
        }
    }

    /// What an argument to a function is: a node or group if it reads as one, or a value.
    fn binding(&self, e: &Expr, scope: &Scope) -> Result<Binding, Error> {
        if let Expr::Path(path) = e {
            let root = path[0].as_str();
            if crate::expr::goes_along(path) {
                return Ok(Binding::Group(self.group(path, scope)?));
            }
            if let Some(b) = scope.vars.get(root) {
                if path.len() == 1 {
                    return Ok(b.clone());
                }
            } else if root == "self" && path.len() == 1 && !scope.is_macro() {
                return Ok(Binding::Node(self.cx.id()));
            } else if let Some(end) = self.end(root).filter(|_| path.len() == 1 && !scope.is_macro()) {
                return Ok(Binding::Node(end));
            } else if along(root).is_some() && !scope.is_macro() {
                let (by, rest) = steps_along(path);
                if rest.is_empty()
                    && let Some(n) = self.cx.row_along(by)
                {
                    return Ok(Binding::Node(n));
                }
            } else if matches!(root, "in" | "out" | "with" | "rows") || (path.len() == 2 && path[1] == "rows") {
                return Ok(Binding::Group(self.group(path, scope)?));
            } else if path.len() == 1 && !scope.is_macro() && root != "self" {
                let nodes = self.group(path, scope)?;
                return Ok(match nodes.as_slice() {
                    [one] => Binding::Node(*one),
                    _ => Binding::Group(nodes),
                });
            }
        }
        Ok(Binding::Value(self.expand(e, scope)?))
    }

    fn expand(&self, e: &Expr, scope: &Scope) -> Result<Id, Error> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MOST_STEPS {
            return failed("this formula expands to something too big to work out");
        }
        // Macros calling macros can go deep, though parts of a formula can't by themselves.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match e {
            Expr::Lit(v) => Ok(self.lit(v.clone())),
            Expr::Not(a) => {
                let a = self.expand(a, scope)?;
                Ok(self.add(Part::Not(a)))
            }
            Expr::Neg(a) => {
                let a = self.expand(a, scope)?;
                Ok(self.add(Part::Neg(a)))
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.expand(a, scope)?, self.expand(b, scope)?);
                Ok(self.add(Part::Bin(*op, a, b)))
            }
            Expr::Path(path) => self.path(path, scope),
            Expr::Row { table, key, path } => {
                if scope.is_macro() {
                    return failed(format!("a macro can only use its parameters, not {table}[…]"));
                }
                let key = self.key(key, scope)?;
                match self.cx.row_at(table, &key) {
                    Some(n) => Ok(self.read(n, path)),
                    None => failed(format!("there's no row {table}[{key}]")),
                }
            }
            Expr::Fold { func, var, group, body, start } => self.fold(func, var, group, body, start.as_deref(), scope),
            Expr::Call(name, args) => self.call(name, args, scope),
            Expr::Seen { what, family, args } => self.seen(what, family, args, scope),
        })
    }

    fn path(&self, path: &[String], scope: &Scope) -> Result<Id, Error> {
        let (root, rest) = path.split_first().expect("a path has a first name");
        if crate::expr::goes_along(path) {
            let said = path.join(".");
            return failed(format!("{said} is a group of nodes, so it needs something like count({said}) or sum(x in {said}: …)"));
        }
        match scope.vars.get(root) {
            // The edge a node was reached along, as `o.via.weight`.
            Some(Binding::Node(_)) if rest.first().is_some_and(|r| r == "via") => {
                return match scope.via.get(root) {
                    Some(edge) if rest.len() > 1 => Ok(self.read(*edge, &rest[1..])),
                    Some(_) => failed(format!("{root}.via is an edge, so it needs to say which cell, such as {root}.via.weight")),
                    None => failed(format!("{root} wasn't reached along an edge, so it has no via")),
                };
            }
            Some(Binding::Node(n)) => return Ok(self.read(*n, rest)),
            Some(Binding::Value(c)) if rest.is_empty() => return Ok(*c),
            Some(Binding::Value(c)) => return Ok(self.add(Part::Field(*c, rest.to_vec()))),
            Some(Binding::Group(g)) => return failed(format!("{root} is a group of {} nodes, so it needs something like sum({root})", g.len())),
            None => {}
        }
        if scope.is_macro() {
            return failed(format!("a macro can only use its parameters, not {root}"));
        }
        match root.as_str() {
            "self" if rest.is_empty() => failed("self needs to say which cell, such as self.need"),
            "self" => Ok(self.read(self.cx.id(), rest)),
            "whole" if rest.is_empty() => failed("whole needs to say which cell, such as whole.plan_end"),
            "whole" => Ok(self.read(Entity::whole(), rest)),
            "from" | "to" if self.end(root).is_some() && rest.is_empty() => failed(format!("{root} needs to say which cell, such as {root}.need")),
            "from" | "to" if self.end(root).is_some() => Ok(self.read(self.end(root).expect("checked above"), rest)),
            // Before the first row, or with a gap, there's nothing there.
            "prev" | "next" => {
                let (by, rest) = steps_along(path);
                Ok(match self.cx.row_along(by) {
                    Some(n) => self.read(n, rest),
                    None => self.lit(Val::Undefined),
                })
            }
            "row" if !rest.is_empty() => failed("row is which row this is, so it has no cells: prev.cost reads the row before"),
            "row" => Ok(self.lit(match self.cx.row_key() {
                Some(Key::Num(n)) => Val::Num(n as f64),
                Some(Key::Text(t)) => Val::Text(t),
                None => Val::Undefined,
            })),
            "in" | "out" | "with" | "rows" => failed(format!("{}.… is a group of nodes, so it needs something like sum({}.…)", root, root)),
            _ => match self.labels.get(root).map(Vec::as_slice) {
                None if rest.first().is_some_and(|r| r == "rows") => failed(format!("{root}.rows is a group of rows, so it needs something like count({root}.rows)")),
                None | Some([]) => failed(format!("no input called {root}")),
                Some([one]) => Ok(self.read(*one, rest)),
                Some(many) => failed(format!("{root} is {} nodes, so it needs something like sum({root})", many.len())),
            },
        }
    }

    /// Which row `Year[…]` means. It has to be known without looking at any world, so it's a number, a
    /// name, or worked out from `row` with plain arithmetic, as `Year[row - 1]` is.
    fn key(&self, e: &Expr, scope: &Scope) -> Result<Key, Error> {
        let number = |e: &Expr| -> Result<i64, Error> {
            match self.key(e, scope)? {
                Key::Num(n) => Ok(n),
                Key::Text(t) => failed(format!("{t} isn't a number, so it can't be added to")),
            }
        };
        Ok(match e {
            Expr::Lit(Val::Num(n)) if n.fract() == 0.0 && n.abs() < 1e15 => Key::Num(*n as i64),
            Expr::Lit(Val::Text(t)) => Key::Text(t.clone()),
            Expr::Path(p) if p.len() == 1 && p[0] == "row" => match self.cx.row_key() {
                Some(k) => k,
                None => return failed("row is only something on a row of a table"),
            },
            // The key of a row being gone over, as `Left[p.row]` in `count(p in Arrived.rows: …)`.
            Expr::Path(p) if p.len() == 2 && p[1] == "row" && matches!(scope.vars.get(&p[0]), Some(Binding::Node(_))) => {
                let Some(Binding::Node(n)) = scope.vars.get(&p[0]) else { unreachable!("matched above") };
                match self.cx.row_key_of(*n) {
                    Some(k) => k,
                    None => return failed(format!("{} isn't a row of a table, so it has no key", p[0])),
                }
            }
            Expr::Path(p) if p.len() == 1 => Key::Text(p[0].clone()),
            Expr::Neg(a) => Key::Num(-number(a)?),
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), a, b) => {
                let (a, b) = (number(a)?, number(b)?);
                Key::Num(match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    _ => a * b,
                })
            }
            _ => return failed("a row's key is a number, a name, or worked out from row, such as Year[3], Car[Hatchback], Year[row - 1] or Left[p.row]"),
        })
    }

    /// No two of these the same: every pair differs.
    fn distinct(&self, each: &[Id]) -> Id {
        let mut acc = self.lit(Val::Bool(true));
        for (i, a) in each.iter().enumerate() {
            for b in &each[i + 1..] {
                let differ = self.add(Part::Bin(BinOp::Ne, *a, *b));
                acc = self.add(Part::Bin(BinOp::And, acc, differ));
            }
        }
        acc
    }

    fn fold(&self, func: &str, var: &str, group: &[String], body: &Expr, start: Option<&Expr>, scope: &Scope) -> Result<Id, Error> {
        let members = self.members(group, scope)?;
        let nodes: Vec<Entity> = members.iter().map(|(n, _)| *n).collect();
        let via: HashMap<Entity, Entity> = members.iter().filter_map(|(n, e)| Some((*n, (*e)?))).collect();
        let copy = |n: Entity| {
            let mut inner = scope.clone();
            inner.vars.insert(var.to_string(), Binding::Node(n));
            match via.get(&n) {
                Some(edge) => inner.via.insert(var.to_string(), *edge),
                None => inner.via.remove(var),
            };
            self.expand(body, &inner)
        };
        let from = |unit: Val| start.map_or_else(|| Ok(self.lit(unit)), |s| self.expand(s, scope));
        let chain = |op: BinOp, unit: Val| -> Result<Id, Error> {
            let mut acc = from(unit)?;
            for n in &nodes {
                let next = copy(*n)?;
                acc = self.add(Part::Bin(op, acc, next));
            }
            Ok(acc)
        };
        match func {
            "sum" => chain(BinOp::Add, Val::Num(0.0)),
            "product" => chain(BinOp::Mul, Val::Num(1.0)),
            "all" => chain(BinOp::And, Val::Bool(true)),
            "any" => chain(BinOp::Or, Val::Bool(false)),
            "max" => chain(BinOp::Max, Val::Undefined),
            "min" => chain(BinOp::Min, Val::Undefined),
            // No two the same: every pair differs.
            "distinct" => {
                let each = nodes.iter().map(|n| copy(*n)).collect::<Result<Vec<_>, _>>()?;
                Ok(self.distinct(&each))
            }
            "count" => {
                let (one, zero) = (self.lit(Val::Num(1.0)), self.lit(Val::Num(0.0)));
                let mut acc = zero;
                for n in &nodes {
                    let this = copy(*n)?;
                    let counted = self.add(Part::If(this, one, zero));
                    acc = self.add(Part::Bin(BinOp::Add, acc, counted));
                }
                Ok(acc)
            }
            _ => unreachable!("the parser only makes folds of these"),
        }
    }

    /// `what ~ family(args…)`. The right side isn't drawn: it's the distribution what was seen is said to
    /// come from, so its arguments are read as they are.
    fn seen(&self, what: &Expr, family: &str, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        let what = self.expand(what, scope)?;
        if family == "one_of" {
            let [group] = args else { return failed(format!("one_of takes one group and was given {}", args.len())) };
            return match self.binding(group, scope)? {
                Binding::Group(nodes) => Ok(self.add(Part::SeenOneOf { what, nodes, site: self.site() })),
                _ => failed("one_of needs a group of nodes, such as the rows of a table: one_of(Runners.rows)"),
            };
        }
        let wants = match family {
            "exponential" | "bernoulli" => 1,
            "triangular" => 3,
            _ => 2,
        };
        if args.len() != wants {
            return failed(format!("{family} takes {wants} and was given {}", args.len()));
        }
        let args = args.iter().map(|a| self.expand(a, scope)).collect::<Result<Vec<_>, _>>()?;
        Ok(self.add(Part::Seen { family: family.to_string(), what, args, site: self.site() }))
    }

    fn call(&self, name: &str, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        if CORE.contains(&name) {
            return self.core(name, args, scope);
        }
        if let Some(body) = scope.macro_uses.and_then(|u| u.get(name)) {
            return self.call_macro(name, &body.params, &body.body, &body.uses, args, scope);
        }
        if let Some(m) = expr::library(name) {
            // The library isn't a macro of the kind a workspace writes: it can follow edges and scan, as
            // `is_leaf()` does, where a workspace's macro only has its parameters.
            let vars = self.bind(name, &m.params, args, scope)?;
            return self.expand(&m.body, &Scope { vars, ..Scope::default() });
        }
        if scope.is_macro() {
            return failed(format!("no function called {name} that this macro has a link to"));
        }
        match self.labels.get(name).map(Vec::as_slice) {
            None | Some([]) => failed(format!("no function called {name}")),
            Some([node]) => match self.cx.get::<MacroBody>(*node)? {
                Some(m) => self.call_macro(name, &m.params, &m.body, &m.uses, args, scope),
                None => failed(format!("{name} isn't a macro")),
            },
            Some(_) => failed(format!("{name} is several nodes, so it can't be called")),
        }
    }

    fn bind(&self, name: &str, params: &[String], args: &[Expr], scope: &Scope) -> Result<HashMap<String, Binding>, Error> {
        if params.len() != args.len() {
            return failed(format!("{name} takes {} and was given {}", params.len(), args.len()));
        }
        params.iter().zip(args).map(|(p, a)| Ok((p.clone(), self.binding(a, scope)?))).collect()
    }

    fn call_macro(&self, name: &str, params: &[String], body: &Expr, uses: &BTreeMap<String, MacroBody>, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        let vars = self.bind(name, params, args, scope)?;
        self.expand(body, &Scope { vars, macro_uses: Some(uses), ..Scope::default() })
    }

    fn core(&self, name: &str, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        let wants = |n: usize| if args.len() == n { Ok(()) } else { failed(format!("{name} takes {n} and was given {}", args.len())) };
        match name {
            "if" => {
                wants(3)?;
                let (c, a, b) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?, self.expand(&args[2], scope)?);
                Ok(self.add(Part::If(c, a, b)))
            }
            "defined" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Defined(a)))
            }
            "has" => {
                wants(1)?;
                let Expr::Path(path) = &args[0] else { return failed("has needs a node and a cell, such as has(self.estimate)") };
                let [root, component] = path.as_slice() else { return failed("has needs a node and a cell, such as has(self.estimate)") };
                match self.binding(&Expr::Path(vec![root.clone()]), scope)? {
                    Binding::Node(n) => Ok(self.add(Part::Has(n, component.clone()))),
                    _ => failed(format!("has needs one node, and {root} isn't one")),
                }
            }
            "max" | "min" => {
                wants(2)?;
                let (a, b) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                Ok(self.add(Part::Bin(if name == "max" { BinOp::Max } else { BinOp::Min }, a, b)))
            }
            "distinct" => {
                let each = args.iter().map(|a| self.expand(a, scope)).collect::<Result<Vec<_>, _>>()?;
                Ok(self.distinct(&each))
            }
            "chose" => {
                wants(2)?;
                let (d, o) = (self.node_arg(&args[0], scope)?, self.node_arg(&args[1], scope)?);
                Ok(self.add(Part::Chose(d, o)))
            }
            "round" => match args {
                [x] => {
                    let (x, zero) = (self.expand(x, scope)?, self.lit(Val::Num(0.0)));
                    Ok(self.add(Part::Round(x, zero)))
                }
                [x, places] => {
                    let (x, places) = (self.expand(x, scope)?, self.expand(places, scope)?);
                    Ok(self.add(Part::Round(x, places)))
                }
                _ => failed(format!("round takes 1 or 2 and was given {}", args.len())),
            },
            "floor" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Floor(a)))
            }
            "ceil" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Ceil(a)))
            }
            // What's a matter of chance taken together, given what's known. Never over a choice of yours.
            "chance" | "mean" => {
                wants(1)?;
                let of = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Expect { mean: name == "mean", of }))
            }
            "exp" | "ln" | "sqrt" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                let op = match name {
                    "exp" => chance::Un::Exp,
                    "ln" => chance::Un::Ln,
                    _ => chance::Un::Sqrt,
                };
                Ok(self.add(Part::Math(op, a)))
            }
            // What's uncertain. Each is written in terms of a standard draw, so its arguments can be
            // anything a number can, and it's undefined where they make no sense.
            "normal" | "lognormal" => {
                wants(2)?;
                let (mean, sd) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                let value = self.normal(mean, sd);
                Ok(if name == "normal" { value } else { self.add(Part::Math(chance::Un::Exp, value)) })
            }
            "between" => {
                wants(2)?;
                let (low, high) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                // Nine times in ten a bell curve is within 1.645 spreads of its middle.
                let (two, wide) = (self.lit(Val::Num(2.0)), self.lit(Val::Num(2.0 * 1.6448536269514722)));
                let mean = self.bin(BinOp::Div, self.bin(BinOp::Add, low, high), two);
                let sd = self.bin(BinOp::Div, self.bin(BinOp::Sub, high, low), wide);
                Ok(self.normal(mean, sd))
            }
            "uniform" => {
                wants(2)?;
                let (low, high) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                let u = self.add(Part::Draw(Std::Uniform, self.site()));
                let value = self.bin(BinOp::Add, low, self.bin(BinOp::Mul, self.bin(BinOp::Sub, high, low), u));
                Ok(self.only_if(self.bin(BinOp::Le, low, high), value))
            }
            "exponential" => {
                wants(1)?;
                let mean = self.expand(&args[0], scope)?;
                let wait = self.add(Part::Draw(Std::Exponential, self.site()));
                let zero = self.lit(Val::Num(0.0));
                Ok(self.only_if(self.bin(BinOp::Gt, mean, zero), self.bin(BinOp::Mul, mean, wait)))
            }
            "triangular" => {
                wants(3)?;
                let (low, likely, high) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?, self.expand(&args[2], scope)?);
                let u = self.add(Part::Draw(Std::Uniform, self.site()));
                Ok(self.add(Part::Tri(u, low, likely, high)))
            }
            "integers" => {
                wants(2)?;
                let (low, high) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                Ok(self.add(Part::Integers(low, high, self.site())))
            }
            "bernoulli" => {
                wants(1)?;
                let p = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Bernoulli(p, self.site())))
            }
            "one_of" => {
                wants(1)?;
                match self.binding(&args[0], scope)? {
                    Binding::Group(rows) => Ok(self.add(Part::OneOf(rows, self.site()))),
                    Binding::Node(_) | Binding::Value(_) => failed("one_of needs a group of nodes, such as the rows of a table: one_of(Runners.rows)"),
                }
            }
            _ => unreachable!("every name in CORE is here"),
        }
    }
}

// ---------- checking: before any world is looked at ----------

fn drill(mut t: Type, fields: &[String]) -> Type {
    for f in fields {
        t = match t {
            Type::Record(of) => of.get(f).cloned().unwrap_or(Type::Undefined),
            _ => Type::Any,
        };
    }
    t
}

fn math_name(op: chance::Un) -> &'static str {
    match op {
        chance::Un::Exp => "exp",
        chance::Un::Ln => "ln",
        _ => "sqrt",
    }
}

fn numberish(t: &Type) -> bool {
    matches!(t, Type::Number | Type::Any | Type::Undefined)
}

fn boolish(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Any | Type::Undefined)
}

/// The type of a formula, without looking at any world.
fn check(cx: &Cx, closed: &Closed) -> Result<Type, Error> {
    let mut types: Vec<Type> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let t = type_of(cx, part, &types)?;
        types.push(t);
    }
    Ok(types.pop().expect("a formula has something in it"))
}

fn type_of(cx: &Cx, part: &Part, types: &[Type]) -> Result<Type, Error> {
    match part {
        Part::Lit(v) => Ok(Type::of(v)),
        Part::Read { node, component, fields } => {
            let t = if component == ValueCell::NAME { cx.get::<ValueType>(*node)?.map_or(Type::Undefined, |t| t.0) } else { component_type(component) };
            Ok(drill(t, fields))
        }
        Part::Field(of, fields) => Ok(drill(types[*of].clone(), fields)),
        Part::If(cond, a, b) => {
            if !boolish(&types[*cond]) {
                return failed(format!("if needs true or false to decide on, not {}", types[*cond].word()));
            }
            let (a, b) = (&types[*a], &types[*b]);
            a.join(b).ok_or_else(|| Error::Failed(format!("the two sides of if have to be the same type, not {} and {}", a.word(), b.word())))
        }
        Part::Not(a) => {
            let a = &types[*a];
            if boolish(a) { Ok(Type::Bool) } else { failed(format!("! needs true or false, not {}", a.word())) }
        }
        Part::Neg(a) => {
            let a = &types[*a];
            if numberish(a) { Ok(Type::Number) } else { failed(format!("- needs a number, not {}", a.word())) }
        }
        Part::Defined(_) | Part::Has(..) | Part::Chose(..) => Ok(Type::Bool),
        Part::Round(x, places) => {
            let (x, places) = (&types[*x], &types[*places]);
            if numberish(x) && numberish(places) { Ok(Type::Number) } else { failed(format!("round needs numbers, not {}", x.word())) }
        }
        Part::Floor(x) | Part::Ceil(x) => {
            let x = &types[*x];
            if numberish(x) { Ok(Type::Number) } else { failed(format!("rounding needs a number, not {}", x.word())) }
        }
        Part::Math(op, x) => {
            let x = &types[*x];
            if numberish(x) { Ok(Type::Number) } else { failed(format!("{} needs a number, not {}", math_name(*op), x.word())) }
        }
        Part::Draw(..) => Ok(Type::Number),
        Part::OneOf(..) => Ok(Type::Ref),
        Part::Seen { family, what, args, .. } => {
            let what = &types[*what];
            let fits = if family == "bernoulli" { boolish(what) } else { numberish(what) };
            if !fits {
                return failed(format!("what's seen to be drawn from {family} is {}, not {}", if family == "bernoulli" { "true or false" } else { "a number" }, what.word()));
            }
            match args.iter().find(|a| !numberish(&types[**a])) {
                None => Ok(Type::Seen),
                Some(a) => failed(format!("{family} needs numbers, not {}", types[*a].word())),
            }
        }
        Part::SeenOneOf { what, .. } => match &types[*what] {
            Type::Ref | Type::Any | Type::Undefined => Ok(Type::Seen),
            other => failed(format!("what's seen to be one of a group is a node, not {}", other.word())),
        },
        Part::Tri(_, a, b, c) => match [a, b, c].into_iter().find(|x| !numberish(&types[**x])) {
            None => Ok(Type::Number),
            Some(x) => failed(format!("triangular needs numbers, not {}", types[*x].word())),
        },
        Part::Integers(a, b, _) => match [a, b].into_iter().find(|x| !numberish(&types[**x])) {
            None => Ok(Type::Number),
            Some(x) => failed(format!("integers needs numbers, not {}", types[*x].word())),
        },
        Part::Expect { mean: false, of } => {
            let of = &types[*of];
            if boolish(of) { Ok(Type::Number) } else { failed(format!("chance needs true or false, not {}", of.word())) }
        }
        Part::Expect { mean: true, of } => {
            let of = &types[*of];
            if numberish(of) { Ok(Type::Number) } else { failed(format!("mean needs a number, not {}", of.word())) }
        }
        Part::Bernoulli(p, _) => {
            let p = &types[*p];
            if numberish(p) { Ok(Type::Bool) } else { failed(format!("bernoulli needs a chance, not {}", p.word())) }
        }
        Part::Bin(op, a, b) => {
            let (ta, tb) = (&types[*a], &types[*b]);
            let sym = op.symbol();
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Max | BinOp::Min => {
                    if numberish(ta) && numberish(tb) { Ok(Type::Number) } else { failed(format!("{sym} needs numbers, not {} and {}", ta.word(), tb.word())) }
                }
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    if numberish(ta) && numberish(tb) { Ok(Type::Bool) } else { failed(format!("{sym} needs numbers, not {} and {}", ta.word(), tb.word())) }
                }
                // Two things seen are both seen, and something seen with a truth is seen where that holds.
                BinOp::And if (*ta == Type::Seen || boolish(ta)) && (*tb == Type::Seen || boolish(tb)) && (*ta == Type::Seen || *tb == Type::Seen) => Ok(Type::Seen),
                BinOp::Or if *ta == Type::Seen || *tb == Type::Seen => failed("|| can't say one of two things was seen and not which: what's seen goes together with &&"),
                BinOp::And | BinOp::Or => {
                    if boolish(ta) && boolish(tb) { Ok(Type::Bool) } else { failed(format!("{sym} needs true or false, not {} and {}", ta.word(), tb.word())) }
                }
                BinOp::Eq | BinOp::Ne if *ta == Type::Seen || *tb == Type::Seen => failed(format!("{sym} can't compare something seen")),
                BinOp::Eq | BinOp::Ne => {
                    if ta.join(tb).is_none() {
                        return failed(format!("{sym} can't compare {} with {}", ta.word(), tb.word()));
                    }
                    Ok(Type::Bool)
                }
            }
        }
    }
}

// ---------- evaluating: in every world at once ----------

/// A value across worlds. True-or-false stays as diagrams, since listing them as cases would throw
/// away what makes them cheap to combine.
#[derive(Clone)]
enum Ev {
    /// The worlds where it's true, and the worlds where it's undefined. Elsewhere it's false.
    Truth { yes: Bool, undefined: Bool },
    Data(Cond<Val>),
}

fn pure(v: Val) -> Cond<Val> {
    Space::global().pure(v)
}

fn no_truth(found: &Val) -> Error {
    Error::Failed(format!("expected true or false, not {}", Type::of(found).word()))
}

impl Ev {
    fn truth(&self) -> Result<(Bool, Bool), Error> {
        match self {
            Ev::Truth { yes, undefined } => Ok((yes.clone(), undefined.clone())),
            Ev::Data(c) => {
                let cases = c.cases();
                if let Some((_, bad)) = cases.iter().find(|(_, v)| !matches!(v, Val::Bool(_) | Val::Undefined)) {
                    return Err(no_truth(bad));
                }
                Ok((c.holds(|v| *v == Val::Bool(true)), c.holds(|v| *v == Val::Undefined)))
            }
        }
    }

    fn data(&self) -> Cond<Val> {
        match self {
            Ev::Data(c) => c.clone(),
            Ev::Truth { yes, undefined } => Cond::when(yes, &pure(Val::Bool(true)), &Cond::when(undefined, &pure(Val::Undefined), &pure(Val::Bool(false)))),
        }
    }
}

fn zip(a: &Cond<Val>, b: &Cond<Val>, f: impl Fn(&Val, &Val) -> Val) -> Result<Cond<Val>, Error> {
    if a.len().saturating_mul(b.len()) > MOST_PAIRS {
        return too_many(a, b);
    }
    let out = a.zip(b, f);
    if out.len() > MOST_CASES {
        return too_many(a, b);
    }
    Ok(out)
}

fn too_many<T>(a: &Cond<Val>, b: &Cond<Val>) -> Result<T, Error> {
    let mut choices: Vec<String> = a.vars().into_iter().chain(b.vars()).map(|v| v.name()).collect();
    choices.sort();
    choices.dedup();
    let shown: Vec<String> = choices.iter().take(8).cloned().collect();
    let more = if choices.len() > 8 { format!(", and {} more", choices.len() - 8) } else { String::new() };
    failed(format!("this has more than {MOST_CASES} different values across worlds, depending on {}{more}", shown.join(", ")))
}

/// A value as the table of uncertain expressions takes one, if it's a number or a truth.
fn term(v: &Val) -> Option<Term> {
    Some(match v {
        Val::Undefined => Term::Undefined,
        Val::Num(n) => Term::Num(*n),
        Val::Bool(b) => Term::Flag(*b),
        Val::Rand(r) => Term::Rand(*r),
        _ => return None,
    })
}

fn val(t: Term) -> Val {
    match t {
        Term::Undefined => Val::Undefined,
        Term::Num(n) => Val::num(n),
        Term::Flag(b) => Val::Bool(b),
        Term::Rand(r) => Val::Rand(r),
    }
}

fn is_rand(v: &Val) -> bool {
    matches!(v, Val::Rand(_))
}

fn number_like(v: &Val) -> bool {
    match v {
        Val::Undefined | Val::Num(_) => true,
        Val::Rand(r) => !r.is_truth(),
        _ => false,
    }
}

fn truth_like(v: &Val) -> bool {
    match v {
        Val::Undefined | Val::Bool(_) => true,
        Val::Rand(r) => r.is_truth(),
        _ => false,
    }
}

fn any_rand(c: &Cond<Val>) -> bool {
    c.cases().iter().any(|(_, v)| is_rand(v))
}

/// Something seen whose likelihood depends on draws is in it somewhere.
fn any_likely(e: &Ev) -> bool {
    matches!(e, Ev::Data(c) if c.cases().iter().any(|(_, v)| matches!(v, Val::Likely(_))))
}

/// Something uncertain is in it somewhere, so it can't be held as diagrams alone.
fn uncertain(e: &Ev) -> bool {
    matches!(e, Ev::Data(c) if any_rand(c))
}

/// Two values in every world at once, as a pair.
fn pairs(a: &Cond<Val>, b: &Cond<Val>) -> Result<Cond<(Val, Val)>, Error> {
    if a.len().saturating_mul(b.len()) > MOST_PAIRS {
        return too_many(a, b);
    }
    Ok(a.zip(b, |x, y| (x.clone(), y.clone())))
}

/// A number, if it's a plain one or an estimate of one, with how far out it could be and from how many draws.
fn estimated(v: &Val) -> Option<(f64, Option<f64>, usize, Option<Given>)> {
    match v {
        Val::Num(n) => Some((*n, Some(0.0), 0, None)),
        Val::Estimate(e) => Some((e.value, e.error, e.draws, e.given)),
        _ => None,
    }
}

/// `op` on two numbers where one or both were read from draws. Adding and taking away add how far out
/// each could be, which is never less than it is, and so does scaling by a plain number. Anything else
/// still says it's an estimate, and no longer how far out. A comparison is only true or false where the
/// two are further apart than they could be out by, and otherwise undefined, since it can't be called.
fn estimate_op(a: &Val, b: &Val, op: chance::Op, f: &dyn Fn(f64, f64) -> Val) -> Val {
    let (Some((x, ex, dx, wx)), Some((y, ey, dy, wy))) = (estimated(a), estimated(b)) else { return Val::Undefined };
    let (draws, given) = (dx.max(dy), wx.or(wy));
    let error = match op {
        chance::Op::Add | chance::Op::Sub => ex.zip(ey).map(|(p, q)| p + q),
        chance::Op::Mul if ey == Some(0.0) => ex.map(|e| e * y.abs()),
        chance::Op::Mul if ex == Some(0.0) => ey.map(|e| e * x.abs()),
        chance::Op::Div if ey == Some(0.0) && y != 0.0 => ex.map(|e| e / y.abs()),
        chance::Op::Lt | chance::Op::Le | chance::Op::Eq | chance::Op::Ne => {
            let apart = ex.zip(ey).map(|(p, q)| p + q);
            return match apart {
                Some(e) if (x - y).abs() > CLOSE * e => f(x, y),
                _ => Val::Undefined,
            };
        }
        _ => None,
    };
    match f(x, y) {
        Val::Num(v) => Val::Estimate(Estimate { value: v, error, draws, given }),
        other => other,
    }
}

/// `f` on two numbers, undefined if either is, and `wrong` set if anything else turns up, which only
/// a value of unknown type can get as far as here. If either is uncertain, it's `op` on them as
/// expressions, which means the same in every draw. If either was read from draws, so is what comes out.
fn numbers(a: &Val, b: &Val, op: chance::Op, f: impl Fn(f64, f64) -> Val, wrong: &StdCell<bool>) -> Val {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => f(*x, *y),
        (Val::Estimate(_), Val::Num(_) | Val::Estimate(_)) | (Val::Num(_), Val::Estimate(_)) => estimate_op(a, b, op, &f),
        (Val::Estimate(_), Val::Undefined) | (Val::Undefined, Val::Estimate(_)) => Val::Undefined,
        (Val::Estimate(_), _) | (_, Val::Estimate(_)) => {
            // Something read from draws together with something uncertain would need the same draws.
            wrong.set(true);
            Val::Undefined
        }
        _ if !number_like(a) || !number_like(b) => {
            wrong.set(true);
            Val::Undefined
        }
        (Val::Undefined, _) | (_, Val::Undefined) => Val::Undefined,
        _ => val(chance::bin(op, &term(a).expect("a number"), &term(b).expect("a number"))),
    }
}

/// The same, for one number.
fn number(a: &Val, op: chance::Un, f: impl Fn(f64) -> f64, wrong: &StdCell<bool>) -> Val {
    match a {
        Val::Num(x) => Val::num(f(*x)),
        Val::Undefined => Val::Undefined,
        Val::Rand(r) if !r.is_truth() => val(chance::un(op, &Term::Rand(*r))),
        Val::Estimate(e) => match Val::num(f(e.value)) {
            Val::Num(v) => Val::Estimate(Estimate { value: v, error: if op == chance::Un::Neg { e.error } else { None }, draws: e.draws, given: e.given }),
            other => other,
        },
        _ => {
            wrong.set(true);
            Val::Undefined
        }
    }
}

/// What a draw is called: the node and cell it's written for, and which of that formula's draws it is.
/// The `~` is what says it's a matter of chance, where a choice is called by its node alone.
fn draw_name(cx: &Cx, site: u32) -> String {
    format!("~{}/{}#{site}", cx.id(), cx.cell())
}

/// The most whole numbers `integers` lists out as a variable of their own. Past this they're drawn.
const MOST_LISTED: f64 = 1000.0;

/// Each whole number from one to another, equally. A short list is a variable, as an uncertain decision
/// is, so what's worked out from it is exact. A long one, or one whose ends are uncertain, is drawn.
fn integers(cx: &Cx, low: &Cond<Val>, high: &Cond<Val>, site: u32) -> Result<Cond<Val>, Error> {
    let name = draw_name(cx, site);
    let ends = pairs(low, high)?;
    let several = ends.len() > 1;
    let mut out = pure(Val::Undefined);
    for (when, (low, high)) in ends.cases() {
        let here = match (low, high) {
            (Val::Num(a), Val::Num(b)) if a.fract() != 0.0 || b.fract() != 0.0 || a > b => pure(Val::Undefined),
            (Val::Num(a), Val::Num(b)) if b - a < MOST_LISTED => {
                let options: Vec<(String, f64, bool)> = (0..=(b - a) as i64).map(|i| ((a + i as f64).to_string(), 1.0, false)).collect();
                // The same draw with other ends is another variable, so it says which it is, and where it's
                // the one in use, so it can be shown as one draw.
                let var = Space::global().var(&if several { format!("{name}[{a}..{b}]") } else { name.clone() }, &options);
                if several {
                    var.in_group(&name, &when);
                }
                var.value().map(|o| Val::num(o.parse().expect("written as a number just above")))
            }
            (low, high) if number_like(low) && number_like(high) => {
                let u = Term::Rand(chance::draw(Std::Uniform, &name));
                let (low, high) = (term(low).expect("a number"), term(high).expect("a number"));
                let width = chance::bin(chance::Op::Add, &chance::bin(chance::Op::Sub, &high, &low), &Term::Num(1.0));
                let value = chance::un(chance::Un::Floor, &chance::bin(chance::Op::Add, &low, &chance::bin(chance::Op::Mul, &width, &u)));
                // Only whole ends, the right way round, mean anything.
                let whole = |x: &Term| chance::bin(chance::Op::Eq, x, &chance::un(chance::Un::Floor, x));
                let ok = chance::bin(chance::Op::And, &chance::bin(chance::Op::Le, &low, &high), &chance::bin(chance::Op::And, &whole(&low), &whole(&high)));
                pure(val(chance::pick(&ok, &value, &Term::Undefined)))
            }
            _ => return failed("integers needs numbers"),
        };
        out = Cond::when(&when, &here, &out);
    }
    Ok(out)
}

/// True this often. A plain chance is a variable with two outcomes, so it's exact. An uncertain one is
/// a draw falling under it.
fn bernoulli(cx: &Cx, p: &Cond<Val>, site: u32) -> Result<Cond<Val>, Error> {
    bernoulli_called(&draw_name(cx, site), p)
}

/// The same, as a variable with this name.
fn bernoulli_called(name: &str, p: &Cond<Val>) -> Result<Cond<Val>, Error> {
    let name = name.to_string();
    let several = p.len() > 1;
    let mut out = pure(Val::Undefined);
    for (when, p) in p.cases() {
        let here = match p {
            Val::Undefined => pure(Val::Undefined),
            Val::Num(p) if !(0.0..=1.0).contains(p) => pure(Val::Undefined),
            Val::Num(p) if *p == 0.0 || *p == 1.0 => pure(Val::Bool(*p == 1.0)),
            Val::Num(p) => {
                let var_name = if several { format!("{name}[{p}]") } else { name.clone() };
                let options = [("true".to_string(), *p, false), ("false".to_string(), 1.0 - p, false)];
                let var = Space::global().var(&var_name, &options);
                if several {
                    var.in_group(&name, &when);
                }
                var.value().map(|o| Val::Bool(o == "true"))
            }
            Val::Rand(r) if !r.is_truth() => {
                let (u, p) = (Term::Rand(chance::draw(Std::Uniform, &name)), Term::Rand(*r));
                let ok = chance::bin(chance::Op::And, &chance::bin(chance::Op::Le, &Term::Num(0.0), &p), &chance::bin(chance::Op::Le, &p, &Term::Num(1.0)));
                pure(val(chance::pick(&ok, &chance::bin(chance::Op::Lt, &u, &p), &Term::Undefined)))
            }
            _ => return failed("bernoulli needs a chance: a number from 0 to 1"),
        };
        out = Cond::when(&when, &here, &out);
    }
    Ok(out)
}

/// A formula's value across worlds.
fn evaluate(cx: &Cx, closed: &Closed) -> Result<Ev, Error> {
    let mut values: Vec<Ev> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let v = value_of(cx, part, &values)?;
        values.push(v);
    }
    Ok(values.pop().expect("a formula has something in it"))
}

fn read_cell(cx: &Cx, node: Entity, component: &str, fields: &[String]) -> Result<Ev, Error> {
    let whole = match component {
        ValueCell::NAME => match cx.get::<ValueCell>(node)? {
            Some(v) => Ev::Data(v.0.0),
            None => Ev::Data(pure(Val::Undefined)),
        },
        Chosen::NAME => match cx.get::<Chosen>(node)? {
            Some(c) => Ev::Data(c.0.0),
            None => Ev::Data(pure(Val::Undefined)),
        },
        // The cells formulas read most, without going through JSON.
        Need::NAME => match cx.get::<Need>(node)? {
            Some(n) => Ev::Truth { yes: n.0, undefined: Space::global().never() },
            None => Ev::Data(pure(Val::Undefined)),
        },
        Done::NAME => flag_cell(cx.get::<Done>(node)?.map(|c| c.0)),
        Dropped::NAME => flag_cell(cx.get::<Dropped>(node)?.map(|c| c.0)),
        crate::plan::Estimate::NAME => number_cell(cx.get::<crate::plan::Estimate>(node)?.map(|c| c.0)),
        Start::NAME => number_cell(cx.get::<Start>(node)?.map(|c| c.0)),
        Finish::NAME => number_cell(cx.get::<Finish>(node)?.map(|c| c.0)),
        LatestStart::NAME => number_cell(cx.get::<LatestStart>(node)?.map(|c| c.0)),
        LatestFinish::NAME => number_cell(cx.get::<LatestFinish>(node)?.map(|c| c.0)),
        PlanEnd::NAME => number_cell(cx.get::<PlanEnd>(node)?.map(|c| c.0)),
        _ => match cx.get_value(node, component)? {
            None => Ev::Data(pure(Val::Undefined)),
            Some(v) => from_json(v).map_err(|detail| Error::Malformed { cell: (node, component.to_string()), detail })?,
        },
    };
    if fields.is_empty() {
        return Ok(whole);
    }
    Ok(Ev::Data(field_of(cx, &whole.data(), fields)?))
}

fn flag_cell(c: Option<Worldly<Option<bool>>>) -> Ev {
    match c {
        Some(c) => Ev::Truth { yes: c.0.holds(|v| *v == Some(true)), undefined: c.0.holds(Option::is_none) },
        None => Ev::Data(pure(Val::Undefined)),
    }
}

fn number_cell(c: Option<Worldly<Option<Num>>>) -> Ev {
    match c {
        Some(c) => Ev::Data(c.0.map(|d| match d {
            None => Val::Undefined,
            Some(Num::Plain(n)) => Val::num(*n),
            Some(Num::Rand(r)) => Val::Rand(*r),
        })),
        None => Ev::Data(pure(Val::Undefined)),
    }
}

fn value_of(cx: &Cx, part: &Part, values: &[Ev]) -> Result<Ev, Error> {
    let space = Space::global();
    Ok(match part {
        Part::Lit(v) => Ev::Data(pure(v.clone())),
        Part::Read { node, component, fields } => read_cell(cx, *node, component, fields)?,
        Part::Field(of, fields) => Ev::Data(field_of(cx, &values[*of].data(), fields)?),
        // Whether something uncertain is defined can differ from draw to draw.
        Part::Defined(a) if uncertain(&values[*a]) => Ev::Data(values[*a].data().map(|v| match v {
            Val::Undefined => Val::Bool(false),
            Val::Rand(r) => val(chance::un(chance::Un::Defined, &Term::Rand(*r))),
            _ => Val::Bool(true),
        })),
        Part::Defined(a) => {
            let undefined = match &values[*a] {
                Ev::Truth { undefined, .. } => undefined.clone(),
                Ev::Data(d) => d.holds(|v| *v == Val::Undefined),
            };
            Ev::Truth { yes: !&undefined, undefined: space.never() }
        }
        // A component, if there's one by that name, and otherwise whether there's anything in that cell.
        Part::Has(node, name) => {
            let has = if cx.registry().component_def(name).is_some() { cx.has(*node, name) } else { cx.get_value(*node, name)?.is_some() };
            Ev::Truth { yes: space.truth(has), undefined: space.never() }
        }
        Part::Chose(choice, option) => {
            let Some(chosen) = cx.get::<Chosen>(*choice)? else { return failed("chose needs a choice") };
            Ev::Truth { yes: chosen.lands_on(*option), undefined: space.never() }
        }
        Part::Not(a) if uncertain(&values[*a]) => {
            let wrong = StdCell::new(false);
            let out = values[*a].data().map(|v| match v {
                Val::Bool(b) => Val::Bool(!b),
                Val::Undefined => Val::Undefined,
                Val::Rand(r) if r.is_truth() => val(chance::un(chance::Un::Not, &Term::Rand(*r))),
                _ => {
                    wrong.set(true);
                    Val::Undefined
                }
            });
            if wrong.get() {
                return failed("! needs true or false");
            }
            Ev::Data(out)
        }
        Part::Not(a) => {
            let (yes, undefined) = values[*a].truth()?;
            Ev::Truth { yes: &!&yes & &!&undefined, undefined }
        }
        Part::Neg(a) => {
            let wrong = StdCell::new(false);
            let out = values[*a].data().map(|v| numbers(&Val::Num(0.0), v, chance::Op::Sub, |z, x| Val::num(z - x), &wrong));
            if wrong.get() {
                return failed("- needs a number");
            }
            Ev::Data(out)
        }
        // Which side it is differs from draw to draw, so it's both, with the condition deciding in each.
        Part::If(cond, a, b) if uncertain(&values[*cond]) => {
            let sides = pairs(&values[*a].data(), &values[*b].data())?;
            let cond = values[*cond].data();
            if cond.len().saturating_mul(sides.len()) > MOST_PAIRS {
                return failed(format!("this has more than {MOST_CASES} different values across worlds"));
            }
            let wrong = StdCell::new(None);
            let out = cond.zip(&sides, |c, (x, y)| match c {
                Val::Bool(true) => x.clone(),
                Val::Bool(false) => y.clone(),
                Val::Undefined => Val::Undefined,
                Val::Rand(r) if r.is_truth() => match (term(x), term(y)) {
                    (Some(x), Some(y)) => val(chance::pick(&Term::Rand(*r), &x, &y)),
                    _ => {
                        wrong.set(Some("an uncertain condition can only choose between numbers, or between truths, so far"));
                        Val::Undefined
                    }
                },
                _ => {
                    wrong.set(Some("if needs true or false to decide on"));
                    Val::Undefined
                }
            });
            if let Some(why) = wrong.get() {
                return failed(why);
            }
            Ev::Data(out)
        }
        Part::If(cond, a, b) => {
            let (yes, undefined) = values[*cond].truth()?;
            let no = &!&yes & &!&undefined;
            if let (Ev::Truth { yes: ay, undefined: au }, Ev::Truth { yes: by, undefined: bu }) = (&values[*a], &values[*b]) {
                Ev::Truth { yes: &(&yes & ay) | &(&no & by), undefined: &undefined | &(&(&yes & au) | &(&no & bu)) }
            } else {
                Ev::Data(Cond::when(&undefined, &pure(Val::Undefined), &Cond::when(&yes, &values[*a].data(), &values[*b].data())))
            }
        }
        Part::Round(x, places) => {
            let wrong = StdCell::new(false);
            let out = zip(&values[*x].data(), &values[*places].data(), |x, p| {
                numbers(x, p, chance::Op::Round, |x, p| {
                    let scale = 10f64.powf(p);
                    Val::num((x * scale).round() / scale)
                }, &wrong)
            })?;
            if wrong.get() {
                return failed("round needs numbers");
            }
            Ev::Data(out)
        }
        Part::Floor(x) | Part::Ceil(x) => {
            let up = matches!(part, Part::Ceil(_));
            let wrong = StdCell::new(false);
            let out = values[*x].data().map(|x| number(x, if up { chance::Un::Ceil } else { chance::Un::Floor }, |x| if up { x.ceil() } else { x.floor() }, &wrong));
            if wrong.get() {
                return failed("rounding needs a number");
            }
            Ev::Data(out)
        }
        Part::Math(op, x) => {
            let wrong = StdCell::new(false);
            let plain = |x: f64| match op {
                chance::Un::Exp => x.exp(),
                chance::Un::Ln => x.ln(),
                _ => x.sqrt(),
            };
            let out = values[*x].data().map(|x| number(x, *op, plain, &wrong));
            if wrong.get() {
                return failed(format!("{} needs a number", math_name(*op)));
            }
            Ev::Data(out)
        }
        Part::Draw(std, site) => Ev::Data(pure(Val::Rand(chance::draw(*std, &draw_name(cx, *site))))),
        Part::Tri(u, low, likely, high) => {
            let ends = pairs(&values[*low].data(), &values[*high].data())?;
            let (u, likely) = (values[*u].data(), values[*likely].data());
            let wrong = StdCell::new(false);
            let out = likely.zip(&ends, |likely, (low, high)| match (term(low), term(likely), term(high)) {
                (Some(low), Some(likely), Some(high)) if [&low, &likely, &high].iter().all(|t| !matches!(t, Term::Flag(_))) => (low, likely, high),
                _ => {
                    wrong.set(true);
                    (Term::Undefined, Term::Undefined, Term::Undefined)
                }
            });
            if wrong.get() {
                return failed("triangular needs numbers");
            }
            Ev::Data(u.zip(&out, |u, (low, likely, high)| val(chance::tri(&term(u).expect("a draw"), low, likely, high))))
        }
        Part::Integers(low, high, site) => Ev::Data(integers(cx, &values[*low].data(), &values[*high].data(), *site)?),
        Part::Bernoulli(p, site) => Ev::Data(bernoulli(cx, &values[*p].data(), *site)?),
        Part::Expect { mean, of } => Ev::Data(expect(cx, *mean, &values[*of])?),
        Part::Bin(op, a, b) => binary(*op, &values[*a], &values[*b])?,
        Part::OneOf(rows, site) => Ev::Data(one_of(cx, rows, *site)?),
        Part::Seen { family, what, args, site } => {
            let mut together = values[*what].data().map(|v| vec![v.clone()]);
            for a in args {
                let a = values[*a].data();
                if together.len().saturating_mul(a.len()) > MOST_PAIRS {
                    return failed(format!("what {family} is given differs too many ways across worlds to work out"));
                }
                together = together.zip(&a, |so_far, v| {
                    let mut all = so_far.clone();
                    all.push(v.clone());
                    all
                });
            }
            let wrong = StdCell::new(false);
            // What's seen is a truth for bernoulli and a number for the rest, and what it's drawn from takes numbers.
            let fits = |vs: &Vec<Val>| (if family == "bernoulli" { truth_like(&vs[0]) } else { number_like(&vs[0]) }) && vs[1..].iter().all(number_like);
            let logs = together.map(|vs| match vs.iter().map(term).collect::<Option<Vec<Term>>>() {
                Some(ts) if fits(vs) => log_likely(family, &ts[0], &ts[1..]),
                _ => {
                    wrong.set(true);
                    Term::Undefined
                }
            });
            if wrong.get() {
                return failed(if family == "bernoulli" { "what's seen to be drawn from bernoulli is true or false, and its chance a number".to_string() } else { format!("what's seen to be drawn from {family}, and what it's drawn from, are numbers") });
            }
            Ev::Data(seen_value(cx, &logs, matches!(family.as_str(), "bernoulli" | "integers"), *site)?)
        }
        Part::SeenOneOf { what, nodes, site } => {
            let n = nodes.len() as f64;
            let logs = values[*what].data().map(|v| match v {
                Val::Ref(e) if nodes.contains(e) => Term::Num(-n.ln()),
                _ => Term::Undefined,
            });
            Ev::Data(seen_value(cx, &logs, true, *site)?)
        }
    })
}

/// The log of how likely a distribution makes what was seen: its density, or its probability for a
/// short list. Undefined where what was seen couldn't come from it, or where it isn't a distribution, as
/// with a spread below nothing: either way, what was seen weighs nothing there. Each is written as
/// arithmetic, so it's the same whether what goes in is plain or uncertain.
fn log_likely(family: &str, x: &Term, args: &[Term]) -> Term {
    use chance::{Op, Un};
    let n = |v: f64| Term::Num(v);
    let bin = |op: Op, a: &Term, b: &Term| chance::bin(op, a, b);
    let un = |op: Un, a: &Term| chance::un(op, a);
    let only = |ok: &Term, v: &Term| chance::pick(ok, v, &Term::Undefined);
    let normal = |x: &Term, mean: &Term, sd: &Term| {
        let z = bin(Op::Div, &bin(Op::Sub, x, mean), sd);
        let log = bin(Op::Sub, &bin(Op::Mul, &n(-0.5), &bin(Op::Mul, &z, &z)), &bin(Op::Add, &un(Un::Ln, sd), &n(0.5 * (2.0 * std::f64::consts::PI).ln())));
        only(&bin(Op::Lt, &n(0.0), sd), &log)
    };
    match (family, args) {
        ("normal", [mean, sd]) => normal(x, mean, sd),
        ("lognormal", [mean, sd]) => {
            let log_x = un(Un::Ln, x);
            bin(Op::Sub, &normal(&log_x, mean, sd), &log_x)
        }
        ("between", [low, high]) => {
            let mean = bin(Op::Div, &bin(Op::Add, low, high), &n(2.0));
            let sd = bin(Op::Div, &bin(Op::Sub, high, low), &n(2.0 * 1.6448536269514722));
            normal(x, &mean, &sd)
        }
        ("uniform", [low, high]) => {
            let inside = bin(Op::And, &bin(Op::Le, low, x), &bin(Op::Le, x, high));
            only(&bin(Op::And, &inside, &bin(Op::Lt, low, high)), &un(Un::Neg, &un(Un::Ln, &bin(Op::Sub, high, low))))
        }
        ("exponential", [mean]) => {
            let ok = bin(Op::And, &bin(Op::Le, &n(0.0), x), &bin(Op::Lt, &n(0.0), mean));
            only(&ok, &un(Un::Neg, &bin(Op::Add, &un(Un::Ln, mean), &bin(Op::Div, x, mean))))
        }
        ("triangular", [low, likely, high]) => {
            let width = bin(Op::Sub, high, low);
            let rising = un(Un::Ln, &bin(Op::Div, &bin(Op::Mul, &n(2.0), &bin(Op::Sub, x, low)), &bin(Op::Mul, &width, &bin(Op::Sub, likely, low))));
            let falling = un(Un::Ln, &bin(Op::Div, &bin(Op::Mul, &n(2.0), &bin(Op::Sub, high, x)), &bin(Op::Mul, &width, &bin(Op::Sub, high, likely))));
            let shaped = bin(Op::And, &bin(Op::Le, low, likely), &bin(Op::And, &bin(Op::Le, likely, high), &bin(Op::Lt, low, high)));
            let inside = bin(Op::And, &bin(Op::Le, low, x), &bin(Op::Le, x, high));
            only(&bin(Op::And, &shaped, &inside), &chance::pick(&bin(Op::Lt, x, likely), &rising, &falling))
        }
        ("bernoulli", [p]) => {
            let ok = bin(Op::And, &bin(Op::Le, &n(0.0), p), &bin(Op::Le, p, &n(1.0)));
            only(&ok, &chance::pick(x, &un(Un::Ln, p), &un(Un::Ln, &bin(Op::Sub, &n(1.0), p))))
        }
        ("integers", [low, high]) => {
            let whole = |t: &Term| bin(Op::Eq, t, &un(Un::Floor, t));
            let ok = bin(Op::And, &bin(Op::And, &whole(x), &bin(Op::And, &whole(low), &whole(high))), &bin(Op::And, &bin(Op::Le, low, x), &bin(Op::Le, x, high)));
            only(&ok, &un(Un::Neg, &un(Un::Ln, &bin(Op::Add, &bin(Op::Sub, high, low), &n(1.0)))))
        }
        _ => Term::Undefined,
    }
}

/// What something seen comes to, from how likely it was in each world, as a log. Where that's the same
/// in every draw, it's what learning that a coin came up heads is, with a chance of how likely it was in
/// each world: a chance of its own, exact, which evidence then keeps the heads of, so the worlds are
/// weighed by Bayes' rule with nothing new. For a density, the chances are each how likely relative to
/// the likeliest world, which changes nothing evidence does. Where it depends on draws, it's the log of
/// how likely, in each draw, for a reader to weigh them by.
fn seen_value(cx: &Cx, logs: &Cond<Term>, probability: bool, site: u32) -> Result<Cond<Val>, Error> {
    let plain = |t: &Term| match t {
        Term::Num(n) => Some(*n),
        Term::Undefined => Some(f64::NAN),
        _ => None,
    };
    let most = logs.cases().iter().filter_map(|(_, t)| plain(t)).filter(|l| l.is_finite()).fold(f64::NEG_INFINITY, f64::max);
    let scale = if probability || !most.is_finite() { 0.0 } else { most };
    let chances = logs.map(|t| match (plain(t), t) {
        (Some(l), _) if l.is_finite() => Val::num((l - scale).exp().min(1.0)),
        (Some(_), _) => Val::Num(0.0),
        // Placeholders, replaced just below.
        (None, _) => Val::Undefined,
    });
    // Called for where it's written, and marked as the chance something was seen, so it isn't asked.
    let coins = bernoulli_called(&format!("{}seen", draw_name(cx, site)), &chances)?;
    let mut out = pure(Val::Undefined);
    for (when, t) in logs.cases() {
        let here = match t {
            Term::Rand(r) => pure(Val::Likely(*r)),
            _ => coins.clone(),
        };
        out = Cond::when(&when, &here, &out);
    }
    Ok(out)
}

/// One of a group of nodes, each as likely, such as a row of a table: a variable with a node for each
/// outcome, as a short list is.
fn one_of(cx: &Cx, rows: &[Entity], site: u32) -> Result<Cond<Val>, Error> {
    if rows.is_empty() {
        return failed("one_of needs a group with something in it, and this has nothing");
    }
    if rows.len() as f64 > MOST_LISTED {
        return failed(format!("one_of lists each node as an outcome, and {} is more than the {MOST_LISTED} it can", rows.len()));
    }
    let options: Vec<(String, f64, bool)> = rows.iter().map(|r| (r.to_string(), 1.0, false)).collect();
    let var = Space::global().var(&draw_name(cx, site), &options);
    Ok(var.value().map(|o| Val::Ref(o.parse().expect("written as a node just above"))))
}

fn binary(op: BinOp, a: &Ev, b: &Ev) -> Result<Ev, Error> {
    let sym = op.symbol();
    match op {
        BinOp::Or if any_likely(a) || any_likely(b) => failed("|| can't say one of two things was seen and not which: what's seen goes together with &&"),
        // Both seen: how likely both are is one times the other, and something seen where a truth holds
        // is seen only there.
        BinOp::And if any_likely(a) || any_likely(b) => {
            let wrong = StdCell::new(false);
            let likely = |t: Term| match t {
                Term::Rand(r) => Val::Likely(r),
                // How likely, the same in every draw, says nothing about the draws.
                Term::Num(_) => Val::Bool(true),
                _ => Val::Bool(false),
            };
            let out = zip(&a.data(), &b.data(), |x, y| match (x, y) {
                (Val::Likely(p), Val::Likely(q)) => likely(chance::bin(chance::Op::Add, &Term::Rand(*p), &Term::Rand(*q))),
                (Val::Likely(p), t) | (t, Val::Likely(p)) => match t {
                    Val::Bool(true) => Val::Likely(*p),
                    Val::Bool(false) => Val::Bool(false),
                    Val::Undefined => Val::Undefined,
                    Val::Rand(r) if r.is_truth() => likely(chance::pick(&Term::Rand(*r), &Term::Rand(*p), &Term::Undefined)),
                    _ => {
                        wrong.set(true);
                        Val::Undefined
                    }
                },
                _ => {
                    wrong.set(true);
                    Val::Undefined
                }
            })?;
            if wrong.get() {
                return failed("&& goes with something seen only with true or false, or something else seen");
            }
            Ok(Ev::Data(out))
        }
        // With something uncertain on either side it's still three-valued, in every draw.
        BinOp::And | BinOp::Or if uncertain(a) || uncertain(b) => {
            let wrong = StdCell::new(false);
            let cop = if op == BinOp::And { chance::Op::And } else { chance::Op::Or };
            let out = zip(&a.data(), &b.data(), |x, y| {
                if truth_like(x) && truth_like(y) {
                    val(chance::bin(cop, &term(x).expect("a truth"), &term(y).expect("a truth")))
                } else {
                    wrong.set(true);
                    Val::Undefined
                }
            })?;
            if wrong.get() {
                return failed(format!("{sym} needs true or false"));
            }
            Ok(Ev::Data(out))
        }
        BinOp::And | BinOp::Or => {
            let ((ay, au), (by, bu)) = (a.truth()?, b.truth()?);
            // Nearly always, nothing's undefined, and a puzzle does this for every constraint.
            if au.is_never() && bu.is_never() {
                return Ok(Ev::Truth { yes: if op == BinOp::And { &ay & &by } else { &ay | &by }, undefined: au });
            }
            let (an, bn) = (&!&ay & &!&au, &!&by & &!&bu);
            // Three-valued, so false && undefined is false, and true || undefined is true.
            let (yes, no) = if op == BinOp::And { (&ay & &by, &an | &bn) } else { (&ay | &by, &an & &bn) };
            let undefined = &!&yes & &!&no;
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Eq | BinOp::Ne if matches!((a, b), (Ev::Truth { .. }, _) | (_, Ev::Truth { .. })) && !uncertain(a) && !uncertain(b) => {
            let ((ay, au), (by, bu)) = (a.truth()?, b.truth()?);
            let undefined = &au | &bu;
            let same = &ay.iff(&by) & &!&undefined;
            let yes = if op == BinOp::Eq { same } else { &!&same & &!&undefined };
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Eq | BinOp::Ne => {
            let cop = if op == BinOp::Eq { chance::Op::Eq } else { chance::Op::Ne };
            let same = zip(&a.data(), &b.data(), |x, y| match (x, y) {
                (Val::Undefined, _) | (_, Val::Undefined) => Val::Undefined,
                // Two numbers, or two truths, are the same in some draws and not in others. Anything else
                // is never the same as something uncertain.
                (Val::Rand(_), _) | (_, Val::Rand(_)) if (number_like(x) && number_like(y)) || (truth_like(x) && truth_like(y)) => val(chance::bin(cop, &term(x).expect("checked"), &term(y).expect("checked"))),
                (Val::Estimate(_), _) | (_, Val::Estimate(_)) => estimate_op(x, y, cop, &|p, q| Val::Bool((p == q) == (op == BinOp::Eq))),
                _ => Val::Bool((x == y) == (op == BinOp::Eq)),
            })?;
            if any_rand(&same) {
                return Ok(Ev::Data(same));
            }
            let e = Ev::Data(same);
            let (yes, undefined) = e.truth()?;
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Max | BinOp::Min => {
            let wrong = StdCell::new(false);
            let cop = match op {
                BinOp::Add => chance::Op::Add,
                BinOp::Sub => chance::Op::Sub,
                BinOp::Mul => chance::Op::Mul,
                BinOp::Pow => chance::Op::Pow,
                BinOp::Max => chance::Op::Max,
                BinOp::Min => chance::Op::Min,
                _ => chance::Op::Div,
            };
            let out = zip(&a.data(), &b.data(), |x, y| {
                numbers(x, y, cop, |x, y| {
                    Val::num(match op {
                        BinOp::Add => x + y,
                        BinOp::Sub => x - y,
                        BinOp::Mul => x * y,
                        // A negative number to a fractional power isn't a number: NaN, so undefined.
                        BinOp::Pow => x.powf(y),
                        BinOp::Max => x.max(y),
                        BinOp::Min => x.min(y),
                        _ => x / y,
                    })
                }, &wrong)
            })?;
            if wrong.get() {
                return failed(format!("{sym} needs numbers"));
            }
            Ok(Ev::Data(out))
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let wrong = StdCell::new(false);
            // Above is below, the other way round.
            let flipped = matches!(op, BinOp::Gt | BinOp::Ge);
            let strict = matches!(op, BinOp::Lt | BinOp::Gt);
            let out = zip(&a.data(), &b.data(), |x, y| {
                let (x, y) = if flipped { (y, x) } else { (x, y) };
                numbers(x, y, if strict { chance::Op::Lt } else { chance::Op::Le }, |x, y| Val::Bool(if strict { x < y } else { x <= y }), &wrong)
            })?;
            if wrong.get() {
                return failed(format!("{sym} needs numbers"));
            }
            if any_rand(&out) {
                return Ok(Ev::Data(out));
            }
            let (yes, undefined) = Ev::Data(out).truth()?;
            Ok(Ev::Truth { yes, undefined })
        }
    }
}

/// What's in these fields, one after another. A field of a record is that record's. A field of a node is
/// its cell, so what's read through a choice is whatever it lands on says, world by world.
fn field_of(cx: &Cx, c: &Cond<Val>, fields: &[String]) -> Result<Cond<Val>, Error> {
    let mut c = c.clone();
    for f in fields {
        c = if c.cases().iter().any(|(_, v)| matches!(v, Val::Ref(_))) {
            let mut out = pure(Val::Undefined);
            for (when, v) in c.cases() {
                let here = match v {
                    Val::Ref(e) => read_cell(cx, *e, f, &[])?.data(),
                    other => pure(other.field(f)),
                };
                out = Cond::when(&when, &here, &out);
            }
            out
        } else {
            c.map(|v| v.field(f))
        };
    }
    Ok(c)
}

/// A cell of any shape, as built-in cells write them: a `Bool`, cases, or something plain.
fn from_json(v: Value) -> Result<Ev, String> {
    let is_bool = v.as_object().is_some_and(|o| o.contains_key("vars") && o.contains_key("nodes") && o.contains_key("root"));
    if is_bool {
        let yes: Bool = serde_json::from_value(v).map_err(|e| e.to_string())?;
        return Ok(Ev::Truth { yes, undefined: Space::global().never() });
    }
    let w: Worldly<Val> = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok(Ev::Data(w.0))
}

// ---------- compiling ----------

/// What goes in the cell, given what a formula works out.
type Wrap<C> = fn(Cond<Val>) -> Result<C, Error>;

/// A formula's value, whichever cell it's on.
fn work_out(cx: &Cx, e: &Expr, expected: &Type) -> Result<Cond<Val>, Error> {
    let closed = Expander::new(cx)?.expand_all(e)?;
    let found = check(cx, &closed)?;
    if expected.join(&found).is_none() {
        return failed(format!("this cell holds {}, and this formula gives {}", expected.word(), found.word()));
    }
    Ok(evaluate(cx, &closed)?.data())
}

/// How a formula on a cell that holds `C` is made: it has to give `expected`, and `wrap` makes that
/// the cell's type.
pub fn compiler<C: CellType>(expected: Type, wrap: Wrap<C>) -> Compile {
    Arc::new(move |text: &str| {
        let e = expr::parse(text)?;
        let text = text.to_string();
        let expected = expected.clone();
        let is_value = C::NAME == ValueCell::NAME;
        Ok(Computed::typed(&format!("formula on {}", C::NAME), move |cx| {
            // A value's type is a cell of its own. Reading it first means a loop or a type error is found
            // there, by looking only at types.
            if is_value {
                cx.own::<ValueType>()?;
            }
            Ok(Some(wrap(work_out(cx, &e, &expected).map_err(|err| in_text(err, &text))?)?))
        }))
    })
}

/// A formula on one of the workspace's own cells, such as a price or a range, which no component
/// brings. Its type is checked as it runs.
fn free_compiler() -> Compile {
    Arc::new(|text: &str| {
        let e = expr::parse(text)?;
        let text = text.to_string();
        Ok(Computed::json("formula", move |cx| {
            let c = work_out(cx, &e, &Type::Any).map_err(|err| in_text(err, &text))?;
            Ok(Some(serde_json::to_value(Worldly(c)).expect("values serialise")))
        }))
    })
}

/// What a formula gives a cell that holds true or false. Undefined is `None`.
pub fn flag_cases(c: Cond<Val>, what: &str) -> Result<Worldly<Option<bool>>, Error> {
    if any_rand(&c) {
        return failed(format!("{what} can't be a matter of chance yet: it has to be true or false in each world"));
    }
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !matches!(v, Val::Bool(_) | Val::Undefined)) {
        return failed(format!("{what} is true or false, not {}", Type::of(bad).word()));
    }
    Ok(Worldly(c.map(|v| if let Val::Bool(b) = v { Some(*b) } else { None })))
}

/// What a formula gives a cell that holds a number, which may be uncertain. Undefined is `None`.
pub fn number_cases(c: Cond<Val>, what: &str) -> Result<Worldly<Option<Num>>, Error> {
    if c.cases().iter().any(|(_, v)| matches!(v, Val::Estimate(_))) {
        return failed(format!("{what} can't be something read from draws yet, such as a mean with no law: it has to be a number, or uncertain"));
    }
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !number_like(v)) {
        return failed(format!("{what} is a number, not {}", Type::of(bad).word()));
    }
    Ok(Worldly(c.map(|v| match v {
        Val::Num(n) => Some(Num::Plain(*n)),
        Val::Rand(r) => Some(Num::Rand(*r)),
        _ => None,
    })))
}

/// What a formula gives a cell that holds a number that has to be a plain one in each world, as what
/// makes the worlds does.
pub fn plain_number_cases(c: Cond<Val>, what: &str) -> Result<Worldly<Option<f64>>, Error> {
    if any_rand(&c) {
        return failed(format!("{what} can't be uncertain: it has to be one number in each world"));
    }
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !matches!(v, Val::Num(_) | Val::Undefined)) {
        return failed(format!("{what} is a number, not {}", Type::of(bad).word()));
    }
    Ok(Worldly(c.map(|v| if let Val::Num(n) = v { Some(*n) } else { None })))
}

/// What this formula would give on this node, and what it expands to, with nothing changed. Any part
/// of a formula can be asked about this way, because it can read what the whole formula can.
pub fn try_formula(g: &Graph, e: Entity, text: &str) -> Result<(Cond<Val>, String), Error> {
    let parsed = expr::parse(text).map_err(Error::Failed)?;
    g.run(e, |cx| {
        let expansion = render(&Expander::new(cx)?.expand_all(&parsed)?);
        let value = work_out(cx, &parsed, &Type::Any).map_err(|err| in_text(err, text))?;
        Ok((value, expansion))
    })
}

/// A formula as it reads once it's been expanded, with nodes as handles. It's as long as it's useful
/// to read, and then it stops.
fn render(closed: &Closed) -> String {
    const LONGEST: usize = 2000;
    fn read(node: &Entity, component: &str, fields: &[String]) -> String {
        let mut s = node.to_string();
        if component != ValueCell::NAME || !fields.is_empty() {
            s = format!("{s}.{component}");
        }
        fields.iter().for_each(|f| s = format!("{s}.{f}"));
        s
    }
    let mut out: Vec<String> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let text = match part {
            Part::Lit(Val::Num(n)) => n.to_string(),
            Part::Lit(Val::Undefined) => "undefined".to_string(),
            Part::Lit(Val::Ref(e)) => e.to_string(),
            Part::Lit(v) => serde_json::to_string(v).expect("values serialise"),
            Part::Read { node, component, fields } => read(node, component, fields),
            Part::Field(a, fields) => format!("{}.{}", out[*a], fields.join(".")),
            Part::If(c, a, b) => format!("if({}, {}, {})", out[*c], out[*a], out[*b]),
            Part::Bin(op @ (BinOp::Max | BinOp::Min), a, b) => format!("{}({}, {})", op.symbol(), out[*a], out[*b]),
            Part::Bin(op, a, b) => format!("({} {} {})", out[*a], op.symbol(), out[*b]),
            Part::Not(a) => format!("!{}", out[*a]),
            Part::Neg(a) => format!("-{}", out[*a]),
            Part::Defined(a) => format!("defined({})", out[*a]),
            Part::Has(n, c) => format!("has({n}.{c})"),
            Part::Chose(d, o) => format!("chose({d}, {o})"),
            Part::Round(a, b) => format!("round({}, {})", out[*a], out[*b]),
            Part::Floor(a) => format!("floor({})", out[*a]),
            Part::Ceil(a) => format!("ceil({})", out[*a]),
            Part::Math(op, a) => format!("{}({})", math_name(*op), out[*a]),
            Part::Draw(Std::Normal, site) => format!("normal#{site}"),
            Part::Draw(Std::Uniform, site) => format!("uniform#{site}"),
            Part::Draw(Std::Exponential, site) => format!("exponential#{site}"),
            Part::OneOf(rows, _) => format!("one_of({} nodes)", rows.len()),
            Part::Tri(_, a, b, c) => format!("triangular({}, {}, {})", out[*a], out[*b], out[*c]),
            Part::Integers(a, b, _) => format!("integers({}, {})", out[*a], out[*b]),
            Part::Bernoulli(p, _) => format!("bernoulli({})", out[*p]),
            Part::Expect { mean, of } => format!("{}({})", if *mean { "mean" } else { "chance" }, out[*of]),
            Part::Seen { family, what, args, .. } => format!("({} ~ {family}({}))", out[*what], args.iter().map(|a| out[*a].clone()).collect::<Vec<_>>().join(", ")),
            Part::SeenOneOf { what, nodes, .. } => format!("({} ~ one_of({} nodes))", out[*what], nodes.len()),
        };
        out.push(if text.len() > LONGEST { "…".to_string() } else { text });
    }
    out.pop().unwrap_or_default()
}

fn in_text(err: Error, text: &str) -> Error {
    match err {
        Error::Failed(why) => Error::Failed(format!("{why}, in: {text}")),
        other => other,
    }
}

/// One of the workspace's own cells as a world value, if it holds what a formula would.
pub fn shown_free(g: &Graph, e: Entity, cell: &str) -> Option<Cell<Shown>> {
    let v = match g.get_value(e, cell) {
        Ok(Some(v)) => v,
        Ok(None) => return Some(Ok(None)),
        Err(err) => return Some(Err(err)),
    };
    let w: Worldly<Val> = serde_json::from_value(v).ok()?;
    Some(Ok(Some(shown_val(&w.0))))
}

// ---------- what's built in ----------

pub(crate) fn numeric(options: &[String]) -> bool {
    !options.is_empty() && options.iter().all(|o| o.parse::<f64>().is_ok_and(f64::is_finite))
}

/// The node nobody wrote for one of a list of options: its name is the option, and so is its value, which
/// is a number if all the options are.
pub fn option_node(g: &Graph, option: &str, numeric: bool) -> Entity {
    g.constant(option_cells(option, numeric))
}

pub(crate) fn option_cells(option: &str, numeric: bool) -> BTreeMap<String, Value> {
    let value = if numeric { json!(option.parse::<f64>().expect("checked by the caller")) } else { json!(option) };
    BTreeMap::from([(recollect_graph::Name::NAME.to_string(), json!(option)), (ValueCell::NAME.to_string(), value)])
}

/// How a choice with a list of options lands: on any of them, equally. A table's options are what its
/// rows choose from: it isn't a choice itself.
fn options_chosen(cx: &Cx) -> Cell<Chosen> {
    // A decision that lists some of its options this way lands as a decision does.
    if cx.own_has(crate::structure::DECISION) {
        return Ok(cx.own::<crate::structure::Falls>()?.map(|f| Chosen(f.on)));
    }
    let Some(Options(options)) = cx.own::<Options>()? else { return Ok(None) };
    if cx.row_key().is_none() && !cx.rows().is_empty() {
        return Ok(None);
    }
    if options.is_empty() {
        return Ok(Some(Chosen(Worldly::plain(Val::Undefined))));
    }
    let is_numeric = numeric(&options);
    let nodes: HashMap<&str, Entity> = options.iter().map(|o| (o.as_str(), cx.constant(option_cells(o, is_numeric)))).collect();
    let names: Vec<&str> = options.iter().map(String::as_str).collect();
    let var = Space::global().uniform(&cx.id().to_string(), &names);
    Ok(Some(Chosen(Worldly(var.value().map(|o| Val::Ref(nodes[o.as_str()]))))))
}

/// How sure is a number above 0 and at most 1.
fn a_chance(v: &Value) -> Result<(), String> {
    match v.as_f64() {
        Some(p) if p > 0.0 && p <= 1.0 => Ok(()),
        _ => Err(format!("how surely it has to be met is a number above 0 and at most 1, not {v}")),
    }
}

/// Settling a choice is naming one node.
pub(crate) fn one_node(v: &Value) -> Result<(), String> {
    match serde_json::from_value::<Val>(v.clone()) {
        Ok(Val::Ref(_)) => Ok(()),
        _ => Err(format!("a choice is settled by naming a node, as {{\"ref\": handle}}, not {v}")),
    }
}

/// The options a node chooses between, if it's a choice: it has options, said or taken from its kind or
/// table, and it isn't a table, whose options are what its rows choose from.
pub fn options_of(g: &Graph, e: Entity) -> Option<Vec<String>> {
    let Options(options) = g.get::<Options>(e).ok()??;
    let table = g.row_of(e).is_none() && g.table_here(e).is_some_and(|t| !g.rows(&t).is_empty());
    (!table).then_some(options)
}

/// What type a node's value is.
fn type_of_value(cx: &Cx) -> Cell<ValueType> {
    Ok(match cx.backed(ValueCell::NAME) {
        None => None,
        Some(Backed::Value(v)) => {
            let v: Worldly<Val> = serde_json::from_value(v).map_err(|e| Error::Malformed { cell: (cx.id(), ValueCell::NAME.to_string()), detail: e.to_string() })?;
            let mut t = Type::Undefined;
            for (_, v) in v.0.cases() {
                t = t.join(&Type::of(v)).unwrap_or(Type::Any);
            }
            Some(ValueType(t))
        }
        Some(Backed::Builtin) => Some(ValueType(Type::Any)),
        Some(Backed::Formula(text)) => {
            let e = expr::parse(&text).map_err(Error::Failed)?;
            let closed = Expander::new(cx)?.expand_all(&e).map_err(|err| in_text(err, &text))?;
            Some(ValueType(check(cx, &closed).map_err(|err| in_text(err, &text))?))
        }
    })
}

/// A macro, with the macros it calls found.
fn macro_of(cx: &Cx) -> Cell<MacroBody> {
    let Some(Macro { params, body }) = cx.own::<Macro>()? else { return Ok(None) };
    let parsed = expr::parse(&body).map_err(|e| Error::Failed(format!("{e}, in: {body}")))?;
    let mut uses = BTreeMap::new();
    for (to, edge) in cx.outgoing(LABEL) {
        if let (Some(Called(name)), Some(m)) = (cx.get::<Called>(edge)?, cx.get::<MacroBody>(to)?) {
            uses.insert(name, m);
        }
    }
    Ok(Some(MacroBody { params, body: parsed, uses }))
}

fn valid_name(v: &Value) -> Result<(), String> {
    match v.as_str() {
        Some(s) if expr::is_name(s) => Ok(()),
        _ => Err(format!("a label has to be a name a formula can use, such as rate, not {v}")),
    }
}

/// How a value reads. If it's only ever true or false, it reads as the worlds where it's true, and if it's
/// a node wherever it's anything, as the nodes it could be.
fn shown_val(c: &Cond<Val>) -> Shown {
    let cases = c.cases();
    if cases.iter().all(|(_, v)| matches!(v, Val::Bool(_))) {
        return Shown::Bool(c.holds(|v| *v == Val::Bool(true)));
    }
    if cases.iter().any(|(_, v)| matches!(v, Val::Ref(_))) && cases.iter().all(|(_, v)| matches!(v, Val::Ref(_) | Val::Undefined)) {
        return Shown::Named(cases.into_iter().filter_map(|(when, v)| if let Val::Ref(e) = v { Some((*e, when)) } else { None }).collect());
    }
    Shown::cond(c)
}

pub fn install(reg: &mut Registry) {
    reg.free_formulas(free_compiler());
    reg.component(VALUE, "An answer: typed in, or a formula over what it has labelled.").icon("=");
    reg.cell::<ValueCell>(VALUE, "This node's answer: typed in, or a formula over what it has labelled.")
        .compiles(compiler::<ValueCell>(Type::Any, |c| Ok(ValueCell(Worldly(c)))))
        .shown(|v| shown_val(&v.0.0))
        .summary()
        .per_entity()
        .initial(json!(0.0));
    reg.cell::<ValueType>(VALUE, "").builtin(type_of_value).internal();

    reg.component(CHOICE, "Lands on one node in each world. What's read through it, as chosen.price, is what that node says.").icon("◇");
    reg.cell::<Chosen>(CHOICE, "What it lands on, in each world. Name one to settle it.").validate(one_node).shown(|c| shown_val(&c.0.0)).summary().per_entity();

    reg.component(OPTIONS, "A choice from a plain list, such as colours: each is a node with that name and value.").requires(CHOICE);
    reg.cell::<Options>(OPTIONS, "What it can land on.").initial(json!([]));
    reg.fill::<Chosen>(OPTIONS, options_chosen);
    reg.component(AT, "Where a choice goes on a board.").requires(OPTIONS);
    reg.cell::<At>(AT, "Where a choice goes on a board.").initial(json!({ "row": 0, "col": 0 }));

    reg.component(REQUIRES, "Demanded of your choices: only the ways they could go that make this node's value true, as surely as it says, count.").icon("✦").requires(VALUE);
    reg.cell::<AtLeast>(REQUIRES, "How surely it has to be met, from 0 to 1, where what it depends on is a matter of chance. 1 is always.").validate(a_chance).initial(json!(1.0));
    // A demand is met or it isn't, so what's seen, which is how likely, isn't one.
    reg.check(REQUIRES, "requirement_is_seen", Severity::Invariant, |cx| {
        Ok(match cx.own::<ValueType>() {
            Ok(Some(ValueType(Type::Seen))) => vec!["a requirement is met or it isn't, and something seen with how likely it was is evidence instead: see docs/inference.md".to_string()],
            _ => vec![],
        })
    });
    reg.component(EVIDENCE, "What's been seen: a truth, and only the worlds and draws where it's true count, or something seen with how likely it was, y ~ normal(…), and each counts as much as it makes that likely. It never restricts a choice of yours.").icon("◉").requires(VALUE).constraint(|g, e| Ok(g.get::<ValueCell>(e)?.map(|v| evidence_worlds(&v))));
    // Known in some worlds as a matter of draws and in others not is neither one thing nor the other,
    // and isn't worked out, so it says so rather than quietly meaning less than it reads.
    reg.check(EVIDENCE, "evidence_partly_a_matter_of_draws", Severity::Invariant, |cx| {
        // A value that can't be worked out says so itself, and isn't this check's to refuse.
        let Some(v) = cx.own::<ValueCell>().ok().flatten() else { return Ok(vec![]) };
        let drawn = v.0.0.cases().iter().any(|(_, v)| matches!(v, Val::Rand(_) | Val::Likely(_)));
        let mixed = drawn && v.0.0.len() > 1;
        Ok(if mixed { vec!["what's known is an uncertain truth in some worlds and not in others, which isn't worked out yet: see docs/distributions.md".to_string()] } else { vec![] })
    });
    reg.advice(judging);
    reg.component(FOUND, "What a view has found out by pinning: never saved, and only on the copy of the graph it looks through.");
    reg.cell::<Found>(FOUND, "Each matter of chance found out, and how it landed.").internal().initial(json!({}));

    reg.component(SHOULD, "What should be true here. Where it isn't, a notice says so, and no world is ruled out.").icon("✓");
    reg.cell::<Should>(SHOULD, "A truth that should hold: written on a kind, each instance is held to it.")
        .compiles(compiler::<Should>(Type::Bool, |c| flag_cases(c, "should").map(Should)))
        .shown(|s| Shown::cond(&s.0.0));
    reg.check(SHOULD, "should_hold", Severity::Notice, |cx| {
        // A kind says what its instances should be; it isn't held to it itself.
        if !cx.incoming(IS_A).is_empty() {
            return Ok(vec![]);
        }
        let Some(Should(holds)) = cx.own::<Should>()? else { return Ok(vec![]) };
        let fails = holds.0.holds(|v| *v == Some(false));
        let unknown = holds.0.holds(|v| v.is_none());
        Ok(if fails.is_always() {
            vec!["isn't so".to_string()]
        } else if !fails.is_never() {
            vec![format!("isn't so where {}", fails.show())]
        } else if unknown.is_always() {
            vec!["can't be told".to_string()]
        } else {
            vec![]
        })
    });
    reg.component(MACRO, "A function of your own, which formulas call by labelling this node.").icon("ƒ");
    reg.cell::<Macro>(MACRO, "Its parameters, and the formula it stands for.").initial(json!({ "params": [], "body": "0" }));
    reg.cell::<MacroBody>(MACRO, "").builtin(macro_of).internal();

    reg.edge(LABEL, "The name a formula on this node uses for that one.").arranges().reversed();
    reg.cell::<Called>(LABEL, "The name a formula on the source uses for the target.").validate(valid_name).initial(json!(""));
    // A kind is where a formula is written, not a step its numbers go through, so it doesn't shape a map.
    reg.edge(IS_A, "What kind of thing this is: it has its kind's components, and takes any cell it doesn't say from there.");
    reg.inherit_through(IS_A);
}

/// How to use what this system gives, for an agent: see `mcp.rs` in the server.
pub const GUIDE: &str = "\
A workspace can have cells of its own, such as `price` or `layer`, which no component brings and any node or edge can have. When \
several nodes are the same kind of thing, make a node for the kind and link each to it with an `is_a` edge: they have its components, \
and take any cell they don't write themselves from it (except a node's own `name` and `value`), and a formula on the kind runs for each \
of them, with `self` meaning that node and its other names labelled once on the kind. A read can pick out a kind's instances with \
`nodes: {kind: Concept}`, and a formula can count them with `count(k.in.is_a)` where `k` labels the kind.\n\
\n\
A node's `value` can be typed in, or a formula, and so can any of the workspace's own cells. That's how to state a claim the app can hold \
you to, rather than only describing it. Write {op: set, id, cell: value, formula: \"prev * (1 + rate)\", labels: {prev: \"Year 2\", \
rate: \"Rate\"}}. A label is an edge whose `called` cell is the name a formula on its source uses for its target, and several nodes under \
one name make a group. A formula means the same thing in every world: what you'd get with ordinary values there, so `a + b` works whether \
or not either varies. Operators: + - * / = != < <= > >= && || !. A number, \\\"text\\\", true, false or undefined. `rate` is that node's \
value, `rate.finish` another of its cells, and `self.need` one of this node's. Functions: if(c, a, b), defined(x), or(x, fallback), abs, \
min(a, b), max(a, b), round(x, places?), floor, ceil, has(x.c), and chose(choice, option), which is true where that choice lands on that \
option. Over a group, which is a label, in.component or out.component (the nodes at the other end of those edges), with.component (the \
nodes with it), or any of those followed by more steps such as `k.in.is_a` or `x.out.part_of`, where `in*` or `out*` goes on as far \
as it goes, as `k.in*.is_a` for instances of instances too: sum, product, all, any, count and \
distinct, as sum(line) or sum(t in task: if(t.need, t.estimate, 0)). In a fold over edges, `o.via.weight` reads a cell of the edge `o` \
was reached along, and a formula on an edge reads its ends as `from` and `to`. with.component is every such node in the workspace, so \
only use it when that's what's meant. Division by zero and missing inputs are undefined, only in the worlds where that's so. A formula \
with a type error or an unknown input is refused, with the reason.\n\
\n\
A choice's `chosen` cell is the node it lands on in each world, and a dot reads through it: with `car` labelling a choice, \
`car.chosen.range >= trip` reads the range of whichever car it lands on. `car.chosen = tom` compares nodes. A node with `options` (a \
plain list, such as [\"red\", \"green\"]) is a choice between them: each option is a node nobody wrote, whose `name` and `value` are the \
option, so `a.chosen.value = \"red\"`, and `a.chosen != b.chosen` says two choices differ.\n\
\n\
A name makes a table. A node named `Year[3]` is row 3 of the table `Year`, and `Car[Hatchback]` is the Hatchback row of `Car`: each takes \
what it doesn't say from the node named `Year` or `Car`, as from a kind. A formula there reads `prev.balance` for the row before (`next`, \
`prev.prev`, `row` for which row this is, and `rows` for all of them), so a recurrence is one formula on the table and a starting value on \
the first row. A numbered table goes on past its last row by itself. A table's columns are cells of the workspace's own, since `value` is \
each node's own answer. A table can be a table of choices: give the table node `options` and each row is a choice from them. For a \
puzzle, make a table for each kind of thing and write each clue as a formula naming the rows it's about: \
`Nationality[English].chosen = Colour[Red].chosen`. Say all different with `distinct(r in rows: r.chosen)`. `Arrived.rows` is a table's \
rows by its name and `Left[v.row]` the row with the same key as one being gone over. Write a whole table with one op: {op: table, name: \
\"Year\", columns: {balance: {formula: \"prev.balance * (1 + rate)\", labels: {rate: \"Rate\"}}}, range: [0, 10], rows: [{key: 0, \
balance: 1000}]}, with `is_a` for a table that's a kind of another, `components` for what every row is, and `alternative_of` to make \
every row an option of a decision. `csv: \"<the file's text>\"` in place of `rows` reads them from a CSV whose first line names the \
columns, numbered from 1 unless `key` names the column of keys. `one_of(Runners.rows)` is one row, each as likely (any group of nodes \
will do), as a short list, so `pick.value.t5k` and `pick.value.t10k` are the same row's and move together. See docs/data.md.\n\
\n\
A node with `requires` and a true-or-false value is a demand on choices: only the ways they could go that make it true count. Where it \
depends on chance, its `at_least` cell says how surely, from 0 to 1. A node with `evidence` is something known to be true: it rules out \
the worlds, and the draws, where it's false, and never a choice. Use `evidence` for what was observed and `requires` for what's wanted.\n\
\n\
What was seen, with how likely it was: `y ~ normal(m * x + c, sigma)` says y was drawn from that, and as evidence each world and draw \
counts as much as it makes y likely, so `m` and `c` are found out. The right of `~` is one of normal, lognormal, between, uniform, \
exponential, triangular, bernoulli, integers or one_of. Combine with `&&` or `all(r in T.rows: …)`, never `||` or `!`, and never as a \
requirement. Over short lists it's exact. Over draws, they're weighted, or moved to fit what's seen where weighting leaves too few, and \
what's read says which, and how much drawing adds to its error: say so. See docs/inference.md.\n\
\n\
A number can be uncertain: normal(mean, sd), lognormal(mean, sd), uniform(low, high), triangular(low, likely, high), exponential(mean), \
between(low, high) for about that nine times in ten, integers(low, high), bernoulli(p) and one_of(group). Each place one is written is its own draw, and \
a cell read twice is the same one. chance(x) is how likely x is, and mean(x) what it comes to on average, over everything that's a matter \
of chance and never over a choice, so `requires` can say chance(cost < 4000) >= 0.9.\n\
\n\
To reuse logic, a node with a `macro` cell {params: [t], body: \"t.estimate * 100\"} is a function its callers link to with a label.";
