//! The open workspace, on disk. Every change goes through here, so the web UI, Claude and the file
//! watcher all see the same thing. What a workspace is and does is `recollect_core`: this reads its
//! files, writes what each change says to, and notices when someone else changes them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use notify::{RecursiveMode, Watcher};
use recollect_core::{Effect, Files, SAMPLE_FORMAT, SAMPLE_MARKER, Workspace, Writes, dispatch, new_files, sample_files};
use recollect_systems::samples::{SAMPLES, Sample};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    GraphChanged { by: String },
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

impl From<recollect_core::Error> for AppError {
    fn from(e: recollect_core::Error) -> Self {
        match e {
            recollect_core::Error::NotFound(s) => AppError::NotFound(s),
            recollect_core::Error::Refused(s) => AppError::Refused(s),
            recollect_core::Error::Other(s) => AppError::Other(anyhow!(s)),
        }
    }
}

/// Held while a workspace is open, so two processes can't both write one.
pub const LOCK: &str = ".recollect.lock";

pub struct Open {
    pub dir: PathBuf,
    pub ws: Workspace,
    _lock: File,
    _watcher: Option<notify::RecommendedWatcher>,
}

pub struct App {
    /// Where this keeps its own things: working copies of samples, and what was open last.
    pub home: PathBuf,
    current: Mutex<Option<Open>>,
    pub events: broadcast::Sender<Event>,
}

/// The files that make up a workspace, as `recollect_core` reads them. Anything else in the directory
/// is the user's own.
fn is_workspace_file(path: &str) -> bool {
    path == "recollect.toml" || path == SAMPLE_MARKER || recollect_graph::is_node_path(path)
}

/// Every workspace file in a directory.
pub fn read_files(dir: &Path) -> Result<Files> {
    let mut out = Files::new();
    for (sub, prefix) in [(dir.to_path_buf(), ""), (dir.join("nodes"), "nodes/")] {
        let Ok(entries) = std::fs::read_dir(&sub) else { continue };
        for entry in entries {
            let entry = entry?;
            let path = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if is_workspace_file(&path) && entry.file_type()?.is_file() {
                out.insert(path, std::fs::read_to_string(entry.path()).with_context(|| format!("reading {}", entry.path().display()))?);
            }
        }
    }
    Ok(out)
}

