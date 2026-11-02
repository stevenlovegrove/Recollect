//! The core: entities and links, each a bag of components. A cell is the data for one (entity,
//! component) pair, and its storage is either a value or a rule that returns one.
//!
//! ```
//! use recollect_graph::{Graph, component, rule};
//!
//! #[component]
//! struct Amount(f64);
//!
//! #[component]
//! struct Addend;
//!
//! let sum = rule("sum").incoming::<Addend>().compute(|cx| {
//!     let mut total = 0.0;
//!     for from in cx.sources::<Addend>()? {
//!         total += cx.value::<Amount>(from)?.0;
//!     }
//!     Ok(Amount(total))
//! });
//!
//! let mut g = Graph::new();
//! let (price, total) = (g.spawn(), g.spawn());
//! g.set(price, Amount(10.0));              // a value
//! g.set(total, sum);                       // a rule, recorded and not yet run
//! g.link(price, total, Addend);            // the edge it follows
//! assert_eq!(g.get(total), Ok(Some(Amount(10.0))));  // runs what's stale, and remembers
//!
//! g.set(price, Amount(12.0));              // runs nothing
//! assert_eq!(g.get(total), Ok(Some(Amount(12.0))));
//! ```
//!
//! Rules run lazily, when a cell is read, and results are memoised across edits. A rule declares
//! the kinds of link it follows and may only read through them. What it actually read is
//! recorded, and that's what invalidation runs on. This module knows nothing about worlds.
//!
//! A `Graph` is `Send + Sync`: reading takes `&self`, and writing takes `&mut self`, so shared use
//! is an `RwLock<Graph>`. Reads currently take turns on one lock while rules run. That's an
//! implementation detail the API doesn't expose.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::registry::{Registry, SavedRule};

/// A node. An opaque handle with no meaning of its own: a name is just a component. Time-ordered,
/// so nodes sort in creation order, and files created on two machines never collide.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Entity(Uuid);

impl Entity {
    pub fn new() -> Self {
        Entity(Uuid::now_v7())
    }

    /// Stands in for "no particular node", such as in an error about a whole column.
    pub(crate) fn nil() -> Self {
        Entity(Uuid::nil())
    }

    /// Where a cell about the graph as a whole lives, so it's worked out once however many nodes read
    /// it. It isn't a node: it has no links and nothing can be stored on it, and only a component's
    /// default can fill it.
    pub fn whole() -> Self {
        Self::nil()
    }
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

pub type Key = (Entity, String);

/// A column. JSON under a name is what a component really is, and a Rust type is a typed way to
/// look one up. Define one with `#[component]`.
pub trait Component: Clone + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;
}

/// A component with no Rust type: its name, and its data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub name: String,
    pub value: Value,
}

impl Runtime {
    pub fn new(name: &str, value: Value) -> Self {
        Runtime { name: name.to_string(), value }
    }

    /// The same data seen through a Rust type.
    pub fn of<C: Component>(c: &C) -> Self {
        Runtime::new(C::NAME, serde_json::to_value(c).expect("components serialise"))
    }

