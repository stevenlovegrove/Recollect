//! The compute model: a spreadsheet over the graph. A cell is a function of one node. Cells
//! read the store and each other only through `Cx`, which caches each result, records what it
//! read, and reports a cycle instead of recursing forever. So nothing declares its dependencies
//! by hand, and evaluation order is simply demand.
//!
//! Everything has a typed form and an untyped one built on the same primitives, so a plugin
//! with no Rust types can read and compute everything a typed cell can.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};

use crate::formula::{F, FALSE, Mdd, TRUE, Var};
use crate::schema::{Alternative, Component, ComponentDef, Decision, LinkComponent, Schema, Unknown};
use crate::store::{Components, Id, PART_OF, Store, Violation};

/// A value that differs between worlds: mutually exclusive guards covering every world, with
/// equal values merged. Cost grows with the number of distinct values.
#[derive(Debug, Clone, PartialEq)]
pub struct Cond<T> {
    pub cases: Vec<(F, T)>,
}

impl<T> Cond<T> {
    /// The value in one world: `decision → option`.
    pub fn at(&self, m: &Mdd, world: &BTreeMap<&str, &str>) -> &T {
        &self.cases.iter().find(|(g, _)| m.eval(*g, world)).expect("guards cover every world").1
    }
}

/// What a cell can hold. Anything serialisable can be held as `Cond<T>`.
pub trait CellValue: Clone + 'static {
    /// The value if this cell is on a cycle. The cycle itself is reported.
    fn fallback() -> Self;
    fn as_formula(&self) -> Option<F> {
        None
    }
    fn to_json(&self, m: &mut Mdd) -> Cond<Value>;
}

impl CellValue for F {
    fn fallback() -> Self {
        FALSE
    }
    fn as_formula(&self) -> Option<F> {
        Some(*self)
    }
    fn to_json(&self, m: &mut Mdd) -> Cond<Value> {
        formula_to_json(*self, m)
    }
}

fn formula_to_json(f: F, m: &mut Mdd) -> Cond<Value> {
    let not = m.not(f);
    let cases = [(f, json!(true)), (not, json!(false))].into_iter().filter(|(g, _)| *g != FALSE).collect();
    Cond { cases }
}

impl<T: Serialize + Clone + Default + 'static> CellValue for Cond<T> {
    fn fallback() -> Self {
        Cond { cases: vec![(TRUE, T::default())] }
    }
    fn to_json(&self, _: &mut Mdd) -> Cond<Value> {
        Cond { cases: self.cases.iter().map(|(g, v)| (*g, json!(v))).collect() }
    }
}

impl CellValue for Option<Id> {
    fn fallback() -> Self {
        None
    }
    fn to_json(&self, _: &mut Mdd) -> Cond<Value> {
        Cond { cases: vec![(TRUE, json!(self))] }
    }
}

impl CellValue for usize {
    fn fallback() -> Self {
        0
    }
    fn to_json(&self, _: &mut Mdd) -> Cond<Value> {
        Cond { cases: vec![(TRUE, json!(self))] }
    }
}

/// A named thing that holds up a node, with the worlds in which it does.
impl CellValue for Vec<(Id, F)> {
    fn fallback() -> Self {
        vec![]
    }
    fn to_json(&self, m: &mut Mdd) -> Cond<Value> {
        let v: Vec<Value> = self.iter().map(|(id, f)| json!({ "id": id, "when": m.show(*f) })).collect();
        Cond { cases: vec![(TRUE, json!(v))] }
    }
}

pub trait Cell: 'static {
    const NAME: &'static str;
    type Value: CellValue;
    fn eval(cx: &Cx, id: &str) -> Self::Value;
    /// The nodes this cell has a value for.
    fn domain(cx: &Cx) -> Vec<Id> {
        cx.entities().iter().map(|id| id.to_string()).collect()
    }
}

