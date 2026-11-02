//! Toy 2: scheduling. Part one is classic critical-path scheduling with no worlds at all, written
//! here and sharing nothing with the schedule system, where the same links are followed forwards
//! for dates and backwards for slack. Part two is the schedule system, holding decisions open, and
//! part one is what checks it: fix a decision and the two must agree.

use std::collections::BTreeMap;

mod support;

use recollect_graph::worlds::Cond;
use recollect_graph::{Compute, Cx, Error, Registry, component, rule};
use serde_json::json;
use recollect_systems::formulas::EXPR;
use recollect_systems::plan::{self, Alternative, Decision, Estimate, Need, Task, Unknown};
use recollect_systems::schedule::{self, Finish as WorldFinish, Slack as WorldSlack, Start as WorldStart};
use support::N;

// ---------- links and stored facts ----------

#[component]
struct DependsOn;

#[component]
struct PartOf;

#[derive(Copy)]
#[component]
struct Duration(f64);

// ---------- part one: a plain schedule ----------

#[derive(Copy)]
#[component]
struct Start(f64);

#[derive(Copy)]
#[component]
struct Finish(f64);

#[derive(Copy)]
#[component]
struct LatestFinish(f64);

#[derive(Copy)]
#[component]
struct Slack(f64);

/// After everything you depend on, and no earlier than whatever you're part of.
#[rule("gantt/start", outgoing(DependsOn, PartOf))]
struct StartRule;

impl Compute for StartRule {
    type Output = Start;
    fn compute(&self, cx: &Cx) -> Result<Start, Error> {
        let mut s = 0.0f64;
        for dep in cx.targets::<DependsOn>()? {
            s = s.max(cx.value::<Finish>(dep)?.0);
        }
        for parent in cx.targets::<PartOf>()? {
            s = s.max(cx.value::<Start>(parent)?.0);
        }
        Ok(Start(s))
    }
}

/// A leaf takes its duration. A summary ends when its last part does.
#[rule("gantt/finish", incoming(PartOf))]
struct FinishRule;

impl Compute for FinishRule {
    type Output = Finish;
    fn compute(&self, cx: &Cx) -> Result<Finish, Error> {
        let s = cx.own_value::<Start>()?.0;
        let parts = cx.incoming::<PartOf>()?;
        if parts.is_empty() {
            return Ok(Finish(s + cx.own::<Duration>()?.map_or(0.0, |d| d.0)));
        }
        let mut f = s;
        for (p, _) in parts {
            f = f.max(cx.value::<Finish>(p)?.0);
        }
        Ok(Finish(f))
    }
}

/// The same links, followed the other way.
#[rule("gantt/latest_finish", incoming(DependsOn), scans(Duration))]
struct LatestFinishRule;

impl Compute for LatestFinishRule {
    type Output = LatestFinish;
    fn compute(&self, cx: &Cx) -> Result<LatestFinish, Error> {
        let mut end = 0.0f64;
        for (t, _) in cx.with::<Duration>()? {
            end = end.max(cx.value::<Finish>(t)?.0);
        }
        let mut latest = end;
        for after in cx.sources::<DependsOn>()? {
            let l = cx.value::<LatestFinish>(after)?.0;
            let d = cx.get::<Duration>(after)?.map_or(0.0, |d| d.0);
            latest = latest.min(l - d);
        }
        Ok(LatestFinish(latest))
    }
}

#[rule("gantt/slack")]
struct SlackRule;

impl Compute for SlackRule {
    type Output = Slack;
    fn compute(&self, cx: &Cx) -> Result<Slack, Error> {
        Ok(Slack(cx.own_value::<LatestFinish>()?.0 - cx.own_value::<Finish>()?.0))
    }
}

fn plain_rules(reg: &mut Registry) {
    reg.set_default(StartRule).set_default(FinishRule).set_default(LatestFinishRule).set_default(SlackRule);
}

fn finish(g: &N, id: &str) -> f64 {
    g.get::<Finish>(id).unwrap().unwrap().0
}

fn slack(g: &N, id: &str) -> f64 {
    g.get::<Slack>(id).unwrap().unwrap().0
}

