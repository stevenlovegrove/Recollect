//! A graph is shared between threads: reads through `&self`, writes through `&mut self`, so
//! shared use is an `RwLock`. Results must be the same as reading from one thread, and a cell
//! shouldn't be computed twice just because two threads asked.

use std::sync::{Arc, RwLock};
use std::thread;

use recollect_graph::worlds::{Bool, Cond, Space};
use recollect_graph::{Cell, Computed, Cx, Entity, Error, Graph, Registry, cell};

#[cell]
struct Amount(f64);

#[cell]
struct Over(Bool);

const SHEET: &str = "sheet";
const ADDEND: &str = "addend";

fn sum(cx: &Cx) -> Cell<Amount> {
    let mut total = 0.0;
    for from in cx.sources(ADDEND) {
        total += cx.value::<Amount>(from)?.0;
    }
    Ok(Some(Amount(total)))
}

/// A sheet whose amounts sum what's added to them, and whose one formula panics.
fn registry() -> Arc<Registry> {
    let mut reg = Registry::new();
    reg.component(SHEET, "A cell of a sheet: what's added to it, unless a value is typed in.");
    reg.cell::<Amount>(SHEET, "The sum of what's added to it.").builtin(sum).compiles(Arc::new(|text| match text {
        "bad" => Ok(Computed::typed::<Amount>("bad", |_| panic!("this formula is broken"))),
        _ => Err(format!("there's no function called {text}")),
    }));
    reg.edge(ADDEND, "Added to that.");
    Arc::new(reg)
}

fn assert_shareable<T: Send + Sync>() {}

#[test]
fn everything_a_reader_holds_can_cross_threads() {
    assert_shareable::<Graph>();
    assert_shareable::<Entity>();
    assert_shareable::<Registry>();
    assert_shareable::<Computed>();
    assert_shareable::<Bool>();
    assert_shareable::<Cond<f64>>();
    assert_shareable::<Error>();
}

/// `depth` layers of `width` cells, each summing every cell in the layer above.
fn layered(depth: usize, width: usize) -> (Graph, Entity, Vec<Entity>) {
    let mut g = Graph::with_registry(registry());
    let seed = g.spawn();
    g.add(seed, SHEET).set(seed, Amount(1.0));
    let mut above = vec![seed];
    for _ in 0..depth {
        let layer: Vec<Entity> = (0..width).map(|_| g.spawn()).collect();
        for e in &layer {
            g.add(*e, SHEET);
            for a in &above {
                g.link(*a, *e, ADDEND);
            }
        }
        above = layer;
    }
    (g, seed, above)
}

#[test]
fn many_readers_agree_and_each_cell_runs_once() {
    let (g, _, last) = layered(6, 4);
    let want = 4f64.powi(5);
    let g = Arc::new(g);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (g, last) = (g.clone(), last.clone());
            thread::spawn(move || {
                for e in &last {
                    assert_eq!(g.get::<Amount>(*e).unwrap().unwrap().0, want);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(g.take_ran().len(), 6 * 4, "eight threads, and still once per cell");
}

#[test]
fn readers_and_a_writer_stay_consistent() {
    let (g, seed, last) = layered(4, 3);
    let g = Arc::new(RwLock::new(g));
    let scale = 3f64.powi(3);

    let readers: Vec<_> = (0..6)
        .map(|_| {
            let (g, last) = (g.clone(), last.clone());
            thread::spawn(move || {
                for _ in 0..200 {
                    let g = g.read().unwrap();
                    // Under one read lock, every cell comes from the same seed.
                    let values: Vec<f64> = last.iter().map(|e| g.get::<Amount>(*e).unwrap().unwrap().0).collect();
                    assert!(values.iter().all(|v| *v == values[0]), "{values:?}");
                    assert_eq!((values[0] / scale).fract(), 0.0, "{}", values[0]);
                }
            })
        })
        .collect();

    let writer = {
        let g = g.clone();
        thread::spawn(move || {
            for i in 2..200 {
                g.write().unwrap().set(seed, Amount(i as f64));
            }
        })
    };
    writer.join().unwrap();
    for r in readers {
        r.join().unwrap();
    }

    let g = g.read().unwrap();
    assert_eq!(g.get::<Amount>(last[0]).unwrap().unwrap().0, 199.0 * scale);
}

#[test]
fn a_world_value_computed_on_one_thread_is_usable_on_another() {
    let var = Space::global().uniform("concurrency_choice", &["cheap", "dear"]);
    let mut reg = Registry::new();
    reg.component("over", "Over budget, in some worlds.");
    reg.cell::<Over>("over", "The worlds where it's over.").builtin(move |_| Ok(Some(Over(var.is("dear")))));
    let mut g = Graph::with_registry(Arc::new(reg));
    let e = g.spawn();
    g.add(e, "over");
    let g = Arc::new(g);

    let (g2, on_other) = (g.clone(), e);
    let over = thread::spawn(move || g2.get::<Over>(on_other).unwrap().unwrap()).join().unwrap();
    assert_eq!(over.0.show(), "concurrency_choice=dear");
    assert!(over == g.get::<Over>(e).unwrap().unwrap(), "the same function, from either thread");
    assert!((over.0.prob() - 0.5).abs() < 1e-9);
}

#[test]
fn a_formula_that_panics_fails_its_cell_and_nothing_else() {
    let mut g = Graph::with_registry(registry());
    let (bad, good, reader) = (g.spawn(), g.spawn(), g.spawn());
    g.add(good, SHEET).add(bad, SHEET).add(reader, SHEET);
    g.set(good, Amount(2.0));
    g.set_formula(bad, "amount", "bad");
    g.link(bad, reader, ADDEND);
    let g = Arc::new(g);

    let g2 = g.clone();
    let result = thread::spawn(move || g2.get::<Amount>(reader)).join().unwrap();
    let Err(Error::Failed(message)) = result else { panic!("expected a failure, got {result:?}") };
    assert!(message.contains("this formula is broken"), "{message}");

    assert_eq!(g.get::<Amount>(good).unwrap().unwrap().0, 2.0, "other cells and other threads are fine");
    assert!(g.get::<Amount>(bad).is_err(), "and it's still reported, not left half done");
}