/// One thing a cell read: the audit trail behind its value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dep {
    Cell(String, Id),
    Component(Id, String),
    LinksInto(Id, String),
    LinksFrom(Id, String),
    AllLinksOf(Id),
    Scan(String),
    AllEntities,
}

pub type Key = (String, Id);

type EvalFn = Arc<dyn Fn(&Cx, &str) -> Result<F, Cond<Value>>>;
type DomainFn = Arc<dyn Fn(&Cx) -> Vec<Id>>;
type CheckFn = Arc<dyn Fn(&Cx) -> Vec<Notice>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub rule: &'static str,
    pub id: Id,
    pub detail: String,
}

/// A library of systems: component rules, cells and checks. A system is whatever installs
/// them; it needs no kernel change.
#[derive(Clone, Default)]
pub struct Registry {
    pub schema: Schema,
    cells: BTreeMap<String, (EvalFn, DomainFn)>,
    checks: Vec<CheckFn>,
}

impl Registry {
    pub fn add<C: Cell>(&mut self) -> &mut Self {
        let eval: EvalFn = Arc::new(|cx, id| {
            let v = cx.cell::<C>(id);
            match v.as_formula() {
                Some(f) => Ok(f),
                None => Err(v.to_json(&mut cx.mdd.borrow_mut())),
            }
        });
        self.insert(C::NAME, eval, Arc::new(C::domain));
        self
    }

    /// A cell with no Rust type: what a TypeScript system would register. `domain` lists the
    /// nodes it covers.
    pub fn add_untyped(&mut self, name: &str, domain: impl Fn(&Cx) -> Vec<Id> + 'static, eval: impl Fn(&Cx, &str) -> Cond<Value> + 'static) -> &mut Self {
        let owned = name.to_string();
        let eval: EvalFn = Arc::new(move |cx, id| Err(cx.memoised(&owned, id, || eval(cx, id), || Cond { cases: vec![(TRUE, Value::Null)] })));
        self.insert(name, eval, Arc::new(domain));
        self
    }

    pub fn add_check(&mut self, check: impl Fn(&Cx) -> Vec<Notice> + 'static) -> &mut Self {
        self.checks.push(Arc::new(check));
        self
    }

    pub fn declare(&mut self, def: ComponentDef) -> &mut Self {
        self.schema.declare(def);
        self
    }

    fn insert(&mut self, name: &str, eval: EvalFn, domain: DomainFn) {
        let previous = self.cells.insert(name.to_string(), (eval, domain));
        assert!(previous.is_none(), "two cells are named {name}");
    }
}

/// A link as seen from one of its ends, with every component, typed or not.
#[derive(Debug, Clone, Copy)]
pub struct LinkRef<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub components: &'a Components,
}

/// All a cell can see. The store is private, so every read goes through here and is recorded.
pub struct Cx<'a> {
    store: &'a Store,
    reg: &'a Registry,
    into: HashMap<&'a str, Vec<LinkRef<'a>>>,
    from: HashMap<&'a str, Vec<LinkRef<'a>>>,
    mdd: RefCell<Mdd>,
    memo: RefCell<HashMap<Key, Rc<dyn Any>>>,
    stack: RefCell<Vec<Key>>,
    trace: RefCell<BTreeMap<Key, BTreeSet<Dep>>>,
    cycles: RefCell<Vec<Vec<Key>>>,
    undecodable: RefCell<Vec<Violation>>,
}

impl<'a> Cx<'a> {
    fn new(store: &'a Store, reg: &'a Registry) -> Self {
        let (mut into, mut from): (HashMap<&str, Vec<LinkRef>>, HashMap<&str, Vec<LinkRef>>) = Default::default();
        for ((f, t), components) in &store.links {
            let link = LinkRef { from: f, to: t, components };
            into.entry(t).or_default().push(link);
            from.entry(f).or_default().push(link);
        }
        Cx {
            store,
            reg,
            into,
            from,
            mdd: RefCell::new(Mdd::new(world_space(store))),
            memo: Default::default(),
            stack: Default::default(),
            trace: Default::default(),
            cycles: Default::default(),
            undecodable: Default::default(),
        }
    }