/// A → {B, C} → D, with C the long way round.
fn textbook() -> N {
    let mut g = N::with(plain_rules);
    for (t, d) in [("a", 3.0), ("b", 2.0), ("c", 4.0), ("d", 2.0)] {
        g.set(t, Duration(d));
    }
    g.link("b", "a", DependsOn).link("c", "a", DependsOn).link("d", "b", DependsOn).link("d", "c", DependsOn);
    g
}

#[test]
fn critical_path_forward_and_backward() {
    let g = textbook();
    assert_eq!(finish(&g, "d"), 9.0);
    g.assert_round_trips();
    assert_eq!(slack(&g, "b"), 2.0, "b could slip by 2");
    for t in ["a", "c", "d"] {
        assert_eq!(slack(&g, t), 0.0, "{t} is on the critical path");
    }
}

#[test]
fn slipping_a_task_with_slack_stops_where_it_stops_mattering() {
    let mut g = textbook();
    finish(&g, "d");
    g.take_ran();
    g.set("b", Duration(3.0));
    assert_eq!(finish(&g, "d"), 9.0);
    let ran = g.ran_cells();
    assert!(ran.contains(&"finish(b)".to_string()) && ran.contains(&"start(d)".to_string()), "{ran:?}");
    assert!(!ran.contains(&"finish(d)".to_string()), "d's start is still 7, so its finish never reruns: {ran:?}");
}

#[test]
fn summary_tasks_roll_up_from_leaves_and_pass_down_their_dependencies() {
    let mut g = textbook();
    g.link("x", "phase", PartOf).link("y", "phase", PartOf).link("phase", "d", DependsOn);
    g.set("x", Duration(1.0)).set("y", Duration(5.0));
    assert_eq!(finish(&g, "phase"), 14.0, "starts after d at 9, and the longest part takes 5");
    assert_eq!(g.get::<Start>("x").unwrap().unwrap().0, 9.0, "x inherits the phase's dependency");
}

#[test]
fn a_dependency_on_your_own_parent_is_a_reported_cycle() {
    let mut g = textbook();
    g.link("b", "phase", PartOf).link("phase", "x", PartOf).link("x", "b", DependsOn);
    g.link("phase", "b", DependsOn);
    assert!(matches!(g.get::<Finish>("b"), Err(Error::Cycle(_))));
    assert_eq!(finish(&g, "a"), 3.0);
}

// ---------- part two: the schedule system, holding decisions open ----------

fn world_rules(reg: &mut Registry) {
    plan::install(reg);
    schedule::install(reg);
}

/// Calibration: three approaches with overlapping work, and a gap.
fn calibration() -> N {
    use plan::{DependsOn, PartOf};
    let mut g = N::with(world_rules);
    g.set("calib", Decision::default());
    for o in ["cad", "free", "board"] {
        g.link(o, "calib", Alternative::default());
    }
    g.set("calib.unknown", Unknown).link("calib.unknown", "calib", Alternative::default());
    for (t, d, parents) in [("imu", 1.0, vec!["calib"]), ("static", 2.0, vec!["cad", "free"]), ("export", 3.0, vec!["cad"]), ("search", 5.0, vec!["free"]), ("detector", 4.0, vec!["board"])] {
        g.set(t, Task {}).set(t, Estimate::days(d));
        for p in parents {
            g.link(t, p, PartOf);
        }
    }
    g.link("export", "static", DependsOn::default());
    g.link("search", "static", DependsOn::default());
    g
}

fn dated(c: &Cond<Option<f64>>, g: &N, o: &str) -> Option<f64> {
    *c.at(&g.world(&[("calib", o)]))
}

#[test]
fn dates_differ_exactly_between_worlds_and_gaps_make_them_unknowable() {
    let g = calibration();
    let end = g.get::<WorldFinish>("calib").unwrap().unwrap().0;
    assert_eq!(dated(&end, &g, "cad"), Some(5.0), "static 2 then export 3");
    assert_eq!(dated(&end, &g, "free"), Some(7.0), "static 2 then search 5");
    assert_eq!(dated(&end, &g, "board"), Some(4.0), "detector alone");
    assert_eq!(dated(&end, &g, "calib.unknown"), None, "you can't date what nobody has specified");
    g.assert_round_trips();
}

