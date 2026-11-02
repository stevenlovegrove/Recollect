//! Toys refer to nodes by name to stay readable, which is sugar over handles: the core only ever
//! sees an `Entity`, and each name here is an ordinary `Name` component. `tests/handles.rs` uses
//! handles directly.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use recollect_graph::worlds::Bool;
use recollect_graph::{Cell, Component, Entity, Error, Graph, Name, Registry, Storage};
use recollect_systems::formulas::{EXPR, Label};

#[derive(Default)]
pub struct N {
    pub g: Graph,
    pub registry: Arc<Registry>,
    ids: BTreeMap<String, Entity>,
    names: HashMap<String, String>,
}

impl Deref for N {
    type Target = Graph;
    fn deref(&self) -> &Graph {
        &self.g
    }
}

impl DerefMut for N {
    fn deref_mut(&mut self) -> &mut Graph {
        &mut self.g
    }
}

impl N {
    pub fn new() -> Self {
        Self::default()
    }

    /// With the rules a system installs.
    pub fn with(install: fn(&mut Registry)) -> Self {
        let mut reg = Registry::new();
        install(&mut reg);
        let registry = Arc::new(reg);
        N { g: Graph::with_registry(registry.clone()), registry, ..Self::default() }
    }

    /// Save to real files, load into a new graph, and check every cell comes back the same.
    pub fn assert_round_trips(&self) -> Graph {
        let dir = tempfile::tempdir().unwrap();
        self.g.save_dir(dir.path()).unwrap();
        let (loaded, problems) = Graph::load_dir(dir.path(), self.registry.clone()).unwrap();
        assert_eq!(problems, vec![], "nothing should need attention");
        assert_eq!(self.g.entities().collect::<Vec<_>>(), loaded.entities().collect::<Vec<_>>());
        for e in self.g.entities() {
            assert_eq!(self.g.components_of(e), loaded.components_of(e), "{}", self.name_of(e));
            for c in self.g.components_of(e) {
                match (self.g.get_runtime(e, &c), loaded.get_runtime(e, &c)) {
                    // Which cell is named as starting a cycle can depend on what was asked first.
                    (Err(Error::Cycle(_)), Err(Error::Cycle(_))) => {}
                    (a, b) => assert_eq!(a, b, "{c} of {}", self.name_of(e)),
                }
            }
        }
        assert_eq!(self.g.save().unwrap(), loaded.save().unwrap(), "and saving it again gives the same thing");
        loaded
    }

    /// The node with this name, created if it's new.
    pub fn e(&mut self, name: &str) -> Entity {
        if let Some(e) = self.ids.get(name) {
            return *e;
        }
        let e = self.g.spawn();
        self.g.set(e, Name(name.to_string()));
        self.ids.insert(name.to_string(), e);
        self.names.insert(e.to_string(), name.to_string());
        e
    }

    pub fn id(&self, name: &str) -> Entity {
        *self.ids.get(name).unwrap_or_else(|| panic!("no node named {name}"))
    }

    pub fn name_of(&self, e: Entity) -> String {
        self.label(&e.to_string())
    }

    /// Handles to names, leaving anything else alone.
    pub fn label(&self, s: &str) -> String {
        self.names.get(s).cloned().unwrap_or_else(|| s.to_string())
    }

    pub fn set<C: Component>(&mut self, name: &str, storage: impl Into<Storage<C>>) -> &mut Self {
        let e = self.e(name);
        self.g.set(e, storage);
        self
    }

    pub fn link<L: Component>(&mut self, from: &str, to: &str, v: L) -> &mut Self {
        let (a, b) = (self.e(from), self.e(to));
        self.g.link(a, b, v);
        self
    }

    /// This node's value, as a formula.
    pub fn formula(&mut self, name: &str, text: &str) -> &mut Self {
        let e = self.e(name);
        self.g.set_rule(e, "value", EXPR, serde_json::json!({ "text": text }));
        self
    }

    /// What a formula on `from` calls `to`.
    pub fn input(&mut self, from: &str, to: &str, label: &str) -> &mut Self {
        self.link(from, to, Label(label.to_string()))
    }

    pub fn unlink<L: Component>(&mut self, from: &str, to: &str) -> &mut Self {
        let (a, b) = (self.id(from), self.id(to));
        self.g.unlink::<L>(a, b);
        self
    }

    pub fn remove<C: Component>(&mut self, name: &str) -> &mut Self {
        let e = self.id(name);
        self.g.remove::<C>(e);
        self
    }

    pub fn get<C: Component>(&self, name: &str) -> Cell<C> {
        self.g.get::<C>(self.id(name))
    }

    pub fn show(&self, b: &Bool) -> String {
        b.show_with(&|s| self.label(s))
    }

    /// One world, by name.
    pub fn world(&self, picks: &[(&str, &str)]) -> BTreeMap<String, String> {
        picks.iter().map(|(d, o)| (self.id(d).to_string(), self.ids.get(*o).map_or(o.to_string(), |e| e.to_string()))).collect()
    }

    /// The nodes whose rules have run since this was last called, sorted, by name.
    pub fn ran(&self) -> Vec<String> {
        let mut r: Vec<String> = self.g.take_ran().into_iter().map(|(e, _)| self.name_of(e)).collect();
        r.sort();
        r
    }

    /// The nodes whose values have been worked out since this was last called, sorted, by name.
    /// Types run too, but only when something about a formula's types might have changed.
    pub fn ran_values(&self) -> Vec<String> {
        let mut r: Vec<String> = self.g.take_ran().into_iter().filter(|(_, c)| c == "value").map(|(e, _)| self.name_of(e)).collect();
        r.sort();
        r
    }

    /// The same, with which component ran.
    pub fn ran_cells(&self) -> Vec<String> {
        self.g.take_ran().into_iter().map(|(e, c)| format!("{c}({})", self.name_of(e))).collect()
    }
}
