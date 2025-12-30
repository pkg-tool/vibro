use crate::{
    ProjectItem as _, ProjectPath,
    search::SearchQuery,
    worktree_store::{WorktreeStore, WorktreeStoreEvent},
};
use anyhow::{Context as _, Result, anyhow};
use collections::{HashMap, HashSet, hash_map};
use fs::Fs;
use futures::{Future, FutureExt as _, StreamExt, future::Shared};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Subscription, Task, WeakEntity,
};
use language::{
    Buffer, BufferEvent, Capability, DiskState, File as _, Language,
};
use smol::channel::Receiver;
use std::{io, path::Path, pin::pin, sync::Arc};
use text::BufferId;
use util::{ResultExt as _, debug_panic};
use worktree::{File, PathChange, ProjectEntryId, Worktree};

/// A set of open buffers.
pub struct BufferStore {
    state: LocalBufferStore,
    #[allow(clippy::type_complexity)]
    loading_buffers: HashMap<ProjectPath, Shared<Task<Result<Entity<Buffer>, Arc<anyhow::Error>>>>>,
    worktree_store: Entity<WorktreeStore>,
    opened_buffers: HashMap<BufferId, WeakEntity<Buffer>>,
    path_to_buffer_id: HashMap<ProjectPath, BufferId>,
}

struct LocalBufferStore {
    local_buffer_ids_by_entry_id: HashMap<ProjectEntryId, BufferId>,
    worktree_store: Entity<WorktreeStore>,
    _subscription: Subscription,
}

pub enum BufferStoreEvent {
    BufferAdded(Entity<Buffer>),
    BufferOpened {
        buffer: Entity<Buffer>,
        project_path: ProjectPath,
    },
    BufferDropped(BufferId),
    BufferChangedFilePath {
        buffer: Entity<Buffer>,
        old_file: Option<Arc<dyn language::File>>,
    },
}

#[derive(Default, Debug)]
pub struct ProjectTransaction(pub HashMap<Entity<Buffer>, language::Transaction>);

impl EventEmitter<BufferStoreEvent> for BufferStore {}

impl LocalBufferStore {
    fn save_local_buffer(
        &self,
        buffer_handle: Entity<Buffer>,
        worktree: Entity<Worktree>,
        path: Arc<Path>,
        mut has_changed_file: bool,
        cx: &mut Context<BufferStore>,
    ) -> Task<Result<()>> {
        let buffer = buffer_handle.read(cx);

        let text = buffer.as_rope().clone();
        let line_ending = buffer.line_ending();
        let version = buffer.version();
        let file = buffer.file().cloned();
        if file
            .as_ref()
            .is_some_and(|file| file.disk_state() == DiskState::New)
        {
            has_changed_file = true;
        }

        let save = worktree.update(cx, |worktree, cx| {
            worktree.write_file(path.as_ref(), text, line_ending, cx)
        });

        cx.spawn(async move |_, cx| {
            let new_file = save.await?;
            let mtime = new_file.disk_state().mtime();
            buffer_handle.update(cx, |buffer, cx| {
                if has_changed_file {
                    buffer.file_updated(new_file, cx);
                }
                buffer.did_save(version.clone(), mtime, cx);
            })
        })
    }

    fn subscribe_to_worktree(
        &mut self,
        worktree: &Entity<Worktree>,
        cx: &mut Context<BufferStore>,
    ) {
        cx.subscribe(worktree, |this, worktree, event, cx| {
            if worktree.read(cx).is_local() {
                match event {
                    worktree::Event::UpdatedEntries(changes) => {
                        Self::local_worktree_entries_changed(this, &worktree, changes, cx);
                    }
                    _ => {}
                }
            }
        })
        .detach();
    }

    fn local_worktree_entries_changed(
        this: &mut BufferStore,
        worktree_handle: &Entity<Worktree>,
        changes: &[(Arc<Path>, ProjectEntryId, PathChange)],
        cx: &mut Context<BufferStore>,
    ) {
        let snapshot = worktree_handle.read(cx).snapshot();
        for (path, entry_id, _) in changes {
            Self::local_worktree_entry_changed(
                this,
                *entry_id,
                path,
                worktree_handle,
                &snapshot,
                cx,
            );
        }
    }