    fn read(&self, dep: Dep) {
        if let Some(top) = self.stack.borrow().last() {
            self.trace.borrow_mut().entry(top.clone()).or_default().insert(dep);
        }
    }

    fn memoised<V: Clone + 'static>(&self, name: &str, id: &str, eval: impl FnOnce() -> V, fallback: impl FnOnce() -> V) -> V {
        let key: Key = (name.to_string(), id.to_string());
        if let Some(v) = self.memo.borrow().get(&key) {
            return v.downcast_ref::<V>().expect("one cell, one value type").clone();
        }
        if let Some(at) = self.stack.borrow().iter().position(|k| *k == key) {
            self.cycles.borrow_mut().push(self.stack.borrow()[at..].to_vec());
            return fallback();
        }
        self.stack.borrow_mut().push(key.clone());
        // Depth follows the graph, so the stack has to grow with it.
        let v = stacker::maybe_grow(256 * 1024, 4 * 1024 * 1024, eval);
        self.stack.borrow_mut().pop();
        self.memo.borrow_mut().insert(key, Rc::new(v.clone()));
        v
    }

    fn decode<T: serde::de::DeserializeOwned>(&self, what: String, v: &Value) -> Option<T> {
        match serde_json::from_value(v.clone()) {
            Ok(t) => Some(t),
            Err(e) => {
                let detail = format!("{what}: {e}");
                let mut bad = self.undecodable.borrow_mut();
                if !bad.iter().any(|b| b.detail == detail) {
                    bad.push(Violation { rule: "malformed_component", detail });
                }
                None
            }
        }
    }

    // ---------- reading another cell ----------

    pub fn cell<C: Cell>(&self, id: &str) -> C::Value {
        self.read(Dep::Cell(C::NAME.into(), id.into()));
        self.memoised(C::NAME, id, || C::eval(self, id), C::Value::fallback)
    }

    /// Any registered cell by name, as JSON. `None` means no cell has that name.
    pub fn cell_untyped(&self, name: &str, id: &str) -> Option<Cond<Value>> {
        let (eval, _) = self.reg.cells.get(name)?;
        self.read(Dep::Cell(name.into(), id.into()));
        Some(match eval(self, id) {
            Ok(f) => formula_to_json(f, &mut self.mdd.borrow_mut()),
            Err(v) => v,
        })
    }

    // ---------- untyped primitives: every component, known or not ----------

    pub fn entities(&self) -> Vec<&'a str> {
        self.read(Dep::AllEntities);
        self.store.entities.keys().map(String::as_str).collect()
    }

    /// Every component on a node, including ones nothing here understands.
    pub fn components(&self, id: &str) -> Option<&'a Components> {
        self.read(Dep::AllLinksOf(id.into()));
        self.store.entities.get(id)
    }

    pub fn raw(&self, id: &str, component: &str) -> Option<&'a Value> {
        self.read(Dep::Component(id.into(), component.into()));
        self.store.entities.get(id)?.get(component)
    }

    /// Enumerate by component. Each value is guaranteed present.
    pub fn with_raw(&self, component: &str) -> Vec<(&'a str, &'a Value)> {
        self.read(Dep::Scan(component.into()));
        self.store.entities.iter().filter_map(|(id, c)| Some((id.as_str(), c.get(component)?))).collect()
    }

    pub fn links_into_raw(&self, to: &str, component: &str) -> Vec<(&'a str, &'a Value)> {
        self.read(Dep::LinksInto(to.into(), component.into()));
        self.into.get(to).into_iter().flatten().filter_map(|l| Some((l.from, l.components.get(component)?))).collect()
    }

    pub fn links_from_raw(&self, from: &str, component: &str) -> Vec<(&'a str, &'a Value)> {
        self.read(Dep::LinksFrom(from.into(), component.into()));
        self.from.get(from).into_iter().flatten().filter_map(|l| Some((l.to, l.components.get(component)?))).collect()
    }

    /// Every link touching a node, in and out, of any kind.
    pub fn links_of(&self, id: &str) -> Vec<LinkRef<'a>> {
        self.read(Dep::AllLinksOf(id.into()));
        self.into.get(id).into_iter().chain(self.from.get(id)).flatten().copied().collect()
    }

    // ---------- typed primitives, built on the ones above ----------

    pub fn has<T: Component>(&self, id: &str) -> bool {
        self.raw(id, T::NAME).is_some()
    }

    pub fn get<T: Component>(&self, id: &str) -> Option<T> {
        self.decode(format!("{id}.{}", T::NAME), self.raw(id, T::NAME)?)
    }

    /// Every node with `T`, along with it.
    pub fn with<T: Component>(&self) -> Vec<(&'a str, T)> {
        self.with_raw(T::NAME).into_iter().filter_map(|(id, v)| Some((id, self.decode(format!("{id}.{}", T::NAME), v)?))).collect()
    }

    /// Links holding `L` that point at `to`: (source, component).
    pub fn links_into<L: LinkComponent>(&self, to: &str) -> Vec<(&'a str, L)> {
        self.links_into_raw(to, L::NAME).into_iter().filter_map(|(f, v)| Some((f, self.decode(format!("{f} → {to}.{}", L::NAME), v)?))).collect()
    }

    /// Links holding `L` that leave `from`: (target, component).
    pub fn links_from<L: LinkComponent>(&self, from: &str) -> Vec<(&'a str, L)> {
        self.links_from_raw(from, L::NAME).into_iter().filter_map(|(t, v)| Some((t, self.decode(format!("{from} → {t}.{}", L::NAME), v)?))).collect()
    }

    // ---------- schema-aware helpers ----------

    pub fn live(&self, id: &str) -> bool {
        self.read(Dep::Component(id.into(), crate::store::DROPPED.into()));
        self.store.live(id)
    }

    pub fn has_obligation(&self, id: &str) -> bool {
        self.components(id).is_some_and(|c| c.keys().any(|k| self.reg.schema.is_obligation(k)))
    }

    /// Whatever this node is placed inside, by any link declared as scoping.
    pub fn scoping_parents(&self, id: &str) -> Vec<&'a str> {
        self.read(Dep::AllLinksOf(id.into()));
        self.from.get(id).into_iter().flatten().filter(|l| l.components.keys().any(|c| self.reg.schema.is_scoping(c))).map(|l| l.to).collect()
    }

    /// Live children over `part_of`.
    pub fn parts(&self, id: &str) -> Vec<&'a str> {
        self.links_into_raw(id, PART_OF).into_iter().map(|(c, _)| c).filter(|c| self.live(c)).collect()
    }

    /// Options still in play: live alternatives that aren't ruled out.
    pub fn options(&self, decision: &str) -> Vec<&'a str> {
        self.links_into::<Alternative>(decision).into_iter().filter(|(o, a)| a.ruled_out.is_none() && self.live(o)).map(|(o, _)| o).collect()
    }

    /// In which worlds `decision` lands on `option`. A decided decision is a constant.
    pub fn selects(&self, decision: &str, option: &str) -> F {
        if let Some(c) = self.get::<Decision>(decision).and_then(|d| d.chosen) {
            return if c == option { TRUE } else { FALSE };
        }
        let mut m = self.mdd.borrow_mut();
        let Some(var) = m.vars.iter().position(|v| v.name == decision) else { return FALSE };
        match m.vars[var].options.iter().position(|o| o == option) {
            Some(i) => m.literal(var, i),
            None => FALSE,
        }
    }

    // ---------- working with values that differ between worlds ----------

    pub fn and(&self, a: F, b: F) -> F {
        self.mdd.borrow_mut().and(a, b)
    }

    pub fn or(&self, a: F, b: F) -> F {
        self.mdd.borrow_mut().or(a, b)
    }

    pub fn not(&self, a: F) -> F {
        self.mdd.borrow_mut().not(a)
    }

    pub fn show(&self, f: F) -> String {
        self.mdd.borrow().show(f)
    }

    pub fn pure<T>(&self, v: T) -> Cond<T> {
        Cond { cases: vec![(TRUE, v)] }
    }

    pub fn map<A, B: PartialEq>(&self, a: &Cond<A>, f: impl Fn(&A) -> B) -> Cond<B> {
        let mut out = Cond { cases: Vec::new() };
        for (g, v) in &a.cases {
            self.push_case(&mut out, *g, f(v));
        }
        out
    }

    pub fn zip<A, B, C: PartialEq>(&self, a: &Cond<A>, b: &Cond<B>, f: impl Fn(&A, &B) -> C) -> Cond<C> {
        let mut out = Cond { cases: Vec::new() };
        for (ga, va) in &a.cases {
            for (gb, vb) in &b.cases {
                let g = self.and(*ga, *gb);
                if g != FALSE {
                    self.push_case(&mut out, g, f(va, vb));
                }
            }
        }
        out
    }

    /// `then` in the worlds where `guard` holds, `otherwise` in the rest.
    pub fn when<T: Clone + PartialEq>(&self, guard: F, then: &Cond<T>, otherwise: &Cond<T>) -> Cond<T> {
        let not = self.not(guard);
        let mut out = Cond { cases: Vec::new() };
        for (side, c) in [(guard, then), (not, otherwise)] {
            for (g, v) in &c.cases {
                let both = self.and(side, *g);
                if both != FALSE {
                    self.push_case(&mut out, both, v.clone());
                }
            }
        }
        out
    }

    pub fn sum(&self, values: impl IntoIterator<Item = Cond<f64>>) -> Cond<f64> {
        values.into_iter().fold(self.pure(0.0), |acc, v| self.zip(&acc, &v, |a, b| a + b))
    }

    fn push_case<T: PartialEq>(&self, out: &mut Cond<T>, g: F, v: T) {
        match out.cases.iter_mut().find(|(_, x)| *x == v) {
            Some((existing, _)) => *existing = self.or(*existing, g),
            None => out.cases.push((g, v)),
        }
    }
}

