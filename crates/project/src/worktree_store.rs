use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    pin::pin,
    sync::{Arc, atomic::AtomicUsize},
};

use anyhow::{Context as _, Result, anyhow};
use collections::{HashMap, HashSet};
use fs::Fs;
use futures::{
    FutureExt, SinkExt,
    future::{BoxFuture, Shared},
};
use gpui::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, Task, WeakEntity,
};
use postage::oneshot;
use smol::{
    channel::{Receiver, Sender},
    stream::StreamExt,
};
use util::{ResultExt, paths::SanitizedPath};
use worktree::{
    Entry, ProjectEntryId, UpdatedEntriesSet, UpdatedGitRepositoriesSet, Worktree, WorktreeId,
    WorktreeSettings,
};

use crate::{ProjectPath, search::SearchQuery};

struct MatchingEntry {
    worktree_path: Arc<Path>,
    path: ProjectPath,
    respond: oneshot::Sender<ProjectPath>,
}

pub struct WorktreeStore {
    next_entry_id: Arc<AtomicUsize>,
    retain_worktrees: bool,
    worktrees: Vec<WorktreeHandle>,
    worktrees_reordered: bool,
    #[allow(clippy::type_complexity)]
    loading_worktrees:
        HashMap<SanitizedPath, Shared<Task<Result<Entity<Worktree>, Arc<anyhow::Error>>>>>,
    fs: Arc<dyn Fs>,
}

#[derive(Debug)]
pub enum WorktreeStoreEvent {
    WorktreeAdded(Entity<Worktree>),
    WorktreeRemoved(EntityId, WorktreeId),
    WorktreeReleased(EntityId, WorktreeId),
    WorktreeOrderChanged,
    WorktreeUpdateSent(Entity<Worktree>),
    WorktreeUpdatedEntries(WorktreeId, UpdatedEntriesSet),
    WorktreeUpdatedGitRepositories(WorktreeId, UpdatedGitRepositoriesSet),
    WorktreeDeletedEntry(WorktreeId, ProjectEntryId),
}

impl EventEmitter<WorktreeStoreEvent> for WorktreeStore {}

impl WorktreeStore {
    pub fn local(retain_worktrees: bool, fs: Arc<dyn Fs>) -> Self {
        Self {
            next_entry_id: Default::default(),
            loading_worktrees: Default::default(),
            worktrees: Vec::new(),
            worktrees_reordered: false,
            retain_worktrees,
            fs,
        }
    }

