//! An open workspace. Every change goes through here, so the web UI, Claude and edits to the files
//! all see the same thing, and every change that lands is an event in the log.

use std::collections::{BTreeMap, BTreeSet};

use recollect_graph::{CommitError, Entity, Log, Op, SavedGraph, SavedNode, Stored, is_node_path, node_of_path, node_text, parse_node, path_of};
use recollect_systems::samples::{FORMAT, SAMPLES, config_text, ops_of};
use recollect_systems::{Built, build, needed};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ops::{Resolved, resolve_ops};

/// A workspace's files, by path relative to its root, with `/` between folders.
pub type Files = BTreeMap<String, String>;

/// What a host has to do to its files after a change: write these, and remove those.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Writes {
    pub write: BTreeMap<String, String>,
    pub delete: Vec<String>,
}

impl Writes {
    pub fn is_empty(&self) -> bool {
        self.write.is_empty() && self.delete.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    NotFound(String),
    /// The change wasn't made, for reasons the person making it can act on.
    Refused(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::Refused(s) | Error::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// What kind of error it is, as a host passes it on: an HTTP status says the same.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::Refused(_) => "refused",
            Error::Other(_) => "other",
        }
    }
}

/// `recollect.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(default)]
    pub systems: Vec<String>,
    // `llm_direct_writes`, from when Claude's changes waited for review, is still read and ignored:
    // Claude now writes directly, and any change can be taken back from History.
}

pub const CONFIG: &str = "recollect.toml";

/// A sample's working copy says which version of the samples it was made from, so one made when
/// samples meant something else is replaced rather than misread.
pub const SAMPLE_MARKER: &str = "sample-format";
pub const SAMPLE_FORMAT: &str = FORMAT;

fn refusal(e: CommitError) -> Error {
    match e {
        CommitError::Refused(v) => Error::Refused(v.iter().map(|v| format!("{}: {}", v.rule, v.detail)).collect::<Vec<_>>().join("; ")),
        CommitError::NoSuchEvent(seq) => Error::NotFound(format!("event {seq}")),
    }
}

/// What a change to the files brought in.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Changed {
    /// The graph changed, and it's an event.
    pub graph: bool,
    /// Its name or settings changed.
    pub workspace: bool,
    /// Which systems it uses changed, so it has to be opened again.
    pub reopen: bool,
    /// Files that couldn't be read, which are left as they were understood before.
    pub problems: Vec<String>,
}

pub struct Workspace {
    /// What the host knows it by.
    pub id: String,
    /// Where it is, for a person to read.
    pub location: String,
    /// Set for a sample's working copy.
    pub sample: Option<String>,
    pub config: Config,
    pub built: Built,
    pub log: Log,
    /// Every file as last seen or written, so a host telling it about its own writes changes nothing.
    known: Files,
    /// The node files as last understood, by path.
    parsed: BTreeMap<String, SavedNode>,
    /// Which of those each node lives in, so a change to it is written there.
    homes: BTreeMap<Entity, String>,
    pending: Writes,
}

/// The files of a new, empty workspace. It takes on systems as it uses them.
pub fn new_files(name: &str) -> Result<Files, Error> {
    let config = Config { name: name.to_string(), about: None, systems: Vec::new() };
    Ok(Files::from([(CONFIG.to_string(), toml::to_string(&config).map_err(|e| Error::Other(e.to_string()))?)]))
}

/// The files of a fresh working copy of a sample, or nothing if there's no such sample.
pub fn sample_files(name: &str) -> Option<Files> {
    let s = SAMPLES.iter().find(|s| s.name == name)?;
    let systems: Vec<String> = s.systems.iter().map(|n| n.to_string()).collect();
    let built = build(&systems).expect("a sample's systems exist");
    let mut log = Log::new(built.registry);
    log.commit("sample", s.about, ops_of(s)).expect("a sample commits");
    let mut files = log.graph().save().to_files();
    files.insert(CONFIG.to_string(), config_text(s));
    files.insert(SAMPLE_MARKER.to_string(), FORMAT.to_string());
    Some(files)
}

fn read_config(text: &str) -> Result<Config, Error> {
    toml::from_str(text).map_err(|e| Error::Other(format!("reading {CONFIG}: {e}")))
}

/// Which file each node lives in. A node can be in a file called anything, as when someone or some
/// other program made it by hand, and it stays there. If two files say they're the same node, the one
/// named for it wins, or else the first.
fn homes_of(parsed: &BTreeMap<String, SavedNode>) -> BTreeMap<Entity, String> {
    let mut homes = BTreeMap::new();
    for (path, node) in parsed {
        if node_of_path(path) == Some(node.id) || !homes.contains_key(&node.id) {
            homes.insert(node.id, path.clone());
        }
    }
    homes
}

