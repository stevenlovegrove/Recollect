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
use recollect_graph::{Compute, Component, Cx, Error, Registry, component};
use serde::{Deserialize, Serialize};

use crate::expr::{Type, Val};
use crate::formulas::{defaults_to, number_cases};
use crate::plan::Need;
use crate::{Catalog, Shown};

/// When it starts, per world. `None` where a gap makes that unknowable.
#[component]
pub struct Start(pub Worldly<Option<f64>>);

/// When it finishes, per world. Where it isn't needed it finishes at 0, so waiting on it costs nothing.
#[component]
pub struct Finish(pub Worldly<Option<f64>>);

/// How late it could start without moving the end of the plan.
#[component]
pub struct LatestStart(pub Worldly<Option<f64>>);

/// How late it could finish without moving the end of the plan.
#[component]
pub struct LatestFinish(pub Worldly<Option<f64>>);

/// How far it could slip without moving the end of the plan. `None` where it isn't needed.
#[component]
pub struct Slack(pub Worldly<Option<f64>>);

/// When something happens, in one world.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: Option<f64>,
    pub finish: Option<f64>,
    pub slack: Option<f64>,
}

/// When it happens, in the worlds that need it, and `None` in those that don't: what to draw, where
/// `Finish` is what to wait for.
#[component]
pub struct Dates(pub Cond<Option<Span>>);

/// After what it depends on, and no earlier than whatever it's in play through.
const START: &str = "max(p in out.part_of: if(p.need, p.start, 0), max(d in out.alternative: if(d.need && chose(d, self), d.start, 0), max(x in out.depends_on: x.finish, 0)))";

/// When its own time is up, or its last part is. Where it isn't needed that's 0, so waiting on it
/// costs nothing.
const FINISH: &str = "if(!self.need, 0, max(max(p in in.part_of: p.finish, if(has(self.unknown), undefined, if(is_leaf(), self.start + days(), self.start))), pick(o in in.alternative: o.finish, 0)))";

const LATEST_FINISH: &str = "latest_given(plan_end())";

/// A summary has to start in time for whichever part has to start first. With nothing inside it to
/// run out of time, it's only as constrained as its own end.
const LATEST_START: &str = "if(is_leaf(), self.latest_finish - days(), min(p in in.part_of: if(p.need, p.latest_start, self.latest_finish), min(o in in.alternative: if(chose(self, o), o.latest_start, self.latest_finish), self.latest_finish)))";

const SLACK: &str = "if(self.need, self.latest_finish - self.finish, undefined)";

#[recollect_graph::rule("schedule/dates")]
pub struct DatesRule;

impl Compute for DatesRule {
    type Output = Dates;
    fn compute(&self, cx: &Cx) -> Result<Dates, Error> {
        let span = cx.own_value::<Start>()?.0.0.zip(&cx.own_value::<Finish>()?.0.0, |s, f| (*s, *f)).zip(&cx.own_value::<Slack>()?.0.0, |(start, finish), slack| Some(Span { start: *start, finish: *finish, slack: *slack }));
        Ok(Dates(Cond::when(&cx.own_value::<Need>()?.0, &span, &Space::global().pure(None))))
    }
}

fn number<C: Component>(reg: &mut Registry, wrap: fn(Cond<Val>) -> Result<C, Error>, text: &str) {
    defaults_to::<C>(reg, Type::Number, wrap, text);
}

pub fn install(reg: &mut Registry) {
    number::<Start>(reg, |c| number_cases(c, "a start").map(Start), START);
    number::<Finish>(reg, |c| number_cases(c, "a finish").map(Finish), FINISH);
    number::<LatestFinish>(reg, |c| number_cases(c, "a latest finish").map(LatestFinish), LATEST_FINISH);
    number::<LatestStart>(reg, |c| number_cases(c, "a latest start").map(LatestStart), LATEST_START);
    number::<Slack>(reg, |c| number_cases(c, "slack").map(Slack), SLACK);
    reg.set_default(DatesRule);
}

pub fn describe(c: &mut Catalog) {
    c.worldly::<Start>("When it starts, in each world.", |s| Shown::cond(&s.0.0));
    c.worldly::<Finish>("When it finishes, in each world.", |f| Shown::cond(&f.0.0));
    c.worldly::<Slack>("How far it could slip without moving the end of the plan.", |s| Shown::cond(&s.0.0));
    c.worldly::<Dates>("When it happens, in the worlds that need it.", |d| Shown::cond(&d.0)).quiet = true;
    c.internal::<LatestStart>();
    c.internal::<LatestFinish>();
}
