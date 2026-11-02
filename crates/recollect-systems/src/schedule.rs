//! Dates, in every world, over the planning system's own facts: a task's estimate is its duration,
//! and it starts once what it depends on has finished and whatever it's in play through has
//! started. Slack comes from following the same links backward.
//!
//! All three are `None` where a gap makes them unknowable, because you can't date what nobody has
//! specified. Finished work still takes up its time: this is the plan, not a record of what happened.

use recollect_graph::worlds::{Bool, Cond, Space};
use recollect_graph::{Compute, Cx, Error, Registry, component, rule};
use serde::{Deserialize, Serialize};

use crate::plan::{Alternative, Choice, DependsOn, Estimate, Need, PartOf, Task, Unknown, in_play_parents, is_live, live_parts};
use crate::{Catalog, Shown};

/// When it starts, per world. `None` where a gap makes that unknowable.
#[component]
pub struct Start(pub Cond<Option<f64>>);

/// When it finishes, per world. Where it isn't needed it finishes at 0, so waiting on it costs nothing.
#[component]
pub struct Finish(pub Cond<Option<f64>>);

/// How late it could start without moving the end of the plan.
#[component]
pub struct LatestStart(pub Cond<Option<f64>>);

/// How late it could finish without moving the end of the plan.
#[component]
pub struct LatestFinish(pub Cond<Option<f64>>);

/// How far it could slip without moving the end of the plan. `None` where it isn't needed.
#[component]
pub struct Slack(pub Cond<Option<f64>>);

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

fn later(a: &Option<f64>, b: &Option<f64>) -> Option<f64> {
    Some(a.as_ref()?.max(*b.as_ref()?))
}

fn earlier(a: &Option<f64>, b: &Option<f64>) -> Option<f64> {
    Some(a.as_ref()?.min(*b.as_ref()?))
}

/// How long it takes in each world. With no estimate that's nothing, and an estimate a formula
/// can't work out makes the dates unknowable.
fn own_duration(cx: &Cx) -> Result<Cond<Option<f64>>, Error> {
    Ok(cx.own::<Estimate>()?.map_or_else(|| Space::global().pure(Some(0.0)), |e| e.0.0))
}

/// Which option a decision's work is under, for a node that's a decision.
fn selected(choice: &Cond<Option<recollect_graph::Entity>>, option: recollect_graph::Entity) -> Bool {
    choice.holds(|c| *c == Some(option))
}

#[rule("schedule/start", outgoing(DependsOn, PartOf, Alternative))]
pub struct StartRule;

impl Compute for StartRule {
    type Output = Start;
    fn compute(&self, cx: &Cx) -> Result<Start, Error> {
        let space = Space::global();
        let mut s = space.pure(Some(0.0));
        for d in cx.targets::<DependsOn>()? {
            if is_live(cx, d)? {
                s = s.zip(&cx.value::<Finish>(d)?.0, later);
            }
        }
        for (p, via) in in_play_parents(cx)? {
            let inherited = Cond::when(&via, &cx.value::<Start>(p)?.0, &space.pure(Some(0.0)));
            s = s.zip(&inherited, later);
        }
        Ok(Start(s))
    }
}

#[rule("schedule/finish", incoming(PartOf, Alternative))]
pub struct FinishRule;

impl Compute for FinishRule {
    type Output = Finish;
    fn compute(&self, cx: &Cx) -> Result<Finish, Error> {
        let space = Space::global();
        let start = cx.own_value::<Start>()?.0;
        let parts = live_parts(cx)?;
        let choice = cx.own::<Choice>()?;
        let mut real = if cx.own::<Unknown>()?.is_some() {
            space.pure(None)
        } else if parts.is_empty() && choice.is_none() {
            start.zip(&own_duration(cx)?, |s, d| Some(s.as_ref()? + d.as_ref()?))
        } else {
            start.clone()
        };
        for p in parts {
            real = real.zip(&cx.value::<Finish>(p)?.0, later);
        }
        if let Some(choice) = choice {
            let mut under = space.pure(Some(0.0));
            for (o, alt) in cx.incoming::<Alternative>()? {
                if alt.ruled_out.is_none() && is_live(cx, o)? {
                    under = Cond::when(&selected(&choice.0, o), &cx.value::<Finish>(o)?.0, &under);
                }
            }
            real = real.zip(&under, later);
        }
        Ok(Finish(Cond::when(&cx.own_value::<Need>()?.0, &real, &space.pure(Some(0.0)))))
    }
}

