//! The core: entities, which are nodes and the edges between them, each with components, and the cells
//! those components bring. A cell holds a value or a formula, or takes it from a kind, or from what its
//! component says it is when nothing says otherwise. See `docs/components.md`.
//!
//! Computed cells run lazily, when they're read, and what they give is memoised across edits. What a
//! computation actually read is recorded, and that's what invalidation runs on. This module knows
//! nothing about worlds or formulas.
//!
//! A `Graph` is `Send + Sync`: reading takes `&self`, and writing takes `&mut self`, so shared use is an
//! `RwLock<Graph>`. Reads currently take turns on one lock while cells are worked out. That's an
//! implementation detail the API doesn't expose.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::name::Name;
use crate::registry::{Default as CellDefault, Place, Registry};
use crate::table::{self, Row};

/// A node or an edge. An opaque handle with no meaning of its own: a name is just a cell. A node's is
/// time-ordered, so nodes sort in creation order, and files created on two machines never collide. An
/// edge's is worked out from its ends, so it's the same whenever those two are linked.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entity(Uuid);

/// Two hashes of some text make up the 128 bits, so a handle worked out this way can't be mistaken for
/// one that was made.
fn hashed(text: &str) -> Entity {
    let fnv = |seed: u64| text.bytes().fold(seed, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
    Entity(Uuid::from_u128((u128::from(fnv(0xcbf29ce484222325)) << 64) | u128::from(fnv(0x84222325cbf29ce4))))
}

impl Entity {
    pub fn new() -> Self {
        Entity(Uuid::now_v7())
    }

    /// Where cells about the graph as a whole live, so they're worked out once however many nodes read
    /// them. It isn't a node: it has no edges and nothing can be written on it.
    pub fn whole() -> Self {
        Entity(Uuid::nil())
    }

    /// The edge from one node to another.
    pub fn edge(from: Entity, to: Entity) -> Self {
        hashed(&format!("edge {from} {to}"))
    }

    /// A row nobody wrote: see `Graph::row_at`.
    fn implicit(table: &str, n: i64) -> Self {
        hashed(&format!("{table}[{n}]"))
    }

    /// A node nobody wrote that's only what these cells say: see `Graph::constant`.
    fn constant(cells: &BTreeMap<String, Value>) -> Self {
        hashed(&format!("constant {}", serde_json::to_string(cells).expect("values serialise")))
    }
}

/// A node nobody wrote, which only exists to be read: see `Graph::row_at` and `Graph::constant`.
#[derive(Clone)]
enum Implicit {
    Row(Row),
    Constant(BTreeMap<String, Value>),
}

impl Default for Entity {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Entity {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Entity)
    }
}

/// One cell: an entity and the cell's name.
pub type Key = (Entity, String);

/// A cell's value as a Rust type. JSON under a name is what a cell really holds, and this is a typed
/// way to read one. Define one with `#[cell]`.
pub trait CellType: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
}

/// A component's name as a Rust type, for asking whether something has it. Define one with
/// `#[component]`.
pub trait ComponentType {
    const NAME: &'static str;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// These cells depend on each other.
    Cycle(Vec<Key>),
    /// Something needed a cell that isn't there.
    Missing(Key),
    Malformed { cell: Key, detail: String },
    /// A computation's own failure, like #DIV/0!, or a formula that can't be read.
    Failed(String),
}

/// What reading a cell gives. `None` means the entity doesn't have it.
pub type Cell<C> = Result<Option<C>, Error>;

type AnyValue = Arc<dyn Any + Send + Sync>;

fn panic_message(p: Box<dyn Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown".into())
}

/// Something that works a cell out: a formula, compiled, or something built in. What it gives is kept
/// as its own Rust type, so reading it typed needs no JSON.
pub struct Computed {
    pub name: String,
    eval: Box<dyn Fn(&Cx) -> AnyValue + Send + Sync>,
    same: fn(&AnyValue, &AnyValue) -> bool,
    json: fn(&AnyValue) -> Result<Option<Value>, Error>,
}

impl Computed {
    fn erased<T: Clone + PartialEq + Send + Sync + 'static>(name: &str, f: impl Fn(&Cx) -> Result<Option<T>, Error> + Send + Sync + 'static, json: fn(&AnyValue) -> Result<Option<Value>, Error>) -> Arc<Computed> {
        let label = name.to_string();
        Arc::new(Computed {
            name: name.to_string(),
            eval: Box::new(move |cx| {
                // A computation that panics fails its own cell, not the graph and every thread reading it.
                let result = catch_unwind(AssertUnwindSafe(|| f(cx))).unwrap_or_else(|p| Err(Error::Failed(format!("{label} panicked: {}", panic_message(p)))));
                Arc::new(result) as AnyValue
            }),
            same: |a, b| a.downcast_ref::<Result<Option<T>, Error>>() == b.downcast_ref::<Result<Option<T>, Error>>(),
            json,
        })
    }

    /// One that gives a typed cell.
    pub fn typed<C: CellType>(name: &str, f: impl Fn(&Cx) -> Cell<C> + Send + Sync + 'static) -> Arc<Computed> {
        Self::erased(name, f, |a| a.downcast_ref::<Cell<C>>().expect("one cell, one type").clone().map(|v| v.map(|v| serde_json::to_value(&v).expect("cells serialise"))))
    }

    /// One that gives JSON, for a cell with no Rust type.
    pub fn json(name: &str, f: impl Fn(&Cx) -> Cell<Value> + Send + Sync + 'static) -> Arc<Computed> {
        Self::erased(name, f, |a| a.downcast_ref::<Cell<Value>>().expect("one cell, one type").clone())
    }
}

/// What a cell holds where it's written.
#[derive(Clone)]
pub(crate) enum Backing {
    Value(Value),
    /// A formula, and what it compiled to, or why it couldn't.
    Formula(String, Result<Arc<Computed>, Error>),
    Builtin(Arc<Computed>),
}

impl Backing {
    fn computed(&self) -> Option<&Arc<Computed>> {
        match self {
            Backing::Formula(_, Ok(c)) | Backing::Builtin(c) => Some(c),
            _ => None,
        }
    }

