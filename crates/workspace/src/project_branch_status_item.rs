use crate::{StatusItemView, TitleBarSettings, Workspace};
use gpui::{Action, App, Context, Entity, Render, Subscription, WeakEntity, Window};
use project::Project;
use settings::{Settings as _, SettingsStore};
use ui::{
    Button, ButtonStyle, Color, Divider, DividerColor, IconName, IconPosition, LabelSize, Tooltip,
    div, h_flex, prelude::*,
};
use util::truncate_and_trailoff;

const MAX_BRANCH_NAME_LENGTH: usize = 40;
const MAX_SHORT_SHA_LENGTH: usize = 8;

pub struct ProjectBranchStatusItem {
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectBranchStatusItem {
    pub fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global::<SettingsStore>(|_, cx| cx.notify()));
        subscriptions.push(cx.subscribe(&project, |_, _, _: &project::Event, cx| cx.notify()));

        Self {
            project,
            workspace,
            _subscriptions: subscriptions,
        }
    }

    fn branch_name(&self, cx: &App) -> Option<String> {
        let repository = self.project.read(cx).active_repository(cx)?;
        let repo = repository.read(cx);
        repo.branch
            .as_ref()
            .map(|branch| branch.name())
            .map(|name| truncate_and_trailoff(&name, MAX_BRANCH_NAME_LENGTH))
            .or_else(|| {
                repo.head_commit.as_ref().map(|commit| {
                    commit
                        .sha
                        .chars()
                        .take(MAX_SHORT_SHA_LENGTH)
                        .collect::<String>()
                })
            })
    }
}

impl Render for ProjectBranchStatusItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar_settings = *TitleBarSettings::get_global(cx);
        if !title_bar_settings.show_branch_name {
            return div().hidden();
        }

        let Some(branch_name) = self.branch_name(cx) else {
            return div().hidden();
        };

        let workspace = self.workspace.clone();
        h_flex()
            .gap_1()
            .items_center()
            .child(Divider::vertical().color(DividerColor::Border))
            .child(
                Button::new("project_branch_trigger", branch_name)
                    .color(Color::Muted)
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .tooltip(move |_window, cx| {
                        Tooltip::with_meta(
                            "Recent Branches",
                            Some(&vector_actions::git::Branch),
                            "Local branches only",
                            cx,
                        )
                    })
                    .on_click(move |_, window, cx| {
                        let Some(workspace) = workspace.upgrade() else {
                            return;
                        };

                        let _ = workspace.update(cx, |_this, cx| {
                            window.dispatch_action(vector_actions::git::Branch.boxed_clone(), cx);
                        });
                    })
                    .when(title_bar_settings.show_branch_icon, |branch_button| {
                        branch_button
                            .icon(IconName::GitBranch)
                            .icon_position(IconPosition::Start)
                            .icon_color(Color::Muted)
                    }),
            )
    }
}

impl StatusItemView for ProjectBranchStatusItem {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn crate::ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Git branch state is derived from project state, independent of the active pane item.
    }
}
