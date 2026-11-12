//! Formulas, as a system. Any cell can hold a value typed in or a formula over what its node has
//! labelled. A choice lands on one node in each world, and what's read through it, as `car.chosen.price`,
//! is that node's. See `docs/expressions.md`.
//!
//! A formula's meaning doesn't mention worlds: in any world, its value is what ordinary evaluation
//! gives on its inputs' values in that world. World values are how that's worked out exactly, and
//! `tests/expr.rs` checks the two agree. That's also why nothing here can collapse worlds into one
//! number: narrowing that number couldn't then agree with really deciding.
//!
//! A formula goes through three steps. It's expanded, which replaces every name with the node it
//! means and every function with its definition. It's type-checked, without looking at any world.
//! Then it's evaluated.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use recollect_graph::table::Key;
use recollect_graph::worlds::{Bool, Cond, Space, Worldly};
use recollect_graph::{Backed, Cell, CellType, Compile, Computed, Cx, Entity, Error, Graph, Registry, Shown, cell};
use serde_json::{Value, json};

use crate::expr::{self, BinOp, CORE, Expr, Type, Val};
use crate::plan::{Done, Dropped, Need};
use crate::schedule::{Finish, LatestFinish, LatestStart, PlanEnd, Start};

/// The most distinct values a formula can have across worlds. Beyond this, it says so.
const MOST_CASES: usize = 10_000;
/// The most pairs of cases one step can look at.
const MOST_PAIRS: usize = 4_000_000;
/// The most steps in expanding one formula. Macros calling macros can get very big very quickly.
pub(crate) const MOST_STEPS: usize = 200_000;

pub const VALUE: &str = "value";
pub const CHOICE: &str = "choice";
pub const OPTIONS: &str = "options";
pub const AT: &str = "at";
pub const REQUIRES: &str = "requires";
pub const MACRO: &str = "macro";
pub const LABEL: &str = "label";
pub const IS_A: &str = "is_a";
pub const STANDS_FOR: &str = "stands_for";

/// A node's answer. Typed in, or a choice from its options, or a formula.
#[cell("value")]
pub struct ValueCell(pub Worldly<Val>);

impl ValueCell {
    pub fn plain(v: Val) -> Self {
        ValueCell(Worldly::plain(v))
    }

    pub fn num(n: f64) -> Self {
        Self::plain(Val::Num(n))
    }
}

/// The worlds where a value is true. Undefined isn't true.
pub fn truth(v: &ValueCell) -> Bool {
    v.0.0.holds(|v| *v == Val::Bool(true))
}

/// What a choice lands on, in each world: a node, or undefined where there's nothing to land on.
/// Writing one in settles it.
#[cell]
pub struct Chosen(pub Worldly<Val>);

impl Chosen {
    /// Settled on this node.
    pub fn on(e: Entity) -> Self {
        Chosen(Worldly::plain(Val::Ref(e)))
    }

    /// The worlds where it lands on this node.
    pub fn lands_on(&self, e: Entity) -> Bool {
        self.0.0.holds(|v| *v == Val::Ref(e))
    }

    /// The one node it's settled on, if it's the same in every world.
    pub fn settled(&self) -> Option<Entity> {
        match self.0.0.cases().as_slice() {
            [(_, Val::Ref(e))] => Some(*e),
            _ => None,
        }
    }
}

/// What a choice lands on once any option that stands for another choice's result has been followed to
/// it. This is what a formula reads as `chosen`, so `final.chosen = car.chosen` is about doors even when
/// the options of `final` are "stick" and "switch".
#[cell]
pub struct Lands(pub Worldly<Val>);

fn lands(cx: &Cx) -> Cell<Lands> {
    let Some(chosen) = cx.own::<Chosen>()? else { return Ok(None) };
    let mut out = chosen.0.0.clone();
    for (when, v) in chosen.0.0.cases() {
        let Val::Ref(option) = v else { continue };
        // Whatever the choice it stands for lands on. Reading that here makes standing for yourself a loop.
        if let Some(target) = cx.outgoing_of(*option, STANDS_FOR).into_iter().next() {
            let there = cx.get::<Lands>(target)?.map_or_else(|| pure(Val::Undefined), |l| l.0.0);
            out = Cond::when(&when, &there, &out);
        }
    }
    Ok(Some(Lands(Worldly(out))))
}

/// A plain list of what a choice can land on, such as a puzzle's colours. Each is a node nobody wrote,
/// whose name and value are the option, and which every choice with that option shares.
#[cell("options")]
pub struct Options(pub Vec<String>);

/// Where a choice goes on a board, if the puzzle has one.
#[cell("at")]
pub struct At {
    pub row: u32,
    pub col: u32,
}

/// On a label edge: the name a formula on its source uses for its target. Several edges with one name
/// make a group.
#[cell]
pub struct Called(pub String);

/// A function of your own, written as a formula over its parameters.
#[cell("macro")]
pub struct Macro {
    pub params: Vec<String>,
    pub body: String,
}

/// A macro, with the macros it uses found. It's a cell so a macro calling itself is a loop in these
/// cells, which needs nothing special, and so a caller only ever reads one hop.
#[cell("macro_body")]
pub struct MacroBody {
    pub params: Vec<String>,
    pub body: Expr,
    pub uses: BTreeMap<String, MacroBody>,
}

/// What type a node's value is. A type doesn't depend on a world, or on any value that isn't typed
/// in, so this reruns far less often than the value does, and stops there when it comes out the same.
#[cell("value_type")]
pub struct ValueType(pub Type);

/// What the cells of compiled systems hold, for a formula reading them. Anything else is checked as
/// it runs.
pub(crate) fn component_type(cell: &str) -> Type {
    match cell {
        "need" | "actionable" | "blocked" | "done" | "dropped" => Type::Bool,
        "chosen" => Type::Ref,
        "estimate" | "start" | "finish" | "latest_start" | "latest_finish" | "slack" | "plan_end" | "weight" => Type::Number,
        "name" | "body" => Type::Text,
        _ => Type::Any,
    }
}

/// How far along its table `prev` and `next` are from a row.
fn along(name: &str) -> Option<i64> {
    match name {
        "prev" => Some(-1),
        "next" => Some(1),
        _ => None,
    }
}

/// How far along a path starting `prev.prev…` goes, and what it reads there.
fn steps_along(path: &[String]) -> (i64, &[String]) {
    let n = path.iter().take_while(|p| along(p).is_some()).count();
    (path[..n].iter().filter_map(|p| along(p)).sum(), &path[n..])
}

fn failed<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::Failed(message.into()))
}

// ---------- expanding: names to nodes, and functions to what they're defined as ----------

/// A part of an expanded formula, as an index into `Closed`.
type Id = usize;

/// A part of a formula with nothing left to look up. Its parts come earlier in the list, so a
/// formula can be as long as its groups are, and going through it needs no recursion.
#[derive(Debug)]
enum Part {
    Lit(Val),
    Read { node: Entity, component: String, fields: Vec<String> },
    Field(Id, Vec<String>),
    If(Id, Id, Id),
    Bin(BinOp, Id, Id),
    Not(Id),
    Neg(Id),
    Defined(Id),
    Has(Entity, String),
    Chose(Entity, Entity),
    Round(Id, Id),
    Floor(Id),
    Ceil(Id),
}