/// Write what a change says to, each file replaced whole so a reader never sees half of one.
pub fn write_files(dir: &Path, writes: &Writes) -> Result<()> {
    for (path, text) in &writes.write {
        let to = dir.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = to.with_file_name(format!(".{}.tmp", to.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
        let mut file = File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &to).with_context(|| format!("writing {}", to.display()))?;
    }
    for path in &writes.delete {
        match std::fs::remove_file(dir.join(path)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// A lock belongs to the open file, so a process being spawned elsewhere in this program holds a
/// copy for an instant until it execs, even for a workspace that was just closed. Only someone who
/// keeps holding it is really using it.
fn take_lock(dir: &Path) -> Result<File> {
    let lock = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK))?;
    let give_up = Instant::now() + Duration::from_millis(500);
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() >= give_up => return Err(anyhow!("{} is open in another process", dir.display())),
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

impl Open {
    /// Read the files again, and bring in whatever someone else changed. Returns what changed.
    fn sync(&mut self) -> Result<recollect_core::Changed> {
        let disk = read_files(&self.dir)?;
        let paths: BTreeSet<&String> = disk.keys().chain(self.ws.files().keys()).collect();
        let changes: BTreeMap<String, Option<String>> = paths.into_iter().filter(|p| disk.get(*p) != self.ws.files().get(*p)).map(|p| (p.clone(), disk.get(p).cloned())).collect();
        if changes.is_empty() {
            return Ok(Default::default());
        }
        let changed = self.ws.files_changed(changes);
        for p in &changed.problems {
            tracing::warn!("{}: {p}", self.dir.display());
        }
        self.flush()?;
        Ok(changed)
    }

    fn flush(&mut self) -> Result<()> {
        let writes = self.ws.take_writes();
        if !writes.is_empty() {
            write_files(&self.dir, &writes).context("couldn't write the workspace's files")?;
        }
        Ok(())
    }
}

impl App {
    pub fn new(home: PathBuf) -> Arc<App> {
        let (events, _) = broadcast::channel(64);
        Arc::new(App { home, current: Mutex::new(None), events })
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

    pub fn sample_list(&self) -> &'static [Sample] {
        SAMPLES
    }

    /// Ask the open workspace something, or change it, as `author`. A change first reads the files
    /// again, so it's made on top of whatever someone else has done to them.
    pub fn call(&self, method: &str, args: Value, author: &str) -> Result<Value, AppError> {
        let (value, effect, changed) = self.with(|o| {
            let reads = matches!(method, "view" | "plan" | "try" | "sweep" | "rows" | "history");
            let changed = if reads { Default::default() } else { o.sync()? };
            let (value, effect) = dispatch(&mut o.ws, method, args, author)?;
            o.flush()?;
            Ok((value, effect, changed))
        })?;
        self.announce(&changed, "disk");
        let Effect { graph } = effect;
        if graph {
            self.send(Event::GraphChanged { by: author.to_string() });
        }
        Ok(value)
    }

    fn announce(&self, changed: &recollect_core::Changed, by: &str) {
        if changed.graph {
            self.send(Event::GraphChanged { by: by.to_string() });
        }
        if changed.workspace {
            self.send(Event::WorkspaceChanged);
        }
    }

    /// Open a workspace directory, in place of whatever's open.
    pub fn open_dir(self: &Arc<Self>, dir: &Path, sample: Option<String>) -> Result<(), AppError> {
        let mut current = self.lock();
        // The lock has to be free, in case this is the same directory.
        *current = None;
        let dir = std::path::absolute(dir).map_err(|e| AppError::Other(e.into()))?;
        if !dir.join("recollect.toml").exists() {
            return Err(AppError::Other(anyhow!("{} isn't a workspace: it has no recollect.toml", dir.display())));
        }
        let lock = take_lock(&dir).map_err(AppError::Other)?;
        let files = read_files(&dir).map_err(AppError::Other)?;
        let (ws, problems) = Workspace::load(&dir.display().to_string(), &dir.display().to_string(), sample, files)?;
        for p in problems {
            tracing::info!("{}: {p}", dir.display());
        }
        let mut opened = Open { dir: dir.clone(), ws, _lock: lock, _watcher: None };
        opened.flush().map_err(AppError::Other)?;
        opened._watcher = Some(watch(self.clone(), &dir).map_err(AppError::Other)?);
        *current = Some(opened);
        drop(current);
        let _ = std::fs::create_dir_all(&self.home);
        let _ = std::fs::write(self.home.join("last"), dir.display().to_string());
        self.send(Event::WorkspaceChanged);
        Ok(())
    }

    /// Open a sample's working copy, making it if it isn't there or `reset` says so.
    pub fn open_sample(self: &Arc<Self>, name: &str, reset: bool) -> Result<(), AppError> {
        let fresh = sample_files(name).ok_or_else(|| AppError::NotFound(format!("sample {name}")))?;
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
        if dir.exists() && std::fs::read_to_string(dir.join(SAMPLE_MARKER)).ok().as_deref() != Some(SAMPLE_FORMAT) {
            let aside = self.home.join("samples-superseded").join(format!("{name}-{}", recollect_graph::now_ms()));
            std::fs::create_dir_all(self.home.join("samples-superseded")).map_err(|e| AppError::Other(e.into()))?;
            std::fs::rename(&dir, &aside).context("couldn't put the old working copy aside").map_err(AppError::Other)?;
        }
        if !dir.exists() {
            write_files(&dir, &Writes { write: fresh, delete: vec![] }).map_err(AppError::Other)?;
        }
        self.open_dir(&dir, Some(name.to_string()))
    }

    /// Make a new, empty workspace, and open it.
    pub fn create(self: &Arc<Self>, dir: &Path, name: &str, systems: Vec<String>) -> Result<(), AppError> {
        if dir.join("recollect.toml").exists() {
            return Err(AppError::Refused(format!("{} is already a workspace", dir.display())));
        }
        init(dir, name, systems)?;
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

    /// Look at the files again, after something on disk changed. Files this wrote itself are no
    /// difference, so this only records what someone else did.
    fn reload(&self, dir: &Path) {
        let changed = {
            let mut current = self.lock();
            let Some(o) = current.as_mut().filter(|o| o.dir == dir) else { return };
            match o.sync() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("couldn't read the files: {e:#}");
                    return;
                }
            }
        };
        self.announce(&changed, "disk");
    }
}

/// Make a workspace directory.
pub fn init(dir: &Path, name: &str, systems: Vec<String>) -> Result<(), AppError> {
    let mut files = new_files(name, systems)?;
    files.insert(".gitignore".to_string(), format!("{LOCK}\n"));
    write_files(dir, &Writes { write: files, delete: vec![] }).map_err(AppError::Other)
}

/// Notice when files change on disk: hand edits, `git pull`, other tools.
fn watch(app: Arc<App>, dir: &Path) -> Result<notify::RecommendedWatcher> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let relevant = ev.paths.iter().any(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with('.') && (name.ends_with(".yaml") || name.ends_with(".toml"))
            });
            if relevant {
                let _ = tx.send(());
            }
        }
    })?;
    std::fs::create_dir_all(dir.join("nodes"))?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    watcher.watch(&dir.join("nodes"), RecursiveMode::NonRecursive)?;
    let dir = dir.to_path_buf();
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            // Debounce bursts: editors, git checkouts, and this process's own writes.
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
            let (app, dir) = (app.clone(), dir.clone());
            let _ = tokio::task::spawn_blocking(move || app.reload(&dir)).await;
        }
    });
    Ok(watcher)
}
