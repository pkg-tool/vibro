use anyhow::{Context as _, Result};
use db::kvp::KEY_VALUE_STORE;
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, Global, Task, Window, actions};
use http_client::{AsyncBody, HttpClient, HttpRequestExt as _, Method, RedirectPolicy, Url};
use release_channel::{AppCommitSha, AppVersion, ReleaseChannel};
use reqwest_client::ReqwestClient;
use semver::Version;
use serde::{Deserialize, Serialize};
use settings::{RegisterSetting, Settings, SettingsStore};
use smol::fs::File;
use smol::io::AsyncReadExt as _;
use std::{
    env::{
        self,
        consts::{ARCH, OS},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use util::command::new_smol_command;
use workspace::Workspace;

const SHOULD_SHOW_UPDATE_NOTIFICATION_KEY: &str = "auto-updater-should-show-updated-notification";
const LAST_SEEN_ASSET_TOKEN_KEY_PREFIX: &str = "auto-updater-last-seen-asset-token";
const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReleaseAsset {
    pub version: String,
    pub url: String,
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
    http_client: Arc<dyn HttpClient>,
    pending_poll: Option<Task<Option<()>>>,
}

#[derive(Default)]
struct GlobalAutoUpdate(Option<Entity<AutoUpdater>>);

impl Global for GlobalAutoUpdate {}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|_, action, window, cx| check(action, window, cx));
        workspace.register_action(|_, action, _, cx| {
            view_release_notes(action, cx);
        });
    })
    .detach();

    let version = AppVersion::global(cx);
    let auto_updater = cx.new(|cx| {
        let updater = AutoUpdater {
            status: AutoUpdateStatus::Idle,
            current_version: version,
            http_client: build_http_client(cx),
            pending_poll: None,
        };

        let mut update_subscription =
            should_poll_for_updates(cx).then(|| updater.start_polling(cx));

        cx.observe_global::<SettingsStore>(move |updater: &mut AutoUpdater, cx| {
            let should_poll = should_poll_for_updates(cx);
            if should_poll {
                if update_subscription.is_none() {
                    update_subscription = Some(updater.start_polling(cx));
                }
            } else {
                update_subscription.take();
            }
        })
        .detach();

        updater
    });
    cx.set_global(GlobalAutoUpdate(Some(auto_updater)));
}

