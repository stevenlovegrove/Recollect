//! A name is just a component. It isn't an identity: two nodes can share one, and renaming a node
//! only affects what reads its name.

use crate::component;

#[component]
pub struct Name(pub String);