    fn same(&self, other: &Backing) -> bool {
        match (self, other) {
            (Backing::Value(a), Backing::Value(b)) => a == b,
            (Backing::Formula(a, _), Backing::Formula(b, _)) => a == b,
            (Backing::Builtin(a), Backing::Builtin(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// How a cell is backed, as someone reading it would say it.
#[derive(Debug, Clone, PartialEq)]
pub enum Backed {
    Value(Value),
    Formula(String),
    /// Worked out by something built in, which has no written form.
    Builtin,
}

impl From<&Backing> for Backed {
    fn from(b: &Backing) -> Self {
        match b {
            Backing::Value(v) => Backed::Value(v.clone()),
            Backing::Formula(t, _) => Backed::Formula(t.clone()),
            Backing::Builtin(_) => Backed::Builtin,
        }
    }
}

/// Where a cell's backing comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The entity says it itself.
    Own,
    /// It's taken from this kind, or table, or other part of the same row.
    Kind(Entity),
    /// It's what its component says it is when nothing else does.
    Default,
}

/// Why an entity has a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// It was given it.
    Given,
    /// Every node has it.
    Everywhere,
    /// It takes it from this kind.
    Kind(Entity),
    /// Another component it has requires it.
    Requires(String),
    /// An edge to or from it brings it.
    Edge(Entity),
}

/// One thing a computed cell read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dep {
    /// Another cell's value.
    Cell(Key),
    /// How this cell is backed, and whether it's there at all.
    Backing(Key),
    /// Which components an entity has.
    Has(Entity),
    /// Edges with this component arriving at a node.
    Into(Entity, String),
    /// Edges with this component leaving a node.
    From(Entity, String),
    /// Which nodes have this component.
    Scan(String),
    /// Every node's name, which says what's a row of what.
    Names,
}

#[derive(Clone)]
struct Memo {
    value: AnyValue,
    deps: Vec<Dep>,
    /// The revision this was last confirmed correct at.
    verified: u64,
    /// The revision its value last actually changed at.
    changed: u64,
}

#[derive(Default)]
struct Engine {
    memo: HashMap<Key, Memo>,
    evaluating: Vec<Key>,
    verifying: HashSet<Key>,
    reads: Vec<Vec<Dep>>,
    ran: Vec<Key>,
}

/// Every node's name, and what's a row of what, so finding a table's rows doesn't mean reading every name.
#[derive(Clone, Default)]
struct Names {
    of: HashMap<Entity, String>,
    named: HashMap<String, BTreeSet<Entity>>,
    rows: HashMap<Entity, Row>,
    tables: HashMap<String, BTreeMap<table::Key, BTreeSet<Entity>>>,
}

impl Names {
    fn set(&mut self, e: Entity, name: Option<String>) {
        if let Some(old) = self.of.remove(&e) {
            self.named.get_mut(&old).map(|s| s.remove(&e));
        }
        if let Some(row) = self.rows.remove(&e) {
            self.tables.get_mut(&row.table).and_then(|t| t.get_mut(&row.key)).map(|s| s.remove(&e));
        }
        let Some(name) = name else { return };
        if let Some(row) = table::row_of(&name) {
            self.tables.entry(row.table.clone()).or_default().entry(row.key.clone()).or_default().insert(e);
            self.rows.insert(e, row);
        }
        self.named.entry(name.clone()).or_default().insert(e);
        self.of.insert(e, name);
    }
}

/// What an entity's components and cells are, to compare before and after a change that can move them.
#[derive(Default)]
struct Shape {
    components: BTreeSet<String>,
    cells: BTreeMap<String, (Backing, Entity)>,
}

/// How far past its last written row a table goes on by itself.
pub const MOST_IMPLICIT: i64 = 100_000;

type Components = Arc<BTreeMap<String, Origin>>;

#[derive(Default)]
pub struct Graph {
    nodes: BTreeSet<Entity>,
    /// Each edge, with its ends. An edge is there as long as it has a component.
    edges: BTreeMap<Entity, (Entity, Entity)>,
    out: HashMap<Entity, BTreeSet<Entity>>,
    into: HashMap<Entity, BTreeSet<Entity>>,
    /// The components each entity was given.
    given: HashMap<Entity, BTreeSet<String>>,
    /// What each entity says itself.
    cells: HashMap<Entity, BTreeMap<String, Backing>>,
    /// What each cell is when nothing says otherwise.
    defaults: Arc<HashMap<String, Backing>>,
    /// What a cell is instead, on an entity with a component that fills it: see `Registry::fill`.
    fills: Arc<HashMap<String, Vec<(String, Backing)>>>,
    /// Names, and the tables they make: see `table`.
    names: Names,
    /// Nodes nobody wrote that something has asked for: numbered rows (see `row_at`), and constants (see
    /// `constant`). They're only ever read, so they live here and in the memos, and never in a file.
    implicit: Mutex<HashMap<Entity, Implicit>>,
    /// Which components each entity has, as worked out since the last change that could move them.
    components: Mutex<HashMap<Entity, Components>>,
    resources: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    pub(crate) registry: Arc<Registry>,
    revision: u64,
    last_change: HashMap<Dep, u64>,
    engine: Mutex<RefCell<Engine>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Graph {
    /// A graph with nothing registered.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(Registry::new()))
    }

    /// A graph of what these systems say there is.
    pub fn with_registry(registry: Arc<Registry>) -> Self {
        let backing = |name: &str, d: &CellDefault| match d {
            CellDefault::Value(v) => Backing::Value(v.clone()),
            CellDefault::Builtin(f) => Backing::Builtin(f.clone()),
            CellDefault::Formula(text) => {
                let compiled = match registry.cell_def(name).and_then(|c| c.compile.as_ref()) {
                    Some(compile) => compile(text).map_err(Error::Failed),
                    None => Err(Error::Failed(format!("{name} has a formula for a default, and nothing to compile it"))),
                };
                Backing::Formula(text.clone(), compiled)
            }
        };
        let mut defaults = HashMap::new();
        for c in registry.cells() {
            if let Some(d) = &c.default {
                defaults.insert(c.name.clone(), backing(&c.name, d));
            }
        }
        let mut fills: HashMap<String, Vec<(String, Backing)>> = HashMap::new();
        for comp in registry.components() {
            for (cell, d) in &comp.fills {
                let owner = registry.owner(cell).unwrap_or_else(|| panic!("{} fills {cell}, which no component brings", comp.name));
                assert!(registry.required_by(&comp.name).iter().any(|r| r == owner), "{} fills {cell}, a cell of {owner}, which it doesn't require", comp.name);
                fills.entry(cell.clone()).or_default().push((comp.name.clone(), backing(cell, d)));
            }
        }
        Graph { defaults: Arc::new(defaults), fills: Arc::new(fills), registry, revision: 1, ..Self::default() }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    fn touch(&mut self, dep: Dep) {
        self.last_change.insert(dep, self.revision);
    }

    // ---------- writing ----------

    /// A new node.
    pub fn spawn(&mut self) -> Entity {
        let e = Entity::new();
        self.adopt(e);
        e
    }

    /// A node that already has an identity, such as one loaded from a file.
    pub fn adopt(&mut self, e: Entity) -> &mut Self {
        if !self.nodes.contains(&e) {
            self.change(&[e], |g| {
                g.nodes.insert(e);
            });
        }
        self
    }

    /// Remove a node, and everything it says. Its edges have to go first, because they say something
    /// about other nodes too. Removing a node with edges, or no such node, changes nothing and returns
    /// false.
    pub fn despawn(&mut self, e: Entity) -> bool {
        let linked = self.out.get(&e).is_some_and(|s| !s.is_empty()) || self.into.get(&e).is_some_and(|s| !s.is_empty());
        if linked || !self.nodes.contains(&e) {
            return false;
        }
        self.change(&[e], |g| {
            g.nodes.remove(&e);
            g.names.set(e, None);
            g.given.remove(&e);
            g.cells.remove(&e);
            g.out.remove(&e);
            g.into.remove(&e);
        });
        true
    }

    fn known(&self, e: Entity) {
        assert!(self.nodes.contains(&e) || self.edges.contains_key(&e), "{e} isn't a node or an edge in this graph");
    }

    /// Give a node a component. Giving it one it has already changes nothing.
    pub fn add(&mut self, e: Entity, component: &str) -> &mut Self {
        assert!(self.nodes.contains(&e), "{e} isn't a node in this graph");
        if !self.given.get(&e).is_some_and(|g| g.contains(component)) {
            self.change(&[e], |g| {
                g.given.entry(e).or_default().insert(component.to_string());
            });
        }
        self
    }

    /// Take a component away from a node. What it said in that component's cells is kept, though it
    /// isn't there while the node doesn't have the component.
    pub fn remove(&mut self, e: Entity, component: &str) -> &mut Self {
        if self.given.get(&e).is_some_and(|g| g.contains(component)) {
            self.change(&[e], |g| {
                g.given.get_mut(&e).map(|s| s.remove(component));
            });
        }
        self
    }

    /// Give the edge from one node to another a component, making the edge if it's new. Returns the edge.
    pub fn link(&mut self, from: Entity, to: Entity, component: &str) -> Entity {
        assert!(self.nodes.contains(&from) && self.nodes.contains(&to), "an edge needs both its ends");
        let edge = Entity::edge(from, to);
        if self.given.get(&edge).is_some_and(|g| g.contains(component)) {
            return edge;
        }
        self.change(&[edge, from, to], |g| {
            g.edges.insert(edge, (from, to));
            g.out.entry(from).or_default().insert(edge);
            g.into.entry(to).or_default().insert(edge);
            g.given.entry(edge).or_default().insert(component.to_string());
        });
        self.touch(Dep::Into(to, component.to_string()));
        self.touch(Dep::From(from, component.to_string()));
        edge
    }

    /// Take a component off the edge from one node to another. An edge is its components, so taking the
    /// last one takes the edge, and everything it says.
    pub fn unlink(&mut self, from: Entity, to: Entity, component: &str) -> &mut Self {
        let edge = Entity::edge(from, to);
        if !self.given.get(&edge).is_some_and(|g| g.contains(component)) {
            return self;
        }
        self.change(&[edge, from, to], |g| {
            let left = g.given.get_mut(&edge).expect("checked above");
            left.remove(component);
            if left.is_empty() {
                g.given.remove(&edge);
                g.cells.remove(&edge);
                g.edges.remove(&edge);
                g.out.get_mut(&from).map(|s| s.remove(&edge));
                g.into.get_mut(&to).map(|s| s.remove(&edge));
            }
        });
        self.touch(Dep::Into(to, component.to_string()));
        self.touch(Dep::From(from, component.to_string()));
        self
    }

    /// Write a value in a cell. Writing the value already there changes nothing.
    pub fn set<C: CellType>(&mut self, e: Entity, value: C) -> &mut Self {
        self.set_value(e, C::NAME, serde_json::to_value(&value).expect("cells serialise"))
    }

    /// The same for a cell given by name.
    pub fn set_value(&mut self, e: Entity, cell: &str, value: Value) -> &mut Self {
        self.known(e);
        self.back(e, cell, Some(Backing::Value(value)));
        self
    }

    /// Write a formula in a cell. If it can't be compiled, the cell says why.
    pub fn set_formula(&mut self, e: Entity, cell: &str, text: &str) -> &mut Self {
        self.known(e);
        let compiled = self.compile(cell, text);
        self.back(e, cell, Some(Backing::Formula(text.to_string(), compiled)));
        self
    }

    /// Take away what an entity says in a cell, so it takes it from its kind, or its default, again.
    pub fn clear(&mut self, e: Entity, cell: &str) -> &mut Self {
        if self.cells.get(&e).is_some_and(|c| c.contains_key(cell)) {
            self.back(e, cell, None);
        }
        self
    }

    pub(crate) fn compile(&self, cell: &str, text: &str) -> Result<Arc<Computed>, Error> {
        let compile = match self.registry.cell_def(cell) {
            Some(def) => def.compile.as_ref().ok_or_else(|| Error::Failed(format!("{cell} can't hold a formula")))?,
            None => self.registry.free().ok_or_else(|| Error::Failed("nothing here can read a formula".to_string()))?,
        };
        compile(text).map_err(Error::Failed)
    }

    fn back(&mut self, e: Entity, cell: &str, b: Option<Backing>) {
        let old = self.cells.get(&e).and_then(|c| c.get(cell));
        let unchanged = match (&b, old) {
            (Some(new), Some(old)) => new.same(old),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.change(&[e], |g| {
            match b {
                Some(b) => {
                    g.cells.entry(e).or_default().insert(cell.to_string(), b);
                }
                None => {
                    g.cells.get_mut(&e).map(|c| c.remove(cell));
                }
            }
            if cell == Name::NAME {
                let name = match g.cells.get(&e).and_then(|c| c.get(cell)) {
                    Some(Backing::Value(Value::String(s))) => Some(s.clone()),
                    _ => None,
                };
                g.names.set(e, name);
            }
        });
        if cell == Name::NAME {
            self.touch(Dep::Names);
        }
    }

    /// Make a change that can move what entities around these have, and tell whatever read them.
    fn change(&mut self, around: &[Entity], f: impl FnOnce(&mut Self)) {
        let before: HashMap<Entity, Shape> = self.region(around).into_iter().map(|e| (e, self.shape(e))).collect();
        f(self);
        self.revision += 1;
        lock(&self.components).clear();
        let mut region = self.region(around);
        region.extend(before.keys().copied());
        let mut before = before;
        for e in region {
            let was = before.remove(&e).unwrap_or_default();
            let is = self.shape(e);
            if was.components != is.components {
                self.touch(Dep::Has(e));
                for c in was.components.symmetric_difference(&is.components) {
                    self.touch(Dep::Scan(c.clone()));
                }
            }
            let names: BTreeSet<&String> = was.cells.keys().chain(is.cells.keys()).collect();
            for cell in names {
                let (old, new) = (was.cells.get(cell), is.cells.get(cell));
                let same = match (old, new) {
                    (Some((a, at)), Some((b, bt))) => at == bt && a.same(b),
                    (None, None) => true,
                    _ => false,
                };
                if same {
                    continue;
                }
                // A memo only stands for an unbroken run of being computed. If readers have seen a value
                // written in, or nothing at all, since, the same answer coming back is still a change to them.
                let both_computed = old.is_some_and(|(b, _)| b.computed().is_some()) && new.is_some_and(|(b, _)| b.computed().is_some());
                if !both_computed {
                    self.forget(e, cell);
                }
                self.touch(Dep::Backing((e, cell.clone())));
            }
        }
    }

    /// These entities, and everything that might take something from them.
    fn region(&self, around: &[Entity]) -> BTreeSet<Entity> {
        let mut out = BTreeSet::new();
        for a in around {
            out.insert(*a);
            if self.nodes.contains(a) {
                out.extend(self.takers_all(*a));
            }
        }
        out
    }

    fn shape(&self, e: Entity) -> Shape {
        if !self.exists(e) {
            return Shape::default();
        }
        let components: BTreeSet<String> = self.components_of(e).keys().cloned().collect();
        let cells = self.cell_names(e).into_iter().filter_map(|c| self.backing_at(e, &c).map(|b| (c, b))).collect();
        Shape { components, cells }
    }

    fn forget(&mut self, e: Entity, cell: &str) {
        self.engine.get_mut().unwrap_or_else(PoisonError::into_inner).get_mut().memo.remove(&(e, cell.to_string()));
    }

    /// Something shared by computations that isn't part of the graph.
    pub fn insert_resource<R: Send + Sync + 'static>(&mut self, r: R) -> &mut Self {
        self.resources.insert(TypeId::of::<R>(), Arc::new(r));
        self
    }

    pub fn resource<R: Send + Sync + 'static>(&self) -> Arc<R> {
        let r = self.resources.get(&TypeId::of::<R>()).unwrap_or_else(|| panic!("no such resource")).clone();
        r.downcast::<R>().expect("keyed by type")
    }

    /// An independent copy to try things on. It keeps the memos, so it only reruns what the changes reach.
    pub fn fork(&self) -> Graph {
        let memo = self.session(|s| s.engine.borrow().memo.clone());
        Graph {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
            out: self.out.clone(),
            into: self.into.clone(),
            given: self.given.clone(),
            cells: self.cells.clone(),
            defaults: self.defaults.clone(),
            fills: self.fills.clone(),
            names: self.names.clone(),
            implicit: Mutex::new(lock(&self.implicit).clone()),
            components: Mutex::new(lock(&self.components).clone()),
            resources: self.resources.clone(),
            registry: self.registry.clone(),
            revision: self.revision,
            last_change: self.last_change.clone(),
            engine: Mutex::new(RefCell::new(Engine { memo, ..Engine::default() })),
        }
    }

    // ---------- reading ----------

    fn session<R>(&self, f: impl FnOnce(Session) -> R) -> R {
        let guard = lock(&self.engine);
        f(Session { g: self, engine: &guard })
    }

    /// Every node, in the order they were made.
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.nodes.iter().copied()
    }

    /// Every edge, with its ends, in the order of their sources and then their targets.
    pub fn edges(&self) -> impl Iterator<Item = (Entity, Entity, Entity)> + '_ {
        let mut all: Vec<(Entity, Entity, Entity)> = self.edges.iter().map(|(e, (f, t))| (*f, *t, *e)).collect();
        all.sort();
        all.into_iter().map(|(f, t, e)| (e, f, t))
    }

