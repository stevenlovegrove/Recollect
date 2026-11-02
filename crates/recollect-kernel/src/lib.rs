//! Prototype of the Recollect kernel: see `docs/kernel-spec.md`.
//!
//! - `store`: stored facts only (entities and links, each a bag of components).
//! - `log`: the append-only action log; a store is the replay of its log.
//! - `schema`: component types and rules, for Rust and for plugins with no Rust types.
//! - `formula`: exact boolean functions over the open decisions.
//! - `compute`: the cell engine, and the primitives cells read the graph with.
//! - `systems`: the kernel's own cells.
//! - `scope`: where nodes belong, and views as transforms of a graph.

pub mod compute;
pub mod formula;
pub mod log;
pub mod schema;
pub mod scope;
pub mod store;
pub mod systems;
