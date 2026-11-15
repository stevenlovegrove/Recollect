//! Structure: what a design or a plan is made of, coarse to fine, and decisions held open. A part is
//! part of something, or has parts, or is an option, and it's needed in some worlds: those where what
//! it's part of is, and, under a decision, where that decision lands on it. Nothing here is work. `plan`
//! adds what's owed, what's done and what waits, on top of this.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use recollect_graph::worlds::{Bool, Cond, Picks, Space, Worldly};
use recollect_graph::{Cell, CellType, Cx, Entity, Error, Graph, Op, Registry, Severity, Shown, cell};

use crate::expr::{Type, Val};
use crate::formulas::{self, CHOICE, Chosen, Options, compiler};
use crate::plan::UNKNOWN;

pub const NAME: &str = "structure";

pub const PART: &str = "part";
pub const DECISION: &str = "decision";
pub const UNCERTAIN: &str = "uncertain";
pub const OPTION: &str = "option";
pub const PART_OF: &str = "part_of";
pub const ALTERNATIVE: &str = "alternative";
pub const EXCLUDES: &str = "excludes";
/// A choice made knowing how something turns out: see `formulas::judge`.
pub const KNOWING: &str = "knowing";
pub const WITHIN: &str = "within";
pub const NARROWED: &str = "narrowed";

/// Deliberately out of the picture, per world.
#[cell]
pub struct Dropped(pub Worldly<Option<bool>>);

impl Dropped {
    pub fn yes() -> Self {
        Dropped(Worldly::plain(Some(true)))
    }
}

/// On an `alternative` edge: how far its option is believed, before any evidence, against the others.
#[cell]
pub struct Weight(pub Worldly<Option<f64>>);

/// On an `alternative` edge: that its option has been ruled out, and why.
#[cell]
pub struct RuledOut(pub serde_json::Value);

/// The worlds where this node is dropped.
fn dropped_when(cx: &Cx, e: Entity) -> Result<Bool, Error> {
    Ok(cx.get::<Dropped>(e)?.map_or_else(|| Space::global().never(), |d| d.0.0.holds(|v| *v == Some(true))))
}

/// Whether it's in the picture in any world.
pub(crate) fn is_live(cx: &Cx, e: Entity) -> Result<bool, Error> {
    Ok(!dropped_when(cx, e)?.is_always())
}

pub(crate) fn is_dropped(g: &Graph, e: Entity) -> Result<bool, Error> {
    Ok(g.get::<Dropped>(e)?.is_some_and(|d| d.0.0.holds(|v| *v == Some(true)).is_always()))
}

/// Whether an option is still live: not ruled out, and not dropped.
fn live_option(cx: &Cx, option: Entity, edge: Entity) -> Result<bool, Error> {
    Ok(cx.get::<RuledOut>(edge)?.is_none() && is_live(cx, option)?)
}

// ---------- what's being attempted, per world ----------

/// The worlds in which this is part of what's being attempted.
#[cell]
pub struct Need(pub Bool);

/// How a decision lands while it's open, in each world, and the worlds where the variables it isn't
/// using are at rest.
///
/// It lands on any of its live options, as likely as their weights say, or on nothing if it has none.
/// What another choice it `excludes` lands on isn't an option in that world, and only what's part of
/// what a choice it's `within` lands on is, so what's left differs between worlds. Each set that can be left has a variable of its own, weighted over what's in it, so
/// how likely an option is depends on what else is left but the weights don't. That isn't the same as
/// landing on anything and ruling out the worlds where it's excluded, which would make the worlds with
/// more left count for more.
#[cell]
pub struct Falls {
    pub on: Worldly<Val>,
    /// Where each of its variables is either the one in use, or on its first option. Counting worlds
    /// only among these counts each way things could go once.
    pub at_rest: Bool,
}

