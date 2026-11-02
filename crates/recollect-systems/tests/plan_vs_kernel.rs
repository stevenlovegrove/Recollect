//! The old kernel's planning rules are an independent implementation with a different compute
//! model: it recomputes everything from a store. So random plans and random edits go through both,
//! and they have to agree on every value, in every world.
//!
//! Two differences are deliberate, and both are handled explicitly rather than by comparing less:
//!
//! - `Done` only counts at a leaf here, so the kernel is given a store without the others.
//! - A node only inherits blocking from parents it's in play through. The kernel inherits from all
//!   of them, so its blocking can only be wider here. Where every node has one parent and the node
//!   is needed in that world, there's nothing to differ about, so they have to be equal.
//!
//! The kernel also has a bug that comes up by chance: if something reaches the same target both by
//! waiting for its work and by waiting for its answer, whichever it reaches last replaces the other,
//! so it can read as free when it isn't. States where that's possible aren't compared with the
//! kernel's blocking, and are counted.
//!
//! Blocking is what the kernel can't vouch for, so everything is also checked against evaluators in
//! this file, which walk each world directly and share no code with either.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use recollect_systems::plan::{self, Actionable, Blocked, Completion, Need, Pressure, Scope, ScopeDepth};
use recollect_systems::view::ViewGraph;
use recollect_graph::worlds::Bool;
use recollect_graph::{Entity, Graph, Log, Registry, Target};
use recollect_kernel::log::{Log as KLog, choose, set};
use recollect_kernel::scope::{Scope as KScope, ScopeDepth as KScopeDepth, ViewGraph as KViewGraph};
use recollect_kernel::store::{ALTERNATIVE, DECISION, DEPENDS_ON, DONE, DROPPED, Id, Op as KOp, PART_OF, Store, TASK, Target as KTarget, UNKNOWN, e, l};
use recollect_kernel::systems::{Derived, Pressure as KPressure, compute, pressure as kpressure};
use serde_json::json;

#[path = "support/kernel.rs"]
mod kernel;
use kernel::Ids;

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

fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    plan::install(&mut reg);
    Arc::new(reg)
}

// ---------- two logs, given the same things ----------

struct Both {
    old: KLog,
    new: Log,
    ids: Ids,
}

impl Both {
    fn commit(&mut self, kops: Vec<KOp>) -> bool {
        let mut state = self.old.store.clone();
        let mut ops = Vec::new();
        for op in &kops {
            ops.extend(self.ids.op(op, &state));
            let _ = state.apply(op);
        }
        let (a, b) = (self.old.commit("t", "x", kops.clone()).is_ok(), self.new.commit("t", "x", ops).is_ok());
        assert_eq!(a, b, "{kops:?}");
        a
    }
}

fn random_plan(rng: &mut Rng, one_parent: bool) -> (Both, Vec<String>) {
    let n = 6 + rng.next(12);
    let names: Vec<String> = (0..n).map(|i| format!("n{i:02}")).collect();
    let mut both = Both { old: KLog::default(), new: Log::new(registry()), ids: Ids(BTreeMap::new()) };
    let (mut ops, mut decisions) = (Vec::new(), Vec::new());
    for id in &names {
        ops.push(set(e(id), "title", json!(id)));
        if rng.chance(40) && decisions.len() < 5 {
            ops.push(set(e(id), DECISION, json!({})));
            decisions.push(id.clone());
        } else if rng.chance(50) {
            ops.push(set(e(id), TASK, json!({})));
        }
        if rng.chance(25) {
            ops.push(set(e(id), DONE, json!({})));
        } else if rng.chance(12) {
            ops.push(set(e(id), UNKNOWN, json!({})));
        }
        if rng.chance(8) {
            ops.push(set(e(id), DROPPED, json!({})));
        }
    }
    let gaps: BTreeSet<String> = ops.iter().filter_map(|op| matches!(op, KOp::Set(_, c, _) if c == UNKNOWN).then(|| target_name(op))).collect();
    let mut linked = BTreeSet::new();
    // Links only point at earlier nodes, so decomposition is acyclic by construction.
    for i in 1..n {
        for _ in 0..(if one_parent { 1 } else { 1 + rng.next(2) }) {
            let parent = &names[rng.next(i)];
            if !linked.insert((i, parent.clone())) {
                continue;
            }
            if decisions.contains(parent) && rng.chance(85) {
                let v = if rng.chance(15) { json!({ "ruled_out": {} }) } else { json!({}) };
                ops.push(set(l(&names[i], parent), ALTERNATIVE, v));
            } else if !gaps.contains(parent) {
                ops.push(set(l(&names[i], parent), PART_OF, json!({})));
            }
        }
    }
    assert!(both.commit(ops), "the generator makes valid plans");
    for _ in 0..(n / 3) {
        let (a, b) = (rng.next(n), rng.next(n));
        if a != b {
            let v = if rng.chance(25) { json!({ "on": "decided" }) } else { json!({}) };
            both.commit(vec![set(l(&names[a], &names[b]), DEPENDS_ON, v)]);
        }
    }
    for d in &decisions {
        if rng.chance(30)
            && let Some(o) = both.old.store.options(d).iter().map(|(o, _)| o.to_string()).find(|o| !both.old.store.has(o, UNKNOWN))
        {
            both.commit(vec![choose(d, &o)]);
        }
    }
    (both, names)
}

