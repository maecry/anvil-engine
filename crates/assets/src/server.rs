//! The asset server: discovery, async loading, dependency resolution, hot reload.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

use crate::cache;
use crate::import::{self, Imported};
use crate::{Asset, AssetError, AssetId, AssetKind, META_VERSION, Meta, SubAsset};

/// Hot-reload debounce: a file must be quiet this long before it is reimported.
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Where an asset is in its life.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    /// Requested, importing or waiting for dependencies.
    Loading,
    /// Ready: [`AssetServer::get`] returns it.
    Loaded,
    /// Import failed or a dependency failed.
    Failed(AssetError),
}

/// What changed during [`AssetServer::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetEvent {
    /// Became `Loaded` for the first time (or after a failure).
    Loaded(AssetId),
    /// Was `Loaded` and has new contents (reimported, or a dependency was reloaded). Replace GPU
    /// copies; dependencies come before their dependents.
    Reloaded(AssetId),
    /// Became `Failed`; GPU copies should be dropped for fallbacks.
    Failed(AssetId),
}

/// Diagnostics counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AssetStats {
    /// Assets still importing or waiting for dependencies.
    pub pending: usize,
    /// Assets in the `Loaded` state.
    pub loaded: usize,
    /// Assets in the `Failed` state.
    pub failed: usize,
    /// Imports served from the artifact cache.
    pub cache_hits: u64,
    /// Imports that ran an importer.
    pub cache_misses: u64,
    /// Total worker time spent on imports, milliseconds.
    pub total_import_ms: f64,
    /// Duration of the most recent import, milliseconds.
    pub last_import_ms: f64,
}

/// One known asset, for listings.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetInfo {
    /// Identity.
    pub id: AssetId,
    /// Source path relative to the project, `/`-separated.
    pub path: String,
    /// Kind.
    pub kind: AssetKind,
    /// Source file's id when this is a sub-asset.
    pub parent: Option<AssetId>,
}

struct Entry {
    path: PathBuf,
    kind: AssetKind,
    importer: &'static str,
    parent: Option<AssetId>,
    /// Source files only.
    meta: Option<Meta>,
}

struct Slot {
    state: LoadState,
    asset: Option<Asset>,
    deps: Vec<AssetId>,
    soft: bool,
}

struct Done {
    root: AssetId,
    result: Result<Vec<Imported>, AssetError>,
    cache_hit: bool,
    ms: f64,
}

/// Owns the asset registry and loads assets on rayon workers; results are applied by
/// [`AssetServer::update`] on the caller's thread.
pub struct AssetServer {
    root: PathBuf,
    entries: HashMap<AssetId, Entry>,
    by_path: HashMap<String, AssetId>,
    slots: HashMap<AssetId, Slot>,
    in_flight: HashSet<AssetId>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    watcher: Option<notify::RecommendedWatcher>,
    changed: Arc<Mutex<HashMap<PathBuf, Instant>>>,
    stats: AssetStats,
}

impl AssetServer {
    /// Opens the project at `project_dir`: scans `assets/**` and creates missing `.meta` files.
    ///
    /// # Errors
    /// [`AssetError::Io`] when `assets/` cannot be read or a `.meta` cannot be written.
    pub fn new(project_dir: impl Into<PathBuf>) -> Result<Self, AssetError> {
        let (tx, rx) = channel();
        let mut s = Self {
            root: project_dir.into(),
            entries: HashMap::new(),
            by_path: HashMap::new(),
            slots: HashMap::new(),
            in_flight: HashSet::new(),
            tx,
            rx,
            watcher: None,
            changed: Arc::default(),
            stats: AssetStats::default(),
        };
        let assets = s.root.join("assets");
        if assets.is_dir() {
            let mut files = Vec::new();
            collect_files(&assets, &mut files)?;
            files.sort();
            for f in files {
                s.discover_file(&f)?;
            }
        }
        tracing::info!(assets = s.entries.len(), root = %s.root.display(), "assets discovered");
        Ok(s)
    }

    /// Project directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Id of the asset at `path` (relative to the project, e.g. `assets/a.png`).
    pub fn id_for_path(&self, path: &str) -> Option<AssetId> {
        self.by_path.get(path).copied()
    }