fn falls(cx: &Cx) -> Cell<Falls> {
    let space = Space::global();
    // Variables are ordered by creation. Whatever this is inside gets its variable first, so formulas
    // read from the outside in.
    cx.own_value::<Need>()?;
    let mut options = Vec::new();
    for (o, edge) in cx.incoming(ALTERNATIVE) {
        if live_option(cx, o, edge)? {
            let gap = cx.has(o, UNKNOWN);
            // How far each answer is believed is what makes the worlds, so it can't differ between them.
            let weight = match cx.get::<Weight>(edge)? {
                None => 0.0,
                Some(w) => match w.0.0.cases().as_slice() {
                    [(_, w)] => w.unwrap_or(0.0),
                    _ => return Err(Error::Failed(format!("the weight of {o} differs between worlds, and a weight has to be the same in all of them"))),
                },
            };
            options.push((o, weight, gap));
        }
    }
    // And the ones it lists by name, which have nothing to say but that, each as likely as the others.
    options.extend(listed(cx)?.into_iter().map(|o| (o, 1.0, false)));
    // The options there are to choose from, world by world.
    let mut left: Cond<BTreeSet<Entity>> = space.pure(options.iter().map(|o| o.0).collect());
    // Only what belongs to whatever each choice this is within lands on. Where that's nothing, nothing does.
    for other in cx.targets(WITHIN) {
        let there = cx.get::<Chosen>(other)?.map_or_else(|| space.pure(Val::Undefined), |l| l.0.0);
        let mut belongs: BTreeMap<Entity, BTreeSet<Entity>> = BTreeMap::new();
        for (_, v) in there.cases() {
            if let Val::Ref(whole) = v {
                belongs.insert(*whole, options.iter().map(|o| o.0).filter(|o| cx.outgoing_of(*o, PART_OF).contains(whole)).collect());
            }
        }
        left = left.zip(&there, |left, v| match v {
            Val::Ref(whole) => left.intersection(&belongs[whole]).copied().collect(),
            _ => BTreeSet::new(),
        });
    }
    // And not whatever each choice this excludes lands on.
    for other in cx.targets(EXCLUDES) {
        let Some(there) = cx.get::<Chosen>(other)? else { continue };
        left = left.zip(&there.0.0, |left, v| {
            let mut left = left.clone();
            if let Val::Ref(e) = v {
                left.remove(e);
            }
            left
        });
    }
    let name = cx.id().to_string();
    let mut on = space.pure(Val::Undefined);
    let mut at_rest = space.always();
    for (when, left) in left.cases() {
        let left: Vec<(String, f64, bool)> = options.iter().filter(|o| left.contains(&o.0)).map(|(o, w, gap)| (o.to_string(), *w, *gap)).collect();
        let here = match left.as_slice() {
            [] => continue,
            [(only, _, _)] => space.pure(Val::Ref(only.parse().expect("options are handles"))),
            _ => {
                // Every one has this decision's name, so pinning it pins whichever is in use.
                let var = space.var(&name, &left);
                at_rest = &at_rest & &(&when | &var.is(&left[0].0));
                var.value().map(|o| Val::Ref(o.parse().expect("options are handles")))
            }
        };
        on = Cond::when(&when, &here, &on);
    }
    Ok(Some(Falls { on: Worldly(on), at_rest }))
}

/// The options a decision lists by name, in its `options`, rather than as nodes linked to it: each is the
/// node nobody wrote that has that name, as a puzzle's colours are.
fn listed(cx: &Cx) -> Result<Vec<Entity>, Error> {
    let Some(Options(names)) = cx.own::<Options>()? else { return Ok(vec![]) };
    let numeric = formulas::numeric(&names);
    Ok(names.iter().map(|o| cx.constant(formulas::option_cells(o, numeric))).collect())
}

/// The same, asked from outside.
pub(crate) fn listed_in(g: &Graph, d: Entity) -> Vec<Entity> {
    let Ok(Some(Options(names))) = g.get::<Options>(d) else { return vec![] };
    let numeric = formulas::numeric(&names);
    names.iter().map(|o| formulas::option_node(g, o, numeric)).collect()
}

fn decision_chosen(cx: &Cx) -> Cell<Chosen> {
    Ok(cx.own::<Falls>()?.map(|f| Chosen(f.on)))
}

