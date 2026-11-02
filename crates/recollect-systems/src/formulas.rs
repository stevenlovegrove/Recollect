//! Formulas, as a system. A node's `value` is a number or some text typed in, a choice from a
//! `Domain`, or a formula over what its labelled links reach. See `docs/expressions.md`.
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

use recollect_graph::worlds::{Bool, Cond, Space, Worldly};
use recollect_graph::{Cell, Component, Compute, Cx, Entity, Error, Graph, Inputs, Place, Registry, Rule, RuleMeta, component, rule};
use serde_json::{Value, json};

use crate::expr::{self, BinOp, CORE, Expr, Type, Val, inputs_of};
use crate::plan::{Choice, Done, Dropped, Need};
use crate::schedule::{Finish, LatestFinish, LatestStart, PlanEnd, Start};
use crate::{Catalog, Shown};

/// The rule. Its meaning is fixed by this key, so a change in meaning needs a new one.
pub const EXPR: &str = "expr@1";

/// The most distinct values a formula can have across worlds. Beyond this, it says so.
const MOST_CASES: usize = 10_000;
/// The most pairs of cases one step can look at.
const MOST_PAIRS: usize = 4_000_000;
/// The most steps in expanding one formula. Macros calling macros can get very big very quickly.
pub(crate) const MOST_STEPS: usize = 200_000;

/// A node's answer. Typed in, or from a `Domain`, or from a formula.
#[component("value")]
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

/// The options this choice has.
#[component]
pub struct Domain(pub Vec<String>);

/// Where this choice goes on a board, if the puzzle has one.
#[derive(Copy)]
#[component]
pub struct At {
    pub row: u32,
    pub col: u32,
}

/// Link: the name a formula here uses for that node. Several links with one name make a group.
#[component]
pub struct Label(pub String);

/// Link: what kind of thing this is. A node takes any of the workspace's own cells it doesn't say for
/// itself from its kind, and a formula taken that way reads what the kind has labelled.
#[component("is_a")]
pub struct IsA;

/// Only the worlds where this node's value is true count.
#[component]
pub struct Requires;

/// A function of your own, written as a formula over its parameters.
#[component("macro")]
pub struct Macro {
    pub params: Vec<String>,
    pub body: String,
}

/// A macro, with the macros it uses found. It's a cell so a macro calling itself is a loop in these
/// cells, which needs nothing special, and so a caller only ever reads one hop.
#[component("macro_body")]
pub struct MacroBody {
    pub params: Vec<String>,
    pub body: Expr,
    pub uses: BTreeMap<String, MacroBody>,
}

/// What type a node's value is. A type doesn't depend on a world, or on any value that isn't typed
/// in, so this reruns far less often than the value does, and stops there when it comes out the same.
#[component("value_type")]
pub struct ValueType(pub Type);

/// A formula tried out on a node without changing anything, and how it expands.
#[component("formula_try")]
pub struct TryValue(pub Worldly<Val>);

#[component("formula_expansion")]
pub struct TryExpansion(pub String);

