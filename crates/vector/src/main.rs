// Disable command line from opening on release mode
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod reliability;
mod app;

use anyhow::{Context as _, Result};
use clap::{Parser, command};
use cli::FORCE_CLI_MODE_ENV_VAR_NAME;
use collections::HashMap;
use db::kvp::KEY_VALUE_STORE;
use editor::Editor;
use extension::ExtensionHostProxy;
use extension_host::ExtensionStore;
use fs::{Fs, RealFs};
use futures::{StreamExt, channel::oneshot};
use git::GitHostingProviderRegistry;
use gpui::{App, AppContext as _, Application, AsyncApp, UpdateGlobal as _};

use gpui_tokio::Tokio;
use http_client::{HttpClientWithUrl, read_proxy_from_env};
use language::LanguageRegistry;
use reqwest_client::ReqwestClient;

use assets::Assets;
use node_runtime::{NodeBinaryOptions, NodeRuntime};
use parking_lot::Mutex;
use project::project_settings::ProjectSettings;
use release_channel::{AppCommitSha, AppVersion, ReleaseChannel};
use session::{AppSession, Session};
use settings::{Settings, SettingsStore, watch_config_file};
use std::{
    env,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process,
    sync::Arc,
};
use theme::{
    ActiveTheme, IconThemeNotFoundError, SystemAppearance, ThemeNotFoundError, ThemeRegistry,
    ThemeSettings,
};
use util::ResultExt;
use welcome::{FIRST_OPEN, show_welcome_view};
use workspace::{AppState, SerializedWorkspaceLocation, WorkspaceSettings, WorkspaceStore};
use app::{
    OpenListener, OpenRequest, app_menus, build_window_options, derive_paths_with_position,
    handle_cli_connection, handle_keymap_file_changes, handle_settings_changed,
    handle_settings_file_changes, initialize_workspace, inline_completion_registry,
    open_paths_with_positions,
};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn files_not_created_on_launch(errors: HashMap<io::ErrorKind, Vec<&Path>>) {
    let message = "Vector failed to launch";
    let error_details = errors
        .into_iter()
        .flat_map(|(kind, paths)| {
            #[allow(unused_mut)] // for non-unix platforms
            let mut error_kind_details = match paths.len() {
                0 => return None,
                1 => format!(
                    "{kind} when creating directory {:?}",
                    paths.first().expect("match arm checks for a single entry")
                ),
                _many => format!("{kind} when creating directories {paths:?}"),
            };

            #[cfg(unix)]
            {
                match kind {
                    io::ErrorKind::PermissionDenied => {
                        error_kind_details.push_str("\n\nConsider using chown and chmod tools for altering the directories permissions if your user has corresponding rights.\
                            \nFor example, `sudo chown $(whoami):staff ~/.config` and `chmod +uwrx ~/.config`");
                    }
                    _ => {}
                }
            }

            Some(error_kind_details)
        })
        .collect::<Vec<_>>().join("\n\n");

    eprintln!("{message}: {error_details}");
    Application::new().run(move |cx| {
        if let Ok(window) = cx.open_window(gpui::WindowOptions::default(), |_, cx| {
            cx.new(|_| gpui::Empty)
        }) {
            window
                .update(cx, |_, window, cx| {
                    let response = window.prompt(
                        gpui::PromptLevel::Critical,
                        message,
                        Some(&error_details),
                        &["Exit"],
                        cx,
                    );

                    cx.spawn_in(window, async move |_, cx| {
                        response.await?;
                        cx.update(|_, cx| cx.quit())
                    })
                    .detach_and_log_err(cx);
                })
                .log_err();
        } else {
            fail_to_open_window(anyhow::anyhow!("{message}: {error_details}"), cx)
        }
    })
}

fn fail_to_open_window_async(e: anyhow::Error, cx: &mut AsyncApp) {
    cx.update(|cx| fail_to_open_window(e, cx)).log_err();
}