    /// Whether this is a node of the graph, rather than an edge, a row nobody wrote or a handle from nowhere.
    pub fn contains(&self, e: Entity) -> bool {
        self.nodes.contains(&e)
    }

    pub fn is_edge(&self, e: Entity) -> bool {
        self.edges.contains_key(&e)
    }

    /// An edge's source and target.
    pub fn ends(&self, edge: Entity) -> Option<(Entity, Entity)> {
        self.edges.get(&edge).copied()
    }

    fn exists(&self, e: Entity) -> bool {
        e == Entity::whole() || self.nodes.contains(&e) || self.edges.contains_key(&e) || self.is_implicit(e)
    }

    /// The components an entity was given itself.
    pub fn given(&self, e: Entity) -> BTreeSet<String> {
        self.given.get(&e).cloned().unwrap_or_default()
    }

    /// The edges with this component arriving at a node, each with its source, in the order those were
    /// made.
    pub fn edges_into(&self, e: Entity, component: &str) -> Vec<(Entity, Entity)> {
        let edges = self.into.get(&e).into_iter().flatten();
        let mut out: Vec<(Entity, Entity)> = edges.filter(|x| self.given.get(x).is_some_and(|g| g.contains(component))).map(|x| (self.edges[x].0, *x)).collect();
        out.sort();
        out
    }