/// The end of the whole plan: when the last task finishes.
fn end_of_plan(cx: &Cx) -> Result<Cond<Option<f64>>, Error> {
    let mut end = Space::global().pure(Some(0.0));
    for (t, _) in cx.with::<Task>()? {
        end = end.zip(&cx.value::<Finish>(t)?.0, later);
    }
    Ok(end)
}

#[rule("schedule/latest_finish", incoming(DependsOn), outgoing(PartOf, Alternative), scans(Task))]
pub struct LatestFinishRule;

impl Compute for LatestFinishRule {
    type Output = LatestFinish;
    fn compute(&self, cx: &Cx) -> Result<LatestFinish, Error> {
        let space = Space::global();
        let mut latest = end_of_plan(cx)?;
        let free = space.pure(Some(f64::INFINITY));
        for after in cx.sources::<DependsOn>()? {
            let need = cx.value::<Need>(after)?.0;
            let by = Cond::when(&need, &cx.value::<LatestStart>(after)?.0, &free);
            latest = latest.zip(&by, earlier);
        }
        for (p, via) in in_play_parents(cx)? {
            let by = Cond::when(&via, &cx.value::<LatestFinish>(p)?.0, &free);
            latest = latest.zip(&by, earlier);
        }
        Ok(LatestFinish(latest))
    }
}

#[rule("schedule/latest_start", incoming(PartOf, Alternative))]
pub struct LatestStartRule;

impl Compute for LatestStartRule {
    type Output = LatestStart;
    fn compute(&self, cx: &Cx) -> Result<LatestStart, Error> {
        let space = Space::global();
        let parts = live_parts(cx)?;
        let choice = cx.own::<Choice>()?;
        if parts.is_empty() && choice.is_none() {
            return Ok(LatestStart(cx.own_value::<LatestFinish>()?.0.zip(&own_duration(cx)?, |l, d| Some(l.as_ref()? - d.as_ref()?))));
        }
        // A summary has to start in time for whichever part has to start first.
        let free = space.pure(Some(f64::INFINITY));
        let mut latest = free.clone();
        for p in parts {
            let need = cx.value::<Need>(p)?.0;
            let by = Cond::when(&need, &cx.value::<LatestStart>(p)?.0, &free);
            latest = latest.zip(&by, earlier);
        }
        if let Some(choice) = choice {
            for (o, alt) in cx.incoming::<Alternative>()? {
                if alt.ruled_out.is_none() && is_live(cx, o)? {
                    let by = Cond::when(&selected(&choice.0, o), &cx.value::<LatestStart>(o)?.0, &free);
                    latest = latest.zip(&by, earlier);
                }
            }
        }
        // With nothing inside it to run out of time, it's only as constrained as its own end.
        let own = cx.own_value::<LatestFinish>()?.0;
        Ok(LatestStart(latest.zip(&own, earlier)))
    }
}

#[rule("schedule/slack")]
pub struct SlackRule;

impl Compute for SlackRule {
    type Output = Slack;
    fn compute(&self, cx: &Cx) -> Result<Slack, Error> {
        let gap = cx.own_value::<LatestFinish>()?.0.zip(&cx.own_value::<Finish>()?.0, |l, f| Some(l.as_ref()? - f.as_ref()?));
        Ok(Slack(Cond::when(&cx.own_value::<Need>()?.0, &gap, &Space::global().pure(None))))
    }
}

#[rule("schedule/dates")]
pub struct DatesRule;

impl Compute for DatesRule {
    type Output = Dates;
    fn compute(&self, cx: &Cx) -> Result<Dates, Error> {
        let span = cx.own_value::<Start>()?.0.zip(&cx.own_value::<Finish>()?.0, |s, f| (*s, *f)).zip(&cx.own_value::<Slack>()?.0, |(start, finish), slack| Some(Span { start: *start, finish: *finish, slack: *slack }));
        Ok(Dates(Cond::when(&cx.own_value::<Need>()?.0, &span, &Space::global().pure(None))))
    }
}

pub fn install(reg: &mut Registry) {
    reg.set_default(StartRule).set_default(FinishRule).set_default(LatestFinishRule).set_default(LatestStartRule).set_default(SlackRule).set_default(DatesRule);
}

pub fn describe(c: &mut Catalog) {
    c.worldly::<Start>("When it starts, in each world.", |s| Shown::cond(&s.0));
    c.worldly::<Finish>("When it finishes, in each world.", |f| Shown::cond(&f.0));
    c.worldly::<Slack>("How far it could slip without moving the end of the plan.", |s| Shown::cond(&s.0));
    c.worldly::<Dates>("When it happens, in the worlds that need it.", |d| Shown::cond(&d.0)).quiet = true;
    c.internal::<LatestStart>();
    c.internal::<LatestFinish>();
}
