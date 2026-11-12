//! Everything the app asks of an open workspace, by name, with JSON in and out. The server's REST API
//! and the browser's engine are both this one function, so they can't drift apart.

use recollect_graph::Entity;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::views::{self, CellRef, World};
use crate::workspace::{Error, Workspace};

/// What a call changed, so a host can tell whoever's watching.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Effect {
    pub graph: bool,
    pub proposals: bool,
}

/// The most values one sweep can try, which is plenty for a chart.
pub const MOST_SWEPT: usize = 200;
/// The most rows one ask can have.
pub const MOST_ROWS: i64 = 1000;

fn args<T: DeserializeOwned>(v: Value) -> Result<T, Error> {
    serde_json::from_value(if v.is_null() { json!({}) } else { v }).map_err(|e| Error::Refused(format!("that isn't what this takes: {e}")))
}

fn to_json<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("views serialise")
}

#[derive(Deserialize, Default)]
struct WorldArgs {
    #[serde(default)]
    world: World,
}

#[derive(Deserialize)]
struct TryArgs {
    node: Entity,
    text: String,
    #[serde(default)]
    world: World,
}

#[derive(Deserialize)]
struct SweepArgs {
    set: CellRef,
    values: Vec<f64>,
    read: Vec<CellRef>,
    #[serde(default)]
    world: World,
}

#[derive(Deserialize)]
struct RowsArgs {
    table: String,
    from: i64,
    to: i64,
    #[serde(default)]
    world: World,
}

#[derive(Deserialize)]
struct HistoryArgs {
    node: Option<Entity>,
}

#[derive(Deserialize)]
struct ApplyArgs {
    ops: Vec<Value>,
    #[serde(default)]
    why: String,
}

#[derive(Deserialize)]
struct UndoArgs {
    /// Which event to take back. The newest one undo would take back, if not given.
    seq: Option<u64>,
    #[serde(default)]
    why: String,
}

#[derive(Deserialize)]
struct ProposeArgs {
    title: String,
    #[serde(default)]
    rationale: String,
    ops: Vec<Value>,
}

#[derive(Deserialize)]
struct AcceptArgs {
    id: String,
    /// Which ops to apply, by index. All of them if absent.
    ops: Option<Vec<usize>>,
}

#[derive(Deserialize)]
struct RejectArgs {
    id: String,
}

/// The methods `dispatch` knows.
pub const METHODS: &[&str] = &["view", "plan", "try", "sweep", "rows", "history", "proposals", "apply", "undo", "redo", "propose", "accept", "reject"];

/// Answer one call on the workspace. `author` is who's asking, for anything that changes it.
pub fn dispatch(ws: &mut Workspace, method: &str, a: Value, author: &str) -> Result<(Value, Effect), Error> {
    let read = |v: Value| Ok((v, Effect::default()));
    let graph = Effect { graph: true, proposals: false };
    match method {
        "view" => read(to_json(views::view(ws, &args::<WorldArgs>(a)?.world))),
        "plan" => read(to_json(views::planning(ws, &args::<WorldArgs>(a)?.world).map_err(Error::Refused)?)),
        "try" => {
            let t: TryArgs = args(a)?;
            read(to_json(views::try_formula(ws, t.node, &t.text, &t.world)))
        }
        "sweep" => {
            let s: SweepArgs = args(a)?;
            if s.values.len() > MOST_SWEPT {
                return Err(Error::Refused(format!("{} values is more than the {MOST_SWEPT} a sweep can try", s.values.len())));
            }
            read(to_json(views::sweep(ws, &s.set, &s.values, &s.read, &s.world).map_err(Error::Refused)?))
        }
        "rows" => {
            let r: RowsArgs = args(a)?;
            if r.to < r.from || r.to - r.from >= MOST_ROWS {
                return Err(Error::Refused(format!("rows {} to {} isn't a run of up to {MOST_ROWS}", r.from, r.to)));
            }
            read(to_json(views::rows(ws, &r.table, r.from, r.to, &r.world)))
        }
        "history" => read(to_json(views::history(ws, args::<HistoryArgs>(a)?.node))),
        "proposals" => read(to_json(views::proposals(ws))),
        "apply" => {
            let o: ApplyArgs = args(a)?;
            Ok((to_json(ws.apply(o.ops, author, &o.why)?), graph))
        }
        "undo" => {
            let u: UndoArgs = args(a)?;
            let seq = match u.seq {
                Some(seq) => Some(ws.undo(seq, author, &u.why)?),
                None => ws.undo_last(author)?,
            };
            Ok((json!({ "seq": seq }), graph))
        }
        "redo" => Ok((json!({ "seq": ws.redo(author)? }), graph)),
        "propose" => {
            let p: ProposeArgs = args(a)?;
            Ok((to_json(ws.propose(p.title, p.rationale, p.ops, author)?), Effect { graph: false, proposals: true }))
        }
        "accept" => {
            let p: AcceptArgs = args(a)?;
            Ok((json!({ "seq": ws.accept(&p.id, p.ops.as_deref())? }), Effect { graph: true, proposals: true }))
        }
        "reject" => {
            ws.reject(&args::<RejectArgs>(a)?.id)?;
            Ok((Value::Null, Effect { graph: false, proposals: true }))
        }
        other => Err(Error::NotFound(format!("method {other}"))),
    }
}