impl Part {
    fn renumbered(self, to: &[Id]) -> Part {
        match self {
            Part::Field(a, f) => Part::Field(to[a], f),
            Part::If(a, b, c) => Part::If(to[a], to[b], to[c]),
            Part::Bin(op, a, b) => Part::Bin(op, to[a], to[b]),
            Part::Not(a) => Part::Not(to[a]),
            Part::Neg(a) => Part::Neg(to[a]),
            Part::Defined(a) => Part::Defined(to[a]),
            Part::Round(a, b) => Part::Round(to[a], to[b]),
            Part::Floor(a) => Part::Floor(to[a]),
            Part::Ceil(a) => Part::Ceil(to[a]),
            other => other,
        }
    }

    fn parts(&self) -> Vec<Id> {
        match self {
            Part::Lit(_) | Part::Read { .. } | Part::Has(..) | Part::Chose(..) => vec![],
            Part::Field(a, _) | Part::Not(a) | Part::Neg(a) | Part::Defined(a) | Part::Floor(a) | Part::Ceil(a) => vec![*a],
            Part::Bin(_, a, b) | Part::Round(a, b) => vec![*a, *b],
            Part::If(a, b, c) => vec![*a, *b, *c],
        }
    }
}

/// An expanded formula: only the parts it really uses, in order, so what each uses comes first. The
/// same part can be used many times, which is how an argument a macro uses twice is worked out once.
struct Closed(Vec<Part>);

#[derive(Clone)]
enum Binding {
    Node(Entity),
    Group(Vec<Entity>),
    Value(Id),
}

#[derive(Clone, Default)]
struct Scope<'m> {
    vars: HashMap<String, Binding>,
    /// For a node being gone over in an edge's direction, the edge it was reached by: `o.via`.
    via: HashMap<String, Entity>,
    /// In a macro: what it can call. It can't see anything else.
    macro_uses: Option<&'m BTreeMap<String, MacroBody>>,
}

impl Scope<'_> {
    fn is_macro(&self) -> bool {
        self.macro_uses.is_some()
    }
}

struct Expander<'c, 'a> {
    cx: &'c Cx<'a>,
    labels: BTreeMap<String, Vec<Entity>>,
    parts: RefCell<Vec<Part>>,
    steps: StdCell<usize>,
}

impl<'c, 'a> Expander<'c, 'a> {
    fn new(cx: &'c Cx<'a>) -> Result<Self, Error> {
        let mut labels: BTreeMap<String, Vec<Entity>> = BTreeMap::new();
        // Names are what the node the formula's written on has labelled, which is a kind's for a
        // formula taken from one.
        for (to, edge) in cx.outgoing_from_origin(LABEL) {
            if let Some(Called(name)) = cx.get::<Called>(edge)? {
                labels.entry(name).or_default().push(to);
            }
        }
        Ok(Expander { cx, labels, parts: RefCell::new(Vec::new()), steps: StdCell::new(0) })
    }

    fn add(&self, p: Part) -> Id {
        let mut parts = self.parts.borrow_mut();
        parts.push(p);
        parts.len() - 1
    }

    fn lit(&self, v: Val) -> Id {
        self.add(Part::Lit(v))
    }

    fn read(&self, node: Entity, path: &[String]) -> Id {
        // A row's key isn't a cell, but it's read the same way, as `q.row` or `prev.row`.
        if let [only] = path
            && only == "row"
        {
            return self.lit(match self.cx.row_key_of(node) {
                Some(Key::Num(n)) => Val::Num(n as f64),
                Some(Key::Text(t)) => Val::Text(t),
                None => Val::Undefined,
            });
        }
        self.add(match path {
            [] => Part::Read { node, component: ValueCell::NAME.to_string(), fields: vec![] },
            [component, fields @ ..] => Part::Read { node, component: component.clone(), fields: fields.to_vec() },
        })
    }

    /// The whole formula, with anything nothing ended up using left out.
    fn expand_all(self, e: &Expr) -> Result<Closed, Error> {
        let root = self.expand(e, &Scope::default())?;
        let parts = self.parts.into_inner();
        let mut used = vec![false; parts.len()];
        used[root] = true;
        for i in (0..parts.len()).rev() {
            if used[i] {
                parts[i].parts().into_iter().for_each(|p| used[p] = true);
            }
        }
        // Parts move down the list, so what refers to them has to follow.
        let mut moved = vec![0; parts.len()];
        let mut out = Vec::with_capacity(parts.len());
        for (i, part) in parts.into_iter().enumerate() {
            if used[i] {
                moved[i] = out.len();
                out.push(part.renumbered(&moved));
            }
        }
        Ok(Closed(out))
    }

    fn group(&self, path: &[String], scope: &Scope) -> Result<Vec<Entity>, Error> {
        Ok(self.members(path, scope)?.into_iter().map(|(n, _)| n).collect())
    }

    /// A group's nodes, each with the edge it was reached by, if it was reached along one.
    fn members(&self, path: &[String], scope: &Scope) -> Result<Vec<(Entity, Option<Entity>)>, Error> {
        let (root, rest) = path.split_first().expect("a path has a first name");
        if let Some(b) = scope.vars.get(root) {
            return match b {
                Binding::Node(n) => Ok(vec![(*n, None)]),
                Binding::Group(g) => Ok(g.iter().map(|n| (*n, None)).collect()),
                Binding::Value(_) => failed(format!("{root} isn't a group of nodes")),
            };
        }
        if matches!(root.as_str(), "in" | "out" | "with") {
            if scope.is_macro() {
                return failed(format!("a macro can only use its parameters, not {root}.…"));
            }
            let Some(component) = rest.first() else { return failed(format!("{root} needs to say which component, such as {root}.part_of")) };
            let along = |edges: Vec<(Entity, Entity)>| edges.into_iter().map(|(n, e)| (n, Some(e))).collect();
            return Ok(match root.as_str() {
                "in" => along(self.cx.incoming(component)),
                "out" => along(self.cx.outgoing(component)),
                _ => self.cx.scan(component).into_iter().map(|n| (n, None)).collect(),
            });
        }
        if scope.is_macro() {
            return failed(format!("a macro can only use its parameters, not {root}"));
        }
        let nodes = if root == "rows" {
            self.cx.rows()
        } else if rest == ["rows"] && !self.labels.contains_key(root) {
            // Another table's rows, by its name, as `Arrived.rows`.
            self.cx.rows_of(root)
        } else {
            // No edges yet is fine for a group: a total with no lines is 0.
            self.labels.get(root).cloned().unwrap_or_default()
        };
        Ok(nodes.into_iter().map(|n| (n, None)).collect())
    }

    /// On an edge, its source as `from` and its target as `to`.
    fn end(&self, root: &str) -> Option<Entity> {
        let (from, to) = self.cx.ends()?;
        match root {
            "from" => Some(from),
            "to" => Some(to),
            _ => None,
        }
    }

    fn node_arg(&self, e: &Expr, scope: &Scope) -> Result<Entity, Error> {
        match self.binding(e, scope)? {
            Binding::Node(n) => Ok(n),
            _ => failed("chose needs one decision and one of its options"),
        }
    }

    /// What an argument to a function is: a node or group if it reads as one, or a value.
    fn binding(&self, e: &Expr, scope: &Scope) -> Result<Binding, Error> {
        if let Expr::Path(path) = e {
            let root = path[0].as_str();
            if let Some(b) = scope.vars.get(root) {
                if path.len() == 1 {
                    return Ok(b.clone());
                }
            } else if root == "self" && path.len() == 1 && !scope.is_macro() {
                return Ok(Binding::Node(self.cx.id()));
            } else if let Some(end) = self.end(root).filter(|_| path.len() == 1 && !scope.is_macro()) {
                return Ok(Binding::Node(end));
            } else if along(root).is_some() && !scope.is_macro() {
                let (by, rest) = steps_along(path);
                if rest.is_empty()
                    && let Some(n) = self.cx.row_along(by)
                {
                    return Ok(Binding::Node(n));
                }
            } else if matches!(root, "in" | "out" | "with" | "rows") || (path.len() == 2 && path[1] == "rows") {
                return Ok(Binding::Group(self.group(path, scope)?));
            } else if path.len() == 1 && !scope.is_macro() && root != "self" {
                let nodes = self.group(path, scope)?;
                return Ok(match nodes.as_slice() {
                    [one] => Binding::Node(*one),
                    _ => Binding::Group(nodes),
                });
            }
        }
        Ok(Binding::Value(self.expand(e, scope)?))
    }

