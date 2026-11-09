//! The action log. A graph held by a `Log` only changes through `commit`, so its state is the replay
//! of its events, and every fact can answer who set it, when and why.
//!
//! ```
//! # use std::sync::Arc;
//! # use recollect_graph::{Log, Op, On, Registry, Stored, Entity};
//! # use serde_json::json;
//! let mut log = Log::new(Arc::new(Registry::new()));
//! let a = Entity::new();
//! let seq = log.commit("steven", "first thoughts", vec![Op::Spawn { id: a }, Op::Set { on: On::Node { id: a }, cell: "note".into(), to: Some(Stored::Value(json!("hello"))) }]).unwrap();
//! assert_eq!(log.graph().get_value(a, "note"), Ok(Some(json!("hello"))));
//! log.undo(seq, "steven", "changed my mind").unwrap();
//! assert!(!log.graph().contains(a));
//! ```
//!
//! On disk the log is what's true, and the YAML files are a projection of it. A commit appends one
//! line to `log.jsonl` and flushes it, which is the moment it happens, and then brings the files
//! up to date and records in `log.applied` how far they've got. So after a crash, `open` knows from
//! the log exactly what's committed, and from `log.applied` which files might merely be behind
//! rather than edited by hand.
//!
//! History is a tree. Every event has a parent, and the log has a head, which is where it is now and
//! what the files show. Going back to an earlier event only moves the head, so the next commit starts
//! a branch from there, and everything that came after is still in the log to go back to. Nothing is
//! ever rewritten. Events are numbered in the order they happened, whichever branch they're on.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Backed, Entity, Error, Graph};
use crate::registry::{Place, Registry, Severity};
use crate::saved::{DirError, LoadProblem, SavedEdge, SavedGraph, SavedNode, node_path, read_files, remove_temp_files, sync_dir, write_atomically, write_files, write_if_changed};

/// What's written in a cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stored {
    Value(Value),
    Formula(String),
}

impl Stored {
    fn of(b: Backed) -> Option<Stored> {
        match b {
            Backed::Value(v) => Some(Stored::Value(v)),
            Backed::Formula(t) => Some(Stored::Formula(t)),
            Backed::Builtin => None,
        }
    }
}

/// What a cell is on: a node, or the edge from one node to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum On {
    Node { id: Entity },
    Edge { from: Entity, to: Entity },
}

impl On {
    pub fn entity(&self) -> Entity {
        match *self {
            On::Node { id } => id,
            On::Edge { from, to } => Entity::edge(from, to),
        }
    }

    pub fn target(&self) -> Target {
        match *self {
            On::Node { id } => Target::Node(id),
            On::Edge { from, to } => Target::Edge(from, to),
        }
    }
}

/// The only ways a logged graph changes. Each has an exact inverse, so undo is just more ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A node with nothing on it.
    Spawn { id: Entity },
    /// Remove a node and everything it says. Its edges have to go first.
    Despawn { id: Entity },
    /// Give a node a component.
    Add { id: Entity, component: String },
    /// Take a component from a node.
    Remove { id: Entity, component: String },
    /// Give the edge from one node to another a component, making the edge if it's new.
    Link { from: Entity, to: Entity, component: String },
    /// Take a component off an edge. Taking the last one takes the edge.
    Unlink { from: Entity, to: Entity, component: String },
    /// Write a value or a formula in a cell. `None` takes away what's written there. As JSON it reads
    /// `{"op": "set", "id": .., "cell": .., "value": ..}`, or `"formula"`, and an edge's cell has
    /// `"from"` and `"to"` in place of `"id"`.
    Set {
        #[serde(flatten)]
        on: On,
        cell: String,
        #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
        to: Option<Stored>,
    },
}

impl Op {
    pub fn set_value(id: Entity, cell: &str, value: Value) -> Op {
        Op::Set { on: On::Node { id }, cell: cell.to_string(), to: Some(Stored::Value(value)) }
    }

    pub fn set<C: crate::CellType>(id: Entity, value: C) -> Op {
        Op::set_value(id, C::NAME, serde_json::to_value(&value).expect("cells serialise"))
    }

    pub fn set_formula(id: Entity, cell: &str, text: &str) -> Op {
        Op::Set { on: On::Node { id }, cell: cell.to_string(), to: Some(Stored::Formula(text.to_string())) }
    }

    pub fn add(id: Entity, component: &str) -> Op {
        Op::Add { id, component: component.to_string() }
    }

    pub fn link(from: Entity, to: Entity, component: &str) -> Op {
        Op::Link { from, to, component: component.to_string() }
    }

    pub fn target(&self) -> Target {
        match self {
            Op::Spawn { id } | Op::Despawn { id } | Op::Add { id, .. } | Op::Remove { id, .. } => Target::Node(*id),
            Op::Link { from, to, .. } | Op::Unlink { from, to, .. } => Target::Edge(*from, *to),
            Op::Set { on, .. } => on.target(),
        }
    }
}