fn target_name(op: &KOp) -> String {
    match op {
        KOp::Set(KTarget::Entity(id), _, _) => id.clone(),
        _ => unreachable!("only set on nodes"),
    }
}

fn random_edit(rng: &mut Rng, names: &[String], one_parent: bool) -> KOp {
    let pick = |rng: &mut Rng| names[rng.next(names.len())].clone();
    let (a, b) = (pick(rng), pick(rng));
    let (node, link) = (e(&a), l(&a, &b));
    match rng.next(if one_parent { 12 } else { 15 }) {
        0 | 1 => set(node, DONE, json!({})),
        2 => KOp::Set(node, DONE.into(), None),
        3 => set(node, DROPPED, json!({})),
        4 => KOp::Set(node, DROPPED.into(), None),
        5 => set(node, DECISION, json!({ "chosen": b })),
        6 => set(node, DECISION, json!({})),
        7 => set(node, TASK, json!({})),
        8 => set(link, ALTERNATIVE, json!({ "ruled_out": {} })),
        9 => set(link, DEPENDS_ON, json!({})),
        10 => set(link, DEPENDS_ON, json!({ "on": "decided" })),
        11 => KOp::Set(node, UNKNOWN.into(), None),
        12 => set(link, PART_OF, json!({})),
        13 => set(link, ALTERNATIVE, json!({})),
        _ => KOp::Remove(link),
    }
}

// ---------- the same plan, as the kernel should see it ----------

/// `Done` only counts at a leaf, so here it's gone from everything above one.
fn leaves_only(s: &Store) -> Store {
    let mut out = s.clone();
    for id in s.entities.keys() {
        if is_above_a_leaf(s, id)
            && let Some(comps) = out.entities.get_mut(id)
        {
            comps.remove(DONE);
        }
    }
    out
}

fn is_above_a_leaf(s: &Store, id: &str) -> bool {
    s.has(id, DECISION) || !s.children(id, PART_OF).is_empty()
}

// ---------- evaluators that walk one world directly ----------

type World<'a> = BTreeMap<&'a str, &'a str>;

fn selects(s: &Store, w: &World, decision: &str, option: &str) -> bool {
    match s.chosen(decision) {
        Some(c) => c == option,
        None => w.get(decision) == Some(&option),
    }
}

/// Whether `id` is in play through this link upwards.
fn through(s: &Store, w: &World, id: &str, parent: &str, link: &recollect_kernel::store::Components) -> bool {
    if link.contains_key(PART_OF) {
        return need(s, w, parent);
    }
    link.get(ALTERNATIVE).is_some_and(|a| a.get("ruled_out").is_none()) && need(s, w, parent) && s.has(parent, DECISION) && selects(s, w, parent, id)
}