    fn local_worktree_entry_changed(
        this: &mut BufferStore,
        entry_id: ProjectEntryId,
        path: &Arc<Path>,
        worktree: &Entity<worktree::Worktree>,
        snapshot: &worktree::Snapshot,
        cx: &mut Context<BufferStore>,
    ) -> Option<()> {
        let project_path = ProjectPath {
            worktree_id: snapshot.id(),
            path: path.clone(),
        };

        let buffer_id = this
            .state
            .local_buffer_ids_by_entry_id
            .get(&entry_id)
            .copied()
            .or_else(|| this.path_to_buffer_id.get(&project_path).copied())?;

        let buffer = if let Some(buffer) = this.get(buffer_id) {
            Some(buffer)
        } else {
            this.opened_buffers.remove(&buffer_id);
            None
        };

        let buffer = if let Some(buffer) = buffer {
            buffer
        } else {
            this.path_to_buffer_id.remove(&project_path);
            this.state.local_buffer_ids_by_entry_id.remove(&entry_id);
            return None;
        };

        let events = buffer.update(cx, |buffer, cx| {
            let file = buffer.file()?;
            let old_file = File::from_dyn(Some(file))?;
            if old_file.worktree != *worktree {
                return None;
            }

            let snapshot_entry = old_file
                .entry_id
                .and_then(|entry_id| snapshot.entry_for_id(entry_id))
                .or_else(|| snapshot.entry_for_path(old_file.path.as_ref()));

            let new_file = if let Some(entry) = snapshot_entry {
                File {
                    disk_state: match entry.mtime {
                        Some(mtime) => DiskState::Present { mtime },
                        None => old_file.disk_state,
                    },
                    is_local: true,
                    entry_id: Some(entry.id),
                    path: entry.path.clone(),
                    worktree: worktree.clone(),
                    is_private: entry.is_private,
                }
            } else {
                File {
                    disk_state: DiskState::Deleted,
                    is_local: true,
                    entry_id: old_file.entry_id,
                    path: old_file.path.clone(),
                    worktree: worktree.clone(),
                    is_private: old_file.is_private,
                }
            };

            if new_file == *old_file {
                return None;
            }

            let mut events = Vec::new();
            if new_file.path != old_file.path {
                this.path_to_buffer_id.remove(&ProjectPath {
                    path: old_file.path.clone(),
                    worktree_id: old_file.worktree_id(cx),
                });
                this.path_to_buffer_id.insert(
                    ProjectPath {
                        worktree_id: new_file.worktree_id(cx),
                        path: new_file.path.clone(),
                    },
                    buffer_id,
                );
                events.push(BufferStoreEvent::BufferChangedFilePath {
                    buffer: cx.entity(),
                    old_file: buffer.file().cloned(),
                });
            }
            if new_file.entry_id != old_file.entry_id {
                if let Some(entry_id) = old_file.entry_id {
                    this.state.local_buffer_ids_by_entry_id.remove(&entry_id);
                }
                if let Some(entry_id) = new_file.entry_id {
                    this.state
                        .local_buffer_ids_by_entry_id
                        .insert(entry_id, buffer_id);
                }
            }

            buffer.file_updated(Arc::new(new_file), cx);
            Some(events)
        })?;

        for event in events {
            cx.emit(event);
        }

        None
    }

    fn save_buffer(
        &self,
        buffer: Entity<Buffer>,
        cx: &mut Context<BufferStore>,
    ) -> Task<Result<()>> {
        let Some(file) = File::from_dyn(buffer.read(cx).file()) else {
            return Task::ready(Err(anyhow!("buffer doesn't have a file")));
        };
        let worktree = file.worktree.clone();
        self.save_local_buffer(buffer, worktree, file.path.clone(), false, cx)
    }

    fn save_buffer_as(
        &self,
        buffer: Entity<Buffer>,
        path: ProjectPath,
        cx: &mut Context<BufferStore>,
    ) -> Task<Result<()>> {
        let Some(worktree) = self
            .worktree_store
            .read(cx)
            .worktree_for_id(path.worktree_id, cx)
        else {
            return Task::ready(Err(anyhow!("no such worktree")));
        };
        self.save_local_buffer(buffer, worktree, path.path.clone(), true, cx)
    }

