use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Context as _;
use collections::HashMap;
use fs::Fs;
use gpui::{App, Context, Entity, EventEmitter, Task};
use language::{
    ContextLocation, ContextProvider as _, LanguageToolchainStore, Location,
};
use settings::{InvalidSettingsError, SettingsLocation};
use task::{TaskContext, TaskVariables};
use util::ResultExt;
use worktree::File;

use crate::{
    BasicContextProvider, Inventory, ProjectEnvironment,
    worktree_store::WorktreeStore,
};

// platform-dependent warning
pub enum TaskStore {
    Functional(StoreState),
    Noop,
}

pub struct StoreState {
    environment: Entity<ProjectEnvironment>,
    task_inventory: Entity<Inventory>,
    worktree_store: Entity<WorktreeStore>,
    toolchain_store: Arc<dyn LanguageToolchainStore>,
}

impl EventEmitter<crate::Event> for TaskStore {}

#[derive(Debug)]
pub enum TaskSettingsLocation<'a> {
    Global(&'a Path),
    Worktree(SettingsLocation<'a>),
}

impl TaskStore {
    pub fn local(
        worktree_store: Entity<WorktreeStore>,
        toolchain_store: Arc<dyn LanguageToolchainStore>,
        environment: Entity<ProjectEnvironment>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::Functional(StoreState {
            environment,
            task_inventory: Inventory::new(cx),
            toolchain_store,
            worktree_store,
        })
    }

    pub fn task_context_for_location(
        &self,
        captured_variables: TaskVariables,
        location: Location,
        cx: &mut App,
    ) -> Task<Option<TaskContext>> {
        match self {
            TaskStore::Functional(state) => local_task_context_for_location(
                state.worktree_store.clone(),
                state.toolchain_store.clone(),
                state.environment.clone(),
                captured_variables,
                location,
                cx,
            ),
            TaskStore::Noop => Task::ready(None),
        }
    }

    pub fn task_inventory(&self) -> Option<&Entity<Inventory>> {
        match self {
            TaskStore::Functional(state) => Some(&state.task_inventory),
            TaskStore::Noop => None,
        }
    }

    pub(super) fn update_user_tasks(
        &self,
        location: TaskSettingsLocation<'_>,
        raw_tasks_json: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<(), InvalidSettingsError> {
        let task_inventory = match self {
            TaskStore::Functional(state) => &state.task_inventory,
            TaskStore::Noop => return Ok(()),
        };
        let raw_tasks_json = raw_tasks_json
            .map(|json| json.trim())
            .filter(|json| !json.is_empty());

        task_inventory.update(cx, |inventory, _| {
            inventory.update_file_based_tasks(location, raw_tasks_json)
        })
    }

    pub(super) fn update_user_debug_scenarios(
        &self,
        location: TaskSettingsLocation<'_>,
        raw_tasks_json: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<(), InvalidSettingsError> {
        let task_inventory = match self {
            TaskStore::Functional(state) => &state.task_inventory,
            TaskStore::Noop => return Ok(()),
        };
        let raw_tasks_json = raw_tasks_json
            .map(|json| json.trim())
            .filter(|json| !json.is_empty());

        task_inventory.update(cx, |inventory, _| {
            inventory.update_file_based_scenarios(location, raw_tasks_json)
        })
    }
}

fn local_task_context_for_location(
    worktree_store: Entity<WorktreeStore>,
    toolchain_store: Arc<dyn LanguageToolchainStore>,
    environment: Entity<ProjectEnvironment>,
    captured_variables: TaskVariables,
    location: Location,
    cx: &App,
) -> Task<Option<TaskContext>> {
    let worktree_id = location.buffer.read(cx).file().map(|f| f.worktree_id(cx));
    let worktree_abs_path = worktree_id
        .and_then(|worktree_id| worktree_store.read(cx).worktree_for_id(worktree_id, cx))
        .and_then(|worktree| worktree.read(cx).root_dir());
    let fs = worktree_store.read(cx).fs();

    cx.spawn(async move |cx| {
        let project_env = environment
            .update(cx, |environment, cx| {
                environment.get_buffer_environment(&location.buffer, &worktree_store, cx)
            })
            .ok()?
            .await;

        let mut task_variables = cx
            .update(|cx| {
                combine_task_variables(
                    captured_variables,
                    Some(fs),
                    worktree_store.clone(),
                    location,
                    project_env.clone(),
                    BasicContextProvider::new(worktree_store),
                    toolchain_store,
                    cx,
                )
            })
            .ok()?
            .await
            .log_err()?;
        // Remove all custom entries starting with _, as they're not intended for use by the end user.
        task_variables.sweep();

        Some(TaskContext {
            project_env: project_env.unwrap_or_default(),
            cwd: worktree_abs_path.map(|p| p.to_path_buf()),
            task_variables,
        })
    })
}

fn combine_task_variables(
    mut captured_variables: TaskVariables,
    fs: Option<Arc<dyn Fs>>,
    worktree_store: Entity<WorktreeStore>,
    location: Location,
    project_env: Option<HashMap<String, String>>,
    baseline: BasicContextProvider,
    toolchain_store: Arc<dyn LanguageToolchainStore>,
    cx: &mut App,
) -> Task<anyhow::Result<TaskVariables>> {
    let language_context_provider = location
        .buffer
        .read(cx)
        .language()
        .and_then(|language| language.context_provider());
    cx.spawn(async move |cx| {
        let baseline = cx
            .update(|cx| {
                let worktree_root = worktree_root(&worktree_store, &location, cx);
                baseline.build_context(
                    &captured_variables,
                    ContextLocation {
                        fs: fs.clone(),
                        worktree_root,
                        file_location: &location,
                    },
                    project_env.clone(),
                    toolchain_store.clone(),
                    cx,
                )
            })?
            .await
            .context("building basic default context")?;
        captured_variables.extend(baseline);
        if let Some(provider) = language_context_provider {
            captured_variables.extend(
                cx.update(|cx| {
                    let worktree_root = worktree_root(&worktree_store, &location, cx);
                    provider.build_context(
                        &captured_variables,
                        ContextLocation {
                            fs,
                            worktree_root,
                            file_location: &location,
                        },
                        project_env,
                        toolchain_store,
                        cx,
                    )
                })?
                .await
                .context("building provider context")?,
            );
        }
        Ok(captured_variables)
    })
}

fn worktree_root(
    worktree_store: &Entity<WorktreeStore>,
    location: &Location,
    cx: &App,
) -> Option<PathBuf> {
    File::from_dyn(location.buffer.read(cx).file())
        .map(|file| file.worktree.read(cx).abs_path().to_path_buf())
        .or_else(|| {
            worktree_store
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
        })
}