#[test]
fn slack_differs_between_worlds_too() {
    let g = calibration();
    let slack = |t: &str| g.get::<WorldSlack>(t).unwrap().unwrap().0;
    assert_eq!(dated(&slack("imu"), &g, "cad"), Some(4.0), "the imu takes 1 of the 5 days that way");
    assert_eq!(dated(&slack("imu"), &g, "free"), Some(6.0));
    assert_eq!(dated(&slack("static"), &g, "cad"), Some(0.0), "it's on the critical path");
    assert_eq!(dated(&slack("detector"), &g, "cad"), None, "it isn't needed, so it has no slack");
    assert_eq!(dated(&slack("detector"), &g, "board"), Some(0.0));
}

#[test]
fn deciding_narrows_the_dates_to_one_value() {
    let mut g = calibration();
    let cad = g.id("cad");
    g.set("calib", Decision { chosen: Some(cad) });
    let end = g.get::<WorldFinish>("calib").unwrap().unwrap().0;
    assert_eq!(end.cases().len(), 1);
    assert_eq!(dated(&end, &g, "free"), Some(5.0), "whichever world you ask about");
}

#[test]
fn shared_work_only_inherits_from_parents_in_play() {
    use plan::{DependsOn, PartOf};
    // `shared` is under both approaches, and only approach a's phase waits on `prereq`.
    let mut g = N::with(world_rules);
    g.set("d", Decision::default());
    g.link("a", "d", Alternative::default()).link("b", "d", Alternative::default());
    g.set("prereq", Task {}).set("prereq", Estimate::days(10.0));
    g.link("a", "prereq", DependsOn::default());
    g.set("shared", Task {}).set("shared", Estimate::days(1.0)).link("shared", "a", PartOf).link("shared", "b", PartOf);
    let start = g.get::<WorldStart>("shared").unwrap().unwrap().0;
    let at = |o: &str| *start.at(&g.world(&[("d", o)]));
    assert_eq!((at("a"), at("b")), (Some(10.0), Some(0.0)));
}

/// A known limit, pinned on purpose: which cells read which is fixed by the graph and doesn't
/// vary between worlds, so a loop spanning two worlds counts even though no single world has it.
#[test]
fn a_loop_that_no_single_world_contains_is_still_reported() {
    use plan::{DependsOn, PartOf};
    let mut g = N::with(world_rules);
    g.set("d", Decision::default());
    g.link("a", "d", Alternative::default()).link("b", "d", Alternative::default());
    g.set("x", Task {}).set("x", Estimate::days(1.0)).link("x", "a", PartOf);
    g.set("y", Task {}).set("y", Estimate::days(1.0)).link("y", "b", PartOf);
    g.link("x", "y", DependsOn::default()).link("y", "x", DependsOn::default());
    assert!(g.get::<Need>("x").is_ok(), "x and y are never both needed");
    assert!(matches!(g.get::<WorldFinish>("x"), Err(Error::Cycle(_))));
}

#[test]
fn memoisation_holds_across_worlds_too() {
    let mut g = calibration();
    g.get::<WorldFinish>("calib").unwrap();
    g.take_ran();
    g.set("detector", Task {}).set("detector", Estimate::days(9.0));
    let end = g.get::<WorldFinish>("calib").unwrap().unwrap().0;
    assert_eq!(dated(&end, &g, "board"), Some(9.0));
    let ran = g.ran_cells();
    assert!(!ran.iter().any(|r| r.contains("export") || r.contains("search") || r.contains("static")), "other approaches don't run: {ran:?}");
    assert!(!ran.iter().any(|r| r.starts_with("need") || r.starts_with("choice")), "what's needed didn't change: {ran:?}");
}

// ---------- the two parts check each other ----------

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.next(100) < pct
    }
}

/// What a generated graph is, independent of either way of computing it.
#[derive(Default)]
struct Plan {
    nodes: Vec<(String, f64)>,
    parts: Vec<(String, String)>,
    options: Vec<(String, String)>,
    deps: Vec<(String, String)>,
    /// A task whose estimate is a formula: this many days if this decision lands on this option, and
    /// its usual number otherwise.
    varying: Vec<(String, String, String, f64)>,
}