    pub fn decode<C: Component>(&self) -> Result<C, serde_json::Error> {
        serde_json::from_value(self.value.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// These cells depend on each other.
    Cycle(Vec<Key>),
    /// A rule read something it didn't declare.
    Undeclared { rule: String, read: String },
    /// A rule required a cell that isn't there.
    Missing(Key),
    Malformed { cell: Key, detail: String },
    /// A rule's own failure, like #DIV/0!.
    Failed(String),
    /// This cell names a rule nothing has registered, such as one from a missing plugin.
    UnknownRule(String),
    /// This cell names a rule that provides a different component.
    WrongRule { rule: String, component: String, provides: String },
}

/// What reading a cell gives. `None` means this node doesn't have the component.
pub type Cell<C> = Result<Option<C>, Error>;

/// What backs a cell: a value kept at the node, or a rule that returns one. Either converts into
/// this, so `set` takes both.
pub enum Storage<C> {
    Value(C),
    Rule(Rule<C>),
}

impl<C: Component> From<C> for Storage<C> {
    fn from(v: C) -> Self {
        Storage::Value(v)
    }
}

impl<C> From<Rule<C>> for Storage<C> {
    fn from(r: Rule<C>) -> Self {
        Storage::Rule(r)
    }
}

impl From<Value> for Storage<Value> {
    fn from(v: Value) -> Self {
        Storage::Value(v)
    }
}

/// What a rule may follow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inputs {
    pub incoming: Vec<String>,
    pub outgoing: Vec<String>,
    pub scans: Vec<String>,
    /// It may also read whatever the rule on this component of the same node may. It's for a rule
    /// that's about another rule, such as working out what type a formula gives.
    pub like: Option<String>,
}

pub struct Rule<C> {
    pub name: String,
    pub inputs: Inputs,
    pub(crate) f: Arc<RuleFn<C>>,
    /// How to write this rule down. A closure has none, so it can't be saved.
    pub(crate) saved: Option<SavedRule>,
}

pub(crate) type RuleFn<C> = dyn Fn(&Cx) -> Cell<C> + Send + Sync;

impl<C> Clone for Rule<C> {
    fn clone(&self) -> Self {
        Rule { name: self.name.clone(), inputs: self.inputs.clone(), f: self.f.clone(), saved: self.saved.clone() }
    }
}

/// Starts a rule: `rule("sum").incoming::<Addend>().compute(|cx| ...)`.
pub fn rule(name: &str) -> RuleSpec {
    RuleSpec { name: name.into(), inputs: Inputs::default() }
}

pub struct RuleSpec {
    pub(crate) name: String,
    pub(crate) inputs: Inputs,
}

impl RuleSpec {
    pub fn incoming<L: Component>(self) -> Self {
        self.incoming_named(L::NAME)
    }
    pub fn outgoing<L: Component>(self) -> Self {
        self.outgoing_named(L::NAME)
    }
    pub fn scans<C: Component>(self) -> Self {
        self.scans_named(C::NAME)
    }
    pub fn incoming_named(mut self, kind: &str) -> Self {
        self.inputs.incoming.push(kind.into());
        self
    }
    pub fn outgoing_named(mut self, kind: &str) -> Self {
        self.inputs.outgoing.push(kind.into());
        self
    }
    pub fn scans_named(mut self, component: &str) -> Self {
        self.inputs.scans.push(component.into());
        self
    }
    /// What it may follow, all at once, for a rule where that depends on its parameters.
    pub fn following(mut self, inputs: Inputs) -> Self {
        self.inputs = inputs;
        self
    }
    /// A rule that gives every node it's on a value.
    pub fn compute<C: 'static>(self, f: impl Fn(&Cx) -> Result<C, Error> + Send + Sync + 'static) -> Rule<C> {
        self.optional(move |cx| f(cx).map(Some))
    }
    /// A rule that may decide the node has no such component.
    pub fn optional<C: 'static>(self, f: impl Fn(&Cx) -> Cell<C> + Send + Sync + 'static) -> Rule<C> {
        Rule { name: self.name, inputs: self.inputs, f: Arc::new(f), saved: None }
    }
}

type AnyValue = Arc<dyn Any + Send + Sync>;
/// It runs for one node, and is written on another if that node takes it from its kind.
type EvalFn = dyn Fn(Session, Entity, Entity, &Inputs, &str) -> AnyValue + Send + Sync;

pub(crate) struct Erased {
    pub(crate) name: String,
    pub(crate) inputs: Inputs,
    pub(crate) saved: Option<SavedRule>,
    eval: Arc<EvalFn>,
    same: fn(&AnyValue, &AnyValue) -> bool,
    json: fn(&AnyValue) -> Result<Option<Value>, Error>,
}

fn panic_message(p: Box<dyn Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown".into())
}

fn erase<C: Clone + PartialEq + Send + Sync + 'static>(r: Rule<C>, json: fn(&AnyValue) -> Result<Option<Value>, Error>) -> Arc<Erased> {
    let f = r.f;
    Arc::new(Erased {
        name: r.name,
        inputs: r.inputs,
        saved: r.saved,
        eval: Arc::new(move |s, id, origin, inputs, name| {
            let cx = Cx { s, id, origin, inputs, rule: name };
            // A rule that panics fails its own cell, not the graph and every thread reading it.
            let result = catch_unwind(AssertUnwindSafe(|| f(&cx))).unwrap_or_else(|p| Err(Error::Failed(format!("rule {name} panicked: {}", panic_message(p)))));
            Arc::new(result) as AnyValue
        }),
        same: |a, b| a.downcast_ref::<Cell<C>>() == b.downcast_ref::<Cell<C>>(),
        json,
    })
}

pub(crate) fn erase_typed<C: Component>(r: Rule<C>) -> Arc<Erased> {
    erase(r, |a| a.downcast_ref::<Cell<C>>().expect("one component, one type").clone().map(|v| v.map(|v| serde_json::to_value(&v).expect("components serialise"))))
}

pub(crate) fn erase_runtime(r: Rule<Value>) -> Arc<Erased> {
    erase(r, |a| a.downcast_ref::<Cell<Value>>().expect("one component, one type").clone())
}

/// `Storage`, once the type has been erased.
#[derive(Clone)]
pub(crate) enum Backing {
    Stored(Value),
    Computed(Arc<Erased>),
    /// A saved rule that can't run. It's kept exactly as written, so saving doesn't lose it.
    Broken(SavedRule, Error),
}

pub(crate) fn typed_backing<C: Component>(s: Storage<C>) -> Backing {
    match s {
        Storage::Value(v) => Backing::Stored(serde_json::to_value(&v).expect("components serialise")),
        Storage::Rule(r) => Backing::Computed(erase_typed(r)),
    }
}

