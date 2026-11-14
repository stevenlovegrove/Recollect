//! What any node can have, whatever else it is: a name, notes, and where it came from. Edges that only
//! say how two ideas relate live here too, because nothing computes with them.

use recollect_graph::{ComponentType, Name, Named, Registry, cell};
use serde_json::{Value, json};

/// Notes, in markdown.
#[cell]
pub struct Body(pub String);

/// Where this came from: anything a reader could follow back, such as a session, a file or a commit.
#[cell]
pub struct Sources(pub Vec<Value>);

/// A way of drawing what's still possible: TypeScript exporting `draw(world)`, which returns SVG or
/// HTML. See `docs/drawings.md`.
#[cell]
pub struct Drawing(pub String);

pub const NOTES: &str = "notes";
pub const SOURCES: &str = "sources";
pub const DRAWING: &str = "drawing";

const NEW_DRAWING: &str = "export function draw(world: World): string {\n  const rows = world.choices.map((c) => `<li>${c.name}: ${c.options.filter((o) => o.possible).map((o) => o.name).join(\", \")}</li>`);\n  return `<ul>${rows.join(\"\")}</ul>`;\n}\n";

/// Edges that say how two ideas relate, and nothing more.
const RELATIONS: &[(&str, &str)] = &[
    ("serves", "Contributes to that, without being part of it."),
    ("supports", "Evidence for that."),
    ("contradicts", "Evidence against that, or a risk threatening it."),
    ("constrains", "A standing rule that applies to that."),
    ("supersedes", "Replaces that."),
    ("relates", "Related, in a way nobody has pinned down yet."),
];

pub fn install(reg: &mut Registry) {
    reg.component(Named::NAME, "What it's called.").everywhere();
    reg.cell::<Name>(Named::NAME, "What it's called. It isn't an identity: two nodes can share one.").per_entity().initial(json!(""));
    reg.component(NOTES, "Notes, in markdown.").icon("¶");
    reg.cell::<Body>(NOTES, "Notes, in markdown.").initial(json!(""));
    reg.component(SOURCES, "Where this came from, so a claim can be followed back.").icon("↩");
    reg.cell::<Sources>(SOURCES, "Where this came from, so a claim can be followed back.").initial(json!([]));
    reg.component(DRAWING, "A way of drawing what's still possible, which the worlds panel can show.").icon("✎");
    reg.cell::<Drawing>(DRAWING, "TypeScript exporting draw(world), which returns SVG or HTML.").initial(json!(NEW_DRAWING));
    for (name, about) in RELATIONS {
        reg.edge(name, about);
    }
}
