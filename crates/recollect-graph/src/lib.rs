//! A general compute core over a graph of nodes and edges, each with components and the cells they
//! bring, and worlds as a value type on top. See `docs/components.md`, and the toys in `tests/` for
//! what it's meant to express.

// So the attribute macros can name this crate from inside it too.
extern crate self as recollect_graph;

pub mod graph;
pub mod log;
pub mod name;
pub mod registry;
pub mod saved;
pub mod shown;
pub mod table;
pub mod worlds;

pub use graph::{Backed, Cell, CellType, ComponentType, Computed, Cx, Dep, Entity, Error, Graph, Key, Origin, Source};
pub use log::{Changes, CommitError, Event, Log, On, Op, Stored, Target, Violation, now_ms};
pub use name::{Name, Named};
pub use recollect_graph_macros::{cell, component};
pub use registry::{Advice, CellBuilder, CellDef, Compile, ComponentDef, Place, Registry, Severity};
pub use saved::{DirError, LoadProblem, SavedEdge, SavedGraph, SavedNode, is_node_path, node_of_path, node_path, node_text, parse_node, path_of};
pub use serde;
pub use shown::Shown;
