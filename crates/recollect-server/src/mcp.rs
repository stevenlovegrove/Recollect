//! MCP tools over the open workspace (streamable HTTP at /mcp).

use std::sync::Arc;

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
components, such as `decision` on a node or `part_of` on an edge, and each component brings cells: `choice` brings `chosen`, an \
`alternative` edge brings `weight`. What's in a cell is written there (a value or a formula), taken from the node's kind, or the \
component's default, which may be worked out. Components require others, and an edge's components bring components to its ends. A \
workspace can also have cells of its own, on nodes and on edges, which no component brings.\n\
\n\
Five tools. `overview` first: the workspace's own cells, kinds and tables, what each of its systems gives, and a guide to each system, \
which says how to use it. `read` to look, `apply` to change, `changes` to see what happened since you last looked (the user works in the \
app between your turns), and `checks` for what's broken or worth a look. Each answers in YAML unless `format: json` is asked for.\n\
\n\
`read` takes a specification and gives everything that matches in one go. By default a node comes back as its file says it: \
`id`, `components`, `cells` and `formulas` as written, and `links` to other nodes with their `to_name` beside each handle, and \
nothing else. Say what you want: `nodes: {ids: [Delta, Header]}`, `{kind: Concept}`, `{component: decision}`, `{name: \"Delta*\"}`, \
`{text: \"some words\"}` or `{tree: {root: X, depth: 3}}`, combined as and; `cells: [body, \"layer*\"]`; `edges: {components: [relates], \
incoming: true, cells: false}`; `worked_out: true` for each cell's value in the worlds in play and where it came from, in a section of its \
own; `history: 3` for the latest events that touched each node. `world` pins choices to options, by name or handle, to read everything as \
it would be then. `where` looks only at the worlds where a true-or-false formula holds: plain text when it names no nodes, as \
`all(d in with.leaning: d.chosen = d.leaning)`, or `{formula: \"chose(d, h)\", labels: {d: <decision>, h: <option>}}`. A world where it \
can't be told is kept, and the answer says how much was. Looking where something uncertain holds is supposing it's found out, and the \
answer says so. That's how to answer what-if questions, and it's the same code as really deciding.\n\
\n\
Values that depend on open decisions are exact: each is the set of worlds where it holds, shown as a formula such as `calib∈{cad,free}`. \
A value that's the same in every world in play is one value.\n\
\n\
Write with `apply`, which commits at once: the user sees it as it lands, and can take any one change back from History. Ops are: \
{op: add_node, id: \"new:x\", name, body?, components?: [component], cells?: {cell: value or {formula, labels}}, links?: [{to, component, cells?}]}; \
{op: add, id, component} and {op: remove, id, component}; \
{op: set, id, cell, value} or {op: set, id, cell, formula, labels?} or {op: set, id, cell} to take away what's written; \
the same with `from` and `to` in place of `id` for a cell of the edge between two nodes; \
{op: link, from, to, component, cells?} and {op: unlink, from, to, component}; {op: delete_node, id}. Writing in a cell of a component \
a node doesn't have gives it that component, and using a component takes on the system it comes from, which `overview` lists. Don't \
write a component an edge already gives its ends. Nodes can be named by handle or by name, and \"new:anything\" stands for a node \
that doesn't exist yet, the same one wherever it appears in a change-set. A change is all or nothing, and refused, with the reason, if \
it wouldn't apply.\n\
\n\
Search before adding, and prefer changing to adding. Say why in every change's `why`, and cite where claims came from in a node's \
`sources`. Reasons belong in the log, not in a node's text.";

#[derive(Clone)]
pub struct RecollectMcp {
    app: Arc<App>,
    tool_router: ToolRouter<Self>,
}

impl RecollectMcp {
    pub fn new(app: Arc<App>) -> Self {
        RecollectMcp { app, tool_router: Self::tool_router() }
    }

    fn looking<T: Serialize>(&self, f: impl FnOnce(&Open) -> Result<T, String>, format: Option<Format>) -> Result<String, String> {
        let v = self.app.with(|o| f(&o.ws).map_err(crate::app::AppError::Refused)).map_err(|e| e.to_string())?;
        written(&serde_json::to_value(v).map_err(|e| e.to_string())?, format)
    }
}

/// A world given by names or handles, as the engine knows it.
fn world_of(o: &Open, given: Option<World>) -> Result<World, String> {
    let Some(given) = given else { return Ok(World::new()) };
    let open = views::view(o, &World::new()).worlds.vars;
    let mut out = World::new();
    for (choice, option) in given {
        // A pin the engine already understands, such as a quantity's, goes through as it is.
        if choice.starts_with('=') || choice.starts_with('!') {
            out.insert(choice, option);
            continue;
        }
        let var = open.iter().find(|v| v.id == choice || v.name == choice).ok_or_else(|| format!("“{choice}” isn't an open choice"))?;
        let opt = var.options.iter().find(|x| x.id == option || x.name == option).ok_or_else(|| format!("“{option}” isn't an option of “{}”", var.name))?;
        out.insert(var.id.clone(), opt.id.clone());
    }
    Ok(out)
}

/// How an answer is written.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// The shape of the workspace's own files.
    #[default]
    Yaml,
    Json,
}