    fn open_buffer(
        &self,
        path: Arc<Path>,
        worktree: Entity<Worktree>,
        cx: &mut Context<BufferStore>,
    ) -> Task<Result<Entity<Buffer>>> {
        let load_buffer = worktree.update(cx, |worktree, cx| {
            let load_file = worktree.load_file(path.as_ref(), cx);
            let reservation = cx.reserve_entity();
            let buffer_id = BufferId::from(reservation.entity_id().as_non_zero_u64());
            cx.spawn(async move |_, cx| {
                let loaded = load_file.await?;
                let text_buffer = cx
                    .background_spawn(async move { text::Buffer::new(0, buffer_id, loaded.text) })
                    .await;
                cx.insert_entity(reservation, |_| {
                    Buffer::build(text_buffer, Some(loaded.file), Capability::ReadWrite)
                })
            })
        });

        cx.spawn(async move |this, cx| {
            let buffer = match load_buffer.await {
                Ok(buffer) => Ok(buffer),
                Err(error) if is_not_found_error(&error) => cx.new(|cx| {
                    let buffer_id = BufferId::from(cx.entity_id().as_non_zero_u64());
                    let text_buffer = text::Buffer::new(0, buffer_id, "");
                    Buffer::build(
                        text_buffer,
                        Some(Arc::new(File {
                            worktree,
                            path,
                            disk_state: DiskState::New,
                            entry_id: None,
                            is_local: true,
                            is_private: false,
                        })),
                        Capability::ReadWrite,
                    )
                }),
                Err(e) => Err(e),
            }?;
            this.update(cx, |this, cx| {
                this.add_buffer(buffer.clone(), cx)?;
                let buffer_id = buffer.read(cx).remote_id();
                if let Some(file) = File::from_dyn(buffer.read(cx).file()) {
                    this.path_to_buffer_id.insert(
                        ProjectPath {
                            worktree_id: file.worktree_id(cx),
                            path: file.path.clone(),
                        },
                        buffer_id,
                    );
                    if let Some(entry_id) = file.entry_id {
                        this.state
                            .local_buffer_ids_by_entry_id
                            .insert(entry_id, buffer_id);
                    }
                }

                anyhow::Ok(())
            })??;

            Ok(buffer)
        })
    }

    fn create_buffer(&self, cx: &mut Context<BufferStore>) -> Task<Result<Entity<Buffer>>> {
        cx.spawn(async move |buffer_store, cx| {
            let buffer =
                cx.new(|cx| Buffer::local("", cx).with_language(language::PLAIN_TEXT.clone(), cx))?;
            buffer_store.update(cx, |buffer_store, cx| {
                buffer_store.add_buffer(buffer.clone(), cx).log_err();
            })?;
            Ok(buffer)
        })
    }

    fn reload_buffers(
        &self,
        buffers: HashSet<Entity<Buffer>>,
        push_to_history: bool,
        cx: &mut Context<BufferStore>,
    ) -> Task<Result<ProjectTransaction>> {
        cx.spawn(async move |_, cx| {
            let mut project_transaction = ProjectTransaction::default();
            for buffer in buffers {
                let transaction = buffer.update(cx, |buffer, cx| buffer.reload(cx))?.await?;
                buffer.update(cx, |buffer, cx| {
                    if let Some(transaction) = transaction {
                        if !push_to_history {
                            buffer.forget_transaction(transaction.id);
                        }
                        project_transaction.0.insert(cx.entity(), transaction);
                    }
                })?;
            }

            Ok(project_transaction)
        })
    }
}

impl BufferStore {
    /// Creates a buffer store, optionally retaining its buffers.
    pub fn local(worktree_store: Entity<WorktreeStore>, cx: &mut Context<Self>) -> Self {
        Self {
            state: LocalBufferStore {
                local_buffer_ids_by_entry_id: Default::default(),
                worktree_store: worktree_store.clone(),
                _subscription: cx.subscribe(&worktree_store, |this, _, event, cx| {
                    if let WorktreeStoreEvent::WorktreeAdded(worktree) = event {
                        this.state.subscribe_to_worktree(worktree, cx);
                    }
                }),
            },
            opened_buffers: Default::default(),
            path_to_buffer_id: Default::default(),
            loading_buffers: Default::default(),
            worktree_store,
        }
    }

