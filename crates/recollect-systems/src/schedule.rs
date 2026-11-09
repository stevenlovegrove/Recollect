//! Dates, in every world, over the planning system's own facts: a task's estimate is its duration,
//! and it starts once what it depends on has finished and whatever it's in play through has
//! started. Slack comes from following the same links backward.
//!
//! All three are undefined where a gap makes them unknowable, because you can't date what nobody has
//! specified. Finished work still takes up its time: this is the plan, not a record of what happened.
//!
//! Each of these is a formula any node can type over, with nothing here that a formula on some other
//! cell couldn't do.

use recollect_graph::worlds::{Cond, Space, Worldly};
use recollect_graph::{Cx, Error, Registry, Shown, cell};
use serde::{Deserialize, Serialize};

use crate::expr::Type;
use crate::formulas::{compiler, number_cases};
use crate::plan::{Need, PART};

/// The graph as a whole, as a plan: when it ends.
pub const SCHEDULE: &str = "schedule";

/// When it starts, per world. `None` where a gap makes that unknowable.
#[cell]
pub struct Start(pub Worldly<Option<f64>>);

/// When it finishes, per world. Where it isn't needed it finishes at 0, so waiting on it costs nothing.
#[cell]
pub struct Finish(pub Worldly<Option<f64>>);

/// How late it could start without moving the end of the plan.
#[cell]
pub struct LatestStart(pub Worldly<Option<f64>>);

/// How late it could finish without moving the end of the plan.
#[cell]
pub struct LatestFinish(pub Worldly<Option<f64>>);

/// How far it could slip without moving the end of the plan. `None` where it isn't needed.
#[cell]
pub struct Slack(pub Worldly<Option<f64>>);

/// When the last task finishes, for the graph as a whole.
#[cell]
pub struct PlanEnd(pub Worldly<Option<f64>>);

/// When something happens, in one world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: Option<f64>,
    pub finish: Option<f64>,
    pub slack: Option<f64>,
}

/// When it happens, in the worlds that need it, and `None` in those that don't: what to draw, where
/// `Finish` is what to wait for.
#[cell]
pub struct Dates(pub Cond<Option<Span>>);

/// After what it depends on, and no earlier than whatever it's in play through.
const START: &str = "max(p in out.part_of: if(p.need, p.start, 0), max(d in out.alternative: if(d.need && chose(d, self), d.start, 0), max(x in out.depends_on: x.finish, 0)))";

/// When its own time is up, or its last part is. Where it isn't needed that's 0, so waiting on it
/// costs nothing.
const FINISH: &str = "if(!self.need, 0, max(max(p in in.part_of: p.finish, if(has(self.unknown), undefined, if(is_leaf(), self.start + days(), self.start))), if(defined(self.chosen), self.chosen.finish, 0)))";

const PLAN_END: &str = "max(t in with.task: t.finish, 0)";

/// How late it could finish, given what has to follow it. The end of the plan stands for no limit,
/// because nothing can be later than that anyway.
const LATEST_FINISH: &str = "min(p in out.part_of: if(p.need, p.latest_finish, whole.plan_end), min(d in out.alternative: if(d.need && chose(d, self), d.latest_finish, whole.plan_end), min(x in in.depends_on: if(x.need, x.latest_start, whole.plan_end), whole.plan_end)))";

/// A summary has to start in time for whichever part has to start first. With nothing inside it to
/// run out of time, it's only as constrained as its own end.
const LATEST_START: &str = "if(is_leaf(), self.latest_finish - days(), min(p in in.part_of: if(p.need, p.latest_start, self.latest_finish), min(o in in.alternative: if(chose(self, o), o.latest_start, self.latest_finish), self.latest_finish)))";

const SLACK: &str = "if(self.need, self.latest_finish - self.finish, undefined)";

fn dates(cx: &Cx) -> Result<Option<Dates>, Error> {
    let span = cx.own_value::<Start>()?.0.0.zip(&cx.own_value::<Finish>()?.0.0, |s, f| (*s, *f)).zip(&cx.own_value::<Slack>()?.0.0, |(start, finish), slack| Some(Span { start: *start, finish: *finish, slack: *slack }));
    Ok(Some(Dates(Cond::when(&cx.own_value::<Need>()?.0, &span, &Space::global().pure(None)))))
}

pub fn install(reg: &mut Registry) {
    reg.whole(SCHEDULE, "When the plan as a whole ends.");
    reg.cell::<Start>(PART, "When it starts, in each world.").compiles(compiler::<Start>(Type::Number, |c| number_cases(c, "a start").map(Start))).default_formula(START).shown(|s| Shown::cond(&s.0.0));
    reg.cell::<Finish>(PART, "When it finishes, in each world.").compiles(compiler::<Finish>(Type::Number, |c| number_cases(c, "a finish").map(Finish))).default_formula(FINISH).shown(|f| Shown::cond(&f.0.0));
    reg.cell::<PlanEnd>(SCHEDULE, "When the last task finishes.").compiles(compiler::<PlanEnd>(Type::Number, |c| number_cases(c, "the end of a plan").map(PlanEnd))).default_formula(PLAN_END).internal();
    reg.cell::<LatestFinish>(PART, "How late it could finish without moving the end of the plan.").compiles(compiler::<LatestFinish>(Type::Number, |c| number_cases(c, "a latest finish").map(LatestFinish))).default_formula(LATEST_FINISH).internal();
    reg.cell::<LatestStart>(PART, "How late it could start without moving the end of the plan.").compiles(compiler::<LatestStart>(Type::Number, |c| number_cases(c, "a latest start").map(LatestStart))).default_formula(LATEST_START).internal();
    reg.cell::<Slack>(PART, "How far it could slip without moving the end of the plan.").compiles(compiler::<Slack>(Type::Number, |c| number_cases(c, "slack").map(Slack))).default_formula(SLACK).shown(|s| Shown::cond(&s.0.0));
    reg.cell::<Dates>(PART, "When it happens, in the worlds that need it.").builtin(dates).shown(|d| Shown::cond(&d.0)).quiet();
}
