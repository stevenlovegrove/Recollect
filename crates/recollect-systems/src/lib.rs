//! What a workspace can be made of. A system is a function that adds components, the cells they bring
//! and the checks they make to a `Registry`, which is all the core ever sees. A workspace lists the
//! systems it has taken on by name, which it does the first time it uses something from one.

pub mod expr;
pub mod formulas;
pub mod notes;
pub mod plan;
pub mod samples;
pub mod schedule;
pub mod structure;
pub mod view;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

use recollect_graph::{CellType, Entity, Graph, Name, Registry};

/// A system, as a workspace names it.
#[derive(Clone, Copy)]
pub struct System {
    pub name: &'static str,
    pub about: &'static str,
    pub install: fn(&mut Registry),
    /// How to use it, for an agent that's starting on a workspace that does.
    pub guide: &'static str,
}

/// Every system there is. A plugin would add to this.
pub const SYSTEMS: &[System] = &[
    System { name: "notes", about: "Names, notes and sources, and edges that say how ideas relate.", install: notes::install, guide: notes::GUIDE },
    System { name: "formulas", about: "Values typed in or worked out, choices, and requirements on the worlds that count.", install: formulas::install, guide: formulas::GUIDE },
    System { name: "structure", about: "Parts and what they're part of, and decisions held open: what a design or a plan is made of.", install: structure::install, guide: structure::GUIDE },
    System { name: "plan", about: "Work, decisions held open, and what can safely start.", install: plan::install, guide: plan::GUIDE },
    System { name: "schedule", about: "Dates and slack in every world. Needs plan.", install: schedule::install, guide: schedule::GUIDE },
];

/// The systems every workspace has, so they aren't anything to choose.
pub const ALWAYS: &[&str] = &["notes", "formulas"];

/// What a workspace's systems make. Notes are always there, because a node needs a name, and so are
/// formulas, because any cell may be worked out.
pub struct Built {
    pub names: Vec<String>,
    pub registry: Arc<Registry>,
}

/// Every system at once, for saying which one a component or cell comes from.
pub fn everything() -> &'static Registry {
    static ALL: OnceLock<Arc<Registry>> = OnceLock::new();
    ALL.get_or_init(|| build(&SYSTEMS.iter().map(|s| s.name.to_string()).collect::<Vec<_>>()).expect("every system builds").registry)
}

/// The systems a workspace needs for what it holds, beyond `ALWAYS`: whichever gives each component
/// or cell it uses, and schedule once there's a task, since a task's estimate is how long it takes and
/// dates are made from that. So nobody has to say which systems they want. In `SYSTEMS` order.
pub fn needed<'a>(components: impl IntoIterator<Item = &'a str>, cells: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let all = everything();
    let mut out = BTreeSet::new();
    let mut task = false;
    for c in components {
        task |= c == plan::TASK;
        if let Some(d) = all.component_def(c) {
            out.insert(d.system.clone());
        }
    }
    for c in cells {
        // A cell's own system, which may not be its component's: `plan` gives `done` to `part`.
        if let Some(def) = all.cell_def(c) {
            task |= def.component == plan::TASK;
            out.insert(def.system.clone());
        }
    }
    if task {
        out.insert("schedule".to_string());
    }
    // A plan is built on structure.
    if out.contains(plan::NAME) {
        out.insert(structure::NAME.to_string());
    }
    SYSTEMS.iter().map(|s| s.name).filter(|n| out.contains(*n) && !ALWAYS.contains(n)).map(String::from).collect()
}

pub fn build(names: &[String]) -> Result<Built, String> {
    let mut wanted: Vec<String> = ALWAYS.iter().map(|s| s.to_string()).collect();
    for n in names {
        if !wanted.contains(n) {
            wanted.push(n.clone());
        }
    }
    if wanted.iter().any(|n| n == "schedule") && !wanted.iter().any(|n| n == "plan") {
        return Err("the schedule system needs the plan system".to_string());
    }
    // A plan is built on structure, which comes first.
    if wanted.iter().any(|n| n == "plan") && !wanted.iter().any(|n| n == structure::NAME) {
        wanted.push(structure::NAME.to_string());
    }
    wanted.sort_by_key(|n| SYSTEMS.iter().position(|s| s.name == n).unwrap_or(usize::MAX));
    let mut reg = Registry::new();
    for n in &wanted {
        let s = SYSTEMS.iter().find(|s| s.name == n).ok_or_else(|| format!("there's no system called {n}"))?;
        reg.system(s.name);
        (s.install)(&mut reg);
    }
    Ok(Built { names: wanted, registry: Arc::new(reg) })
}

/// Handles to names, for reading a formula. A node with no name reads as the end of its handle.
pub fn labels(g: &Graph) -> HashMap<String, String> {
    let rows = g.implicit_rows().into_iter().map(|(e, name)| (e.to_string(), name));
    let constants = g.constants().into_iter().map(|(e, _)| (e.to_string(), label_of(g, e)));
    rows.chain(constants).chain(g.entities().map(|e| (e.to_string(), label_of(g, e)))).collect()
}

pub fn label_of(g: &Graph, e: Entity) -> String {
    match g.get_value(e, Name::NAME) {
        Ok(Some(serde_json::Value::String(n))) => n,
        _ => e.to_string().chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect(),
    }
}


/// Whether a variable is a draw from a short list, such as a die, rather than a choice: see
/// `docs/distributions.md`. A draw is called by where it's written, starting with `~`.
pub fn is_draw(var: &str) -> bool {
    var.starts_with('~')
}

/// Whether a variable is the chance that something seen, with how likely it was, was seen: a coin
/// whose heads evidence keeps, which is how what's seen weighs the worlds. It's bookkeeping, not a
/// question anyone would ask, so it isn't shown as one. See `docs/inference.md`.
pub fn is_seen(var: &str) -> bool {
    var.starts_with('~') && var.split_once('#').is_some_and(|(_, which)| which.trim_start_matches(|c: char| c.is_ascii_digit()).starts_with("seen"))
}

/// Where a draw is written, from what it's called: the node, the cell, and which of that cell's draws.
pub fn draw_site(var: &str) -> Option<(&str, &str, &str)> {
    let (node, rest) = var.strip_prefix('~')?.split_once('/')?;
    let (cell, which) = rest.split_once('#')?;
    // What follows the number says which ends it had, where those differ between worlds.
    let digits = which.find(|c: char| !c.is_ascii_digit()).unwrap_or(which.len());
    Some((node, cell, &which[..digits]))
}
