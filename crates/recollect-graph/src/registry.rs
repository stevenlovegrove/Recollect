//! What the systems say there is: components, and the cells each brings.
//!
//! A component is a facet of an entity, such as Decision or Task, and is what a badge shows. It brings
//! cells, each with a default for when nothing says otherwise: a value, a formula, or something built
//! in. It can require other components on the same entity, and fill in what one of their cells is when
//! nothing says otherwise, as Decision says how a choice lands. A component on an edge can bring
//! components to the edge's ends. So nothing here applies to every node: a cell is only there on an
//! entity that has its component.
//!
//! Registration is explicit. A system is a function that adds to a `Registry`, so what's active can be
//! read from one place.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::graph::{Cell, CellType, Computed, Cx, Entity, Error, Graph};
use crate::shown::Shown;
use crate::worlds::Bool;

/// Whether a value has the right shape, and if not, what's wrong with it.
pub type Validate = fn(&Value) -> Result<(), String>;

/// What a formula written on a cell becomes, or why it can't be one.
pub type Compile = Arc<dyn Fn(&str) -> Result<Arc<Computed>, String> + Send + Sync>;

/// A cell as a world value, for reading.
pub type ShownFn = Arc<dyn Fn(&Graph, Entity) -> Cell<Shown> + Send + Sync>;

/// The worlds an entity with some component allows, if it has something to say.
pub type ConstraintFn = Arc<dyn Fn(&Graph, Entity) -> Cell<Bool> + Send + Sync>;

/// What's wrong with one entity, in words.
pub type CheckFn = Arc<dyn Fn(&Cx) -> Result<Vec<String>, Error> + Send + Sync>;

/// What's worth saying about a graph as a whole, which no single entity's cells can see.
pub type AdviceFn = fn(&Graph) -> Vec<Advice>;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advice {
    pub rule: String,
    pub on: Entity,
    pub detail: String,
}

/// Where a component goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Node,
    Edge,
    /// The graph as a whole, where something is worked out once however many nodes read it.
    Whole,
}

/// How much a check matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A commit that newly breaks this is refused.
    Invariant,
    /// Worth a look, but nothing is refused.
    Notice,
}

#[derive(Clone)]
pub struct CheckDef {
    pub name: String,
    pub severity: Severity,
}

impl CheckDef {
    /// The cell the check's findings are worked out in.
    pub fn cell(&self) -> String {
        format!("check/{}", self.name)
    }
}

/// A facet of an entity.
#[derive(Clone, Serialize)]
pub struct ComponentDef {
    pub name: String,
    pub system: String,
    pub place: Place,
    pub about: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Other components an entity with this one has too.
    pub requires: Vec<String>,
    /// For an edge: components its source has because of it.
    pub source: Vec<String>,
    /// For an edge: components its target has because of it.
    pub target: Vec<String>,
    /// It says something about this entity alone, such as which option was chosen, so it can't come
    /// from a kind.
    pub per_entity: bool,
    /// Every node has it, and it can't be taken away.
    pub everywhere: bool,
    /// An edge that gives a graph its shape, rather than just saying something about two nodes.
    pub arranges: bool,
    /// For an edge that gives a graph its shape: what it points at goes beneath, not above.
    pub reversed: bool,
    /// An edge that says what comes first. It isn't containment, but a map can be laid out by it.
    pub orders: bool,
    /// Its cells, in the order they're shown.
    pub cells: Vec<String>,
    #[serde(skip)]
    pub checks: Vec<CheckDef>,
    #[serde(skip)]
    pub constraint: Option<ConstraintFn>,
    /// What cells of the components it requires are, when nothing says otherwise, in place of their own
    /// defaults.
    #[serde(skip)]
    pub fills: BTreeMap<String, Default>,
}

impl ComponentDef {
    pub fn icon(&mut self, icon: &str) -> &mut Self {
        self.icon = Some(icon.to_string());
        self
    }

    pub fn requires(&mut self, component: &str) -> &mut Self {
        if !self.requires.iter().any(|r| r == component) {
            self.requires.push(component.to_string());
        }
        self
    }

    /// For an edge: its source has this too.
    pub fn source_has(&mut self, component: &str) -> &mut Self {
        self.source.push(component.to_string());
        self
    }

