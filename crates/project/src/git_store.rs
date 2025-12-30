mod conflict_set;
pub mod git_traversal;

use crate::{
    ProjectEnvironment, ProjectItem, ProjectPath,
    buffer_store::{BufferStore, BufferStoreEvent},
    worktree_store::{WorktreeStore, WorktreeStoreEvent},
};
use anyhow::{Context as _, Result, anyhow, bail};
use askpass::AskPassDelegate;
use buffer_diff::{BufferDiff, BufferDiffEvent};
use collections::HashMap;
pub use conflict_set::{ConflictRegion, ConflictSet, ConflictSetSnapshot, ConflictSetUpdate};
use fs::Fs;
use futures::{
    FutureExt, StreamExt as _,
    channel::{mpsc, oneshot},
    future::{self, Shared},
};
use git::{
    BuildPermalinkParams, GitHostingProviderRegistry, WORK_DIRECTORY_REPO_PATH,
    blame::Blame,
    parse_git_remote_url,
    repository::{
        Branch, CommitDetails, CommitDiff, CommitOptions, DiffType, GitRepository,
        GitRepositoryCheckpoint, PushOptions, Remote, RemoteCommandOutput, RepoPath, ResetMode,
    },
    status::{FileStatus, GitSummary},
};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, EventEmitter, SharedString, Subscription, Task,
    WeakEntity,
};
use language::{Buffer, BufferEvent, Language, LanguageRegistry};
use postage::stream::Stream as _;
use serde::Deserialize;
use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
    mem,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{self, AtomicU64},
    },
    time::Instant,
};
use sum_tree::{Edit, SumTree, TreeSet};
use text::{Bias, BufferId};
use util::{ResultExt, post_inc};
use worktree::{
    File, PathKey, PathProgress, PathSummary, PathTarget, UpdatedGitRepositoriesSet,
    UpdatedGitRepository, Worktree,
};

pub struct GitStore {
    state: GitStoreState,
    buffer_store: Entity<BufferStore>,
    worktree_store: Entity<WorktreeStore>,
    repositories: HashMap<RepositoryId, Entity<Repository>>,
    active_repo_id: Option<RepositoryId>,
    #[allow(clippy::type_complexity)]
    loading_diffs:
        HashMap<(BufferId, DiffKind), Shared<Task<Result<Entity<BufferDiff>, Arc<anyhow::Error>>>>>,
    diffs: HashMap<BufferId, Entity<BufferGitState>>,
    _subscriptions: Vec<Subscription>,
}

struct BufferGitState {
    unstaged_diff: Option<WeakEntity<BufferDiff>>,
    uncommitted_diff: Option<WeakEntity<BufferDiff>>,
    conflict_set: Option<WeakEntity<ConflictSet>>,
    recalculate_diff_task: Option<Task<Result<()>>>,
    reparse_conflict_markers_task: Option<Task<Result<()>>>,
    language: Option<Arc<Language>>,
    language_registry: Option<Arc<LanguageRegistry>>,
    conflict_updated_futures: Vec<oneshot::Sender<()>>,
    recalculating_tx: postage::watch::Sender<bool>,

    /// These operation counts are used to ensure that head and index text
    /// values read from the git repository are up-to-date with any hunk staging
    /// operations that have been performed on the BufferDiff.
    ///
    /// The operation count is incremented immediately when the user initiates a
    /// hunk stage/unstage operation. Then, upon finishing writing the new index
    /// text do disk, the `operation count as of write` is updated to reflect
    /// the operation count that prompted the write.
    hunk_staging_operation_count: usize,
    hunk_staging_operation_count_as_of_write: usize,

    head_text: Option<Arc<String>>,
    index_text: Option<Arc<String>>,
    head_changed: bool,
    index_changed: bool,
    language_changed: bool,
}