/// Settling a decision whose options depend on other choices is finding out how it fell, which says
/// something about them: only the worlds where it would have fallen that way count.
fn settled_as_it_fell(g: &Graph, e: Entity) -> Cell<Bool> {
    if g.stored(e, Chosen::NAME).is_none() {
        return Ok(None);
    }
    let (Some(chosen), Some(falls)) = (g.get::<Chosen>(e)?, g.get::<Falls>(e)?) else { return Ok(None) };
    Ok(chosen.settled().map(|on| Chosen(falls.on).lands_on(on)))
}

/// The worlds in which `decision` lands on `option`.
pub(crate) fn selects(cx: &Cx, decision: Entity, option: Entity) -> Result<Bool, Error> {
    Ok(match cx.get::<Chosen>(decision)? {
        Some(chosen) => chosen.lands_on(option),
        None => Space::global().never(),
    })
}

/// What a decision has been settled on, if anything: written in, not worked out.
pub(crate) fn settled_on(cx: &Cx, decision: Entity) -> Result<Option<Entity>, Error> {
    if !cx.is_stored(decision, Chosen::NAME) {
        return Ok(None);
    }
    Ok(cx.get::<Chosen>(decision)?.and_then(|c| c.settled()))
}

/// Parents, each with the worlds in which this node is in play through it. Inheriting from a parent only
/// makes sense there: shared work doesn't wait on a phase this world isn't doing.
pub(crate) fn in_play_parents(cx: &Cx) -> Result<Vec<(Entity, Bool)>, Error> {
    let mut out = Vec::new();
    for p in cx.targets(PART_OF) {
        out.push((p, cx.value::<Need>(p)?.0));
    }
    for (d, edge) in cx.outgoing(ALTERNATIVE) {
        // Need before choice, so that an outer decision gets its variable first.
        let need = cx.value::<Need>(d)?.0;
        let via = if cx.get::<RuledOut>(edge)?.is_some() { Space::global().never() } else { &need & &selects(cx, d, cx.id())? };
        out.push((d, via));
    }
    Ok(out)
}

fn need(cx: &Cx) -> Cell<Need> {
    let space = Space::global();
    let dropped = dropped_when(cx, cx.id())?;
    if dropped.is_always() {
        return Ok(Some(Need(space.never())));
    }
    let parents = in_play_parents(cx)?;
    let through = if parents.is_empty() { space.always() } else { space.any(parents.into_iter().map(|(_, via)| via)) };
    Ok(Some(Need(&through & &!&dropped)))
}

/// The parts that are still in the picture.
pub(crate) fn live_parts(cx: &Cx) -> Result<Vec<Entity>, Error> {
    let mut out = Vec::new();
    for p in cx.sources(PART_OF) {
        if is_live(cx, p)? {
            out.push(p);
        }
    }
    Ok(out)
}

/// A gap has no likelihood of its own unless someone gives it one.
const WEIGHT: &str = "if(has(from.unknown), 0, 1)";

/// A decision that leans one way: what's preferred for now, which isn't deciding.
pub const LEANING: &str = "leaning";

/// Which of a decision's options is preferred for now, as `{ref: handle}`. Only ever one, as `chosen`
/// is. Leaning doesn't narrow anything: a read or a view looks at it only through a formula, such as
/// `all(d in with.leaning: d.chosen = d.leaning)`.
#[cell("leaning")]
pub struct Leans(pub serde_json::Value);

/// Why it leans that way, which goes with it in the files, where the log's reasons don't.
#[cell("leaning_why")]
pub struct LeaningWhy(pub String);