/// What an op changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Node(Entity),
    Edge(Entity, Entity),
}

impl Target {
    /// The nodes it's about.
    pub fn nodes(&self) -> Vec<Entity> {
        match *self {
            Target::Node(e) => vec![e],
            Target::Edge(a, b) => vec![a, b],
        }
    }
}

/// Something that doesn't hold, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub rule: String,
    pub on: Target,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    /// The event this one follows. Only the first event has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u64>,
    /// Milliseconds since 1970.
    pub at: u64,
    pub author: String,
    /// Why, in the actor's words. Reasons live here, not in node text.
    pub why: String,
    pub ops: Vec<Op>,
    /// Exactly undoes `ops`, applied in order.
    pub inverse: Vec<Op>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record {
    Event(Event),
    /// The head moved here, with no new event.
    Head { seq: u64, at: u64, author: String, why: String },
}

#[derive(Debug)]
pub enum CommitError {
    /// Nothing changed: these checks would be newly broken, or the ops don't apply.
    Refused(Vec<Violation>),
    /// Nothing changed: the log couldn't be written.
    Io(std::io::Error),
    /// It did happen: the event is safely in the log, but the files couldn't be brought up to date.
    /// The next commit or open tries again.
    FilesBehind { seq: u64, error: DirError },
    /// There's no such event.
    NoSuchEvent(u64),
    /// This event isn't part of where the log is now, so it can't be undone from here.
    NotOnThisBranch(u64),
}

impl CommitError {
    /// What was refused, or nothing if this wasn't a refusal.
    pub fn violations(&self) -> &[Violation] {
        match self {
            CommitError::Refused(v) => v,
            _ => &[],
        }
    }
}

#[derive(Debug)]
pub enum OpenError {
    /// Another process has this directory open.
    Locked,
    /// A complete line in the log is unreadable, or doesn't replay. A crash can't cause this.
    Corrupt { line: usize, detail: String },
    Dir(DirError),
}

impl From<std::io::Error> for OpenError {
    fn from(e: std::io::Error) -> Self {
        OpenError::Dir(DirError::Io(e))
    }
}

impl From<DirError> for OpenError {
    fn from(e: DirError) -> Self {
        OpenError::Dir(e)
    }
}

/// The exit status of a process that died at a chosen step.
#[doc(hidden)]
pub const CRASHED: i32 = 86;

/// Testing: die here, as a crash would, once this many steps have passed.
fn step(crash_after: &mut Option<u32>) {
    if let Some(n) = crash_after {
        if *n == 0 {
            // No destructors or flushing run, and nothing here buffers, so the disk is left as a
            // crash would leave it. Losing power is a different test, which this can't do.
            std::process::exit(CRASHED);
        }
        *n -= 1;
    }
}

struct Disk {
    dir: PathBuf,
    log: File,
    /// Held for its lock, so two processes can't write one log.
    _lock: File,
    /// A write of the files failed, so the next one does all of them.
    stale: bool,
    crash_after: Option<u32>,
}

impl Disk {
    fn append(&mut self, records: &[Record]) -> std::io::Result<()> {
        let mut text = String::new();
        for r in records {
            text.push_str(&serde_json::to_string(r).expect("records serialise"));
            text.push('\n');
        }
        let good = self.log.metadata()?.len();
        step(&mut self.crash_after);
        let written = self.log.write_all(text.as_bytes());
        step(&mut self.crash_after);
        let synced = written.and_then(|()| self.log.sync_all());
        if synced.is_err() {
            // Best effort: anything left over is an unfinished line, which open cuts off.
            let _ = self.log.set_len(good);
        }
        synced?;
        step(&mut self.crash_after);
        Ok(())
    }

    fn mark_applied(&mut self, seq: Option<u64>) -> std::io::Result<()> {
        let text = seq.map_or(String::new(), |s| format!("{s}\n"));
        let crash = &mut self.crash_after;
        write_atomically(&self.dir.join("log.applied"), &text, &mut || step(crash))?;
        sync_dir(&self.dir)
    }
}

/// A graph whose only way to change is a commit.
pub struct Log {
    graph: Graph,
    events: Vec<Event>,
    /// Where the log is now. `None` only before anything has happened.
    head: Option<u64>,
    disk: Option<Disk>,
}

impl Log {
    /// A log that lives only in memory.
    pub fn new(registry: Arc<Registry>) -> Self {
        Log { graph: Graph::with_registry(registry), events: Vec::new(), head: None, disk: None }
    }

    /// Open a directory, creating what's missing and repairing what a crash left. If the files
    /// differ from the log for any reason other than a crash, that's an edit made outside the log,
    /// and it's recorded under `author`. What needs attention comes back with it.
    pub fn open(dir: &Path, registry: Arc<Registry>, author: &str) -> Result<(Log, Vec<LoadProblem>), OpenError> {
        Self::open_inner(dir, registry, author, None)
    }