/// One thing a computed cell read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dep {
    /// Another cell's value.
    Cell(Key),
    /// How this cell is backed: replacing a value or a rule changes this.
    Backing(Key),
    /// Links of a kind arriving at a node.
    Into(Entity, String),
    /// Links of a kind leaving a node.
    From(Entity, String),
    /// Which nodes hold a component.
    Scan(String),
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

/// How a node takes the cells it doesn't have from its kind.
pub(crate) struct Inheriting {
    /// The kind of link from a node to its kind.
    pub(crate) through: String,
    /// What the systems say: a kind only says what the workspace's own cells are.
    pub(crate) owned: HashSet<String>,
}

#[derive(Default)]
pub struct Graph {
    pub(crate) entities: BTreeSet<Entity>,
    pub(crate) cells: HashMap<Entity, BTreeMap<String, Backing>>,
    /// What backs a component on every node that doesn't say otherwise.
    pub(crate) defaults: HashMap<String, Backing>,
    inheriting: Option<Arc<Inheriting>>,
    pub(crate) links: BTreeMap<(Entity, Entity), BTreeMap<String, Value>>,
    into: HashMap<Entity, BTreeSet<Entity>>,
    from: HashMap<Entity, BTreeSet<Entity>>,
    resources: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
    pub(crate) registry: Arc<Registry>,
    revision: u64,
    last_change: HashMap<Dep, u64>,
    engine: Mutex<RefCell<Engine>>,
}

impl Graph {
    /// A graph with no rules registered: closures still work, but nothing can be saved by key.
    pub fn new() -> Self {
        Self::default()
    }

    /// A graph using these rules, and whatever defaults they set.
    pub fn with_registry(registry: Arc<Registry>) -> Self {
        let inheriting = registry.inherits_through.as_ref().map(|through| Arc::new(Inheriting { through: through.clone(), owned: registry.owned() }));
        let mut g = Graph { defaults: registry.defaults.clone(), inheriting, registry, ..Self::default() };
        g.revision = 1;
        g
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
        if self.entities.insert(e) {
            self.revision += 1;
            for c in self.defaults.keys().cloned().collect::<Vec<_>>() {
                // It may have existed before, so anything that read it then has to look again.
                self.touch(Dep::Backing((e, c.clone())));
                self.touch(Dep::Scan(c));
            }
        }
        self
    }

    /// Remove a node and every cell on it. Its links have to go first, because they say something
    /// about other nodes too. Removing a node with links, or no such node, changes nothing and
    /// returns false.
    pub fn despawn(&mut self, e: Entity) -> bool {
        let linked = self.into.get(&e).is_some_and(|s| !s.is_empty()) || self.from.get(&e).is_some_and(|s| !s.is_empty());
        if linked || !self.entities.remove(&e) {
            return false;
        }
        self.revision += 1;
        let own = self.cells.remove(&e).unwrap_or_default().into_keys();
        let columns: BTreeSet<String> = own.chain(self.defaults.keys().cloned()).collect();
        for c in columns {
            self.forget(e, &c);
            self.touch(Dep::Backing((e, c.clone())));
            self.touch(Dep::Scan(c));
        }
        self.into.remove(&e);
        self.from.remove(&e);
        true
    }

    fn known(&self, e: Entity) {
        assert!(self.entities.contains(&e), "{e} isn't a node in this graph");
    }

    /// Back a cell with a value or a rule. Setting a value equal to the one there changes
    /// nothing. A rule is only recorded here, and runs when the cell is read.
    pub fn set<C: Component>(&mut self, e: Entity, storage: impl Into<Storage<C>>) -> &mut Self {
        self.known(e);
        self.back(e, C::NAME, typed_backing(storage.into()));
        self
    }

    /// The same for a component with no Rust type, given by name.
    pub fn set_runtime(&mut self, e: Entity, name: &str, storage: impl Into<Storage<Value>>) -> &mut Self {
        self.known(e);
        let backing = match storage.into() {
            Storage::Value(v) => Backing::Stored(v),
            Storage::Rule(r) => Backing::Computed(erase_runtime(r)),
        };
        self.back(e, name, backing);
        self
    }

    /// Back a cell with a registered rule by its key, as a file or the LLM would. If it can't
    /// run, the cell says why.
    pub fn set_rule(&mut self, e: Entity, component: &str, rule: &str, params: Value) -> &mut Self {
        self.known(e);
        let saved = SavedRule { rule: rule.to_string(), params };
        let backing = self.registry.clone().resolve(&(e, component.to_string()), &saved);
        self.back(e, component, backing);
        self
    }

    /// What backs this component on every node that doesn't say otherwise.
    pub fn set_default<C: Component>(&mut self, storage: impl Into<Storage<C>>) -> &mut Self {
        self.back_default(C::NAME, Some(typed_backing(storage.into())));
        self
    }