/// The components and cells the node files use, which say which systems the workspace needs.
fn held(parsed: &BTreeMap<String, SavedNode>) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let (mut components, mut cells) = (BTreeSet::new(), BTreeSet::new());
    for n in parsed.values() {
        components.extend(n.components.iter().map(String::as_str));
        cells.extend(n.cells.keys().chain(n.formulas.keys()).map(String::as_str));
        for l in &n.links {
            components.extend(l.components.iter().map(String::as_str));
            cells.extend(l.cells.keys().chain(l.formulas.keys()).map(String::as_str));
        }
    }
    (components, cells)
}

/// The components and cells a change-set gives things.
fn touched(ops: &[Op]) -> (Vec<&str>, Vec<&str>) {
    let (mut components, mut cells) = (Vec::new(), Vec::new());
    for op in ops {
        match op {
            Op::Add { component, .. } | Op::Link { component, .. } => components.push(component.as_str()),
            Op::Set { cell, to: Some(_), .. } => cells.push(cell.as_str()),
            _ => {}
        }
    }
    (components, cells)
}

/// What a workspace has, together with what it needs, in the order systems are listed. A workspace
/// never loses a system by not using it any more.
fn with_needed(have: &[String], needs: Vec<String>) -> Vec<String> {
    let mut out = have.to_vec();
    out.extend(needs.into_iter().filter(|n| !have.contains(n)));
    out
}

/// The graph the node files say, one node per handle, from the file it lives in.
fn graph_of(parsed: &BTreeMap<String, SavedNode>, homes: &BTreeMap<Entity, String>) -> SavedGraph {
    SavedGraph { nodes: homes.values().map(|p| parsed[p].clone()).collect() }
}

impl Workspace {
    /// Open a workspace from its files. Anything that isn't part of one, such as a log from an older
    /// version, is ignored. What needs attention comes back with it, in words.
    pub fn load(id: &str, location: &str, sample: Option<String>, files: Files) -> Result<(Workspace, Vec<String>), Error> {
        let mut config = read_config(files.get(CONFIG).ok_or_else(|| Error::Other(format!("{location} isn't a workspace: it has no {CONFIG}")))?)?;
        let mut parsed = BTreeMap::new();
        for (path, text) in files.iter().filter(|(p, _)| is_node_path(p)) {
            parsed.insert(path.clone(), parse_node(text).map_err(|e| Error::Other(format!("{path} isn't a valid node: {e}")))?);
        }
        // Whatever its nodes use, as when they were written by hand or by an older version.
        let (components, cells) = held(&parsed);
        let systems = with_needed(&config.systems, needed(components, cells));
        let took_on = systems != config.systems;
        config.systems = systems;
        let built = build(&config.systems).map_err(Error::Other)?;
        let homes = homes_of(&parsed);
        let (log, problems) = Log::from_saved(graph_of(&parsed, &homes), built.registry.clone());
        let mut ws = Workspace { id: id.to_string(), location: location.to_string(), sample, config, built, log, known: files, parsed, homes, pending: Writes::default() };
        // A node only an edge mentions gets a file.
        let homeless: BTreeSet<Entity> = ws.log.graph().entities().filter(|e| !ws.homes.contains_key(e)).collect();
        for node in ws.log.graph().save().nodes.into_iter().filter(|n| homeless.contains(&n.id)) {
            ws.put_node(node);
        }
        if took_on {
            ws.put_config();
        }
        Ok((ws, problems.into_iter().map(|p| format!("{p:?}")).collect()))
    }

    /// Write a node to the file it lives in, or a new one named for it, if that differs from what's there.
    fn put_node(&mut self, node: SavedNode) {
        let path = self.homes.get(&node.id).cloned().unwrap_or_else(|| path_of(node.id));
        self.homes.insert(node.id, path.clone());
        let text = node_text(&node);
        if self.known.get(&path) != Some(&text) {
            self.known.insert(path.clone(), text.clone());
            self.pending.delete.retain(|p| *p != path);
            self.pending.write.insert(path.clone(), text);
        }
        self.parsed.insert(path, node);
    }

    fn put(&mut self, path: String, text: String) {
        self.known.insert(path.clone(), text.clone());
        self.pending.delete.retain(|p| *p != path);
        self.pending.write.insert(path, text);
    }

    fn put_config(&mut self) {
        let text = toml::to_string(&self.config).expect("settings serialise");
        self.put(CONFIG.to_string(), text);
    }

    fn remove(&mut self, path: &str) {
        if let Some(node) = self.parsed.remove(path)
            && self.homes.get(&node.id).is_some_and(|h| h == path)
        {
            self.homes.remove(&node.id);
        }
        self.pending.write.remove(path);
        if self.known.remove(path).is_some() && !self.pending.delete.iter().any(|p| p == path) {
            self.pending.delete.push(path.to_string());
        }
    }

    /// What the host has to write since this was last asked, and forgets it.
    pub fn take_writes(&mut self) -> Writes {
        let changes = self.log.take_changes();
        for node in changes.nodes {
            self.put_node(node);
        }
        for gone in changes.gone {
            let paths: Vec<String> = self.parsed.iter().filter(|(_, n)| n.id == gone).map(|(p, _)| p.clone()).collect();
            for p in paths.into_iter().chain([path_of(gone)]) {
                self.remove(&p);
            }
            self.homes.remove(&gone);
        }
        std::mem::take(&mut self.pending)
    }

