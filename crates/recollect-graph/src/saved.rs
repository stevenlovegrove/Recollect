//! The graph as files: one per node, holding the components it was given, what it says in its cells,
//! and its edges, which live with their source. Memos are never saved, and nor is anything a default
//! or a kind says: only what was written. Nothing here needs a system to be understood: a component or
//! cell nothing provides goes back out exactly as it came in.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
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
    /// A crash cut this many bytes of an unfinished write off the end of the log. That commit never
    /// returned, so nothing that was promised is lost.
    TornLog { bytes: u64 },
    /// The files differed from the log, so the difference was recorded as this event.
    ExternalEdit { seq: u64 },
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

    /// Write `nodes/<id>.yaml`. Files whose text is unchanged aren't touched, so a commit only shows
    /// what really changed, and files for nodes that are gone are removed.
    ///
    /// Each file is replaced atomically, but a crash part way through can leave some files newer than
    /// others. Anything that has to survive that belongs in a `Log`.
    pub fn save_dir(&self, dir: &Path) -> Result<(), DirError> {
        write_files(&self.save(), dir, &mut || {})
    }

    pub fn load_dir(dir: &Path, registry: Arc<Registry>) -> Result<(Graph, Vec<LoadProblem>), DirError> {
        Ok(Graph::load(read_files(dir)?, registry))
    }
}

pub(crate) fn node_path(dir: &Path, id: Entity) -> std::path::PathBuf {
    dir.join("nodes").join(format!("{id}.yaml"))
}

/// Bring every file in line with this. `step` is called between the things a crash could fall
/// between.
pub(crate) fn write_files(saved: &SavedGraph, dir: &Path, step: &mut dyn FnMut()) -> Result<(), DirError> {
    let nodes_dir = dir.join("nodes");
    fs::create_dir_all(&nodes_dir)?;
    let keep: std::collections::BTreeSet<String> = saved.nodes.iter().map(|n| format!("{}.yaml", n.id)).collect();
    for node in &saved.nodes {
        write_if_changed(&node_path(dir, node.id), &serde_yaml_ng::to_string(node)?, step)?;
    }
    sync_dir(dir)?;
    for entry in fs::read_dir(&nodes_dir)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // Only ever a file this wrote: a yaml file named for a node. Anything else is left alone.
        let is_node_file = name.strip_suffix(".yaml").is_some_and(|stem| stem.parse::<Entity>().is_ok());
        if is_node_file && !keep.contains(&name) {
            fs::remove_file(path)?;
            step();
        }
    }
    sync_dir(&nodes_dir)?;
    Ok(())
}

pub(crate) fn read_files(dir: &Path) -> Result<SavedGraph, DirError> {
    let mut nodes = Vec::new();
    let mut paths: Vec<_> = fs::read_dir(dir.join("nodes"))?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
    paths.sort();
    for path in paths {
        if path.extension().is_some_and(|x| x == "yaml") {
            let node: SavedNode = serde_yaml_ng::from_str(&fs::read_to_string(&path)?).map_err(|e| DirError::Parse(path.display().to_string(), e))?;
            nodes.push(node);
        }
    }
    Ok(SavedGraph { nodes })
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

pub(crate) fn write_if_changed(path: &Path, text: &str, step: &mut dyn FnMut()) -> std::io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    write_atomically(path, text, step)
}

/// Replace a file so that anyone reading it, even after a crash, sees all of the old text or all
/// of the new. The rename isn't durable until the caller flushes the directory, which it can do
/// once for many files.
pub(crate) fn write_atomically(path: &Path, text: &str, step: &mut dyn FnMut()) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    step();
    let mut file = fs::File::create(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    step();
    fs::rename(&temp, path)?;
    step();
    Ok(())
}

/// A rename or removal isn't durable until its directory is flushed.
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Whatever a crash in `write_atomically` left behind.
pub(crate) fn remove_temp_files(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') && name.ends_with(".tmp") {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
