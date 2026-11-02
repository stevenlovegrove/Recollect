//! The graph as files: one per node, plus one for anything that applies to the whole graph. A
//! component is plain data, a rule says which code backs it, and a link lives with its source.
//! Memos are never saved. Nothing here needs a rule to be understood: an unknown rule or
//! component goes back out exactly as it came in.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Backing, Entity, Error, Graph, Key, Runtime};
use crate::log::Violation;
use crate::registry::{Code, Registry, SavedRule};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedLink {
    pub to: Entity,
    pub components: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedNode {
    pub id: Entity,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, SavedRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<SavedLink>,
}

/// What applies to every node, beyond what the registry sets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedDefaults {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, SavedRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedGraph {
    pub nodes: Vec<SavedNode>,
    #[serde(default)]
    pub defaults: SavedDefaults,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    /// These are backed by closures, which have no written form.
    Closures(Vec<Key>),
    /// These rules have a key, but the registry can't load it, so the file couldn't be read back.
    Unregistered(Vec<String>),
}

/// Something about loaded data that needs attention. The data is kept either way.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadProblem {
    /// A cell whose rule can't run.
    BrokenRule(Key, Error),
    /// A link to a node with no file. It's kept, and the node exists with nothing on it.
    DanglingLink { from: Entity, to: Entity },
    /// A crash cut this many bytes of an unfinished write off the end of the log. That commit never
    /// returned, so nothing that was promised is lost.
    TornLog { bytes: u64 },
    /// The files differed from the log, so the difference was recorded as this event.
    ExternalEdit { seq: u64 },
    /// The log's last events ran under different code from this.
    CodeChanged { was: Code, now: Code },
    /// Invariants that don't hold right now, such as after a hand edit. They don't block other work.
    Standing(Vec<Violation>),
}

impl Graph {
    pub fn save(&self) -> Result<SavedGraph, SaveError> {
        self.save_only(self.entities())
    }

    /// The written form of these nodes, and of the defaults.
    pub(crate) fn save_only(&self, ids: impl Iterator<Item = Entity>) -> Result<SavedGraph, SaveError> {
        let (mut closures, mut unregistered) = (Vec::new(), Vec::new());
        let mut write = |key: Key, b: &Backing, components: &mut BTreeMap<String, Value>, rules: &mut BTreeMap<String, SavedRule>| match b {
            Backing::Stored(v) => {
                components.insert(key.1, v.clone());
            }
            Backing::Broken(saved, _) => {
                rules.insert(key.1, saved.clone());
            }
            Backing::Computed(r) => match &r.saved {
                None => closures.push(key),
                Some(saved) => {
                    if !self.registry.contains(&saved.rule) {
                        unregistered.push(saved.rule.clone());
                    }
                    rules.insert(key.1, saved.clone());
                }
            },
        };

        let mut nodes = Vec::new();
        for id in ids {
            let (mut components, mut rules) = (BTreeMap::new(), BTreeMap::new());
            for (name, b) in self.cells.get(&id).into_iter().flatten() {
                write((id, name.clone()), b, &mut components, &mut rules);
            }
            let links = self.links.iter().filter(|((f, _), _)| *f == id).map(|((_, to), c)| SavedLink { to: *to, components: c.clone() }).collect();
            nodes.push(SavedNode { id, components, rules, links });
        }

        let mut defaults = SavedDefaults::default();
        let mut names: Vec<&String> = self.defaults.keys().collect();
        names.sort();
        for name in names {
            let b = &self.defaults[name];
            if !self.registry.defaults.get(name).is_some_and(|r| same(r, b)) {
                write(Registry::column_key(name), b, &mut defaults.components, &mut defaults.rules);
            }
        }

        if !closures.is_empty() {
            return Err(SaveError::Closures(closures));
        }
        if !unregistered.is_empty() {
            unregistered.sort();
            unregistered.dedup();
            return Err(SaveError::Unregistered(unregistered));
        }
        Ok(SavedGraph { nodes, defaults })
    }

    pub fn load(saved: SavedGraph, registry: Arc<Registry>) -> (Graph, Vec<LoadProblem>) {
        let mut g = Graph::with_registry(registry.clone());
        let mut problems = Vec::new();
        for node in &saved.nodes {
            g.adopt(node.id);
        }
        for (name, v) in saved.defaults.components {
            g.defaults.insert(name, Backing::Stored(v));
        }
        for (name, s) in saved.defaults.rules {
            let key = Registry::column_key(&name);
            let b = registry.resolve(&key, &s);
            if let Backing::Broken(_, e) = &b {
                problems.push(LoadProblem::BrokenRule(key, e.clone()));
            }
            g.defaults.insert(name, b);
        }
        for node in saved.nodes {
            let id = node.id;
            for (name, v) in node.components {
                g.set_runtime(id, &name, v);
            }
            for (name, s) in node.rules {
                let key = (id, name.clone());
                let b = registry.resolve(&key, &s);
                if let Backing::Broken(_, e) = &b {
                    problems.push(LoadProblem::BrokenRule(key, e.clone()));
                }
                g.back(id, &name, b);
            }
            for link in node.links {
                if !g.entities.contains(&link.to) {
                    problems.push(LoadProblem::DanglingLink { from: id, to: link.to });
                    g.adopt(link.to);
                }
                for (kind, v) in link.components {
                    g.link_runtime(id, link.to, Runtime::new(&kind, v));
                }
            }
        }
        (g, problems)
    }

    /// Write `nodes/<id>.yaml` and `graph.yaml`. Files whose text is unchanged aren't touched, so
    /// a commit only shows what really changed, and files for nodes that are gone are removed.
    ///
    /// Each file is replaced atomically, but a crash part way through can leave some files newer
    /// than others. Anything that has to survive that belongs in a `Log`.
    pub fn save_dir(&self, dir: &Path) -> Result<(), DirError> {
        write_files(&self.save().map_err(DirError::Save)?, dir, &mut || {})
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
    write_if_changed(&dir.join("graph.yaml"), &serde_yaml_ng::to_string(&saved.defaults)?, step)?;
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
    let path = dir.join("graph.yaml");
    let defaults = if path.exists() { serde_yaml_ng::from_str(&fs::read_to_string(&path)?).map_err(|e| DirError::Parse(path.display().to_string(), e))? } else { SavedDefaults::default() };
    Ok(SavedGraph { nodes, defaults })
}

#[derive(Debug)]
pub enum DirError {
    Save(SaveError),
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

pub(crate) fn same(a: &Backing, b: &Backing) -> bool {
    match (a, b) {
        (Backing::Stored(x), Backing::Stored(y)) => x == y,
        (Backing::Computed(x), Backing::Computed(y)) => x.saved.is_some() && x.saved == y.saved,
        (Backing::Broken(x, _), Backing::Broken(y, _)) => x == y,
        _ => false,
    }
}