/// One variable per open decision, whose values are its live options. Outer decisions come
/// first, so formulas read top-down and a canonical form doesn't depend on naming.
fn world_space(s: &Store) -> Vec<Var> {
    fn depth(s: &Store, id: &str, memo: &mut BTreeMap<String, usize>) -> usize {
        if let Some(&n) = memo.get(id) {
            return n;
        }
        // Set first, so a cycle from a hand edit ends here. It's reported by check.
        memo.insert(id.to_string(), 0);
        let n = s.parents(id).iter().map(|p| stacker::maybe_grow(64 * 1024, 1024 * 1024, || depth(s, p, memo)) + 1).max().unwrap_or(0);
        memo.insert(id.to_string(), n);
        n
    }
    let mut memo = BTreeMap::new();
    let mut open = s.open_decisions();
    open.sort_by_key(|id| (depth(s, id, &mut memo), id.to_string()));
    open.into_iter()
        .filter_map(|id| {
            let opts = s.options(id);
            if opts.is_empty() {
                return None;
            }
            let unknown: Vec<bool> = opts.iter().map(|(o, _)| s.has(o, Unknown::NAME)).collect();
            // A gap has no likelihood of its own unless someone gives it one.
            let raw: Vec<f64> = opts.iter().zip(&unknown).map(|((_, v), u)| v.get("weight").and_then(|w| w.as_f64()).unwrap_or(if *u { 0.0 } else { 1.0 })).collect();
            let total: f64 = raw.iter().sum();
            let weights = raw.iter().map(|w| if total > 0.0 { w / total } else { 1.0 / raw.len() as f64 }).collect();
            Some(Var { name: id.to_string(), options: opts.iter().map(|(o, _)| o.to_string()).collect(), weights, unknown })
        })
        .collect()
}