    /// The edges with this component leaving a node, each with its target, in the order those were made.
    pub fn edges_from(&self, e: Entity, component: &str) -> Vec<(Entity, Entity)> {
        let edges = self.out.get(&e).into_iter().flatten();
        let mut out: Vec<(Entity, Entity)> = edges.filter(|x| self.given.get(x).is_some_and(|g| g.contains(component))).map(|x| (self.edges[x].1, *x)).collect();
        out.sort();
        out
    }

    /// The nodes linking to this one by an edge with this component.
    pub fn sources_of(&self, e: Entity, component: &str) -> Vec<Entity> {
        self.edges_into(e, component).into_iter().map(|(n, _)| n).collect()
    }

    /// The nodes this one links to by an edge with this component.
    pub fn targets_of(&self, e: Entity, component: &str) -> Vec<Entity> {
        self.edges_from(e, component).into_iter().map(|(n, _)| n).collect()
    }

    /// Whether the edge from one node to another has this component.
    pub fn linked(&self, from: Entity, to: Entity, component: &str) -> bool {
        self.given.get(&Entity::edge(from, to)).is_some_and(|g| g.contains(component))
    }

    /// The components an entity has, each with why.
    pub fn components_of(&self, e: Entity) -> Components {
        if let Some(c) = lock(&self.components).get(&e) {
            return c.clone();
        }
        let worked_out = Arc::new(self.work_out_components(e));
        lock(&self.components).insert(e, worked_out.clone());
        worked_out
    }

    fn work_out_components(&self, e: Entity) -> BTreeMap<String, Origin> {
        let reg = &self.registry;
        // A component given somewhere it doesn't go is kept, and a check says so, but it isn't there.
        let fits = |c: &String, place: Place| reg.component_def(c).is_none_or(|d| d.place == place);
        let mut out: BTreeMap<String, Origin> = BTreeMap::new();
        if e == Entity::whole() {
            for c in reg.components().filter(|c| c.place == Place::Whole) {
                out.insert(c.name.clone(), Origin::Everywhere);
            }
        } else if self.edges.contains_key(&e) {
            for c in self.given.get(&e).into_iter().flatten().filter(|c| fits(c, Place::Edge)) {
                out.insert(c.clone(), Origin::Given);
            }
        } else if let Some(Implicit::Constant(cells)) = self.implicit_of(e) {
            // It has what its cells need, and nothing else.
            for c in cells.keys().filter_map(|c| reg.owner(c)) {
                out.insert(c.to_string(), Origin::Given);
            }
        } else if self.nodes.contains(&e) || self.is_implicit(e) {
            if self.nodes.contains(&e) {
                for c in reg.components().filter(|c| c.everywhere && c.place == Place::Node) {
                    out.insert(c.name.clone(), Origin::Everywhere);
                }
                for c in self.given.get(&e).into_iter().flatten().filter(|c| fits(c, Place::Node)) {
                    out.insert(c.clone(), Origin::Given);
                }
            }
            for k in self.lookup(e) {
                for c in self.given.get(&k).into_iter().flatten().filter(|c| reg.inheritable(c)) {
                    out.entry(c.clone()).or_insert(Origin::Kind(k));
                }
            }
            for edge in self.out.get(&e).into_iter().flatten() {
                for c in self.given.get(edge).into_iter().flatten().filter_map(|c| reg.component_def(c)) {
                    for s in &c.source {
                        out.entry(s.clone()).or_insert(Origin::Edge(*edge));
                    }
                }
            }
            for edge in self.into.get(&e).into_iter().flatten() {
                for c in self.given.get(edge).into_iter().flatten().filter_map(|c| reg.component_def(c)) {
                    for t in &c.target {
                        out.entry(t.clone()).or_insert(Origin::Edge(*edge));
                    }
                }
            }
        }
        let mut todo: Vec<String> = out.keys().cloned().collect();
        while let Some(c) = todo.pop() {
            for r in reg.component_def(&c).map(|d| d.requires.clone()).unwrap_or_default() {
                if !out.contains_key(&r) {
                    out.insert(r.clone(), Origin::Requires(c.clone()));
                    todo.push(r);
                }
            }
        }
        out
    }