/// How to use what this system gives, for an agent: see `mcp.rs` in the server.
pub const GUIDE: &str = "\
A design or a plan is made coarse to fine. `part_of` edges point from a part to what it's part of, and \
all of a node's parts are needed. It gives both ends `part`, so don't write that yourself. A `decision` \
node's options are ordinary nodes with an `alternative` edge to it, and only the option taken is needed: \
what's beneath an option that isn't taken is needed in no world. Something several options share is \
part_of each of them. Decide with {op: set, id: <decision>, cell: chosen, value: {ref: <option's handle>}}, \
and rule an option out with {op: set, from: <option>, to: <decision>, cell: ruled_out, value: \"why\"}. \
Give a decision `uncertain` when it isn't anyone's to choose: its options' weights then say how far each is \
believed. `dropped` means deliberately out of the picture: prefer it to deleting. To model a design, make a \
node for each kind of thing (Concept, Format, Interface) with the cells its instances share, such as \
`layer`, link each instance to its kind by `is_a`, and break each concept into finer ones with \
`part_of`. Read what's beneath a node with `read {nodes: {tree: {root: <node>}}}`.";

/// Parts, decisions and options, and what must hold of them.
pub fn install(reg: &mut Registry) {
    formulas::install(reg);
    if reg.component_def(PART).is_some() {
        return;
    }
    let was = reg.current_system().to_string();
    reg.system(NAME);

    reg.component(PART, "Part of something, with parts, or an option: needed in some worlds.");
    reg.cell::<Need>(PART, "The worlds where this is part of what's being attempted.").builtin(need).shown(|n| Shown::Bool(n.0.clone())).summary();
    reg.cell::<Dropped>(PART, "Deliberately out of the picture.").compiles(compiler::<Dropped>(Type::Bool, |c| formulas::flag_cases(c, "dropped").map(Dropped))).default_formula("false").shown(|d| Shown::cond(&d.0.0)).truth().quiet();

    reg.component(DECISION, "An open question whose options are linked to it. Until one is chosen, it lands on each of them in some world.").icon("⑂").requires(PART).requires(CHOICE).per_entity();
    reg.cell::<Falls>(DECISION, "").builtin(falls).internal();
    reg.fill::<Chosen>(DECISION, decision_chosen);
    reg.component(NARROWED, "A decision whose options depend on how other choices land.").requires(DECISION).constraint(settled_as_it_fell);
    reg.component(UNCERTAIN, "Not anyone's choice: it lands one way or another, as likely as its options' weights say. Expected values average over it.").requires(DECISION);
    reg.component(OPTION, "One of a decision's options: only the option taken is needed.").requires(PART);

    reg.edge(PART_OF, "Part of that: all of a node's parts are needed.").arranges().source_has(PART).target_has(PART);
    reg.edge(ALTERNATIVE, "One of that decision's options.").arranges().source_has(OPTION).target_has(DECISION);
    reg.cell::<Weight>(ALTERNATIVE, "How far this option is believed, before any evidence, against the others.")
        .compiles(compiler::<Weight>(Type::Number, |c| formulas::plain_number_cases(c, "a weight").map(Weight)))
        .default_formula(WEIGHT)
        .initial(json!(1.0));
    reg.cell::<RuledOut>(ALTERNATIVE, "That this option has been ruled out, and why.").initial(json!(true));
    reg.edge(WITHIN, "Only the options that are part of whatever that choice lands on are options here, so this comes after it.").arranges().reversed().source_has(NARROWED).target_has(CHOICE);
    reg.edge(KNOWING, "This choice is made knowing how that turns out, so what's required of it is judged for each way that could.").arranges().reversed().source_has(DECISION);
    reg.edge(EXCLUDES, "Whatever that choice lands on isn't an option here, so this comes after it.").arranges().reversed().source_has(NARROWED).target_has(CHOICE);

    reg.component(LEANING, "Leans towards one option for now, without deciding.").icon("↗").requires(DECISION).per_entity();
    reg.cell::<Leans>(LEANING, "The option preferred for now. Name one, as {\"ref\": handle}.")
        .validate(formulas::one_node)
        // It reads as the option it names, as chosen does.
        .shown(|l| match l.0.get("ref").and_then(|r| r.as_str()).and_then(|r| r.parse::<recollect_graph::Entity>().ok()) {
            Some(o) => Shown::Named(vec![(o, recollect_graph::worlds::Space::global().always())]),
            None => Shown::Cond(recollect_graph::worlds::Space::global().pure(l.0.clone())),
        })
        .summary()
        .per_entity();
    reg.cell::<LeaningWhy>(LEANING, "Why it leans that way.").initial(json!("")).per_entity();

    checks(reg);
    reg.system(&was);
}