    pub(crate) fn back_default(&mut self, component: &str, b: Option<Backing>) {
        self.revision += 1;
        let keeps_memo = matches!(b, Some(Backing::Computed(_)));
        match b {
            Some(b) => self.defaults.insert(component.to_string(), b),
            None => self.defaults.remove(component),
        };
        for e in self.entities.iter().copied().collect::<Vec<_>>() {
            if !keeps_memo {
                self.forget(e, component);
            }
            self.touch(Dep::Backing((e, component.to_string())));
        }
        self.touch(Dep::Scan(component.to_string()));
    }

    pub(crate) fn back(&mut self, e: Entity, component: &str, b: Backing) {
        let unchanged = match (&b, self.cells.get(&e).and_then(|c| c.get(component))) {
            (Backing::Stored(new), Some(Backing::Stored(old))) => new == old,
            (Backing::Computed(new), Some(Backing::Computed(old))) => new.saved.is_some() && new.saved == old.saved,
            (Backing::Broken(new, _), Some(Backing::Broken(old, _))) => new == old,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.revision += 1;
        if matches!(b, Backing::Stored(_)) {
            self.forget(e, component);
        }
        let is_new = self.cells.entry(e).or_default().insert(component.to_string(), b).is_none();
        self.rebacked(e, component);
        if is_new {
            self.touch(Dep::Scan(component.to_string()));
        }
    }

    /// The nodes that take this cell from `e`, because it's their kind or their kind's kind, and that
    /// don't say it for themselves.
    fn heirs(&self, e: Entity, component: &str) -> Vec<Entity> {
        let Some(inh) = self.inheriting.as_ref().filter(|i| !i.owned.contains(component)) else { return Vec::new() };
        let (mut out, mut seen, mut todo) = (Vec::new(), HashSet::from([e]), vec![e]);
        while let Some(k) = todo.pop() {
            for s in self.sources_of(k, &inh.through) {
                if seen.insert(s) && !self.has_own(s, component) {
                    out.push(s);
                    todo.push(s);
                }
            }
        }
        out
    }

    /// What backs this cell changed, here and for whatever takes it from here.
    fn rebacked(&mut self, e: Entity, component: &str) {
        self.touch(Dep::Backing((e, component.to_string())));
        let heirs = self.heirs(e, component);
        if heirs.is_empty() {
            return;
        }
        for h in heirs {
            // Whether it's now a value or a rule, it isn't an unbroken run of the old one.
            self.forget(h, component);
            self.touch(Dep::Backing((h, component.to_string())));
        }
        self.touch(Dep::Scan(component.to_string()));
    }

    /// `e` took a kind, or stopped taking one, so every cell it could take from there may have changed.
    fn rekinded(&mut self, e: Entity, kind: Entity) {
        let Some(inh) = self.inheriting.clone() else { return };
        for c in self.components_of(kind) {
            if inh.owned.contains(&c) || self.has_own(e, &c) {
                continue;
            }
            self.forget(e, &c);
            self.rebacked(e, &c);
            self.touch(Dep::Scan(c));
        }
    }

    /// A memo only stands for an unbroken run of being computed. If readers have seen a typed-in
    /// value since, a rule coming back with the old answer is still a change to them.
    fn forget(&mut self, e: Entity, component: &str) {
        self.engine.get_mut().unwrap_or_else(PoisonError::into_inner).get_mut().memo.remove(&(e, component.to_string()));
    }

    pub fn remove<C: Component>(&mut self, e: Entity) -> &mut Self {
        self.remove_named(e, C::NAME)
    }

    pub fn remove_named(&mut self, e: Entity, component: &str) -> &mut Self {
        if self.cells.get_mut(&e).and_then(|c| c.remove(component)).is_some() {
            self.revision += 1;
            self.forget(e, component);
            self.rebacked(e, component);
            self.touch(Dep::Scan(component.to_string()));
        }
        self
    }

    pub fn link<L: Component>(&mut self, from: Entity, to: Entity, value: L) -> &mut Self {
        self.link_runtime(from, to, Runtime::of(&value))
    }

    pub fn link_runtime(&mut self, from: Entity, to: Entity, c: Runtime) -> &mut Self {
        self.known(from);
        self.known(to);
        let bag = self.links.entry((from, to)).or_default();
        if bag.get(&c.name) == Some(&c.value) {
            return self;
        }
        bag.insert(c.name.clone(), c.value);
        self.into.entry(to).or_default().insert(from);
        self.from.entry(from).or_default().insert(to);
        self.revision += 1;
        self.touch(Dep::Into(to, c.name.clone()));
        if self.inheriting.as_ref().is_some_and(|i| i.through == c.name) {
            self.rekinded(from, to);
        }
        self.touch(Dep::From(from, c.name));
        self
    }

    pub fn unlink<L: Component>(&mut self, from: Entity, to: Entity) -> &mut Self {
        self.unlink_named(from, to, L::NAME)
    }

    /// A link is its kinds, so removing the last kind removes the link.
    pub fn unlink_named(&mut self, from: Entity, to: Entity, kind: &str) -> &mut Self {
        let key = (from, to);
        let removed = self.links.get_mut(&key).and_then(|b| b.remove(kind)).is_some();
        if removed {
            if self.links[&key].is_empty() {
                self.links.remove(&key);
                self.into.get_mut(&to).map(|s| s.remove(&from));
                self.from.get_mut(&from).map(|s| s.remove(&to));
            }
            self.revision += 1;
            self.touch(Dep::Into(to, kind.to_string()));
            self.touch(Dep::From(from, kind.to_string()));
            if self.inheriting.as_ref().is_some_and(|i| i.through == kind) {
                self.rekinded(from, to);
            }
        }
        self
    }

    /// Something shared by rules that isn't part of the graph.
    pub fn insert_resource<R: Send + Sync + 'static>(&mut self, r: R) -> &mut Self {
        self.resources.insert(TypeId::of::<R>(), Arc::new(r));
        self
    }