fn written(v: &Value, format: Option<Format>) -> Result<String, String> {
    match format.unwrap_or_default() {
        Format::Yaml => serde_yaml_ng::to_string(v).map_err(|e| e.to_string()),
        Format::Json => serde_json::to_string_pretty(v).map_err(|e| e.to_string()),
    }
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct OverviewParams {
    /// Everything said about each component and cell, rather than one line for each component.
    pub full: Option<bool>,
    /// Systems the workspace doesn't use yet whose guides and components to give too, such as ["plan"].
    pub also: Option<Vec<String>>,
    pub format: Option<Format>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ReadParams {
    /// Which nodes: {ids: [handle or name]}, {kind: name}, {component: name}, {name: "wild*card"}, {text: "words"} or {tree: {root, depth?}}, combined as and. Every node, if not given.
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub nodes: Option<Value>,
    /// Which cells, by name or with * as a wildcard. What was written, if not given. The name always comes.
    pub cells: Option<Vec<String>>,
    /// Which edges: {components?: [name], outgoing?: true, incoming?: false, cells?: true}.
    #[schemars(with = "Option<serde_json::Map<String, Value>>")]
    pub edges: Option<Value>,
    /// Each cell's value in the worlds in play, and where it comes from, in a section of its own.
    pub worked_out: Option<bool>,
    /// How many of the latest events that touched each node.
    pub history: Option<usize>,
    /// Choices pinned to options, keyed by choice, by name or handle.
    pub world: Option<World>,
    /// Only the worlds where this true-or-false formula holds: text, or {formula, labels: {name: node}}.
    #[serde(rename = "where")]
    #[schemars(rename = "where", with = "Option<Value>")]
    pub where_: Option<Value>,
    /// The most nodes. 100 if not given.
    pub limit: Option<usize>,
    /// Where each piece of work stands and what each decision is holding up, for a plan.
    pub plan: Option<bool>,
    pub format: Option<Format>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ChangesParams {
    /// Only events after this one. Each answer gives each event's seq.
    pub since: Option<u64>,
    /// Only what touched this node, by handle or name.
    pub node: Option<String>,
    /// The most events, newest first. 50 if not given.
    pub limit: Option<usize>,
    pub format: Option<Format>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ChecksParams {
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplyParams {
    /// Why these changes, and what evidence they rest on. Recorded in the log.
    pub why: String,
    /// Applied in order, atomically. See the server instructions for their form.
    #[schemars(with = "Vec<serde_json::Map<String, Value>>")]
    pub ops: Vec<Value>,
}

/// Only what was given, as the engine reads it.
fn given(pairs: Vec<(&str, Option<Value>)>) -> Value {
    Value::Object(pairs.into_iter().filter_map(|(k, v)| Some((k.to_string(), v?))).collect())
}

#[tool_router]
impl RecollectMcp {
    #[tool(description = "The workspace: its own cells, kinds and tables; what each system it uses gives, with a guide to using it; the systems it doesn't use yet; what checks report; and its open choices. Call first.")]
    pub(crate) async fn overview(&self, Parameters(p): Parameters<OverviewParams>) -> Result<String, String> {
        let also = p.also.unwrap_or_default();
        self.looking(|o| Ok(views::overview(o, p.full.unwrap_or(false), &also)), p.format)
    }

    #[tool(description = "Read what a specification asks for, in one go, shaped like the workspace's files: which nodes, which of their cells and edges, and whether to add what's worked out, recent history, or where a plan stands. Pins and a where formula read it as it would be in those worlds. See the server instructions.")]
    pub(crate) async fn read(&self, Parameters(p): Parameters<ReadParams>) -> Result<String, String> {
        let world = self.app.with(|o| world_of(&o.ws, p.world).map_err(crate::app::AppError::Refused)).map_err(|e| e.to_string())?;
        let spec = given(vec![
            ("nodes", p.nodes),
            ("cells", p.cells.map(|c| json!(c))),
            ("edges", p.edges),
            ("worked_out", p.worked_out.map(Value::Bool)),
            ("history", p.history.map(|h| json!(h))),
            ("world", Some(json!(world))),
            ("where", p.where_),
            ("limit", p.limit.map(|l| json!(l))),
            ("plan", p.plan.map(Value::Bool)),
        ]);
        let v = self.app.call("read", spec, "claude").map_err(|e| e.to_string())?;
        written(&v, p.format)
    }

    #[tool(description = "What's happened, newest first: each event's seq, who, when and why, what it did and what that replaced, with names beside handles. Give since: <seq> to see only what's new since you last looked.")]
    pub(crate) async fn changes(&self, Parameters(p): Parameters<ChangesParams>) -> Result<String, String> {
        let spec = given(vec![("since", p.since.map(|s| json!(s))), ("node", p.node.map(Value::String)), ("limit", p.limit.map(|l| json!(l)))]);
        let v = self.app.call("changes", spec, "claude").map_err(|e| e.to_string())?;
        written(&v, p.format)
    }

    #[tool(description = "What the checks report: invariants that are broken, and notices worth a look. Use as a starting point, and look beyond them.")]
    pub(crate) async fn checks(&self, Parameters(p): Parameters<ChecksParams>) -> Result<String, String> {
        self.looking(|o| Ok(views::view(o, &World::new()).checks), p.format)
    }

    #[tool(description = "Commit a change-set. It's all or nothing, and refused, with the reason, if it wouldn't apply or would newly break an invariant. The user can take it back from History.")]
    pub(crate) async fn apply(&self, Parameters(p): Parameters<ApplyParams>) -> Result<String, String> {
        let done = self.app.call("apply", json!({ "ops": p.ops, "why": p.why }), "claude").map_err(|e| e.to_string())?;
        written(&done, Some(Format::Yaml))
    }
}

#[tool_handler(router = self.tool_router, name = "recollect")]
impl ServerHandler for RecollectMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new("recollect", env!("CARGO_PKG_VERSION"))).with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests;
