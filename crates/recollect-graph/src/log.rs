//! The action log. A graph held by a `Log` only changes through `commit`, so its state is the
//! replay of its events, and every fact can answer who set it, when and why.
//!
//! ```
//! # use std::sync::Arc;
//! # use recollect_graph::{Log, Op, Registry, component};
//! #[component]
//! struct Note(String);
//!
//! let mut log = Log::new(Arc::new(Registry::new()));
//! let a = recollect_graph::Entity::new();
//! let seq = log.commit("steven", "first thoughts", vec![Op::set(a, Note("hello".into()))]).unwrap();
//! assert_eq!(log.graph().get(a), Ok(Some(Note("hello".into()))));
//! log.undo(seq, "steven", "changed my mind").unwrap();
//! assert_eq!(log.graph().get::<Note>(a), Ok(None));
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

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::graph::{Backing, Component, Entity, Error, Graph, Runtime, Storage};
use crate::registry::{Code, Place, Registry, SavedRule, Severity};
use crate::saved::{DirError, LoadProblem, SavedDefaults, SavedGraph, SavedLink, SavedNode, node_path, read_files, remove_temp_files, sync_dir, write_atomically, write_files, write_if_changed};

/// What backs a cell, written down. A closure has no written form, so it can't get into a log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stored {
    Value(Value),
    Rule(SavedRule),
}

/// The only ways a logged graph changes. Each has an exact inverse, so undo is just more ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// A node with nothing on it.
    Spawn { id: Entity },
    /// Remove a node and every cell on it. Its links have to go first.
    Despawn { id: Entity },
    /// Back one cell, creating the node if it's new. `None` removes the cell.
    Set { id: Entity, component: String, to: Option<Stored> },
    /// Set one kind on a link. `None` removes it, and removing the last kind removes the link.
    Link {
        from: Entity,
        to: Entity,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
        value: Option<Value>,
    },
    /// What backs a component on every node that doesn't say otherwise. `None` goes back to what
    /// the code says.
    Default { component: String, to: Option<Stored> },
}

/// A link value can be `null`, as a unit component is, so `null` here still means it's there.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

impl Op {
    /// Back a cell with a value or a registered rule. A closure can't be logged, so passing one
    /// is a programming error and panics.
    pub fn set<C: Component>(id: Entity, storage: impl Into<Storage<C>>) -> Op {
        let to = match storage.into() {
            Storage::Value(v) => Stored::Value(serde_json::to_value(&v).expect("components serialise")),
            Storage::Rule(r) => Stored::Rule(r.saved.unwrap_or_else(|| panic!("rule {} is a closure, so it can't be logged: register it", r.name))),
        };
        Op::Set { id, component: C::NAME.to_string(), to: Some(to) }
    }

    pub fn set_runtime(id: Entity, component: &str, value: Value) -> Op {
        Op::Set { id, component: component.to_string(), to: Some(Stored::Value(value)) }
    }

    /// A registered rule by its key.
    pub fn set_rule(id: Entity, component: &str, rule: &str, params: Value) -> Op {
        Op::Set { id, component: component.to_string(), to: Some(Stored::Rule(SavedRule { rule: rule.to_string(), params })) }
    }

    pub fn remove<C: Component>(id: Entity) -> Op {
        Op::Set { id, component: C::NAME.to_string(), to: None }
    }

    pub fn link<L: Component>(from: Entity, to: Entity, value: L) -> Op {
        Op::link_runtime(from, to, Runtime::of(&value))
    }

    pub fn link_runtime(from: Entity, to: Entity, c: Runtime) -> Op {
        Op::Link { from, to, kind: c.name, value: Some(c.value) }
    }

    pub fn unlink<L: Component>(from: Entity, to: Entity) -> Op {
        Op::Link { from, to, kind: L::NAME.to_string(), value: None }
    }