fn checks(reg: &mut Registry) {
    use Severity::{Invariant, Notice};

    reg.check(PART_OF, "one_decomposition_per_link", Invariant, |cx| {
        let (_, to) = cx.ends().expect("an edge");
        Ok(if cx.own_has(ALTERNATIVE) { vec![format!("its edge to {to} is both part_of and alternative")] } else { vec![] })
    });

    reg.check(DECISION, "chosen_must_be_a_live_option", Invariant, |cx| {
        let Some(c) = settled_on(cx, cx.id())? else { return Ok(vec![]) };
        let ok = match cx.incoming(ALTERNATIVE).into_iter().find(|(o, _)| *o == c) {
            Some((o, edge)) => live_option(cx, o, edge)?,
            None => listed(cx)?.contains(&c),
        };
        Ok(if ok { vec![] } else { vec![format!("chose {c}, which isn't a live option")] })
    });

    reg.check(DECISION, "decision_without_options", Notice, |cx| {
        if cx.own_is_stored(Chosen::NAME) || !is_live(cx, cx.id())? {
            return Ok(vec![]);
        }
        for (o, edge) in cx.incoming(ALTERNATIVE) {
            if live_option(cx, o, edge)? {
                return Ok(vec![]);
            }
        }
        if !listed(cx)?.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec!["open, with no live options".to_string()])
    });

    reg.check(DECISION, "one_option_left", Notice, |cx| {
        if cx.own_is_stored(Chosen::NAME) || !is_live(cx, cx.id())? {
            return Ok(vec![]);
        }
        let (mut live, mut gaps) = (listed(cx)?.len(), 0);
        for (o, edge) in cx.incoming(ALTERNATIVE) {
            if live_option(cx, o, edge)? {
                live += 1;
                gaps += usize::from(cx.has(o, UNKNOWN));
            }
        }
        // A gap can't be chosen, and alongside one there may be more to come.
        Ok(if live == 1 && gaps == 0 { vec!["only one option is left: choose it, or add alternatives".to_string()] } else { vec![] })
    });

    reg.check(PART, "dropped_part_of_live_work", Notice, |cx| {
        if is_live(cx, cx.id())? {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for p in cx.targets(PART_OF) {
            if !cx.value::<Need>(p)?.0.is_never() {
                out.push(format!("dropped, but {p} is still in play and no longer requires it"));
            }
        }
        Ok(out)
    });


    reg.check(LEANING, "leaning_must_be_a_live_option", Notice, |cx| {
        let Some(Leans(v)) = cx.own::<Leans>()? else { return Ok(vec![]) };
        let Ok(crate::expr::Val::Ref(o)) = serde_json::from_value::<crate::expr::Val>(v) else { return Ok(vec![]) };
        let ok = match cx.incoming(ALTERNATIVE).into_iter().find(|(x, _)| *x == o) {
            Some((x, edge)) => live_option(cx, x, edge)?,
            None => listed(cx)?.contains(&o),
        };
        Ok(if ok { vec![] } else { vec![format!("leans towards {o}, which isn't a live option of it")] })
    });
}

/// Take an option.
pub fn choose(decision: Entity, option: Entity) -> Op {
    Op::set(decision, Chosen::on(option))
}

/// Take a decision back. Nothing beneath it was ever touched, so it all comes back on its own.
pub fn reopen(decision: Entity) -> Op {
    Op::Set { on: recollect_graph::On::Node { id: decision }, cell: Chosen::NAME.to_string(), to: None }
}

/// Whether a decision is open: it's a decision, and nothing's been chosen.
pub(crate) fn is_open(g: &Graph, d: Entity) -> Result<bool, Error> {
    Ok(g.has(d, DECISION) && g.stored(d, Chosen::NAME).is_none())
}

// ---------- questions about the graph as a whole ----------