    /// All known assets (source files and their sub-assets), sorted by path then id.
    pub fn list(&self) -> Vec<AssetInfo> {
        let mut v: Vec<AssetInfo> = self
            .entries
            .iter()
            .map(|(id, e)| AssetInfo {
                id: *id,
                path: self.rel(&e.path),
                kind: e.kind,
                parent: e.parent,
            })
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
        v
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Registers `file` (creating its `.meta` when missing). Non-asset extensions are ignored.
    fn discover_file(&mut self, file: &Path) -> Result<(), AssetError> {
        let Some((kind, importer)) = AssetKind::of_path(file) else {
            return Ok(());
        };
        let meta_path = meta_path(file);
        let existing = match std::fs::read_to_string(&meta_path) {
            Ok(text) => match ron::from_str::<Meta>(&text) {
                Ok(m) if m.version <= META_VERSION => Some(m),
                Ok(m) => {
                    tracing::warn!(path = %meta_path.display(), version = m.version, "meta is from a newer engine, asset skipped");
                    return Ok(());
                }
                Err(e) => {
                    // shortcut: a damaged .meta skips its asset instead of regenerating (a new id would break references), add a repair tool later.
                    tracing::warn!(path = %meta_path.display(), error = %e, "unreadable meta, asset skipped");
                    return Ok(());
                }
            },
            Err(_) => None,
        };
        let mut meta = existing.unwrap_or_else(|| Meta {
            version: META_VERSION,
            id: AssetId::random(),
            importer: importer.to_owned(),
            sub_assets: Vec::new(),
        });
        let mut dirty = !meta_path.exists();
        if self.entries.contains_key(&meta.id) {
            // A copied file brought its meta along: give the later copy a new identity.
            tracing::warn!(path = %file.display(), id = %meta.id, "duplicate asset id, assigning a new one");
            meta = Meta {
                id: AssetId::random(),
                sub_assets: Vec::new(),
                ..meta
            };
            dirty = true;
        }
        if dirty {
            write_meta(&meta_path, &meta)?;
        }
        let id = meta.id;
        let rel = self.rel(file);
        for s in &meta.sub_assets {
            self.entries.insert(
                s.id,
                Entry {
                    path: file.to_owned(),
                    kind: s.kind,
                    importer,
                    parent: Some(id),
                    meta: None,
                },
            );
        }
        self.entries.insert(
            id,
            Entry {
                path: file.to_owned(),
                kind,
                importer,
                parent: None,
                meta: Some(meta),
            },
        );
        self.by_path.insert(rel, id);
        Ok(())
    }

    /// Registers source files that appeared under `assets/` since the last scan (creating their
    /// `.meta`), for tools that create or import files while running. Returns how many are new.
    ///
    /// # Errors
    /// [`AssetError::Io`] when `assets/` cannot be read or a `.meta` cannot be written.
    pub fn rescan(&mut self) -> Result<usize, AssetError> {
        let mut files = Vec::new();
        collect_files(&self.root.join("assets"), &mut files)?;
        files.sort();
        let before = self.by_path.len();
        for f in files {
            if !self.by_path.contains_key(&self.rel(&f)) {
                self.discover_file(&f)?;
            }
        }
        Ok(self.by_path.len() - before)
    }

    /// Starts loading the asset at `path` (relative to the project).
    ///
    /// # Errors
    /// [`AssetError::Io`] for an unknown path.
    pub fn load_path(&mut self, path: &str) -> Result<AssetId, AssetError> {
        let id = self
            .id_for_path(path)
            .ok_or_else(|| AssetError::Io(format!("unknown asset path {path}")))?;
        self.load(id);
        Ok(id)
    }

    /// Starts loading `id` (and its dependencies) if not already started; returns the state.
    pub fn load(&mut self, id: AssetId) -> LoadState {
        if let Some(s) = self.slots.get(&id) {
            return s.state.clone();
        }
        let Some(root) = self.entries.get(&id).map(|e| e.parent.unwrap_or(id)) else {
            let e = AssetError::Io(format!("unknown asset {id}"));
            self.slots.insert(id, failed_slot(e.clone()));
            return LoadState::Failed(e);
        };
        self.slots.insert(
            id,
            Slot {
                state: LoadState::Loading,
                asset: None,
                deps: Vec::new(),
                soft: true,
            },
        );
        self.spawn_import(root);
        LoadState::Loading
    }

    fn spawn_import(&mut self, root: AssetId) {
        if !self.in_flight.insert(root) {
            return;
        }
        let Some(e) = self.entries.get(&root) else {
            return;
        };
        let (path, importer, project) = (e.path.clone(), e.importer, self.root.clone());
        let tx = self.tx.clone();
        rayon::spawn(move || {
            let started = Instant::now();
            // Importers should not panic on bad input; if one does, it is a failed asset, not a crash.
            let (result, cache_hit) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                load_or_import(&project, &path, importer, root)
            }))
            .unwrap_or_else(|_| (Err(AssetError::Corrupt("importer panicked".into())), false));
            let _ = tx.send(Done {
                root,
                result,
                cache_hit,
                ms: started.elapsed().as_secs_f64() * 1000.0,
            });
        });
    }

    /// Current state of `id`, `None` if never requested.
    pub fn state(&self, id: AssetId) -> Option<LoadState> {
        self.slots.get(&id).map(|s| s.state.clone())
    }

    /// The loaded asset (`None` unless `Loaded`).
    pub fn get(&self, id: AssetId) -> Option<Asset> {
        self.slots
            .get(&id)
            .filter(|s| s.state == LoadState::Loaded)
            .and_then(|s| s.asset.clone())
    }

    /// Diagnostics counters.
    pub fn stats(&self) -> AssetStats {
        let mut s = self.stats;
        for slot in self.slots.values() {
            match slot.state {
                LoadState::Loaded => s.loaded += 1,
                LoadState::Failed(_) => s.failed += 1,
                LoadState::Loading => s.pending += 1,
            }
        }
        s
    }

    /// True while anything is importing or waiting for dependencies.
    pub fn is_busy(&self) -> bool {
        !self.in_flight.is_empty() || self.slots.values().any(|s| s.state == LoadState::Loading)
    }

    /// Starts watching `assets/` for changes (200 ms debounce); changed sources that were
    /// loaded are reimported and reported as [`AssetEvent::Reloaded`].
    ///
    /// # Errors
    /// [`AssetError::Io`] when the watcher cannot be created.
    pub fn enable_hot_reload(&mut self) -> Result<(), AssetError> {
        let changed = self.changed.clone();
        let mut w = notify::recommended_watcher(move |ev: notify::Result<notify::Event>| {
            // Reading a file (our own import) raises access events; only real changes count.
            let Ok(ev) = ev else { return };
            if ev.kind.is_access() {
                return;
            }
            if let Ok(mut map) = changed.lock() {
                for p in ev.paths {
                    map.insert(p, Instant::now());
                }
            }
        })
        .map_err(|e| AssetError::Io(e.to_string()))?;
        w.watch(&self.root.join("assets"), RecursiveMode::Recursive)
            .map_err(|e| AssetError::Io(e.to_string()))?;
        self.watcher = Some(w);
        tracing::info!("asset hot reload enabled");
        Ok(())
    }

    fn poll_changes(&mut self) {
        let due: Vec<PathBuf> = match self.changed.lock() {
            Ok(mut map) => {
                let due: Vec<PathBuf> = map
                    .iter()
                    .filter(|(_, t)| t.elapsed() >= DEBOUNCE)
                    .map(|(p, _)| p.clone())
                    .collect();
                for p in &due {
                    map.remove(p);
                }
                due
            }
            Err(_) => return,
        };
        for p in due {
            if !p.is_file() || AssetKind::of_path(&p).is_none() {
                continue;
            }
            let rel = self.rel(&p);
            let id = match self.by_path.get(&rel) {
                Some(id) => *id,
                None => {
                    if let Err(e) = self.discover_file(&p) {
                        tracing::warn!(error = %e, "discovery of changed file failed");
                    }
                    continue;
                }
            };
            // Only assets somebody loaded are reimported; others load fresh when requested.
            if self.slots.contains_key(&id) {
                tracing::info!(path = %rel, "source changed, reimporting");
                self.spawn_import(id);
            }
        }
    }

    /// Applies finished imports, resolves dependencies and returns what changed. Call once per
    /// frame on the main thread.
    pub fn update(&mut self) -> Vec<AssetEvent> {
        self.poll_changes();
        let mut events = Vec::new();
        let mut dirty = false;
        while let Ok(done) = self.rx.try_recv() {
            self.apply(done, &mut events);
            dirty = true;
        }
        if dirty || self.slots.values().any(|s| s.state == LoadState::Loading) {
            self.resolve(&mut events);
        }
        // Dependents of a reloaded asset are reloaded too (dependencies first).
        let reloaded: Vec<AssetId> = events
            .iter()
            .filter(|e| matches!(e, AssetEvent::Reloaded(_)))
            .map(|e| ev_id(*e))
            .collect();
        let mut seen: HashSet<AssetId> = events.iter().map(|e| ev_id(*e)).collect();
        let mut queue = reloaded;
        while let Some(id) = queue.pop() {
            let mut users: Vec<AssetId> = self
                .slots
                .iter()
                .filter(|(_, s)| !s.soft && s.deps.contains(&id) && s.state == LoadState::Loaded)
                .map(|(i, _)| *i)
                .collect();
            users.sort();
            for u in users {
                if seen.insert(u) {
                    events.push(AssetEvent::Reloaded(u));
                    queue.push(u);
                }
            }
        }
        events
    }

    fn apply(&mut self, done: Done, events: &mut Vec<AssetEvent>) {
        self.in_flight.remove(&done.root);
        self.stats.total_import_ms += done.ms;
        self.stats.last_import_ms = done.ms;
        let subs: Vec<AssetId> = self
            .entries
            .iter()
            .filter(|(_, e)| e.parent == Some(done.root))
            .map(|(i, _)| *i)
            .collect();
        match done.result {
            Err(e) => {
                tracing::warn!(id = %done.root, error = %e, "asset load failed");
                for id in std::iter::once(done.root).chain(subs) {
                    if self.slots.contains_key(&id) {
                        self.fail(id, e.clone(), events);
                    }
                }
            }
            Ok(bundle) => {
                if done.cache_hit {
                    self.stats.cache_hits += 1;
                } else {
                    self.stats.cache_misses += 1;
                }
                tracing::info!(id = %done.root, ms = done.ms, cache_hit = done.cache_hit, assets = bundle.len(), "asset imported");
                self.register_subs(done.root, &bundle);
                let present: HashSet<AssetId> = bundle.iter().map(|b| b.id).collect();
                for gone in subs.iter().filter(|s| !present.contains(s)) {
                    if self.slots.contains_key(gone) {
                        let e = AssetError::Io("sub-asset no longer exists in its source".into());
                        self.fail(*gone, e, events);
                    }
                }
                let mut to_load = Vec::new();
                for item in bundle {
                    let (deps, hard) = item.asset.deps();
                    let was_loaded = self
                        .slots
                        .get(&item.id)
                        .is_some_and(|s| s.state == LoadState::Loaded);
                    // A reloaded asset keeps serving its old contents until the new one is ready.
                    self.slots.insert(
                        item.id,
                        Slot {
                            state: if was_loaded {
                                LoadState::Loaded
                            } else {
                                LoadState::Loading
                            },
                            asset: Some(item.asset),
                            deps: deps.clone(),
                            soft: !hard,
                        },
                    );
                    if was_loaded {
                        events.push(AssetEvent::Reloaded(item.id));
                    }
                    to_load.extend(deps);
                }
                for d in to_load {
                    self.load(d);
                }
            }
        }
    }

    fn fail(&mut self, id: AssetId, e: AssetError, events: &mut Vec<AssetEvent>) {
        let slot = self
            .slots
            .entry(id)
            .or_insert_with(|| failed_slot(e.clone()));
        slot.state = LoadState::Failed(e);
        slot.asset = None;
        events.push(AssetEvent::Failed(id));
    }

    /// Updates sub-asset registrations and rewrites the root's `.meta` when they changed.
    fn register_subs(&mut self, root: AssetId, bundle: &[Imported]) {
        let subs: Vec<SubAsset> = bundle
            .iter()
            .filter(|b| b.id != root)
            .map(|b| SubAsset {
                id: b.id,
                kind: b.asset.kind(),
                name: b.name.clone(),
            })
            .collect();
        let Some(entry) = self.entries.get(&root) else {
            return;
        };
        let (path, importer) = (entry.path.clone(), entry.importer);
        for s in &subs {
            self.entries.entry(s.id).or_insert(Entry {
                path: path.clone(),
                kind: s.kind,
                importer,
                parent: Some(root),
                meta: None,
            });
        }
        if let Some(meta) = self.entries.get_mut(&root).and_then(|e| e.meta.as_mut())
            && meta.sub_assets != subs
        {
            meta.sub_assets = subs;
            if let Err(e) = write_meta(&meta_path(&path), meta) {
                tracing::warn!(error = %e, "could not update meta");
            }
        }
    }

    /// Moves `Loading` assets whose dependencies are settled to `Loaded` / `Failed`, and fails
    /// loaded assets whose dependency failed later (to a fixpoint).
    fn resolve(&mut self, events: &mut Vec<AssetEvent>) {
        loop {
            let mut changed = false;
            let mut ids: Vec<AssetId> = self.slots.keys().copied().collect();
            ids.sort();
            for id in ids {
                let Some(slot) = self.slots.get(&id) else {
                    continue;
                };
                if slot.asset.is_none() || matches!(slot.state, LoadState::Failed(_)) {
                    continue;
                }
                let mut all_loaded = true;
                let mut failure = None;
                if !slot.soft {
                    for d in &slot.deps {
                        match self.slots.get(d).map(|s| &s.state) {
                            Some(LoadState::Failed(e)) => {
                                failure = Some(AssetError::Dependency {
                                    id: *d,
                                    reason: e.to_string(),
                                });
                                break;
                            }
                            Some(LoadState::Loaded) => {}
                            _ => all_loaded = false,
                        }
                    }
                }
                let state = slot.state.clone();
                if let Some(e) = failure {
                    self.fail(id, e, events);
                    changed = true;
                } else if all_loaded && state == LoadState::Loading {
                    if let Some(s) = self.slots.get_mut(&id) {
                        s.state = LoadState::Loaded;
                    }
                    events.push(AssetEvent::Loaded(id));
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }
}

fn ev_id(e: AssetEvent) -> AssetId {
    match e {
        AssetEvent::Loaded(i) | AssetEvent::Reloaded(i) | AssetEvent::Failed(i) => i,
    }
}

fn failed_slot(e: AssetError) -> Slot {
    Slot {
        state: LoadState::Failed(e),
        asset: None,
        deps: Vec::new(),
        soft: true,
    }
}

fn meta_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".meta");
    PathBuf::from(s)
}

