//! Views are pure transforms of a graph: filter it, collapse scopes, look inside one. What's
//! expanded is the viewer's state and never touches the graph.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::{Entity, Graph};

use crate::plan::{Scope, ScopeDepth};

#[derive(Debug, Clone, PartialEq)]
pub struct ViewNode {
    /// Nodes folded into this one, not counting itself.
    pub hidden: usize,
    /// Outside the scope being viewed, shown only because a link reaches it.
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewEdge {
    /// The kinds of the links this edge stands for.
    pub kinds: BTreeSet<String>,
    /// How many links it stands for.
    pub links: usize,
}

/// What a viewer sees. Each transform takes one and returns another, so they compose.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewGraph {
    pub nodes: BTreeMap<Entity, ViewNode>,
    pub edges: BTreeMap<(Entity, Entity), ViewEdge>,
}

/// What a node is inside, nearest first. A loop from a hand edit is an error in `ScopeDepth`, so a
/// node on or under one reads as being at the top level, and a view can still be drawn.
fn scopes(g: &Graph, e: Entity) -> Vec<Entity> {
    let mut chain = Vec::new();
    if g.get::<ScopeDepth>(e).is_err() {
        return chain;
    }
    let mut cur = e;
    while let Ok(Some(Scope(up))) = g.get::<Scope>(cur) {
        chain.push(up);
        cur = up;
    }
    chain
}

impl ViewGraph {
    pub fn of(g: &Graph) -> Self {
        ViewGraph {
            nodes: g.entities().map(|e| (e, ViewNode { hidden: 0, external: false })).collect(),
            edges: g.links().map(|(f, t, bag)| ((f, t), ViewEdge { kinds: bag.keys().cloned().collect(), links: 1 })).collect(),
        }
    }

    /// Keep the nodes that pass, and the edges between them.
    pub fn retain(&self, keep: impl Fn(Entity) -> bool) -> Self {
        let nodes: BTreeMap<Entity, ViewNode> = self.nodes.iter().filter(|(e, _)| keep(**e)).map(|(k, v)| (*k, v.clone())).collect();
        let edges = self.edges.iter().filter(|((f, t), _)| nodes.contains_key(f) && nodes.contains_key(t)).map(|(k, v)| (*k, v.clone())).collect();
        ViewGraph { nodes, edges }
    }

    /// Fold every node into its outermost enclosing scope that isn't expanded. Links reaching
    /// inside are redrawn to what's visible, so none is lost, and those wholly inside one folded
    /// node disappear with it.
    pub fn collapse(&self, g: &Graph, expanded: &BTreeSet<Entity>) -> Self {
        let shown = |e: Entity| -> Entity {
            let mut chain = vec![e];
            chain.extend(scopes(g, e));
            chain.iter().rev().find(|a| self.nodes.contains_key(*a) && !expanded.contains(*a)).copied().unwrap_or(e)
        };
        let mut nodes: BTreeMap<Entity, ViewNode> = BTreeMap::new();
        for (e, n) in &self.nodes {
            let at = shown(*e);
            let entry = nodes.entry(at).or_insert(ViewNode { hidden: 0, external: n.external });
            entry.hidden += n.hidden + usize::from(at != *e);
        }
        let mut edges: BTreeMap<(Entity, Entity), ViewEdge> = BTreeMap::new();
        for ((f, t), edge) in &self.edges {
            let (sf, st) = (shown(*f), shown(*t));
            if sf == st {
                continue;
            }
            let entry = edges.entry((sf, st)).or_insert(ViewEdge { kinds: BTreeSet::new(), links: 0 });
            entry.kinds.extend(edge.kinds.iter().cloned());
            entry.links += edge.links;
        }
        ViewGraph { nodes, edges }
    }

    /// Look inside a scope: the node and everything within it, plus whatever links reach out,
    /// marked external so the boundary stays visible.
    pub fn within(&self, g: &Graph, root: Entity) -> Self {
        let inside = |e: Entity| e == root || scopes(g, e).contains(&root);
        let mut nodes: BTreeMap<Entity, ViewNode> = self.nodes.iter().filter(|(e, _)| inside(**e)).map(|(k, v)| (*k, v.clone())).collect();
        let mut edges = BTreeMap::new();
        for ((f, t), edge) in &self.edges {
            let (fi, ti) = (inside(*f), inside(*t));
            if fi || ti {
                edges.insert((*f, *t), edge.clone());
                if !(fi && ti) {
                    nodes.entry(if fi { *t } else { *f }).or_insert(ViewNode { hidden: 0, external: true });
                }
            }
        }
        ViewGraph { nodes, edges }
    }
}