fn need(s: &Store, w: &World, id: &str) -> bool {
    if !s.live(id) {
        return false;
    }
    let ups: Vec<_> = s.links.iter().filter(|((f, _), c)| f == id && (c.contains_key(PART_OF) || c.contains_key(ALTERNATIVE))).collect();
    ups.is_empty() || ups.iter().any(|((_, t), c)| through(s, w, id, t, c))
}

/// Something is still owed here: the opposite of the kernel's `done`, for a live node.
fn owed(s: &Store, w: &World, id: &str) -> bool {
    if !s.live(id) {
        return false;
    }
    let parts = s.children(id, PART_OF);
    let is_decision = s.has(id, DECISION);
    if parts.is_empty() && !is_decision {
        return s.has(id, UNKNOWN) || (!s.has(id, DONE) && ["task", "goal", "question", "risk"].iter().any(|c| s.has(id, c)));
    }
    parts.iter().any(|(p, _)| owed(s, w, p)) || (is_decision && !s.options(id).iter().any(|(o, _)| selects(s, w, id, o) && !owed(s, w, o)))
}

fn blocked_in_play(s: &Store, w: &World, id: &str) -> bool {
    let own = s.links.iter().filter(|((f, _), c)| f == id && c.contains_key(DEPENDS_ON)).any(|((_, t), c)| {
        s.live(t)
            && if c[DEPENDS_ON].get("on").is_some() {
                s.has(t, DECISION) && s.chosen(t).is_none()
            } else {
                need(s, w, t) && owed(s, w, t)
            }
    });
    own || s.links.iter().filter(|((f, _), c)| f == id && (c.contains_key(PART_OF) || c.contains_key(ALTERNATIVE))).any(|((_, t), c)| through(s, w, id, t, c) && blocked_in_play(s, w, t))
}

// ---------- comparing ----------

#[derive(Default)]
struct Seen {
    compared: usize,
    worlds_with_choices: usize,
    blocked: usize,
    inherits_less: usize,
    mixed_waits: usize,
    gaps_needed: usize,
    actionable: usize,
    views: usize,
    multi_parent: usize,
    pressure: BTreeMap<&'static str, usize>,
    notices: BTreeMap<String, usize>,
}

fn at(b: &Bool, world: &BTreeMap<String, String>) -> bool {
    b.at(world)
}

fn formula(k: &Derived, cell: &str, id: &str, w: &World) -> bool {
    k.mdd.eval(k.get(cell, id), w)
}