fn random_plan(rng: &mut Rng) -> Plan {
    let n = 6 + rng.next(10);
    let ids: Vec<String> = (0..n).map(|i| format!("n{i:02}")).collect();
    let decisions: Vec<usize> = (0..n.min(6)).filter(|_| rng.chance(40)).collect();
    let mut p = Plan::default();
    for id in &ids {
        p.nodes.push((id.clone(), (1 + rng.next(9)) as f64));
    }
    for i in 1..n {
        for _ in 0..(1 + rng.next(2)) {
            let parent = rng.next(i);
            let link = (ids[i].clone(), ids[parent].clone());
            if p.parts.contains(&link) || p.options.contains(&link) {
                continue;
            }
            if decisions.contains(&parent) && rng.chance(80) {
                p.options.push(link);
            } else {
                p.parts.push(link);
            }
        }
    }
    for (id, _) in &ids.iter().map(|i| (i.clone(), ())).collect::<Vec<_>>() {
        if rng.chance(30) && !p.options.is_empty() {
            let (o, d) = p.options[rng.next(p.options.len())].clone();
            if *id != o && *id != d {
                p.varying.push((id.clone(), d, o, (1 + rng.next(9)) as f64));
            }
        }
    }
    // Keep a dependency only if it leaves the plan computable, since most random ones deadlock.
    for _ in 0..n {
        let (a, b) = (rng.next(n), rng.next(n));
        if a != b {
            p.deps.push((ids[a].clone(), ids[b].clone()));
            let g = with_worlds(&p);
            if ids.iter().any(|id| g.get::<WorldFinish>(id).is_err() || g.get::<WorldSlack>(id).is_err()) {
                p.deps.pop();
            }
        }
    }
    p
}

/// With this plan's decisions all open.
fn with_worlds(p: &Plan) -> N {
    let mut g = N::with(world_rules);
    for (id, d) in &p.nodes {
        g.set(id, Task {}).set(id, Estimate::days(*d));
    }
    for (c, par) in &p.parts {
        g.link(c, par, plan::PartOf);
    }
    for (o, d) in &p.options {
        g.link(o, d, Alternative::default());
        g.set(d, Decision::default());
    }
    for (a, b) in &p.deps {
        g.link(a, b, plan::DependsOn::default());
    }
    for (id, d, o, days) in &p.varying {
        let usual = p.nodes.iter().find(|n| n.0 == *id).unwrap().1;
        let e = g.id(id);
        g.g.set_rule(e, "estimate", EXPR, json!({ "text": format!("if(chose(decision, option), {days}, {usual})") }));
        g.input(id, d, "decision").input(id, o, "option");
    }
    g
}

/// With each decision fixed by `world`: unselected options simply aren't in the graph, which is
/// a plain schedule with no idea that worlds exist. A node is kept if it's still reachable, so
/// this mirrors `need` from first principles.
fn plain_in_world(p: &Plan, world: &BTreeMap<String, String>, slower: Option<(&str, f64)>) -> (N, Vec<String>) {
    let mut kept = Vec::new();
    fn is_needed(p: &Plan, id: &str, world: &BTreeMap<String, String>) -> bool {
        let part_parents: Vec<&String> = p.parts.iter().filter(|(c, _)| c == id).map(|(_, x)| x).collect();
        let opt_parents: Vec<&String> = p.options.iter().filter(|(o, _)| o == id).map(|(_, d)| d).collect();
        if part_parents.is_empty() && opt_parents.is_empty() {
            return true;
        }
        part_parents.iter().any(|x| is_needed(p, x, world)) || opt_parents.iter().any(|d| world.get(*d).is_some_and(|c| c == id) && is_needed(p, d, world))
    }
    for (id, _) in &p.nodes {
        if is_needed(p, id, world) {
            kept.push(id.clone());
        }
    }
    let mut g = N::with(plain_rules);
    for (id, d) in &p.nodes {
        if kept.contains(id) {
            let extra = slower.filter(|(s, _)| *s == id).map_or(0.0, |(_, by)| by);
            let here = p.varying.iter().find(|(v, decision, option, _)| v == id && world.get(decision) == Some(option)).map_or(*d, |v| v.3);
            g.set(id, Duration(here + extra));
        }
    }
    for (c, par) in &p.parts {
        if kept.contains(c) && kept.contains(par) {
            g.link(c, par, PartOf);
        }
    }
    for (o, d) in &p.options {
        if kept.contains(o) && kept.contains(d) && world.get(d) == Some(o) {
            g.link(o, d, PartOf);
        }
    }
    for (a, b) in &p.deps {
        if kept.contains(a) && kept.contains(b) {
            g.link(a, b, DependsOn);
        }
    }
    (g, kept)
}