/// An open decision whose options depend on how other choices land. It isn't one variable, so what it
/// lands on is asked of it rather than of a variable.
#[derive(Debug, Clone)]
pub struct Narrowed {
    pub id: Entity,
    /// Each live option, whether it's a gap, and the worlds where the decision lands on it.
    pub options: Vec<(Entity, bool, Bool)>,
}

pub fn narrowed(g: &Graph) -> Result<Vec<Narrowed>, Error> {
    let mut out = Vec::new();
    for d in g.having(NARROWED) {
        if !is_open(g, d)? {
            continue;
        }
        let Some(falls) = g.get::<Falls>(d)? else { continue };
        let chosen = Chosen(falls.on);
        let mut options = Vec::new();
        for (o, edge) in g.edges_into(d, ALTERNATIVE) {
            if g.get::<RuledOut>(edge)?.is_none() && !is_dropped(g, o)? {
                options.push((o, g.has(o, UNKNOWN), chosen.lands_on(o)));
            }
        }
        options.extend(listed_in(g, d).into_iter().map(|o| (o, false, chosen.lands_on(o))));
        out.push(Narrowed { id: d, options });
    }
    Ok(out)
}

/// Where the variables of every decision whose options depend on other choices are at rest, or nothing
/// if there are no such decisions. See `Falls::at_rest`.
pub fn at_rest(g: &Graph) -> Option<Bool> {
    let mut out = None;
    for d in g.having(NARROWED) {
        if let Ok(Some(falls)) = g.get::<Falls>(d) {
            out = Some(out.map_or(falls.at_rest.clone(), |all: Bool| &all & &falls.at_rest));
        }
    }
    out
}

/// The variables a world pins. A decision whose options depend on other choices isn't among them: how
/// likely it was to land where it's pinned differs between worlds, which fixing its variables would
/// lose, so it's `pinned` that narrows to it.
pub fn picks(g: &Graph, world: &BTreeMap<String, String>) -> Picks {
    let narrowed: BTreeSet<String> = g.having(NARROWED).into_iter().map(|d| d.to_string()).collect();
    if narrowed.is_empty() {
        return Space::global().picks(world);
    }
    Space::global().picks(&world.iter().filter(|(d, _)| !narrowed.contains(*d)).map(|(d, o)| (d.clone(), o.clone())).collect())
}

/// What pinning these decisions to an option each leaves, beyond what pinning their variables does: the
/// worlds where each lands on its option. A decision that isn't narrowed is all in its variable.
pub fn pinned(g: &Graph, world: &BTreeMap<String, String>) -> Bool {
    let mut out = Space::global().always();
    for n in narrowed(g).unwrap_or_default() {
        if let Some(picked) = world.get(&n.id.to_string())
            && let Some((_, _, lands)) = n.options.iter().find(|o| o.0.to_string() == *picked)
        {
            out = &out & lands;
        }
    }
    out
}

/// How a world says an option is ruled out, only for looking: `!choice/option`, beside the choices it
/// pins. Nothing about it is saved, unlike ruling an option out on its edge.
pub fn ruled_out_key(choice: &str, option: &str) -> String {
    format!("!{choice}/{option}")
}

/// The options a world rules out, as (choice, option).
pub fn ruled_out(world: &BTreeMap<String, String>) -> Vec<(String, String)> {
    world.keys().filter_map(|k| k.strip_prefix('!')?.split_once('/').map(|(c, o)| (c.to_string(), o.to_string()))).collect()
}

/// The worlds a world's ruled-out options leave: those where none of them is what its choice lands on.
pub fn not_ruled_out(g: &Graph, world: &BTreeMap<String, String>) -> Bool {
    let out_of = ruled_out(world);
    if out_of.is_empty() {
        return Space::global().always();
    }
    let narrowed = narrowed(g).unwrap_or_default();
    let mut left = Space::global().always();
    for (choice, option) in out_of {
        let lands = match narrowed.iter().find(|n| n.id.to_string() == choice) {
            Some(n) => n.options.iter().find(|o| o.0.to_string() == option).map_or_else(|| Space::global().never(), |o| o.2.clone()),
            None => Space::global().lands(&choice, &option),
        };
        left = &left & &!&lands;
    }
    left
}