fn compare(both: &mut Both, one_parent: bool, rng: &mut Rng, seen: &mut Seen, context: &str) {
    let store = both.old.store.clone();
    let view = leaves_only(&store);
    let k = compute(&view);
    let g = both.new.graph();
    let names: Vec<String> = store.entities.keys().cloned().collect();
    let handle: BTreeMap<String, Entity> = names.iter().map(|n| (n.clone(), both.ids.of(n))).collect();
    let name_of: BTreeMap<Entity, String> = handle.iter().map(|(n, e)| (*e, n.clone())).collect();

    let mut wait_kinds: BTreeMap<&str, BTreeSet<bool>> = BTreeMap::new();
    for ((_, t), c) in &view.links {
        if let Some(dep) = c.get(DEPENDS_ON) {
            wait_kinds.entry(t).or_default().insert(dep.get("on").is_some());
        }
    }
    let mixed_waits = wait_kinds.values().any(|kinds| kinds.len() > 1);
    seen.mixed_waits += usize::from(mixed_waits);
    let one_parent = one_parent && !mixed_waits;

    struct Cells {
        name: String,
        need: Bool,
        open: Bool,
        blocked: Bool,
        actionable: Option<Bool>,
    }
    let cells: Vec<Cells> = names
        .iter()
        .map(|n| {
            let e = handle[n];
            Cells {
                name: n.clone(),
                need: g.get::<Need>(e).unwrap().unwrap().0,
                open: g.get::<Completion>(e).unwrap().unwrap().open(),
                blocked: g.get::<Blocked>(e).unwrap().unwrap().0,
                actionable: g.get::<Actionable>(e).unwrap().map(|a| a.0),
            }
        })
        .collect();

    // Things that don't depend on the world.
    for c in &cells {
        let n = &c.name;
        assert!((c.need.prob() - k.prob("need", n)).abs() < 1e-9, "probability of need({n}): {context}");
        assert_eq!(c.need.holds_among_known(), k.holds_among_known("need", n), "need({n}) among known: {context}");
        assert_eq!(k.cells["actionable"].contains_key(n), c.actionable.is_some(), "whether {n} is work: {context}");
        let scope = g.get::<Scope>(handle[n]).unwrap().map(|s| name_of[&s.0].clone());
        assert_eq!(scope, k.of::<KScope>(n).unwrap().clone(), "scope({n}): {context}");
        assert_eq!(g.get::<ScopeDepth>(handle[n]).unwrap().unwrap().0, *k.of::<KScopeDepth>(n).unwrap(), "depth({n}): {context}");
        seen.multi_parent += usize::from(store.parents(n).len() > 1);
    }

    // Every world, or a sample if there are a lot.
    let total: usize = k.mdd.vars.iter().map(|v| v.options.len()).product();
    let every = total <= 300;
    seen.worlds_with_choices += usize::from(total > 1);
    let mut pick = vec![0usize; k.mdd.vars.len()];
    for round in 0..total.min(300) {
        if !every {
            pick = k.mdd.vars.iter().map(|v| rng.next(v.options.len())).collect();
        } else if round > 0 {
            for (p, v) in pick.iter_mut().zip(&k.mdd.vars) {
                *p += 1;
                if *p < v.options.len() {
                    break;
                }
                *p = 0;
            }
        }
        let w: World = k.mdd.vars.iter().zip(&pick).map(|(v, i)| (v.name.as_str(), v.options[*i].as_str())).collect();
        let world: BTreeMap<String, String> = w.iter().map(|(d, o)| (handle[*d].to_string(), handle[*o].to_string())).collect();
        for c in &cells {
            let n = c.name.as_str();
            let needed = at(&c.need, &world);
            assert_eq!(needed, formula(&k, "need", n, &w), "need({n}) in {w:?}: {context}");
            assert_eq!(needed, need(&view, &w, n), "need({n}) by hand, in {w:?}: {context}");

            let open = at(&c.open, &world);
            assert_eq!(open, store.live(n) && !formula(&k, "done", n, &w), "open({n}) against the kernel, in {w:?}: {context}");
            assert_eq!(open, owed(&view, &w, n), "open({n}) by hand, in {w:?}: {context}");

            let blocked = at(&c.blocked, &world);
            let wider = formula(&k, "blocked", n, &w);
            assert_eq!(blocked, blocked_in_play(&view, &w, n), "blocked({n}) by hand, in {w:?}: {context}");
            if !mixed_waits {
                assert!(!blocked || wider, "blocked({n}) where the kernel says it isn't, in {w:?}: {context}");
            }
            if one_parent && needed {
                assert_eq!(blocked, wider, "blocked({n}) in {w:?}: {context}");
            }
            seen.blocked += usize::from(blocked);
            seen.inherits_less += usize::from(wider && !blocked);
            seen.gaps_needed += usize::from(view.has(n, UNKNOWN) && needed);

            if let Some(a) = &c.actionable {
                let free = at(a, &world);
                let kernel = formula(&k, "actionable", n, &w);
                assert_eq!(free, needed && open && !blocked, "actionable({n}) in {w:?}: {context}");
                if !mixed_waits {
                    assert!(!kernel || free, "actionable({n}) in the kernel and not here, in {w:?}: {context}");
                }
                if one_parent {
                    assert_eq!(free, kernel, "actionable({n}) in {w:?}: {context}");
                }
                seen.actionable += usize::from(free);
            }
            seen.compared += 1;
        }
    }

    // Pressure comes from `actionable`, so it only has to match where that does.
    for d in view.open_decisions() {
        let new = plan::pressure(g, handle[d]).unwrap();
        let by_name = |ids: &[Entity]| ids.iter().map(|e| name_of[e].clone()).collect::<BTreeSet<_>>();
        let label = match &new {
            Pressure::Free(_) => "free",
            Pressure::Blocking(_) => "blocking",
            Pressure::Idle => "idle",
            Pressure::Decided => "decided",
        };
        *seen.pressure.entry(label).or_default() += 1;
        if one_parent {
            let same = match (&new, kpressure(&view, &k, d)) {
                (Pressure::Free(a), KPressure::Free(b)) | (Pressure::Blocking(a), KPressure::Blocking(b)) => by_name(a) == b.into_iter().collect::<BTreeSet<_>>(),
                (Pressure::Idle, KPressure::Idle) | (Pressure::Decided, KPressure::Decided) => true,
                _ => false,
            };
            assert!(same, "pressure({d}): {new:?}: {context}");
        }
    }

    // Notices. Whether a stated done disagrees has no equivalent over the store the kernel was given,
    // so it's stated from that store's columns.
    let mut want: BTreeSet<(String, String)> = k.notices.iter().map(|n| (n.rule.to_string(), n.id.clone())).collect();
    for n in &names {
        if store.has(n, DONE) && store.live(n) && is_above_a_leaf(&store, n) && k.get("all_required_done", n) != recollect_kernel::formula::TRUE {
            want.insert(("stated_done_with_open_work".to_string(), n.clone()));
        }
    }
    let got: BTreeSet<(String, String)> = both
        .new
        .notices()
        .into_iter()
        .map(|v| match v.on {
            Target::Node(e) => (v.rule, name_of[&e].clone()),
            other => panic!("{other:?}"),
        })
        // The kernel only has these three. The others are tested in tests/plan.rs.
        .filter(|(rule, _)| ["stated_done_with_open_work", "dropped_part_of_live_work", "decision_without_options"].contains(&rule.as_str()))
        .collect();
    assert_eq!(got, want, "notices: {context}");
    for (rule, _) in &got {
        *seen.notices.entry(rule.clone()).or_default() += 1;
    }

    compare_views(g, &store, &k, &handle, &name_of, rng, context);
    seen.views += 1;
}

