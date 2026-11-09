//! The open workspace. Every change goes through here, so the web UI, Claude and the file watcher
//! all see the same thing, and every change that lands is an event in the log.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use notify::{RecursiveMode, Watcher};
use recollect_graph::{CommitError, Log, Op, OpenError, Stored};
use recollect_systems::samples::{FORMAT, SAMPLES, Sample};
use recollect_systems::{Built, build};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::ops::{Resolved, resolve_ops};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    GraphChanged { by: String },
    ProposalsChanged,
    WorkspaceChanged,
}

#[derive(Debug)]
pub enum AppError {
    /// There's no workspace open to do this to.
    NothingOpen,
    NotFound(String),
    /// The change wasn't made, for reasons the person making it can act on.
    Refused(String),
    Other(anyhow::Error),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::NothingOpen => write!(f, "no workspace is open"),
            AppError::NotFound(s) => write!(f, "not found: {s}"),
            AppError::Refused(s) => write!(f, "{s}"),
            AppError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Other(e)
    }
}

/// `recollect.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(default)]
    pub systems: Vec<String>,
    /// Whether Claude may change things directly, rather than proposing them for review.
    #[serde(default)]
    pub llm_direct_writes: bool,
}

/// A change-set waiting for someone to look at it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub ops: Vec<Op>,
    pub author: String,
    /// Milliseconds since 1970.
    pub created: u64,
}

/// A proposal, and why it can't be accepted any more, if it can't.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalView {
    #[serde(flatten)]
    pub proposal: Proposal,
    pub error: Option<String>,
}

pub struct Open {
    pub dir: PathBuf,
    pub config: Config,
    pub built: Built,
    pub log: Log,
    pub proposals: Vec<Proposal>,
    /// Set for a sample's working copy.
    pub sample: Option<String>,
    _watcher: Option<notify::RecommendedWatcher>,
}

pub struct App {
    /// Where samples are copied to, so working in one never touches the originals.
    pub home: PathBuf,
    /// The samples as shipped.
    pub samples: PathBuf,
    current: Mutex<Option<Open>>,
    pub events: broadcast::Sender<Event>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn slug(s: &str) -> String {
    let mut out: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(40).collect()
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let name = path.file_name().context("a file with no name")?;
        if name == "log.lock" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(name))?;
        } else {
            std::fs::copy(&path, to.join(name))?;
        }
    }
    Ok(())
}

/// A formula written through the app or by Claude has to work when it's written, or it's refused,
/// saying why. A hand edit can't be refused, so it stands and its cell says what's wrong.
fn formulas_work(o: &Open, ops: &[Op]) -> Result<(), AppError> {
    let g = o.log.what_if(ops).map_err(|v| AppError::Refused(v.iter().map(|v| format!("{}: {}", v.rule, v.detail)).collect::<Vec<_>>().join("; ")))?;
    for op in ops {
        if let Op::Set { on, cell, to: Some(Stored::Formula(_)) } = op
            && let Err(e) = g.get_value(on.entity(), cell)
        {
            return Err(AppError::Refused(crate::views::error_text(&e, &g)));
        }
    }
    Ok(())
}

fn refusal(e: CommitError) -> AppError {
    match e {
        CommitError::Refused(v) => AppError::Refused(v.iter().map(|v| format!("{}: {}", v.rule, v.detail)).collect::<Vec<_>>().join("; ")),
        CommitError::NoSuchEvent(seq) => AppError::NotFound(format!("event {seq}")),
        CommitError::NotOnThisBranch(seq) => AppError::Refused(format!("event {seq} isn't on this branch, so there's nothing here to undo")),
        CommitError::Io(e) => AppError::Other(anyhow!(e).context("couldn't write the log")),
        CommitError::FilesBehind { seq, error } => AppError::Other(anyhow!("event {seq} happened, but the files couldn't be brought up to date: {error:?}")),
    }
}

impl App {
    pub fn new(home: PathBuf, samples: PathBuf) -> Arc<App> {
        let (events, _) = broadcast::channel(64);
        Arc::new(App { home, samples, current: Mutex::new(None), events })
    }

