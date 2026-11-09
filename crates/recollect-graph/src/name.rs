//! A name is just a cell, which every node has a place for. It isn't an identity: two nodes can share
//! one, and renaming a node only affects what reads its name. What it does say is which row of which
//! table a node is: see `table`.

use crate::{cell, component};

#[cell]
pub struct Name(pub String);

/// What every node has, so it can be called something.
#[component]
pub struct Named;