fn same_view(v: &ViewGraph, k: &KViewGraph, name_of: &BTreeMap<Entity, String>, what: &str, context: &str) {
    let nodes: BTreeMap<String, (usize, bool)> = v.nodes.iter().map(|(e, n)| (name_of[e].clone(), (n.hidden, n.external))).collect();
    let knodes: BTreeMap<Id, (usize, bool)> = k.nodes.iter().map(|(id, n)| (id.clone(), (n.hidden, n.external))).collect();
    assert_eq!(nodes, knodes, "{what} nodes: {context}");
    let edges: BTreeMap<(String, String), (BTreeSet<String>, usize)> = v.edges.iter().map(|((f, t), e)| ((name_of[f].clone(), name_of[t].clone()), (e.kinds.clone(), e.links))).collect();
    let kedges: BTreeMap<(Id, Id), (BTreeSet<String>, usize)> = k.edges.iter().map(|(key, e)| (key.clone(), (e.components.clone(), e.links))).collect();
    assert_eq!(edges, kedges, "{what} edges: {context}");
}

fn compare_views(g: &Graph, store: &Store, k: &Derived, handle: &BTreeMap<String, Entity>, name_of: &BTreeMap<Entity, String>, rng: &mut Rng, context: &str) {
    let mut links_only = store.clone();
    // The kernel keeps a link with nothing on it. Here, a link is its kinds.
    links_only.links.retain(|_, c| !c.is_empty());
    let (full, kfull) = (ViewGraph::of(g), KViewGraph::of(&links_only));
    same_view(&full, &kfull, name_of, "whole", context);
    if handle.is_empty() {
        return; // Undoing a plan's first event leaves nothing.
    }
    let expanded: BTreeSet<Id> = handle.keys().filter(|_| rng.chance(40)).cloned().collect();
    let by_handle: BTreeSet<Entity> = expanded.iter().map(|n| handle[n]).collect();
    same_view(&full.collapse(g, &by_handle), &kfull.collapse(k, &expanded), name_of, "collapsed", context);
    let root = handle.keys().nth(rng.next(handle.len())).expect("plans have nodes");
    same_view(&full.within(g, handle[root]), &kfull.within(k, root), name_of, "within", context);
}