    /// Testing: `open` that dies after this many steps of its repair, as a crash would.
    #[doc(hidden)]
    pub fn open_crashing_after(dir: &Path, registry: Arc<Registry>, author: &str, steps: u32) -> Result<(Log, Vec<LoadProblem>), OpenError> {
        Self::open_inner(dir, registry, author, Some(steps))
    }

    /// Testing: die after this many more steps of writing, as a crash would.
    #[doc(hidden)]
    pub fn crash_after_steps(&mut self, steps: u32) {
        self.disk.as_mut().expect("a log on disk").crash_after = Some(steps);
    }

    fn open_inner(dir: &Path, registry: Arc<Registry>, author: &str, crash_after: Option<u32>) -> Result<(Log, Vec<LoadProblem>), OpenError> {
        fs::create_dir_all(dir.join("nodes"))?;
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("log.lock"))?;
        take_lock(&lock)?;
        remove_temp_files(dir)?;
        remove_temp_files(&dir.join("nodes"))?;
        let mut problems = Vec::new();

        let path = dir.join("log.jsonl");
        let (records, torn) = read_log(&path)?;
        if torn > 0 {
            problems.push(LoadProblem::TornLog { bytes: torn });
        }
        let applied: Option<u64> = fs::read_to_string(dir.join("log.applied")).ok().and_then(|s| s.trim().parse().ok());

        let mut log = Log::new(registry.clone());
        let mut lines = BTreeMap::new();
        for (line, record) in records.into_iter().enumerate() {
            match record {
                Record::Head { seq, .. } => log.head = Some(seq),
                Record::Event(ev) => {
                    if ev.seq != log.events.len() as u64 || ev.parent.is_some_and(|p| p >= ev.seq) {
                        return Err(OpenError::Corrupt { line: line + 1, detail: format!("event {} is out of order", ev.seq) });
                    }
                    lines.insert(ev.seq, line + 1);
                    log.head = Some(ev.seq);
                    log.events.push(ev);
                }
            }
        }
        if let Some(head) = log.head
            && head as usize >= log.events.len()
        {
            return Err(OpenError::Corrupt { line: 0, detail: format!("the head is at {head}, which isn't an event") });
        }
        // Replay what leads here. The other branches aren't part of what's true now.
        for seq in log.head.map_or(Vec::new(), |h| log.ancestors(h)) {
            for op in log.events[seq as usize].ops.clone() {
                apply(&mut log.graph, &op).map_err(|detail| OpenError::Corrupt { line: lines[&seq], detail })?;
            }
        }
        let replayed = log.graph.save();
        // The files may be as old as the state at `applied`, which can even be on another branch.
        let base = match applied {
            None => SavedGraph::default(),
            Some(a) if Some(a) != log.head && (a as usize) < log.events.len() => log.state_at(a).save(),
            Some(_) => replayed.clone(),
        };
        let last = log.last_seq();

        // A file that matches what it should be, or what it last was, isn't an edit. It's at most
        // behind. Anything else was changed by someone else, so it stands.
        let on_disk = read_files(dir)?;
        let intended = merge(&on_disk, &base, &replayed, &mut problems);

        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        log.disk = Some(Disk { dir: dir.to_path_buf(), log: file, _lock: lock, stale: false, crash_after });
        if let Some(ops) = Some(diff(&replayed, &intended)).filter(|ops| !ops.is_empty()) {
            let seq = log.record_unchecked(author, "external edit", ops).map_err(|e| match e {
                CommitError::Io(e) => OpenError::Dir(DirError::Io(e)),
                CommitError::FilesBehind { error, .. } => OpenError::Dir(error),
                other => unreachable!("{other:?}"),
            })?;
            problems.push(LoadProblem::ExternalEdit { seq });
        }

        // Anything still out of step is only behind.
        let now = log.graph.save();
        if applied != log.last_seq() || last != log.last_seq() || !same_graph(&on_disk, &now) {
            log.write_all_files().map_err(OpenError::Dir)?;
        }