    /// Iterates through all worktrees, including ones that don't appear in the project panel
    pub fn worktrees(&self) -> impl '_ + DoubleEndedIterator<Item = Entity<Worktree>> {
        self.worktrees
            .iter()
            .filter_map(move |worktree| worktree.upgrade())
    }

    /// Iterates through all user-visible worktrees, the ones that appear in the project panel.
    pub fn visible_worktrees<'a>(
        &'a self,
        cx: &'a App,
    ) -> impl 'a + DoubleEndedIterator<Item = Entity<Worktree>> {
        self.worktrees()
            .filter(|worktree| worktree.read(cx).is_visible())
    }

    pub fn worktree_for_id(&self, id: WorktreeId, cx: &App) -> Option<Entity<Worktree>> {
        self.worktrees()
            .find(|worktree| worktree.read(cx).id() == id)
    }

    pub fn worktree_for_entry(
        &self,
        entry_id: ProjectEntryId,
        cx: &App,
    ) -> Option<Entity<Worktree>> {
        self.worktrees()
            .find(|worktree| worktree.read(cx).contains_entry(entry_id))
    }

    pub fn find_worktree(
        &self,
        abs_path: impl Into<SanitizedPath>,
        cx: &App,
    ) -> Option<(Entity<Worktree>, PathBuf)> {
        let abs_path: SanitizedPath = abs_path.into();
        for tree in self.worktrees() {
            if let Ok(relative_path) = abs_path.as_path().strip_prefix(tree.read(cx).abs_path()) {
                return Some((tree.clone(), relative_path.into()));
            }
        }
        None
    }

    pub fn absolutize(&self, project_path: &ProjectPath, cx: &App) -> Option<PathBuf> {
        let worktree = self.worktree_for_id(project_path.worktree_id, cx)?;
        worktree.read(cx).absolutize(&project_path.path).ok()
    }

    pub fn find_or_create_worktree(
        &mut self,
        abs_path: impl AsRef<Path>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Entity<Worktree>, PathBuf)>> {
        let abs_path = abs_path.as_ref();
        if let Some((tree, relative_path)) = self.find_worktree(abs_path, cx) {
            Task::ready(Ok((tree, relative_path)))
        } else {
            let worktree = self.create_worktree(abs_path, visible, cx);
            cx.background_spawn(async move { Ok((worktree.await?, PathBuf::new())) })
        }
    }

    pub fn entry_for_id<'a>(&'a self, entry_id: ProjectEntryId, cx: &'a App) -> Option<&'a Entry> {
        self.worktrees()
            .find_map(|worktree| worktree.read(cx).entry_for_id(entry_id))
    }

    pub fn worktree_and_entry_for_id<'a>(
        &'a self,
        entry_id: ProjectEntryId,
        cx: &'a App,
    ) -> Option<(Entity<Worktree>, &'a Entry)> {
        self.worktrees().find_map(|worktree| {
            worktree
                .read(cx)
                .entry_for_id(entry_id)
                .map(|e| (worktree.clone(), e))
        })
    }

    pub fn entry_for_path(&self, path: &ProjectPath, cx: &App) -> Option<Entry> {
        self.worktree_for_id(path.worktree_id, cx)?
            .read(cx)
            .entry_for_path(&path.path)
            .cloned()
    }

    pub fn create_worktree(
        &mut self,
        abs_path: impl Into<SanitizedPath>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Worktree>>> {
        let abs_path: SanitizedPath = abs_path.into();
        if !self.loading_worktrees.contains_key(&abs_path) {
            let task = self.create_local_worktree(self.fs.clone(), abs_path.clone(), visible, cx);

            self.loading_worktrees
                .insert(abs_path.clone(), task.shared());
        }
        let task = self.loading_worktrees.get(&abs_path).unwrap().clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, _| this.loading_worktrees.remove(&abs_path))
                .ok();
            match result {
                Ok(worktree) => Ok(worktree),
                Err(err) => Err(anyhow!(err.to_string())),
            }
        })
    }

    fn create_local_worktree(
        &mut self,
        fs: Arc<dyn Fs>,
        abs_path: impl Into<SanitizedPath>,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Worktree>, Arc<anyhow::Error>>> {
        let next_entry_id = self.next_entry_id.clone();
        let path: SanitizedPath = abs_path.into();

        cx.spawn(async move |this, cx| {
            let worktree = Worktree::local(path.clone(), visible, fs, next_entry_id, cx).await;

            let worktree = worktree?;

            this.update(cx, |this, cx| this.add(&worktree, cx))?;

            if visible {
                cx.update(|cx| {
                    cx.add_recent_document(path.as_path());
                })
                .log_err();
            }

            Ok(worktree)
        })
    }

    pub fn add(&mut self, worktree: &Entity<Worktree>, cx: &mut Context<Self>) {
        let worktree_id = worktree.read(cx).id();
        debug_assert!(self.worktrees().all(|w| w.read(cx).id() != worktree_id));

        let push_strong_handle = self.retain_worktrees || worktree.read(cx).is_visible();
        let handle = if push_strong_handle {
            WorktreeHandle::Strong(worktree.clone())
        } else {
            WorktreeHandle::Weak(worktree.downgrade())
        };
        if self.worktrees_reordered {
            self.worktrees.push(handle);
        } else {
            let i = match self
                .worktrees
                .binary_search_by_key(&Some(worktree.read(cx).abs_path()), |other| {
                    other.upgrade().map(|worktree| worktree.read(cx).abs_path())
                }) {
                Ok(i) | Err(i) => i,
            };
            self.worktrees.insert(i, handle);
        }

        cx.emit(WorktreeStoreEvent::WorktreeAdded(worktree.clone()));

        let handle_id = worktree.entity_id();
        cx.subscribe(worktree, |_, worktree, event, cx| {
            let worktree_id = worktree.read(cx).id();
            match event {
                worktree::Event::UpdatedEntries(changes) => {
                    cx.emit(WorktreeStoreEvent::WorktreeUpdatedEntries(
                        worktree_id,
                        changes.clone(),
                    ));
                }
                worktree::Event::UpdatedGitRepositories(set) => {
                    cx.emit(WorktreeStoreEvent::WorktreeUpdatedGitRepositories(
                        worktree_id,
                        set.clone(),
                    ));
                }
                worktree::Event::DeletedEntry(id) => {
                    cx.emit(WorktreeStoreEvent::WorktreeDeletedEntry(worktree_id, *id))
                }
            }
        })
        .detach();
        cx.observe_release(worktree, move |_, worktree, cx| {
            cx.emit(WorktreeStoreEvent::WorktreeReleased(
                handle_id,
                worktree.id(),
            ));
            cx.emit(WorktreeStoreEvent::WorktreeRemoved(
                handle_id,
                worktree.id(),
            ));
        })
        .detach();
    }

    pub fn remove_worktree(&mut self, id_to_remove: WorktreeId, cx: &mut Context<Self>) {
        self.worktrees.retain(|worktree| {
            if let Some(worktree) = worktree.upgrade() {
                if worktree.read(cx).id() == id_to_remove {
                    cx.emit(WorktreeStoreEvent::WorktreeRemoved(
                        worktree.entity_id(),
                        id_to_remove,
                    ));
                    false
                } else {
                    true
                }
            } else {
                false
            }
        });
    }

    pub fn set_worktrees_reordered(&mut self, worktrees_reordered: bool) {
        self.worktrees_reordered = worktrees_reordered;
    }

    pub fn move_worktree(
        &mut self,
        source: WorktreeId,
        destination: WorktreeId,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if source == destination {
            return Ok(());
        }

        let mut source_index = None;
        let mut destination_index = None;
        for (i, worktree) in self.worktrees.iter().enumerate() {
            if let Some(worktree) = worktree.upgrade() {
                let worktree_id = worktree.read(cx).id();
                if worktree_id == source {
                    source_index = Some(i);
                    if destination_index.is_some() {
                        break;
                    }
                } else if worktree_id == destination {
                    destination_index = Some(i);
                    if source_index.is_some() {
                        break;
                    }
                }
            }
        }

        let source_index =
            source_index.with_context(|| format!("Missing worktree for id {source}"))?;
        let destination_index =
            destination_index.with_context(|| format!("Missing worktree for id {destination}"))?;

        if source_index == destination_index {
            return Ok(());
        }

        let worktree_to_move = self.worktrees.remove(source_index);
        self.worktrees.insert(destination_index, worktree_to_move);
        self.worktrees_reordered = true;
        cx.emit(WorktreeStoreEvent::WorktreeOrderChanged);
        cx.notify();
        Ok(())
    }

    /// search over all worktrees and return buffers that *might* match the search.
    pub fn find_search_candidates(
        &self,
        query: SearchQuery,
        limit: usize,
        open_entries: HashSet<ProjectEntryId>,
        fs: Arc<dyn Fs>,
        cx: &Context<Self>,
    ) -> Receiver<ProjectPath> {
        let snapshots = self
            .visible_worktrees(cx)
            .filter_map(|tree| {
                let tree = tree.read(cx);
                Some((tree.snapshot(), tree.as_local()?.settings()))
            })
            .collect::<Vec<_>>();

        let executor = cx.background_executor().clone();

        // We want to return entries in the order they are in the worktrees, so we have one
        // thread that iterates over the worktrees (and ignored directories) as necessary,
        // and pushes a oneshot::Receiver to the output channel and a oneshot::Sender to the filter
        // channel.
        // We spawn a number of workers that take items from the filter channel and check the query
        // against the version of the file on disk.
        let (filter_tx, filter_rx) = smol::channel::bounded(64);
        let (output_tx, output_rx) = smol::channel::bounded(64);
        let (matching_paths_tx, matching_paths_rx) = smol::channel::unbounded();

        let input = cx.background_spawn({
            let fs = fs.clone();
            let query = query.clone();
            async move {
                Self::find_candidate_paths(
                    fs,
                    snapshots,
                    open_entries,
                    query,
                    filter_tx,
                    output_tx,
                )
                .await
                .log_err();
            }
        });
        const MAX_CONCURRENT_FILE_SCANS: usize = 64;
        let filters = cx.background_spawn(async move {
            let fs = &fs;
            let query = &query;
            executor
                .scoped(move |scope| {
                    for _ in 0..MAX_CONCURRENT_FILE_SCANS {
                        let filter_rx = filter_rx.clone();
                        scope.spawn(async move {
                            Self::filter_paths(fs, filter_rx, query)
                                .await
                                .log_with_level(log::Level::Debug);
                        })
                    }
                })
                .await;
        });
        cx.background_spawn(async move {
            let mut matched = 0;
            while let Ok(mut receiver) = output_rx.recv().await {
                let Some(path) = receiver.next().await else {
                    continue;
                };
                let Ok(_) = matching_paths_tx.send(path).await else {
                    break;
                };
                matched += 1;
                if matched == limit {
                    break;
                }
            }
            drop(input);
            drop(filters);
        })
        .detach();
        matching_paths_rx
    }

    fn scan_ignored_dir<'a>(
        fs: &'a Arc<dyn Fs>,
        snapshot: &'a worktree::Snapshot,
        path: &'a Path,
        query: &'a SearchQuery,
        filter_tx: &'a Sender<MatchingEntry>,
        output_tx: &'a Sender<oneshot::Receiver<ProjectPath>>,
    ) -> BoxFuture<'a, Result<()>> {
        async move {
            let abs_path = snapshot.abs_path().join(path);
            let Some(mut files) = fs
                .read_dir(&abs_path)
                .await
                .with_context(|| format!("listing ignored path {abs_path:?}"))
                .log_err()
            else {
                return Ok(());
            };

            let mut results = Vec::new();

            while let Some(Ok(file)) = files.next().await {
                let Some(metadata) = fs
                    .metadata(&file)
                    .await
                    .with_context(|| format!("fetching fs metadata for {abs_path:?}"))
                    .log_err()
                    .flatten()
                else {
                    continue;
                };
                if metadata.is_symlink || metadata.is_fifo {
                    continue;
                }
                results.push((
                    file.strip_prefix(snapshot.abs_path())?.to_path_buf(),
                    !metadata.is_dir,
                ))
            }
            results.sort_by(|(a_path, _), (b_path, _)| a_path.cmp(b_path));
            for (path, is_file) in results {
                if is_file {
                    if query.filters_path() {
                        let matched_path = if query.match_full_paths() {
                            let mut full_path = PathBuf::from(snapshot.root_name());
                            full_path.push(&path);
                            query.match_path(&full_path)
                        } else {
                            query.match_path(&path)
                        };
                        if !matched_path {
                            continue;
                        }
                    }
                    let (tx, rx) = oneshot::channel();
                    output_tx.send(rx).await?;
                    filter_tx
                        .send(MatchingEntry {
                            respond: tx,
                            worktree_path: snapshot.abs_path().clone(),
                            path: ProjectPath {
                                worktree_id: snapshot.id(),
                                path: Arc::from(path),
                            },
                        })
                        .await?;
                } else {
                    Self::scan_ignored_dir(fs, snapshot, &path, query, filter_tx, output_tx)
                        .await?;
                }
            }
            Ok(())
        }
        .boxed()
    }

    async fn find_candidate_paths(
        fs: Arc<dyn Fs>,
        snapshots: Vec<(worktree::Snapshot, WorktreeSettings)>,
        open_entries: HashSet<ProjectEntryId>,
        query: SearchQuery,
        filter_tx: Sender<MatchingEntry>,
        output_tx: Sender<oneshot::Receiver<ProjectPath>>,
    ) -> Result<()> {
        for (snapshot, settings) in snapshots {
            for entry in snapshot.entries(query.include_ignored(), 0) {
                if entry.is_dir() && entry.is_ignored {
                    if !settings.is_path_excluded(&entry.path) {
                        Self::scan_ignored_dir(
                            &fs,
                            &snapshot,
                            &entry.path,
                            &query,
                            &filter_tx,
                            &output_tx,
                        )
                        .await?;
                    }
                    continue;
                }

                if entry.is_fifo || !entry.is_file() {
                    continue;
                }

                if query.filters_path() {
                    let matched_path = if query.match_full_paths() {
                        let mut full_path = PathBuf::from(snapshot.root_name());
                        full_path.push(&entry.path);
                        query.match_path(&full_path)
                    } else {
                        query.match_path(&entry.path)
                    };
                    if !matched_path {
                        continue;
                    }
                }

                let (mut tx, rx) = oneshot::channel();

                if open_entries.contains(&entry.id) {
                    tx.send(ProjectPath {
                        worktree_id: snapshot.id(),
                        path: entry.path.clone(),
                    })
                    .await?;
                } else {
                    filter_tx
                        .send(MatchingEntry {
                            respond: tx,
                            worktree_path: snapshot.abs_path().clone(),
                            path: ProjectPath {
                                worktree_id: snapshot.id(),
                                path: entry.path.clone(),
                            },
                        })
                        .await?;
                }

                output_tx.send(rx).await?;
            }
        }
        Ok(())
    }

    async fn filter_paths(
        fs: &Arc<dyn Fs>,
        input: Receiver<MatchingEntry>,
        query: &SearchQuery,
    ) -> Result<()> {
        let mut input = pin!(input);
        while let Some(mut entry) = input.next().await {
            let abs_path = entry.worktree_path.join(&entry.path.path);
            let Some(file) = fs.open_sync(&abs_path).await.log_err() else {
                continue;
            };

            let mut file = BufReader::new(file);
            let file_start = file.fill_buf()?;

            if let Err(Some(starting_position)) =
                std::str::from_utf8(file_start).map_err(|e| e.error_len())
            {
                // Before attempting to match the file content, throw away files that have invalid UTF-8 sequences early on;
                // That way we can still match files in a streaming fashion without having look at "obviously binary" files.
                log::debug!(
                    "Invalid UTF-8 sequence in file {abs_path:?} at byte position {starting_position}"
                );
                continue;
            }

            if query.detect(file).unwrap_or(false) {
                entry.respond.send(entry.path).await?
            }
        }

        Ok(())
    }

    pub fn fs(&self) -> Arc<dyn Fs> {
        self.fs.clone()
    }
}

#[derive(Clone, Debug)]
enum WorktreeHandle {
    Strong(Entity<Worktree>),
    Weak(WeakEntity<Worktree>),
}

impl WorktreeHandle {
    fn upgrade(&self) -> Option<Entity<Worktree>> {
        match self {
            WorktreeHandle::Strong(handle) => Some(handle.clone()),
            WorktreeHandle::Weak(handle) => handle.upgrade(),
        }
    }
}