    pub fn target(&self) -> Target {
        match self {
            Op::Spawn { id } | Op::Despawn { id } | Op::Set { id, .. } => Target::Node(*id),
            Op::Link { from, to, .. } => Target::Link(*from, *to),
            Op::Default { .. } => Target::Defaults,
        }
    }
}

/// What an op changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Node(Entity),
    Link(Entity, Entity),
    Defaults,
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

/// What a cell read as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Seen {
    Absent,
    Value(Value),
    Error(String),
}

/// A record that a cell read as this just after this event, under the code current then. Derived
/// values aren't otherwise kept, so this is what lets a later audit tell if the code has moved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Witness {
    pub seq: Option<u64>,
    pub node: Entity,
    pub component: String,
    pub seen: Seen,
}

/// A witnessed cell that reads differently now, from the same stored facts. So the code changed
/// what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct Drift {
    pub witness: Witness,
    pub now: Seen,
    pub build_then: Option<String>,
    pub build_now: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum Record {
    /// The code in use from this seq on.
    Code { from: u64, code: Code },
    Event(Event),
    Witness(Witness),
    /// The head moved here, with no new event.
    Head { seq: u64, at: u64, author: String, why: String },
}

#[derive(Debug)]
pub enum CommitError {
    /// Nothing changed: these rules would be newly broken, or the ops don't apply.
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
    codes: Vec<(u64, Code)>,
    witnesses: Vec<Witness>,
    disk: Option<Disk>,
}