    /// Whether an entity has a component, however it came to.
    pub fn has(&self, e: Entity, component: &str) -> bool {
        self.components_of(e).contains_key(component)
    }

    /// The nodes with a component.
    pub fn having(&self, component: &str) -> Vec<Entity> {
        self.nodes.iter().copied().filter(|e| self.has(*e, component)).collect()
    }

    /// The cells an entity could have: what it says itself, what its kinds say, and every cell of its
    /// components. Some of these may turn out to have nothing in them.
    fn cell_names(&self, e: Entity) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self.cells.get(&e).into_iter().flat_map(|c| c.keys().cloned()).collect();
        if let Some(Implicit::Constant(cells)) = self.implicit_of(e) {
            out.extend(cells.into_keys());
        }
        for k in self.lookup(e) {
            out.extend(self.cells.get(&k).into_iter().flat_map(|c| c.keys().filter(|c| self.registry.cell_inheritable(c)).cloned()));
        }
        for c in self.components_of(e).keys() {
            if let Some(def) = self.registry.component_def(c) {
                out.extend(def.cells.iter().cloned());
                out.extend(def.checks.iter().map(|k| k.cell()));
            }
        }
        out
    }

    /// The cells an entity has: there's something in each, whichever way it comes.
    pub fn cells_of(&self, e: Entity) -> BTreeSet<String> {
        self.cell_names(e).into_iter().filter(|c| self.backing_at(e, c).is_some()).collect()
    }

    /// What backs a cell, and the entity it's written on: this one, unless it's taken from a kind, or
    /// the graph as a whole for a default.
    fn backing_at(&self, e: Entity, cell: &str) -> Option<(Backing, Entity)> {
        if !self.exists(e) {
            return None;
        }
        let owner = self.registry.owner(cell);
        if let Some(o) = owner
            && !self.has(e, o)
        {
            return None;
        }
        if let Some(b) = self.cells.get(&e).and_then(|c| c.get(cell)) {
            return Some((b.clone(), e));
        }
        if let Some(Implicit::Constant(cells)) = self.implicit_of(e) {
            return cells.get(cell).map(|v| (Backing::Value(v.clone()), e));
        }
        if self.registry.cell_inheritable(cell)
            && let Some(found) = self.taken_from_kind(e, cell)
        {
            return Some(found);
        }
        if let Some(fills) = self.fills.get(cell) {
            let has = self.components_of(e);
            if let Some((_, b)) = fills.iter().find(|(c, _)| has.contains_key(c)) {
                return Some((b.clone(), Entity::whole()));
            }
        }
        owner.and_then(|_| self.defaults.get(cell)).map(|b| (b.clone(), Entity::whole()))
    }

    /// The nearest of its kinds that says what this cell is: see `lookup`.
    fn taken_from_kind(&self, e: Entity, cell: &str) -> Option<(Backing, Entity)> {
        self.lookup(e).into_iter().find_map(|k| self.cells.get(&k).and_then(|c| c.get(cell)).map(|b| (b.clone(), k)))
    }

    /// How a cell is backed, if it's there, and where that comes from.
    pub fn backed(&self, e: Entity, cell: &str) -> Option<(Backed, Source)> {
        let (b, at) = self.backing_at(e, cell)?;
        let source = if at == e {
            Source::Own
        } else if at == Entity::whole() {
            Source::Default
        } else {
            Source::Kind(at)
        };
        Some(((&b).into(), source))
    }

    /// The value written on this very entity, if that's what's there.
    pub fn stored(&self, e: Entity, cell: &str) -> Option<&Value> {
        match self.cells.get(&e)?.get(cell)? {
            Backing::Value(v) => Some(v),
            _ => None,
        }
    }

    /// What this entity says itself in each of its cells, whether or not it has their components now.
    pub fn written(&self, e: Entity) -> BTreeMap<String, Backed> {
        self.cells.get(&e).into_iter().flatten().map(|(c, b)| (c.clone(), b.into())).collect()
    }

    /// Why a formula this entity says itself can't be read, if it can't.
    pub fn formula_error(&self, e: Entity, cell: &str) -> Option<Error> {
        match self.cells.get(&e)?.get(cell)? {
            Backing::Formula(_, Err(error)) => Some(error.clone()),
            _ => None,
        }
    }

    /// Every formula written anywhere that can't be read, and why.
    pub fn broken(&self) -> Vec<(Key, Error)> {
        let mut out = Vec::new();
        for (e, cells) in &self.cells {
            for (cell, b) in cells {
                if let Backing::Formula(_, Err(error)) = b {
                    out.push(((*e, cell.clone()), error.clone()));
                }
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Whether this entity says what's in a cell itself, rather than taking it from elsewhere.
    pub fn has_own(&self, e: Entity, cell: &str) -> bool {
        self.cells.get(&e).is_some_and(|c| c.contains_key(cell))
    }

    /// The kind that says what this cell is, whether or not the entity says otherwise.
    pub fn kind_beneath(&self, e: Entity, cell: &str) -> Option<Entity> {
        self.registry.cell_inheritable(cell).then(|| self.taken_from_kind(e, cell).map(|(_, k)| k)).flatten()
    }

    /// The typed way to read a cell.
    pub fn get<C: CellType>(&self, e: Entity) -> Cell<C> {
        self.session(|s| s.read(e))
    }

    /// Any cell by its name, whether or not Rust has a type for it.
    pub fn get_value(&self, e: Entity, cell: &str) -> Cell<Value> {
        self.session(|s| s.read_value(e, cell))
    }

    /// Run something with what a computed cell on this entity could see, as one would, without keeping
    /// anything: to try out a formula, say.
    pub fn run<R>(&self, e: Entity, f: impl FnOnce(&Cx) -> R) -> R {
        self.session(|s| {
            s.engine.borrow_mut().reads.push(Vec::new());
            let out = f(&Cx { s, id: e, origin: e });
            s.engine.borrow_mut().reads.pop();
            out
        })
    }

    /// What a computed cell read last time it ran.
    pub fn reads_of(&self, e: Entity, cell: &str) -> Vec<Dep> {
        self.session(|s| s.engine.borrow().memo.get(&(e, cell.to_string())).map(|m| m.deps.clone()).unwrap_or_default())
    }

    /// The cells that have actually been worked out since this was last called.
    pub fn take_ran(&self) -> Vec<Key> {
        self.session(|s| std::mem::take(&mut s.engine.borrow_mut().ran))
    }

    // ---------- kinds ----------

    /// Where a node looks for a cell or component it doesn't say itself, in order. First the other parts
    /// of the same thing, such as `Car[Hatchback]` for `Petrol car[Hatchback]`. Then the kinds of all of
    /// them: what they link to by `is_a`, and their tables, then those kinds' kinds. Something more
    /// specific always comes first, so Petrol car comes before Car. Otherwise it's nearest first, and in
    /// the order nodes were made.
    fn lookup(&self, e: Entity) -> Vec<Entity> {
        let mut parts = self.parts(e);
        let mut seen: HashSet<Entity> = parts.iter().copied().chain([e]).collect();
        let mut kinds = Vec::new();
        let mut level: Vec<Entity> = std::iter::once(e).chain(parts.iter().copied()).collect();
        while !level.is_empty() {
            let mut next = Vec::new();
            for k in level.iter().flat_map(|n| self.kinds_of(*n)) {
                if seen.insert(k) {
                    kinds.push(k);
                    next.push(k);
                }
            }
            level = next;
        }
        // Stable, so what's equally specific stays nearest first.
        parts.sort_by_key(|p| std::cmp::Reverse(self.above(*p)));
        kinds.sort_by_key(|k| std::cmp::Reverse(self.above(*k)));
        parts.extend(kinds);
        parts
    }

    /// What a node is directly a kind of: what it links to by `is_a`, and the table it's a row of.
    fn kinds_of(&self, n: Entity) -> Vec<Entity> {
        let mut out = match &self.registry.inherits_through {
            Some(through) => self.targets_of(n, through),
            None => Vec::new(),
        };
        if let Some(row) = self.row_of(n) {
            out.extend(self.names.named.get(&row.table).into_iter().flatten().copied().filter(|t| *t != n));
        }
        out
    }

    /// How many kinds are above a node, all the way up.
    fn above(&self, n: Entity) -> usize {
        let mut seen = HashSet::from([n]);
        let mut todo = vec![n];
        while let Some(k) = todo.pop() {
            for up in self.kinds_of(k) {
                if seen.insert(up) {
                    todo.push(up);
                }
            }
        }
        seen.len() - 1
    }

    /// The names of a table and of every kind above it.
    fn lineage(&self, table: &str) -> HashSet<String> {
        let mut names = HashSet::from([table.to_string()]);
        let mut seen = HashSet::new();
        let mut todo: Vec<Entity> = self.names.named.get(table).into_iter().flatten().copied().collect();
        while let Some(k) = todo.pop() {
            if seen.insert(k) {
                names.extend(self.names.of.get(&k).cloned());
                todo.extend(self.kinds_of(k));
            }
        }
        names
    }

    /// The other rows with the same key, in tables related to this one's: one is a kind of the other, or
    /// they're kinds of the same thing. Together they're one thing, each saying some of its cells.
    fn parts(&self, e: Entity) -> Vec<Entity> {
        let Some(row) = self.row_of(e) else { return Vec::new() };
        let mine = self.lineage(&row.table);
        let mut out: Vec<Entity> = Vec::new();
        for (table, rows) in &self.names.tables {
            let Some(same) = rows.get(&row.key).filter(|s| s.iter().any(|p| *p != e)) else { continue };
            if *table == row.table || !self.lineage(table).is_disjoint(&mine) {
                out.extend(same.iter().copied().filter(|p| *p != e));
            }
        }
        out.sort();
        out
    }

    /// The nodes that look straight at this one for what they don't say: what's of its kind, the rows of
    /// the table it is, and the other parts of the same row.
    fn takers(&self, k: Entity) -> Vec<Entity> {
        let mut out = match &self.registry.inherits_through {
            Some(through) => self.sources_of(k, through),
            None => Vec::new(),
        };
        if let Some(name) = self.names.of.get(&k) {
            out.extend(self.rows(name).into_iter().filter(|r| *r != k));
            out.extend(lock(&self.implicit).iter().filter(|(_, i)| matches!(i, Implicit::Row(r) if r.table == *name)).map(|(e, _)| *e));
        }
        out.extend(self.parts(k));
        out
    }

    /// Everything that might take something from this node, however indirectly.
    fn takers_all(&self, e: Entity) -> Vec<Entity> {
        let (mut out, mut seen, mut todo) = (Vec::new(), HashSet::from([e]), vec![e]);
        while let Some(k) = todo.pop() {
            for t in self.takers(k) {
                if seen.insert(t) {
                    out.push(t);
                    todo.push(t);
                }
            }
        }
        out
    }

    // ---------- tables ----------

    /// The other parts of the same thing as this row: see `parts`.
    pub fn parts_of(&self, e: Entity) -> Vec<Entity> {
        self.parts(e)
    }

    /// Which row of which table a node is, if its name says, or if it's a row nobody wrote: see `table`.
    pub fn row_of(&self, e: Entity) -> Option<Row> {
        self.names.rows.get(&e).cloned().or_else(|| match self.implicit_of(e)? {
            Implicit::Row(r) => Some(r),
            Implicit::Constant(_) => None,
        })
    }

    /// The rows nobody wrote that something has asked for, with what they'd be called.
    pub fn implicit_rows(&self) -> Vec<(Entity, String)> {
        let implicit = lock(&self.implicit);
        implicit.iter().filter_map(|(e, i)| if let Implicit::Row(r) = i { Some((*e, format!("{}[{}]", r.table, r.key))) } else { None }).collect()
    }

    /// The constants something has asked for, with what each says.
    pub fn constants(&self) -> Vec<(Entity, BTreeMap<String, Value>)> {
        let implicit = lock(&self.implicit);
        let mut out: Vec<_> = implicit.iter().filter_map(|(e, i)| if let Implicit::Constant(c) = i { Some((*e, c.clone())) } else { None }).collect();
        out.sort_by_key(|c| c.0);
        out
    }

    fn is_implicit(&self, e: Entity) -> bool {
        lock(&self.implicit).contains_key(&e)
    }

    fn implicit_of(&self, e: Entity) -> Option<Implicit> {
        lock(&self.implicit).get(&e).cloned()
    }

    /// A node nobody wrote, which is only what these cells say, such as one of a puzzle's options: a name,
    /// and a value. The same cells always make the same node, so two choices with an option in common
    /// land on the same thing. It's never saved, and nothing can be written on it.
    pub fn constant(&self, cells: BTreeMap<String, Value>) -> Entity {
        let e = Entity::constant(&cells);
        if let std::collections::hash_map::Entry::Vacant(slot) = lock(&self.implicit).entry(e) {
            slot.insert(Implicit::Constant(cells));
            lock(&self.components).remove(&e);
        }
        e
    }

    /// The row of a table with this key. That's the node with that name, if there is one. Otherwise, a
    /// numbered table goes on past its first written row as far as it's asked, with rows nobody wrote
    /// filling any gap: each takes everything from the table, and reading one of the workspace's own cells
    /// the table doesn't say is an error. Such a row only exists to be read, so it's never saved, and it's
    /// the same handle each time, so what's worked out on it stays in the memos.
    pub fn row_at(&self, table: &str, key: &table::Key) -> Option<Entity> {
        let rows = self.names.tables.get(table)?;
        if let Some(e) = rows.get(key).and_then(|s| s.first()) {
            return Some(*e);
        }
        let table::Key::Num(n) = key else { return None };
        let numbers = rows.iter().filter(|(_, s)| !s.is_empty()).filter_map(|(k, _)| if let table::Key::Num(m) = k { Some(*m) } else { None });
        let (first, last) = numbers.fold((i64::MAX, i64::MIN), |(lo, hi), m| (lo.min(m), hi.max(m)));
        if *n < first || *n > last.saturating_add(MOST_IMPLICIT) {
            return None;
        }
        let e = Entity::implicit(table, *n);
        if let std::collections::hash_map::Entry::Vacant(slot) = lock(&self.implicit).entry(e) {
            slot.insert(Implicit::Row(Row { table: table.to_string(), key: key.clone() }));
            lock(&self.components).remove(&e);
        }
        Some(e)
    }

    /// A table's rows: those with numbers in order, then the rest in the order they were made.
    pub fn rows(&self, table: &str) -> Vec<Entity> {
        let Some(rows) = self.names.tables.get(table) else { return Vec::new() };
        let (mut numbered, mut rest) = (Vec::new(), Vec::new());
        for (key, nodes) in rows {
            match key {
                table::Key::Num(_) => numbered.extend(nodes.iter().copied()),
                table::Key::Text(_) => rest.extend(nodes.iter().copied()),
            }
        }
        rest.sort();
        numbered.extend(rest);
        numbered
    }

    /// The table whose rows are around this node: its own, if it's a table, or the one it's a row of.
    pub fn table_here(&self, e: Entity) -> Option<String> {
        match self.row_of(e) {
            Some(row) => Some(row.table),
            None => self.names.of.get(&e).cloned(),
        }
    }

    /// The row this many along from a numbered row in the same table: the one before is -1. See `row_at`.
    pub fn row_along(&self, e: Entity, by: i64) -> Option<Entity> {
        let row = self.row_of(e)?;
        let table::Key::Num(n) = row.key else { return None };
        self.row_at(&row.table, &table::Key::Num(n.checked_add(by)?))
    }

    fn computed_by(&self, key: &Key) -> Option<Arc<Computed>> {
        self.backing_at(key.0, &key.1).and_then(|(b, _)| b.computed().cloned())
    }
}

/// One read in progress, holding the engine.
#[derive(Clone, Copy)]
struct Session<'a> {
    g: &'a Graph,
    engine: &'a RefCell<Engine>,
}

impl Session<'_> {
    fn note(&self, dep: Dep) {
        if let Some(top) = self.engine.borrow_mut().reads.last_mut() {
            top.push(dep);
        }
    }

    fn read<C: CellType>(&self, e: Entity) -> Cell<C> {
        let key: Key = (e, C::NAME.to_string());
        self.note(Dep::Cell(key.clone()));
        match self.g.backing_at(e, C::NAME) {
            None => self.missing(key),
            Some((Backing::Value(v), _)) => decode(&key, &v).map(Some),
            Some((Backing::Formula(_, Err(error)), _)) => Err(error),
            Some((b, _)) => {
                let c = b.computed().expect("the rest are computed").clone();
                self.settle(&key, &c)?;
                let engine = self.engine.borrow();
                let value = &engine.memo[&key].value;
                if let Some(typed) = value.downcast_ref::<Cell<C>>() {
                    return typed.clone();
                }
                // Worked out with no Rust type: JSON is the common form.
                match (c.json)(value)? {
                    Some(v) => decode(&key, &v).map(Some),
                    None => Ok(None),
                }
            }
        }
    }

    fn read_value(&self, e: Entity, cell: &str) -> Cell<Value> {
        let key: Key = (e, cell.to_string());
        self.note(Dep::Cell(key.clone()));
        match self.g.backing_at(e, cell) {
            None => self.missing(key),
            Some((Backing::Value(v), _)) => Ok(Some(v)),
            Some((Backing::Formula(_, Err(error)), _)) => Err(error),
            Some((b, _)) => {
                let c = b.computed().expect("the rest are computed").clone();
                self.settle(&key, &c)?;
                (c.json)(&self.engine.borrow().memo[&key].value)
            }
        }
    }

    /// Nothing backs this cell. On a node, that's just a cell it doesn't have. On a row nobody wrote, one
    /// of the workspace's own cells its table doesn't say is one nothing could ever fill.
    fn missing<T>(&self, key: Key) -> Cell<T> {
        let row = matches!(self.g.implicit_of(key.0), Some(Implicit::Row(_)));
        if row && self.g.registry.owner(&key.1).is_none() { Err(Error::Missing(key)) } else { Ok(None) }
    }

    /// Make sure the memo for a cell about to be read is current.
    fn settle(&self, key: &Key, c: &Arc<Computed>) -> Result<(), Error> {
        let evaluating = self.engine.borrow().evaluating.iter().position(|k| k == key);
        if let Some(at) = evaluating {
            let mut members = self.engine.borrow().evaluating[at..].to_vec();
            members.sort();
            return Err(Error::Cycle(members));
        }
        let verifying = self.engine.borrow().verifying.contains(key);
        if verifying {
            // Something checking this cell turned out to need its value, so stop checking.
            self.evaluate(key, c);
        } else {
            self.freshen(key);
        }
        Ok(())
    }

    /// Bring a cell up to date if it's out of date, returning when its value last changed.
    fn freshen(&self, key: &Key) -> u64 {
        let revision = self.g.revision;
        let Some(c) = self.g.computed_by(key) else {
            return self.g.last_change.get(&Dep::Backing(key.clone())).copied().unwrap_or(0);
        };
        let known = {
            let engine = self.engine.borrow();
            if engine.evaluating.contains(key) || engine.verifying.contains(key) {
                return revision; // Part of a loop. Treating it as changed is always safe.
            }
            engine.memo.get(key).map(|m| (m.verified, m.changed, m.deps.clone()))
        };
        let Some((verified, changed, deps)) = known else {
            return self.evaluate(key, &c);
        };
        if verified == revision {
            return changed;
        }
        self.engine.borrow_mut().verifying.insert(key.clone());
        let same = deps.iter().all(|d| stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.last_changed(d)) <= verified);
        self.engine.borrow_mut().verifying.remove(key);
        if !same {
            return self.evaluate(key, &c);
        }
        if let Some(m) = self.engine.borrow_mut().memo.get_mut(key) {
            m.verified = revision;
        }
        changed
    }

    fn last_changed(&self, dep: &Dep) -> u64 {
        match dep {
            Dep::Cell(key) => self.freshen(key),
            other => self.g.last_change.get(other).copied().unwrap_or(0),
        }
    }

    fn evaluate(&self, key: &Key, c: &Arc<Computed>) -> u64 {
        {
            let mut engine = self.engine.borrow_mut();
            engine.evaluating.push(key.clone());
            engine.reads.push(vec![Dep::Backing(key.clone())]);
            engine.ran.push(key.clone());
        }
        // Written on a kind, it reads what the kind's formula names from there. A default is about the
        // entity it's for.
        let origin = self.g.backing_at(key.0, &key.1).map_or(key.0, |(_, at)| if at == Entity::whole() { key.0 } else { at });
        let value = stacker::maybe_grow(256 * 1024, 4 * 1024 * 1024, || (c.eval)(&Cx { s: *self, id: key.0, origin }));
        let mut engine = self.engine.borrow_mut();
        let mut deps = engine.reads.pop().expect("pushed above");
        engine.evaluating.pop();
        // In the order they were made, which `freshen` relies on. A computation is deterministic, so if
        // its first reads are unchanged then so is its next. Checked in that order, its old reads never
        // lead anywhere it wouldn't go again, so they can't report a loop that isn't there.
        let mut seen = HashSet::new();
        deps.retain(|d| seen.insert(d.clone()));
        let revision = self.g.revision;
        let changed = match engine.memo.get(key) {
            Some(old) if (c.same)(&old.value, &value) => old.changed,
            _ => revision,
        };
        engine.memo.insert(key.clone(), Memo { value, deps, verified: revision, changed });
        changed
    }
}

