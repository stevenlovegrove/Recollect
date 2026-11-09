//! Memoisation must be invisible: after any sequence of edits, what a long-lived graph answers
//! has to match a brand new graph holding the same state, which has no memos to get wrong.

use std::collections::{BTreeMap, BTreeSet};

mod support;

use recollect_graph::{Cell, CellType, Cx, Error, Graph, Registry, cell};
use support::N;

#[derive(Copy)]
#[cell]
struct Amount(i64);

const SHEET: &str = "sheet";
const ADDEND: &str = "addend";

/// Clamped, so different inputs often give the same answer and early cut-off gets exercised.
fn sum(cx: &Cx) -> Cell<Amount> {
    let mut total = 0;
    for from in cx.sources(ADDEND) {
        total += cx.get::<Amount>(from)?.map_or(0, |a| a.0);
    }
    Ok(Some(Amount(total.clamp(-3, 3))))
}

fn install(reg: &mut Registry) {
    reg.component(SHEET, "A cell of a sheet: what's added to it, unless a value is typed in.");
    reg.cell::<Amount>(SHEET, "The sum of what's added to it.").builtin(sum);
    reg.edge(ADDEND, "Added to that.");
}

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Literal(i64),
    Formula,
}

#[derive(Default)]
struct State {
    cells: BTreeMap<String, Kind>,
    links: BTreeSet<(String, String)>,
}

fn fresh(s: &State) -> N {
    let mut g = N::with(install);
    for (id, k) in &s.cells {
        g.add(id, SHEET);
        if let Kind::Literal(v) = k {
            g.set(id, Amount(*v));
        }
    }
    for (a, b) in &s.links {
        g.link(a, b, ADDEND);
    }
    g
}

/// Which cell is reported as starting a cycle can depend on what was asked first, so only
/// whether it's an error has to match, and the value when it isn't.
fn outcome(g: &N, id: &str) -> Result<Option<i64>, ()> {
    match g.get::<Amount>(id) {
        Ok(v) => Ok(v.map(|a| a.0)),
        Err(Error::Cycle(_)) => Err(()),
        Err(e) => panic!("unexpected {e:?}"),
    }
}

#[test]
fn a_long_lived_graph_always_agrees_with_a_new_one() {
    let mut rng = Rng(0x2545F4914F6CDB5D);
    let (mut edits, mut reused, mut reran, mut cycles, mut reloads) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for _ in 0..400 {
        let ids: Vec<String> = (0..12).map(|i| format!("n{i:02}")).collect();
        let mut state = State::default();
        for id in &ids {
            let kind = if rng.next(100) < 40 { Kind::Literal(rng.next(5) as i64 - 2) } else { Kind::Formula };
            state.cells.insert(id.clone(), kind);
        }
        let mut g = fresh(&state);

        for _ in 0..60 {
            let (a, b) = (ids[rng.next(12)].clone(), ids[rng.next(12)].clone());
            match rng.next(5) {
                0 => {
                    let v = rng.next(5) as i64 - 2;
                    state.cells.insert(a.clone(), Kind::Literal(v));
                    g.set(&a, Amount(v));
                }
                1 => {
                    state.cells.insert(a.clone(), Kind::Formula);
                    g.clear(&a, Amount::NAME);
                }
                2 | 3 if a != b => {
                    // Mostly forward links, with the odd cycle.
                    let (x, y) = if a < b || rng.next(10) == 0 { (a, b) } else { (b, a) };
                    state.links.insert((x.clone(), y.clone()));
                    g.link(&x, &y, ADDEND);
                }
                4 => {
                    if let Some(l) = state.links.iter().nth(rng.next(state.links.len().max(1))).cloned() {
                        state.links.remove(&l);
                        g.unlink(&l.0, &l.1, ADDEND);
                    }
                }
                _ => continue,
            }
            edits += 1;

            // Several edits can land between one read and the next, as a batch does.
            if rng.next(100) < 50 {
                continue;
            }

            // Ask about some cells only, so others are left to go stale for a while.
            let new = fresh(&state);
            g.take_ran();
            for id in ids.iter().filter(|_| rng.next(100) < 40) {
                let (got, want) = (outcome(&g, id), outcome(&new, id));
                assert_eq!(got, want, "{id}, with {:?} and {:?}", state.cells, state.links);
                cycles += usize::from(want.is_err());
            }
            // And what it would save reads back the same as a graph that was never saved.
            if edits % 10 == 0 {
                let (reloaded, problems) = Graph::load(g.g.save(), g.registry.clone());
                assert_eq!(problems, vec![]);
                for id in &ids {
                    let got = match reloaded.get::<Amount>(g.id(id)) {
                        Ok(v) => Ok(v.map(|a| a.0)),
                        Err(Error::Cycle(_)) => Err(()),
                        Err(e) => panic!("unexpected {e:?}"),
                    };
                    assert_eq!(got, outcome(&new, id), "{id} after a round trip");
                    reloads += 1;
                }
            }
            let ran = g.take_ran().len();
            let total = new.take_ran().len();
            reran += ran;
            reused += total.saturating_sub(ran);
        }
    }
    println!("{edits} edits: reran {reran} cells where a new graph needs {}, {cycles} cycle answers checked, {reloads} cells checked after a reload", reran + reused);
    assert!(reused > reran, "memoisation should actually be saving work");
    assert!(cycles > 100, "the edits should actually exercise cycles");
}