    fn lock(&self) -> MutexGuard<'_, Option<Open>> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Do something with the open workspace.
    pub fn with<T>(&self, f: impl FnOnce(&mut Open) -> Result<T, AppError>) -> Result<T, AppError> {
        f(self.lock().as_mut().ok_or(AppError::NothingOpen)?)
    }

    pub fn send(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub fn sample_list(&self) -> Vec<&'static Sample> {
        SAMPLES.iter().filter(|s| self.samples.join(s.name).exists()).collect()
    }

    /// Open a workspace directory, in place of whatever's open.
    pub fn open_dir(self: &Arc<Self>, dir: &Path, sample: Option<String>) -> Result<(), AppError> {
        let mut current = self.lock();
        // The log's lock has to be free, in case this is the same directory.
        *current = None;
        let opened = load(dir, sample).map_err(AppError::Other)?;
        let mut opened = opened;
        opened._watcher = watch(self.clone(), dir).map_err(AppError::Other).map(Some)?;
        *current = Some(opened);
        drop(current);
        let _ = std::fs::create_dir_all(&self.home);
        let _ = std::fs::write(self.home.join("last"), dir.display().to_string());
        self.send(Event::WorkspaceChanged);
        Ok(())
    }

    /// Open a sample's working copy, making it from the original if it isn't there or `reset` says so.
    pub fn open_sample(self: &Arc<Self>, name: &str, reset: bool) -> Result<(), AppError> {
        if !SAMPLES.iter().any(|s| s.name == name) || !self.samples.join(name).exists() {
            return Err(AppError::NotFound(format!("sample {name}")));
        }
        let dir = self.home.join("samples").join(name);
        {
            let mut current = self.lock();
            if current.as_ref().is_some_and(|o| o.dir == dir) {
                *current = None;
            }
        }
        if reset && dir.exists() {
            std::fs::remove_dir_all(&dir).context("couldn't remove the working copy").map_err(AppError::Other)?;
        }
        // A copy made when samples meant something else would be misread, so it's put aside.
        let marker = dir.join("sample-format");
        if dir.exists() && std::fs::read_to_string(&marker).ok().as_deref() != Some(FORMAT) {
            let aside = self.home.join("samples-superseded").join(format!("{name}-{}", now_ms()));
            std::fs::create_dir_all(self.home.join("samples-superseded")).map_err(|e| AppError::Other(e.into()))?;
            std::fs::rename(&dir, &aside).context("couldn't put the old working copy aside").map_err(AppError::Other)?;
        }
        if !dir.exists() {
            copy_dir(&self.samples.join(name), &dir).map_err(AppError::Other)?;
            std::fs::write(&marker, FORMAT).map_err(|e| AppError::Other(e.into()))?;
        }
        self.open_dir(&dir, Some(name.to_string()))
    }

    /// Make a new, empty workspace, and open it.
    pub fn create(self: &Arc<Self>, dir: &Path, name: &str, systems: Vec<String>) -> Result<(), AppError> {
        build(&systems).map_err(AppError::Refused)?;
        if dir.join("recollect.toml").exists() {
            return Err(AppError::Refused(format!("{} is already a workspace", dir.display())));
        }
        init(dir, name, systems).map_err(AppError::Other)?;
        self.open_dir(dir, None)
    }

    /// Open whatever was open last, if it's still there.
    pub fn reopen_last(self: &Arc<Self>) {
        let Ok(text) = std::fs::read_to_string(self.home.join("last")) else { return };
        let dir = PathBuf::from(text.trim());
        if !dir.join("recollect.toml").exists() {
            return;
        }
        let sample = dir.parent().filter(|p| *p == self.home.join("samples")).and_then(|_| dir.file_name()).map(|n| n.to_string_lossy().to_string());
        let opened = match &sample {
            Some(name) => self.open_sample(name, false),
            None => self.open_dir(&dir, None),
        };
        if let Err(e) = opened {
            tracing::warn!("couldn't reopen {}: {e}", dir.display());
        }
    }

    /// Commit these ops. Returns what any `new:` names in them became.
    pub fn apply(&self, ops: Vec<Value>, author: &str, why: &str) -> Result<Resolved, AppError> {
        let resolved = self.with(|o| {
            let resolved = resolve_ops(ops, o.log.graph()).map_err(AppError::Refused)?;
            formulas_work(o, &resolved.ops)?;
            o.log.commit(author, why, resolved.ops.clone()).map_err(refusal)?;
            Ok(resolved)
        })?;
        self.send(Event::GraphChanged { by: author.to_string() });
        Ok(resolved)
    }