/// All a computation can see: the entity it's working a cell out for, and anything it can reach from
/// there. What it reads is recorded, so it's worked out again only when that changes.
pub struct Cx<'a> {
    s: Session<'a>,
    id: Entity,
    origin: Entity,
}

impl Cx<'_> {
    /// The entity this is working a cell out for.
    pub fn id(&self) -> Entity {
        self.id
    }

    /// Where what's being worked out is written: the same entity, or the kind it takes it from.
    pub fn origin(&self) -> Entity {
        self.origin
    }

    /// If this is an edge, its source and target.
    pub fn ends(&self) -> Option<(Entity, Entity)> {
        self.s.g.ends(self.id)
    }

    pub fn resource<R: Send + Sync + 'static>(&self) -> Arc<R> {
        self.s.g.resource()
    }

    pub fn registry(&self) -> &Registry {
        &self.s.g.registry
    }

    /// Whether an entity has a component.
    pub fn has(&self, e: Entity, component: &str) -> bool {
        self.s.note(Dep::Has(e));
        self.s.g.has(e, component)
    }

    /// Whether this entity has a component.
    pub fn own_has(&self, component: &str) -> bool {
        self.has(self.id, component)
    }

    /// How one of this entity's cells is backed, if it's there.
    pub fn backed(&self, cell: &str) -> Option<Backed> {
        self.s.note(Dep::Backing((self.id, cell.to_string())));
        self.s.g.backing_at(self.id, cell).map(|(b, _)| (&b).into())
    }

    /// Whether this entity has a value written in this cell itself.
    pub fn own_is_stored(&self, cell: &str) -> bool {
        self.is_stored(self.id, cell)
    }

    /// Whether that entity has a value written in this cell itself.
    pub fn is_stored(&self, e: Entity, cell: &str) -> bool {
        self.s.note(Dep::Backing((e, cell.to_string())));
        self.s.g.stored(e, cell).is_some()
    }

    /// A cell on this entity, if it has it.
    pub fn own<C: CellType>(&self) -> Cell<C> {
        self.s.read(self.id)
    }

    /// A cell this entity must have.
    pub fn own_value<C: CellType>(&self) -> Result<C, Error> {
        self.value(self.id)
    }

    /// A cell on another entity, if it has it.
    pub fn get<C: CellType>(&self, e: Entity) -> Cell<C> {
        self.s.read(e)
    }

    /// A cell that entity must have.
    pub fn value<C: CellType>(&self, e: Entity) -> Result<C, Error> {
        self.get(e)?.ok_or_else(|| Error::Missing((e, C::NAME.to_string())))
    }

    /// Any cell, by its name.
    pub fn get_value(&self, e: Entity, cell: &str) -> Cell<Value> {
        self.s.read_value(e, cell)
    }

    /// The edges with this component arriving here, each with its source.
    pub fn incoming(&self, component: &str) -> Vec<(Entity, Entity)> {
        self.s.note(Dep::Into(self.id, component.to_string()));
        self.s.g.edges_into(self.id, component)
    }

    /// The edges with this component leaving here, each with its target.
    pub fn outgoing(&self, component: &str) -> Vec<(Entity, Entity)> {
        self.outgoing_at(self.id, component)
    }

    /// The same, from where what's being worked out is written: see `origin`.
    pub fn outgoing_from_origin(&self, component: &str) -> Vec<(Entity, Entity)> {
        self.outgoing_at(self.origin, component)
    }

    fn outgoing_at(&self, at: Entity, component: &str) -> Vec<(Entity, Entity)> {
        self.s.note(Dep::From(at, component.to_string()));
        self.s.g.edges_from(at, component)
    }

    /// The nodes linking here by an edge with this component.
    pub fn sources(&self, component: &str) -> Vec<Entity> {
        self.incoming(component).into_iter().map(|(n, _)| n).collect()
    }

    /// The nodes this links to by an edge with this component.
    pub fn targets(&self, component: &str) -> Vec<Entity> {
        self.outgoing(component).into_iter().map(|(n, _)| n).collect()
    }

    /// Every node with this component.
    pub fn scan(&self, component: &str) -> Vec<Entity> {
        self.s.note(Dep::Scan(component.to_string()));
        self.s.g.having(component)
    }

    // ---------- tables: see `table` ----------

    /// The rows of the table this entity is, or is a row of. These depend on names, so renaming anything
    /// reruns what read them.
    pub fn rows(&self) -> Vec<Entity> {
        self.s.note(Dep::Names);
        self.s.g.table_here(self.id).map(|t| self.s.g.rows(&t)).unwrap_or_default()
    }

    /// The rows of the table with this name, as `Arrived.rows` reads them from anywhere.
    pub fn rows_of(&self, table: &str) -> Vec<Entity> {
        self.s.note(Dep::Names);
        self.s.g.rows(table)
    }

    /// The row this many along from this entity, if it's a numbered row and there's one there: the one
    /// before is -1.
    pub fn row_along(&self, by: i64) -> Option<Entity> {
        self.s.note(Dep::Names);
        self.s.g.row_along(self.id, by)
    }

    /// The row of a table with this key, as `Year[3]` addresses it: see `Graph::row_at`.
    pub fn row_at(&self, table: &str, key: &table::Key) -> Option<Entity> {
        self.s.note(Dep::Names);
        self.s.g.row_at(table, key)
    }

    /// A node that's only what these cells say: see `Graph::constant`. Nothing about it can change, so
    /// there's nothing to depend on.
    pub fn constant(&self, cells: BTreeMap<String, Value>) -> Entity {
        self.s.g.constant(cells)
    }

    /// Which row this entity is in its table.
    pub fn row_key(&self) -> Option<table::Key> {
        self.row_key_of(self.id)
    }

    /// Which row that node is in its table, as `q.row` reads it.
    pub fn row_key_of(&self, e: Entity) -> Option<table::Key> {
        self.s.note(Dep::Names);
        self.s.g.row_of(e).map(|r| r.key)
    }
}

fn decode<C: DeserializeOwned>(cell: &Key, v: &Value) -> Result<C, Error> {
    serde_json::from_value(v.clone()).map_err(|e| Error::Malformed { cell: cell.clone(), detail: e.to_string() })
}
