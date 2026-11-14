//! MCP tools over the open workspace (streamable HTTP at /mcp).

use std::sync::Arc;

use recollect_graph::Entity;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use recollect_core::Workspace as Open;
use recollect_core::views::{self, World};
use serde_json::json;

use crate::app::App;

const INSTRUCTIONS: &str = "\
Recollect holds a graph the user develops with your help. A node and an edge (from one node to another) are both entities. Each has \
components, such as `task` or `decision` on a node and `part_of` or `label` on an edge, and each component brings cells: `task` brings \
`estimate`, `choice` brings `chosen`, an `alternative` edge brings `weight`. A cell is only there on an entity that has its component. \
What's in a cell is written there (a value or a formula), taken from the node's kind, or the component's default, which may be worked \
out, or filled in by a component that requires it (`decision` and `options` each say how a choice lands). Components require others (a task is work, which is part of a plan), and an edge's components bring components to its ends (an \
`alternative` edge makes its source an `option` and its target a `decision`). get_overview lists every component and cell this \
workspace's systems provide, and what each means. Start there.\n\
\n\
In a planning workspace, structure is coarse to fine. `part_of` edges point child to parent, and all of a node's parts are needed. A \
`decision` node's options are ordinary nodes with an `alternative` edge to it, and only the option taken is needed, so work that several \
options share is simply part_of each of them, and is safe to do before deciding. Finding that overlap is worth effort. `depends_on` is \
ordering, inherited by what's inside. An `unknown` node is a gap: it says something here isn't specified yet, and removing it is the \
claim that it's closed. `done` is true or false, and is worked out from what's beneath unless a node says otherwise. A stated `done` \
that disagrees with what's beneath gets a notice, so to finish something early, say so and drop what's left. `dropped` means \
deliberately out of the picture. Any cell that's worked out can be written over, and taking away what was written brings back what \
it's worked out to.\n\
\n\
A workspace can have cells of its own, such as `price` or `range`, which no component brings and any node can have. When several nodes \
are the same kind of thing, make a node for the kind and link each to it with an `is_a` edge: they have its components, and take any \
cell they don't write themselves from it (except a node's own `name` and `value`), and a formula on the kind runs for each of them, \
with `self` meaning that node and its other names labelled once on the kind. A car's mpg goes on the car, and how mpg and the fuel price \
make a cost a mile goes on Petrol car. A choice's `chosen` cell is the node it lands on in each world, and a dot reads through it to \
that node's cells: with `car` labelling a decision, `car.chosen.range >= trip` reads the range of whichever car it lands on, and \
`miles.chosen.value` the value of whichever option. `car.chosen = tom` compares nodes. Give a \
decision `uncertain` when it isn't anyone's to choose, such as how far someone will drive: its options' weights then say how far each \
is believed, and what's expected averages over it.\n\
\n\
A name makes a table. A node named `Year[3]` is row 3 of the table `Year`, and `Car[Hatchback]` is the Hatchback row of `Car`: each \
takes what it doesn't say from the node named `Year` or `Car`, as from a kind, with no `is_a` edge needed. A formula there reads \
`prev.balance` for the row before (`next`, `prev.prev`, `row` for which row this is, and `rows` for all of them), so a recurrence is \
one formula on the table and a starting value on the first row. A numbered table goes on past its last row by itself, so \
`Year[100].balance` works without writing 100 rows: those rows take everything from the table and aren't saved. If Petrol car `is_a` Car, then `Petrol car[Hatchback]` and \
`Car[Hatchback]` are parts of one car: put what every car has on the Car row and what only petrol cars have on the Petrol car row. \
A table's columns are cells of the workspace's own, such as `balance`, since `value` is each node's own answer. \
A node with `options` (a plain list, such as [\"red\", \"green\"]) is a choice between them: each option is a node nobody wrote, whose \
`name` and `value` are the option, so `a.chosen.value = \"red\"`, and two choices landing on the same option land on the same node, so \
`a.chosen != b.chosen` says they differ. A table can be a table of choices: give the table node `options` and each row is a choice from \
them, while the table isn't a choice itself. For a puzzle, make a table for each kind of thing, such as `Pet[Zebra]` choosing a house, \
and write each clue as a formula naming the rows it's about, with nothing to label: `Nationality[English].chosen = Colour[Red].chosen`, \
or `Drink[Milk].chosen.value = 3`. Say all different with `distinct`: `distinct(r in rows: r.chosen)` as the table's own value, \
`distinct(q in rows: q.chosen.value + q.row)` where `q.row` is a row's key, or `distinct(Cell[A1].chosen, Cell[A2].chosen)`. \
Anywhere, `Arrived.rows` is a table's rows by its name and `Left[v.row]` finds the row with the same key as one being gone over: \
`count(v in Arrived.rows: v < 9.5 && Left[v.row] > 9.5) = 1`. A statement someone made is a row whose formula is what they said, so \
\"Dee is lying\" is `!Says[Dee]`. \
Write a whole table with one op: `{\"op\": \"table\", \"name\": \"Year\", \"columns\": {\"balance\": {\"formula\": \"prev.balance * (1 + rate)\", \
\"labels\": {\"rate\": \"Rate\"}}}, \"range\": [0, 10], \"rows\": [{\"key\": 0, \"balance\": 1000}]}`, with `is_a` for a table that's a kind of \
another, `components` for what every row is, and `alternative_of` to make every row an option of a decision.\n\
\n\
Values that depend on open decisions are exact: each is the set of worlds where it holds, shown as a formula such as `calib∈{cad,free}`. \
A world is one way every open choice could land. Tools that take a `world` read everything as it would be with those choices pinned, \
keyed by choice and option, by name or handle. That's how to answer what-if questions, and it's the same code as really deciding.\n\
\n\
Typical jobs: ELABORATE a coarse node. DECOMPOSE it (get_tree first, reusing existing nodes wherever the same work appears, and stop \
when a leaf could be started today). Find what's shared. Surface RISKS, with a `contradicts` edge to what they threaten. Suggest what \
would discriminate between the options of an open decision. Search before adding, and prefer dropping to deleting. Say why in every \
change's `why`, and cite where claims came from in a node's `sources`. Reasons belong in the log, not in a node's text.\n\
\n\
Write with `apply`, which commits at once: the user sees it as it lands, and can take any one change back from History. Ops are: \
{op: add_node, id: \"new:x\", name, body?, components?: [component], cells?: {cell: value or {formula, labels}}, links?: [{to, component, cells?}]}; \
{op: add, id, component} and {op: remove, id, component}; \
{op: set, id, cell, value} or {op: set, id, cell, formula, labels?} or {op: set, id, cell} to take away what's written; \
the same with `from` and `to` in place of `id` for a cell of the edge between two nodes, such as an option's `weight`; \
{op: link, from, to, component, cells?} and {op: unlink, from, to, component}; {op: delete_node, id}. Writing in a cell of a component \
a node doesn't have gives it that component. Nodes can be named by handle or by name, and \"new:anything\" stands for a node that \
doesn't exist yet, the same one wherever it appears in a change-set. So decide on a decision with {op: set, id: \"How do we publish it?\", \
cell: chosen, value: {ref: <option's handle>}}, and rule an option out with {op: set, from: <option>, to: <decision>, cell: ruled_out, value: \"why\"}.\n\
\n\
A node's `value` can be typed in, or a formula, and so can a task's `estimate` and any of the workspace's own cells. That's how to state \
a claim the app can hold you to, rather than only describing it. Write {op: set, id, cell: value, formula: \"prev * (1 + rate)\", labels: \
{prev: \"Year 2\", rate: \"Rate\"}}. A label is an edge whose `called` cell is the name a formula on its source uses for its target, and \
several nodes under one name make a group. A formula means the same thing in every world: \
what you'd get with ordinary values there, so it needs no thought about worlds, and `a + b` works whether or not either varies. \
Operators: + - * / = != < <= > >= && || !. A number, \\\"text\\\", true, false or undefined. `rate` is that node's value, `rate.finish` another of its cells, and `self.need` one of this node's. \
Functions: if(c, a, b), defined(x), or(x, fallback), abs, min(a, b), max(a, b), round(x, places?), floor, ceil, has(x.c) (whether x has component c, or else something in cell c), and chose(choice, option), which is true where that choice lands on that option. \
Over a group, which is a label, in.component or out.component (the nodes at the other end of those edges) or with.component (the nodes with it): sum, product, all, any, count and distinct: sum(line), sum(t in task: if(t.need, t.estimate, 0)). \
In a fold over edges, `o.via.weight` reads a cell of the edge `o` was reached along, and a formula on an edge reads its ends as `from` and `to`. \
Say what a total is over by labelling it: a plan's days are sum(t in task: if(t.need, t.estimate, 0)) with each task labelled `task`, and when something ends is its own `finish`, as goal.finish. with.component is every such node in the workspace, so only use it when that's what's meant. Division by zero and missing inputs are undefined, only in the worlds where that's so, which says what isn't known. \
chance(x) is how likely x is, and mean(x) what it comes to on average, over everything that's a matter of chance and never over a choice: so `requires` can say chance(cost < 4000) >= 0.9. Where either has no exact answer it's an estimate, which says so. \
A `knowing` edge from a decision to something says it's made knowing how that turns out, so what's required of it is judged for each way it could. \
A number can be uncertain: normal(mean, sd), lognormal(mean, sd), uniform(low, high), triangular(low, likely, high), exponential(mean), between(low, high) for about that nine times in ten, integers(low, high) and bernoulli(p). Each place one is written is its own draw, and a cell read twice is the same one. What comes back says what it comes to and whether that's exact or read from draws. \
A node with `requires` and a true-or-false value is a demand on choices: only the ways they could go that make it true count, which is how to say a constraint must be met. Where it depends on chance, its `at_least` cell says how surely, from 0 to 1, and it's always unless it says. \
A node with `evidence` is something known to be true: it rules out the worlds, and the draws, where it's false, and never a choice. Use `evidence` for what was observed and `requires` for what's wanted. \
To reuse logic, a node with a `macro` cell {params: [t], body: \"t.estimate * 100\"} is a function its callers link to with a label. A formula with a type error or an unknown input is refused, with the reason.\n\
\n\
To show a problem the way it's really thought about, add a node with a `drawing` cell holding TypeScript: `export function draw(world: World): string` returns SVG or HTML, and the Worlds tab can show it. \
world.choices is [{id, name, options: [{id, name, possible, picked, gap, weight}]}], already narrowed to what's pinned, and world.nodes is [{id, name, cells}], with world.edges, world.count and world.picks. \
An element with data-pin-choice and data-pin-option pins that option when clicked, and unpins it if it's already pinned. Colours are CSS variables: --text, --muted, --accent, --bg, --panel, --border. \
It runs sandboxed with no network, no DOM and no memory between worlds, so everything it shows has to come from `world`, and anything that would load or run something is removed from what it returns. There can be several drawings.";