    /// Go to any point in the log. The next change follows from there, starting a branch if it needs
    /// to, and nothing is lost. Any edit on disk that hasn't been noticed yet is recorded first, so it
    /// stays on the branch it was made on.
    pub fn checkout(&self, seq: u64, author: &str, why: &str) -> Result<(), AppError> {
        self.with(|o| {
            o.log.reload("disk").map_err(|e| AppError::Other(anyhow!("couldn't read the files: {e:?}")))?;
            o.log.checkout(seq, author, why).map_err(refusal)
        })?;
        self.send(Event::GraphChanged { by: author.to_string() });
        Ok(())
    }

    pub fn undo(&self, seq: u64, author: &str, why: &str) -> Result<u64, AppError> {
        let out = self.with(|o| o.log.undo(seq, author, why).map_err(refusal))?;
        self.send(Event::GraphChanged { by: author.to_string() });
        Ok(out)
    }

    /// Store a change-set for review. It has to apply cleanly as things stand.
    pub fn propose(&self, title: String, rationale: String, ops: Vec<Value>, author: &str) -> Result<Proposal, AppError> {
        let p = self.with(|o| {
            let resolved = resolve_ops(ops, o.log.graph()).map_err(AppError::Refused)?;
            formulas_work(o, &resolved.ops)?;
            let created = now_ms();
            let mut id = format!("{created}-{}", slug(&title));
            let mut n = 2;
            while o.proposals.iter().any(|p| p.id == id) {
                id = format!("{created}-{}-{n}", slug(&title));
                n += 1;
            }
            let p = Proposal { id, title, rationale, ops: resolved.ops, author: author.to_string(), created };
            save_proposal(&o.dir, &p).map_err(AppError::Other)?;
            o.proposals.push(p.clone());
            Ok(p)
        })?;
        self.send(Event::ProposalsChanged);
        Ok(p)
    }

    pub fn proposals(&self) -> Result<Vec<ProposalView>, AppError> {
        self.with(|o| {
            Ok(o.proposals
                .iter()
                .map(|p| {
                    let error = o.log.what_if(&p.ops).err().map(|v| v.iter().map(|v| format!("{}: {}", v.rule, v.detail)).collect::<Vec<_>>().join("; "));
                    ProposalView { proposal: p.clone(), error }
                })
                .collect())
        })
    }

    /// Commit a proposal, or some of its ops by index, and take it out of the inbox.
    pub fn accept(&self, id: &str, only: Option<&[usize]>) -> Result<u64, AppError> {
        let seq = self.with(|o| {
            let at = o.proposals.iter().position(|p| p.id == id).ok_or_else(|| AppError::NotFound(format!("proposal {id}")))?;
            let p = o.proposals[at].clone();
            let ops: Vec<Op> = match only {
                Some(idx) => idx.iter().filter_map(|i| p.ops.get(*i).cloned()).collect(),
                None => p.ops.clone(),
            };
            let why = if p.rationale.is_empty() { p.title.clone() } else { format!("{}: {}", p.title, p.rationale) };
            let seq = o.log.commit(&p.author, &why, ops).map_err(refusal)?;
            remove_proposal(&o.dir, id).map_err(AppError::Other)?;
            o.proposals.remove(at);
            Ok(seq)
        })?;
        self.send(Event::GraphChanged { by: "review".to_string() });
        self.send(Event::ProposalsChanged);
        Ok(seq)
    }

    pub fn reject(&self, id: &str) -> Result<(), AppError> {
        self.with(|o| {
            let before = o.proposals.len();
            o.proposals.retain(|p| p.id != id);
            if o.proposals.len() == before {
                return Err(AppError::NotFound(format!("proposal {id}")));
            }
            remove_proposal(&o.dir, id).map_err(AppError::Other)
        })?;
        self.send(Event::ProposalsChanged);
        Ok(())
    }