    fn expand(&self, e: &Expr, scope: &Scope) -> Result<Id, Error> {
        self.steps.set(self.steps.get() + 1);
        if self.steps.get() > MOST_STEPS {
            return failed("this formula expands to something too big to work out");
        }
        // Macros calling macros can go deep, though parts of a formula can't by themselves.
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || match e {
            Expr::Lit(v) => Ok(self.lit(v.clone())),
            Expr::Not(a) => {
                let a = self.expand(a, scope)?;
                Ok(self.add(Part::Not(a)))
            }
            Expr::Neg(a) => {
                let a = self.expand(a, scope)?;
                Ok(self.add(Part::Neg(a)))
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.expand(a, scope)?, self.expand(b, scope)?);
                Ok(self.add(Part::Bin(*op, a, b)))
            }
            Expr::Path(path) => self.path(path, scope),
            Expr::Row { table, key, path } => {
                if scope.is_macro() {
                    return failed(format!("a macro can only use its parameters, not {table}[…]"));
                }
                let key = self.key(key, scope)?;
                match self.cx.row_at(table, &key) {
                    Some(n) => Ok(self.read(n, path)),
                    None => failed(format!("there's no row {table}[{key}]")),
                }
            }
            Expr::Fold { func, var, group, body, start } => self.fold(func, var, group, body, start.as_deref(), scope),
            Expr::Call(name, args) => self.call(name, args, scope),
        })
    }

    fn path(&self, path: &[String], scope: &Scope) -> Result<Id, Error> {
        let (root, rest) = path.split_first().expect("a path has a first name");
        match scope.vars.get(root) {
            // The edge a node was reached along, as `o.via.weight`.
            Some(Binding::Node(_)) if rest.first().is_some_and(|r| r == "via") => {
                return match scope.via.get(root) {
                    Some(edge) if rest.len() > 1 => Ok(self.read(*edge, &rest[1..])),
                    Some(_) => failed(format!("{root}.via is an edge, so it needs to say which cell, such as {root}.via.weight")),
                    None => failed(format!("{root} wasn't reached along an edge, so it has no via")),
                };
            }
            Some(Binding::Node(n)) => return Ok(self.read(*n, rest)),
            Some(Binding::Value(c)) if rest.is_empty() => return Ok(*c),
            Some(Binding::Value(c)) => return Ok(self.add(Part::Field(*c, rest.to_vec()))),
            Some(Binding::Group(g)) => return failed(format!("{root} is a group of {} nodes, so it needs something like sum({root})", g.len())),
            None => {}
        }
        if scope.is_macro() {
            return failed(format!("a macro can only use its parameters, not {root}"));
        }
        match root.as_str() {
            "self" if rest.is_empty() => failed("self needs to say which cell, such as self.need"),
            "self" => Ok(self.read(self.cx.id(), rest)),
            "whole" if rest.is_empty() => failed("whole needs to say which cell, such as whole.plan_end"),
            "whole" => Ok(self.read(Entity::whole(), rest)),
            "from" | "to" if self.end(root).is_some() && rest.is_empty() => failed(format!("{root} needs to say which cell, such as {root}.need")),
            "from" | "to" if self.end(root).is_some() => Ok(self.read(self.end(root).expect("checked above"), rest)),
            // Before the first row, or with a gap, there's nothing there.
            "prev" | "next" => {
                let (by, rest) = steps_along(path);
                Ok(match self.cx.row_along(by) {
                    Some(n) => self.read(n, rest),
                    None => self.lit(Val::Undefined),
                })
            }
            "row" if !rest.is_empty() => failed("row is which row this is, so it has no cells: prev.cost reads the row before"),
            "row" => Ok(self.lit(match self.cx.row_key() {
                Some(Key::Num(n)) => Val::Num(n as f64),
                Some(Key::Text(t)) => Val::Text(t),
                None => Val::Undefined,
            })),
            "in" | "out" | "with" | "rows" => failed(format!("{}.… is a group of nodes, so it needs something like sum({}.…)", root, root)),
            _ => match self.labels.get(root).map(Vec::as_slice) {
                None if rest.first().is_some_and(|r| r == "rows") => failed(format!("{root}.rows is a group of rows, so it needs something like count({root}.rows)")),
                None | Some([]) => failed(format!("no input called {root}")),
                Some([one]) => Ok(self.read(*one, rest)),
                Some(many) => failed(format!("{root} is {} nodes, so it needs something like sum({root})", many.len())),
            },
        }
    }

    /// Which row `Year[…]` means. It has to be known without looking at any world, so it's a number, a
    /// name, or worked out from `row` with plain arithmetic, as `Year[row - 1]` is.
    fn key(&self, e: &Expr, scope: &Scope) -> Result<Key, Error> {
        let number = |e: &Expr| -> Result<i64, Error> {
            match self.key(e, scope)? {
                Key::Num(n) => Ok(n),
                Key::Text(t) => failed(format!("{t} isn't a number, so it can't be added to")),
            }
        };
        Ok(match e {
            Expr::Lit(Val::Num(n)) if n.fract() == 0.0 && n.abs() < 1e15 => Key::Num(*n as i64),
            Expr::Lit(Val::Text(t)) => Key::Text(t.clone()),
            Expr::Path(p) if p.len() == 1 && p[0] == "row" => match self.cx.row_key() {
                Some(k) => k,
                None => return failed("row is only something on a row of a table"),
            },
            // The key of a row being gone over, as `Left[p.row]` in `count(p in Arrived.rows: …)`.
            Expr::Path(p) if p.len() == 2 && p[1] == "row" && matches!(scope.vars.get(&p[0]), Some(Binding::Node(_))) => {
                let Some(Binding::Node(n)) = scope.vars.get(&p[0]) else { unreachable!("matched above") };
                match self.cx.row_key_of(*n) {
                    Some(k) => k,
                    None => return failed(format!("{} isn't a row of a table, so it has no key", p[0])),
                }
            }
            Expr::Path(p) if p.len() == 1 => Key::Text(p[0].clone()),
            Expr::Neg(a) => Key::Num(-number(a)?),
            Expr::Bin(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), a, b) => {
                let (a, b) = (number(a)?, number(b)?);
                Key::Num(match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    _ => a * b,
                })
            }
            _ => return failed("a row's key is a number, a name, or worked out from row, such as Year[3], Car[Hatchback], Year[row - 1] or Left[p.row]"),
        })
    }

    /// No two of these the same: every pair differs.
    fn distinct(&self, each: &[Id]) -> Id {
        let mut acc = self.lit(Val::Bool(true));
        for (i, a) in each.iter().enumerate() {
            for b in &each[i + 1..] {
                let differ = self.add(Part::Bin(BinOp::Ne, *a, *b));
                acc = self.add(Part::Bin(BinOp::And, acc, differ));
            }
        }
        acc
    }

    fn fold(&self, func: &str, var: &str, group: &[String], body: &Expr, start: Option<&Expr>, scope: &Scope) -> Result<Id, Error> {
        let members = self.members(group, scope)?;
        let nodes: Vec<Entity> = members.iter().map(|(n, _)| *n).collect();
        let via: HashMap<Entity, Entity> = members.iter().filter_map(|(n, e)| Some((*n, (*e)?))).collect();
        let copy = |n: Entity| {
            let mut inner = scope.clone();
            inner.vars.insert(var.to_string(), Binding::Node(n));
            match via.get(&n) {
                Some(edge) => inner.via.insert(var.to_string(), *edge),
                None => inner.via.remove(var),
            };
            self.expand(body, &inner)
        };
        let from = |unit: Val| start.map_or_else(|| Ok(self.lit(unit)), |s| self.expand(s, scope));
        let chain = |op: BinOp, unit: Val| -> Result<Id, Error> {
            let mut acc = from(unit)?;
            for n in &nodes {
                let next = copy(*n)?;
                acc = self.add(Part::Bin(op, acc, next));
            }
            Ok(acc)
        };
        match func {
            "sum" => chain(BinOp::Add, Val::Num(0.0)),
            "product" => chain(BinOp::Mul, Val::Num(1.0)),
            "all" => chain(BinOp::And, Val::Bool(true)),
            "any" => chain(BinOp::Or, Val::Bool(false)),
            "max" => chain(BinOp::Max, Val::Undefined),
            "min" => chain(BinOp::Min, Val::Undefined),
            // No two the same: every pair differs.
            "distinct" => {
                let each = nodes.iter().map(|n| copy(*n)).collect::<Result<Vec<_>, _>>()?;
                Ok(self.distinct(&each))
            }
            "count" => {
                let (one, zero) = (self.lit(Val::Num(1.0)), self.lit(Val::Num(0.0)));
                let mut acc = zero;
                for n in &nodes {
                    let this = copy(*n)?;
                    let counted = self.add(Part::If(this, one, zero));
                    acc = self.add(Part::Bin(BinOp::Add, acc, counted));
                }
                Ok(acc)
            }
            _ => unreachable!("the parser only makes folds of these"),
        }
    }

    fn call(&self, name: &str, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        if CORE.contains(&name) {
            return self.core(name, args, scope);
        }
        if let Some(body) = scope.macro_uses.and_then(|u| u.get(name)) {
            return self.call_macro(name, &body.params, &body.body, &body.uses, args, scope);
        }
        if let Some(m) = expr::library(name) {
            // The library isn't a macro of the kind a workspace writes: it can follow edges and scan, as
            // `effort()` does, where a workspace's macro only has its parameters.
            let vars = self.bind(name, &m.params, args, scope)?;
            return self.expand(&m.body, &Scope { vars, ..Scope::default() });
        }
        if scope.is_macro() {
            return failed(format!("no function called {name} that this macro has a link to"));
        }
        match self.labels.get(name).map(Vec::as_slice) {
            None | Some([]) => failed(format!("no function called {name}")),
            Some([node]) => match self.cx.get::<MacroBody>(*node)? {
                Some(m) => self.call_macro(name, &m.params, &m.body, &m.uses, args, scope),
                None => failed(format!("{name} isn't a macro")),
            },
            Some(_) => failed(format!("{name} is several nodes, so it can't be called")),
        }
    }

    fn bind(&self, name: &str, params: &[String], args: &[Expr], scope: &Scope) -> Result<HashMap<String, Binding>, Error> {
        if params.len() != args.len() {
            return failed(format!("{name} takes {} and was given {}", params.len(), args.len()));
        }
        params.iter().zip(args).map(|(p, a)| Ok((p.clone(), self.binding(a, scope)?))).collect()
    }

    fn call_macro(&self, name: &str, params: &[String], body: &Expr, uses: &BTreeMap<String, MacroBody>, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        let vars = self.bind(name, params, args, scope)?;
        self.expand(body, &Scope { vars, macro_uses: Some(uses), ..Scope::default() })
    }

    fn core(&self, name: &str, args: &[Expr], scope: &Scope) -> Result<Id, Error> {
        let wants = |n: usize| if args.len() == n { Ok(()) } else { failed(format!("{name} takes {n} and was given {}", args.len())) };
        match name {
            "if" => {
                wants(3)?;
                let (c, a, b) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?, self.expand(&args[2], scope)?);
                Ok(self.add(Part::If(c, a, b)))
            }
            "defined" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Defined(a)))
            }
            "has" => {
                wants(1)?;
                let Expr::Path(path) = &args[0] else { return failed("has needs a node and a cell, such as has(self.estimate)") };
                let [root, component] = path.as_slice() else { return failed("has needs a node and a cell, such as has(self.estimate)") };
                match self.binding(&Expr::Path(vec![root.clone()]), scope)? {
                    Binding::Node(n) => Ok(self.add(Part::Has(n, component.clone()))),
                    _ => failed(format!("has needs one node, and {root} isn't one")),
                }
            }
            "max" | "min" => {
                wants(2)?;
                let (a, b) = (self.expand(&args[0], scope)?, self.expand(&args[1], scope)?);
                Ok(self.add(Part::Bin(if name == "max" { BinOp::Max } else { BinOp::Min }, a, b)))
            }
            "distinct" => {
                let each = args.iter().map(|a| self.expand(a, scope)).collect::<Result<Vec<_>, _>>()?;
                Ok(self.distinct(&each))
            }
            "chose" => {
                wants(2)?;
                let (d, o) = (self.node_arg(&args[0], scope)?, self.node_arg(&args[1], scope)?);
                Ok(self.add(Part::Chose(d, o)))
            }
            "round" => match args {
                [x] => {
                    let (x, zero) = (self.expand(x, scope)?, self.lit(Val::Num(0.0)));
                    Ok(self.add(Part::Round(x, zero)))
                }
                [x, places] => {
                    let (x, places) = (self.expand(x, scope)?, self.expand(places, scope)?);
                    Ok(self.add(Part::Round(x, places)))
                }
                _ => failed(format!("round takes 1 or 2 and was given {}", args.len())),
            },
            "floor" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Floor(a)))
            }
            "ceil" => {
                wants(1)?;
                let a = self.expand(&args[0], scope)?;
                Ok(self.add(Part::Ceil(a)))
            }
            _ => unreachable!("every name in CORE is here"),
        }
    }
}

