//! Structure: what a design or a plan is made of, coarse to fine, and decisions held open. A part is
//! part of something, or has parts, or is an option, and it's needed in some worlds: those where what
//! it's part of is, and, under a decision, where that decision lands on it. Nothing here is work. `plan`
//! adds what's owed, what's done and what waits, on top of this.
//!
//! What's worked out here is written in `plan.rs` for now, beside what reads it there.

use serde_json::json;

use recollect_graph::{CellType, Registry, Severity, Shown};

use crate::expr::Type;
use crate::formulas::{self, CHOICE, Chosen, compiler};
pub use crate::plan::{ALTERNATIVE, DECISION, Dropped, EXCLUDES, KNOWING, NARROWED, Need, OPTION, PART, PART_OF, RuledOut, UNCERTAIN, WITHIN, Weight};
use crate::plan::{Falls, UNKNOWN, WEIGHT, decision_chosen, falls, is_live, listed, live_option, need, settled_as_it_fell, settled_on};

pub const NAME: &str = "structure";

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
}
