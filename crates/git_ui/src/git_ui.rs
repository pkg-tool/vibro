use std::any::Any;

use command_palette_hooks::CommandPaletteFilter;
use commit_modal::CommitModal;
use editor::{Editor, actions::DiffClipboardWithSelectionData};
use project::ProjectPath;
use ui::{
    Headline, HeadlineSize, Icon, IconName, IconSize, IntoElement, ParentElement, Render, Styled,
    StyledExt, div, h_flex, rems, v_flex,
};

mod blame_ui;
pub mod clone;

use git::status::{FileStatus, StatusCode, UnmergedStatus, UnmergedStatusCode};
use gpui::{
    Action, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Window, actions,
};
use menu::{Cancel, Confirm};
use onboarding::GitOnboardingModal;
use project::git_store::Repository;
use project_diff::ProjectDiff;
use ui::prelude::*;
use workspace::{ModalView, Workspace, notifications::DetachAndPromptErr};

use crate::text_diff_view::TextDiffView;

mod askpass_modal;
pub mod branch_picker;
mod commit_modal;
pub mod commit_tooltip;
pub mod commit_view;
mod conflict_view;
pub mod file_diff_view;
pub mod file_history_view;
pub mod git_panel;
mod git_panel_settings;
pub mod onboarding;
pub mod picker_prompt;
pub mod project_diff;
pub mod repository_selector;
pub mod stash_picker;
pub mod text_diff_view;

actions!(
    git,
    [
        /// Resets the git onboarding state to show the tutorial again.
        ResetOnboarding
    ]
);

pub fn init(cx: &mut App) {
    editor::set_blame_renderer(blame_ui::GitBlameRenderer, cx);
    commit_view::init(cx);
    file_history_view::init(cx);

    cx.observe_new(|editor: &mut Editor, _, cx| {
        conflict_view::register_editor(editor, editor.buffer().clone(), cx);
    })
    .detach();

    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        ProjectDiff::register(workspace, cx);
        CommitModal::register(workspace);
        git_panel::register(workspace);
        repository_selector::register(workspace);
        branch_picker::register(workspace);
        stash_picker::register(workspace);

        let project = workspace.project().read(cx);
        if project.is_read_only(cx) {
            return;
        }
        workspace.register_action(|workspace, action: &git::StashAll, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.stash_all(action, window, cx);
            });
        });
        workspace.register_action(|workspace, action: &git::StashPop, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.stash_pop(action, window, cx);
            });
        });
        workspace.register_action(|workspace, action: &git::StashApply, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.stash_apply(action, window, cx);
            });
        });
        workspace.register_action(|workspace, action: &git::StageAll, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.stage_all(action, window, cx);
            });
        });
        workspace.register_action(|workspace, action: &git::UnstageAll, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.unstage_all(action, window, cx);
            });
        });
        workspace.register_action(|workspace, _: &git::Uncommit, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.uncommit(window, cx);
            })
        });
        CommandPaletteFilter::update_global(cx, |filter, _cx| {
            filter.hide_action_types(&[
                vector_actions::OpenGitIntegrationOnboarding.type_id(),
                // ResetOnboarding.type_id(),
            ]);
        });
        workspace.register_action(
            move |workspace, _: &vector_actions::OpenGitIntegrationOnboarding, window, cx| {
                GitOnboardingModal::toggle(workspace, window, cx)
            },
        );
        workspace.register_action(move |_, _: &ResetOnboarding, window, cx| {
            window.dispatch_action(workspace::RestoreBanner.boxed_clone(), cx);
            window.refresh();
        });
        workspace.register_action(|workspace, _action: &git::Init, window, cx| {
            let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
                return;
            };
            panel.update(cx, |panel, cx| {
                panel.git_init(window, cx);
            });
        });
        workspace.register_action(|workspace, _: &git::OpenModifiedFiles, window, cx| {
            open_modified_files(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &git::RenameBranch, window, cx| {
            rename_current_branch(workspace, window, cx);
        });
        workspace.register_action(
            |workspace, action: &DiffClipboardWithSelectionData, window, cx| {
                if let Some(task) = TextDiffView::open(action, workspace, window, cx) {
                    task.detach();
                };
            },
        );
        workspace.register_action(|workspace, _: &git::FileHistory, window, cx| {
            let Some(active_item) = workspace.active_item(cx) else {
                return;
            };
            let Some(editor) = active_item.downcast::<Editor>() else {
                return;
            };
            let Some(buffer) = editor.read(cx).buffer().read(cx).as_singleton() else {
                return;
            };
            let Some(file) = buffer.read(cx).file() else {
                return;
            };
            let worktree_id = file.worktree_id(cx);
            let project_path = ProjectPath {
                worktree_id,
                path: file.path().clone(),
            };
            let project = workspace.project();
            let git_store = project.read(cx).git_store();
            let Some((repo, repo_path)) = git_store
                .read(cx)
                .repository_and_path_for_project_path(&project_path, cx)
            else {
                return;
            };
            file_history_view::FileHistoryView::open(
                repo_path,
                git_store.downgrade(),
                repo.downgrade(),
                workspace.weak_handle(),
                window,
                cx,
            );
        });
    })
    .detach();
}