#[derive(Clone, Debug)]
enum DiffBasesChange {
    SetIndex(Option<String>),
    SetHead(Option<String>),
    SetEach {
        index: Option<String>,
        head: Option<String>,
    },
    SetBoth(Option<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DiffKind {
    Unstaged,
    Uncommitted,
}

enum GitStoreState {
    Local {
        next_repository_id: Arc<AtomicU64>,
        project_environment: Entity<ProjectEnvironment>,
        fs: Arc<dyn Fs>,
    },
}

#[derive(Clone, Debug)]
pub struct GitStoreCheckpoint {
    checkpoints_by_work_dir_abs_path: HashMap<Arc<Path>, GitRepositoryCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub repo_path: RepoPath,
    pub status: FileStatus,
}

impl StatusEntry {
}

impl sum_tree::Item for StatusEntry {
    type Summary = PathSummary<GitSummary>;

    fn summary(&self, _: &<Self::Summary as sum_tree::Summary>::Context) -> Self::Summary {
        PathSummary {
            max_path: self.repo_path.0.clone(),
            item_summary: self.status.summary(),
        }
    }
}

impl sum_tree::KeyedItem for StatusEntry {
    type Key = PathKey;

    fn key(&self) -> Self::Key {
        PathKey(self.repo_path.0.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepositoryId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeDetails {
    pub conflicted_paths: TreeSet<RepoPath>,
    pub message: Option<SharedString>,
    pub heads: Vec<Option<SharedString>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositorySnapshot {
    pub id: RepositoryId,
    pub statuses_by_path: SumTree<StatusEntry>,
    pub work_directory_abs_path: Arc<Path>,
    pub branch: Option<Branch>,
    pub head_commit: Option<CommitDetails>,
    pub scan_id: u64,
    pub merge: MergeDetails,
}

type JobId = u64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobInfo {
    pub start: Instant,
    pub message: SharedString,
}

pub struct Repository {
    this: WeakEntity<Self>,
    snapshot: RepositorySnapshot,
    commit_message_buffer: Option<Entity<Buffer>>,
    git_store: WeakEntity<GitStore>,
    // For a local repository, holds paths that have had worktree events since the last status scan completed,
    // and that should be examined during the next status scan.
    paths_needing_status_update: BTreeSet<RepoPath>,
    job_sender: mpsc::UnboundedSender<GitJob>,
    active_jobs: HashMap<JobId, JobInfo>,
    job_id: JobId,
}

impl std::ops::Deref for Repository {
    type Target = RepositorySnapshot;

    fn deref(&self) -> &Self::Target {
        &self.snapshot
    }
}

#[derive(Clone)]
pub enum RepositoryState {
    Local {
        backend: Arc<dyn GitRepository>,
        environment: Arc<HashMap<String, String>>,
    },
}

#[derive(Clone, Debug)]
pub enum RepositoryEvent {
    Updated { full_scan: bool },
    MergeHeadsChanged,
}

#[derive(Clone, Debug)]
pub struct JobsUpdated;

#[derive(Debug)]
pub enum GitStoreEvent {
    ActiveRepositoryChanged(Option<RepositoryId>),
    RepositoryUpdated(RepositoryId, RepositoryEvent, bool),
    RepositoryAdded(RepositoryId),
    RepositoryRemoved(RepositoryId),
    IndexWriteError(anyhow::Error),
    JobsUpdated,
    ConflictsUpdated,
}

impl EventEmitter<RepositoryEvent> for Repository {}
impl EventEmitter<JobsUpdated> for Repository {}
impl EventEmitter<GitStoreEvent> for GitStore {}

pub struct GitJob {
    job: Box<dyn FnOnce(RepositoryState, &mut AsyncApp) -> Task<()>>,
    key: Option<GitJobKey>,
}

#[derive(PartialEq, Eq)]
enum GitJobKey {
    WriteIndex(RepoPath),
    ReloadBufferDiffBases,
    RefreshStatuses,
    ReloadGitState,
}

impl GitStore {
    pub fn local(
        worktree_store: &Entity<WorktreeStore>,
        buffer_store: Entity<BufferStore>,
        environment: Entity<ProjectEnvironment>,
        fs: Arc<dyn Fs>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(
            worktree_store.clone(),
            buffer_store,
            GitStoreState::Local {
                next_repository_id: Arc::new(AtomicU64::new(1)),
                project_environment: environment,
                fs,
            },
            cx,
        )
    }

    fn new(
        worktree_store: Entity<WorktreeStore>,
        buffer_store: Entity<BufferStore>,
        state: GitStoreState,
        cx: &mut Context<Self>,
    ) -> Self {
        let _subscriptions = vec![
            cx.subscribe(&worktree_store, Self::on_worktree_store_event),
            cx.subscribe(&buffer_store, Self::on_buffer_store_event),
        ];

        GitStore {
            state,
            buffer_store,
            worktree_store,
            repositories: HashMap::default(),
            active_repo_id: None,
            _subscriptions,
            loading_diffs: HashMap::default(),
            diffs: HashMap::default(),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self.state, GitStoreState::Local { .. })
    }

    pub fn active_repository(&self) -> Option<Entity<Repository>> {
        self.active_repo_id
            .as_ref()
            .map(|id| self.repositories[&id].clone())
    }

    pub fn open_unstaged_diff(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<BufferDiff>>> {
        let buffer_id = buffer.read(cx).remote_id();
        if let Some(diff_state) = self.diffs.get(&buffer_id) {
            if let Some(unstaged_diff) = diff_state
                .read(cx)
                .unstaged_diff
                .as_ref()
                .and_then(|weak| weak.upgrade())
            {
                if let Some(task) =
                    diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation())
                {
                    return cx.background_executor().spawn(async move {
                        task.await;
                        Ok(unstaged_diff)
                    });
                }
                return Task::ready(Ok(unstaged_diff));
            }
        }

        let Some((repo, repo_path)) =
            self.repository_and_path_for_buffer_id(buffer.read(cx).remote_id(), cx)
        else {
            return Task::ready(Err(anyhow!("failed to find git repository for buffer")));
        };

        let task = self
            .loading_diffs
            .entry((buffer_id, DiffKind::Unstaged))
            .or_insert_with(|| {
                let staged_text = repo.update(cx, |repo, cx| {
                    repo.load_staged_text(buffer_id, repo_path, cx)
                });
                cx.spawn(async move |this, cx| {
                    Self::open_diff_internal(
                        this,
                        DiffKind::Unstaged,
                        staged_text.await.map(DiffBasesChange::SetIndex),
                        buffer,
                        cx,
                    )
                    .await
                    .map_err(Arc::new)
                })
                .shared()
            })
            .clone();

        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    pub fn open_uncommitted_diff(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<BufferDiff>>> {
        let buffer_id = buffer.read(cx).remote_id();

        if let Some(diff_state) = self.diffs.get(&buffer_id) {
            if let Some(uncommitted_diff) = diff_state
                .read(cx)
                .uncommitted_diff
                .as_ref()
                .and_then(|weak| weak.upgrade())
            {
                if let Some(task) =
                    diff_state.update(cx, |diff_state, _| diff_state.wait_for_recalculation())
                {
                    return cx.background_executor().spawn(async move {
                        task.await;
                        Ok(uncommitted_diff)
                    });
                }
                return Task::ready(Ok(uncommitted_diff));
            }
        }

        let Some((repo, repo_path)) =
            self.repository_and_path_for_buffer_id(buffer.read(cx).remote_id(), cx)
        else {
            return Task::ready(Err(anyhow!("failed to find git repository for buffer")));
        };

        let task = self
            .loading_diffs
            .entry((buffer_id, DiffKind::Uncommitted))
            .or_insert_with(|| {
                let changes = repo.update(cx, |repo, cx| {
                    repo.load_committed_text(buffer_id, repo_path, cx)
                });

                cx.spawn(async move |this, cx| {
                    Self::open_diff_internal(this, DiffKind::Uncommitted, changes.await, buffer, cx)
                        .await
                        .map_err(Arc::new)
                })
                .shared()
            })
            .clone();

        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    async fn open_diff_internal(
        this: WeakEntity<Self>,
        kind: DiffKind,
        texts: Result<DiffBasesChange>,
        buffer_entity: Entity<Buffer>,
        cx: &mut AsyncApp,
    ) -> Result<Entity<BufferDiff>> {
        let diff_bases_change = match texts {
            Err(e) => {
                this.update(cx, |this, cx| {
                    let buffer = buffer_entity.read(cx);
                    let buffer_id = buffer.remote_id();
                    this.loading_diffs.remove(&(buffer_id, kind));
                })?;
                return Err(e);
            }
            Ok(change) => change,
        };

        this.update(cx, |this, cx| {
            let buffer = buffer_entity.read(cx);
            let buffer_id = buffer.remote_id();
            let language = buffer.language().cloned();
            let language_registry = buffer.language_registry();
            let text_snapshot = buffer.text_snapshot();
            this.loading_diffs.remove(&(buffer_id, kind));

            let diff_state = this
                .diffs
                .entry(buffer_id)
                .or_insert_with(|| cx.new(|_| BufferGitState::new()));

            let diff = cx.new(|cx| BufferDiff::new(&text_snapshot, cx));

            cx.subscribe(&diff, Self::on_buffer_diff_event).detach();
            diff_state.update(cx, |diff_state, cx| {
                diff_state.language = language;
                diff_state.language_registry = language_registry;

                match kind {
                    DiffKind::Unstaged => diff_state.unstaged_diff = Some(diff.downgrade()),
                    DiffKind::Uncommitted => {
                        let unstaged_diff = if let Some(diff) = diff_state.unstaged_diff() {
                            diff
                        } else {
                            let unstaged_diff = cx.new(|cx| BufferDiff::new(&text_snapshot, cx));
                            diff_state.unstaged_diff = Some(unstaged_diff.downgrade());
                            unstaged_diff
                        };

                        diff.update(cx, |diff, _| diff.set_secondary_diff(unstaged_diff));
                        diff_state.uncommitted_diff = Some(diff.downgrade())
                    }
                }

                diff_state.diff_bases_changed(text_snapshot, Some(diff_bases_change), cx);
                let rx = diff_state.wait_for_recalculation();

                anyhow::Ok(async move {
                    if let Some(rx) = rx {
                        rx.await;
                    }
                    Ok(diff)
                })
            })
        })??
        .await
    }

    pub fn get_unstaged_diff(&self, buffer_id: BufferId, cx: &App) -> Option<Entity<BufferDiff>> {
        let diff_state = self.diffs.get(&buffer_id)?;
        diff_state.read(cx).unstaged_diff.as_ref()?.upgrade()
    }

    pub fn get_uncommitted_diff(
        &self,
        buffer_id: BufferId,
        cx: &App,
    ) -> Option<Entity<BufferDiff>> {
        let diff_state = self.diffs.get(&buffer_id)?;
        diff_state.read(cx).uncommitted_diff.as_ref()?.upgrade()
    }

    pub fn open_conflict_set(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Entity<ConflictSet> {
        log::debug!("open conflict set");
        let buffer_id = buffer.read(cx).remote_id();

        if let Some(git_state) = self.diffs.get(&buffer_id) {
            if let Some(conflict_set) = git_state
                .read(cx)
                .conflict_set
                .as_ref()
                .and_then(|weak| weak.upgrade())
            {
                let conflict_set = conflict_set.clone();
                let buffer_snapshot = buffer.read(cx).text_snapshot();

                git_state.update(cx, |state, cx| {
                    let _ = state.reparse_conflict_markers(buffer_snapshot, cx);
                });

                return conflict_set;
            }
        }

        let is_unmerged = self
            .repository_and_path_for_buffer_id(buffer_id, cx)
            .map_or(false, |(repo, path)| {
                repo.read(cx).snapshot.has_conflict(&path)
            });
        let buffer_git_state = self
            .diffs
            .entry(buffer_id)
            .or_insert_with(|| cx.new(|_| BufferGitState::new()));
        let conflict_set = cx.new(|cx| ConflictSet::new(buffer_id, is_unmerged, cx));

        self._subscriptions
            .push(cx.subscribe(&conflict_set, |_, _, _, cx| {
                cx.emit(GitStoreEvent::ConflictsUpdated);
            }));

        buffer_git_state.update(cx, |state, cx| {
            state.conflict_set = Some(conflict_set.downgrade());
            let buffer_snapshot = buffer.read(cx).text_snapshot();
            let _ = state.reparse_conflict_markers(buffer_snapshot, cx);
        });

        conflict_set
    }

    pub fn project_path_git_status(
        &self,
        project_path: &ProjectPath,
        cx: &App,
    ) -> Option<FileStatus> {
        let (repo, repo_path) = self.repository_and_path_for_project_path(project_path, cx)?;
        Some(repo.read(cx).status_for_path(&repo_path)?.status)
    }

    pub fn checkpoint(&self, cx: &mut App) -> Task<Result<GitStoreCheckpoint>> {
        let mut work_directory_abs_paths = Vec::new();
        let mut checkpoints = Vec::new();
        for repository in self.repositories.values() {
            repository.update(cx, |repository, _| {
                work_directory_abs_paths.push(repository.snapshot.work_directory_abs_path.clone());
                checkpoints.push(repository.checkpoint().map(|checkpoint| checkpoint?));
            });
        }

        cx.background_executor().spawn(async move {
            let checkpoints = future::try_join_all(checkpoints).await?;
            Ok(GitStoreCheckpoint {
                checkpoints_by_work_dir_abs_path: work_directory_abs_paths
                    .into_iter()
                    .zip(checkpoints)
                    .collect(),
            })
        })
    }

    pub fn restore_checkpoint(
        &self,
        checkpoint: GitStoreCheckpoint,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let repositories_by_work_dir_abs_path = self
            .repositories
            .values()
            .map(|repo| (repo.read(cx).snapshot.work_directory_abs_path.clone(), repo))
            .collect::<HashMap<_, _>>();

        let mut tasks = Vec::new();
        for (work_dir_abs_path, checkpoint) in checkpoint.checkpoints_by_work_dir_abs_path {
            if let Some(repository) = repositories_by_work_dir_abs_path.get(&work_dir_abs_path) {
                let restore = repository.update(cx, |repository, _| {
                    repository.restore_checkpoint(checkpoint)
                });
                tasks.push(async move { restore.await? });
            }
        }
        cx.background_spawn(async move {
            future::try_join_all(tasks).await?;
            Ok(())
        })
    }

    /// Compares two checkpoints, returning true if they are equal.
    pub fn compare_checkpoints(
        &self,
        left: GitStoreCheckpoint,
        mut right: GitStoreCheckpoint,
        cx: &mut App,
    ) -> Task<Result<bool>> {
        let repositories_by_work_dir_abs_path = self
            .repositories
            .values()
            .map(|repo| (repo.read(cx).snapshot.work_directory_abs_path.clone(), repo))
            .collect::<HashMap<_, _>>();

        let mut tasks = Vec::new();
        for (work_dir_abs_path, left_checkpoint) in left.checkpoints_by_work_dir_abs_path {
            if let Some(right_checkpoint) = right
                .checkpoints_by_work_dir_abs_path
                .remove(&work_dir_abs_path)
            {
                if let Some(repository) = repositories_by_work_dir_abs_path.get(&work_dir_abs_path)
                {
                    let compare = repository.update(cx, |repository, _| {
                        repository.compare_checkpoints(left_checkpoint, right_checkpoint)
                    });

                    tasks.push(async move { compare.await? });
                }
            } else {
                return Task::ready(Ok(false));
            }
        }
        cx.background_spawn(async move {
            Ok(future::try_join_all(tasks)
                .await?
                .into_iter()
                .all(|result| result))
        })
    }

    /// Blames a buffer.
    pub fn blame_buffer(
        &self,
        buffer: &Entity<Buffer>,
        version: Option<clock::Global>,
        cx: &mut App,
    ) -> Task<Result<Option<Blame>>> {
        let buffer = buffer.read(cx);
        let Some((repo, repo_path)) =
            self.repository_and_path_for_buffer_id(buffer.remote_id(), cx)
        else {
            return Task::ready(Err(anyhow!("failed to find a git repository for buffer")));
        };
        let content = match &version {
            Some(version) => buffer.rope_for_version(version).clone(),
            None => buffer.as_rope().clone(),
        };

        let rx = repo.update(cx, |repo, _| {
            repo.send_job(None, move |state, _| async move {
                let RepositoryState::Local { backend, .. } = state;
                backend
                    .blame(repo_path.clone(), content)
                    .await
                    .with_context(|| format!("Failed to blame {:?}", repo_path.0))
                    .map(Some)
            })
        });

        cx.spawn(|_: &mut AsyncApp| async move { rx.await? })
    }

    pub fn get_permalink_to_line(
        &self,
        buffer: &Entity<Buffer>,
        selection: Range<u32>,
        cx: &mut App,
    ) -> Task<Result<url::Url>> {
        let Some(file) = File::from_dyn(buffer.read(cx).file()) else {
            return Task::ready(Err(anyhow!("buffer has no file")));
        };

        let Some((repo, repo_path)) = self.repository_and_path_for_project_path(
            &(file.worktree.read(cx).id(), file.path.clone()).into(),
            cx,
        ) else {
            // If we're not in a Git repo, check whether this is a Rust source
            // file in the Cargo registry (presumably opened with go-to-definition
            // from a normal Rust file). If so, we can put together a permalink
            // using crate metadata.
            if buffer
                .read(cx)
                .language()
                .is_none_or(|lang| lang.name() != "Rust".into())
            {
                return Task::ready(Err(anyhow!("no permalink available")));
            }
            let Some(file_path) = file.worktree.read(cx).absolutize(&file.path).ok() else {
                return Task::ready(Err(anyhow!("no permalink available")));
            };
            return cx.spawn(async move |cx| {
                let provider_registry = cx.update(GitHostingProviderRegistry::default_global)?;
                get_permalink_in_rust_registry_src(provider_registry, file_path, selection)
                    .context("no permalink available")
            });

            // TODO remote case
        };

        let branch = repo.read(cx).branch.clone();
        let remote = branch
            .as_ref()
            .and_then(|b| b.upstream.as_ref())
            .and_then(|b| b.remote_name())
            .unwrap_or("origin")
            .to_string();

        let rx = repo.update(cx, |repo, _| {
            repo.send_job(None, move |state, cx| async move {
                let RepositoryState::Local { backend, .. } = state;

                let origin_url = backend
                    .remote_url(&remote)
                    .with_context(|| format!("remote \"{remote}\" not found"))?;

                let sha = backend.head_sha().await.context("reading HEAD SHA")?;

                let provider_registry = cx.update(GitHostingProviderRegistry::default_global)?;

                let (provider, remote) = parse_git_remote_url(provider_registry, &origin_url)
                    .context("parsing Git remote URL")?;

                let path = repo_path.to_str().with_context(|| {
                    format!("converting repo path {repo_path:?} to string")
                })?;

                Ok(provider.build_permalink(
                    remote,
                    BuildPermalinkParams {
                        sha: &sha,
                        path,
                        selection: Some(selection),
                    },
                ))
            })
        });
        cx.spawn(|_: &mut AsyncApp| async move { rx.await? })
    }

    fn on_worktree_store_event(
        &mut self,
        worktree_store: Entity<WorktreeStore>,
        event: &WorktreeStoreEvent,
        cx: &mut Context<Self>,
    ) {
        let GitStoreState::Local {
            project_environment,
            next_repository_id,
            fs,
        } = &self.state;

        match event {
            WorktreeStoreEvent::WorktreeUpdatedEntries(worktree_id, updated_entries) => {
                let mut paths_by_git_repo = HashMap::<_, Vec<_>>::default();
                for (relative_path, _, _) in updated_entries.iter() {
                    let Some((repo, repo_path)) = self.repository_and_path_for_project_path(
                        &(*worktree_id, relative_path.clone()).into(),
                        cx,
                    ) else {
                        continue;
                    };
                    paths_by_git_repo.entry(repo).or_default().push(repo_path)
                }

                for (repo, paths) in paths_by_git_repo {
                    repo.update(cx, |repo, cx| {
                        repo.paths_changed(paths, cx);
                    });
                }
            }
            WorktreeStoreEvent::WorktreeUpdatedGitRepositories(worktree_id, changed_repos) => {
                let Some(worktree) = worktree_store.read(cx).worktree_for_id(*worktree_id, cx)
                else {
                    return;
                };
                if !worktree.read(cx).is_visible() {
                    log::debug!(
                        "not adding repositories for local worktree {:?} because it's not visible",
                        worktree.read(cx).abs_path()
                    );
                    return;
                }
                self.update_repositories_from_worktree(
                    project_environment.clone(),
                    next_repository_id.clone(),
                    changed_repos.clone(),
                    fs.clone(),
                    cx,
                );
                self.local_worktree_git_repos_changed(worktree, changed_repos, cx);
            }
            _ => {}
        }
    }

    fn on_repository_event(
        &mut self,
        repo: Entity<Repository>,
        event: &RepositoryEvent,
        cx: &mut Context<Self>,
    ) {
        let id = repo.read(cx).id;
        let repo_snapshot = repo.read(cx).snapshot.clone();
        for (buffer_id, diff) in self.diffs.iter() {
            if let Some((buffer_repo, repo_path)) =
                self.repository_and_path_for_buffer_id(*buffer_id, cx)
            {
                if buffer_repo == repo {
                    diff.update(cx, |diff, cx| {
                        if let Some(conflict_set) = &diff.conflict_set {
                            let conflict_status_changed =
                                conflict_set.update(cx, |conflict_set, cx| {
                                    let has_conflict = repo_snapshot.has_conflict(&repo_path);
                                    conflict_set.set_has_conflict(has_conflict, cx)
                                })?;
                            if conflict_status_changed {
                                let buffer_store = self.buffer_store.read(cx);
                                if let Some(buffer) = buffer_store.get(*buffer_id) {
                                    let _ = diff.reparse_conflict_markers(
                                        buffer.read(cx).text_snapshot(),
                                        cx,
                                    );
                                }
                            }
                        }
                        anyhow::Ok(())
                    })
                    .ok();
                }
            }
        }
        cx.emit(GitStoreEvent::RepositoryUpdated(
            id,
            event.clone(),
            self.active_repo_id == Some(id),
        ))
    }

    fn on_jobs_updated(&mut self, _: Entity<Repository>, _: &JobsUpdated, cx: &mut Context<Self>) {
        cx.emit(GitStoreEvent::JobsUpdated)
    }

    /// Update our list of repositories and schedule git scans in response to a notification from a worktree,
    fn update_repositories_from_worktree(
        &mut self,
        project_environment: Entity<ProjectEnvironment>,
        next_repository_id: Arc<AtomicU64>,
        updated_git_repositories: UpdatedGitRepositoriesSet,
        fs: Arc<dyn Fs>,
        cx: &mut Context<Self>,
    ) {
        let mut removed_ids = Vec::new();
        for update in updated_git_repositories.iter() {
            if let Some((id, existing)) = self.repositories.iter().find(|(_, repo)| {
                let existing_work_directory_abs_path =
                    repo.read(cx).work_directory_abs_path.clone();
                Some(&existing_work_directory_abs_path)
                    == update.old_work_directory_abs_path.as_ref()
                    || Some(&existing_work_directory_abs_path)
                        == update.new_work_directory_abs_path.as_ref()
            }) {
                if let Some(new_work_directory_abs_path) =
                    update.new_work_directory_abs_path.clone()
                {
                    existing.update(cx, |existing, cx| {
                        existing.snapshot.work_directory_abs_path = new_work_directory_abs_path;
                        existing.schedule_scan(cx);
                    });
                } else {
                    removed_ids.push(*id);
                }
            } else if let UpdatedGitRepository {
                new_work_directory_abs_path: Some(work_directory_abs_path),
                dot_git_abs_path: Some(dot_git_abs_path),
                repository_dir_abs_path: Some(repository_dir_abs_path),
                common_dir_abs_path: Some(common_dir_abs_path),
                ..
            } = update
            {
                let id = RepositoryId(next_repository_id.fetch_add(1, atomic::Ordering::Release));
                let git_store = cx.weak_entity();
                let repo = cx.new(|cx| {
                    let mut repo = Repository::local(
                        id,
                        work_directory_abs_path.clone(),
                        dot_git_abs_path.clone(),
                        repository_dir_abs_path.clone(),
                        common_dir_abs_path.clone(),
                        project_environment.downgrade(),
                        fs.clone(),
                        git_store,
                        cx,
                    );
                    repo.schedule_scan(cx);
                    repo
                });
                self._subscriptions
                    .push(cx.subscribe(&repo, Self::on_repository_event));
                self._subscriptions
                    .push(cx.subscribe(&repo, Self::on_jobs_updated));
                self.repositories.insert(id, repo);
                cx.emit(GitStoreEvent::RepositoryAdded(id));
                self.active_repo_id.get_or_insert_with(|| {
                    cx.emit(GitStoreEvent::ActiveRepositoryChanged(Some(id)));
                    id
                });
            }
        }

        for id in removed_ids {
            if self.active_repo_id == Some(id) {
                self.active_repo_id = None;
                cx.emit(GitStoreEvent::ActiveRepositoryChanged(None));
            }
            self.repositories.remove(&id);
        }
    }

    fn on_buffer_store_event(
        &mut self,
        _: Entity<BufferStore>,
        event: &BufferStoreEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BufferStoreEvent::BufferAdded(buffer) => {
                cx.subscribe(&buffer, |this, buffer, event, cx| {
                    if let BufferEvent::LanguageChanged = event {
                        let buffer_id = buffer.read(cx).remote_id();
                        if let Some(diff_state) = this.diffs.get(&buffer_id) {
                            diff_state.update(cx, |diff_state, cx| {
                                diff_state.buffer_language_changed(buffer, cx);
                            });
                        }
                    }
                })
                .detach();
            }
            BufferStoreEvent::BufferDropped(buffer_id) => {
                self.diffs.remove(&buffer_id);
            }

            _ => {}
        }
    }

    pub fn recalculate_buffer_diffs(
        &mut self,
        buffers: Vec<Entity<Buffer>>,
        cx: &mut Context<Self>,
    ) -> impl Future<Output = ()> + use<> {
        let mut futures = Vec::new();
        for buffer in buffers {
            if let Some(diff_state) = self.diffs.get_mut(&buffer.read(cx).remote_id()) {
                let buffer = buffer.read(cx).text_snapshot();
                diff_state.update(cx, |diff_state, cx| {
                    diff_state.recalculate_diffs(buffer.clone(), cx);
                    futures.extend(diff_state.wait_for_recalculation().map(FutureExt::boxed));
                });
                futures.push(diff_state.update(cx, |diff_state, cx| {
                    diff_state
                        .reparse_conflict_markers(buffer, cx)
                        .map(|_| {})
                        .boxed()
                }));
            }
        }
        async move {
            futures::future::join_all(futures).await;
        }
    }

    fn on_buffer_diff_event(
        &mut self,
        diff: Entity<buffer_diff::BufferDiff>,
        event: &BufferDiffEvent,
        cx: &mut Context<Self>,
    ) {
        if let BufferDiffEvent::HunksStagedOrUnstaged(new_index_text) = event {
            let buffer_id = diff.read(cx).buffer_id;
            if let Some(diff_state) = self.diffs.get(&buffer_id) {
                let hunk_staging_operation_count = diff_state.update(cx, |diff_state, _| {
                    diff_state.hunk_staging_operation_count += 1;
                    diff_state.hunk_staging_operation_count
                });
                if let Some((repo, path)) = self.repository_and_path_for_buffer_id(buffer_id, cx) {
                    let recv = repo.update(cx, |repo, cx| {
                        log::debug!("hunks changed for {}", path.display());
                        repo.spawn_set_index_text_job(
                            path,
                            new_index_text.as_ref().map(|rope| rope.to_string()),
                            Some(hunk_staging_operation_count),
                            cx,
                        )
                    });
                    let diff = diff.downgrade();
                    cx.spawn(async move |this, cx| {
                        if let Ok(Err(error)) = cx.background_spawn(recv).await {
                            diff.update(cx, |diff, cx| {
                                diff.clear_pending_hunks(cx);
                            })
                            .ok();
                            this.update(cx, |_, cx| cx.emit(GitStoreEvent::IndexWriteError(error)))
                                .ok();
                        }
                    })
                    .detach();
                }
            }
        }
    }

    fn local_worktree_git_repos_changed(
        &mut self,
        worktree: Entity<Worktree>,
        changed_repos: &UpdatedGitRepositoriesSet,
        cx: &mut Context<Self>,
    ) {
        log::debug!("local worktree repos changed");
        debug_assert!(worktree.read(cx).is_local());

        for repository in self.repositories.values() {
            repository.update(cx, |repository, cx| {
                let repo_abs_path = &repository.work_directory_abs_path;
                if changed_repos.iter().any(|update| {
                    update.old_work_directory_abs_path.as_ref() == Some(&repo_abs_path)
                        || update.new_work_directory_abs_path.as_ref() == Some(&repo_abs_path)
                }) {
                    repository.reload_buffer_diff_bases(cx);
                }
            });
        }
    }

    pub fn repositories(&self) -> &HashMap<RepositoryId, Entity<Repository>> {
        &self.repositories
    }

    pub fn status_for_buffer_id(&self, buffer_id: BufferId, cx: &App) -> Option<FileStatus> {
        let (repo, path) = self.repository_and_path_for_buffer_id(buffer_id, cx)?;
        let status = repo.read(cx).snapshot.status_for_path(&path)?;
        Some(status.status)
    }

    pub fn repository_and_path_for_buffer_id(
        &self,
        buffer_id: BufferId,
        cx: &App,
    ) -> Option<(Entity<Repository>, RepoPath)> {
        let buffer = self.buffer_store.read(cx).get(buffer_id)?;
        let project_path = buffer.read(cx).project_path(cx)?;
        self.repository_and_path_for_project_path(&project_path, cx)
    }

    pub fn repository_and_path_for_project_path(
        &self,
        path: &ProjectPath,
        cx: &App,
    ) -> Option<(Entity<Repository>, RepoPath)> {
        let abs_path = self.worktree_store.read(cx).absolutize(path, cx)?;
        self.repositories
            .values()
            .filter_map(|repo| {
                let repo_path = repo.read(cx).abs_path_to_repo_path(&abs_path)?;
                Some((repo.clone(), repo_path))
            })
            .max_by_key(|(repo, _)| repo.read(cx).work_directory_abs_path.clone())
    }

    pub fn git_init(
        &self,
        path: Arc<Path>,
        fallback_branch_name: String,
        cx: &App,
    ) -> Task<Result<()>> {
        let GitStoreState::Local { fs, .. } = &self.state;
        let fs = fs.clone();
        cx.background_executor()
            .spawn(async move { fs.git_init(&path, fallback_branch_name) })
    }

    pub fn repo_snapshots(&self, cx: &App) -> HashMap<RepositoryId, RepositorySnapshot> {
        self.repositories
            .iter()
            .map(|(id, repo)| (*id, repo.read(cx).snapshot.clone()))
            .collect()
    }
}

impl BufferGitState {
    fn new() -> Self {
        Self {
            unstaged_diff: Default::default(),
            uncommitted_diff: Default::default(),
            recalculate_diff_task: Default::default(),
            language: Default::default(),
            language_registry: Default::default(),
            recalculating_tx: postage::watch::channel_with(false).0,
            hunk_staging_operation_count: 0,
            hunk_staging_operation_count_as_of_write: 0,
            head_text: Default::default(),
            index_text: Default::default(),
            head_changed: Default::default(),
            index_changed: Default::default(),
            language_changed: Default::default(),
            conflict_updated_futures: Default::default(),
            conflict_set: Default::default(),
            reparse_conflict_markers_task: Default::default(),
        }
    }

    fn buffer_language_changed(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        self.language = buffer.read(cx).language().cloned();
        self.language_changed = true;
        let _ = self.recalculate_diffs(buffer.read(cx).text_snapshot(), cx);
    }

    fn reparse_conflict_markers(
        &mut self,
        buffer: text::BufferSnapshot,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();

        let Some(conflict_set) = self
            .conflict_set
            .as_ref()
            .and_then(|conflict_set| conflict_set.upgrade())
        else {
            return rx;
        };

        let old_snapshot = conflict_set.read_with(cx, |conflict_set, _| {
            if conflict_set.has_conflict {
                Some(conflict_set.snapshot())
            } else {
                None
            }
        });

        if let Some(old_snapshot) = old_snapshot {
            self.conflict_updated_futures.push(tx);
            self.reparse_conflict_markers_task = Some(cx.spawn(async move |this, cx| {
                let (snapshot, changed_range) = cx
                    .background_spawn(async move {
                        let new_snapshot = ConflictSet::parse(&buffer);
                        let changed_range = old_snapshot.compare(&new_snapshot, &buffer);
                        (new_snapshot, changed_range)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(conflict_set) = &this.conflict_set {
                        conflict_set
                            .update(cx, |conflict_set, cx| {
                                conflict_set.set_snapshot(snapshot, changed_range, cx);
                            })
                            .ok();
                    }
                    let futures = std::mem::take(&mut this.conflict_updated_futures);
                    for tx in futures {
                        tx.send(()).ok();
                    }
                })
            }))
        }

        rx
    }

    fn unstaged_diff(&self) -> Option<Entity<BufferDiff>> {
        self.unstaged_diff.as_ref().and_then(|set| set.upgrade())
    }

    fn uncommitted_diff(&self) -> Option<Entity<BufferDiff>> {
        self.uncommitted_diff.as_ref().and_then(|set| set.upgrade())
    }

    pub fn wait_for_recalculation(&mut self) -> Option<impl Future<Output = ()> + use<>> {
        if *self.recalculating_tx.borrow() {
            let mut rx = self.recalculating_tx.subscribe();
            return Some(async move {
                loop {
                    let is_recalculating = rx.recv().await;
                    if is_recalculating != Some(true) {
                        break;
                    }
                }
            });
        } else {
            None
        }
    }

    fn diff_bases_changed(
        &mut self,
        buffer: text::BufferSnapshot,
        diff_bases_change: Option<DiffBasesChange>,
        cx: &mut Context<Self>,
    ) {
        match diff_bases_change {
            Some(DiffBasesChange::SetIndex(index)) => {
                self.index_text = index.map(|mut index| {
                    text::LineEnding::normalize(&mut index);
                    Arc::new(index)
                });
                self.index_changed = true;
            }
            Some(DiffBasesChange::SetHead(head)) => {
                self.head_text = head.map(|mut head| {
                    text::LineEnding::normalize(&mut head);
                    Arc::new(head)
                });
                self.head_changed = true;
            }
            Some(DiffBasesChange::SetBoth(text)) => {
                let text = text.map(|mut text| {
                    text::LineEnding::normalize(&mut text);
                    Arc::new(text)
                });
                self.head_text = text.clone();
                self.index_text = text;
                self.head_changed = true;
                self.index_changed = true;
            }
            Some(DiffBasesChange::SetEach { index, head }) => {
                self.index_text = index.map(|mut index| {
                    text::LineEnding::normalize(&mut index);
                    Arc::new(index)
                });
                self.index_changed = true;
                self.head_text = head.map(|mut head| {
                    text::LineEnding::normalize(&mut head);
                    Arc::new(head)
                });
                self.head_changed = true;
            }
            None => {}
        }

        self.recalculate_diffs(buffer, cx)
    }

    fn recalculate_diffs(&mut self, buffer: text::BufferSnapshot, cx: &mut Context<Self>) {
        *self.recalculating_tx.borrow_mut() = true;

        let language = self.language.clone();
        let language_registry = self.language_registry.clone();
        let unstaged_diff = self.unstaged_diff();
        let uncommitted_diff = self.uncommitted_diff();
        let head = self.head_text.clone();
        let index = self.index_text.clone();
        let index_changed = self.index_changed;
        let head_changed = self.head_changed;
        let language_changed = self.language_changed;
        let prev_hunk_staging_operation_count = self.hunk_staging_operation_count_as_of_write;
        let index_matches_head = match (self.index_text.as_ref(), self.head_text.as_ref()) {
            (Some(index), Some(head)) => Arc::ptr_eq(index, head),
            (None, None) => true,
            _ => false,
        };
        self.recalculate_diff_task = Some(cx.spawn(async move |this, cx| {
            log::debug!(
                "start recalculating diffs for buffer {}",
                buffer.remote_id()
            );

            let mut new_unstaged_diff = None;
            if let Some(unstaged_diff) = &unstaged_diff {
                new_unstaged_diff = Some(
                    BufferDiff::update_diff(
                        unstaged_diff.clone(),
                        buffer.clone(),
                        index,
                        index_changed,
                        language_changed,
                        language.clone(),
                        language_registry.clone(),
                        cx,
                    )
                    .await?,
                );
            }

            let mut new_uncommitted_diff = None;
            if let Some(uncommitted_diff) = &uncommitted_diff {
                new_uncommitted_diff = if index_matches_head {
                    new_unstaged_diff.clone()
                } else {
                    Some(
                        BufferDiff::update_diff(
                            uncommitted_diff.clone(),
                            buffer.clone(),
                            head,
                            head_changed,
                            language_changed,
                            language.clone(),
                            language_registry.clone(),
                            cx,
                        )
                        .await?,
                    )
                }
            }

            let cancel = this.update(cx, |this, _| {
                // This checks whether all pending stage/unstage operations
                // have quiesced (i.e. both the corresponding write and the
                // read of that write have completed). If not, then we cancel
                // this recalculation attempt to avoid invalidating pending
                // state too quickly; another recalculation will come along
                // later and clear the pending state once the state of the index has settled.
                if this.hunk_staging_operation_count > prev_hunk_staging_operation_count {
                    *this.recalculating_tx.borrow_mut() = false;
                    true
                } else {
                    false
                }
            })?;
            if cancel {
                log::debug!(
                    concat!(
                        "aborting recalculating diffs for buffer {}",
                        "due to subsequent hunk operations",
                    ),
                    buffer.remote_id()
                );
                return Ok(());
            }

            let unstaged_changed_range = if let Some((unstaged_diff, new_unstaged_diff)) =
                unstaged_diff.as_ref().zip(new_unstaged_diff.clone())
            {
                unstaged_diff.update(cx, |diff, cx| {
                    if language_changed {
                        diff.language_changed(cx);
                    }
                    diff.set_snapshot(new_unstaged_diff, &buffer, cx)
                })?
            } else {
                None
            };

            if let Some((uncommitted_diff, new_uncommitted_diff)) =
                uncommitted_diff.as_ref().zip(new_uncommitted_diff.clone())
            {
                uncommitted_diff.update(cx, |diff, cx| {
                    if language_changed {
                        diff.language_changed(cx);
                    }
                    diff.set_snapshot_with_secondary(
                        new_uncommitted_diff,
                        &buffer,
                        unstaged_changed_range,
                        true,
                        cx,
                    );
                })?;
            }

            log::debug!(
                "finished recalculating diffs for buffer {}",
                buffer.remote_id()
            );

            if let Some(this) = this.upgrade() {
                this.update(cx, |this, _| {
                    this.index_changed = false;
                    this.head_changed = false;
                    this.language_changed = false;
                    *this.recalculating_tx.borrow_mut() = false;
                })?;
            }

            Ok(())
        }));
    }
}

impl RepositorySnapshot {
    fn empty(id: RepositoryId, work_directory_abs_path: Arc<Path>) -> Self {
        Self {
            id,
            statuses_by_path: Default::default(),
            work_directory_abs_path,
            branch: None,
            head_commit: None,
            scan_id: 0,
            merge: Default::default(),
        }
    }

    pub fn status(&self) -> impl Iterator<Item = StatusEntry> + '_ {
        self.statuses_by_path.iter().cloned()
    }

    pub fn status_summary(&self) -> GitSummary {
        self.statuses_by_path.summary().item_summary
    }

    pub fn status_for_path(&self, path: &RepoPath) -> Option<StatusEntry> {
        self.statuses_by_path
            .get(&PathKey(path.0.clone()), &())
            .cloned()
    }

    pub fn abs_path_to_repo_path(&self, abs_path: &Path) -> Option<RepoPath> {
        abs_path
            .strip_prefix(&self.work_directory_abs_path)
            .map(RepoPath::from)
            .ok()
    }

    pub fn had_conflict_on_last_merge_head_change(&self, repo_path: &RepoPath) -> bool {
        self.merge.conflicted_paths.contains(&repo_path)
    }

    pub fn has_conflict(&self, repo_path: &RepoPath) -> bool {
        let had_conflict_on_last_merge_head_change =
            self.merge.conflicted_paths.contains(&repo_path);
        let has_conflict_currently = self
            .status_for_path(&repo_path)
            .map_or(false, |entry| entry.status.is_conflicted());
        had_conflict_on_last_merge_head_change || has_conflict_currently
    }

    /// This is the name that will be displayed in the repository selector for this repository.
    pub fn display_name(&self) -> SharedString {
        self.work_directory_abs_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
            .into()
    }
}

impl MergeDetails {
    async fn load(
        backend: &Arc<dyn GitRepository>,
        status: &SumTree<StatusEntry>,
        prev_snapshot: &RepositorySnapshot,
    ) -> Result<(MergeDetails, bool)> {
        log::debug!("load merge details");
        let message = backend.merge_message().await;
        let heads = backend
            .revparse_batch(vec![
                "MERGE_HEAD".into(),
                "CHERRY_PICK_HEAD".into(),
                "REBASE_HEAD".into(),
                "REVERT_HEAD".into(),
                "APPLY_HEAD".into(),
            ])
            .await
            .log_err()
            .unwrap_or_default()
            .into_iter()
            .map(|opt| opt.map(SharedString::from))
            .collect::<Vec<_>>();
        let merge_heads_changed = heads != prev_snapshot.merge.heads;
        let conflicted_paths = if merge_heads_changed {
            let current_conflicted_paths = TreeSet::from_ordered_entries(
                status
                    .iter()
                    .filter(|entry| entry.status.is_conflicted())
                    .map(|entry| entry.repo_path.clone()),
            );

            // It can happen that we run a scan while a lengthy merge is in progress
            // that will eventually result in conflicts, but before those conflicts
            // are reported by `git status`. Since for the moment we only care about
            // the merge heads state for the purposes of tracking conflicts, don't update
            // this state until we see some conflicts.
            if heads.iter().any(Option::is_some)
                && !prev_snapshot.merge.heads.iter().any(Option::is_some)
                && current_conflicted_paths.is_empty()
            {
                log::debug!("not updating merge heads because no conflicts found");
                return Ok((
                    MergeDetails {
                        message: message.map(SharedString::from),
                        ..prev_snapshot.merge.clone()
                    },
                    false,
                ));
            }

            current_conflicted_paths
        } else {
            prev_snapshot.merge.conflicted_paths.clone()
        };
        let details = MergeDetails {
            conflicted_paths,
            message: message.map(SharedString::from),
            heads,
        };
        Ok((details, merge_heads_changed))
    }
}

impl Repository {
    pub fn snapshot(&self) -> RepositorySnapshot {
        self.snapshot.clone()
    }

    fn local(
        id: RepositoryId,
        work_directory_abs_path: Arc<Path>,
        dot_git_abs_path: Arc<Path>,
        repository_dir_abs_path: Arc<Path>,
        common_dir_abs_path: Arc<Path>,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<dyn Fs>,
        git_store: WeakEntity<GitStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = RepositorySnapshot::empty(id, work_directory_abs_path.clone());
        Repository {
            this: cx.weak_entity(),
            git_store,
            snapshot,
            commit_message_buffer: None,
            paths_needing_status_update: Default::default(),
            job_sender: Repository::spawn_local_git_worker(
                work_directory_abs_path,
                dot_git_abs_path,
                repository_dir_abs_path,
                common_dir_abs_path,
                project_environment,
                fs,
                cx,
            ),
            job_id: 0,
            active_jobs: Default::default(),
        }
    }

    pub fn git_store(&self) -> Option<Entity<GitStore>> {
        self.git_store.upgrade()
    }

    fn reload_buffer_diff_bases(&mut self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let git_store = self.git_store.clone();
        let _ = self.send_keyed_job(
            Some(GitJobKey::ReloadBufferDiffBases),
            None,
            |state, mut cx| async move {
                let RepositoryState::Local { backend, .. } = state;

                let Some(this) = this.upgrade() else {
                    return Ok(());
                };

                let repo_diff_state_updates = this.update(&mut cx, |this, cx| {
                    git_store.update(cx, |git_store, cx| {
                        git_store
                            .diffs
                            .iter()
                            .filter_map(|(buffer_id, diff_state)| {
                                let buffer_store = git_store.buffer_store.read(cx);
                                let buffer = buffer_store.get(*buffer_id)?;
                                let file = File::from_dyn(buffer.read(cx).file())?;
                                let abs_path =
                                    file.worktree.read(cx).absolutize(&file.path).ok()?;
                                let repo_path = this.abs_path_to_repo_path(&abs_path)?;
                                log::debug!(
                                    "start reload diff bases for repo path {}",
                                    repo_path.0.display()
                                );
                                diff_state.update(cx, |diff_state, _| {
                                    let has_unstaged_diff = diff_state
                                        .unstaged_diff
                                        .as_ref()
                                        .is_some_and(|diff| diff.is_upgradable());
                                    let has_uncommitted_diff = diff_state
                                        .uncommitted_diff
                                        .as_ref()
                                        .is_some_and(|set| set.is_upgradable());

                                    Some((
                                        buffer,
                                        repo_path,
                                        has_unstaged_diff.then(|| diff_state.index_text.clone()),
                                        has_uncommitted_diff.then(|| diff_state.head_text.clone()),
                                    ))
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                })??;

                let buffer_diff_base_changes = cx
                    .background_spawn(async move {
                        let mut changes = Vec::new();
                        for (buffer, repo_path, current_index_text, current_head_text) in
                            &repo_diff_state_updates
                        {
                            let index_text = if current_index_text.is_some() {
                                backend.load_index_text(repo_path.clone()).await
                            } else {
                                None
                            };
                            let head_text = if current_head_text.is_some() {
                                backend.load_committed_text(repo_path.clone()).await
                            } else {
                                None
                            };

                            let change =
                                match (current_index_text.as_ref(), current_head_text.as_ref()) {
                                    (Some(current_index), Some(current_head)) => {
                                        let index_changed =
                                            index_text.as_ref() != current_index.as_deref();
                                        let head_changed =
                                            head_text.as_ref() != current_head.as_deref();
                                        if index_changed && head_changed {
                                            if index_text == head_text {
                                                Some(DiffBasesChange::SetBoth(head_text))
                                            } else {
                                                Some(DiffBasesChange::SetEach {
                                                    index: index_text,
                                                    head: head_text,
                                                })
                                            }
                                        } else if index_changed {
                                            Some(DiffBasesChange::SetIndex(index_text))
                                        } else if head_changed {
                                            Some(DiffBasesChange::SetHead(head_text))
                                        } else {
                                            None
                                        }
                                    }
                                    (Some(current_index), None) => {
                                        let index_changed =
                                            index_text.as_ref() != current_index.as_deref();
                                        index_changed
                                            .then_some(DiffBasesChange::SetIndex(index_text))
                                    }
                                    (None, Some(current_head)) => {
                                        let head_changed =
                                            head_text.as_ref() != current_head.as_deref();
                                        head_changed.then_some(DiffBasesChange::SetHead(head_text))
                                    }
                                    (None, None) => None,
                                };

                            changes.push((buffer.clone(), change))
                        }
                        changes
                    })
                    .await;

                git_store.update(&mut cx, |git_store, cx| {
                    for (buffer, diff_bases_change) in buffer_diff_base_changes {
                        let buffer_snapshot = buffer.read(cx).text_snapshot();
                        let buffer_id = buffer_snapshot.remote_id();
                        let Some(diff_state) = git_store.diffs.get(&buffer_id) else {
                            continue;
                        };

                        diff_state.update(cx, |diff_state, cx| {
                            diff_state.diff_bases_changed(buffer_snapshot, diff_bases_change, cx);
                        });
                    }
                })
            },
        );
    }

    pub fn send_job<F, Fut, R>(
        &mut self,
        status: Option<SharedString>,
        job: F,
    ) -> oneshot::Receiver<R>
    where
        F: FnOnce(RepositoryState, AsyncApp) -> Fut + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        self.send_keyed_job(None, status, job)
    }

    fn send_keyed_job<F, Fut, R>(
        &mut self,
        key: Option<GitJobKey>,
        status: Option<SharedString>,
        job: F,
    ) -> oneshot::Receiver<R>
    where
        F: FnOnce(RepositoryState, AsyncApp) -> Fut + 'static,
        Fut: Future<Output = R> + 'static,
        R: Send + 'static,
    {
        let (result_tx, result_rx) = futures::channel::oneshot::channel();
        let job_id = post_inc(&mut self.job_id);
        let this = self.this.clone();
        self.job_sender
            .unbounded_send(GitJob {
                key,
                job: Box::new(move |state, cx: &mut AsyncApp| {
                    let job = job(state, cx.clone());
                    cx.spawn(async move |cx| {
                        if let Some(s) = status.clone() {
                            this.update(cx, |this, cx| {
                                this.active_jobs.insert(
                                    job_id,
                                    JobInfo {
                                        start: Instant::now(),
                                        message: s.clone(),
                                    },
                                );

                                cx.notify();
                            })
                            .ok();
                        }
                        let result = job.await;

                        this.update(cx, |this, cx| {
                            this.active_jobs.remove(&job_id);
                            cx.notify();
                        })
                        .ok();

                        result_tx.send(result).ok();
                    })
                }),
            })
            .ok();
        result_rx
    }

    pub fn set_as_active_repository(&self, cx: &mut Context<Self>) {
        let Some(git_store) = self.git_store.upgrade() else {
            return;
        };
        let entity = cx.entity();
        git_store.update(cx, |git_store, cx| {
            let Some((&id, _)) = git_store
                .repositories
                .iter()
                .find(|(_, handle)| *handle == &entity)
            else {
                return;
            };
            git_store.active_repo_id = Some(id);
            cx.emit(GitStoreEvent::ActiveRepositoryChanged(Some(id)));
        });
    }

    pub fn cached_status(&self) -> impl '_ + Iterator<Item = StatusEntry> {
        self.snapshot.status()
    }

    pub fn repo_path_to_project_path(&self, path: &RepoPath, cx: &App) -> Option<ProjectPath> {
        let git_store = self.git_store.upgrade()?;
        let worktree_store = git_store.read(cx).worktree_store.read(cx);
        let abs_path = self.snapshot.work_directory_abs_path.join(&path.0);
        let (worktree, relative_path) = worktree_store.find_worktree(abs_path, cx)?;
        Some(ProjectPath {
            worktree_id: worktree.read(cx).id(),
            path: relative_path.into(),
        })
    }

    pub fn project_path_to_repo_path(&self, path: &ProjectPath, cx: &App) -> Option<RepoPath> {
        let git_store = self.git_store.upgrade()?;
        let worktree_store = git_store.read(cx).worktree_store.read(cx);
        let abs_path = worktree_store.absolutize(path, cx)?;
        self.snapshot.abs_path_to_repo_path(&abs_path)
    }

    pub fn contains_sub_repo(&self, other: &Entity<Self>, cx: &App) -> bool {
        other
            .read(cx)
            .snapshot
            .work_directory_abs_path
            .starts_with(&self.snapshot.work_directory_abs_path)
    }

    pub fn open_commit_buffer(
        &mut self,
        languages: Option<Arc<LanguageRegistry>>,
        buffer_store: Entity<BufferStore>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Buffer>>> {
        if let Some(buffer) = self.commit_message_buffer.clone() {
            return Task::ready(Ok(buffer));
        }
        let this = cx.weak_entity();

        let rx = self.send_job(None, move |state, mut cx| async move {
            let Some(this) = this.upgrade() else {
                bail!("git store was dropped");
            };
            let _ = state;
            this.update(&mut cx, |_, cx| {
                Self::open_local_commit_buffer(languages, buffer_store, cx)
            })?
            .await
        });

        cx.spawn(|_, _: &mut AsyncApp| async move { rx.await? })
    }

    fn open_local_commit_buffer(
        language_registry: Option<Arc<LanguageRegistry>>,
        buffer_store: Entity<BufferStore>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Buffer>>> {
        cx.spawn(async move |repository, cx| {
            let buffer = buffer_store
                .update(cx, |buffer_store, cx| buffer_store.create_buffer(cx))?
                .await?;

            if let Some(language_registry) = language_registry {
                let git_commit_language = language_registry.language_for_name("Git Commit").await?;
                buffer.update(cx, |buffer, cx| {
                    buffer.set_language(Some(git_commit_language), cx);
                })?;
            }

            repository.update(cx, |repository, _| {
                repository.commit_message_buffer = Some(buffer.clone());
            })?;
            Ok(buffer)
        })
    }

    pub fn checkout_files(
        &mut self,
        commit: &str,
        paths: Vec<RepoPath>,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<()>> {
        let commit = commit.to_string();

        self.send_job(
            Some(format!("git checkout {}", commit).into()),
            move |git_repo, _| async move {
                let RepositoryState::Local {
                    backend,
                    environment,
                    ..
                } = git_repo;
                backend
                    .checkout_files(commit, paths, environment.clone())
                    .await
            },
        )
    }

    pub fn reset(
        &mut self,
        commit: String,
        reset_mode: ResetMode,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<()>> {
        let commit = commit.to_string();

        self.send_job(None, move |git_repo, _| async move {
            let RepositoryState::Local {
                backend,
                environment,
                ..
            } = git_repo;
            backend.reset(commit, reset_mode, environment).await
        })
    }

    pub fn show(&mut self, commit: String) -> oneshot::Receiver<Result<CommitDetails>> {
        self.send_job(None, move |git_repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = git_repo;
            backend.show(commit).await
        })
    }

    pub fn load_commit_diff(&mut self, commit: String) -> oneshot::Receiver<Result<CommitDiff>> {
        self.send_job(None, move |git_repo, cx| async move {
            let RepositoryState::Local { backend, .. } = git_repo;
            backend.load_commit(commit, cx).await
        })
    }

    fn buffer_store(&self, cx: &App) -> Option<Entity<BufferStore>> {
        Some(self.git_store.upgrade()?.read(cx).buffer_store.clone())
    }

    pub fn stage_entries(
        &self,
        entries: Vec<RepoPath>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        if entries.is_empty() {
            return Task::ready(Ok(()));
        }

        let mut save_futures = Vec::new();
        if let Some(buffer_store) = self.buffer_store(cx) {
            buffer_store.update(cx, |buffer_store, cx| {
                for path in &entries {
                    let Some(project_path) = self.repo_path_to_project_path(path, cx) else {
                        continue;
                    };
                    if let Some(buffer) = buffer_store.get_by_path(&project_path, cx) {
                        if buffer
                            .read(cx)
                            .file()
                            .map_or(false, |file| file.disk_state().exists())
                        {
                            save_futures.push(buffer_store.save_buffer(buffer, cx));
                        }
                    }
                }
            })
        }

        cx.spawn(async move |this, cx| {
            for save_future in save_futures {
                save_future.await?;
            }

            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    let RepositoryState::Local {
                        backend,
                        environment,
                        ..
                    } = git_repo;
                    backend.stage_paths(entries, environment.clone()).await
                })
            })?
            .await??;

            Ok(())
        })
    }

    pub fn unstage_entries(
        &self,
        entries: Vec<RepoPath>,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        if entries.is_empty() {
            return Task::ready(Ok(()));
        }

        let mut save_futures = Vec::new();
        if let Some(buffer_store) = self.buffer_store(cx) {
            buffer_store.update(cx, |buffer_store, cx| {
                for path in &entries {
                    let Some(project_path) = self.repo_path_to_project_path(path, cx) else {
                        continue;
                    };
                    if let Some(buffer) = buffer_store.get_by_path(&project_path, cx) {
                        if buffer
                            .read(cx)
                            .file()
                            .map_or(false, |file| file.disk_state().exists())
                        {
                            save_futures.push(buffer_store.save_buffer(buffer, cx));
                        }
                    }
                }
            })
        }

        cx.spawn(async move |this, cx| {
            for save_future in save_futures {
                save_future.await?;
            }

            this.update(cx, |this, _| {
                this.send_job(None, move |git_repo, _cx| async move {
                    let RepositoryState::Local {
                        backend,
                        environment,
                        ..
                    } = git_repo;
                    backend.unstage_paths(entries, environment).await
                })
            })?
            .await??;

            Ok(())
        })
    }

    pub fn stage_all(&self, cx: &mut Context<Self>) -> Task<anyhow::Result<()>> {
        let to_stage = self
            .cached_status()
            .filter(|entry| !entry.status.staging().is_fully_staged())
            .map(|entry| entry.repo_path.clone())
            .collect();
        self.stage_entries(to_stage, cx)
    }

    pub fn unstage_all(&self, cx: &mut Context<Self>) -> Task<anyhow::Result<()>> {
        let to_unstage = self
            .cached_status()
            .filter(|entry| entry.status.staging().has_staged())
            .map(|entry| entry.repo_path.clone())
            .collect();
        self.unstage_entries(to_unstage, cx)
    }

    pub fn commit(
        &mut self,
        message: SharedString,
        name_and_email: Option<(SharedString, SharedString)>,
        options: CommitOptions,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<()>> {
        self.send_job(Some("git commit".into()), move |git_repo, _cx| async move {
            let RepositoryState::Local {
                backend,
                environment,
                ..
            } = git_repo;
            backend
                .commit(message, name_and_email, options, environment)
                .await
        })
    }

    pub fn fetch(
        &mut self,
        askpass: AskPassDelegate,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        self.send_job(Some("git fetch".into()), move |git_repo, cx| async move {
            let RepositoryState::Local {
                backend,
                environment,
                ..
            } = git_repo;
            backend.fetch(askpass, environment, cx).await
        })
    }

    pub fn push(
        &mut self,
        branch: SharedString,
        remote: SharedString,
        options: Option<PushOptions>,
        askpass: AskPassDelegate,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        let args = options
            .map(|option| match option {
                PushOptions::SetUpstream => " --set-upstream",
                PushOptions::Force => " --force-with-lease",
            })
            .unwrap_or("");

        let this = cx.weak_entity();
        self.send_job(
            Some(format!("git push {} {} {}", args, branch, remote).into()),
            move |git_repo, mut cx| async move {
                let RepositoryState::Local {
                    backend,
                    environment,
                    ..
                } = git_repo;

                let result = backend
                    .push(
                        branch.to_string(),
                        remote.to_string(),
                        options,
                        askpass,
                        environment.clone(),
                        cx.clone(),
                    )
                    .await;
                if result.is_ok() {
                    let branches = backend.branches().await?;
                    let branch = branches.into_iter().find(|branch| branch.is_head);
                    log::info!("head branch after scan is {branch:?}");
                    this.update(&mut cx, |this, cx| {
                        this.snapshot.branch = branch;
                        cx.emit(RepositoryEvent::Updated { full_scan: false });
                    })?;
                }
                result
            },
        )
    }

    pub fn pull(
        &mut self,
        branch: SharedString,
        remote: SharedString,
        askpass: AskPassDelegate,
        _cx: &mut App,
    ) -> oneshot::Receiver<Result<RemoteCommandOutput>> {
        self.send_job(
            Some(format!("git pull {} {}", remote, branch).into()),
            move |git_repo, cx| async move {
                let RepositoryState::Local {
                    backend,
                    environment,
                    ..
                } = git_repo;
                backend
                    .pull(
                        branch.to_string(),
                        remote.to_string(),
                        askpass,
                        environment.clone(),
                        cx,
                    )
                    .await
            },
        )
    }

    fn spawn_set_index_text_job(
        &mut self,
        path: RepoPath,
        content: Option<String>,
        hunk_staging_operation_count: Option<usize>,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<anyhow::Result<()>> {
        let this = cx.weak_entity();
        let git_store = self.git_store.clone();
        self.send_keyed_job(
            Some(GitJobKey::WriteIndex(path.clone())),
            None,
            move |git_repo, mut cx| async move {
                log::debug!("start updating index text for buffer {}", path.display());
                let RepositoryState::Local {
                    backend,
                    environment,
                    ..
                } = git_repo;
                backend
                    .set_index_text(path.clone(), content, environment.clone())
                    .await?;
                log::debug!("finish updating index text for buffer {}", path.display());

                if let Some(hunk_staging_operation_count) = hunk_staging_operation_count {
                    let project_path = this
                        .read_with(&cx, |this, cx| this.repo_path_to_project_path(&path, cx))
                        .ok()
                        .flatten();
                    git_store.update(&mut cx, |git_store, cx| {
                        let buffer_id = git_store
                            .buffer_store
                            .read(cx)
                            .get_by_path(&project_path?, cx)?
                            .read(cx)
                            .remote_id();
                        let diff_state = git_store.diffs.get(&buffer_id)?;
                        diff_state.update(cx, |diff_state, _| {
                            diff_state.hunk_staging_operation_count_as_of_write =
                                hunk_staging_operation_count;
                        });
                        Some(())
                    })?;
                }
                Ok(())
            },
        )
    }

    pub fn get_remotes(
        &mut self,
        branch_name: Option<String>,
    ) -> oneshot::Receiver<Result<Vec<Remote>>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.get_remotes(branch_name).await
        })
    }

    pub fn branches(&mut self) -> oneshot::Receiver<Result<Vec<Branch>>> {
        self.send_job(None, move |repo, _| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.branches().await
        })
    }

    pub fn diff(&mut self, diff_type: DiffType, _cx: &App) -> oneshot::Receiver<Result<String>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.diff(diff_type).await
        })
    }

    pub fn create_branch(&mut self, branch_name: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(
            Some(format!("git switch -c {branch_name}").into()),
            move |repo, _cx| async move {
                let RepositoryState::Local { backend, .. } = repo;
                backend.create_branch(branch_name).await
            },
        )
    }

    pub fn change_branch(&mut self, branch_name: String) -> oneshot::Receiver<Result<()>> {
        self.send_job(
            Some(format!("git switch {branch_name}").into()),
            move |repo, _cx| async move {
                let RepositoryState::Local { backend, .. } = repo;
                backend.change_branch(branch_name).await
            },
        )
    }

    pub fn check_for_pushed_commits(&mut self) -> oneshot::Receiver<Result<Vec<SharedString>>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.check_for_pushed_commit().await
        })
    }

    pub fn checkpoint(&mut self) -> oneshot::Receiver<Result<GitRepositoryCheckpoint>> {
        self.send_job(None, |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.checkpoint().await
        })
    }

    pub fn restore_checkpoint(
        &mut self,
        checkpoint: GitRepositoryCheckpoint,
    ) -> oneshot::Receiver<Result<()>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.restore_checkpoint(checkpoint).await
        })
    }

    pub fn compare_checkpoints(
        &mut self,
        left: GitRepositoryCheckpoint,
        right: GitRepositoryCheckpoint,
    ) -> oneshot::Receiver<Result<bool>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend.compare_checkpoints(left, right).await
        })
    }

    pub fn diff_checkpoints(
        &mut self,
        base_checkpoint: GitRepositoryCheckpoint,
        target_checkpoint: GitRepositoryCheckpoint,
    ) -> oneshot::Receiver<Result<String>> {
        self.send_job(None, move |repo, _cx| async move {
            let RepositoryState::Local { backend, .. } = repo;
            backend
                .diff_checkpoints(base_checkpoint, target_checkpoint)
                .await
        })
    }

    fn schedule_scan(&mut self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let _ = self.send_keyed_job(
            Some(GitJobKey::ReloadGitState),
            None,
            |state, mut cx| async move {
                log::debug!("run scheduled git status scan");

                let Some(this) = this.upgrade() else {
                    return Ok::<(), anyhow::Error>(());
                };
                let RepositoryState::Local { backend, .. } = state;
                let (snapshot, events) = this
                    .read_with(&mut cx, |this, _| {
                        compute_snapshot(
                            this.id,
                            this.work_directory_abs_path.clone(),
                            this.snapshot.clone(),
                            backend.clone(),
                        )
                    })?
                    .await?;
                this.update(&mut cx, |this, cx| {
                    this.snapshot = snapshot.clone();
                    for event in events {
                        cx.emit(event);
                    }
                })?;
                Ok::<(), anyhow::Error>(())
            },
        );
    }

    fn spawn_local_git_worker(
        work_directory_abs_path: Arc<Path>,
        dot_git_abs_path: Arc<Path>,
        _repository_dir_abs_path: Arc<Path>,
        _common_dir_abs_path: Arc<Path>,
        project_environment: WeakEntity<ProjectEnvironment>,
        fs: Arc<dyn Fs>,
        cx: &mut Context<Self>,
    ) -> mpsc::UnboundedSender<GitJob> {
        let (job_tx, mut job_rx) = mpsc::unbounded::<GitJob>();

        cx.spawn(async move |_, cx| {
            let environment = project_environment
                .upgrade()
                .context("missing project environment")?
                .update(cx, |project_environment, cx| {
                    project_environment.get_directory_environment(work_directory_abs_path.clone(), cx)
                })?
                .await
                .unwrap_or_else(|| {
                    log::error!("failed to get working directory environment for repository {work_directory_abs_path:?}");
                    HashMap::default()
                });
            let backend = cx
                .background_spawn(async move {
                    fs.open_repo(&dot_git_abs_path)
                        .with_context(|| format!("opening repository at {dot_git_abs_path:?}"))
                })
                .await?;

            if let Some(git_hosting_provider_registry) =
                cx.update(|cx| GitHostingProviderRegistry::try_global(cx))?
            {
                git_hosting_providers::register_additional_providers(
                    git_hosting_provider_registry,
                    backend.clone(),
                );
            }

            let state = RepositoryState::Local {
                backend,
                environment: Arc::new(environment),
            };
            let mut jobs = VecDeque::new();
            loop {
                while let Ok(Some(next_job)) = job_rx.try_next() {
                    jobs.push_back(next_job);
                }

                if let Some(job) = jobs.pop_front() {
                    if let Some(current_key) = &job.key {
                        if jobs
                            .iter()
                            .any(|other_job| other_job.key.as_ref() == Some(current_key))
                        {
                            continue;
                        }
                    }
                    (job.job)(state.clone(), cx).await;
                } else if let Some(job) = job_rx.next().await {
                    jobs.push_back(job);
                } else {
                    break;
                }
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);

        job_tx
    }

    fn load_staged_text(
        &mut self,
        buffer_id: BufferId,
        repo_path: RepoPath,
        cx: &App,
    ) -> Task<Result<Option<String>>> {
        let rx = self.send_job(None, move |state, _| async move {
            let _ = buffer_id;
            let RepositoryState::Local { backend, .. } = state;
            anyhow::Ok(backend.load_index_text(repo_path).await)
        });
        cx.spawn(|_: &mut AsyncApp| async move { rx.await? })
    }

    fn load_committed_text(
        &mut self,
        buffer_id: BufferId,
        repo_path: RepoPath,
        cx: &App,
    ) -> Task<Result<DiffBasesChange>> {
        let rx = self.send_job(None, move |state, _| async move {
            let _ = buffer_id;
            let RepositoryState::Local { backend, .. } = state;
            let committed_text = backend.load_committed_text(repo_path.clone()).await;
            let staged_text = backend.load_index_text(repo_path).await;
            let diff_bases_change = if committed_text == staged_text {
                DiffBasesChange::SetBoth(committed_text)
            } else {
                DiffBasesChange::SetEach {
                    index: staged_text,
                    head: committed_text,
                }
            };
            anyhow::Ok(diff_bases_change)
        });

        cx.spawn(|_: &mut AsyncApp| async move { rx.await? })
    }

    fn paths_changed(
        &mut self,
        paths: Vec<RepoPath>,
        cx: &mut Context<Self>,
    ) {
        self.paths_needing_status_update.extend(paths);

        let this = cx.weak_entity();
        let _ = self.send_keyed_job(
            Some(GitJobKey::RefreshStatuses),
            None,
            |state, mut cx| async move {
                let (prev_snapshot, mut changed_paths) = this.update(&mut cx, |this, _| {
                    (
                        this.snapshot.clone(),
                        mem::take(&mut this.paths_needing_status_update),
                    )
                })?;
                let RepositoryState::Local { backend, .. } = state;

                let paths = changed_paths.iter().cloned().collect::<Vec<_>>();
                let statuses = backend.status(&paths).await?;

                let changed_path_statuses = cx
                    .background_spawn(async move {
                        let mut changed_path_statuses = Vec::new();
                        let prev_statuses = prev_snapshot.statuses_by_path.clone();
                        let mut cursor = prev_statuses.cursor::<PathProgress>(&());

                        for (repo_path, status) in &*statuses.entries {
                            changed_paths.remove(repo_path);
                            if cursor.seek_forward(&PathTarget::Path(repo_path), Bias::Left, &()) {
                                if cursor.item().is_some_and(|entry| entry.status == *status) {
                                    continue;
                                }
                            }

                            changed_path_statuses.push(Edit::Insert(StatusEntry {
                                repo_path: repo_path.clone(),
                                status: *status,
                            }));
                        }
                        let mut cursor = prev_statuses.cursor::<PathProgress>(&());
                        for path in changed_paths.into_iter() {
                            if cursor.seek_forward(&PathTarget::Path(&path), Bias::Left, &()) {
                                changed_path_statuses.push(Edit::Remove(PathKey(path.0)));
                            }
                        }
                        changed_path_statuses
                    })
                    .await;

                this.update(&mut cx, |this, cx| {
                    if !changed_path_statuses.is_empty() {
                        this.snapshot
                            .statuses_by_path
                            .edit(changed_path_statuses, &());
                        this.snapshot.scan_id += 1;
                    }
                    cx.emit(RepositoryEvent::Updated { full_scan: false });
                })
            },
        );
    }

    /// currently running git command and when it started
    pub fn current_job(&self) -> Option<JobInfo> {
        self.active_jobs.values().next().cloned()
    }

    pub fn barrier(&mut self) -> oneshot::Receiver<()> {
        self.send_job(None, |_, _| async {})
    }
}

fn get_permalink_in_rust_registry_src(
    provider_registry: Arc<GitHostingProviderRegistry>,
    path: PathBuf,
    selection: Range<u32>,
) -> Result<url::Url> {
    #[derive(Deserialize)]
    struct CargoVcsGit {
        sha1: String,
    }

    #[derive(Deserialize)]
    struct CargoVcsInfo {
        git: CargoVcsGit,
        path_in_vcs: String,
    }

    #[derive(Deserialize)]
    struct CargoPackage {
        repository: String,
    }

    #[derive(Deserialize)]
    struct CargoToml {
        package: CargoPackage,
    }

    let Some((dir, cargo_vcs_info_json)) = path.ancestors().skip(1).find_map(|dir| {
        let json = std::fs::read_to_string(dir.join(".cargo_vcs_info.json")).ok()?;
        Some((dir, json))
    }) else {
        bail!("No .cargo_vcs_info.json found in parent directories")
    };
    let cargo_vcs_info = serde_json::from_str::<CargoVcsInfo>(&cargo_vcs_info_json)?;
    let cargo_toml = std::fs::read_to_string(dir.join("Cargo.toml"))?;
    let manifest = toml::from_str::<CargoToml>(&cargo_toml)?;
    let (provider, remote) = parse_git_remote_url(provider_registry, &manifest.package.repository)
        .context("parsing package.repository field of manifest")?;
    let path = PathBuf::from(cargo_vcs_info.path_in_vcs).join(path.strip_prefix(dir).unwrap());
    let permalink = provider.build_permalink(
        remote,
        BuildPermalinkParams {
            sha: &cargo_vcs_info.git.sha1,
            path: &path.to_string_lossy(),
            selection: Some(selection),
        },
    );
    Ok(permalink)
}

async fn compute_snapshot(
    id: RepositoryId,
    work_directory_abs_path: Arc<Path>,
    prev_snapshot: RepositorySnapshot,
    backend: Arc<dyn GitRepository>,
) -> Result<(RepositorySnapshot, Vec<RepositoryEvent>)> {
    let mut events = Vec::new();
    let branches = backend.branches().await?;
    let branch = branches.into_iter().find(|branch| branch.is_head);
    let statuses = backend.status(&[WORK_DIRECTORY_REPO_PATH.clone()]).await?;
    let statuses_by_path = SumTree::from_iter(
        statuses
            .entries
            .iter()
            .map(|(repo_path, status)| StatusEntry {
                repo_path: repo_path.clone(),
                status: *status,
            }),
        &(),
    );
    let (merge_details, merge_heads_changed) =
        MergeDetails::load(&backend, &statuses_by_path, &prev_snapshot).await?;
    log::debug!("new merge details (changed={merge_heads_changed:?}): {merge_details:?}");

    if merge_heads_changed
        || branch != prev_snapshot.branch
        || statuses_by_path != prev_snapshot.statuses_by_path
    {
        events.push(RepositoryEvent::Updated { full_scan: true });
    }

    // Cache merge conflict paths so they don't change from staging/unstaging,
    // until the merge heads change (at commit time, etc.).
    if merge_heads_changed {
        events.push(RepositoryEvent::MergeHeadsChanged);
    }

    // Useful when branch is None in detached head state
    let head_commit = match backend.head_sha().await {
        Some(head_sha) => backend.show(head_sha).await.log_err(),
        None => None,
    };

    let snapshot = RepositorySnapshot {
        id,
        statuses_by_path,
        work_directory_abs_path,
        scan_id: prev_snapshot.scan_id + 1,
        branch,
        head_commit,
        merge: merge_details,
    };

    Ok((snapshot, events))
}