    /// Look at the files again, after something on disk changed. Files the log wrote itself are
    /// no difference, so this only records what someone else did.
    fn reload(&self, dir: &Path) {
        let mut changed = false;
        let mut proposals_changed = false;
        {
            let mut current = self.lock();
            let Some(o) = current.as_mut().filter(|o| o.dir == dir) else { return };
            match o.log.reload("disk") {
                Ok(seq) => changed = seq.is_some(),
                Err(e) => tracing::warn!("couldn't read the files: {e:?}"),
            }
            match load_proposals(dir) {
                Ok(p) if p != o.proposals => {
                    o.proposals = p;
                    proposals_changed = true;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("couldn't read the proposals: {e:#}"),
            }
        }
        if changed {
            self.send(Event::GraphChanged { by: "disk".to_string() });
        }
        if proposals_changed {
            self.send(Event::ProposalsChanged);
        }
    }
}

/// Make a workspace directory.
pub fn init(dir: &Path, name: &str, systems: Vec<String>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let config = Config { name: name.to_string(), about: None, systems, llm_direct_writes: false };
    std::fs::write(dir.join("recollect.toml"), toml::to_string(&config)?)?;
    std::fs::write(dir.join(".gitignore"), "log.lock\nlog.applied\n")?;
    Ok(())
}

fn load(dir: &Path, sample: Option<String>) -> Result<Open> {
    let path = dir.join("recollect.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("{} isn't a workspace: it has no recollect.toml", dir.display()))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
    if let Ok(entries) = std::fs::read_dir(dir.join("nodes"))
        && entries.flatten().any(|e| e.path().extension().is_some_and(|x| x == "md"))
    {
        bail!("{} was made by an older version of Recollect, which this can't read", dir.display());
    }
    let built = build(&config.systems).map_err(|e| anyhow!(e))?;
    let (log, problems) = Log::open(dir, built.registry.clone(), "disk").map_err(|e| match e {
        OpenError::Locked => anyhow!("{} is open in another process", dir.display()),
        OpenError::Corrupt { line, detail } => anyhow!("line {line} of the log can't be read: {detail}"),
        OpenError::Dir(e) => anyhow!("{e:?}"),
    })?;
    for p in problems {
        tracing::info!("{}: {p:?}", dir.display());
    }
    let proposals = load_proposals(dir)?;
    Ok(Open { dir: dir.to_path_buf(), config, built, log, proposals, sample, _watcher: None })
}

fn proposals_dir(dir: &Path) -> PathBuf {
    dir.join("proposals")
}

fn load_proposals(dir: &Path) -> Result<Vec<Proposal>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(proposals_dir(dir)) else { return Ok(vec![]) };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let p: Proposal = serde_json::from_str(&std::fs::read_to_string(&path)?).with_context(|| format!("reading {}", path.display()))?;
            out.insert(p.id.clone(), p);
        }
    }
    Ok(out.into_values().collect())
}

fn save_proposal(dir: &Path, p: &Proposal) -> Result<()> {
    std::fs::create_dir_all(proposals_dir(dir))?;
    std::fs::write(proposals_dir(dir).join(format!("{}.json", p.id)), serde_json::to_string_pretty(p)?)?;
    Ok(())
}

fn remove_proposal(dir: &Path, id: &str) -> Result<()> {
    let path = proposals_dir(dir).join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Notice when node or proposal files change on disk: hand edits, `git pull`, other tools.
fn watch(app: Arc<App>, dir: &Path) -> Result<notify::RecommendedWatcher> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let relevant = ev.paths.iter().any(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with('.') && (name.ends_with(".yaml") || name.ends_with(".json"))
            });
            if relevant {
                let _ = tx.send(());
            }
        }
    })?;
    std::fs::create_dir_all(proposals_dir(dir))?;
    watcher.watch(&dir.join("nodes"), RecursiveMode::NonRecursive)?;
    watcher.watch(&proposals_dir(dir), RecursiveMode::NonRecursive)?;
    let dir = dir.to_path_buf();
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            // Debounce bursts: editors, git checkouts, and this process's own commits.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
            let (app, dir) = (app.clone(), dir.clone());
            let _ = tokio::task::spawn_blocking(move || app.reload(&dir)).await;
        }
    });
    Ok(watcher)
}
