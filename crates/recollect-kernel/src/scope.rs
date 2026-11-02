//! Scope: where a node belongs, derived rather than stored. A node's scope is its immediate
//! dominator over the links declared as scoping: the nearest node every path from the top must
//! pass through. One parent means it lives inside that parent. Two parents means it lives
//! inside the closest thing they share, so adding a parent moves it with nothing to maintain.
//!
//! Views are pure transforms of a graph: filter it, collapse scopes, look inside one. What is
//! expanded is the viewer's state and never touches the store.

use std::collections::{BTreeMap, BTreeSet};

use crate::compute::{Cell, Cx, Derived};
use crate::store::{Id, Store};

/// The nearest common scope of a node's parents, or `None` at the top level.
pub struct Scope;
impl Cell for Scope {
    const NAME: &'static str = "scope";
    type Value = Option<Id>;
    fn eval(cx: &Cx, id: &str) -> Option<Id> {
        let parents = cx.scoping_parents(id);
        let mut common: Option<Id> = parents.first().map(|p| p.to_string());
        for p in parents.iter().skip(1) {
            common = meet(cx, common?.as_str(), p);
        }
        common
    }
}

/// How many scopes a node is inside.
pub struct ScopeDepth;
impl Cell for ScopeDepth {
    const NAME: &'static str = "scope_depth";
    type Value = usize;
    fn eval(cx: &Cx, id: &str) -> usize {
        cx.cell::<Scope>(id).map_or(0, |p| cx.cell::<ScopeDepth>(&p) + 1)
    }
}

/// The closest node containing both, each counting as containing itself.
fn meet(cx: &Cx, a: &str, b: &str) -> Option<Id> {
    let (mut a, mut b) = (a.to_string(), b.to_string());
    let (mut da, mut db) = (cx.cell::<ScopeDepth>(&a), cx.cell::<ScopeDepth>(&b));
    while a != b {
        if da >= db {
            a = cx.cell::<Scope>(&a)?;
            da = da.checked_sub(1)?;
        } else {
            b = cx.cell::<Scope>(&b)?;
            db = db.checked_sub(1)?;
        }
    }
    Some(a)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewNode {
    /// Nodes folded into this one, not counting itself.
    pub hidden: usize,
    /// Outside the scope being viewed, shown only because a link reaches it.
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewEdge {
    /// The union of the components of the links this edge stands for.
    pub components: BTreeSet<String>,
    /// How many links it stands for.
    pub links: usize,
}

/// What a viewer sees. Each transform takes one and returns another, so they compose.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewGraph {
    pub nodes: BTreeMap<Id, ViewNode>,
    pub edges: BTreeMap<(Id, Id), ViewEdge>,
}

impl ViewGraph {
    pub fn of(s: &Store) -> Self {
        ViewGraph {
            nodes: s.entities.keys().map(|id| (id.clone(), ViewNode { hidden: 0, external: false })).collect(),
            edges: s.links.iter().map(|(k, c)| (k.clone(), ViewEdge { components: c.keys().cloned().collect(), links: 1 })).collect(),
        }
    }

    /// Keep the nodes that pass, and the edges between them.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) -> Self {
        let nodes: BTreeMap<Id, ViewNode> = self.nodes.iter().filter(|(id, _)| keep(id)).map(|(k, v)| (k.clone(), v.clone())).collect();
        let edges = self.edges.iter().filter(|((f, t), _)| nodes.contains_key(f) && nodes.contains_key(t)).map(|(k, v)| (k.clone(), v.clone())).collect();
        ViewGraph { nodes, edges }
    }

    /// Fold every node into its outermost enclosing scope that isn't expanded. Links reaching
    /// inside are redrawn to what's visible, so none is lost; those wholly inside one folded
    /// node disappear with it.
    pub fn collapse(&self, d: &Derived, expanded: &BTreeSet<Id>) -> Self {
        let shown = |id: &str| -> Id {
            let mut chain = vec![id.to_string()];
            while let Some(Some(p)) = d.of::<Scope>(chain.last().unwrap()) {
                if chain.contains(p) {
                    break;
                }
                chain.push(p.clone());
            }
            chain.iter().rev().find(|a| self.nodes.contains_key(*a) && !expanded.contains(*a)).cloned().unwrap_or_else(|| id.to_string())
        };
        let mut nodes: BTreeMap<Id, ViewNode> = BTreeMap::new();
        for (id, n) in &self.nodes {
            let at = shown(id);
            let folded = at != *id;
            let entry = nodes.entry(at).or_insert(ViewNode { hidden: 0, external: n.external });
            entry.hidden += n.hidden + usize::from(folded);
        }
        let mut edges: BTreeMap<(Id, Id), ViewEdge> = BTreeMap::new();
        for ((f, t), e) in &self.edges {
            let (sf, st) = (shown(f), shown(t));
            if sf == st {
                continue;
            }
            let entry = edges.entry((sf, st)).or_insert(ViewEdge { components: BTreeSet::new(), links: 0 });
            entry.components.extend(e.components.iter().cloned());
            entry.links += e.links;
        }
        ViewGraph { nodes, edges }
    }

    /// Look inside a scope: the node and everything within it, plus whatever links reach out,
    /// marked external so the boundary stays visible.
    pub fn within(&self, d: &Derived, root: &str) -> Self {
        let inside = |id: &str| -> bool {
            let mut cur = id.to_string();
            let mut steps = 0;
            loop {
                if cur == root {
                    return true;
                }
                match d.of::<Scope>(&cur) {
                    Some(Some(p)) if steps <= self.nodes.len() => cur = p.clone(),
                    _ => return false,
                }
                steps += 1;
            }
        };
        let mut nodes: BTreeMap<Id, ViewNode> = self.nodes.iter().filter(|(id, _)| inside(id)).map(|(k, v)| (k.clone(), v.clone())).collect();
        let mut edges = BTreeMap::new();
        for ((f, t), e) in &self.edges {
            let (fi, ti) = (inside(f), inside(t));
            if fi || ti {
                edges.insert((f.clone(), t.clone()), e.clone());
                let other = if fi { t } else { f };
                if !(fi && ti) {
                    nodes.entry(other.clone()).or_insert(ViewNode { hidden: 0, external: true });
                }
            }
        }
        ViewGraph { nodes, edges }
    }
}