#[derive(Clone)]
pub struct RecollectMcp {
    app: Arc<App>,
    tool_router: ToolRouter<Self>,
}

impl RecollectMcp {
    pub fn new(app: Arc<App>) -> Self {
        RecollectMcp { app, tool_router: Self::tool_router() }
    }

    fn read<T: Serialize>(&self, f: impl FnOnce(&Open) -> Result<T, String>) -> Result<String, String> {
        let v = self.app.with(|o| f(&o.ws).map_err(crate::app::AppError::Refused)).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
    }
}

/// A node by handle or by name.
fn node_of(o: &Open, s: &str) -> Result<Entity, String> {
    if let Ok(e) = s.parse::<Entity>() {
        return if o.log.graph().entities().any(|x| x == e) { Ok(e) } else { Err(format!("there's no node {s}")) };
    }
    let g = o.log.graph();
    let found: Vec<Entity> = g.entities().filter(|e| recollect_systems::label_of(g, *e) == s).collect();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("there's no node called “{s}”")),
        many => Err(format!("{} nodes are called “{s}”: use a handle", many.len())),
    }
}

/// A world given by names or handles, as the engine knows it.
fn world_of(o: &Open, given: Option<World>) -> Result<World, String> {
    let Some(given) = given else { return Ok(World::new()) };
    let open = views::view(o, &World::new()).worlds.vars;
    let mut out = World::new();
    for (choice, option) in given {
        let var = open.iter().find(|v| v.id == choice || v.name == choice).ok_or_else(|| format!("“{choice}” isn't an open choice"))?;
        let opt = var.options.iter().find(|x| x.id == option || x.name == option).ok_or_else(|| format!("“{option}” isn't an option of “{}”", var.name))?;
        out.insert(var.id.clone(), opt.id.clone());
    }
    Ok(out)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NodeParams {
    /// A handle or a name.
    pub node: String,
    /// Choices pinned to options, to read it as it would be then.
    pub world: Option<World>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// Whitespace-separated terms. All must match a node's name or notes.
    pub query: String,
    /// Default 25.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    /// Only nodes with this component, however they came to have it.
    pub component: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TreeParams {
    /// A handle or a name.
    pub root: String,
    /// Levels to expand. Default 6.
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WorldParams {
    /// Choices pinned to options, keyed by choice, by name or handle. Leave it out for all of them open.
    pub world: Option<World>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HistoryParams {
    /// Only what touched this node, by handle or name.
    pub node: Option<String>,
    /// Default 50.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplyParams {
    /// Why these changes, and what evidence they rest on. Recorded in the log.
    pub why: String,
    /// Applied in order, atomically. See the server instructions for their form.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub ops: Vec<Value>,
}

#[tool_router]
impl RecollectMcp {
    #[tool(description = "The workspace: which systems it uses, every component and the cells it brings and what each means, what checks currently report, its open choices, and how much is pending. Call first.")]
    async fn get_overview(&self) -> Result<String, String> {
        self.read(|o| Ok(views::overview(o)))
    }

    #[tool(description = "One node: its components and why it has each, every cell and where what's in it comes from, its edges either way with their cells, and the history of what touched it. Takes a world to read it as it would be then.")]
    async fn get_node(&self, Parameters(p): Parameters<NodeParams>) -> Result<String, String> {
        self.read(|o| {
            let world = world_of(o, p.world)?;
            Ok(views::node(o, node_of(o, &p.node)?, &world))
        })
    }

    #[tool(description = "Find nodes by text in their name or notes.")]
    async fn search_nodes(&self, Parameters(p): Parameters<SearchParams>) -> Result<String, String> {
        self.read(|o| Ok(views::search(o, &p.query, p.limit.unwrap_or(25))))
    }

    #[tool(description = "List nodes, optionally only those with a component.")]
    async fn list_nodes(&self, Parameters(p): Parameters<ListParams>) -> Result<String, String> {
        self.read(|o| Ok(views::list(o, p.component.as_deref())))
    }

    #[tool(description = "What's beneath a node, nested, with the worlds each piece is needed in. Use before decomposing further.")]
    async fn get_tree(&self, Parameters(p): Parameters<TreeParams>) -> Result<String, String> {
        self.read(|o| Ok(views::tree(o, node_of(o, &p.root)?, p.depth.unwrap_or(6))))
    }

    #[tool(description = "The whole graph as it would be in a world: every node's cells narrowed to it, the open choices and which options are still possible, how many worlds remain, and, for a puzzle, its solutions. Use it for what-if questions. It can be large, so prefer get_work, get_decisions or get_node when they'll do.")]
    async fn read_world(&self, Parameters(p): Parameters<WorldParams>) -> Result<String, String> {
        self.read(|o| {
            let world = world_of(o, p.world)?;
            Ok(views::view(o, &world))
        })
    }

    #[tool(description = "Work that can be picked up, and where it stands in a world: could start now, depends on a decision, waiting (and on what), finished, or not needed.")]
    async fn get_work(&self, Parameters(p): Parameters<WorldParams>) -> Result<String, String> {
        self.read(|o| {
            let world = world_of(o, p.world)?;
            Ok(views::planning(o, &world)?.work)
        })
    }

    #[tool(description = "Every decision: its options, how much of each option's worlds need each piece of work beneath it, and whether it's holding work up (pressure: free, blocking, idle, decided).")]
    async fn get_decisions(&self, Parameters(p): Parameters<WorldParams>) -> Result<String, String> {
        self.read(|o| {
            let world = world_of(o, p.world)?;
            Ok(views::planning(o, &world)?.decisions)
        })
    }

    #[tool(description = "What the checks report: invariants that are broken, and notices worth a look. Use as a starting point, and look beyond them.")]
    async fn find_checks(&self) -> Result<String, String> {
        self.read(|o| Ok(views::view(o, &World::new()).checks))
    }

    #[tool(description = "What's changed since the workspace was opened: who changed what, when and why, newest first.")]
    async fn get_history(&self, Parameters(p): Parameters<HistoryParams>) -> Result<String, String> {
        self.read(|o| {
            let node = p.node.as_deref().map(|n| node_of(o, n)).transpose()?;
            let mut h = views::history(o, node);
            h.truncate(p.limit.unwrap_or(50));
            Ok(h)
        })
    }

    #[tool(description = "Commit a change-set. It's all or nothing, and refused, with the reason, if it wouldn't apply or would newly break an invariant. The user can take it back from History.")]
    async fn apply(&self, Parameters(p): Parameters<ApplyParams>) -> Result<String, String> {
        let done = self.app.call("apply", json!({ "ops": p.ops, "why": p.why }), "claude").map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&done).map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router, name = "recollect")]
impl ServerHandler for RecollectMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new("recollect", env!("CARGO_PKG_VERSION"))).with_instructions(INSTRUCTIONS)
    }
}
