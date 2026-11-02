//! A general compute core over a graph, and worlds as a value type on top. See
//! `docs/graph-core.md`, and the toys in `tests/` for what it's meant to express.

// So the attribute macros can name this crate from inside it too.
extern crate self as recollect_graph;

pub mod graph;
pub mod log;
pub mod name;
pub mod registry;
pub mod saved;
pub mod worlds;

pub use graph::{Cell, Component, Cx, Dep, Entity, Error, Graph, Inputs, Rule, Runtime, Storage, rule};
pub use log::{CommitError, Drift, Event, Log, Op, OpenError, Seen, Stored, Target, Violation, Witness};
pub use name::Name;
pub use recollect_graph_macros::{component, rule};
pub use registry::{CellResult, Code, Compute, OutputOf, Place, Registry, RuleInfo, RuleMeta, RuntimeRule, SavedRule, Severity, into_rule};
pub use schemars;
pub use serde;
pub use saved::{DirError, LoadProblem, SaveError, SavedDefaults, SavedGraph, SavedLink, SavedNode};
