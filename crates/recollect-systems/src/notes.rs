//! What any node can have, whatever else it is: a name, notes, and where it came from. Edges that only
//! say how two ideas relate live here too, because nothing computes with them.

use std::collections::BTreeSet;

use recollect_graph::{CellType, ComponentType, Cx, Entity, Error, Name, Named, Registry, Severity, cell};
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

/// Where a node's card was put on the map by hand, if it was: a view's to read, saved with the node so
/// it goes where the workspace goes.
#[cell("placed")]
pub struct Placed {
    pub x: f64,
    pub y: f64,
}

/// Where the card of the table a node names, which stands for its rows, was put on the map by hand.
#[cell("placed_rows")]
pub struct PlacedRows {
    pub x: f64,
    pub y: f64,
}

/// A kind of edge a workspace defines, such as *names*: a node, as a kind is, which edges of that kind
/// take the cells they don't say from.
pub const RELATION: &str = "relation";
pub const RELATES: &str = "relates";
/// A format or a trait realises a concept: it's how that concept is in a later layer.
pub const REALISES: &str = "realises";

/// On an edge: its kind, `{ref: <relation>}`. It takes what it doesn't say from there, and can be
/// followed by that relation's name, as `in.names`.
#[cell("kind")]
pub struct EdgeKind(pub Value);

/// On a relation: whether its edges lay the map and the outline out, as part_of does.
#[cell("arranges")]
pub struct Arranges(pub bool);

/// On a relation that arranges: what its edges point at goes beneath, not above.
#[cell("reversed")]
pub struct Reversed(pub bool);

/// On a relation: the kind its edges come from, if they all should.
#[cell("from_kind")]
pub struct FromKind(pub Value);

/// On a relation: the kind its edges go to, if they all should.
#[cell("to_kind")]
pub struct ToKind(pub Value);

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
    ("relates", "Related: say how with its kind, a relation of the workspace's own."),
    ("realises", "How that is in a later layer, as a format or a trait realises a concept."),
];

pub fn install(reg: &mut Registry) {
    reg.component(Named::NAME, "What it's called.").everywhere();
    reg.cell::<Name>(Named::NAME, "What it's called. It isn't an identity: two nodes can share one.").per_entity().initial(json!(""));
    reg.cell::<Placed>(Named::NAME, "Where its card was put on the map by hand, {x, y}, if it was.").per_entity().quiet();
    reg.cell::<PlacedRows>(Named::NAME, "Where the card of the table it names was put on the map by hand, {x, y}, if it was.").per_entity().quiet();
    reg.component(NOTES, "Notes, in markdown.").icon("¶");
    // A kind's notes say what the kind is, which isn't something each instance says about itself.
    reg.cell::<Body>(NOTES, "Notes, in markdown.").initial(json!("")).per_entity();
    reg.component(SOURCES, "Where this came from, so a claim can be followed back.").icon("↩");
    reg.cell::<Sources>(SOURCES, "Where this came from, so a claim can be followed back.").initial(json!([]));
    reg.component(DRAWING, "A way of drawing what's still possible, which the worlds panel can show.").icon("✎");
    reg.cell::<Drawing>(DRAWING, "TypeScript exporting draw(world), which returns SVG or HTML.").initial(json!(NEW_DRAWING));
    for (name, about) in RELATIONS {
        reg.edge(name, about);
    }

    reg.component(RELATION, "A kind of edge the workspace defines: edges of this kind take the cells they don't say from here.").icon("↔");
    reg.cell::<Arranges>(RELATION, "Whether its edges lay the map and the outline out.").initial(json!(true));
    reg.cell::<Reversed>(RELATION, "Whether what its edges point at goes beneath, not above.").initial(json!(false));
    reg.cell::<FromKind>(RELATION, "The kind its edges come from, if they all should.").validate(one_node);
    reg.cell::<ToKind>(RELATION, "The kind its edges go to, if they all should.").validate(one_node);
    reg.cell::<EdgeKind>(RELATES, "Its kind: a relation of the workspace's own, which says what it means.").validate(one_node).per_entity();
    reg.edge_kind_cell(EdgeKind::NAME);
    checks(reg);
}

fn one_node(v: &Value) -> Result<(), String> {
    match v.get("ref").and_then(Value::as_str).map(str::parse::<Entity>) {
        Some(Ok(_)) => Ok(()),
        _ => Err(format!("this names a node, as {{\"ref\": handle}}, not {v}")),
    }
}

fn node_ref(v: &Value) -> Option<Entity> {
    v.get("ref")?.as_str()?.parse().ok()
}