fn write_meta(path: &Path, meta: &Meta) -> Result<(), AssetError> {
    let text = ron::ser::to_string_pretty(meta, ron::ser::PrettyConfig::default())
        .map_err(|e| AssetError::Format(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| AssetError::Io(format!("{}: {e}", path.display())))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AssetError> {
    let rd =
        std::fs::read_dir(dir).map_err(|e| AssetError::Io(format!("{}: {e}", dir.display())))?;
    for entry in rd.flatten() {
        let p = entry.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if p.is_dir() {
            collect_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// Worker body: artifact cache lookup, else import and store.
fn load_or_import(
    project: &Path,
    path: &Path,
    importer: &str,
    id: AssetId,
) -> (Result<Vec<Imported>, AssetError>, bool) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return (
                Err(AssetError::Io(format!("{}: {e}", path.display()))),
                false,
            );
        }
    };
    let hash = cache::hash(&bytes);
    let cache_file = project.join(".anvil/imported").join(format!("{id}.bin"));
    if let Ok(c) = std::fs::read(&cache_file)
        && let Ok(bundle) = cache::decode(&c, hash)
        // A damaged count or id could drop the root asset, which would leave it `Loading` forever.
        && bundle.first().is_some_and(|b| b.id == id)
    {
        return (Ok(bundle), true);
    }
    let bundle = match import::import(path, importer, id, &bytes) {
        Ok(b) => b,
        Err(e) => return (Err(e), false),
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(cache_file.parent().unwrap_or(project))?;
        std::fs::write(&cache_file, cache::encode(&bundle, hash))
    };
    if let Err(e) = write() {
        tracing::warn!(error = %e, "could not write the import cache");
    }
    (Ok(bundle), false)
}