// ---------- checking: before any world is looked at ----------

fn drill(mut t: Type, fields: &[String]) -> Type {
    for f in fields {
        t = match t {
            Type::Record(of) => of.get(f).cloned().unwrap_or(Type::Undefined),
            _ => Type::Any,
        };
    }
    t
}

fn numberish(t: &Type) -> bool {
    matches!(t, Type::Number | Type::Any | Type::Undefined)
}

fn boolish(t: &Type) -> bool {
    matches!(t, Type::Bool | Type::Any | Type::Undefined)
}

/// The type of a formula, without looking at any world.
fn check(cx: &Cx, closed: &Closed) -> Result<Type, Error> {
    let mut types: Vec<Type> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let t = type_of(cx, part, &types)?;
        types.push(t);
    }
    Ok(types.pop().expect("a formula has something in it"))
}

fn type_of(cx: &Cx, part: &Part, types: &[Type]) -> Result<Type, Error> {
    match part {
        Part::Lit(v) => Ok(Type::of(v)),
        Part::Read { node, component, fields } => {
            let t = if component == ValueCell::NAME { cx.get::<ValueType>(*node)?.map_or(Type::Undefined, |t| t.0) } else { component_type(component) };
            Ok(drill(t, fields))
        }
        Part::Field(of, fields) => Ok(drill(types[*of].clone(), fields)),
        Part::If(cond, a, b) => {
            if !boolish(&types[*cond]) {
                return failed(format!("if needs true or false to decide on, not {}", types[*cond].word()));
            }
            let (a, b) = (&types[*a], &types[*b]);
            a.join(b).ok_or_else(|| Error::Failed(format!("the two sides of if have to be the same type, not {} and {}", a.word(), b.word())))
        }
        Part::Not(a) => {
            let a = &types[*a];
            if boolish(a) { Ok(Type::Bool) } else { failed(format!("! needs true or false, not {}", a.word())) }
        }
        Part::Neg(a) => {
            let a = &types[*a];
            if numberish(a) { Ok(Type::Number) } else { failed(format!("- needs a number, not {}", a.word())) }
        }
        Part::Defined(_) | Part::Has(..) | Part::Chose(..) => Ok(Type::Bool),
        Part::Round(x, places) => {
            let (x, places) = (&types[*x], &types[*places]);
            if numberish(x) && numberish(places) { Ok(Type::Number) } else { failed(format!("round needs numbers, not {}", x.word())) }
        }
        Part::Floor(x) | Part::Ceil(x) => {
            let x = &types[*x];
            if numberish(x) { Ok(Type::Number) } else { failed(format!("rounding needs a number, not {}", x.word())) }
        }
        Part::Bin(op, a, b) => {
            let (ta, tb) = (&types[*a], &types[*b]);
            let sym = op.symbol();
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Max | BinOp::Min => {
                    if numberish(ta) && numberish(tb) { Ok(Type::Number) } else { failed(format!("{sym} needs numbers, not {} and {}", ta.word(), tb.word())) }
                }
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    if numberish(ta) && numberish(tb) { Ok(Type::Bool) } else { failed(format!("{sym} needs numbers, not {} and {}", ta.word(), tb.word())) }
                }
                BinOp::And | BinOp::Or => {
                    if boolish(ta) && boolish(tb) { Ok(Type::Bool) } else { failed(format!("{sym} needs true or false, not {} and {}", ta.word(), tb.word())) }
                }
                BinOp::Eq | BinOp::Ne => {
                    if ta.join(tb).is_none() {
                        return failed(format!("{sym} can't compare {} with {}", ta.word(), tb.word()));
                    }
                    Ok(Type::Bool)
                }
            }
        }
    }
}

