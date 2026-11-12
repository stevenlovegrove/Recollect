//! The graph as files: one per node, holding the components it was given, what it says in its cells,
//! and its edges, which live with their source. Memos are never saved, and nor is anything a default
//! or a kind says: only what was written. Nothing here needs a system to be understood: a component or
//! cell nothing provides goes back out exactly as it came in.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Backed, Entity, Error, Graph, Key};
use crate::log::Violation;
use crate::registry::Registry;

/// What's written: values in `cells`, and formulas in `formulas`.
type Written = (BTreeMap<String, Value>, BTreeMap<String, String>);

fn split(written: BTreeMap<String, Backed>) -> Written {
    let (mut cells, mut formulas) = (BTreeMap::new(), BTreeMap::new());
    for (c, b) in written {
        match b {
            Backed::Value(v) => {
                cells.insert(c, v);
            }
            Backed::Formula(t) => {
                formulas.insert(c, t);
            }
            Backed::Builtin => {}
        }
    }
    (cells, formulas)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedEdge {
    pub to: Entity,
    pub components: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub formulas: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedNode {
    pub id: Entity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cells: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub formulas: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<SavedEdge>,
}

impl SavedNode {
    pub fn empty(id: Entity) -> Self {
        SavedNode { id, components: Vec::new(), cells: BTreeMap::new(), formulas: BTreeMap::new(), links: Vec::new() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedGraph {
    pub nodes: Vec<SavedNode>,
}

/// Something about loaded data that needs attention. The data is kept either way.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadProblem {
    /// A formula that can't be read.
    BrokenFormula(Key, Error),
    /// An edge to a node with no file. It's kept, and the node exists with nothing on it.
    DanglingLink { from: Entity, to: Entity },
    /// Invariants that don't hold right now, such as after a hand edit. They don't block other work.
    Standing(Vec<Violation>),
}

impl Graph {
    pub fn save(&self) -> SavedGraph {
        self.save_only(self.entities())
    }

    /// The written form of these nodes.
    pub(crate) fn save_only(&self, ids: impl Iterator<Item = Entity>) -> SavedGraph {
        let mut nodes = Vec::new();
        for id in ids {
            let (cells, formulas) = split(self.written(id));
            let mut links = Vec::new();
            for (edge, from, to) in self.edges().filter(|(_, f, _)| *f == id) {
                debug_assert_eq!(from, id);
                let (cells, formulas) = split(self.written(edge));
                links.push(SavedEdge { to, components: self.given(edge).into_iter().collect(), cells, formulas });
            }
            links.sort_by_key(|l| l.to);
            nodes.push(SavedNode { id, components: self.given(id).into_iter().collect(), cells, formulas, links });
        }
        SavedGraph { nodes }
    }

    pub fn load(saved: SavedGraph, registry: Arc<Registry>) -> (Graph, Vec<LoadProblem>) {
        let mut g = Graph::with_registry(registry);
        let mut problems = Vec::new();
        for node in &saved.nodes {
            g.adopt(node.id);
        }
        let write = |g: &mut Graph, e: Entity, cells: BTreeMap<String, Value>, formulas: BTreeMap<String, String>| {
            for (c, v) in cells {
                g.set_value(e, &c, v);
            }
            for (c, f) in formulas {
                g.set_formula(e, &c, &f);
            }
        };
        for node in saved.nodes {
            let id = node.id;
            for c in &node.components {
                g.add(id, c);
            }
            write(&mut g, id, node.cells, node.formulas);
            for link in node.links {
                if !g.contains(link.to) {
                    problems.push(LoadProblem::DanglingLink { from: id, to: link.to });
                    g.adopt(link.to);
                }
                let mut edge = None;
                for c in &link.components {
                    edge = Some(g.link(id, link.to, c));
                }
                if let Some(edge) = edge {
                    write(&mut g, edge, link.cells, link.formulas);
                }
            }
        }
        problems.extend(g.broken().into_iter().map(|(key, error)| LoadProblem::BrokenFormula(key, error)));
        (g, problems)
    }

    /// Write `nodes/<id>.yaml` into a directory, for tests and tools. Files whose text is unchanged
    /// aren't touched, and files for nodes that are gone are removed.
    pub fn save_dir(&self, dir: &Path) -> Result<(), DirError> {
        let saved = self.save();
        let nodes_dir = dir.join("nodes");
        fs::create_dir_all(&nodes_dir)?;
        let keep: BTreeSet<String> = saved.nodes.iter().map(node_path).collect();
        for node in &saved.nodes {
            let path = dir.join(node_path(node));
            let text = node_text(node);
            if !fs::read_to_string(&path).is_ok_and(|old| old == text) {
                fs::write(&path, text)?;
            }
        }
        for entry in fs::read_dir(&nodes_dir)? {
            let name = entry?.file_name().to_string_lossy().to_string();
            let rel = format!("nodes/{name}");
            // Only ever a file this wrote: one named for a node. Anything else is left alone.
            if node_of_path(&rel).is_some() && !keep.contains(&rel) {
                fs::remove_file(dir.join(&rel))?;
            }
        }
        Ok(())
    }

    pub fn load_dir(dir: &Path, registry: Arc<Registry>) -> Result<(Graph, Vec<LoadProblem>), DirError> {
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(dir.join("nodes"))? {
            let name = entry?.file_name().to_string_lossy().to_string();
            let rel = format!("nodes/{name}");
            if is_node_path(&rel) {
                files.insert(rel.clone(), fs::read_to_string(dir.join(&rel))?);
            }
        }
        let saved = SavedGraph::from_files(files.iter().map(|(p, t)| (p.as_str(), t.as_str()))).map_err(|(path, e)| DirError::Parse(path, e))?;
        Ok(Graph::load(saved, registry))
    }
}

/// Where a node's file goes, relative to the workspace, with `/` between folders.
pub fn node_path(node: &SavedNode) -> String {
    path_of(node.id)
}

pub fn path_of(id: Entity) -> String {
    format!("nodes/{id}.yaml")
}

/// A file read as a node: any `.yaml` directly in `nodes/`. The node is the `id` inside it, so a
/// hand-made file can be called anything, though only a file named for its node is ever written.
pub fn is_node_path(path: &str) -> bool {
    path.strip_prefix("nodes/").and_then(|n| n.strip_suffix(".yaml")).is_some_and(|stem| !stem.is_empty() && !stem.starts_with('.') && !stem.contains('/'))
}

/// The node a file is for, if it's a node's file.
pub fn node_of_path(path: &str) -> Option<Entity> {
    path.strip_prefix("nodes/").and_then(|n| n.strip_suffix(".yaml")).and_then(|stem| stem.parse().ok())
}

/// A node's file, as text.
pub fn node_text(node: &SavedNode) -> String {
    serde_yaml_ng::to_string(node).expect("nodes serialise")
}

pub fn parse_node(text: &str) -> Result<SavedNode, serde_yaml_ng::Error> {
    serde_yaml_ng::from_str(text)
}

impl SavedGraph {
    /// The node files among these, by path. Anything else is ignored, and the first file that isn't
    /// a valid node comes back with why.
    pub fn from_files<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<SavedGraph, (String, serde_yaml_ng::Error)> {
        let mut nodes = Vec::new();
        let mut files: Vec<_> = files.into_iter().filter(|(p, _)| is_node_path(p)).collect();
        files.sort();
        for (path, text) in files {
            nodes.push(parse_node(text).map_err(|e| (path.to_string(), e))?);
        }
        Ok(SavedGraph { nodes })
    }

    /// Every node's file, by path.
    pub fn to_files(&self) -> BTreeMap<String, String> {
        self.nodes.iter().map(|n| (node_path(n), node_text(n))).collect()
    }
}

#[derive(Debug)]
pub enum DirError {
    Io(std::io::Error),
    Yaml(serde_yaml_ng::Error),
    /// This file isn't valid.
    Parse(String, serde_yaml_ng::Error),
}

impl From<std::io::Error> for DirError {
    fn from(e: std::io::Error) -> Self {
        DirError::Io(e)
    }
}

impl From<serde_yaml_ng::Error> for DirError {
    fn from(e: serde_yaml_ng::Error) -> Self {
        DirError::Yaml(e)
    }
}