/// y used to read z. Now y reads nothing, and z reads x, which reads y: no loop, but one if y's old
/// read is followed. So a computation's old reads have to be checked in the order it made them: if
/// its first reads are unchanged then so is its next one, and checking them in any other order can
/// run something it no longer reads, in the middle of evaluating something else.
#[test]
fn checking_a_memo_doesnt_follow_a_read_that_is_no_longer_made() {
    let mut g = N::with(install);
    for id in ["x", "y", "z"] {
        g.add(id, SHEET);
    }
    g.add("w", SHEET).set("w", Amount(2));
    g.link("z", "y", ADDEND);
    g.link("w", "z", ADDEND);
    assert_eq!(g.get::<Amount>("y"), Ok(Some(Amount(2))));
    assert_eq!(g.get::<Amount>("x"), Ok(Some(Amount(0))));

    g.unlink("z", "y", ADDEND);
    g.link("y", "x", ADDEND);
    g.link("x", "z", ADDEND);

    assert_eq!(g.get::<Amount>("x"), Ok(Some(Amount(0))));
    assert_eq!(g.get::<Amount>("z"), Ok(Some(Amount(2))));
    assert_eq!(g.get::<Amount>("y"), Ok(Some(Amount(0))));
}

/// A cell that reads the amounts added to it.
#[derive(Copy)]
#[cell]
struct Seen(i64);

/// Every amount there is.
#[derive(Copy)]
#[cell]
struct Everything(i64);

const SEER: &str = "seer";
const SCANNER: &str = "scanner";

fn seen(cx: &Cx) -> Cell<Seen> {
    let mut total = 0;
    for from in cx.sources(ADDEND) {
        total += cx.get::<Amount>(from)?.map_or(0, |a| a.0);
    }
    Ok(Some(Seen(total)))
}

fn everything(cx: &Cx) -> Cell<Everything> {
    let mut total = 0;
    for e in cx.scan(SHEET) {
        total += cx.get::<Amount>(e)?.map_or(0, |a| a.0);
    }
    Ok(Some(Everything(total)))
}

fn install_seen(reg: &mut Registry) {
    install(reg);
    reg.component(SEER, "Sees what's added to it.");
    reg.cell::<Seen>(SEER, "The amounts added to it.").builtin(seen);
    reg.component(SCANNER, "Sees every amount.");
    reg.cell::<Everything>(SCANNER, "Every amount there is.").builtin(everything);
}

/// The same as for a single cell: a memo only stands for an unbroken run of being computed, so a cell
/// swapped from its builtin to a value and back has to count as changed for whatever read the value.
#[test]
fn a_builtin_swapped_for_a_value_and_back_is_a_change_to_its_readers() {
    let mut g = N::with(install_seen);
    g.add("reader", SEER);
    g.add("source", SHEET);
    g.link("source", "reader", ADDEND);
    assert_eq!(g.get::<Seen>("reader"), Ok(Some(Seen(0))));

    g.set("source", Amount(2));
    assert_eq!(g.get::<Seen>("reader"), Ok(Some(Seen(2))));

    g.clear("source", Amount::NAME);
    assert_eq!(g.get::<Seen>("reader"), Ok(Some(Seen(0))), "the builtin is back, with the answer it had before");
}

#[test]
fn removing_a_node_and_bringing_it_back_is_a_change_to_what_scans_it() {
    let mut g = N::with(install_seen);
    let (a, b) = (g.g.spawn(), g.g.spawn());
    g.g.add(a, SHEET).add(b, SHEET);
    g.g.set(a, Amount(1)).set(b, Amount(2));
    let total = g.g.spawn();
    g.g.add(total, SCANNER);
    assert_eq!(g.g.get::<Everything>(total), Ok(Some(Everything(3))));

    assert!(g.g.despawn(b));
    assert_eq!(g.g.get::<Everything>(total), Ok(Some(Everything(1))));
    assert_eq!(g.g.get::<Amount>(b), Ok(None));

    g.g.adopt(b);
    assert_eq!(g.g.get::<Everything>(total), Ok(Some(Everything(1))), "back, but with nothing on it");
    g.g.add(b, SHEET).set(b, Amount(2));
    assert_eq!(g.g.get::<Everything>(total), Ok(Some(Everything(3))));
}

/// A node with a link can't go: the link says something about the other end too.
#[test]
fn a_node_with_links_cant_be_removed() {
    let mut g = N::with(install);
    g.link("a", "b", ADDEND);
    let (a, b) = (g.id("a"), g.id("b"));
    assert!(!g.g.despawn(a) && !g.g.despawn(b));
    g.unlink("a", "b", ADDEND);
    assert!(g.g.despawn(a));
    assert!(!g.g.despawn(a), "and it's gone");
}