pub fn check(_: &Check, window: &mut Window, cx: &mut App) {
    if let Some(message) = update_explanation() {
        drop(window.prompt(
            gpui::PromptLevel::Info,
            "Vector was installed via a package manager.",
            Some(&message),
            &["Ok"],
            cx,
        ));
        return;
    }

    let Some(_) = configured_update_url(cx) else {
        drop(window.prompt(
            gpui::PromptLevel::Info,
            "Auto-updates are disabled",
            Some("Set `auto_update_url` in settings to enable auto-updates."),
            &["Ok"],
            cx,
        ));
        return;
    };

    if !AutoUpdateSetting::get_global(cx).0 {
        drop(window.prompt(
            gpui::PromptLevel::Info,
            "Auto-updates are disabled",
            Some("Set `auto_update` to true in settings to enable auto-updates."),
            &["Ok"],
            cx,
        ));
        return;
    }

    if let Some(updater) = AutoUpdater::get(cx) {
        updater.update(cx, |updater, cx| updater.poll(UpdateCheckType::Manual, cx));
    }
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

    pub fn start_polling(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            loop {
                this.update(cx, |this, cx| this.poll(UpdateCheckType::Automatic, cx))?;
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        })
    }

    pub fn poll(&mut self, check_type: UpdateCheckType, cx: &mut Context<Self>) {
        if self.pending_poll.is_some() {
            return;
        }

        cx.notify();

        self.pending_poll = Some(cx.spawn(async move |this, cx| {
            let result = Self::update(this.upgrade()?, check_type, cx).await;
            this.update(cx, |this, cx| {
                this.pending_poll = None;
                if let Err(error) = result {
                    this.status = match check_type {
                        UpdateCheckType::Automatic => {
                            log::info!("auto-update check failed: {error:#}");
                            AutoUpdateStatus::Idle
                        }
                        UpdateCheckType::Manual => {
                            log::error!("auto-update failed: {error:#}");
                            AutoUpdateStatus::Errored {
                                error: Arc::new(error),
                            }
                        }
                    };
                    cx.notify();
                }
            })
            .ok()
        }));
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
        should_show: bool,
        cx: &App,
    ) -> Task<Result<()>> {
        cx.background_spawn(async move {
            if should_show {
                KEY_VALUE_STORE
                    .write_kvp(
                        SHOULD_SHOW_UPDATE_NOTIFICATION_KEY.to_string(),
                        "".to_string(),
                    )
                    .await?;
            } else {
                KEY_VALUE_STORE
                    .delete_kvp(SHOULD_SHOW_UPDATE_NOTIFICATION_KEY.to_string())
                    .await?;
            }
            Ok(())
        })
    }

    pub fn should_show_update_notification(&self, cx: &App) -> Task<Result<bool>> {
        cx.background_spawn(async move {
            Ok(KEY_VALUE_STORE
                .read_kvp(SHOULD_SHOW_UPDATE_NOTIFICATION_KEY)?
                .is_some())
        })
    }

    async fn update(
        this: Entity<Self>,
        check_type: UpdateCheckType,
        cx: &mut AsyncApp,
    ) -> Result<()> {
        let (
            http_client,
            installed_version,
            previous_status,
            release_channel,
            auto_update_enabled,
            app_commit_sha,
            update_url,
        ) = this.read_with(cx, |this, cx| {
            (
                this.http_client.clone(),
                this.current_version.clone(),
                this.status.clone(),
                ReleaseChannel::try_global(cx).unwrap_or(ReleaseChannel::Stable),
                AutoUpdateSetting::get_global(cx).0,
                AppCommitSha::try_global(cx).map(|sha| sha.full()),
                configured_update_url(cx),
            )
        })?;

        // Only allow self-updates when explicitly configured.
        let Some(update_url) = update_url else {
            return Ok(());
        };

        // Auto-updates can still be disabled via `auto_update=false`.
        if !auto_update_enabled {
            return Ok(());
        }

        Self::check_dependencies()?;

        this.update(cx, |this, cx| {
            this.status = AutoUpdateStatus::Checking;
            cx.notify();
        })?;

        let update_url = expand_template(&update_url, release_channel, &installed_version);

        let (version_check, download_url, new_release_token) =
            if looks_like_manifest_url(&update_url) {
                let manifest_url = Url::parse(&update_url).context("invalid auto_update_url")?;
                let asset = fetch_release_asset(
                    &http_client,
                    &manifest_url,
                    release_channel,
                    &installed_version,
                )
                .await?;

                let version_check = parse_version_check_type(release_channel, &asset.version);

                if !should_download_manifest_update(
                    release_channel,
                    app_commit_sha.as_deref(),
                    &installed_version,
                    &version_check,
                    &previous_status,
                ) {
                    return this.update(cx, |this, cx| {
                        this.status = match previous_status {
                            AutoUpdateStatus::Updated { .. } => previous_status,
                            _ => AutoUpdateStatus::Idle,
                        };
                        cx.notify();
                    });
                }

                let download_url_raw =
                    expand_template(&asset.url, release_channel, &installed_version);
                let download_url = Url::parse(&download_url_raw)
                    .or_else(|_| manifest_url.join(&download_url_raw))
                    .context("invalid download url in update manifest")?;

                (version_check, download_url, None)
            } else {
                let download_url = Url::parse(&update_url).context("invalid auto_update_url")?;
                let token = fetch_artifact_token(&http_client, &download_url).await?;
                let token_key = last_seen_asset_token_key(release_channel);
                if let Some(token) = token.as_ref()
                    && let Some(previous) = KEY_VALUE_STORE.read_kvp(&token_key)?
                    && previous == *token
                {
                    return this.update(cx, |this, cx| {
                        this.status = match previous_status {
                            AutoUpdateStatus::Updated { .. } => previous_status,
                            _ => AutoUpdateStatus::Idle,
                        };
                        cx.notify();
                    });
                }

                if matches!(check_type, UpdateCheckType::Automatic) && token.is_none() {
                    log::info!(
                        "auto-update URL did not provide cache headers; skipping automatic download"
                    );
                    return this.update(cx, |this, cx| {
                        this.status = AutoUpdateStatus::Idle;
                        cx.notify();
                    });
                }

                let version_check = VersionCheckType::Sha(AppCommitSha::new(
                    token.clone().unwrap_or_else(|| "manual".to_string()),
                ));
                (version_check, download_url, token)
            };

        this.update(cx, |this, cx| {
            this.status = AutoUpdateStatus::Downloading {
                version: version_check.clone(),
            };
            cx.notify();
        })?;

        let installer_dir = InstallerDir::new().await?;
        let target_path = target_path(&installer_dir).await?;
        download_release(&target_path, &download_url, http_client.clone()).await?;

        this.update(cx, |this, cx| {
            this.status = AutoUpdateStatus::Installing {
                version: version_check.clone(),
            };
            cx.notify();
        })?;

        let new_binary_path = install_release(installer_dir, target_path, cx).await?;
        if let Some(new_binary_path) = new_binary_path {
            cx.update(|cx| cx.set_restart_path(new_binary_path))?;
        }

        if let Some(token) = new_release_token {
            KEY_VALUE_STORE
                .write_kvp(last_seen_asset_token_key(release_channel), token)
                .await?;
        }

        this.update(cx, |this, cx| {
            this.set_should_show_update_notification(true, cx)
                .detach_and_log_err(cx);
            this.status = AutoUpdateStatus::Updated {
                version: version_check,
            };
            cx.notify();
        })
    }

    fn check_dependencies() -> Result<()> {
        #[cfg(not(target_os = "windows"))]
        anyhow::ensure!(
            which::which("rsync").is_ok(),
            "Could not auto-update because the required rsync utility was not found."
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum UpdateCheckType {
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, RegisterSetting)]
struct AutoUpdateSetting(bool);

impl Settings for AutoUpdateSetting {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self(content.auto_update.unwrap())
    }
}