    pub fn open_buffer(
        &mut self,
        project_path: ProjectPath,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Buffer>>> {
        if let Some(buffer) = self.get_by_path(&project_path, cx) {
            cx.emit(BufferStoreEvent::BufferOpened {
                buffer: buffer.clone(),
                project_path,
            });

            return Task::ready(Ok(buffer));
        }

        let task = match self.loading_buffers.entry(project_path.clone()) {
            hash_map::Entry::Occupied(e) => e.get().clone(),
            hash_map::Entry::Vacant(entry) => {
                let path = project_path.path.clone();
                let Some(worktree) = self
                    .worktree_store
                    .read(cx)
                    .worktree_for_id(project_path.worktree_id, cx)
                else {
                    return Task::ready(Err(anyhow!("no such worktree")));
                };
                let load_buffer = self.state.open_buffer(path, worktree, cx);

                entry
                    .insert(
                        cx.spawn(async move |this, cx| {
                            let load_result = load_buffer.await;
                            this.update(cx, |this, cx| {
                                // Record the fact that the buffer is no longer loading.
                                this.loading_buffers.remove(&project_path);

                                let buffer = load_result.map_err(Arc::new)?;
                                cx.emit(BufferStoreEvent::BufferOpened {
                                    buffer: buffer.clone(),
                                    project_path,
                                });

                                Ok(buffer)
                            })?
                        })
                        .shared(),
                    )
                    .clone()
            }
        };

        cx.background_spawn(async move { task.await.map_err(|e| anyhow!("{e}")) })
    }

    pub fn create_buffer(&mut self, cx: &mut Context<Self>) -> Task<Result<Entity<Buffer>>> {
        self.state.create_buffer(cx)
    }

    pub fn save_buffer(
        &mut self,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.state.save_buffer(buffer, cx)
    }

    pub fn save_buffer_as(
        &mut self,
        buffer: Entity<Buffer>,
        path: ProjectPath,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let old_file = buffer.read(cx).file().cloned();
        let task = self.state.save_buffer_as(buffer.clone(), path, cx);
        cx.spawn(async move |this, cx| {
            task.await?;
            this.update(cx, |_, cx| {
                cx.emit(BufferStoreEvent::BufferChangedFilePath { buffer, old_file });
            })
        })
    }

    fn add_buffer(&mut self, buffer_entity: Entity<Buffer>, cx: &mut Context<Self>) -> Result<()> {
        let buffer = buffer_entity.read(cx);
        let remote_id = buffer.remote_id();
        let path = File::from_dyn(buffer.file()).map(|file| ProjectPath {
            path: file.path.clone(),
            worktree_id: file.worktree_id(cx),
        });

        let handle = cx.entity().downgrade();
        buffer_entity.update(cx, move |_, cx| {
            cx.on_release(move |buffer, cx| {
                handle
                    .update(cx, |_, cx| {
                        cx.emit(BufferStoreEvent::BufferDropped(buffer.remote_id()))
                    })
                    .ok();
            })
            .detach()
        });

        match self.opened_buffers.entry(remote_id) {
            hash_map::Entry::Vacant(entry) => {
                entry.insert(buffer_entity.downgrade());
            }
            hash_map::Entry::Occupied(mut entry) => {
                if entry.get().upgrade().is_some() {
                    debug_panic!("buffer {remote_id} was already registered");
                    anyhow::bail!("buffer {remote_id} was already registered");
                }
                entry.insert(buffer_entity.downgrade());
            }
        };

        if let Some(path) = path {
            self.path_to_buffer_id.insert(path, remote_id);
        }

        cx.subscribe(&buffer_entity, Self::on_buffer_event).detach();
        cx.emit(BufferStoreEvent::BufferAdded(buffer_entity));
        Ok(())
    }

    pub fn buffers(&self) -> impl '_ + Iterator<Item = Entity<Buffer>> {
        self.opened_buffers
            .values()
            .filter_map(|buffer| buffer.upgrade())
    }

    pub fn loading_buffers(
        &self,
    ) -> impl Iterator<Item = (&ProjectPath, impl Future<Output = Result<Entity<Buffer>>>)> {
        self.loading_buffers.iter().map(|(path, task)| {
            let task = task.clone();
            (path, async move { task.await.map_err(|e| anyhow!("{e}")) })
        })
    }

    pub fn buffer_id_for_project_path(&self, project_path: &ProjectPath) -> Option<&BufferId> {
        self.path_to_buffer_id.get(project_path)
    }

    pub fn get_by_path(&self, path: &ProjectPath, _cx: &App) -> Option<Entity<Buffer>> {
        self.path_to_buffer_id.get(path).and_then(|buffer_id| {
            let buffer = self.get(*buffer_id);
            buffer
        })
    }

    pub fn get(&self, buffer_id: BufferId) -> Option<Entity<Buffer>> {
        self.opened_buffers.get(&buffer_id)?.upgrade()
    }