    pub fn resource<R: Send + Sync + 'static>(&self) -> Arc<R> {
        let r = self.resources.get(&TypeId::of::<R>()).unwrap_or_else(|| panic!("no such resource")).clone();
        r.downcast::<R>().expect("keyed by type")
    }

    /// An independent copy to try things on. It keeps the memos, so it only reruns what the
    /// changes reach.
    pub fn fork(&self) -> Graph {
        let memo = self.session(|s| s.engine.borrow().memo.clone());
        Graph {
            entities: self.entities.clone(),
            cells: self.cells.clone(),
            defaults: self.defaults.clone(),
            inheriting: self.inheriting.clone(),
            links: self.links.clone(),
            into: self.into.clone(),
            from: self.from.clone(),
            resources: self.resources.clone(),
            registry: self.registry.clone(),
            revision: self.revision,
            last_change: self.last_change.clone(),
            engine: Mutex::new(RefCell::new(Engine { memo, ..Engine::default() })),
        }
    }

    // ---------- reading ----------

    fn session<R>(&self, f: impl FnOnce(Session) -> R) -> R {
        let guard = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        f(Session { g: self, engine: &guard })
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.entities.iter().copied()
    }

    /// Every link, with what it holds.
    pub fn links(&self) -> impl Iterator<Item = (Entity, Entity, &BTreeMap<String, Value>)> + '_ {
        self.links.iter().map(|((from, to), bag)| (*from, *to, bag))
    }

    /// The nodes linking to this one by this kind of link.
    pub fn sources_of(&self, e: Entity, kind: &str) -> Vec<Entity> {
        self.into.get(&e).into_iter().flatten().copied().filter(|f| self.has_link(*f, e, kind)).collect()
    }

    /// The typed way to look a component up.
    pub fn get<C: Component>(&self, e: Entity) -> Cell<C> {
        self.session(|s| s.read(e))
    }

    /// Any component by its name, whether or not Rust has a type for it.
    pub fn get_runtime(&self, e: Entity, component: &str) -> Cell<Runtime> {
        self.session(|s| s.read_runtime(e, component))
    }

    /// The value stored on this very node, if that's what backs this cell.
    pub fn stored(&self, e: Entity, component: &str) -> Option<&Value> {
        match self.cells.get(&e)?.get(component)? {
            Backing::Stored(v) => Some(v),
            _ => None,
        }
    }

    /// Whether this node says for itself what backs a cell, rather than taking the default.
    pub fn has_own(&self, e: Entity, component: &str) -> bool {
        self.cells.get(&e).is_some_and(|c| c.contains_key(component))
    }

    /// What a link holds under this kind, if it has that kind.
    pub fn link_value(&self, from: Entity, to: Entity, kind: &str) -> Option<&Value> {
        self.links.get(&(from, to))?.get(kind)
    }

    /// The nodes this one links to by this kind of link.
    pub fn targets_of(&self, e: Entity, kind: &str) -> Vec<Entity> {
        self.from.get(&e).into_iter().flatten().copied().filter(|t| self.has_link(e, *t, kind)).collect()
    }

    /// Which rule backs a cell, and what it may follow.
    pub fn rule_of(&self, e: Entity, component: &str) -> Option<(String, Inputs)> {
        self.computed_by(e, component).map(|r| (r.name.clone(), r.inputs.clone()))
    }

    /// How a cell's rule is written down, if a rule backs it and it can be written down.
    pub fn saved_rule(&self, e: Entity, component: &str) -> Option<SavedRule> {
        match self.backing(e, component)? {
            Backing::Computed(r) => r.saved.clone(),
            Backing::Broken(saved, _) => Some(saved),
            Backing::Stored(_) => None,
        }
    }

    /// What a computed cell read last time it ran.
    pub fn reads_of(&self, e: Entity, component: &str) -> Vec<Dep> {
        self.session(|s| s.engine.borrow().memo.get(&(e, component.to_string())).map(|m| m.deps.clone()).unwrap_or_default())
    }

    /// The cells whose rules have actually run since this was last called.
    pub fn take_ran(&self) -> Vec<Key> {
        self.session(|s| std::mem::take(&mut s.engine.borrow_mut().ran))
    }

    fn backing(&self, e: Entity, component: &str) -> Option<Backing> {
        self.backing_at(e, component).map(|(b, _)| b)
    }

    /// What backs a cell, and the node it's written on: this one, unless it's taken from a kind.
    fn backing_at(&self, e: Entity, component: &str) -> Option<(Backing, Entity)> {
        if e == Entity::whole() {
            return self.defaults.get(component).map(|b| (b.clone(), e));
        }
        if !self.entities.contains(&e) {
            return None;
        }
        if let Some(b) = self.cells.get(&e).and_then(|c| c.get(component)) {
            return Some((b.clone(), e));
        }
        self.from_kind(e, component).or_else(|| self.defaults.get(component).map(|b| (b.clone(), e)))
    }

    /// The nearest kind that says what this cell is: its kinds first, in the order they were made, then
    /// theirs.
    fn from_kind(&self, e: Entity, component: &str) -> Option<(Backing, Entity)> {
        let inh = self.inheriting.as_ref().filter(|i| !i.owned.contains(component))?;
        let mut seen = HashSet::from([e]);
        let mut level = vec![e];
        while !level.is_empty() {
            let mut next = Vec::new();
            for k in level.iter().flat_map(|n| self.targets_of(*n, &inh.through)) {
                if !seen.insert(k) {
                    continue;
                }
                if let Some(b) = self.cells.get(&k).and_then(|c| c.get(component)) {
                    return Some((b.clone(), k));
                }
                next.push(k);
            }
            level = next;
        }
        None
    }

    /// The node a node takes this cell from, if it takes it from a kind.
    pub fn kind_of(&self, e: Entity, component: &str) -> Option<Entity> {
        self.backing_at(e, component).map(|(_, at)| at).filter(|at| *at != e)
    }

    /// The components a node has a cell for, whichever way each is backed.
    pub fn components_of(&self, e: Entity) -> BTreeSet<String> {
        let own = self.cells.get(&e).into_iter().flat_map(|c| c.keys());
        let mut out: BTreeSet<String> = own.chain(self.defaults.keys()).cloned().collect();
        if let Some(inh) = &self.inheriting {
            let mut seen = HashSet::from([e]);
            let mut todo = self.targets_of(e, &inh.through);
            while let Some(k) = todo.pop() {
                if seen.insert(k) {
                    out.extend(self.cells.get(&k).into_iter().flat_map(|c| c.keys()).filter(|c| !inh.owned.contains(*c)).cloned());
                    todo.extend(self.targets_of(k, &inh.through));
                }
            }
        }
        out
    }

    fn computed_by(&self, e: Entity, component: &str) -> Option<Arc<Erased>> {
        match self.backing(e, component) {
            Some(Backing::Computed(r)) => Some(r),
            _ => None,
        }
    }

    fn has_link(&self, from: Entity, to: Entity, kind: &str) -> bool {
        self.links.get(&(from, to)).is_some_and(|b| b.contains_key(kind))
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

    fn read<C: Component>(&self, e: Entity) -> Cell<C> {
        let key: Key = (e, C::NAME.to_string());
        self.note(Dep::Cell(key.clone()));
        match self.g.backing(e, C::NAME) {
            None => Ok(None),
            Some(Backing::Stored(v)) => decode(&key, &v).map(Some),
            Some(Backing::Broken(_, error)) => Err(error),
            Some(Backing::Computed(r)) => {
                self.settle(&key, &r)?;
                let engine = self.engine.borrow();
                let value = &engine.memo[&key].value;
                if let Some(typed) = value.downcast_ref::<Cell<C>>() {
                    return typed.clone();
                }
                // Computed by a rule with no Rust type: JSON is the common form.
                match (r.json)(value)? {
                    Some(v) => decode(&key, &v).map(Some),
                    None => Ok(None),
                }
            }
        }
    }

    fn read_runtime(&self, e: Entity, component: &str) -> Cell<Runtime> {
        let key: Key = (e, component.to_string());
        self.note(Dep::Cell(key.clone()));
        let value = match self.g.backing(e, component) {
            None => None,
            Some(Backing::Stored(v)) => Some(v),
            Some(Backing::Broken(_, error)) => return Err(error),
            Some(Backing::Computed(r)) => {
                self.settle(&key, &r)?;
                (r.json)(&self.engine.borrow().memo[&key].value)?
            }
        };
        Ok(value.map(|value| Runtime { name: component.to_string(), value }))
    }

    /// Make sure the memo for a cell about to be read is current.
    fn settle(&self, key: &Key, r: &Arc<Erased>) -> Result<(), Error> {
        let evaluating = self.engine.borrow().evaluating.iter().position(|k| k == key);
        if let Some(at) = evaluating {
            let mut members = self.engine.borrow().evaluating[at..].to_vec();
            members.sort();
            return Err(Error::Cycle(members));
        }
        let verifying = self.engine.borrow().verifying.contains(key);
        if verifying {
            // Something checking this cell turned out to need its value, so stop checking.
            self.evaluate(key, r);
        } else {
            self.freshen(key);
        }
        Ok(())
    }

    /// Bring a cell up to date if it's out of date, returning when its value last changed.
    fn freshen(&self, key: &Key) -> u64 {
        let revision = self.g.revision;
        let Some(r) = self.g.computed_by(key.0, &key.1) else {
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
            return self.evaluate(key, &r);
        };
        if verified == revision {
            return changed;
        }
        self.engine.borrow_mut().verifying.insert(key.clone());
        let same = deps.iter().all(|d| stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.last_changed(d)) <= verified);
        self.engine.borrow_mut().verifying.remove(key);
        if !same {
            return self.evaluate(key, &r);
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

    fn evaluate(&self, key: &Key, r: &Arc<Erased>) -> u64 {
        {
            let mut engine = self.engine.borrow_mut();
            engine.evaluating.push(key.clone());
            engine.reads.push(vec![Dep::Backing(key.clone())]);
            engine.ran.push(key.clone());
        }
        let origin = self.g.backing_at(key.0, &key.1).map_or(key.0, |(_, at)| at);
        let value = stacker::maybe_grow(256 * 1024, 4 * 1024 * 1024, || (r.eval)(*self, key.0, origin, &r.inputs, &r.name));
        let mut engine = self.engine.borrow_mut();
        let mut deps = engine.reads.pop().expect("pushed above");
        engine.evaluating.pop();
        // In the order they were made, which `freshen` relies on. A rule is deterministic, so if
        // its first reads are unchanged then so is its next. Checked in that order, a rule's old
        // reads never lead anywhere it wouldn't go again, so they can't report a loop that isn't
        // there.
        let mut seen = HashSet::new();
        deps.retain(|d| seen.insert(d.clone()));
        let revision = self.g.revision;
        let changed = match engine.memo.get(key) {
            Some(old) if (r.same)(&old.value, &value) => old.changed,
            _ => revision,
        };
        engine.memo.insert(key.clone(), Memo { value, deps, verified: revision, changed });
        changed
    }
}

/// All a rule can see.
pub struct Cx<'a> {
    s: Session<'a>,
    id: Entity,
    origin: Entity,
    inputs: &'a Inputs,
    rule: &'a str,
}