        problems.extend(log.graph.broken().into_iter().map(|(key, error)| LoadProblem::BrokenFormula(key, error)));
        let standing = log.violations();
        if !standing.is_empty() {
            problems.push(LoadProblem::Standing(standing));
        }
        Ok((log, problems))
    }

    // ---------- reading ----------

    /// Read-only: writing goes through `commit`.
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Where the log is now, which is what the files show.
    pub fn last_seq(&self) -> Option<u64> {
        self.head
    }

    /// The events leading to this one, oldest first, ending with it.
    fn ancestors(&self, seq: u64) -> Vec<u64> {
        let mut out = vec![seq];
        let mut at = seq;
        while let Some(parent) = self.events[at as usize].parent {
            out.push(parent);
            at = parent;
        }
        out.reverse();
        out
    }

    /// The events that led to where the log is now, oldest first.
    pub fn path(&self) -> Vec<&Event> {
        self.head.map_or(Vec::new(), |h| self.ancestors(h)).into_iter().map(|s| &self.events[s as usize]).collect()
    }

    /// The newest event that follows from this one, if any does: where to go to return from having
    /// gone back. Events are numbered in order, so the newest is the last one found.
    pub fn newest_after(&self, seq: u64) -> Option<u64> {
        let mut reached = BTreeSet::from([seq]);
        let mut newest = None;
        for ev in &self.events[seq as usize + 1..] {
            if ev.parent.is_some_and(|p| reached.contains(&p)) {
                reached.insert(ev.seq);
                newest = Some(ev.seq);
            }
        }
        newest
    }

    /// The graph as it was just after event `seq`, worked out by the code running now.
    pub fn state_at(&self, seq: u64) -> Graph {
        let mut g = Graph::with_registry(self.graph.registry.clone());
        for s in self.ancestors(seq) {
            for op in &self.events[s as usize].ops {
                apply(&mut g, op).expect("logged ops replay");
            }
        }
        g
    }

    /// Every event leading here that touched a target, oldest first.
    pub fn history(&self, target: Target) -> Vec<&Event> {
        self.path().into_iter().filter(|ev| ev.ops.iter().any(|op| op.target() == target)).collect()
    }

    /// Who last wrote in a cell on the way here, and why, if anyone has.
    pub fn blame(&self, on: On, cell: &str) -> Option<&Event> {
        self.path().into_iter().rev().find(|ev| ev.ops.iter().any(|op| matches!(op, Op::Set { on: o, cell: c, to: Some(_) } if *o == on && c == cell)))
    }

    /// Every invariant that doesn't hold now, such as after a hand edit.
    pub fn violations(&self) -> Vec<Violation> {
        let mut out = check_values(&self.graph, &all_targets(&self.graph));
        out.extend(check_rules(&self.graph, Severity::Invariant));
        out.sort();
        out
    }

    /// What's worth a look now.
    pub fn notices(&self) -> Vec<Violation> {
        check_rules(&self.graph, Severity::Notice)
    }

    /// The graph these ops would give, if they'd be accepted. It's the same code as a commit, and
    /// nothing is written or logged.
    pub fn what_if(&self, ops: &[Op]) -> Result<Graph, Vec<Violation>> {
        let mut hypothetical = self.graph.fork();
        try_apply(&mut hypothetical, ops)?;
        Ok(hypothetical)
    }

    // ---------- writing ----------

    /// Apply `ops` atomically, or refuse them and change nothing. Checks already broken, such as by a
    /// hand edit, don't block a commit unless it breaks them in a new way.
    pub fn commit(&mut self, author: &str, why: &str, ops: Vec<Op>) -> Result<u64, CommitError> {
        let inverse = try_apply(&mut self.graph, &ops).map_err(CommitError::Refused)?;
        self.record(author, why, ops, inverse)
    }

    /// Undo by appending the inverse. History is never rewritten, so the undo is itself audited.
    pub fn undo(&mut self, seq: u64, author: &str, why: &str) -> Result<u64, CommitError> {
        let event = self.events.get(seq as usize).ok_or(CommitError::NoSuchEvent(seq))?;
        let inverse = event.inverse.clone();
        if !self.head.is_some_and(|h| self.ancestors(h).contains(&seq)) {
            return Err(CommitError::NotOnThisBranch(seq));
        }
        self.commit(author, why, inverse)
    }

    /// Go to any event, on any branch. Nothing is lost, and the next commit follows from here, which
    /// starts a new branch if this isn't the end of one. Going takes the shortest way, back to where
    /// the two branches split and on from there, so only what differs has to run again.
    pub fn checkout(&mut self, seq: u64, author: &str, why: &str) -> Result<(), CommitError> {
        if seq as usize >= self.events.len() {
            return Err(CommitError::NoSuchEvent(seq));
        }
        if self.head == Some(seq) {
            return Ok(());
        }
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        // This is the moment it happens, as for a commit.
        if let Some(disk) = &mut self.disk {
            disk.append(&[Record::Head { seq, at, author: author.to_string(), why: why.to_string() }]).map_err(CommitError::Io)?;
        }
        let from = self.head.map_or(Vec::new(), |h| self.ancestors(h));
        let to = self.ancestors(seq);
        let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
        for s in from[shared..].iter().rev() {
            for op in self.events[*s as usize].inverse.clone() {
                apply(&mut self.graph, &op).expect("an inverse applies");
            }
        }
        for s in &to[shared..] {
            for op in self.events[*s as usize].ops.clone() {
                apply(&mut self.graph, &op).expect("logged ops replay");
            }
        }
        self.head = Some(seq);
        self.write_all_files().map_err(|error| {
            if let Some(disk) = &mut self.disk {
                disk.stale = true;
            }
            CommitError::FilesBehind { seq, error }
        })
    }

    /// Bring in a graph edited outside the log, such as by hand or `git pull`. The files are what
    /// they are, so this always records, as ordinary ops worked out by diffing, and reports any
    /// checks the edit now breaks rather than refusing it.
    pub fn reconcile(&mut self, disk: &SavedGraph, author: &str) -> Result<Vec<Violation>, CommitError> {
        let ops = diff(&self.graph.save(), disk);
        if !ops.is_empty() {
            self.record_unchecked(author, "external edit", ops)?;
        }
        Ok(self.violations())
    }

    /// Look at the files again, and record any difference as an edit made outside the log. Returns
    /// the event if there was one. Files this log wrote itself never differ, so it's cheap to call
    /// whenever something on disk may have changed.
    pub fn reload(&mut self, author: &str) -> Result<Option<u64>, DirError> {
        let Some(dir) = self.disk.as_ref().map(|d| d.dir.clone()) else { return Ok(None) };
        let ops = diff(&self.graph.save(), &read_files(&dir)?);
        if ops.is_empty() {
            return Ok(None);
        }
        match self.record_unchecked(author, "external edit", ops) {
            Ok(seq) => Ok(Some(seq)),
            Err(CommitError::FilesBehind { seq, .. }) => Ok(Some(seq)),
            Err(CommitError::Io(e)) => Err(DirError::Io(e)),
            Err(other) => unreachable!("{other:?}"),
        }
    }

    fn record_unchecked(&mut self, author: &str, why: &str, ops: Vec<Op>) -> Result<u64, CommitError> {
        let mut inverse = Vec::new();
        for op in &ops {
            let mut undo = apply(&mut self.graph, op).expect("a diff applies");
            undo.extend(std::mem::take(&mut inverse));
            inverse = undo;
        }
        self.record(author, why, ops, inverse)
    }

    /// The graph already holds these ops. Make them permanent, or take them back out.
    fn record(&mut self, author: &str, why: &str, ops: Vec<Op>, inverse: Vec<Op>) -> Result<u64, CommitError> {
        let seq = self.events.len() as u64;
        let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let event = Event { seq, parent: self.head, at, author: author.to_string(), why: why.to_string(), ops, inverse };

        // This is the moment it happens.
        if let Some(disk) = &mut self.disk
            && let Err(e) = disk.append(&[Record::Event(event.clone())])
        {
            for op in &event.inverse {
                apply(&mut self.graph, op).expect("an inverse applies");
            }
            return Err(CommitError::Io(e));
        }
        let touched: BTreeSet<Target> = event.ops.iter().map(Op::target).collect();
        self.events.push(event);
        self.head = Some(seq);
        self.write_touched(&touched).map_err(|error| CommitError::FilesBehind { seq, error })?;
        Ok(seq)
    }

    fn write_touched(&mut self, touched: &BTreeSet<Target>) -> Result<(), DirError> {
        let Some(disk) = &self.disk else { return Ok(()) };
        if disk.stale {
            return self.write_all_files();
        }
        let result = self.write_only(touched);
        if result.is_err()
            && let Some(disk) = &mut self.disk
        {
            disk.stale = true;
        }
        result
    }

    /// Just the files these changes affect, which is what keeps a commit from costing the whole
    /// graph. An edge lives with its source.
    fn write_only(&mut self, touched: &BTreeSet<Target>) -> Result<(), DirError> {
        let last = self.last_seq();
        let ids: BTreeSet<Entity> = touched
            .iter()
            .map(|t| match *t {
                Target::Node(e) | Target::Edge(e, _) => e,
            })
            .collect();
        let saved = self.graph.save_only(ids.iter().copied().filter(|e| self.graph.contains(*e)));
        let disk = self.disk.as_mut().expect("checked by the caller");
        let (dir, crash) = (disk.dir.clone(), &mut disk.crash_after);
        let mut tick = || step(crash);
        for node in &saved.nodes {
            write_if_changed(&node_path(&dir, node.id), &serde_yaml_ng::to_string(node)?, &mut tick)?;
        }
        let kept: BTreeSet<Entity> = saved.nodes.iter().map(|n| n.id).collect();
        for gone in ids.difference(&kept) {
            let path = node_path(&dir, *gone);
            if path.exists() {
                fs::remove_file(path)?;
                tick();
            }
        }
        // Everything has to be durable before anything says it's applied.
        sync_dir(&dir.join("nodes"))?;
        disk.mark_applied(last)?;
        Ok(())
    }

    fn write_all_files(&mut self) -> Result<(), DirError> {
        let (saved, last) = (self.graph.save(), self.last_seq());
        let Some(disk) = &mut self.disk else { return Ok(()) };
        let dir = disk.dir.clone();
        let crash = &mut disk.crash_after;
        write_files(&saved, &dir, &mut || step(crash))?;
        disk.mark_applied(last)?;
        disk.stale = false;
        Ok(())
    }
}