fn end_of(g: &N, kept: &[String]) -> f64 {
    kept.iter().map(|id| finish(g, id)).fold(0.0, f64::max)
}

#[test]
fn every_world_agrees_with_a_plain_schedule_of_that_world() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let (mut compared, mut slack_checked, mut with_choices, mut round_trips, mut has_slack, mut varying) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for _ in 0..300 {
        let p = random_plan(&mut rng);
        varying += p.varying.len();
        let g = with_worlds(&p);
        g.assert_round_trips();
        round_trips += 1;
        let ids: Vec<String> = p.nodes.iter().map(|n| n.0.clone()).collect();
        let decisions: Vec<String> = ids.iter().filter(|id| p.options.iter().any(|(_, d)| d == *id)).cloned().collect();
        if !decisions.is_empty() {
            with_choices += 1;
        }
        let parents: Vec<&String> = p.parts.iter().map(|x| &x.1).chain(p.options.iter().map(|x| &x.1)).collect();

        let per_decision: Vec<Vec<String>> = decisions.iter().map(|d| p.options.iter().filter(|(_, x)| x == d).map(|(o, _)| o.clone()).collect()).collect();
        let mut pick = vec![0usize; decisions.len()];
        loop {
            let world: BTreeMap<String, String> = decisions.iter().zip(&pick).zip(&per_decision).map(|((d, i), opts)| (d.clone(), opts[*i].clone())).collect();
            let (plain, kept) = plain_in_world(&p, &world, None);
            let by_handle: BTreeMap<String, String> = world.iter().map(|(d, o)| (g.id(d).to_string(), g.id(o).to_string())).collect();
            let end = end_of(&plain, &kept);
            for id in &ids {
                let is_kept = kept.contains(id);
                assert_eq!(g.get::<Need>(id).unwrap().unwrap().0.at(&by_handle), is_kept, "need({id}) in {world:?}");
                let w = g.get::<WorldFinish>(id).unwrap().unwrap().0;
                let got = w.at(&by_handle).unwrap();
                let want = if is_kept {
                    match plain.get::<Finish>(id) {
                        Ok(f) => f.unwrap().0,
                        Err(e) => panic!("plain schedule failed where worlds didn't: {e:?} in {world:?}"),
                    }
                } else {
                    0.0
                };
                assert_eq!(got, want, "finish({id}) in {world:?}");
                compared += 1;

                // Slack means how far it could slip without moving the end, so slip it and see.
                let slack = *g.get::<WorldSlack>(id).unwrap().unwrap().0.at(&by_handle);
                if !is_kept {
                    assert_eq!(slack, None, "slack({id}) in {world:?}");
                } else if !parents.contains(&id) {
                    let slack = slack.unwrap();
                    has_slack += usize::from(slack > 0.0);
                    let (at, _) = plain_in_world(&p, &world, Some((id, slack)));
                    assert_eq!(end_of(&at, &kept), end, "{id} slipping by its slack {slack} moved the end in {world:?}");
                    let (past, _) = plain_in_world(&p, &world, Some((id, slack + 1.0)));
                    assert!(end_of(&past, &kept) > end, "{id} slipping past its slack {slack} didn't move the end in {world:?}");
                    slack_checked += 1;
                }
            }
            let mut i = 0;
            while i < pick.len() {
                pick[i] += 1;
                if pick[i] < per_decision[i].len() {
                    break;
                }
                pick[i] = 0;
                i += 1;
            }
            if i == pick.len() {
                break;
            }
        }
    }
    println!("{compared} node-world comparisons, {slack_checked} slacks checked by slipping, {has_slack} of them above zero, {with_choices} plans with choices, {varying} estimates that vary");
    assert!(varying > 100, "estimates that vary by world should be exercised too");
    assert!(round_trips > 100, "plans with open decisions should save and reload");
    assert!(with_choices > 100 && compared > 1000 && slack_checked > 1000 && has_slack > 200, "the generator should actually exercise this");
}
