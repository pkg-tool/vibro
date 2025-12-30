use std::{
    path::Path,
    sync::Arc,
};

use async_trait::async_trait;
use collections::BTreeMap;
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Subscription, Task, WeakEntity,
};
use language::{LanguageName, LanguageRegistry, LanguageToolchainStore, Toolchain, ToolchainList};
use settings::WorktreeId;

use crate::{ProjectEnvironment, ProjectPath, worktree_store::WorktreeStore};

pub struct ToolchainStore(ToolchainStoreInner);
enum ToolchainStoreInner {
    Local(
        Entity<LocalToolchainStore>,
        #[allow(dead_code)] Subscription,
    ),
}

impl EventEmitter<ToolchainStoreEvent> for ToolchainStore {}
impl ToolchainStore {
    pub fn local(
        languages: Arc<LanguageRegistry>,
        worktree_store: Entity<WorktreeStore>,
        project_environment: Entity<ProjectEnvironment>,
        cx: &mut Context<Self>,
    ) -> Self {
        let entity = cx.new(|_| LocalToolchainStore {
            languages,
            worktree_store,
            project_environment,
            active_toolchains: Default::default(),
        });
        let subscription = cx.subscribe(&entity, |_, _, e: &ToolchainStoreEvent, cx| {
            cx.emit(e.clone())
        });
        Self(ToolchainStoreInner::Local(entity, subscription))
    }

    pub(crate) fn activate_toolchain(
        &self,
        path: ProjectPath,
        toolchain: Toolchain,
        cx: &mut App,
    ) -> Task<Option<()>> {
        match &self.0 {
            ToolchainStoreInner::Local(local, _) => {
                local.update(cx, |this, cx| this.activate_toolchain(path, toolchain, cx))
            }
        }
    }
    pub(crate) fn list_toolchains(
        &self,
        path: ProjectPath,
        language_name: LanguageName,
        cx: &App,
    ) -> Task<Option<ToolchainList>> {
        match &self.0 {
            ToolchainStoreInner::Local(local, _) => {
                local.read(cx).list_toolchains(path, language_name, cx)
            }
        }
    }
    pub(crate) fn active_toolchain(
        &self,
        path: ProjectPath,
        language_name: LanguageName,
        cx: &App,
    ) -> Task<Option<Toolchain>> {
        match &self.0 {
            ToolchainStoreInner::Local(local, _) => {
                local.read(cx).active_toolchain(path, language_name, cx)
            }
        }
    }
    pub fn as_language_toolchain_store(&self) -> Arc<dyn LanguageToolchainStore> {
        match &self.0 {
            ToolchainStoreInner::Local(local, _) => Arc::new(LocalStore(local.downgrade())),
        }
    }
}

struct LocalToolchainStore {
    languages: Arc<LanguageRegistry>,
    worktree_store: Entity<WorktreeStore>,
    project_environment: Entity<ProjectEnvironment>,
    active_toolchains: BTreeMap<(WorktreeId, LanguageName), BTreeMap<Arc<Path>, Toolchain>>,
}

#[async_trait(?Send)]
impl language::LanguageToolchainStore for LocalStore {
    async fn active_toolchain(
        self: Arc<Self>,
        worktree_id: WorktreeId,
        path: Arc<Path>,
        language_name: LanguageName,
        cx: &mut AsyncApp,
    ) -> Option<Toolchain> {
        self.0
            .update(cx, |this, cx| {
                this.active_toolchain(ProjectPath { worktree_id, path }, language_name, cx)
            })
            .ok()?
            .await
    }
}

pub(crate) struct EmptyToolchainStore;
#[async_trait(?Send)]
impl language::LanguageToolchainStore for EmptyToolchainStore {
    async fn active_toolchain(
        self: Arc<Self>,
        _: WorktreeId,
        _: Arc<Path>,
        _: LanguageName,
        _: &mut AsyncApp,
    ) -> Option<Toolchain> {
        None
    }
}
struct LocalStore(WeakEntity<LocalToolchainStore>);

#[derive(Clone)]
pub enum ToolchainStoreEvent {
    ToolchainActivated,
}

impl EventEmitter<ToolchainStoreEvent> for LocalToolchainStore {}

impl LocalToolchainStore {
    pub(crate) fn activate_toolchain(
        &self,
        path: ProjectPath,
        toolchain: Toolchain,
        cx: &mut Context<Self>,
    ) -> Task<Option<()>> {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| {
                this.active_toolchains
                    .entry((path.worktree_id, toolchain.language_name.clone()))
                    .or_default()
                    .insert(path.path, toolchain.clone());
                cx.emit(ToolchainStoreEvent::ToolchainActivated);
            })
            .ok();
            Some(())
        })
    }
    pub(crate) fn list_toolchains(
        &self,
        path: ProjectPath,
        language_name: LanguageName,
        cx: &App,
    ) -> Task<Option<ToolchainList>> {
        let registry = self.languages.clone();
        let Some(abs_path) = self
            .worktree_store
            .read(cx)
            .worktree_for_id(path.worktree_id, cx)
            .map(|worktree| worktree.read(cx).abs_path())
        else {
            return Task::ready(None);
        };
        let environment = self.project_environment.clone();
        cx.spawn(async move |cx| {
            let project_env = environment
                .update(cx, |environment, cx| {
                    environment.get_directory_environment(abs_path.clone(), cx)
                })
                .ok()?
                .await;

            cx.background_spawn(async move {
                let language = registry
                    .language_for_name(language_name.as_ref())
                    .await
                    .ok()?;
                let toolchains = language.toolchain_lister()?;
                Some(toolchains.list(abs_path.to_path_buf(), project_env).await)
            })
            .await
        })
    }
    pub(crate) fn active_toolchain(
        &self,
        path: ProjectPath,
        language_name: LanguageName,
        _: &App,
    ) -> Task<Option<Toolchain>> {
        let ancestors = path.path.ancestors();
        Task::ready(
            self.active_toolchains
                .get(&(path.worktree_id, language_name))
                .and_then(|paths| {
                    ancestors
                        .into_iter()
                        .find_map(|root_path| paths.get(root_path))
                })
                .cloned(),
        )
    }
}
// Proto conversions were used for remote/collab plumbing and are intentionally removed.