// ---------- evaluating: in every world at once ----------

/// A value across worlds. True-or-false stays as diagrams, since listing them as cases would throw
/// away what makes them cheap to combine.
#[derive(Clone)]
enum Ev {
    /// The worlds where it's true, and the worlds where it's undefined. Elsewhere it's false.
    Truth { yes: Bool, undefined: Bool },
    Data(Cond<Val>),
}

fn pure(v: Val) -> Cond<Val> {
    Space::global().pure(v)
}

fn no_truth(found: &Val) -> Error {
    Error::Failed(format!("expected true or false, not {}", Type::of(found).word()))
}

impl Ev {
    fn truth(&self) -> Result<(Bool, Bool), Error> {
        match self {
            Ev::Truth { yes, undefined } => Ok((yes.clone(), undefined.clone())),
            Ev::Data(c) => {
                let cases = c.cases();
                if let Some((_, bad)) = cases.iter().find(|(_, v)| !matches!(v, Val::Bool(_) | Val::Undefined)) {
                    return Err(no_truth(bad));
                }
                Ok((c.holds(|v| *v == Val::Bool(true)), c.holds(|v| *v == Val::Undefined)))
            }
        }
    }

    fn data(&self) -> Cond<Val> {
        match self {
            Ev::Data(c) => c.clone(),
            Ev::Truth { yes, undefined } => Cond::when(yes, &pure(Val::Bool(true)), &Cond::when(undefined, &pure(Val::Undefined), &pure(Val::Bool(false)))),
        }
    }
}

fn zip(a: &Cond<Val>, b: &Cond<Val>, f: impl Fn(&Val, &Val) -> Val) -> Result<Cond<Val>, Error> {
    if a.len().saturating_mul(b.len()) > MOST_PAIRS {
        return too_many(a, b);
    }
    let out = a.zip(b, f);
    if out.len() > MOST_CASES {
        return too_many(a, b);
    }
    Ok(out)
}

fn too_many<T>(a: &Cond<Val>, b: &Cond<Val>) -> Result<T, Error> {
    let mut choices: Vec<String> = a.vars().into_iter().chain(b.vars()).map(|v| v.name()).collect();
    choices.sort();
    choices.dedup();
    let shown: Vec<String> = choices.iter().take(8).cloned().collect();
    let more = if choices.len() > 8 { format!(", and {} more", choices.len() - 8) } else { String::new() };
    failed(format!("this has more than {MOST_CASES} different values across worlds, depending on {}{more}", shown.join(", ")))
}

/// `f` on two numbers, undefined if either is, and `wrong` set if anything else turns up, which only
/// a value of unknown type can get as far as here.
fn numbers(a: &Val, b: &Val, f: impl Fn(f64, f64) -> Val, wrong: &StdCell<bool>) -> Val {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => f(*x, *y),
        (Val::Undefined, Val::Num(_) | Val::Undefined) | (Val::Num(_), Val::Undefined) => Val::Undefined,
        _ => {
            wrong.set(true);
            Val::Undefined
        }
    }
}

/// A formula's value across worlds.
fn evaluate(cx: &Cx, closed: &Closed) -> Result<Ev, Error> {
    let mut values: Vec<Ev> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let v = value_of(cx, part, &values)?;
        values.push(v);
    }
    Ok(values.pop().expect("a formula has something in it"))
}

fn read_cell(cx: &Cx, node: Entity, component: &str, fields: &[String]) -> Result<Ev, Error> {
    let whole = match component {
        ValueCell::NAME => match cx.get::<ValueCell>(node)? {
            Some(v) => Ev::Data(v.0.0),
            None => Ev::Data(pure(Val::Undefined)),
        },
        // Through any option that stands for another choice's result, to what that lands on.
        Chosen::NAME => match cx.get::<Lands>(node)? {
            Some(c) => Ev::Data(c.0.0),
            None => Ev::Data(pure(Val::Undefined)),
        },
        // The cells formulas read most, without going through JSON.
        Need::NAME => match cx.get::<Need>(node)? {
            Some(n) => Ev::Truth { yes: n.0, undefined: Space::global().never() },
            None => Ev::Data(pure(Val::Undefined)),
        },
        Done::NAME => flag_cell(cx.get::<Done>(node)?.map(|c| c.0)),
        Dropped::NAME => flag_cell(cx.get::<Dropped>(node)?.map(|c| c.0)),
        crate::plan::Estimate::NAME => number_cell(cx.get::<crate::plan::Estimate>(node)?.map(|c| c.0)),
        Start::NAME => number_cell(cx.get::<Start>(node)?.map(|c| c.0)),
        Finish::NAME => number_cell(cx.get::<Finish>(node)?.map(|c| c.0)),
        LatestStart::NAME => number_cell(cx.get::<LatestStart>(node)?.map(|c| c.0)),
        LatestFinish::NAME => number_cell(cx.get::<LatestFinish>(node)?.map(|c| c.0)),
        PlanEnd::NAME => number_cell(cx.get::<PlanEnd>(node)?.map(|c| c.0)),
        _ => match cx.get_value(node, component)? {
            None => Ev::Data(pure(Val::Undefined)),
            Some(v) => from_json(v).map_err(|detail| Error::Malformed { cell: (node, component.to_string()), detail })?,
        },
    };
    if fields.is_empty() {
        return Ok(whole);
    }
    Ok(Ev::Data(field_of(cx, &whole.data(), fields)?))
}

