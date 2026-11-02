//! What components mean. Rust types give typed reads; runtime declarations give the same rules
//! to components Rust has no type for. A component nobody declares is kept and has no effect.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::store::{Id, Store, Violation};

pub trait Component: DeserializeOwned + 'static {
    const NAME: &'static str;
}

pub trait LinkComponent: DeserializeOwned + 'static {
    const NAME: &'static str;
    /// A component the link's target must have.
    const TO: Option<&'static str> = None;
}

macro_rules! entity {
    ($t:ident, $name:literal) => {
        impl Component for $t {
            const NAME: &'static str = $name;
        }
    };
}

/// Work that can be picked up.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Task {
    pub estimate: Option<f64>,
}
entity!(Task, "task");

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Goal {}
entity!(Goal, "goal");

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Question {}
entity!(Question, "question");

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Risk {}
entity!(Risk, "risk");

/// A gap: something here isn't specified yet. It can't be done or chosen, so whatever requires
/// it stays incomplete until someone removes it, which says the gap is closed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Unknown {}
entity!(Unknown, "unknown");

/// Stated complete.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Done {}
entity!(Done, "done");

/// Deliberately out of the picture.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Dropped {}
entity!(Dropped, "dropped");

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Decision {
    pub chosen: Option<Id>,
}
entity!(Decision, "decision");

/// Child → parent: AND.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PartOf {}
impl LinkComponent for PartOf {
    const NAME: &'static str = "part_of";
}

/// Option → decision: OR.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Alternative {
    pub ruled_out: Option<Value>,
    pub weight: Option<f64>,
}
impl LinkComponent for Alternative {
    const NAME: &'static str = "alternative";
    const TO: Option<&'static str> = Some(Decision::NAME);
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WaitFor {
    Done,
    Decided,
}

/// From waits for to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DependsOn {
    pub on: Option<WaitFor>,
}
impl LinkComponent for DependsOn {
    const NAME: &'static str = "depends_on";
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum On {
    Entity,
    Link,
}

/// One component's rules, as data, so a plugin with no Rust type can declare them too.
#[derive(Debug, Clone)]
pub struct ComponentDef {
    pub name: String,
    pub on: On,
    /// Has work of its own, which only `done` discharges.
    pub obligation: bool,
    /// A link holding this places its source inside its target's scope.
    pub scoping: bool,
    pub to: Option<String>,
    pub validate: Option<fn(&Value) -> Result<(), String>>,
}

impl ComponentDef {
    pub fn entity(name: &str) -> Self {
        ComponentDef { name: name.into(), on: On::Entity, obligation: false, scoping: false, to: None, validate: None }
    }

    pub fn link(name: &str) -> Self {
        ComponentDef { on: On::Link, ..Self::entity(name) }
    }

    pub fn typed<T: Component>() -> Self {
        ComponentDef { validate: Some(|v| serde_json::from_value::<T>(v.clone()).map(drop).map_err(|e| e.to_string())), ..Self::entity(T::NAME) }
    }

    pub fn typed_link<L: LinkComponent>() -> Self {
        ComponentDef { to: L::TO.map(String::from), validate: Some(|v| serde_json::from_value::<L>(v.clone()).map(drop).map_err(|e| e.to_string())), ..Self::link(L::NAME) }
    }

    pub fn obligation(mut self) -> Self {
        self.obligation = true;
        self
    }

    pub fn scoping(mut self) -> Self {
        self.scoping = true;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub defs: BTreeMap<String, ComponentDef>,
}

impl Default for Schema {
    fn default() -> Self {
        let mut s = Schema { defs: BTreeMap::new() };
        s.declare(ComponentDef::typed::<Task>().obligation());
        s.declare(ComponentDef::typed::<Goal>().obligation());
        s.declare(ComponentDef::typed::<Question>().obligation());
        s.declare(ComponentDef::typed::<Risk>().obligation());
        s.declare(ComponentDef::typed::<Unknown>().obligation());
        s.declare(ComponentDef::typed::<Done>());
        s.declare(ComponentDef::typed::<Dropped>());
        s.declare(ComponentDef::typed::<Decision>());
        s.declare(ComponentDef::typed_link::<PartOf>().scoping());
        s.declare(ComponentDef::typed_link::<Alternative>().scoping());
        s.declare(ComponentDef::typed_link::<DependsOn>());
        s
    }
}

impl Schema {
    pub fn declare(&mut self, def: ComponentDef) {
        self.defs.insert(def.name.clone(), def);
    }

    pub fn is_obligation(&self, name: &str) -> bool {
        self.defs.get(name).is_some_and(|d| d.obligation)
    }

    pub fn is_scoping(&self, name: &str) -> bool {
        self.defs.get(name).is_some_and(|d| d.scoping)
    }

    fn invalid(&self, name: &str, v: &Value) -> Option<String> {
        self.defs.get(name)?.validate?(v).err()
    }

    /// The rules declared here, plus the kernel's structural ones.
    pub fn check(&self, s: &Store) -> Vec<Violation> {
        let mut out = s.check_structure();
        for (id, comps) in &s.entities {
            for (c, v) in comps {
                if self.defs.get(c).is_some_and(|d| d.on != On::Entity) {
                    out.push(Violation { rule: "component_on_wrong_target", detail: format!("{id} holds {c}, which belongs on a link") });
                }
                if let Some(e) = self.invalid(c, v) {
                    out.push(Violation { rule: "malformed_component", detail: format!("{id}.{c}: {e}") });
                }
            }
        }
        for ((f, t), comps) in &s.links {
            for (c, v) in comps {
                let Some(def) = self.defs.get(c) else { continue };
                if def.on != On::Link {
                    out.push(Violation { rule: "component_on_wrong_target", detail: format!("{f} → {t} holds {c}, which belongs on an entity") });
                }
                if let Some(e) = self.invalid(c, v) {
                    out.push(Violation { rule: "malformed_component", detail: format!("{f} → {t}.{c}: {e}") });
                }
                if let Some(need) = &def.to
                    && !s.has(t, need)
                {
                    out.push(Violation { rule: "link_target_lacks_component", detail: format!("{f} → {t} holds {c}, but {t} has no {need}") });
                }
            }
        }
        out
    }

    /// What compute sees. A hand edit can leave a malformed value on disk, and a typed read
    /// must never meet one, so it counts as absent here and is reported. The store keeps it.
    pub fn normalise<'a>(&self, s: &'a Store) -> (Cow<'a, Store>, Vec<Violation>) {
        let mut problems = Vec::new();
        let mut bad_entities = Vec::new();
        let mut bad_links = Vec::new();
        for (id, comps) in &s.entities {
            for (c, v) in comps {
                if let Some(e) = self.invalid(c, v) {
                    problems.push(Violation { rule: "malformed_component", detail: format!("{id}.{c}: {e}") });
                    bad_entities.push((id.clone(), c.clone()));
                }
            }
        }
        for (k, comps) in &s.links {
            for (c, v) in comps {
                if let Some(e) = self.invalid(c, v) {
                    problems.push(Violation { rule: "malformed_component", detail: format!("{} → {}.{c}: {e}", k.0, k.1) });
                    bad_links.push((k.clone(), c.clone()));
                }
            }
        }
        if problems.is_empty() {
            return (Cow::Borrowed(s), problems);
        }
        let mut view = s.clone();
        for (id, c) in bad_entities {
            view.entities.get_mut(&id).unwrap().remove(&c);
        }
        for (k, c) in bad_links {
            view.links.get_mut(&k).unwrap().remove(&c);
        }
        (Cow::Owned(view), problems)
    }
}
