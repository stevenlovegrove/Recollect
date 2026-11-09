//! A value that differs between worlds, in the forms a viewer knows how to read.

use serde::Serialize;
use serde_json::Value;

use crate::graph::Entity;
use crate::worlds::{Bool, Cond, Picks, Var};

#[derive(Debug, Clone)]
pub enum Shown {
    /// The worlds where something holds.
    Bool(Bool),
    /// Any value, per world.
    Cond(Cond<Value>),
    /// Named nodes, each with the worlds it applies in.
    Named(Vec<(Entity, Bool)>),
}

impl Shown {
    pub fn cond<T: Serialize>(c: &Cond<T>) -> Shown {
        Shown::Cond(c.map(|v| serde_json::to_value(v).expect("values serialise")))
    }

    /// This value among the worlds consistent with `picks`.
    pub fn given(&self, picks: &Picks) -> Shown {
        match self {
            Shown::Bool(b) => Shown::Bool(b.given(picks)),
            Shown::Cond(c) => Shown::Cond(c.given(picks)),
            Shown::Named(items) => Shown::Named(items.iter().map(|(e, b)| (*e, b.given(picks))).filter(|(_, b)| !b.is_never()).collect()),
        }
    }

    /// The open choices this depends on.
    pub fn vars(&self) -> Vec<Var> {
        match self {
            Shown::Bool(b) => b.vars(),
            Shown::Cond(c) => c.vars(),
            Shown::Named(items) => items.iter().flat_map(|(_, b)| b.vars()).collect(),
        }
    }
}