fn open_modified_files(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
        return;
    };
    let modified_paths: Vec<_> = panel.update(cx, |panel, cx| {
        let Some(repo) = panel.active_repository.as_ref() else {
            return Vec::new();
        };
        let repo = repo.read(cx);
        repo.cached_status()
            .filter_map(|entry| {
                if entry.status.is_modified() {
                    repo.repo_path_to_project_path(&entry.repo_path, cx)
                } else {
                    None
                }
            })
            .collect()
    });
    for path in modified_paths {
        workspace.open_path(path, None, true, window, cx).detach();
    }
}

pub fn git_status_icon(status: FileStatus) -> impl IntoElement {
    GitStatusIcon::new(status)
}

struct RenameBranchModal {
    current_branch: SharedString,
    editor: Entity<Editor>,
    repo: Entity<Repository>,
}

impl RenameBranchModal {
    fn new(
        current_branch: String,
        repo: Entity<Repository>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(current_branch.clone(), window, cx);
            editor
        });
        Self {
            current_branch: current_branch.into(),
            editor,
            repo,
        }
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let new_name = self.editor.read(cx).text(cx);
        if new_name.is_empty() || new_name == self.current_branch.as_ref() {
            cx.emit(DismissEvent);
            return;
        }

        let repo = self.repo.clone();
        let current_branch = self.current_branch.to_string();
        cx.spawn(async move |_, cx| {
            match repo
                .update(cx, |repo, _| {
                    repo.rename_branch(current_branch, new_name.clone())
                })?
                .await
            {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(anyhow::anyhow!("Operation was canceled")),
            }
        })
        .detach_and_prompt_err("Failed to rename branch", window, cx, |_, _, _| None);
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for RenameBranchModal {}
impl ModalView for RenameBranchModal {}
impl Focusable for RenameBranchModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Render for RenameBranchModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("RenameBranchModal")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .elevation_2(cx)
            .w(rems(34.))
            .child(
                h_flex()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .w_full()
                    .gap_1p5()
                    .child(Icon::new(IconName::GitBranch).size(IconSize::XSmall))
                    .child(
                        Headline::new(format!("Rename Branch ({})", self.current_branch))
                            .size(HeadlineSize::XSmall),
                    ),
            )
            .child(div().px_3().pb_3().w_full().child(self.editor.clone()))
    }
}

fn rename_current_branch(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(panel) = workspace.panel::<git_panel::GitPanel>(cx) else {
        return;
    };
    let current_branch: Option<String> = panel.update(cx, |panel, cx| {
        let repo = panel.active_repository.as_ref()?;
        let repo = repo.read(cx);
        repo.branch.as_ref().map(|branch| branch.name().to_string())
    });

    let Some(current_branch_name) = current_branch else {
        return;
    };

    let repo = panel.read(cx).active_repository.clone();
    let Some(repo) = repo else {
        return;
    };

    workspace.toggle_modal(window, cx, |window, cx| {
        RenameBranchModal::new(current_branch_name, repo, window, cx)
    });
}

/// A visual representation of a file's Git status.
#[derive(IntoElement, RegisterComponent)]
pub struct GitStatusIcon {
    status: FileStatus,
}

impl GitStatusIcon {
    pub fn new(status: FileStatus) -> Self {
        Self { status }
    }
}

impl RenderOnce for GitStatusIcon {
    fn render(self, _window: &mut ui::Window, cx: &mut App) -> impl IntoElement {
        let status = self.status;

        let (icon_name, color) = if status.is_conflicted() {
            (
                IconName::Warning,
                cx.theme().colors().version_control_conflict,
            )
        } else if status.is_deleted() {
            (
                IconName::SquareMinus,
                cx.theme().colors().version_control_deleted,
            )
        } else if status.is_modified() {
            (
                IconName::SquareDot,
                cx.theme().colors().version_control_modified,
            )
        } else {
            (
                IconName::SquarePlus,
                cx.theme().colors().version_control_added,
            )
        };

        Icon::new(icon_name).color(Color::Custom(color))
    }
}

// View this component preview using `workspace: open component-preview`
impl Component for GitStatusIcon {
    fn scope() -> ComponentScope {
        ComponentScope::VersionControl
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        fn tracked_file_status(code: StatusCode) -> FileStatus {
            FileStatus::Tracked(git::status::TrackedStatus {
                index_status: code,
                worktree_status: code,
            })
        }

        let modified = tracked_file_status(StatusCode::Modified);
        let added = tracked_file_status(StatusCode::Added);
        let deleted = tracked_file_status(StatusCode::Deleted);
        let conflict = UnmergedStatus {
            first_head: UnmergedStatusCode::Updated,
            second_head: UnmergedStatusCode::Updated,
        }
        .into();

        Some(
            v_flex()
                .gap_6()
                .children(vec![example_group(vec![
                    single_example("Modified", GitStatusIcon::new(modified).into_any_element()),
                    single_example("Added", GitStatusIcon::new(added).into_any_element()),
                    single_example("Deleted", GitStatusIcon::new(deleted).into_any_element()),
                    single_example(
                        "Conflicted",
                        GitStatusIcon::new(conflict).into_any_element(),
                    ),
                ])])
                .into_any_element(),
        )
    }
}
