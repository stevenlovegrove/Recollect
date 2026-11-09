//! What a workspace can be made of. A system is a function that adds components, the cells they bring
//! and the checks they make to a `Registry`, which is all the core ever sees. A workspace lists the
//! systems it uses by name.

pub mod expr;
pub mod formulas;
pub mod notes;
pub mod plan;
pub mod samples;
pub mod schedule;
pub mod view;

use std::collections::HashMap;
use std::sync::Arc;

use recollect_graph::{CellType, Entity, Graph, Name, Registry};

/// A system, as a workspace names it.
#[derive(Clone, Copy)]
pub struct System {
    pub name: &'static str,
    pub about: &'static str,
    pub install: fn(&mut Registry),
}

/// Every system there is. A plugin would add to this.
pub const SYSTEMS: &[System] = &[
    System { name: "notes", about: "Names, notes and sources, and edges that say how ideas relate.", install: notes::install },
    System { name: "formulas", about: "Values typed in or worked out, choices, and requirements on the worlds that count.", install: formulas::install },
    System { name: "plan", about: "Work, decisions held open, and what can safely start.", install: plan::install },
    System { name: "schedule", about: "Dates and slack in every world. Needs plan.", install: schedule::install },
];

/// The systems every workspace has, so they aren't anything to choose.
pub const ALWAYS: &[&str] = &["notes", "formulas"];

/// What a workspace's systems make. Notes are always there, because a node needs a name, and so are
/// formulas, because any cell may be worked out.
pub struct Built {
    pub names: Vec<String>,
    pub registry: Arc<Registry>,
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
    rows.chain(g.entities().map(|e| (e.to_string(), label_of(g, e)))).collect()
}

pub fn label_of(g: &Graph, e: Entity) -> String {
    match g.get_value(e, Name::NAME) {
        Ok(Some(serde_json::Value::String(n))) => n,
        _ => e.to_string().chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect(),
    }
}