fn flag_cell(c: Option<Worldly<Option<bool>>>) -> Ev {
    match c {
        Some(c) => Ev::Truth { yes: c.0.holds(|v| *v == Some(true)), undefined: c.0.holds(Option::is_none) },
        None => Ev::Data(pure(Val::Undefined)),
    }
}

fn number_cell(c: Option<Worldly<Option<f64>>>) -> Ev {
    match c {
        Some(c) => Ev::Data(c.0.map(|d| d.map_or(Val::Undefined, Val::num))),
        None => Ev::Data(pure(Val::Undefined)),
    }
}

fn value_of(cx: &Cx, part: &Part, values: &[Ev]) -> Result<Ev, Error> {
    let space = Space::global();
    Ok(match part {
        Part::Lit(v) => Ev::Data(pure(v.clone())),
        Part::Read { node, component, fields } => read_cell(cx, *node, component, fields)?,
        Part::Field(of, fields) => Ev::Data(field_of(cx, &values[*of].data(), fields)?),
        Part::Defined(a) => {
            let undefined = match &values[*a] {
                Ev::Truth { undefined, .. } => undefined.clone(),
                Ev::Data(d) => d.holds(|v| *v == Val::Undefined),
            };
            Ev::Truth { yes: !&undefined, undefined: space.never() }
        }
        // A component, if there's one by that name, and otherwise whether there's anything in that cell.
        Part::Has(node, name) => {
            let has = if cx.registry().component_def(name).is_some() { cx.has(*node, name) } else { cx.get_value(*node, name)?.is_some() };
            Ev::Truth { yes: space.truth(has), undefined: space.never() }
        }
        Part::Chose(choice, option) => {
            let Some(chosen) = cx.get::<Chosen>(*choice)? else { return failed("chose needs a choice") };
            Ev::Truth { yes: chosen.lands_on(*option), undefined: space.never() }
        }
        Part::Not(a) => {
            let (yes, undefined) = values[*a].truth()?;
            Ev::Truth { yes: &!&yes & &!&undefined, undefined }
        }
        Part::Neg(a) => {
            let wrong = StdCell::new(false);
            let out = values[*a].data().map(|v| numbers(&Val::Num(0.0), v, |z, x| Val::num(z - x), &wrong));
            if wrong.get() {
                return failed("- needs a number");
            }
            Ev::Data(out)
        }
        Part::If(cond, a, b) => {
            let (yes, undefined) = values[*cond].truth()?;
            let no = &!&yes & &!&undefined;
            if let (Ev::Truth { yes: ay, undefined: au }, Ev::Truth { yes: by, undefined: bu }) = (&values[*a], &values[*b]) {
                Ev::Truth { yes: &(&yes & ay) | &(&no & by), undefined: &undefined | &(&(&yes & au) | &(&no & bu)) }
            } else {
                Ev::Data(Cond::when(&undefined, &pure(Val::Undefined), &Cond::when(&yes, &values[*a].data(), &values[*b].data())))
            }
        }
        Part::Round(x, places) => {
            let wrong = StdCell::new(false);
            let out = zip(&values[*x].data(), &values[*places].data(), |x, p| {
                numbers(x, p, |x, p| {
                    let scale = 10f64.powf(p);
                    Val::num((x * scale).round() / scale)
                }, &wrong)
            })?;
            if wrong.get() {
                return failed("round needs numbers");
            }
            Ev::Data(out)
        }
        Part::Floor(x) | Part::Ceil(x) => {
            let up = matches!(part, Part::Ceil(_));
            let wrong = StdCell::new(false);
            let out = values[*x].data().map(|x| numbers(x, &Val::Num(0.0), |x, _| Val::num(if up { x.ceil() } else { x.floor() }), &wrong));
            if wrong.get() {
                return failed("rounding needs a number");
            }
            Ev::Data(out)
        }
        Part::Bin(op, a, b) => binary(*op, &values[*a], &values[*b])?,
    })
}

