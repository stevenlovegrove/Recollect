//! Structure on its own: a design broken down coarse to fine, with decisions held open, and no work.

mod support;

use recollect_graph::CellType;
use recollect_systems::formulas::Chosen;
use recollect_systems::plan::Done;
use recollect_systems::structure::{ALTERNATIVE, DECISION, Need, PART_OF};
use support::N;

/// A delta store whose ancestry is still open: by hash puts a parent list in the header, and by count
/// a version vector.
fn design() -> N {
    let mut g = N::systems(&["structure"]);
    g.link("Header", "Delta", PART_OF).link("Payload", "Delta", PART_OF);
    g.add("How is ancestry recorded?", DECISION);
    g.link("By hash", "How is ancestry recorded?", ALTERNATIVE).link("By count", "How is ancestry recorded?", ALTERNATIVE);
    g.link("Parent list", "By hash", PART_OF).link("Version vector", "By count", PART_OF);
    g.link("How is ancestry recorded?", "Header", PART_OF);
    g
}

#[test]
fn whats_under_an_option_is_needed_only_where_its_taken() {
    let g = design();
    let need = |n: &str| g.get::<Need>(n).unwrap().unwrap().0;
    assert!(need("Header").is_always() && need("Payload").is_always(), "all of a node's parts are needed");
    let (hash, count) = (need("Parent list"), need("Version vector"));
    assert!(!hash.is_always() && !hash.is_never(), "{}", hash.show());
    assert!((&hash & &count).is_never(), "never both");
    assert!((&hash | &count).is_always(), "and always one");
    assert!((hash.prob() - 0.5).abs() < 1e-9);
}

#[test]
fn a_design_has_no_work_in_it() {
    let g = design();
    for n in ["Delta", "Header", "Parent list", "How is ancestry recorded?"] {
        assert!(g.get::<Done>(n).is_err() || g.get::<Done>(n).unwrap().is_none(), "{n} has no done");
        assert!(!g.cells_of(g.id(n)).contains(Done::NAME), "{n} has no done cell");
    }
    assert!(g.registry.component_def("task").is_none(), "and nothing to make work with");
    assert!(g.registry.component_def("part").is_some_and(|c| c.system == "structure"));
}

#[test]
fn a_decision_in_a_design_still_makes_worlds_and_can_be_settled() {
    let mut g = design();
    let hash = g.id("By hash");
    g.set("How is ancestry recorded?", Chosen::on(hash));
    let need = |n: &str| g.get::<Need>(n).unwrap().unwrap().0;
    assert!(need("Parent list").is_always());
    assert!(need("Version vector").is_never());
}