    /// For an edge: its target has this too.
    pub fn target_has(&mut self, component: &str) -> &mut Self {
        self.target.push(component.to_string());
        self
    }

    pub fn per_entity(&mut self) -> &mut Self {
        self.per_entity = true;
        self
    }

    pub fn everywhere(&mut self) -> &mut Self {
        self.everywhere = true;
        self
    }

    pub fn arranges(&mut self) -> &mut Self {
        self.arranges = true;
        self
    }

    pub fn reversed(&mut self) -> &mut Self {
        self.reversed = true;
        self
    }

    pub fn orders(&mut self) -> &mut Self {
        self.orders = true;
        self
    }

    /// Only the worlds this allows, on each entity with this component, count.
    pub fn constraint(&mut self, f: impl Fn(&Graph, Entity) -> Cell<Bool> + Send + Sync + 'static) -> &mut Self {
        self.constraint = Some(Arc::new(f));
        self
    }
}

/// What a cell is when nothing says otherwise.
#[derive(Clone)]
pub enum Default {
    Value(Value),
    Formula(String),
    Builtin(Arc<Computed>),
}

/// A cell a component brings.
#[derive(Clone, Serialize)]
pub struct CellDef {
    pub name: String,
    pub component: String,
    pub about: String,
    /// Plumbing, which only makes sense to what reads it.
    pub internal: bool,
    /// Worth a line on a card.
    pub summary: bool,
    /// For a view to read, rather than a person.
    pub quiet: bool,
    /// It's either true or false, so it can be read and set as a tick.
    pub truth: bool,
    /// It says something about this entity alone, such as its name or its answer, so it isn't taken
    /// from a kind.
    pub per_entity: bool,
    /// What a new one starts as, when it's typed in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<Value>,
    #[serde(skip)]
    pub default: Option<Default>,
    #[serde(skip)]
    pub compile: Option<Compile>,
    #[serde(skip)]
    pub validate: Option<Validate>,
    #[serde(skip)]
    pub shown: Option<ShownFn>,
}

impl CellDef {
    /// Whether it's worked out, unless something says otherwise.
    pub fn computed(&self) -> bool {
        matches!(self.default, Some(Default::Formula(_) | Default::Builtin(_)))
    }

    /// Whether it can hold a formula.
    pub fn formulas(&self) -> bool {
        self.compile.is_some()
    }

    /// Whether anything can be written on it: a value, or a formula.
    pub fn settable(&self) -> bool {
        self.compile.is_some() || !self.computed()
    }
}

/// Says more about a cell as it's registered.
pub struct CellBuilder<'r, C> {
    def: &'r mut CellDef,
    _c: PhantomData<C>,
}

impl<C: CellType> CellBuilder<'_, C> {
    /// Worked out by this when nothing says otherwise.
    pub fn builtin(self, f: impl Fn(&Cx) -> Cell<C> + Send + Sync + 'static) -> Self {
        self.def.default = Some(Default::Builtin(Computed::typed(&format!("{} default", C::NAME), f)));
        self
    }

    pub fn default_value(self, v: C) -> Self {
        self.def.default = Some(Default::Value(serde_json::to_value(&v).expect("cells serialise")));
        self
    }

    /// Worked out by this formula when nothing says otherwise. It's compiled by `compiles`.
    pub fn default_formula(self, text: &str) -> Self {
        self.def.default = Some(Default::Formula(text.to_string()));
        self
    }

    /// How a formula written on this cell is made.
    pub fn compiles(self, c: Compile) -> Self {
        self.def.compile = Some(c);
        self
    }

    pub fn shown(self, f: fn(&C) -> Shown) -> Self {
        self.def.shown = Some(Arc::new(move |g, e| Ok(g.get::<C>(e)?.map(|c| f(&c)))));
        self
    }

    pub fn validate(self, v: Validate) -> Self {
        self.def.validate = Some(v);
        self
    }

    pub fn internal(self) -> Self {
        self.def.internal = true;
        self
    }

    pub fn summary(self) -> Self {
        self.def.summary = true;
        self
    }

    pub fn quiet(self) -> Self {
        self.def.quiet = true;
        self
    }

    pub fn truth(self) -> Self {
        self.def.truth = true;
        self
    }

    pub fn per_entity(self) -> Self {
        self.def.per_entity = true;
        self
    }

    pub fn initial(self, v: Value) -> Self {
        self.def.initial = Some(v);
        self
    }
}