    pub fn get_existing(&self, buffer_id: BufferId) -> Result<Entity<Buffer>> {
        self.get(buffer_id)
            .with_context(|| format!("unknown buffer id {buffer_id}"))
    }

    fn buffer_changed_file(&mut self, buffer: Entity<Buffer>, cx: &mut App) -> Option<()> {
        let file = File::from_dyn(buffer.read(cx).file())?;

        let remote_id = buffer.read(cx).remote_id();
        if let Some(entry_id) = file.entry_id {
            if self
                .state
                .local_buffer_ids_by_entry_id
                .contains_key(&entry_id)
            {
                return None;
            }

            self.state
                .local_buffer_ids_by_entry_id
                .insert(entry_id, remote_id);

            self.path_to_buffer_id.insert(
                ProjectPath {
                    worktree_id: file.worktree_id(cx),
                    path: file.path.clone(),
                },
                remote_id,
            );
        };

        Some(())
    }

    pub fn find_search_candidates(
        &mut self,
        query: &SearchQuery,
        mut limit: usize,
        fs: Arc<dyn Fs>,
        cx: &mut Context<Self>,
    ) -> Receiver<Entity<Buffer>> {
        let (tx, rx) = smol::channel::unbounded();
        let mut open_buffers = HashSet::default();
        let mut unnamed_buffers = Vec::new();
        for handle in self.buffers() {
            let buffer = handle.read(cx);
            if let Some(entry_id) = buffer.entry_id(cx) {
                open_buffers.insert(entry_id);
            } else {
                limit = limit.saturating_sub(1);
                unnamed_buffers.push(handle)
            };
        }

        const MAX_CONCURRENT_BUFFER_OPENS: usize = 64;
        let project_paths_rx = self
            .worktree_store
            .update(cx, |worktree_store, cx| {
                worktree_store.find_search_candidates(query.clone(), limit, open_buffers, fs, cx)
            })
            .chunks(MAX_CONCURRENT_BUFFER_OPENS);

        cx.spawn(async move |this, cx| {
            for buffer in unnamed_buffers {
                tx.send(buffer).await.ok();
            }

            let mut project_paths_rx = pin!(project_paths_rx);
            while let Some(project_paths) = project_paths_rx.next().await {
                let buffers = this.update(cx, |this, cx| {
                    project_paths
                        .into_iter()
                        .map(|project_path| this.open_buffer(project_path, cx))
                        .collect::<Vec<_>>()
                })?;
                for buffer_task in buffers {
                    if let Some(buffer) = buffer_task.await.log_err() {
                        if tx.send(buffer).await.is_err() {
                            return anyhow::Ok(());
                        }
                    }
                }
            }
            anyhow::Ok(())
        })
        .detach();
        rx
    }

    fn on_buffer_event(
        &mut self,
        buffer: Entity<Buffer>,
        event: &BufferEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BufferEvent::FileHandleChanged => {
                self.buffer_changed_file(buffer, cx);
            }
            BufferEvent::LanguageChanged => {}
            _ => {}
        }
    }

    pub fn reload_buffers(
        &self,
        buffers: HashSet<Entity<Buffer>>,
        push_to_history: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<ProjectTransaction>> {
        if buffers.is_empty() {
            return Task::ready(Ok(ProjectTransaction::default()));
        }
        self.state.reload_buffers(buffers, push_to_history, cx)
    }

    pub fn create_local_buffer(
        &mut self,
        text: &str,
        language: Option<Arc<Language>>,
        cx: &mut Context<Self>,
    ) -> Entity<Buffer> {
        let buffer = cx.new(|cx| {
            Buffer::local(text, cx)
                .with_language(language.unwrap_or_else(|| language::PLAIN_TEXT.clone()), cx)
        });

        self.add_buffer(buffer.clone(), cx).log_err();
        let buffer_id = buffer.read(cx).remote_id();

        if let Some(file) = File::from_dyn(buffer.read(cx).file()) {
            self.path_to_buffer_id.insert(
                ProjectPath {
                    worktree_id: file.worktree_id(cx),
                    path: file.path.clone(),
                },
                buffer_id,
            );
            if let Some(entry_id) = file.entry_id {
                self.state
                    .local_buffer_ids_by_entry_id
                    .insert(entry_id, buffer_id);
            }
        }
        buffer
    }

}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    error
        .root_cause()
        .downcast_ref::<io::Error>()
        .is_some_and(|err| err.kind() == io::ErrorKind::NotFound)
}