#[derive(Clone, Debug, RegisterSetting)]
struct AutoUpdateUrlSetting(Option<String>);

impl Settings for AutoUpdateUrlSetting {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self(content.auto_update_url.clone())
    }
}

fn update_explanation() -> Option<String> {
    option_env!("VECTOR_UPDATE_EXPLANATION")
        .map(|s| s.to_string())
        .or_else(|| option_env!("ZED_UPDATE_EXPLANATION").map(|s| s.to_string()))
        .or_else(|| env::var("VECTOR_UPDATE_EXPLANATION").ok())
        .or_else(|| env::var("ZED_UPDATE_EXPLANATION").ok())
}

fn configured_update_url(cx: &App) -> Option<String> {
    AutoUpdateUrlSetting::get_global(cx)
        .0
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn should_poll_for_updates(cx: &App) -> bool {
    if update_explanation().is_some() {
        return false;
    }

    let poll_for_channel = ReleaseChannel::try_global(cx)
        .map(|channel| channel.poll_for_updates())
        .unwrap_or(false);

    poll_for_channel && AutoUpdateSetting::get_global(cx).0 && configured_update_url(cx).is_some()
}

fn build_http_client(cx: &App) -> Arc<dyn HttpClient> {
    let version = AppVersion::global(cx);
    let user_agent = format!("Vector/{} ({}/{})", version, OS, ARCH);
    match ReqwestClient::user_agent(&user_agent) {
        Ok(client) => Arc::new(client),
        Err(error) => {
            log::warn!("Failed to build update HTTP client with custom user-agent: {error:#}");
            Arc::new(ReqwestClient::new())
        }
    }
}

fn looks_like_manifest_url(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.ends_with(".json") || url.ends_with(".jsonc")
}

fn expand_template(
    template: &str,
    release_channel: ReleaseChannel,
    current_version: &Version,
) -> String {
    template
        .replace("{channel}", release_channel.dev_name())
        .replace("{os}", OS)
        .replace("{arch}", ARCH)
        .replace("{version}", &current_version.to_string())
}

fn last_seen_asset_token_key(release_channel: ReleaseChannel) -> String {
    format!(
        "{LAST_SEEN_ASSET_TOKEN_KEY_PREFIX}-{}-{}-{}",
        release_channel.dev_name(),
        OS,
        ARCH
    )
}

fn parse_version_check_type(release_channel: ReleaseChannel, version: &str) -> VersionCheckType {
    if let Ok(semantic) = version.parse::<Version>() {
        if matches!(release_channel, ReleaseChannel::Nightly) {
            let sha = semantic
                .build
                .as_str()
                .rsplit('.')
                .next()
                .unwrap_or(version);
            return VersionCheckType::Sha(AppCommitSha::new(sha.to_string()));
        }

        return VersionCheckType::Semantic(semantic);
    }

    VersionCheckType::Sha(AppCommitSha::new(version.to_string()))
}

fn should_download_manifest_update(
    _release_channel: ReleaseChannel,
    app_commit_sha: Option<&str>,
    installed_version: &Version,
    fetched_version: &VersionCheckType,
    previous_status: &AutoUpdateStatus,
) -> bool {
    if let AutoUpdateStatus::Updated {
        version: cached, ..
    } = previous_status
        && cached == fetched_version
    {
        return false;
    }

    match fetched_version {
        VersionCheckType::Semantic(fetched) => {
            strip_semver_build_pre(fetched.clone())
                > strip_semver_build_pre(installed_version.clone())
        }
        VersionCheckType::Sha(fetched_sha) => {
            if let Some(app_commit_sha) = app_commit_sha
                && fetched_sha.full() == app_commit_sha
            {
                return false;
            }
            true
        }
    }
}

fn strip_semver_build_pre(mut version: Version) -> Version {
    version.build = semver::BuildMetadata::EMPTY;
    version.pre = semver::Prerelease::EMPTY;
    version
}

async fn fetch_release_asset(
    http_client: &Arc<dyn HttpClient>,
    manifest_url: &Url,
    release_channel: ReleaseChannel,
    installed_version: &Version,
) -> Result<ReleaseAsset> {
    let manifest_url = expand_template(manifest_url.as_str(), release_channel, installed_version);
    let mut response = http_client
        .get(&manifest_url, AsyncBody::default(), true)
        .await?;

    let mut body = Vec::new();
    response.body_mut().read_to_end(&mut body).await?;

    anyhow::ensure!(
        response.status().is_success(),
        "failed to fetch update manifest: {:?}",
        String::from_utf8_lossy(&body),
    );

    serde_json::from_slice(body.as_slice()).with_context(|| {
        format!(
            "error deserializing update manifest: {:?}",
            String::from_utf8_lossy(&body),
        )
    })
}

async fn fetch_artifact_token(
    http_client: &Arc<dyn HttpClient>,
    url: &Url,
) -> Result<Option<String>> {
    let request = http_client::Builder::new()
        .method(Method::HEAD)
        .uri(url.as_str())
        .follow_redirects(RedirectPolicy::FollowAll)
        .body(AsyncBody::default())?;

    let response = match http_client.send(request).await {
        Ok(response) => response,
        Err(error) => {
            log::info!("failed to HEAD auto-update URL: {error:#}");
            return Ok(None);
        }
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    let headers = response.headers();
    let token = headers
        .get(http_client::http::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get(http_client::http::header::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            headers
                .get(http_client::http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .map(|s| format!("content-length:{s}"))
        });

    Ok(token)
}

async fn download_release(
    target_path: &Path,
    url: &Url,
    http_client: Arc<dyn HttpClient>,
) -> Result<()> {
    let mut target_file = File::create(target_path).await?;

    let mut response = http_client
        .get(url.as_str(), AsyncBody::default(), true)
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "failed to download update: {:?}",
        response.status()
    );
    smol::io::copy(response.body_mut(), &mut target_file).await?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
struct InstallerDir(tempfile::TempDir);

#[cfg(not(target_os = "windows"))]
impl InstallerDir {
    async fn new() -> Result<Self> {
        Ok(Self(
            tempfile::Builder::new()
                .prefix("vector-auto-update")
                .tempdir()?,
        ))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }
}

#[cfg(target_os = "windows")]
struct InstallerDir(PathBuf);

#[cfg(target_os = "windows")]
impl InstallerDir {
    async fn new() -> Result<Self> {
        let installer_dir = std::env::current_exe()?
            .parent()
            .context("No parent dir for Vector.exe")?
            .join("updates");
        if smol::fs::metadata(&installer_dir).await.is_ok() {
            smol::fs::remove_dir_all(&installer_dir).await?;
        }
        smol::fs::create_dir(&installer_dir).await?;
        Ok(Self(installer_dir))
    }

    fn path(&self) -> &Path {
        self.0.as_path()
    }
}

async fn target_path(installer_dir: &InstallerDir) -> Result<PathBuf> {
    let filename = match OS {
        "macos" => "Vector.dmg",
        "linux" => "vector.tar.gz",
        "windows" => "Vector.exe",
        unsupported_os => anyhow::bail!("not supported: {unsupported_os}"),
    };

    Ok(installer_dir.path().join(filename))
}

async fn install_release(
    installer_dir: InstallerDir,
    target_path: PathBuf,
    cx: &AsyncApp,
) -> Result<Option<PathBuf>> {
    match OS {
        "macos" => install_release_macos(installer_dir, target_path, cx).await,
        "linux" => install_release_linux(installer_dir, target_path, cx).await,
        "windows" => install_release_windows(target_path).await,
        unsupported_os => anyhow::bail!("not supported: {unsupported_os}"),
    }
}

#[cfg(target_os = "linux")]
async fn install_release_linux(
    temp_dir: InstallerDir,
    downloaded_tar_gz: PathBuf,
    cx: &AsyncApp,
) -> Result<Option<PathBuf>> {
    let channel = cx.update(|cx| ReleaseChannel::global(cx).dev_name())?;
    let home_dir = PathBuf::from(env::var("HOME").context("no HOME env var set")?);
    let running_app_path = cx.update(|cx| cx.app_path())??;

    let extracted = temp_dir.path().join("vector");
    smol::fs::create_dir_all(&extracted)
        .await
        .context("failed to create directory into which to extract update")?;

    let output = new_smol_command("tar")
        .arg("-xzf")
        .arg(&downloaded_tar_gz)
        .arg("-C")
        .arg(&extracted)
        .output()
        .await?;

    anyhow::ensure!(
        output.status.success(),
        "failed to extract {:?} to {:?}: {:?}",
        downloaded_tar_gz,
        extracted,
        String::from_utf8_lossy(&output.stderr)
    );

    let suffix = if channel != "stable" {
        format!("-{}", channel)
    } else {
        String::default()
    };
    let app_folder_name = format!("vector{suffix}.app");

    let from = extracted.join(&app_folder_name);
    let mut to = home_dir.join(".local");

    let expected_suffix = format!("{}/libexec/vector-editor", app_folder_name);

    if let Some(prefix) = running_app_path
        .to_str()
        .and_then(|str| str.strip_suffix(&expected_suffix))
    {
        to = PathBuf::from(prefix);
    }

    let output = new_smol_command("rsync")
        .args(["-av", "--delete"])
        .arg(&from)
        .arg(&to)
        .output()
        .await?;

    anyhow::ensure!(
        output.status.success(),
        "failed to copy Vector update from {:?} to {:?}: {:?}",
        from,
        to,
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(Some(to.join(expected_suffix)))
}

#[cfg(target_os = "macos")]
struct MacOsUnmounter {
    mount_path: PathBuf,
    background_executor: gpui::BackgroundExecutor,
}

#[cfg(target_os = "macos")]
impl Drop for MacOsUnmounter {
    fn drop(&mut self) {
        let mount_path = std::mem::take(&mut self.mount_path);
        self.background_executor
            .spawn(async move {
                let unmount_output = new_smol_command("hdiutil")
                    .args(["detach", "-force"])
                    .arg(&mount_path)
                    .output()
                    .await;
                match unmount_output {
                    Ok(output) if output.status.success() => {
                        log::info!("Successfully unmounted the disk image");
                    }
                    Ok(output) => {
                        log::error!(
                            "Failed to unmount disk image: {:?}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                    Err(error) => {
                        log::error!("Error while trying to unmount disk image: {:?}", error);
                    }
                }
            })
            .detach();
    }
}

#[cfg(target_os = "macos")]
async fn install_release_macos(
    _temp_dir: InstallerDir,
    downloaded_dmg: PathBuf,
    cx: &AsyncApp,
) -> Result<Option<PathBuf>> {
    let running_app_path = cx.update(|cx| cx.app_path())??;
    let running_app_filename = running_app_path
        .file_name()
        .with_context(|| format!("invalid running app path {running_app_path:?}"))?;

    let output = new_smol_command("hdiutil")
        .args(["attach", "-nobrowse"])
        .arg(&downloaded_dmg)
        .output()
        .await?;

    anyhow::ensure!(
        output.status.success(),
        "failed to mount: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount_path = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .find(|field| field.starts_with("/Volumes/"))
        .map(PathBuf::from)
        .context("failed to determine mount path")?;

    let _unmounter = MacOsUnmounter {
        mount_path: mount_path.clone(),
        background_executor: cx.background_executor().clone(),
    };

    let mut mounted_app_path = mount_path.join(running_app_filename);
    mounted_app_path.push("/");

    let output = new_smol_command("rsync")
        .args(["-av", "--delete"])
        .arg(&mounted_app_path)
        .arg(&running_app_path)
        .output()
        .await?;

    anyhow::ensure!(
        output.status.success(),
        "failed to copy app: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(None)
}

#[cfg(not(target_os = "macos"))]
async fn install_release_macos(
    _temp_dir: InstallerDir,
    _downloaded_dmg: PathBuf,
    _cx: &AsyncApp,
) -> Result<Option<PathBuf>> {
    anyhow::bail!("not supported")
}

#[cfg(not(target_os = "linux"))]
async fn install_release_linux(
    _temp_dir: InstallerDir,
    _downloaded_tar_gz: PathBuf,
    _cx: &AsyncApp,
) -> Result<Option<PathBuf>> {
    anyhow::bail!("not supported")
}

async fn install_release_windows(_downloaded_installer: PathBuf) -> Result<Option<PathBuf>> {
    anyhow::bail!("not supported")
}