/// Everything the systems say there is.
#[derive(Default)]
pub struct Registry {
    components: BTreeMap<String, ComponentDef>,
    cells: BTreeMap<String, CellDef>,
    /// How a formula on one of the workspace's own cells is made: a cell no component brings.
    free: Option<Compile>,
    /// The edge from a node to its kind, if nodes can take what they don't say from one.
    pub(crate) inherits_through: Option<String>,
    advice: Vec<AdviceFn>,
    build: String,
    current: String,
}

fn typed_validate<C: CellType>(v: &Value) -> Result<(), String> {
    serde_json::from_value::<C>(v.clone()).map(drop).map_err(|e| e.to_string())
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Which system is saying what follows.
    pub fn system(&mut self, name: &str) -> &mut Self {
        self.current = name.to_string();
        self
    }

    fn define(&mut self, name: &str, place: Place, about: &str) -> &mut ComponentDef {
        let system = self.current.clone();
        self.components.entry(name.to_string()).or_insert_with(|| ComponentDef {
            name: name.to_string(),
            system,
            place,
            about: about.to_string(),
            icon: None,
            requires: Vec::new(),
            source: Vec::new(),
            target: Vec::new(),
            per_entity: false,
            everywhere: false,
            arranges: false,
            reversed: false,
            orders: false,
            cells: Vec::new(),
            checks: Vec::new(),
            constraint: None,
            fills: BTreeMap::new(),
        })
    }

    /// A component that goes on nodes. Saying one again is harmless, and gives the one already there.
    pub fn component(&mut self, name: &str, about: &str) -> &mut ComponentDef {
        self.define(name, Place::Node, about)
    }

    /// A component that goes on edges.
    pub fn edge(&mut self, name: &str, about: &str) -> &mut ComponentDef {
        self.define(name, Place::Edge, about)
    }

    /// A component the graph as a whole has.
    pub fn whole(&mut self, name: &str, about: &str) -> &mut ComponentDef {
        self.define(name, Place::Whole, about)
    }

    /// A cell this component brings. Values written on it have to read as `C`.
    pub fn cell<C: CellType>(&mut self, component: &str, about: &str) -> CellBuilder<'_, C> {
        assert!(self.components.contains_key(component), "{component} has to be registered before its cells");
        let comp = self.components.get_mut(component).expect("checked above");
        if !comp.cells.iter().any(|c| c == C::NAME) {
            comp.cells.push(C::NAME.to_string());
        }
        if let Some(existing) = self.cells.get(C::NAME) {
            assert_eq!(existing.component, component, "{} is already a cell of {}", C::NAME, existing.component);
        }
        let def = self.cells.entry(C::NAME.to_string()).or_insert_with(|| CellDef {
            name: C::NAME.to_string(),
            component: component.to_string(),
            about: about.to_string(),
            internal: false,
            summary: false,
            quiet: false,
            truth: false,
            per_entity: false,
            initial: None,
            default: None,
            compile: None,
            validate: Some(typed_validate::<C>),
            shown: None,
        });
        CellBuilder { def, _c: PhantomData }
    }

    /// On an entity with `component`, a cell of a component it requires is worked out by this when nothing
    /// says otherwise, rather than by that cell's own default. Two components that fill the same cell
    /// shouldn't go together: the first by name wins.
    pub fn fill<C: CellType>(&mut self, component: &str, f: impl Fn(&Cx) -> Cell<C> + Send + Sync + 'static) -> &mut Self {
        let comp = self.components.get_mut(component).unwrap_or_else(|| panic!("{component} has to be registered before what it fills"));
        comp.fills.insert(C::NAME.to_string(), Default::Builtin(Computed::typed(&format!("{} for {component}", C::NAME), f)));
        self
    }

    /// The components an entity with this one has too, however indirectly, and this one.
    pub fn required_by(&self, component: &str) -> Vec<String> {
        let mut out = vec![component.to_string()];
        let mut i = 0;
        while i < out.len() {
            for r in self.components.get(&out[i]).map(|c| c.requires.clone()).unwrap_or_default() {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
            i += 1;
        }
        out
    }

    /// Something that must hold, or is worth a look, on every entity with this component. It's worked
    /// out in a cell of its own, so it's memoised, and only reruns what an edit reaches.
    pub fn check(&mut self, component: &str, name: &str, severity: Severity, f: impl Fn(&Cx) -> Result<Vec<String>, Error> + Send + Sync + 'static) -> &mut Self {
        let check = CheckDef { name: name.to_string(), severity };
        let cell = check.cell();
        let comp = self.components.get_mut(component).unwrap_or_else(|| panic!("{component} has to be registered before its checks"));
        comp.checks.push(check);
        let computed = Computed::json(&cell, move |cx| Ok(Some(serde_json::to_value(f(cx)?).expect("strings serialise"))));
        self.cells.insert(
            cell.clone(),
            CellDef { name: cell, component: component.to_string(), about: String::new(), internal: true, summary: false, quiet: false, truth: false, per_entity: true, initial: None, default: Some(Default::Builtin(computed)), compile: None, validate: None, shown: None },
        );
        self
    }

    /// How a formula on one of the workspace's own cells is made.
    pub fn free_formulas(&mut self, c: Compile) -> &mut Self {
        self.free = Some(c);
        self
    }

    /// Nodes take what they don't say from what they link to by this edge, as an instance does from its
    /// kind.
    pub fn inherit_through(&mut self, edge: &str) -> &mut Self {
        self.inherits_through = Some(edge.to_string());
        self
    }

    pub fn advice(&mut self, f: AdviceFn) -> &mut Self {
        // A system installed twice, as one is when another brings it, still only says a thing once.
        if !self.advice.iter().any(|known| *known as usize == f as usize) {
            self.advice.push(f);
        }
        self
    }

    /// Which code this is, such as a commit hash. Recorded in the log.
    pub fn build(&mut self, id: &str) -> &mut Self {
        self.build = id.to_string();
        self
    }

    pub fn build_id(&self) -> &str {
        &self.build
    }

    // ---------- reading ----------

    pub fn component_def(&self, name: &str) -> Option<&ComponentDef> {
        self.components.get(name)
    }

    pub fn cell_def(&self, name: &str) -> Option<&CellDef> {
        self.cells.get(name)
    }

    pub fn components(&self) -> impl Iterator<Item = &ComponentDef> {
        self.components.values()
    }

    pub fn cells(&self) -> impl Iterator<Item = &CellDef> {
        self.cells.values()
    }

    /// The component whose cell this is, if a component brings it.
    pub fn owner(&self, cell: &str) -> Option<&str> {
        self.cells.get(cell).map(|d| d.component.as_str())
    }

    /// Whether a node takes this component from its kind.
    pub fn inheritable(&self, component: &str) -> bool {
        self.components.get(component).is_none_or(|c| !c.per_entity && c.place == Place::Node)
    }

    /// Whether a node takes what's in this cell from its kind, when it doesn't say it itself.
    pub fn cell_inheritable(&self, cell: &str) -> bool {
        match self.cells.get(cell) {
            Some(def) => !def.per_entity && self.inheritable(&def.component),
            None => true,
        }
    }

    pub(crate) fn free(&self) -> Option<&Compile> {
        self.free.as_ref()
    }

    pub fn all_advice(&self, g: &Graph) -> Vec<Advice> {
        self.advice.iter().flat_map(|f| f(g)).collect()
    }

    /// Every check, with the component it's about.
    pub fn checks(&self) -> impl Iterator<Item = (&str, &CheckDef)> {
        self.components.values().flat_map(|c| c.checks.iter().map(move |k| (c.name.as_str(), k)))
    }

    /// The worlds that count: those every entity's constraints allow. `None` if nothing narrows them.
    pub fn constrained(&self, g: &Graph) -> Option<Bool> {
        let mut acc: Option<Bool> = None;
        for c in self.components.values() {
            let Some(f) = &c.constraint else { continue };
            for e in g.having(&c.name) {
                if let Ok(Some(b)) = f(g, e) {
                    acc = Some(match acc {
                        Some(a) => &a & &b,
                        None => b,
                    });
                }
            }
        }
        acc
    }
}