/// The results of a run. Everything here can be deleted and recomputed from the store.
#[derive(Debug, Default, Clone)]
pub struct Derived {
    pub mdd: Mdd,
    /// Cells whose value is a yes/no formula.
    pub cells: BTreeMap<String, BTreeMap<Id, F>>,
    /// Every other cell, as JSON.
    pub values: BTreeMap<String, BTreeMap<Id, Cond<Value>>>,
    pub notices: Vec<Notice>,
    /// What each cell read while computing.
    pub trace: BTreeMap<Key, BTreeSet<Dep>>,
    /// Cells that depend on themselves. Each took its fallback value at the point of the cycle.
    pub cycles: Vec<Vec<Key>>,
    /// Values that don't match their type, which compute treated as absent.
    pub malformed: Vec<Violation>,
    memo: HashMap<Key, Rc<dyn Any>>,
}

impl Derived {
    /// A yes/no cell's formula. `FALSE` if it has no value for that node; asking for a cell
    /// that doesn't exist is a mistake, not an answer of "never".
    pub fn get(&self, cell: &str, id: &str) -> F {
        let Some(c) = self.cells.get(cell) else { panic!("no yes/no cell named {cell}") };
        c.get(id).copied().unwrap_or(FALSE)
    }

    /// A cell's value, typed.
    pub fn of<C: Cell>(&self, id: &str) -> Option<&C::Value> {
        self.memo.get(&(C::NAME.to_string(), id.to_string()))?.downcast_ref()
    }