impl Cx<'_> {
    /// The node this rule is running for.
    pub fn id(&self) -> Entity {
        self.id
    }

    /// The node this rule is written on: the same node, or the kind it takes it from. What it may
    /// follow, it may follow from either.
    pub fn origin(&self) -> Entity {
        self.origin
    }

    pub fn resource<R: Send + Sync + 'static>(&self) -> Arc<R> {
        self.s.g.resource()
    }

    fn undeclared(&self, read: String) -> Error {
        Error::Undeclared { rule: self.rule.to_string(), read }
    }

    fn may_read(&self, other: Entity) -> Result<(), Error> {
        let g = self.s.g;
        let from = |at: Entity, i: &Inputs| i.incoming.iter().any(|k| g.has_link(other, at, k)) || i.outgoing.iter().any(|k| g.has_link(at, other, k));
        let reaches = |i: &Inputs| from(self.id, i) || (self.origin != self.id && from(self.origin, i)) || i.scans.iter().any(|c| g.backing(other, c).is_some());
        let reached = other == self.id || other == self.origin || other == Entity::whole() || reaches(self.inputs) || self.like().is_some_and(|r| reaches(&r.inputs));
        if reached { Ok(()) } else { Err(self.undeclared(format!("{other}, which no declared link reaches"))) }
    }

    /// The rule whose inputs this one may also use.
    fn like(&self) -> Option<Arc<Erased>> {
        self.inputs.like.as_ref().and_then(|c| self.s.g.computed_by(self.id, c))
    }

    /// How a cell on this node is written down, if a rule backs it. Replacing that rule reruns this.
    pub fn own_rule(&self, component: &str) -> Option<SavedRule> {
        self.s.note(Dep::Backing((self.id, component.to_string())));
        self.s.g.saved_rule(self.id, component)
    }

    /// Whether this node has this component stored on it, rather than worked out.
    pub fn own_is_stored(&self, component: &str) -> bool {
        self.s.note(Dep::Backing((self.id, component.to_string())));
        self.s.g.stored(self.id, component).is_some()
    }

    // ---------- typed: shortcuts over the runtime primitives below ----------

    /// A component on this node, if it has one.
    pub fn own<C: Component>(&self) -> Cell<C> {
        self.s.read(self.id)
    }

    /// A component this node must have.
    pub fn own_value<C: Component>(&self) -> Result<C, Error> {
        self.value(self.id)
    }

    /// A component on a node this rule's links reach, if it has one.
    pub fn get<C: Component>(&self, e: Entity) -> Cell<C> {
        self.may_read(e)?;
        self.s.read(e)
    }

    /// A component that node must have.
    pub fn value<C: Component>(&self, e: Entity) -> Result<C, Error> {
        self.get(e)?.ok_or_else(|| Error::Missing((e, C::NAME.to_string())))
    }

    /// The nodes linking here by `L`.
    pub fn sources<L: Component>(&self) -> Result<Vec<Entity>, Error> {
        Ok(self.incoming_runtime(L::NAME)?.into_iter().map(|(e, _)| e).collect())
    }

    /// The nodes this links to by `L`.
    pub fn targets<L: Component>(&self) -> Result<Vec<Entity>, Error> {
        Ok(self.outgoing_runtime(L::NAME)?.into_iter().map(|(e, _)| e).collect())
    }

    /// As `sources`, with what each link holds.
    pub fn incoming<L: Component>(&self) -> Result<Vec<(Entity, L)>, Error> {
        self.incoming_runtime(L::NAME)?.into_iter().map(|(e, c)| decode(&(e, L::NAME.to_string()), &c.value).map(|l| (e, l))).collect()
    }

    /// As `targets`, with what each link holds.
    pub fn outgoing<L: Component>(&self) -> Result<Vec<(Entity, L)>, Error> {
        self.outgoing_runtime(L::NAME)?.into_iter().map(|(e, c)| decode(&(e, L::NAME.to_string()), &c.value).map(|l| (e, l))).collect()
    }

    /// Every node with `C` in a declared scan, with its value. Enumerating by type guarantees
    /// each has it.
    pub fn with<C: Component>(&self) -> Result<Vec<(Entity, C)>, Error> {
        let mut out = Vec::new();
        for e in self.scan(C::NAME)? {
            if let Some(v) = self.s.read::<C>(e)? {
                out.push((e, v));
            }
        }
        Ok(out)
    }

    // ---------- runtime: everything, for components Rust has no type for ----------

    pub fn get_runtime(&self, e: Entity, component: &str) -> Cell<Runtime> {
        self.may_read(e)?;
        self.s.read_runtime(e, component)
    }

    pub fn incoming_runtime(&self, kind: &str) -> Result<Vec<(Entity, Runtime)>, Error> {
        if !self.inputs.incoming.iter().any(|k| k == kind) && !self.like().is_some_and(|r| r.inputs.incoming.iter().any(|k| k == kind)) {
            return Err(self.undeclared(format!("incoming {kind} links")));
        }
        self.s.note(Dep::Into(self.id, kind.to_string()));
        let g = self.s.g;
        let sources = g.into.get(&self.id).into_iter().flatten();
        Ok(sources.filter_map(|f| Some((*f, Runtime::new(kind, g.links.get(&(*f, self.id))?.get(kind)?.clone())))).collect())
    }

    pub fn outgoing_runtime(&self, kind: &str) -> Result<Vec<(Entity, Runtime)>, Error> {
        self.outgoing_from(self.id, kind)
    }

    /// The same, from where this rule is written: see `origin`.
    pub fn outgoing_from_origin(&self, kind: &str) -> Result<Vec<(Entity, Runtime)>, Error> {
        self.outgoing_from(self.origin, kind)
    }

    fn outgoing_from(&self, at: Entity, kind: &str) -> Result<Vec<(Entity, Runtime)>, Error> {
        if !self.inputs.outgoing.iter().any(|k| k == kind) && !self.like().is_some_and(|r| r.inputs.outgoing.iter().any(|k| k == kind)) {
            return Err(self.undeclared(format!("outgoing {kind} links")));
        }
        self.s.note(Dep::From(at, kind.to_string()));
        let g = self.s.g;
        let targets = g.from.get(&at).into_iter().flatten();
        Ok(targets.filter_map(|t| Some((*t, Runtime::new(kind, g.links.get(&(at, *t))?.get(kind)?.clone())))).collect())
    }

    pub fn scan(&self, component: &str) -> Result<Vec<Entity>, Error> {
        if !self.inputs.scans.iter().any(|c| c == component) && !self.like().is_some_and(|r| r.inputs.scans.iter().any(|c| c == component)) {
            return Err(self.undeclared(format!("a scan of {component}")));
        }
        self.s.note(Dep::Scan(component.to_string()));
        let g = self.s.g;
        Ok(g.entities.iter().filter(|e| g.backing(**e, component).is_some()).copied().collect())
    }
}

fn decode<C: DeserializeOwned>(cell: &Key, v: &Value) -> Result<C, Error> {
    serde_json::from_value(v.clone()).map_err(|e| Error::Malformed { cell: cell.clone(), detail: e.to_string() })
}