impl Log {
    /// A log that lives only in memory.
    pub fn new(registry: Arc<Registry>) -> Self {
        Log { graph: Graph::with_registry(registry), events: Vec::new(), head: None, codes: Vec::new(), witnesses: Vec::new(), disk: None }
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
                Record::Code { from, code } => log.codes.push((from, code)),
                Record::Witness(w) => log.witnesses.push(w),
                Record::Head { seq, .. } => log.head = Some(seq),
                Record::Event(mut ev) => {
                    // A log from before there were branches says nothing, and is one straight line.
                    if ev.parent.is_none() && ev.seq > 0 {
                        ev.parent = Some(ev.seq - 1);
                    }
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
        let replayed = log.saved();
        // The files may be as old as the state at `applied`, which can even be on another branch.
        let base = match applied {
            None => SavedGraph::default(),
            Some(a) if Some(a) != log.head && (a as usize) < log.events.len() => log.state_at(a).save().expect("only written-down rules reach a log"),
            Some(_) => replayed.clone(),
        };
        let last = log.last_seq();

        // A file that matches what it should be, or what it last was, isn't an edit. It's at most
        // behind. Anything else was changed by someone else, so it stands.
        let on_disk = read_files(dir)?;
        let intended = merge(&on_disk, &base, &replayed, &mut problems);

        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        log.disk = Some(Disk { dir: dir.to_path_buf(), log: file, _lock: lock, stale: false, crash_after });
        if let Some(code) = log.codes.last().map(|(_, c)| c.clone())
            && code != registry.code()
        {
            problems.push(LoadProblem::CodeChanged { was: code, now: registry.code() });
        }
        if let Some(ops) = Some(diff(&replayed, &intended)).filter(|ops| !ops.is_empty()) {
            let seq = log.record_unchecked(author, "external edit", ops).map_err(|e| match e {
                CommitError::Io(e) => OpenError::Dir(DirError::Io(e)),
                CommitError::FilesBehind { error, .. } => OpenError::Dir(error),
                other => unreachable!("{other:?}"),
            })?;
            problems.push(LoadProblem::ExternalEdit { seq });
        }

        // Anything still out of step is only behind.
        let now = log.saved();
        if applied != log.last_seq() || last != log.last_seq() || !same_graph(&on_disk, &now) {
            log.write_all_files().map_err(OpenError::Dir)?;
        }

        for e in log.graph.entities() {
            for (name, b) in log.graph.cells.get(&e).into_iter().flatten() {
                if let Backing::Broken(_, error) = b {
                    problems.push(LoadProblem::BrokenRule((e, name.clone()), error.clone()));
                }
            }
        }
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

    fn saved(&self) -> SavedGraph {
        self.graph.save().expect("only written-down rules reach a log, and each resolved against this registry")
    }

    /// The graph as it was just after event `seq`, with its derived values worked out by the code
    /// running now.
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

    /// Who last gave a cell or link kind a value on the way here, and why, if anyone has.
    pub fn blame(&self, target: Target, component: &str) -> Option<&Event> {
        self.path().into_iter().rev().find(|ev| {
            ev.ops.iter().any(|op| {
                op.target() == target
                    && match op {
                        Op::Set { component: c, to: Some(_), .. } | Op::Default { component: c, to: Some(_) } => c == component,
                        Op::Link { kind, value: Some(_), .. } => kind == component,
                        _ => false,
                    }
            })
        })
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

    /// The code in use at this event, if it was recorded.
    pub fn code_at(&self, seq: u64) -> Option<&Code> {
        self.codes.iter().rev().find(|(from, _)| *from <= seq).map(|(_, c)| c)
    }

    pub fn witnesses(&self) -> &[Witness] {
        &self.witnesses
    }

    // ---------- writing ----------

    /// Apply `ops` atomically, or refuse them and change nothing. Rules already broken, such as
    /// by a hand edit, don't block a commit unless it breaks them in a new way.
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
    /// rules the edit now breaks rather than refusing it.
    pub fn reconcile(&mut self, disk: &SavedGraph, author: &str) -> Result<Vec<Violation>, CommitError> {
        let ops = diff(&self.saved(), disk);
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
        let ops = diff(&self.saved(), &read_files(&dir)?);
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

    /// Record what a cell reads as now, for a later audit to compare.
    pub fn witness(&mut self, node: Entity, component: &str) -> Result<Witness, CommitError> {
        let w = Witness { seq: self.last_seq(), node, component: component.to_string(), seen: seen(&self.graph, node, component) };
        if let Some(disk) = &mut self.disk {
            disk.append(&[Record::Witness(w.clone())]).map_err(CommitError::Io)?;
        }
        self.witnesses.push(w.clone());
        Ok(w)
    }

    /// Replay the log under the code running now, and compare every witnessed cell with what it
    /// read as at the time. The stored facts are the same by construction, so a difference means
    /// the code changed what they mean, even if no rule's key or signature did.
    pub fn audit(&self) -> Vec<Drift> {
        let build_now = self.graph.registry.code().build;
        let mut drift = Vec::new();
        let mut g = Graph::with_registry(self.graph.registry.clone());
        let mut compare = |g: &Graph, seq: Option<u64>| {
            for w in self.witnesses.iter().filter(|w| w.seq == seq) {
                let now = seen(g, w.node, &w.component);
                if now != w.seen {
                    let build_then = w.seq.and_then(|s| self.code_at(s)).map(|c| c.build.clone());
                    drift.push(Drift { witness: w.clone(), now, build_then, build_now: build_now.clone() });
                }
            }
        };
        compare(&g, None);
        // Each from nothing, since witnesses can be on any branch.
        let witnessed: BTreeSet<u64> = self.witnesses.iter().filter_map(|w| w.seq).collect();
        for seq in witnessed {
            g = self.state_at(seq);
            compare(&g, Some(seq));
        }
        drift
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

        let code = self.graph.registry.code();
        let new_code = self.codes.last().is_none_or(|(_, c)| *c != code);
        let mut records = Vec::new();
        if new_code {
            records.push(Record::Code { from: seq, code: code.clone() });
        }
        records.push(Record::Event(event.clone()));

        // This is the moment it happens.
        if let Some(disk) = &mut self.disk
            && let Err(e) = disk.append(&records)
        {
            for op in &event.inverse {
                apply(&mut self.graph, op).expect("an inverse applies");
            }
            return Err(CommitError::Io(e));
        }
        if new_code {
            self.codes.push((seq, code));
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
    /// graph. A link lives with its source.
    fn write_only(&mut self, touched: &BTreeSet<Target>) -> Result<(), DirError> {
        let last = self.last_seq();
        let mut ids = BTreeSet::new();
        for t in touched {
            match t {
                Target::Node(e) | Target::Link(e, _) => ids.insert(*e),
                Target::Defaults => false,
            };
        }
        let saved = self.graph.save_only(ids.iter().copied().filter(|e| self.graph.entities.contains(e))).map_err(DirError::Save)?;
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
        if touched.contains(&Target::Defaults) {
            write_if_changed(&dir.join("graph.yaml"), &serde_yaml_ng::to_string(&saved.defaults)?, &mut tick)?;
        }
        // Everything has to be durable before anything says it's applied.
        sync_dir(&dir.join("nodes"))?;
        disk.mark_applied(last)?;
        Ok(())
    }

    fn write_all_files(&mut self) -> Result<(), DirError> {
        let (saved, last) = (self.saved(), self.last_seq());
        let Some(disk) = &mut self.disk else { return Ok(()) };
        let dir = disk.dir.clone();
        let crash = &mut disk.crash_after;
        write_files(&saved, &dir, &mut || step(crash))?;
        disk.mark_applied(last)?;
        disk.stale = false;
        Ok(())
    }
}

fn seen(g: &Graph, node: Entity, component: &str) -> Seen {
    match g.get_runtime(node, component) {
        Ok(None) => Seen::Absent,
        Ok(Some(r)) => Seen::Value(r.value),
        // Which cell a loop is said to start from can depend on what was asked first.
        Err(Error::Cycle(_)) => Seen::Error("a cycle".to_string()),
        Err(e) => Seen::Error(format!("{e:?}")),
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

fn backing_of(g: &Graph, id: Entity, component: &str, to: &Stored) -> Backing {
    match to {
        Stored::Value(v) => Backing::Stored(v.clone()),
        Stored::Rule(saved) => g.registry.resolve(&(id, component.to_string()), saved),
    }
}

fn stored_of(b: &Backing) -> Stored {
    match b {
        Backing::Stored(v) => Stored::Value(v.clone()),
        Backing::Broken(saved, _) => Stored::Rule(saved.clone()),
        Backing::Computed(r) => Stored::Rule(r.saved.clone().expect("a closure can't get into a log")),
    }
}

/// Apply one op, returning what exactly undoes it, in order.
pub(crate) fn apply(g: &mut Graph, op: &Op) -> Result<Vec<Op>, String> {
    match op {
        Op::Spawn { id } => {
            if g.entities.contains(id) {
                return Err(format!("{id} already exists"));
            }
            g.adopt(*id);
            Ok(vec![Op::Despawn { id: *id }])
        }
        Op::Despawn { id } => {
            if !g.entities.contains(id) {
                return Err(format!("{id} doesn't exist"));
            }
            let mut undo = vec![Op::Spawn { id: *id }];
            for (name, b) in g.cells.get(id).into_iter().flatten() {
                undo.push(Op::Set { id: *id, component: name.clone(), to: Some(stored_of(b)) });
            }
            if !g.despawn(*id) {
                return Err(format!("{id} still has links"));
            }
            Ok(undo)
        }
        Op::Set { id, component, to } => {
            let existed = g.entities.contains(id);
            if !existed && to.is_none() {
                return Err(format!("{id} doesn't exist"));
            }
            let old = g.cells.get(id).and_then(|c| c.get(component)).map(stored_of);
            g.adopt(*id);
            match to {
                Some(to) => {
                    let backing = backing_of(g, *id, component, to);
                    g.back(*id, component, backing);
                }
                None => {
                    g.remove_named(*id, component);
                }
            }
            // Creating something is undone by removing it, not by leaving it empty.
            Ok(if existed { vec![Op::Set { id: *id, component: component.clone(), to: old }] } else { vec![Op::Despawn { id: *id }] })
        }
        Op::Link { from, to, kind, value } => {
            if !g.entities.contains(from) || !g.entities.contains(to) {
                return Err(format!("link {from} → {to} needs both ends to exist"));
            }
            let bag = g.links.get(&(*from, *to));
            if bag.is_none() && value.is_none() {
                return Err(format!("link {from} → {to} doesn't exist"));
            }
            let old = bag.and_then(|b| b.get(kind)).cloned();
            match value {
                Some(v) => g.link_runtime(*from, *to, Runtime::new(kind, v.clone())),
                None => g.unlink_named(*from, *to, kind),
            };
            Ok(vec![Op::Link { from: *from, to: *to, kind: kind.clone(), value: old }])
        }
        Op::Default { component, to } => {
            let from_code = g.registry.defaults.get(component).cloned();
            let old = g.defaults.get(component).filter(|b| !from_code.as_ref().is_some_and(|c| crate::saved::same(c, b))).map(stored_of);
            let backing = match to {
                Some(to) => Some(backing_of(g, Entity::nil(), component, to)),
                None => from_code,
            };
            g.back_default(component, backing);
            Ok(vec![Op::Default { component: component.clone(), to: old }])
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

/// What declarations say about these targets, which doesn't depend on anything else, so a commit
/// only has to look at what it touched.
fn check_values(g: &Graph, targets: &BTreeSet<Target>) -> Vec<Violation> {
    let mut out = Vec::new();
    let declared = &g.registry.declared;
    let mut check = |on: Target, place: Place, what: String, c: &str, v: Option<&Value>| {
        let Some(d) = declared.get(c) else { return };
        if d.place != place {
            out.push(Violation { rule: "component_on_wrong_target".to_string(), on, detail: format!("{what} holds {c}, which goes on a {}", if d.place == Place::Node { "node" } else { "link" }) });
        }
        if let (Some(v), Some(validate)) = (v, d.validate)
            && let Err(e) = validate(v)
        {
            out.push(Violation { rule: "malformed_component".to_string(), on, detail: format!("{what}.{c}: {e}") });
        }
    };
    let mut broken = Vec::new();
    for t in targets {
        match *t {
            Target::Node(e) => {
                for (c, b) in g.cells.get(&e).into_iter().flatten() {
                    let value = if let Backing::Stored(v) = b { Some(v) } else { None };
                    check(*t, Place::Node, e.to_string(), c, value);
                    if let Backing::Broken(_, error) = b {
                        broken.push(Violation { rule: "rule_cannot_run".to_string(), on: *t, detail: format!("{e}.{c}: {error:?}") });
                    }
                }
            }
            Target::Link(from, to) => {
                for (c, v) in g.links.get(&(from, to)).into_iter().flatten() {
                    check(*t, Place::Link, format!("{from} → {to}"), c, Some(v));
                }
                if from == to && g.links.contains_key(&(from, to)) {
                    broken.push(Violation { rule: "no_self_links".to_string(), on: *t, detail: format!("{from} links to itself") });
                }
            }
            Target::Defaults => {
                for (c, b) in &g.defaults {
                    if let Backing::Broken(_, error) = b {
                        broken.push(Violation { rule: "rule_cannot_run".to_string(), on: *t, detail: format!("{c}: {error:?}") });
                    }
                }
            }
        }
    }
    out.append(&mut broken);
    out
}

fn all_targets(g: &Graph) -> BTreeSet<Target> {
    let nodes = g.entities().map(Target::Node);
    let links = g.links.keys().map(|(f, t)| Target::Link(*f, *t));
    nodes.chain(links).chain([Target::Defaults]).collect()
}

/// Every check of this severity, at every node. They're memoised, so this only reruns the ones an
/// edit reached.
fn check_rules(g: &Graph, severity: Severity) -> Vec<Violation> {
    let mut out = Vec::new();
    for check in g.registry.checks.iter().filter(|c| c.severity == severity) {
        let column = check.column();
        for e in g.entities() {
            let details = match g.get_runtime(e, &column) {
                Ok(Some(r)) => serde_json::from_value::<Vec<String>>(r.value).unwrap_or_default(),
                Ok(None) => Vec::new(),
                Err(Error::Cycle(_)) => vec!["couldn't be checked: it reads itself".to_string()],
                Err(error) => vec![format!("couldn't be checked: {error:?}")],
            };
            for detail in details {
                out.push(Violation { rule: check.name.clone(), on: Target::Node(e), detail });
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
    by_id(a) == by_id(b) && a.defaults == b.defaults
}

/// What the files are saying, given the state they might still be catching up from and the state
/// they should now be in. `disk` may also mention nodes only as link targets, which get an empty
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
    nodes.extend(dangling.into_iter().map(|id| SavedNode { id, components: BTreeMap::new(), rules: BTreeMap::new(), links: Vec::new() }));
    nodes.sort_by_key(|n| n.id);
    let defaults: SavedDefaults = if disk.defaults == base.defaults || disk.defaults == current.defaults { current.defaults.clone() } else { disk.defaults.clone() };
    SavedGraph { nodes, defaults }
}

fn cells_of(components: &BTreeMap<String, Value>, rules: &BTreeMap<String, SavedRule>) -> BTreeMap<String, Stored> {
    let values = components.iter().map(|(c, v)| (c.clone(), Stored::Value(v.clone())));
    values.chain(rules.iter().map(|(c, r)| (c.clone(), Stored::Rule(r.clone())))).collect()
}

fn links_of(links: &[SavedLink]) -> BTreeMap<(Entity, String), Value> {
    links.iter().flat_map(|l| l.components.iter().map(|(k, v)| ((l.to, k.clone()), v.clone()))).collect()
}

/// The ops that turn one written-down graph into another: new nodes first and gone nodes last, so
/// they all apply.
fn diff(from: &SavedGraph, to: &SavedGraph) -> Vec<Op> {
    let (old, new) = (by_id(from), by_id(to));
    let empty = SavedNode { id: Entity::nil(), components: BTreeMap::new(), rules: BTreeMap::new(), links: Vec::new() };
    let mut ops: Vec<Op> = new.keys().filter(|id| !old.contains_key(id)).map(|id| Op::Spawn { id: *id }).collect();

    let (was, is) = (cells_of(&from.defaults.components, &from.defaults.rules), cells_of(&to.defaults.components, &to.defaults.rules));
    for (c, v) in &is {
        if was.get(c) != Some(v) {
            ops.push(Op::Default { component: c.clone(), to: Some(v.clone()) });
        }
    }
    ops.extend(was.keys().filter(|c| !is.contains_key(*c)).map(|c| Op::Default { component: c.clone(), to: None }));

    let mut link_ops = Vec::new();
    for id in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        let (a, b) = (old.get(id).copied().unwrap_or(&empty), new.get(id).copied().unwrap_or(&empty));
        if new.contains_key(id) {
            let (was, is) = (cells_of(&a.components, &a.rules), cells_of(&b.components, &b.rules));
            for (c, v) in &is {
                if was.get(c) != Some(v) {
                    ops.push(Op::Set { id: *id, component: c.clone(), to: Some(v.clone()) });
                }
            }
            ops.extend(was.keys().filter(|c| !is.contains_key(*c)).map(|c| Op::Set { id: *id, component: c.clone(), to: None }));
        }
        let (was, is) = (links_of(&a.links), links_of(&b.links));
        for ((t, k), v) in &is {
            if was.get(&(*t, k.clone())) != Some(v) {
                link_ops.push(Op::Link { from: *id, to: *t, kind: k.clone(), value: Some(v.clone()) });
            }
        }
        link_ops.extend(was.keys().filter(|k| !is.contains_key(*k)).map(|(t, k)| Op::Link { from: *id, to: *t, kind: k.clone(), value: None }));
    }
    ops.append(&mut link_ops);
    ops.extend(old.keys().filter(|id| !new.contains_key(id)).map(|id| Op::Despawn { id: *id }));
    ops
}