    pub fn show(&self, cell: &str, id: &str) -> String {
        self.mdd.show(self.get(cell, id))
    }

    pub fn prob(&self, cell: &str, id: &str) -> f64 {
        self.mdd.prob(self.get(cell, id))
    }

    /// In every world.
    pub fn holds(&self, cell: &str, id: &str) -> bool {
        self.get(cell, id) == TRUE
    }

    /// In every world where each open decision lands on an option someone has actually named.
    pub fn holds_among_known(&self, cell: &str, id: &str) -> bool {
        self.mdd.holds_among_known(self.get(cell, id))
    }

    pub fn possible(&self, cell: &str, id: &str) -> bool {
        self.get(cell, id) != FALSE
    }

    pub fn ids_where(&self, cell: &str, f: impl Fn(F) -> bool) -> Vec<&str> {
        self.cells.get(cell).into_iter().flatten().filter(|(_, v)| f(**v)).map(|(k, _)| k.as_str()).collect()
    }
}

pub fn run(s: &Store, reg: &Registry) -> Derived {
    let (view, mut malformed) = reg.schema.normalise(s);
    let cx = Cx::new(&view, reg);
    let mut cells: BTreeMap<String, BTreeMap<Id, F>> = BTreeMap::new();
    let mut values: BTreeMap<String, BTreeMap<Id, Cond<Value>>> = BTreeMap::new();
    for (name, (eval, domain)) in &reg.cells {
        for id in domain(&cx) {
            match eval(&cx, &id) {
                Ok(f) => {
                    cells.entry(name.clone()).or_default().insert(id, f);
                }
                Err(v) => {
                    values.entry(name.clone()).or_default().insert(id, v);
                }
            }
        }
        // A cell with an empty domain still exists.
        if !values.contains_key(name) {
            cells.entry(name.clone()).or_default();
        }
    }
    let notices = reg.checks.iter().flat_map(|c| c(&cx)).collect();
    malformed.extend(cx.undecodable.into_inner());
    Derived { mdd: cx.mdd.into_inner(), cells, values, notices, trace: cx.trace.into_inner(), cycles: cx.cycles.into_inner(), malformed, memo: cx.memo.into_inner() }
}
