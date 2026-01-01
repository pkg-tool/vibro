use std::sync::Arc;

use anyhow::Result;
use gpui::{App, AppContext as _, Context, Entity, Global, Task, Window, actions};
use release_channel::{AppCommitSha, AppVersion};
use semver::Version;
use workspace::Workspace;

actions!(
    auto_update,
    [
        /// Checks for available updates.
        Check,
        /// Dismisses the update error message.
        DismissMessage,
        /// Opens the release notes for the current version in a browser.
        ViewReleaseNotes,
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionCheckType {
    Sha(AppCommitSha),
    Semantic(Version),
}

#[derive(Clone, Debug)]
pub enum AutoUpdateStatus {
    Idle,
    Checking,
    Downloading { version: VersionCheckType },
    Installing { version: VersionCheckType },
    Updated { version: VersionCheckType },
    Errored { error: Arc<anyhow::Error> },
}

impl PartialEq for AutoUpdateStatus {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AutoUpdateStatus::Idle, AutoUpdateStatus::Idle) => true,
            (AutoUpdateStatus::Checking, AutoUpdateStatus::Checking) => true,
            (
                AutoUpdateStatus::Downloading { version: v1 },
                AutoUpdateStatus::Downloading { version: v2 },
            ) => v1 == v2,
            (
                AutoUpdateStatus::Installing { version: v1 },
                AutoUpdateStatus::Installing { version: v2 },
            ) => v1 == v2,
            (
                AutoUpdateStatus::Updated { version: v1 },
                AutoUpdateStatus::Updated { version: v2 },
            ) => v1 == v2,
            (AutoUpdateStatus::Errored { error: e1 }, AutoUpdateStatus::Errored { error: e2 }) => {
                e1.to_string() == e2.to_string()
            }
            _ => false,
        }
    }
}

impl AutoUpdateStatus {
    pub fn is_updated(&self) -> bool {
        matches!(self, Self::Updated { .. })
    }
}

pub struct AutoUpdater {
    status: AutoUpdateStatus,
    current_version: Version,
}

#[derive(Default)]
struct GlobalAutoUpdate(Option<Entity<AutoUpdater>>);

impl Global for GlobalAutoUpdate {}

/// Offline build: auto-updates are disabled (no network).
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|_, action, window, cx| check(action, window, cx));
        workspace.register_action(|_, action, _, cx| {
            view_release_notes(action, cx);
        });
    })
    .detach();

    let version = AppVersion::global(cx);
    let auto_updater = cx.new(|_| AutoUpdater {
        status: AutoUpdateStatus::Idle,
        current_version: version,
    });
    cx.set_global(GlobalAutoUpdate(Some(auto_updater)));
}

pub fn check(_: &Check, window: &mut Window, cx: &mut App) {
    drop(window.prompt(
        gpui::PromptLevel::Info,
        "Auto-updates are disabled",
        Some("Vector is running in offline mode."),
        &["Ok"],
        cx,
    ));
}

pub fn view_release_notes(_: &ViewReleaseNotes, _: &mut App) -> Option<()> {
    None
}

impl AutoUpdater {
    pub fn get(cx: &mut App) -> Option<Entity<Self>> {
        cx.default_global::<GlobalAutoUpdate>().0.clone()
    }

    pub fn current_version(&self) -> Version {
        self.current_version.clone()
    }

    pub fn status(&self) -> AutoUpdateStatus {
        self.status.clone()
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if let AutoUpdateStatus::Idle = self.status {
            return false;
        }
        self.status = AutoUpdateStatus::Idle;
        cx.notify();
        true
    }

    pub fn set_should_show_update_notification(
        &self,
        _should_show: bool,
        _cx: &App,
    ) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    pub fn should_show_update_notification(&self, _cx: &App) -> Task<Result<bool>> {
        Task::ready(Ok(false))
    }
}