/// Whether a node is an instance of a kind: it links to it by `is_a`, or to something that does.
fn is_instance(cx: &Cx, node: Entity, kind: Entity) -> bool {
    let mut seen = BTreeSet::from([node]);
    let mut todo = vec![node];
    while let Some(n) = todo.pop() {
        for k in cx.outgoing_of(n, "is_a") {
            if k == kind {
                return true;
            }
            if seen.insert(k) {
                todo.push(k);
            }
        }
    }
    false
}

fn checks(reg: &mut Registry) {
    use Severity::Notice;

    reg.check(RELATES, "edge_of_the_wrong_kinds", Notice, |cx| {
        let Some(EdgeKind(v)) = cx.own::<EdgeKind>()? else { return Ok(vec![]) };
        let Some(kind) = node_ref(&v) else { return Ok(vec![]) };
        if !cx.has(kind, RELATION) {
            return Ok(vec![format!("its kind is {kind}, which isn't a relation")]);
        }
        let (from, to) = cx.ends().expect("an edge");
        let mut out = Vec::new();
        for (end, which, cell) in [(from, "from", FromKind::NAME), (to, "to", ToKind::NAME)] {
            let said = if cell == FromKind::NAME { cx.get::<FromKind>(kind)?.map(|k| k.0) } else { cx.get::<ToKind>(kind)?.map(|k| k.0) };
            if let Some(want) = said.as_ref().and_then(node_ref)
                && !is_instance(cx, end, want)
            {
                out.push(format!("goes {which} {end}, which isn't a {want}, as {kind} says it should be"));
            }
        }
        Ok(out)
    });

    reg.check(RELATION, "arranging_loop", Notice, |cx| {
        if !cx.own::<Arranges>()?.is_some_and(|a| a.0) {
            return Ok(vec![]);
        }
        Ok(match loop_of(cx, cx.id())? {
            Some(at) => vec![format!("its edges go round in a loop through {at}, so the map can't lay them out")],
            None => vec![],
        })
    });
}

/// A node its edges lead back to, following them from source to target, if they make a loop.
fn loop_of(cx: &Cx, kind: Entity) -> Result<Option<Entity>, Error> {
    let Some(Name(name)) = cx.get::<Name>(kind)? else { return Ok(None) };
    // Every node an edge of this kind leaves, as where a loop could start.
    let mut starts: Vec<Entity> = cx.scan(Named::NAME).into_iter().filter(|n| !cx.outgoing_at(*n, &name).is_empty()).collect();
    starts.sort();
    for start in starts {
        let mut seen = BTreeSet::new();
        let mut todo = vec![start];
        while let Some(n) = todo.pop() {
            for (next, _) in cx.outgoing_at(n, &name) {
                if next == start {
                    return Ok(Some(start));
                }
                if seen.insert(next) {
                    todo.push(next);
                }
            }
        }
    }
    Ok(None)
}

/// How to use what this system gives, for an agent: see `mcp.rs` in the server.
pub const GUIDE: &str = "\
Every node has a `name`. Its notes are `body`, in markdown, and `sources` says where a claim came from, so it can be followed back. \
Reasons for a change belong in its `why`, which is in the log, not in a node's text.\n\
\n\
Edges say how ideas relate: `serves`, `supports`, `contradicts`, `constrains`, `supersedes`, `realises` (a format or a trait is how \
a concept is in a later layer) and `relates`. An edge can carry cells of the workspace's own, as a node can. A workspace defines its own \
kinds of edge as relations: a node with the `relation` component, such as *names*, whose notes say what it means, with `arranges` (its \
edges lay out the map and outline), `reversed`, `from_kind` and `to_kind` (as {ref}), and any cells every edge of the kind has, such as \
`many: true`. Link with {op: link, from, to, kind: \"names\"}, which makes a `relates` edge whose `kind` names it: the edge takes the cells \
it doesn't say from the relation, and a formula can follow it as `in.names` or `out.names`.\n\
\n\
A node with a `drawing` cell holding TypeScript shows the problem the way it's really thought about: `export function draw(world: World): \
string` returns SVG or HTML, and the app's worlds panel can show it. world.choices is [{id, name, options: [{id, name, possible, picked, \
gap, weight}]}], already narrowed to what's pinned, and world.nodes is [{id, name, cells}], with world.edges, world.count and world.picks. \
An element with data-pin-choice and data-pin-option pins that option when clicked, and unpins it if it's already pinned. Colours are CSS \
variables: --text, --muted, --accent, --bg, --panel, --border. It runs sandboxed with no network, no DOM and no memory between worlds, so \
everything it shows has to come from `world`. There can be several drawings.";