/// What the components of compiled systems hold, for a formula reading them. Anything else is
/// checked as it runs.
pub(crate) fn component_type(component: &str) -> Type {
    match component {
        "need" | "actionable" | "blocked" | "done" | "dropped" | "requires" => Type::Bool,
        "estimate" | "start" | "finish" | "latest_start" | "latest_finish" | "slack" | "plan_end" => Type::Number,
        "name" | "body" | "choice" => Type::Text,
        _ => Type::Any,
    }
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
        for (to, label) in cx.outgoing_from_origin(Label::NAME)? {
            let Label(name) = label.decode().map_err(|e| Error::Malformed { cell: (cx.origin(), Label::NAME.to_string()), detail: e.to_string() })?;
            labels.entry(name).or_default().push(to);
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
        let (root, rest) = path.split_first().expect("a path has a first name");
        if let Some(b) = scope.vars.get(root) {
            return match b {
                Binding::Node(n) => Ok(vec![*n]),
                Binding::Group(g) => Ok(g.clone()),
                Binding::Value(_) => failed(format!("{root} isn't a group of nodes")),
            };
        }
        if matches!(root.as_str(), "in" | "out" | "with") {
            if scope.is_macro() {
                return failed(format!("a macro can only use its parameters, not {root}.…"));
            }
            let Some(kind) = rest.first() else { return failed(format!("{root} needs to say which kind")) };
            return match root.as_str() {
                "in" => Ok(self.cx.incoming_runtime(kind)?.into_iter().map(|(e, _)| e).collect()),
                "out" => Ok(self.cx.outgoing_runtime(kind)?.into_iter().map(|(e, _)| e).collect()),
                _ => self.cx.scan(kind),
            };
        }
        if scope.is_macro() {
            return failed(format!("a macro can only use its parameters, not {root}"));
        }
        // No links yet is fine for a group: a total with no lines is 0.
        Ok(self.labels.get(root).cloned().unwrap_or_default())
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
            } else if matches!(root, "in" | "out" | "with") {
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
            Expr::Fold { func, var, group, body, start } => self.fold(func, var, group, body, start.as_deref(), scope),
            Expr::Call(name, args) => self.call(name, args, scope),
        })
    }

    fn path(&self, path: &[String], scope: &Scope) -> Result<Id, Error> {
        let (root, rest) = path.split_first().expect("a path has a first name");
        match scope.vars.get(root) {
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
            "in" | "out" | "with" => failed(format!("{}.… is a group of nodes, so it needs something like sum({}.…)", root, root)),
            _ => match self.labels.get(root).map(Vec::as_slice) {
                None | Some([]) => failed(format!("no input called {root}")),
                Some([one]) => Ok(self.read(*one, rest)),
                Some(many) => failed(format!("{root} is {} nodes, so it needs something like sum({root})", many.len())),
            },
        }
    }

    fn fold(&self, func: &str, var: &str, group: &[String], body: &Expr, start: Option<&Expr>, scope: &Scope) -> Result<Id, Error> {
        let nodes = self.group(group, scope)?;
        let copy = |n: Entity| {
            let mut inner = scope.clone();
            inner.vars.insert(var.to_string(), Binding::Node(n));
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
            "pick" => {
                if scope.is_macro() {
                    return failed("a macro can only use its parameters, so it can't pick for the node calling it");
                }
                let mut acc = from(Val::Undefined)?;
                for n in nodes.iter().rev() {
                    let chosen = self.add(Part::Chose(self.cx.id(), *n));
                    let this = copy(*n)?;
                    acc = self.add(Part::If(chosen, this, acc));
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
            // The library isn't a macro of the kind a workspace writes: it can follow what it says,
            // which is why `inputs_of` looks inside it.
            let vars = self.bind(name, &m.params, args, scope)?;
            return self.expand(&m.body, &Scope { vars, macro_uses: None });
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
        self.expand(body, &Scope { vars, macro_uses: Some(uses) })
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
        let t = type_of(cx, part, &types, closed)?;
        types.push(t);
    }
    Ok(types.pop().expect("a formula has something in it"))
}

fn type_of(cx: &Cx, part: &Part, types: &[Type], closed: &Closed) -> Result<Type, Error> {
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
                    for (side, other) in [(*a, tb), (*b, ta)] {
                        if let (Part::Lit(Val::Text(s)), Type::Enum(options)) = (&closed.0[side], other)
                            && !options.contains(s)
                        {
                            return failed(format!("“{s}” isn't one of the options, which are {}", options.join(", ")));
                        }
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
        _ => match cx.get_runtime(node, component)? {
            None => Ev::Data(pure(Val::Undefined)),
            Some(r) => from_json(r.value).map_err(|detail| Error::Malformed { cell: (node, component.to_string()), detail })?,
        },
    };
    if fields.is_empty() {
        return Ok(whole);
    }
    Ok(Ev::Data(field_of(&whole.data(), fields)))
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
        Part::Field(of, fields) => Ev::Data(field_of(&values[*of].data(), fields)),
        Part::Defined(a) => {
            let undefined = match &values[*a] {
                Ev::Truth { undefined, .. } => undefined.clone(),
                Ev::Data(d) => d.holds(|v| *v == Val::Undefined),
            };
            Ev::Truth { yes: !&undefined, undefined: space.never() }
        }
        Part::Has(node, component) => Ev::Truth { yes: space.truth(cx.get_runtime(*node, component)?.is_some()), undefined: space.never() },
        Part::Chose(decision, option) => {
            let Some(Choice(choice)) = cx.get::<Choice>(*decision)? else { return failed("chose needs a decision") };
            Ev::Truth { yes: choice.holds(|c| *c == Some(*option)), undefined: space.never() }
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

fn field_of(c: &Cond<Val>, fields: &[String]) -> Cond<Val> {
    c.map(|v| fields.iter().fold(v.clone(), |v, f| v.field(f)))
}

/// A cell of any shape, as compiled rules write them: a `Bool`, cases, or something plain.
fn from_json(v: Value) -> Result<Ev, String> {
    let is_bool = v.as_object().is_some_and(|o| o.contains_key("vars") && o.contains_key("nodes") && o.contains_key("root"));
    if is_bool {
        let yes: Bool = serde_json::from_value(v).map_err(|e| e.to_string())?;
        return Ok(Ev::Truth { yes, undefined: Space::global().never() });
    }
    let w: Worldly<Val> = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok(Ev::Data(w.0))
}

// ---------- the rules ----------

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

/// The text a formula is written in, from its rule's parameters.
pub fn formula_text(params: &Value) -> Result<&str, String> {
    params.get("text").and_then(Value::as_str).ok_or_else(|| "a formula's parameters are { text }".to_string())
}

/// Say that a formula can fill this component.
pub fn fills<C: Component>(reg: &mut Registry, expected: Type, wrap: Wrap<C>) {
    reg.rule_on::<C>(EXPR, move |params| build::<C>(params, expected.clone(), wrap));
}

/// Say that a formula can fill this component, and what it's worked out by on any node that doesn't
/// say otherwise. A node can type over it with a value or a formula of its own, and removing that
/// brings this back.
pub fn defaults_to<C: Component>(reg: &mut Registry, expected: Type, wrap: Wrap<C>, text: &str) {
    fills::<C>(reg, expected, wrap);
    reg.on_nodes::<C>();
    reg.set_default_rule(C::NAME, EXPR, json!({ "text": text }));
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

fn build<C: Component>(params: &Value, expected: Type, wrap: Wrap<C>) -> Result<Rule<C>, String> {
    let text = formula_text(params)?;
    let e = expr::parse(text)?;
    let inputs = inputs_of(&e);
    let text = text.to_string();
    let is_value = C::NAME == ValueCell::NAME;
    Ok(rule(EXPR).following(inputs).optional(move |cx| {
        // A value's type is its own column. Reading it first means a loop or a type error is found
        // there, by looking only at types.
        if is_value {
            cx.own::<ValueType>()?;
        }
        Ok(Some(wrap(work_out(cx, &e, &expected).map_err(|err| in_text(err, &text))?)?))
    }))
}

/// What this formula would give on this node, and what it expands to, with nothing changed. Any part
/// of a formula can be asked about this way, because it can read what the whole formula can.
pub fn try_formula(g: &Graph, e: Entity, text: &str) -> Result<(Cond<Val>, String), Error> {
    let mut tried = g.fork();
    let params = json!({ "text": text });
    tried.set_rule(e, TryValue::NAME, EXPR, params.clone());
    tried.set_rule(e, TryExpansion::NAME, EXPR, params);
    let value = tried.get::<TryValue>(e)?.expect("a formula always gives something");
    let expansion = tried.get::<TryExpansion>(e)?.expect("a formula always gives something");
    Ok((value.0.0, expansion.0))
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

fn build_expansion(params: &Value) -> Result<Rule<TryExpansion>, String> {
    let text = formula_text(params)?;
    let e = expr::parse(text)?;
    Ok(rule(EXPR).following(inputs_of(&e)).compute(move |cx| Ok(TryExpansion(render(&Expander::new(cx)?.expand_all(&e)?)))))
}

/// What `expr` gives a `value`.
fn value_cell(c: Cond<Val>) -> Result<ValueCell, Error> {
    Ok(ValueCell(Worldly(c)))
}

/// A formula on one of the workspace's own cells, such as a price or a range, which no system says
/// anything about. Its type is checked as it runs.
#[allow(clippy::type_complexity)]
fn build_unowned(_component: &str, params: &Value) -> Result<(Inputs, std::sync::Arc<dyn Fn(&Cx) -> Cell<Value> + Send + Sync>), String> {
    let text = formula_text(params)?.to_string();
    let e = expr::parse(&text)?;
    let inputs = inputs_of(&e);
    Ok((
        inputs,
        std::sync::Arc::new(move |cx: &Cx| {
            let c = work_out(cx, &e, &Type::Any).map_err(|err| in_text(err, &text))?;
            Ok(Some(serde_json::to_value(Worldly(c)).expect("values serialise")))
        }),
    ))
}

/// One of the workspace's own cells as a world value, if it holds what a formula would.
pub(crate) fn shown_unowned(g: &Graph, e: Entity, component: &str) -> Option<Cell<Shown>> {
    let r = match g.get_runtime(e, component) {
        Ok(Some(r)) => r,
        Ok(None) => return Some(Ok(None)),
        Err(err) => return Some(Err(err)),
    };
    let w: Worldly<Val> = serde_json::from_value(r.value).ok()?;
    Some(Ok(Some(shown_of(&ValueCell(w)))))
}

/// A choice's value, for a node with a domain. If its options are all numbers, it reads as numbers.
#[recollect_graph::rule("formulas/choice")]
pub struct ChoiceOf;

fn numeric(options: &[String]) -> bool {
    !options.is_empty() && options.iter().all(|o| o.parse::<f64>().is_ok_and(f64::is_finite))
}

impl Compute for ChoiceOf {
    type Output = Option<ValueCell>;
    fn compute(&self, cx: &Cx) -> Result<Option<ValueCell>, Error> {
        let Some(Domain(options)) = cx.own::<Domain>()? else { return Ok(None) };
        let names: Vec<&str> = options.iter().map(String::as_str).collect();
        let var = Space::global().uniform(&cx.id().to_string(), &names);
        let is_numeric = numeric(&options);
        Ok(Some(ValueCell(Worldly(var.value().map(|o| if is_numeric { Val::Num(o.parse().expect("checked above")) } else { Val::Text(o.clone()) })))))
    }
}

/// What type a node's value is. It's allowed to read whatever the formula on `value` can.
#[recollect_graph::rule("formulas/type")]
pub struct TypeOf;

impl Compute for TypeOf {
    type Output = Option<ValueType>;

    fn inputs(&self) -> Inputs {
        Inputs { like: Some(ValueCell::NAME.to_string()), outgoing: vec![Label::NAME.to_string()], ..Inputs::default() }
    }

    fn compute(&self, cx: &Cx) -> Result<Option<ValueType>, Error> {
        let Some(saved) = cx.own_rule(ValueCell::NAME) else {
            let Some(stored) = cx.get_runtime(cx.id(), ValueCell::NAME)? else { return Ok(None) };
            let v: Worldly<Val> = serde_json::from_value(stored.value).map_err(|e| Error::Malformed { cell: (cx.id(), ValueCell::NAME.to_string()), detail: e.to_string() })?;
            let mut t = Type::Undefined;
            for (_, v) in v.0.cases() {
                t = t.join(&Type::of(v)).unwrap_or(Type::Any);
            }
            return Ok(Some(ValueType(t)));
        };
        if saved.rule == ChoiceOf::KEY {
            let Some(Domain(options)) = cx.own::<Domain>()? else { return Ok(None) };
            return Ok(Some(ValueType(if numeric(&options) { Type::Number } else { Type::Enum(options) })));
        }
        if saved.rule != EXPR {
            return Ok(Some(ValueType(Type::Any)));
        }
        let text = formula_text(&saved.params).map_err(Error::Failed)?;
        let e = expr::parse(text).map_err(Error::Failed)?;
        let closed = Expander::new(cx)?.expand_all(&e).map_err(|err| in_text(err, text))?;
        let found = check(cx, &closed).map_err(|err| in_text(err, text))?;
        Ok(Some(ValueType(found)))
    }
}

fn in_text(err: Error, text: &str) -> Error {
    match err {
        Error::Failed(why) => Error::Failed(format!("{why}, in: {text}")),
        other => other,
    }
}

/// A macro, with the macros it calls found.
#[recollect_graph::rule("formulas/macro", outgoing(Label))]
pub struct MacroOf;

impl Compute for MacroOf {
    type Output = Option<MacroBody>;
    fn compute(&self, cx: &Cx) -> Result<Option<MacroBody>, Error> {
        let Some(Macro { params, body }) = cx.own::<Macro>()? else { return Ok(None) };
        let parsed = expr::parse(&body).map_err(|e| Error::Failed(format!("{e}, in: {body}")))?;
        let mut uses = BTreeMap::new();
        for (to, Label(name)) in cx.outgoing::<Label>()? {
            if let Some(m) = cx.get::<MacroBody>(to)? {
                uses.insert(name, m);
            }
        }
        Ok(Some(MacroBody { params, body: parsed, uses }))
    }
}

fn valid_label(v: &Value) -> Result<(), String> {
    match v.as_str() {
        Some(s) if expr::is_name(s) => Ok(()),
        _ => Err(format!("a label has to be a name a formula can use, such as rate, not {v}")),
    }
}

pub fn install(reg: &mut Registry) {
    if reg.contains(EXPR) {
        return;
    }
    fills::<ValueCell>(reg, Type::Any, value_cell);
    fills::<TryValue>(reg, Type::Any, |c| Ok(TryValue(Worldly(c))));
    reg.rule_on::<TryExpansion>(EXPR, build_expansion);
    reg.rule_on_unowned(EXPR, build_unowned);
    reg.set_default(ChoiceOf).set_default(TypeOf).set_default(MacroOf);
    reg.on_nodes::<ValueCell>().on_nodes::<Domain>().on_nodes::<At>().on_nodes::<Requires>().on_nodes::<Macro>();
    reg.declare(Label::NAME, Place::Link, Some(valid_label));
    reg.on_links::<IsA>().inherit_through(IsA::NAME);
}

/// How a value reads. If it's only ever true or false, it reads as the worlds where it's true.
fn shown_of(v: &ValueCell) -> Shown {
    let c = &v.0.0;
    if c.cases().iter().all(|(_, v)| matches!(v, Val::Bool(_))) { Shown::Bool(truth(v)) } else { Shown::cond(c) }
}

/// The worlds where this node's requirement is met, if it has one.
fn requirement(g: &Graph, e: Entity) -> Cell<Bool> {
    if !g.has_own(e, Requires::NAME) {
        return Ok(None);
    }
    Ok(g.get::<ValueCell>(e)?.map(|v| truth(&v)))
}

pub fn describe(c: &mut Catalog) {
    let value = c.worldly::<ValueCell>("This node's answer: typed in, a choice from its domain, or a formula over what it has labelled.", shown_of);
    value.derived = false;
    value.summary = true;
    value.initial = Some(json!(0.0));
    c.fact::<Domain>("The options this choice has.", json!([]));
    c.fact::<At>("Where this choice goes on a board.", json!({ "row": 0, "col": 0 }));
    c.fact::<Requires>("Only the worlds where this node's value is true count.", Value::Null);
    c.fact::<Macro>("A function of your own, which formulas can call by linking here with a label.", json!({ "params": [], "body": "0" }));
    c.internal::<ValueType>();
    c.internal::<MacroBody>();
    c.internal::<TryValue>();
    c.internal::<TryExpansion>();
    let label = c.arranging::<Label>("The name a formula on this node uses for that one.", json!(""));
    label.reversed = true;
    c.link::<IsA>("What kind of thing this is: it takes any cell of its own it doesn't say from there.", Value::Null);
    c.constraint_on(requirement);
}