#[test]
fn random_plans_agree_with_the_old_kernel_in_every_world() {
    let mut seen = Seen::default();
    let mut rng = Rng(0x9E3779B97F4A7C15);
    let (mut accepted, mut refused) = (0, 0);
    for round in 0..800 {
        let one_parent = round % 2 == 0;
        let (mut both, names) = random_plan(&mut rng, one_parent);
        compare(&mut both, one_parent, &mut rng, &mut seen, &format!("round {round}, as generated"));
        // After edits, the memos here have to give what a kernel starting over gives.
        for step in 0..12 {
            if rng.chance(10) && !both.old.events.is_empty() {
                let seq = both.old.events.len() - 1;
                assert_eq!(both.old.undo(seq, "t", "u").is_ok(), both.new.undo(seq as u64, "t", "u").is_ok());
            } else {
                let ops = vec![random_edit(&mut rng, &names, one_parent)];
                if both.commit(ops) {
                    accepted += 1;
                } else {
                    refused += 1;
                }
            }
            // One edit can make a node above a leaf, or take away the only parent it had.
            let one_parent = one_parent && both.old.store.entities.keys().all(|id| both.old.store.parents(id).len() <= 1);
            compare(&mut both, one_parent, &mut rng, &mut seen, &format!("round {round}, step {step}"));
        }
    }
    println!(
        "{} node-world comparisons, {} states with choices, {accepted} edits accepted and {refused} refused, {} blocked, {} where the kernel inherits more, {} where a gap is needed, {} actionable, {} views, {} with several parents, {} states not compared on blocking",
        seen.compared, seen.worlds_with_choices, seen.blocked, seen.inherits_less, seen.gaps_needed, seen.actionable, seen.views, seen.multi_parent, seen.mixed_waits
    );
    println!("pressure {:?}, notices {:?}", seen.pressure, seen.notices);
    assert!(seen.compared > 200_000 && seen.worlds_with_choices > 2000 && accepted > 2000 && refused > 500);
    assert!(seen.mixed_waits * 2 < seen.views, "too few states are compared on blocking: {} of {}", seen.mixed_waits, seen.views);
    assert!(seen.blocked > 1000 && seen.inherits_less > 100 && seen.gaps_needed > 100 && seen.actionable > 1000 && seen.multi_parent > 1000);
    for label in ["free", "blocking", "idle"] {
        assert!(seen.pressure.get(label).is_some_and(|n| *n > 20), "{:?}", seen.pressure);
    }
    for rule in ["stated_done_with_open_work", "dropped_part_of_live_work", "decision_without_options"] {
        assert!(seen.notices.get(rule).is_some_and(|n| *n > 20), "{:?}", seen.notices);
    }
}

/// Everything here reads the same in a new graph that replayed the log, and after a save and a load.
#[test]
fn memos_and_files_change_nothing() {
    let mut rng = Rng(0xDEADBEEFCAFE1234);
    let mut cells = 0;
    for _ in 0..30 {
        let (mut both, names) = random_plan(&mut rng, false);
        for _ in 0..6 {
            let ops = vec![random_edit(&mut rng, &names, false)];
            both.commit(ops);
            let g = both.new.graph();
            for e in g.entities() {
                for c in g.components_of(e) {
                    g.get_runtime(e, &c).unwrap();
                }
            }
        }
        let g = both.new.graph();
        let replayed = both.new.state_at(both.new.last_seq().unwrap());
        let dir = tempfile::tempdir().unwrap();
        g.save_dir(dir.path()).unwrap();
        let (loaded, problems) = Graph::load_dir(dir.path(), registry()).unwrap();
        assert_eq!(problems, vec![]);
        for e in g.entities() {
            for c in g.components_of(e) {
                let got = g.get_runtime(e, &c);
                assert_eq!(got, replayed.get_runtime(e, &c), "{c} of {e}, replayed");
                assert_eq!(got, loaded.get_runtime(e, &c), "{c} of {e}, loaded");
                cells += 1;
            }
        }
    }
    println!("{cells} cells");
    assert!(cells > 2000, "{cells}");
}