fn fail_to_open_window(e: anyhow::Error, _cx: &mut App) {
    eprintln!(
        "Vector failed to open a window: {e:?}. See https://vector.dev/docs/linux for troubleshooting steps."
    );
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        process::exit(1);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use ashpd::desktop::notification::{Notification, NotificationProxy, Priority};
        _cx.spawn(async move |_cx| {
            let Ok(proxy) = NotificationProxy::new().await else {
                process::exit(1);
            };

            let notification_id = "dev.vector.Oops";
            proxy
                .add_notification(
                    notification_id,
                    Notification::new("Vector failed to launch")
                        .body(Some(
                            format!(
                                "{e:?}. See https://vector.dev/docs/linux for troubleshooting steps."
                            )
                            .as_str(),
                        ))
                        .priority(Priority::High)
                        .icon(ashpd::desktop::Icon::with_names(&[
                            "dialog-question-symbolic",
                        ])),
                )
                .await
                .ok();

            process::exit(1);
        })
        .detach();
    }
}

fn main() {
    #[cfg(unix)]
    {
        let is_root = nix::unistd::geteuid().is_root();
        let allow_root = env::var("VECTOR_ALLOW_ROOT").is_ok_and(|val| val == "true");

        // Prevent running Vector with root privileges on Unix systems unless explicitly allowed
        if is_root && !allow_root {
            eprintln!(
                "\
Error: Running Vector as root or via sudo is unsupported.
       Doing so (even once) may subtly break things for all subsequent non-root usage of Vector.
       It is untested and not recommended, don't complain when things break.
       If you wish to proceed anyways, set `VECTOR_ALLOW_ROOT=true` in your environment."
            );
            process::exit(1);
        }
    }

    // Check if there is a pending installer
    // If there is, run the installer and exit
    // And we don't want to run the installer if we are not the first instance
    #[cfg(target_os = "windows")]
    let is_first_instance = crate::app::windows_only_instance::is_first_instance();
    #[cfg(target_os = "windows")]
    if is_first_instance && auto_update::check_pending_installation() {
        return;
    }

    let args = Args::parse();

    if let Some(socket) = &args.askpass {
        askpass::main(socket);
        return;
    }

    // Set custom data directory.
    if let Some(dir) = &args.user_data_dir {
        paths::set_custom_data_dir(dir);
    }

    #[cfg(all(not(debug_assertions), target_os = "windows"))]
    unsafe {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

        if args.foreground {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }

    menu::init();
    vector_actions::init();

    let file_errors = init_paths();
    if !file_errors.is_empty() {
        files_not_created_on_launch(file_errors);
        return;
    }

    zlog::init();
    if stdout_is_a_pty() {
        zlog::init_output_stdout();
    } else {
        let result = zlog::init_output_file(paths::log_file(), Some(paths::old_log_file()));
        if let Err(err) = result {
            eprintln!("Could not open log file: {}... Defaulting to stdout", err);
            zlog::init_output_stdout();
        };
    }

    let app_version = AppVersion::load(env!("CARGO_PKG_VERSION"));
    let app_commit_sha = option_env!("VECTOR_COMMIT_SHA")
        .map(|commit_sha| AppCommitSha::new(commit_sha.to_string()));

    log::info!("========== starting vector ==========");

    let app = Application::new().with_assets(Assets);

    let session = app.background_executor().block(Session::new());

    reliability::init_panic_hook(app_version, app_commit_sha.clone());

    let (open_listener, mut open_rx) = OpenListener::new();

    let failed_single_instance_check =
        if *db::VECTOR_STATELESS || *release_channel::RELEASE_CHANNEL == ReleaseChannel::Dev {
            false
        } else {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                crate::app::listen_for_cli_connections(open_listener.clone()).is_err()
            }

            #[cfg(target_os = "windows")]
            {
                !crate::app::windows_only_instance::handle_single_instance(
                    open_listener.clone(),
                    &args,
                    is_first_instance,
                )
            }

            #[cfg(target_os = "macos")]
            {
                use app::mac_only_instance::*;
                ensure_only_instance() != IsOnlyInstance::Yes
            }
        };
    if failed_single_instance_check {
        println!("Vector is already running");
        return;
    }

    let git_hosting_provider_registry = Arc::new(GitHostingProviderRegistry::new());
    let git_binary_path =
        if cfg!(target_os = "macos") && option_env!("VECTOR_BUNDLE").as_deref() == Some("true") {
            app.path_for_auxiliary_executable("git")
                .context("could not find git binary path")
                .log_err()
        } else {
            None
        };
    log::info!("Using git binary path: {:?}", git_binary_path);

    let fs = Arc::new(RealFs::new(git_binary_path, app.background_executor()));
    let user_settings_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::settings_file().clone(),
    );
    let global_settings_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::global_settings_file().clone(),
    );
    let user_keymap_file_rx = watch_config_file(
        &app.background_executor(),
        fs.clone(),
        paths::keymap_file().clone(),
    );

    let (shell_env_loaded_tx, shell_env_loaded_rx) = oneshot::channel();
    if !stdout_is_a_pty() {
        app.background_executor()
            .spawn(async {
                #[cfg(unix)]
                util::load_login_shell_environment().log_err();
                shell_env_loaded_tx.send(()).ok();
            })
            .detach()
    } else {
        drop(shell_env_loaded_tx)
    }

    app.on_open_urls({
        let open_listener = open_listener.clone();
        move |urls| open_listener.open_urls(urls)
    });
    app.on_reopen(move |cx| {
        if let Some(app_state) = AppState::try_global(cx).and_then(|app_state| app_state.upgrade())
        {
            cx.spawn({
                let app_state = app_state.clone();
                async move |mut cx| {
                    if let Err(e) = restore_or_create_workspace(app_state, &mut cx).await {
                        fail_to_open_window_async(e, &mut cx)
                    }
                }
            })
            .detach();
        }
    });

    app.run(move |cx| {
        release_channel::init(app_version, cx);
        gpui_tokio::init(cx);
        if let Some(app_commit_sha) = app_commit_sha {
            AppCommitSha::set_global(app_commit_sha, cx);
        }
        settings::init(cx);
        zlog_settings::init(cx);
        handle_settings_file_changes(
            user_settings_file_rx,
            global_settings_file_rx,
            cx,
            handle_settings_changed,
        );
        handle_keymap_file_changes(user_keymap_file_rx, cx);
        let user_agent = format!(
            "Vector/{} ({}; {})",
            AppVersion::global(cx),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let proxy_url = read_proxy_from_env();
        let base_http = {
            let _guard = Tokio::handle(cx).enter();

            ReqwestClient::proxy_and_user_agent(proxy_url.clone(), &user_agent)
                .expect("could not start HTTP client")
        };
        let base_url = env::var("VECTOR_BASE_URL")
            .or_else(|_| env::var("VECTOR_SERVER_URL"))
            .unwrap_or_else(|_| "https://vector.dev".to_string());
        let http = Arc::new(HttpClientWithUrl::new_url(
            Arc::new(base_http),
            base_url,
            proxy_url,
        ));
        cx.set_http_client(http.clone());

        <dyn Fs>::set_global(fs.clone(), cx);

        GitHostingProviderRegistry::set_global(git_hosting_provider_registry, cx);
        git_hosting_providers::init(cx);

        OpenListener::set_global(cx, open_listener.clone());

        extension::init(cx);
        let extension_host_proxy = ExtensionHostProxy::global(cx);

        let mut languages = LanguageRegistry::new(cx.background_executor().clone());
        languages.set_language_server_download_dir(paths::languages_dir().clone());
        let languages = Arc::new(languages);

        project::Project::init_settings(cx);

        let (tx, rx) = async_watch::channel(None);
        cx.observe_global::<SettingsStore>(move |cx| {
            let settings = &ProjectSettings::get_global(cx).node;
            let options = NodeBinaryOptions {
                allow_path_lookup: !settings.ignore_system_version,
                // TODO: Expose this setting
                allow_binary_download: true,
                use_paths: settings.path.as_ref().map(|node_path| {
                    let node_path = PathBuf::from(shellexpand::tilde(node_path).as_ref());
                    let npm_path = settings
                        .npm_path
                        .as_ref()
                        .map(|path| PathBuf::from(shellexpand::tilde(&path).as_ref()));
                    (
                        node_path.clone(),
                        npm_path.unwrap_or_else(|| {
                            let base_path = PathBuf::new();
                            node_path.parent().unwrap_or(&base_path).join("npm")
                        }),
                    )
                }),
            };
            tx.send(Some(options)).log_err();
        })
        .detach();
        let node_runtime = NodeRuntime::new(http.clone(), Some(shell_env_loaded_rx), rx);

        debug_adapter_extension::init(extension_host_proxy.clone(), cx);
        language::init(cx);
        language_extension::init(extension_host_proxy.clone(), languages.clone());
        languages::init(languages.clone(), node_runtime.clone(), cx);
        let workspace_store = cx.new(WorkspaceStore::new);

        app::init(cx);
        project::Project::init(cx);
        debugger_ui::init(cx);
        debugger_tools::init(cx);
        let app_session = cx.new(|cx| AppSession::new(session, cx));

        let app_state = Arc::new(AppState {
            languages: languages.clone(),
            http_client: http.clone(),
            fs: fs.clone(),
            build_window_options,
            workspace_store,
            node_runtime: node_runtime.clone(),
            session: app_session,
        });
        AppState::set_global(Arc::downgrade(&app_state), cx);

        auto_update::init(http.clone(), cx);
        dap_adapters::init(cx);
        auto_update_ui::init(cx);

        SystemAppearance::init(cx);
        theme::init(theme::LoadThemes::All(Box::new(Assets)), cx);
        theme_extension::init(
            extension_host_proxy.clone(),
            ThemeRegistry::global(cx),
            cx.background_executor().clone(),
        );
        command_palette::init(cx);
        let copilot_language_server_id = app_state.languages.next_language_server_id();
        copilot::init(
            copilot_language_server_id,
            app_state.fs.clone(),
            app_state.node_runtime.clone(),
            cx,
        );
        supermaven::init(app_state.http_client.clone(), cx);
        language_model::init(cx);
        web_search::init(cx);
        snippet_provider::init(cx);
        inline_completion_registry::init(cx);
        repl::init(app_state.fs.clone(), cx);
        extension_host::init(
            extension_host_proxy,
            app_state.fs.clone(),
            app_state.http_client.clone(),
            app_state.node_runtime.clone(),
            cx,
        );
        recent_projects::init(cx);

        load_embedded_fonts(cx);

        app_state.languages.set_theme(cx.theme().clone());
        editor::init(cx);
        image_viewer::init(cx);
        repl::notebook::init(cx);
        diagnostics::init(cx);

        audio::init(Assets, cx);
        workspace::init(app_state.clone(), cx);
        ui_prompt::init(cx);

        go_to_line::init(cx);
        file_finder::init(cx);
        tab_switcher::init(cx);
        outline::init(cx);
        project_symbols::init(cx);
        project_panel::init(cx);
        outline_panel::init(cx);
        tasks_ui::init(cx);
        snippets_ui::init(cx);
        search::init(cx);
        vim::init(cx);
        terminal_view::init(cx);
        journal::init(app_state.clone(), cx);
        language_selector::init(cx);
        toolchain_selector::init(cx);
        theme_selector::init(cx);
        language_tools::init(cx);
        git_ui::init(cx);
        jj_ui::init(cx);
        markdown_preview::init(cx);
        welcome::init(cx);
        settings_ui::init(cx);
        extensions_ui::init(cx);
        inspector_ui::init(app_state.clone(), cx);

        cx.observe_global::<SettingsStore>({
            let fs = fs.clone();
            let languages = app_state.languages.clone();
            move |cx| {
                for &mut window in cx.windows().iter_mut() {
                    let background_appearance = cx.theme().window_background_appearance();
                    window
                        .update(cx, |_, window, _| {
                            window.set_background_appearance(background_appearance)
                        })
                        .ok();
                }

                eager_load_active_theme_and_icon_theme(fs.clone(), cx);

                languages.set_theme(cx.theme().clone());
            }
        })
        .detach();

        let fs = app_state.fs.clone();
        load_user_themes_in_background(fs.clone(), cx);
        watch_themes(fs.clone(), cx);
        watch_languages(fs.clone(), app_state.languages.clone(), cx);

        cx.set_menus(app_menus());
        initialize_workspace(app_state.clone(), cx);

        cx.activate(true);

        let urls: Vec<_> = args
            .paths_or_urls
            .iter()
            .filter_map(|arg| parse_url_arg(arg, cx).log_err())
            .collect();

        if !urls.is_empty() {
            open_listener.open_urls(urls)
        }

        match open_rx
            .try_next()
            .ok()
            .flatten()
            .and_then(|urls| OpenRequest::parse(urls).log_err())
        {
            Some(request) => {
                handle_open_request(request, app_state.clone(), cx);
            }
            None => {
                cx.spawn({
                    let app_state = app_state.clone();
                    async move |mut cx| {
                        if let Err(e) = restore_or_create_workspace(app_state, &mut cx).await {
                            fail_to_open_window_async(e, &mut cx)
                        }
                    }
                })
                .detach();
            }
        }

        let app_state = app_state.clone();

        crate::app::component_preview::init(app_state.clone(), cx);

        cx.spawn(async move |cx| {
            while let Some(urls) = open_rx.next().await {
                cx.update(|cx| {
                    if let Some(request) = OpenRequest::parse(urls).log_err() {
                        handle_open_request(request, app_state.clone(), cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    });
}

fn handle_open_request(request: OpenRequest, app_state: Arc<AppState>, cx: &mut App) {
    if let Some(connection) = request.cli_connection {
        let app_state = app_state.clone();
        cx.spawn(async move |cx| handle_cli_connection(connection, app_state, cx).await)
            .detach();
        return;
    }

    if let Some(action_index) = request.dock_menu_action {
        cx.perform_dock_menu_action(action_index);
        return;
    }

    let mut task = None;
    if !request.open_paths.is_empty() {
        let app_state = app_state.clone();
        task = Some(cx.spawn(async move |mut cx| {
            let paths_with_position =
                derive_paths_with_position(app_state.fs.as_ref(), request.open_paths).await;
            let (_window, results) = open_paths_with_positions(
                &paths_with_position,
                app_state,
                workspace::OpenOptions::default(),
                &mut cx,
            )
            .await?;
            for result in results.into_iter().flatten() {
                if let Err(err) = result {
                    log::error!("Error opening path: {err}",);
                }
            }
            anyhow::Ok(())
        }));
    }

    if let Some(task) = task {
        cx.spawn(async move |mut cx| {
            if let Err(err) = task.await {
                fail_to_open_window_async(err, &mut cx);
            }
        })
        .detach();
    }
}

async fn restore_or_create_workspace(app_state: Arc<AppState>, cx: &mut AsyncApp) -> Result<()> {
    if let Some(locations) = restorable_workspace_locations(cx, &app_state).await {
        for location in locations {
            match location {
                SerializedWorkspaceLocation::Local(location, _) => {
                    let task = cx.update(|cx| {
                        workspace::open_paths(
                            location.paths().as_ref(),
                            app_state.clone(),
                            workspace::OpenOptions::default(),
                            cx,
                        )
                    })?;
                    task.await?;
                }
            }
        }
    } else if matches!(KEY_VALUE_STORE.read_kvp(FIRST_OPEN), Ok(None)) {
        cx.update(|cx| show_welcome_view(app_state, cx))?.await?;
    } else {
        cx.update(|cx| {
            workspace::open_new(
                Default::default(),
                app_state,
                cx,
                |workspace, window, cx| {
                    Editor::new_file(workspace, &Default::default(), window, cx)
                },
            )
        })?
        .await?;
    }

    Ok(())
}

pub(crate) async fn restorable_workspace_locations(
    cx: &mut AsyncApp,
    app_state: &Arc<AppState>,
) -> Option<Vec<SerializedWorkspaceLocation>> {
    let mut restore_behavior = cx
        .update(|cx| WorkspaceSettings::get(None, cx).restore_on_startup)
        .ok()?;

    let session_handle = app_state.session.clone();
    let (last_session_id, last_session_window_stack) = cx
        .update(|cx| {
            let session = session_handle.read(cx);

            (
                session.last_session_id().map(|id| id.to_string()),
                session.last_session_window_stack(),
            )
        })
        .ok()?;

    if last_session_id.is_none()
        && matches!(
            restore_behavior,
            workspace::RestoreOnStartupBehavior::LastSession
        )
    {
        restore_behavior = workspace::RestoreOnStartupBehavior::LastWorkspace;
    }

    match restore_behavior {
        workspace::RestoreOnStartupBehavior::LastWorkspace => {
            workspace::last_opened_workspace_location()
                .await
                .map(|location| vec![location])
        }
        workspace::RestoreOnStartupBehavior::LastSession => {
            if let Some(last_session_id) = last_session_id {
                let ordered = last_session_window_stack.is_some();

                let mut locations = workspace::last_session_workspace_locations(
                    &last_session_id,
                    last_session_window_stack,
                )
                .filter(|locations| !locations.is_empty());

                // Since last_session_window_order returns the windows ordered front-to-back
                // we need to open the window that was frontmost last.
                if ordered {
                    if let Some(locations) = locations.as_mut() {
                        locations.reverse();
                    }
                }

                locations
            } else {
                None
            }
        }
        _ => None,
    }
}

fn init_paths() -> HashMap<io::ErrorKind, Vec<&'static Path>> {
    [
        paths::config_dir(),
        paths::extensions_dir(),
        paths::languages_dir(),
        paths::database_dir(),
        paths::logs_dir(),
        paths::temp_dir(),
    ]
    .into_iter()
    .fold(HashMap::default(), |mut errors, path| {
        if let Err(e) = std::fs::create_dir_all(path) {
            errors.entry(e.kind()).or_insert_with(Vec::new).push(path);
        }
        errors
    })
}

fn stdout_is_a_pty() -> bool {
    std::env::var(FORCE_CLI_MODE_ENV_VAR_NAME).ok().is_none() && io::stdout().is_terminal()
}

#[derive(Parser, Debug)]
#[command(name = "vector", disable_version_flag = true)]
struct Args {
    /// A sequence of space-separated paths or urls that you want to open.
    ///
    /// Use `path:line:row` syntax to open a file at a specific location.
    /// Non-existing paths and directories will ignore `:line:row` suffix.
    ///
    /// URLs can either be `file://` or `vector://` scheme.
    paths_or_urls: Vec<String>,

    /// Sets a custom directory for all user data (e.g., database, extensions, logs).
    /// This overrides the default platform-specific data directory location.
    /// On macOS, the default is `~/Library/Application Support/Vector`.
    /// On Linux/FreeBSD, the default is `$XDG_DATA_HOME/vector`.
    /// On Windows, the default is `%LOCALAPPDATA%\Vector`.
    #[arg(long, value_name = "DIR")]
    user_data_dir: Option<String>,

    /// Used for SSH/Git password authentication, to remove the need for netcat as a dependency,
    /// by having Vector act like netcat communicating over a Unix socket.
    #[arg(long, hide = true)]
    askpass: Option<String>,

    /// Run Vector in the foreground, only used on Windows, to match the behavior on macOS.
    #[arg(long)]
    #[cfg(target_os = "windows")]
    #[arg(hide = true)]
    foreground: bool,

    /// The dock action to perform. This is used on Windows only.
    #[arg(long)]
    #[cfg(target_os = "windows")]
    #[arg(hide = true)]
    dock_action: Option<usize>,
}

fn parse_url_arg(arg: &str, cx: &App) -> Result<String> {
    match std::fs::canonicalize(Path::new(&arg)) {
        Ok(path) => Ok(format!("file://{}", path.display())),
        Err(error) => {
            if arg.starts_with("file://")
                || arg.starts_with("vector-cli://")
                || is_vector_url_candidate(arg, cx)
            {
                Ok(arg.into())
            } else {
                anyhow::bail!("error parsing path argument: {error}")
            }
        }
    }
}

fn is_vector_url_candidate(arg: &str, cx: &App) -> bool {
    if arg.starts_with("vector://") {
        return true;
    }

    let Some(app_state) = AppState::try_global(cx).and_then(|state| state.upgrade()) else {
        return false;
    };

    arg.strip_prefix(&app_state.http_client.base_url())
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some()
}

fn load_embedded_fonts(cx: &App) {
    let asset_source = cx.asset_source();
    let font_paths = asset_source.list("fonts").unwrap();
    let embedded_fonts = Mutex::new(Vec::new());
    let executor = cx.background_executor();

    executor.block(executor.scoped(|scope| {
        for font_path in &font_paths {
            if !font_path.ends_with(".ttf") {
                continue;
            }

            scope.spawn(async {
                let font_bytes = asset_source.load(font_path).unwrap().unwrap();
                embedded_fonts.lock().push(font_bytes);
            });
        }
    }));

    cx.text_system()
        .add_fonts(embedded_fonts.into_inner())
        .unwrap();
}

/// Eagerly loads the active theme and icon theme based on the selections in the
/// theme settings.
///
/// This fast path exists to load these themes as soon as possible so the user
/// doesn't see the default themes while waiting on extensions to load.
fn eager_load_active_theme_and_icon_theme(fs: Arc<dyn Fs>, cx: &App) {
    let extension_store = ExtensionStore::global(cx);
    let theme_registry = ThemeRegistry::global(cx);
    let theme_settings = ThemeSettings::get_global(cx);
    let appearance = SystemAppearance::global(cx).0;

    if let Some(theme_selection) = theme_settings.theme_selection.as_ref() {
        let theme_name = theme_selection.theme(appearance);
        if matches!(theme_registry.get(theme_name), Err(ThemeNotFoundError(_))) {
            if let Some(theme_path) = extension_store.read(cx).path_to_extension_theme(theme_name) {
                cx.spawn({
                    let theme_registry = theme_registry.clone();
                    let fs = fs.clone();
                    async move |cx| {
                        theme_registry.load_user_theme(&theme_path, fs).await?;

                        cx.update(|cx| {
                            ThemeSettings::reload_current_theme(cx);
                        })
                    }
                })
                .detach_and_log_err(cx);
            }
        }
    }

    if let Some(icon_theme_selection) = theme_settings.icon_theme_selection.as_ref() {
        let icon_theme_name = icon_theme_selection.icon_theme(appearance);
        if matches!(
            theme_registry.get_icon_theme(icon_theme_name),
            Err(IconThemeNotFoundError(_))
        ) {
            if let Some((icon_theme_path, icons_root_path)) = extension_store
                .read(cx)
                .path_to_extension_icon_theme(icon_theme_name)
            {
                cx.spawn({
                    let theme_registry = theme_registry.clone();
                    let fs = fs.clone();
                    async move |cx| {
                        theme_registry
                            .load_icon_theme(&icon_theme_path, &icons_root_path, fs)
                            .await?;

                        cx.update(|cx| {
                            ThemeSettings::reload_current_icon_theme(cx);
                        })
                    }
                })
                .detach_and_log_err(cx);
            }
        }
    }
}

/// Spawns a background task to load the user themes from the themes directory.
fn load_user_themes_in_background(fs: Arc<dyn fs::Fs>, cx: &mut App) {
    cx.spawn({
        let fs = fs.clone();
        async move |cx| {
            if let Some(theme_registry) =
                cx.update(|cx| ThemeRegistry::global(cx).clone()).log_err()
            {
                let themes_dir = paths::themes_dir().as_ref();
                match fs
                    .metadata(themes_dir)
                    .await
                    .ok()
                    .flatten()
                    .map(|m| m.is_dir)
                {
                    Some(is_dir) => {
                        anyhow::ensure!(is_dir, "Themes dir path {themes_dir:?} is not a directory")
                    }
                    None => {
                        fs.create_dir(themes_dir).await.with_context(|| {
                            format!("Failed to create themes dir at path {themes_dir:?}")
                        })?;
                    }
                }
                theme_registry.load_user_themes(themes_dir, fs).await?;
                cx.update(ThemeSettings::reload_current_theme)?;
            }
            anyhow::Ok(())
        }
    })
    .detach_and_log_err(cx);
}

/// Spawns a background task to watch the themes directory for changes.
fn watch_themes(fs: Arc<dyn fs::Fs>, cx: &mut App) {
    use std::time::Duration;
    cx.spawn(async move |cx| {
        let (mut events, _) = fs
            .watch(paths::themes_dir(), Duration::from_millis(100))
            .await;

        while let Some(paths) = events.next().await {
            for event in paths {
                if fs.metadata(&event.path).await.ok().flatten().is_some() {
                    if let Some(theme_registry) =
                        cx.update(|cx| ThemeRegistry::global(cx).clone()).log_err()
                    {
                        if let Some(()) = theme_registry
                            .load_user_theme(&event.path, fs.clone())
                            .await
                            .log_err()
                        {
                            cx.update(ThemeSettings::reload_current_theme).log_err();
                        }
                    }
                }
            }
        }
    })
    .detach()
}

#[cfg(debug_assertions)]
fn watch_languages(fs: Arc<dyn fs::Fs>, languages: Arc<LanguageRegistry>, cx: &mut App) {
    use std::time::Duration;

    let path = {
        let p = Path::new("crates/languages/src");
        let Ok(full_path) = p.canonicalize() else {
            return;
        };
        full_path
    };

    cx.spawn(async move |_| {
        let (mut events, _) = fs.watch(path.as_path(), Duration::from_millis(100)).await;
        while let Some(event) = events.next().await {
            let has_language_file = event.iter().any(|event| {
                event
                    .path
                    .extension()
                    .map(|ext| ext.to_string_lossy().as_ref() == "scm")
                    .unwrap_or(false)
            });
            if has_language_file {
                languages.reload();
            }
        }
    })
    .detach()
}

#[cfg(not(debug_assertions))]
fn watch_languages(_fs: Arc<dyn fs::Fs>, _languages: Arc<LanguageRegistry>, _cx: &mut App) {}