    /// Every file, as it is now.
    pub fn files(&self) -> &Files {
        &self.known
    }

    /// Files changed underneath it, by hand or by another program: each path with its text now, or
    /// nothing if it's gone. What this wrote itself is no change. A change to the graph is recorded as
    /// an event by `disk`, and a file that can't be read is left as it was understood before.
    pub fn files_changed(&mut self, changes: BTreeMap<String, Option<String>>) -> Changed {
        let mut out = Changed::default();
        let mut graph = false;
        for (path, text) in changes {
            if self.known.get(&path) == text.as_ref() {
                continue;
            }
            match &text {
                Some(t) => self.known.insert(path.clone(), t.clone()),
                None => self.known.remove(&path),
            };
            if is_node_path(&path) {
                graph = true;
                match text.as_deref().map(parse_node) {
                    Some(Ok(node)) => {
                        self.parsed.insert(path, node);
                    }
                    Some(Err(e)) => out.problems.push(format!("{path} isn't a valid node: {e}")),
                    None => {
                        self.parsed.remove(&path);
                    }
                }
            } else if path == CONFIG {
                match text.as_deref().map(read_config) {
                    Some(Ok(c)) if c.systems == self.config.systems => {
                        out.workspace = c != self.config;
                        self.config = c;
                    }
                    Some(Ok(_)) => out.reopen = true,
                    Some(Err(e)) => out.problems.push(e.to_string()),
                    None => out.problems.push(format!("{CONFIG} is gone")),
                }
            }
        }
        // Something written by hand that needs a system this hasn't got is only understood once it's
        // opened again, with it.
        let (components, cells) = held(&self.parsed);
        if with_needed(&self.config.systems, needed(components, cells)) != self.config.systems {
            out.reopen = true;
            return out;
        }
        if graph {
            self.homes = homes_of(&self.parsed);
            let (seq, _) = self.log.reconcile(&graph_of(&self.parsed, &self.homes), "disk");
            out.graph = seq.is_some();
        }
        out
    }

    // ---------- changing it ----------

    /// A formula written through the app or by Claude has to work when it's written, or it's refused,
    /// saying why. A hand edit can't be refused, so it stands and its cell says what's wrong.
    fn formulas_work(&self, ops: &[Op]) -> Result<(), Error> {
        let g = self.log.what_if(ops).map_err(|v| Error::Refused(v.iter().map(|v| format!("{}: {}", v.rule, v.detail)).collect::<Vec<_>>().join("; ")))?;
        for op in ops {
            if let Op::Set { on, cell, to: Some(Stored::Formula(_)) } = op
                && let Err(e) = g.get_value(on.entity(), cell)
            {
                return Err(Error::Refused(crate::views::error_text(&e, &g)));
            }
        }
        Ok(())
    }

    /// Commit these ops. Returns what any `new:` names in them became. Using something from a system
    /// the workspace hasn't got yet, such as a first task, takes that system on, and saying so is part
    /// of the same change: if it's refused, the workspace is as it was.
    pub fn apply(&mut self, ops: Vec<Value>, author: &str, why: &str) -> Result<Resolved, Error> {
        let resolved = resolve_ops(ops.clone(), self.log.graph()).map_err(Error::Refused)?;
        let (components, cells) = touched(&resolved.ops);
        let systems = with_needed(&self.config.systems, needed(components, cells));
        if systems == self.config.systems {
            self.formulas_work(&resolved.ops)?;
            self.log.commit(author, why, resolved.ops.clone()).map_err(refusal)?;
            return Ok(resolved);
        }
        let built = build(&systems).map_err(Error::Refused)?;
        let before = self.built.registry.clone();
        self.log.widen(built.registry.clone());
        // Again, now that what it uses is known, so its cells bring their components.
        let done = resolve_ops(ops, self.log.graph()).map_err(Error::Refused).and_then(|r| {
            self.formulas_work(&r.ops)?;
            self.log.commit(author, why, r.ops.clone()).map_err(refusal)?;
            Ok(r)
        });
        match done {
            Ok(r) => {
                self.built = built;
                self.config.systems = systems;
                self.put_config();
                Ok(r)
            }
            Err(e) => {
                self.log.widen(before);
                Err(e)
            }
        }
    }

    /// Take back any one event.
    pub fn undo(&mut self, seq: u64, author: &str, why: &str) -> Result<u64, Error> {
        self.log.undo(seq, author, why).map_err(refusal)
    }

    /// Take back the newest change, if there's one to take back.
    pub fn undo_last(&mut self, author: &str) -> Result<Option<u64>, Error> {
        self.log.undo_last(author).map_err(refusal)
    }

    /// Take back the newest undo, if nothing's changed since.
    pub fn redo(&mut self, author: &str) -> Result<Option<u64>, Error> {
        self.log.redo(author).map_err(refusal)
    }

}

#[cfg(test)]
mod tests;