fn binary(op: BinOp, a: &Ev, b: &Ev) -> Result<Ev, Error> {
    let sym = op.symbol();
    match op {
        BinOp::And | BinOp::Or => {
            let ((ay, au), (by, bu)) = (a.truth()?, b.truth()?);
            // Nearly always, nothing's undefined, and a puzzle does this for every constraint.
            if au.is_never() && bu.is_never() {
                return Ok(Ev::Truth { yes: if op == BinOp::And { &ay & &by } else { &ay | &by }, undefined: au });
            }
            let (an, bn) = (&!&ay & &!&au, &!&by & &!&bu);
            // Three-valued, so false && undefined is false, and true || undefined is true.
            let (yes, no) = if op == BinOp::And { (&ay & &by, &an | &bn) } else { (&ay | &by, &an & &bn) };
            let undefined = &!&yes & &!&no;
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Eq | BinOp::Ne if matches!((a, b), (Ev::Truth { .. }, _) | (_, Ev::Truth { .. })) => {
            let ((ay, au), (by, bu)) = (a.truth()?, b.truth()?);
            let undefined = &au | &bu;
            let same = &ay.iff(&by) & &!&undefined;
            let yes = if op == BinOp::Eq { same } else { &!&same & &!&undefined };
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Eq | BinOp::Ne => {
            let same = zip(&a.data(), &b.data(), |x, y| match (x, y) {
                (Val::Undefined, _) | (_, Val::Undefined) => Val::Undefined,
                _ => Val::Bool((x == y) == (op == BinOp::Eq)),
            })?;
            let e = Ev::Data(same);
            let (yes, undefined) = e.truth()?;
            Ok(Ev::Truth { yes, undefined })
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow | BinOp::Max | BinOp::Min => {
            let wrong = StdCell::new(false);
            let out = zip(&a.data(), &b.data(), |x, y| {
                numbers(x, y, |x, y| {
                    Val::num(match op {
                        BinOp::Add => x + y,
                        BinOp::Sub => x - y,
                        BinOp::Mul => x * y,
                        // A negative number to a fractional power isn't a number: NaN, so undefined.
                        BinOp::Pow => x.powf(y),
                        BinOp::Max => x.max(y),
                        BinOp::Min => x.min(y),
                        _ => x / y,
                    })
                }, &wrong)
            })?;
            if wrong.get() {
                return failed(format!("{sym} needs numbers"));
            }
            Ok(Ev::Data(out))
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let wrong = StdCell::new(false);
            let out = zip(&a.data(), &b.data(), |x, y| {
                numbers(x, y, |x, y| {
                    Val::Bool(match op {
                        BinOp::Lt => x < y,
                        BinOp::Le => x <= y,
                        BinOp::Gt => x > y,
                        _ => x >= y,
                    })
                }, &wrong)
            })?;
            if wrong.get() {
                return failed(format!("{sym} needs numbers"));
            }
            let (yes, undefined) = Ev::Data(out).truth()?;
            Ok(Ev::Truth { yes, undefined })
        }
    }
}

/// What's in these fields, one after another. A field of a record is that record's. A field of a node is
/// its cell, so what's read through a choice is whatever it lands on says, world by world.
fn field_of(cx: &Cx, c: &Cond<Val>, fields: &[String]) -> Result<Cond<Val>, Error> {
    let mut c = c.clone();
    for f in fields {
        c = if c.cases().iter().any(|(_, v)| matches!(v, Val::Ref(_))) {
            let mut out = pure(Val::Undefined);
            for (when, v) in c.cases() {
                let here = match v {
                    Val::Ref(e) => read_cell(cx, *e, f, &[])?.data(),
                    other => pure(other.field(f)),
                };
                out = Cond::when(&when, &here, &out);
            }
            out
        } else {
            c.map(|v| v.field(f))
        };
    }
    Ok(c)
}

/// A cell of any shape, as built-in cells write them: a `Bool`, cases, or something plain.
fn from_json(v: Value) -> Result<Ev, String> {
    let is_bool = v.as_object().is_some_and(|o| o.contains_key("vars") && o.contains_key("nodes") && o.contains_key("root"));
    if is_bool {
        let yes: Bool = serde_json::from_value(v).map_err(|e| e.to_string())?;
        return Ok(Ev::Truth { yes, undefined: Space::global().never() });
    }
    let w: Worldly<Val> = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok(Ev::Data(w.0))
}

// ---------- compiling ----------

/// What goes in the cell, given what a formula works out.
type Wrap<C> = fn(Cond<Val>) -> Result<C, Error>;

/// A formula's value, whichever cell it's on.
fn work_out(cx: &Cx, e: &Expr, expected: &Type) -> Result<Cond<Val>, Error> {
    let closed = Expander::new(cx)?.expand_all(e)?;
    let found = check(cx, &closed)?;
    if expected.join(&found).is_none() {
        return failed(format!("this cell holds {}, and this formula gives {}", expected.word(), found.word()));
    }
    Ok(evaluate(cx, &closed)?.data())
}

/// How a formula on a cell that holds `C` is made: it has to give `expected`, and `wrap` makes that
/// the cell's type.
pub fn compiler<C: CellType>(expected: Type, wrap: Wrap<C>) -> Compile {
    Arc::new(move |text: &str| {
        let e = expr::parse(text)?;
        let text = text.to_string();
        let expected = expected.clone();
        let is_value = C::NAME == ValueCell::NAME;
        Ok(Computed::typed(&format!("formula on {}", C::NAME), move |cx| {
            // A value's type is a cell of its own. Reading it first means a loop or a type error is found
            // there, by looking only at types.
            if is_value {
                cx.own::<ValueType>()?;
            }
            Ok(Some(wrap(work_out(cx, &e, &expected).map_err(|err| in_text(err, &text))?)?))
        }))
    })
}

/// A formula on one of the workspace's own cells, such as a price or a range, which no component
/// brings. Its type is checked as it runs.
fn free_compiler() -> Compile {
    Arc::new(|text: &str| {
        let e = expr::parse(text)?;
        let text = text.to_string();
        Ok(Computed::json("formula", move |cx| {
            let c = work_out(cx, &e, &Type::Any).map_err(|err| in_text(err, &text))?;
            Ok(Some(serde_json::to_value(Worldly(c)).expect("values serialise")))
        }))
    })
}

/// What a formula gives a cell that holds true or false. Undefined is `None`.
pub fn flag_cases(c: Cond<Val>, what: &str) -> Result<Worldly<Option<bool>>, Error> {
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !matches!(v, Val::Bool(_) | Val::Undefined)) {
        return failed(format!("{what} is true or false, not {}", Type::of(bad).word()));
    }
    Ok(Worldly(c.map(|v| if let Val::Bool(b) = v { Some(*b) } else { None })))
}

/// What a formula gives a cell that holds a number. Undefined is `None`.
pub fn number_cases(c: Cond<Val>, what: &str) -> Result<Worldly<Option<f64>>, Error> {
    if let Some((_, bad)) = c.cases().iter().find(|(_, v)| !matches!(v, Val::Num(_) | Val::Undefined)) {
        return failed(format!("{what} is a number, not {}", Type::of(bad).word()));
    }
    Ok(Worldly(c.map(|v| if let Val::Num(n) = v { Some(*n) } else { None })))
}

/// What this formula would give on this node, and what it expands to, with nothing changed. Any part
/// of a formula can be asked about this way, because it can read what the whole formula can.
pub fn try_formula(g: &Graph, e: Entity, text: &str) -> Result<(Cond<Val>, String), Error> {
    let parsed = expr::parse(text).map_err(Error::Failed)?;
    g.run(e, |cx| {
        let expansion = render(&Expander::new(cx)?.expand_all(&parsed)?);
        let value = work_out(cx, &parsed, &Type::Any).map_err(|err| in_text(err, text))?;
        Ok((value, expansion))
    })
}

/// A formula as it reads once it's been expanded, with nodes as handles. It's as long as it's useful
/// to read, and then it stops.
fn render(closed: &Closed) -> String {
    const LONGEST: usize = 2000;
    fn read(node: &Entity, component: &str, fields: &[String]) -> String {
        let mut s = node.to_string();
        if component != ValueCell::NAME || !fields.is_empty() {
            s = format!("{s}.{component}");
        }
        fields.iter().for_each(|f| s = format!("{s}.{f}"));
        s
    }
    let mut out: Vec<String> = Vec::with_capacity(closed.0.len());
    for part in &closed.0 {
        let text = match part {
            Part::Lit(Val::Num(n)) => n.to_string(),
            Part::Lit(Val::Undefined) => "undefined".to_string(),
            Part::Lit(Val::Ref(e)) => e.to_string(),
            Part::Lit(v) => serde_json::to_string(v).expect("values serialise"),
            Part::Read { node, component, fields } => read(node, component, fields),
            Part::Field(a, fields) => format!("{}.{}", out[*a], fields.join(".")),
            Part::If(c, a, b) => format!("if({}, {}, {})", out[*c], out[*a], out[*b]),
            Part::Bin(op @ (BinOp::Max | BinOp::Min), a, b) => format!("{}({}, {})", op.symbol(), out[*a], out[*b]),
            Part::Bin(op, a, b) => format!("({} {} {})", out[*a], op.symbol(), out[*b]),
            Part::Not(a) => format!("!{}", out[*a]),
            Part::Neg(a) => format!("-{}", out[*a]),
            Part::Defined(a) => format!("defined({})", out[*a]),
            Part::Has(n, c) => format!("has({n}.{c})"),
            Part::Chose(d, o) => format!("chose({d}, {o})"),
            Part::Round(a, b) => format!("round({}, {})", out[*a], out[*b]),
            Part::Floor(a) => format!("floor({})", out[*a]),
            Part::Ceil(a) => format!("ceil({})", out[*a]),
        };
        out.push(if text.len() > LONGEST { "…".to_string() } else { text });
    }
    out.pop().unwrap_or_default()
}

fn in_text(err: Error, text: &str) -> Error {
    match err {
        Error::Failed(why) => Error::Failed(format!("{why}, in: {text}")),
        other => other,
    }
}

/// One of the workspace's own cells as a world value, if it holds what a formula would.
pub fn shown_free(g: &Graph, e: Entity, cell: &str) -> Option<Cell<Shown>> {
    let v = match g.get_value(e, cell) {
        Ok(Some(v)) => v,
        Ok(None) => return Some(Ok(None)),
        Err(err) => return Some(Err(err)),
    };
    let w: Worldly<Val> = serde_json::from_value(v).ok()?;
    Some(Ok(Some(shown_val(&w.0))))
}

// ---------- what's built in ----------

fn numeric(options: &[String]) -> bool {
    !options.is_empty() && options.iter().all(|o| o.parse::<f64>().is_ok_and(f64::is_finite))
}

/// The node nobody wrote for one of a list of options: its name is the option, and so is its value, which
/// is a number if all the options are.
pub fn option_node(g: &Graph, option: &str, numeric: bool) -> Entity {
    g.constant(option_cells(option, numeric))
}

fn option_cells(option: &str, numeric: bool) -> BTreeMap<String, Value> {
    let value = if numeric { json!(option.parse::<f64>().expect("checked by the caller")) } else { json!(option) };
    BTreeMap::from([(recollect_graph::Name::NAME.to_string(), json!(option)), (ValueCell::NAME.to_string(), value)])
}

/// How a choice with a list of options lands: on any of them, equally. A table's options are what its
/// rows choose from: it isn't a choice itself.
fn options_chosen(cx: &Cx) -> Cell<Chosen> {
    let Some(Options(options)) = cx.own::<Options>()? else { return Ok(None) };
    if cx.row_key().is_none() && !cx.rows().is_empty() {
        return Ok(None);
    }
    if options.is_empty() {
        return Ok(Some(Chosen(Worldly::plain(Val::Undefined))));
    }
    let is_numeric = numeric(&options);
    let nodes: HashMap<&str, Entity> = options.iter().map(|o| (o.as_str(), cx.constant(option_cells(o, is_numeric)))).collect();
    let names: Vec<&str> = options.iter().map(String::as_str).collect();
    let var = Space::global().uniform(&cx.id().to_string(), &names);
    Ok(Some(Chosen(Worldly(var.value().map(|o| Val::Ref(nodes[o.as_str()]))))))
}

/// Settling a choice is naming one node.
fn one_node(v: &Value) -> Result<(), String> {
    match serde_json::from_value::<Val>(v.clone()) {
        Ok(Val::Ref(_)) => Ok(()),
        _ => Err(format!("a choice is settled by naming a node, as {{\"ref\": handle}}, not {v}")),
    }
}

/// The options a node chooses between, if it's a choice: it has options, said or taken from its kind or
/// table, and it isn't a table, whose options are what its rows choose from.
pub fn options_of(g: &Graph, e: Entity) -> Option<Vec<String>> {
    let Options(options) = g.get::<Options>(e).ok()??;
    let table = g.row_of(e).is_none() && g.table_here(e).is_some_and(|t| !g.rows(&t).is_empty());
    (!table).then_some(options)
}

/// What type a node's value is.
fn type_of_value(cx: &Cx) -> Cell<ValueType> {
    Ok(match cx.backed(ValueCell::NAME) {
        None => None,
        Some(Backed::Value(v)) => {
            let v: Worldly<Val> = serde_json::from_value(v).map_err(|e| Error::Malformed { cell: (cx.id(), ValueCell::NAME.to_string()), detail: e.to_string() })?;
            let mut t = Type::Undefined;
            for (_, v) in v.0.cases() {
                t = t.join(&Type::of(v)).unwrap_or(Type::Any);
            }
            Some(ValueType(t))
        }
        Some(Backed::Builtin) => Some(ValueType(Type::Any)),
        Some(Backed::Formula(text)) => {
            let e = expr::parse(&text).map_err(Error::Failed)?;
            let closed = Expander::new(cx)?.expand_all(&e).map_err(|err| in_text(err, &text))?;
            Some(ValueType(check(cx, &closed).map_err(|err| in_text(err, &text))?))
        }
    })
}

/// A macro, with the macros it calls found.
fn macro_of(cx: &Cx) -> Cell<MacroBody> {
    let Some(Macro { params, body }) = cx.own::<Macro>()? else { return Ok(None) };
    let parsed = expr::parse(&body).map_err(|e| Error::Failed(format!("{e}, in: {body}")))?;
    let mut uses = BTreeMap::new();
    for (to, edge) in cx.outgoing(LABEL) {
        if let (Some(Called(name)), Some(m)) = (cx.get::<Called>(edge)?, cx.get::<MacroBody>(to)?) {
            uses.insert(name, m);
        }
    }
    Ok(Some(MacroBody { params, body: parsed, uses }))
}

fn valid_name(v: &Value) -> Result<(), String> {
    match v.as_str() {
        Some(s) if expr::is_name(s) => Ok(()),
        _ => Err(format!("a label has to be a name a formula can use, such as rate, not {v}")),
    }
}

/// How a value reads. If it's only ever true or false, it reads as the worlds where it's true, and if it's
/// a node wherever it's anything, as the nodes it could be.
fn shown_val(c: &Cond<Val>) -> Shown {
    let cases = c.cases();
    if cases.iter().all(|(_, v)| matches!(v, Val::Bool(_))) {
        return Shown::Bool(c.holds(|v| *v == Val::Bool(true)));
    }
    if cases.iter().any(|(_, v)| matches!(v, Val::Ref(_))) && cases.iter().all(|(_, v)| matches!(v, Val::Ref(_) | Val::Undefined)) {
        return Shown::Named(cases.into_iter().filter_map(|(when, v)| if let Val::Ref(e) = v { Some((*e, when)) } else { None }).collect());
    }
    Shown::cond(c)
}

pub fn install(reg: &mut Registry) {
    reg.free_formulas(free_compiler());
    reg.component(VALUE, "An answer: typed in, or a formula over what it has labelled.").icon("=");
    reg.cell::<ValueCell>(VALUE, "This node's answer: typed in, or a formula over what it has labelled.")
        .compiles(compiler::<ValueCell>(Type::Any, |c| Ok(ValueCell(Worldly(c)))))
        .shown(|v| shown_val(&v.0.0))
        .summary()
        .per_entity()
        .initial(json!(0.0));
    reg.cell::<ValueType>(VALUE, "").builtin(type_of_value).internal();

    reg.component(CHOICE, "Lands on one node in each world. What's read through it, as chosen.price, is what that node says.").icon("◇");
    reg.cell::<Chosen>(CHOICE, "What it lands on, in each world. Name one to settle it.").validate(one_node).shown(|c| shown_val(&c.0.0)).summary().per_entity();

    // It reads as the nodes it could be, as `chosen` does, so what's read through it is known to be.
    reg.cell::<Lands>(CHOICE, "").builtin(lands).shown(|c| shown_val(&c.0.0)).internal();
    reg.edge(STANDS_FOR, "This option is whatever that choice lands on: choosing it is choosing that.").target_has(CHOICE);

    reg.component(OPTIONS, "A choice from a plain list, such as colours: each is a node with that name and value.").requires(CHOICE);
    reg.cell::<Options>(OPTIONS, "What it can land on.").initial(json!([]));
    reg.fill::<Chosen>(OPTIONS, options_chosen);
    reg.component(AT, "Where a choice goes on a board.").requires(OPTIONS);
    reg.cell::<At>(AT, "Where a choice goes on a board.").initial(json!({ "row": 0, "col": 0 }));

    reg.component(REQUIRES, "Only the worlds where this node's value is true count.").icon("✦").requires(VALUE).constraint(|g, e| Ok(g.get::<ValueCell>(e)?.map(|v| truth(&v))));

    reg.component(MACRO, "A function of your own, which formulas call by labelling this node.").icon("ƒ");
    reg.cell::<Macro>(MACRO, "Its parameters, and the formula it stands for.").initial(json!({ "params": [], "body": "0" }));
    reg.cell::<MacroBody>(MACRO, "").builtin(macro_of).internal();

    reg.edge(LABEL, "The name a formula on this node uses for that one.").arranges().reversed();
    reg.cell::<Called>(LABEL, "The name a formula on the source uses for the target.").validate(valid_name).initial(json!(""));
    // A kind is where a formula is written, not a step its numbers go through, so it doesn't shape a map.
    reg.edge(IS_A, "What kind of thing this is: it has its kind's components, and takes any cell it doesn't say from there.");
    reg.inherit_through(IS_A);
}
