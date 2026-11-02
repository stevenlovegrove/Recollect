//! What any node can have, whatever else it is: a name, notes, and where it came from. Links that
//! only say how two ideas relate live here too, because nothing computes with them.

use recollect_graph::{Name, Registry, component};
use serde_json::{Value, json};

use crate::Catalog;

/// Notes, in markdown.
#[component]
pub struct Body(pub String);

/// Where this came from: anything a reader could follow back, such as a session, a file or a commit.
#[component]
pub struct Sources(pub Vec<Value>);

/// A way of drawing what's still possible: TypeScript exporting `draw(world)`, which returns SVG or
/// HTML. See `docs/drawings.md`.
#[component]
pub struct Drawing(pub String);

const NEW_DRAWING: &str = "export function draw(world: World): string {\n  const rows = world.choices.map((c) => `<li>${c.name}: ${c.options.filter((o) => o.possible).map((o) => o.name).join(\", \")}</li>`);\n  return `<ul>${rows.join(\"\")}</ul>`;\n}\n";

pub fn install(reg: &mut Registry) {
    reg.on_nodes::<Name>().on_nodes::<Body>().on_nodes::<Sources>().on_nodes::<Drawing>();
}

pub fn describe(c: &mut Catalog) {
    c.fact::<Name>("What it's called. It isn't an identity: two nodes can share one.", json!(""));
    c.fact::<Body>("Notes, in markdown.", json!(""));
    c.fact::<Sources>("Where this came from, so a claim can be followed back.", json!([]));
    c.fact::<Drawing>("A way of drawing what's still possible, which the Worlds tab can show.", json!(NEW_DRAWING));
    c.link_named("serves", "Contributes to that, without being part of it.");
    c.link_named("supports", "Evidence for that.");
    c.link_named("contradicts", "Evidence against that, or a risk threatening it.");
    c.link_named("constrains", "A standing rule that applies to that.");
    c.link_named("supersedes", "Replaces that.");
    c.link_named("relates", "Related, in a way nobody has pinned down yet.");
}