/// A lock belongs to the open file, so a process being spawned elsewhere in this program holds a
/// copy for an instant until it execs, even for a log that was just closed. Only someone who keeps
/// holding it is really using it.
fn take_lock(lock: &File) -> Result<(), OpenError> {
    let give_up = Instant::now() + Duration::from_millis(500);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
            Err(fs::TryLockError::WouldBlock) if Instant::now() >= give_up => return Err(OpenError::Locked),
            Err(fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

/// The log's text, and how much of the end had to be cut off. A line only counts once its newline
/// is there, so an unfinished write is never mistaken for a commit.
fn read_log(path: &Path) -> Result<(Vec<Record>, u64), OpenError> {
    let text = match fs::read(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(e.into()),
    };
    let good = text.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let torn = (text.len() - good) as u64;
    if torn > 0 {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(good as u64)?;
        file.sync_all()?;
    }
    let mut records = Vec::new();
    for (i, line) in text[..good].split(|b| *b == b'\n').enumerate() {
        if !line.is_empty() {
            records.push(serde_json::from_slice(line).map_err(|e| OpenError::Corrupt { line: i + 1, detail: e.to_string() })?);
        }
    }
    Ok((records, torn))
}

// ---------- applying ----------

/// What's written on an entity, as the ops that would write it back.
fn rewrite(g: &Graph, on: On) -> Vec<Op> {
    g.written(on.entity()).into_iter().filter_map(|(cell, b)| Some(Op::Set { on, cell, to: Some(Stored::of(b)?) })).collect()
}

/// Apply one op, returning what exactly undoes it, in order.
pub(crate) fn apply(g: &mut Graph, op: &Op) -> Result<Vec<Op>, String> {
    let node = |g: &Graph, id: &Entity| if g.contains(*id) { Ok(()) } else { Err(format!("{id} doesn't exist")) };
    match op {
        Op::Spawn { id } => {
            if g.contains(*id) {
                return Err(format!("{id} already exists"));
            }
            g.adopt(*id);
            Ok(vec![Op::Despawn { id: *id }])
        }
        Op::Despawn { id } => {
            node(g, id)?;
            let mut undo = vec![Op::Spawn { id: *id }];
            undo.extend(g.given(*id).into_iter().map(|component| Op::Add { id: *id, component }));
            undo.extend(rewrite(g, On::Node { id: *id }));
            if !g.despawn(*id) {
                return Err(format!("{id} still has edges"));
            }
            Ok(undo)
        }
        Op::Add { id, component } => {
            node(g, id)?;
            if g.given(*id).contains(component) {
                return Ok(vec![]);
            }
            g.add(*id, component);
            Ok(vec![Op::Remove { id: *id, component: component.clone() }])
        }
        Op::Remove { id, component } => {
            node(g, id)?;
            if !g.given(*id).contains(component) {
                return Err(format!("{id} doesn't have {component}"));
            }
            g.remove(*id, component);
            Ok(vec![Op::Add { id: *id, component: component.clone() }])
        }
        Op::Link { from, to, component } => {
            node(g, from)?;
            node(g, to)?;
            if g.linked(*from, *to, component) {
                return Ok(vec![]);
            }
            g.link(*from, *to, component);
            Ok(vec![Op::Unlink { from: *from, to: *to, component: component.clone() }])
        }
        Op::Unlink { from, to, component } => {
            if !g.linked(*from, *to, component) {
                return Err(format!("the edge {from} → {to} doesn't have {component}"));
            }
            let edge = Entity::edge(*from, *to);
            let last = g.given(edge).len() == 1;
            let mut undo = vec![Op::Link { from: *from, to: *to, component: component.clone() }];
            if last {
                undo.extend(rewrite(g, On::Edge { from: *from, to: *to }));
            }
            g.unlink(*from, *to, component);
            Ok(undo)
        }
        Op::Set { on, cell, to } => {
            let e = on.entity();
            match on {
                On::Node { id } => node(g, id)?,
                On::Edge { from, to } if !g.is_edge(e) => return Err(format!("there's no edge {from} → {to}")),
                On::Edge { .. } => {}
            }
            let old = g.written(e).remove(cell).and_then(Stored::of);
            match to {
                Some(Stored::Value(v)) => g.set_value(e, cell, v.clone()),
                Some(Stored::Formula(text)) => g.set_formula(e, cell, text),
                None => g.clear(e, cell),
            };
            Ok(vec![Op::Set { on: *on, cell: cell.clone(), to: old }])
        }
    }
}

/// Apply `ops` if nothing new breaks, returning what undoes them. Otherwise leave `g` as it was.
fn try_apply(g: &mut Graph, ops: &[Op]) -> Result<Vec<Op>, Vec<Violation>> {
    let touched: BTreeSet<Target> = ops.iter().map(Op::target).collect();
    let mut before: BTreeSet<Violation> = check_values(g, &touched).into_iter().collect();
    before.extend(check_rules(g, Severity::Invariant));

    let mut inverse: Vec<Op> = Vec::new();
    let rollback = |g: &mut Graph, inverse: &[Op]| {
        for op in inverse {
            apply(g, op).expect("an inverse applies");
        }
    };
    for op in ops {
        match apply(g, op) {
            Ok(mut undo) => {
                undo.append(&mut inverse);
                inverse = undo;
            }
            Err(detail) => {
                rollback(g, &inverse);
                return Err(vec![Violation { rule: "op_failed".to_string(), on: op.target(), detail }]);
            }
        }
    }
    let mut new = check_values(g, &touched);
    new.extend(check_rules(g, Severity::Invariant));
    new.retain(|v| !before.contains(v));
    if !new.is_empty() {
        rollback(g, &inverse);
        new.sort();
        new.dedup();
        return Err(new);
    }
    Ok(inverse)
}

// ---------- checking ----------

/// What the definitions say about these targets, which doesn't depend on anything else, so a commit
/// only has to look at what it touched.
fn check_values(g: &Graph, targets: &BTreeSet<Target>) -> Vec<Violation> {
    let reg = g.registry();
    let mut out = Vec::new();
    for t in targets {
        let (e, place, what) = match *t {
            Target::Node(e) if g.contains(e) => (e, Place::Node, e.to_string()),
            Target::Edge(from, to) if g.is_edge(Entity::edge(from, to)) => {
                if from == to {
                    out.push(Violation { rule: "no_self_links".to_string(), on: *t, detail: format!("{from} links to itself") });
                }
                (Entity::edge(from, to), Place::Edge, format!("{from} → {to}"))
            }
            _ => continue,
        };
        for c in g.given(e) {
            match reg.component_def(&c) {
                None => out.push(Violation { rule: "unknown_component".to_string(), on: *t, detail: format!("{what} has {c}, which no system provides") }),
                Some(def) if def.place != place => out.push(Violation { rule: "component_on_wrong_target".to_string(), on: *t, detail: format!("{what} has {c}, which goes on {}", if def.place == Place::Node { "a node" } else { "an edge" }) }),
                Some(_) => {}
            }
        }
        for (cell, b) in g.written(e) {
            let def = reg.cell_def(&cell);
            if let Some(def) = def
                && !g.has(e, &def.component)
            {
                out.push(Violation { rule: "cell_without_component".to_string(), on: *t, detail: format!("{what}.{cell} belongs to {}, which it doesn't have", def.component) });
            }
            match b {
                Backed::Value(v) => {
                    if let Some(Err(why)) = def.and_then(|d| d.validate).map(|f| f(&v)) {
                        out.push(Violation { rule: "malformed_cell".to_string(), on: *t, detail: format!("{what}.{cell}: {why}") });
                    }
                }
                Backed::Formula(_) => {
                    if let Some(error) = g.formula_error(e, &cell) {
                        out.push(Violation { rule: "formula_cannot_be_read".to_string(), on: *t, detail: format!("{what}.{cell}: {error:?}") });
                    }
                }
                Backed::Builtin => {}
            }
        }
    }
    out
}

fn all_targets(g: &Graph) -> BTreeSet<Target> {
    let nodes = g.entities().map(Target::Node);
    let edges = g.edges().map(|(_, f, t)| Target::Edge(f, t));
    nodes.chain(edges).collect()
}

/// Every check of this severity, on everything with its component. They're memoised, so this only
/// reruns the ones an edit reached.
fn check_rules(g: &Graph, severity: Severity) -> Vec<Violation> {
    let mut out = Vec::new();
    let entities: Vec<(Entity, Target)> = g.entities().map(|e| (e, Target::Node(e))).chain(g.edges().map(|(e, f, t)| (e, Target::Edge(f, t)))).collect();
    for (component, check) in g.registry().checks().filter(|(_, c)| c.severity == severity) {
        let cell = check.cell();
        for (e, target) in entities.iter().filter(|(e, _)| g.has(*e, component)) {
            let details = match g.get_value(*e, &cell) {
                Ok(Some(v)) => serde_json::from_value::<Vec<String>>(v).unwrap_or_default(),
                Ok(None) => Vec::new(),
                Err(Error::Cycle(_)) => vec!["couldn't be checked: it reads itself".to_string()],
                Err(error) => vec![format!("couldn't be checked: {error:?}")],
            };
            for detail in details {
                out.push(Violation { rule: check.name.clone(), on: *target, detail });
            }
        }
    }
    out
}

// ---------- files and the log ----------

fn by_id(g: &SavedGraph) -> BTreeMap<Entity, &SavedNode> {
    g.nodes.iter().map(|n| (n.id, n)).collect()
}

fn same_graph(a: &SavedGraph, b: &SavedGraph) -> bool {
    by_id(a) == by_id(b)
}

/// What the files are saying, given the state they might still be catching up from and the state
/// they should now be in. `disk` may also mention nodes only as edge targets, which get an empty
/// node, as a load would.
fn merge(disk: &SavedGraph, base: &SavedGraph, current: &SavedGraph, problems: &mut Vec<LoadProblem>) -> SavedGraph {
    let (d, b, c) = (by_id(disk), by_id(base), by_id(current));
    let ids: BTreeSet<Entity> = d.keys().chain(b.keys()).chain(c.keys()).copied().collect();
    let mut nodes = Vec::new();
    for id in ids {
        let (on_disk, was, is) = (d.get(&id).copied(), b.get(&id).copied(), c.get(&id).copied());
        let chosen = if on_disk == was || on_disk == is { is } else { on_disk };
        nodes.extend(chosen.cloned());
    }
    let known: BTreeSet<Entity> = nodes.iter().map(|n| n.id).collect();
    let mut dangling = BTreeSet::new();
    for n in &nodes {
        for link in n.links.iter().filter(|l| !known.contains(&l.to)) {
            problems.push(LoadProblem::DanglingLink { from: n.id, to: link.to });
            dangling.insert(link.to);
        }
    }
    nodes.extend(dangling.into_iter().map(SavedNode::empty));
    nodes.sort_by_key(|n| n.id);
    SavedGraph { nodes }
}

fn cells_of(cells: &BTreeMap<String, Value>, formulas: &BTreeMap<String, String>) -> BTreeMap<String, Stored> {
    let values = cells.iter().map(|(c, v)| (c.clone(), Stored::Value(v.clone())));
    values.chain(formulas.iter().map(|(c, f)| (c.clone(), Stored::Formula(f.clone())))).collect()
}

/// The ops that turn one cell list into another.
fn set_ops(on: On, was: BTreeMap<String, Stored>, is: BTreeMap<String, Stored>, ops: &mut Vec<Op>) {
    for (c, v) in &is {
        if was.get(c) != Some(v) {
            ops.push(Op::Set { on, cell: c.clone(), to: Some(v.clone()) });
        }
    }
    ops.extend(was.keys().filter(|c| !is.contains_key(*c)).map(|c| Op::Set { on, cell: c.clone(), to: None }));
}

/// The ops that turn one written-down graph into another: new nodes first and gone nodes last, so
/// they all apply.
fn diff(from: &SavedGraph, to: &SavedGraph) -> Vec<Op> {
    let (old, new) = (by_id(from), by_id(to));
    let empty = SavedNode::empty(Entity::whole());
    let mut ops: Vec<Op> = new.keys().filter(|id| !old.contains_key(id)).map(|id| Op::Spawn { id: *id }).collect();
    let (mut edge_ops, mut removals) = (Vec::new(), Vec::new());
    for id in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (old.get(id).copied().unwrap_or(&empty), new.get(id).copied().unwrap_or(&empty));
        if new.contains_key(id) {
            ops.extend(b.components.iter().filter(|c| !a.components.contains(c)).map(|c| Op::Add { id: *id, component: c.clone() }));
            set_ops(On::Node { id: *id }, cells_of(&a.cells, &a.formulas), cells_of(&b.cells, &b.formulas), &mut ops);
            removals.extend(a.components.iter().filter(|c| !b.components.contains(c)).map(|c| Op::Remove { id: *id, component: c.clone() }));
        }
        let edges = |n: &SavedNode| -> BTreeMap<Entity, SavedEdge> { n.links.iter().map(|l| (l.to, l.clone())).collect() };
        let (was, is) = (edges(a), edges(b));
        for t in was.keys().chain(is.keys()).collect::<BTreeSet<_>>() {
            let blank = SavedEdge { to: *t, components: Vec::new(), cells: BTreeMap::new(), formulas: BTreeMap::new() };
            let (x, y) = (was.get(t).unwrap_or(&blank), is.get(t).unwrap_or(&blank));
            edge_ops.extend(y.components.iter().filter(|c| !x.components.contains(c)).map(|c| Op::Link { from: *id, to: *t, component: c.clone() }));
            if !y.components.is_empty() {
                set_ops(On::Edge { from: *id, to: *t }, cells_of(&x.cells, &x.formulas), cells_of(&y.cells, &y.formulas), &mut edge_ops);
            }
            edge_ops.extend(x.components.iter().filter(|c| !y.components.contains(c)).map(|c| Op::Unlink { from: *id, to: *t, component: c.clone() }));
        }
    }
    ops.append(&mut edge_ops);
    ops.append(&mut removals);
    ops.extend(old.keys().filter(|id| !new.contains_key(id)).map(|id| Op::Despawn { id: *id }));
    ops
}
