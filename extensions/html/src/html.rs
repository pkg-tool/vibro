use std::{env, fs};
use vector::settings::LspSettings;
use vector_extension_api::{self as vector, LanguageServerId, Result, serde_json::json};

const BINARY_NAME: &str = "vscode-html-language-server";
const SERVER_PATH: &str =
    "node_modules/vscode-langservers-extracted/bin/vscode-html-language-server";
const PACKAGE_NAME: &str = "vscode-langservers-extracted";

struct HtmlExtension {
    cached_binary_path: Option<String>,
}

impl HtmlExtension {
    fn server_exists(&self) -> bool {
        fs::metadata(SERVER_PATH).is_ok_and(|stat| stat.is_file())
    }

    fn server_script_path(&mut self, language_server_id: &LanguageServerId) -> Result<String> {
        let server_exists = self.server_exists();
        if self.cached_binary_path.is_some() && server_exists {
            return Ok(SERVER_PATH.to_string());
        }

        vector::set_language_server_installation_status(
            language_server_id,
            &vector::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let version = vector::npm_package_latest_version(PACKAGE_NAME)?;

        if !server_exists
            || vector::npm_package_installed_version(PACKAGE_NAME)?.as_ref() != Some(&version)
        {
            vector::set_language_server_installation_status(
                language_server_id,
                &vector::LanguageServerInstallationStatus::Downloading,
            );
            let result = vector::npm_install_package(PACKAGE_NAME, &version);
            match result {
                Ok(()) => {
                    if !self.server_exists() {
                        Err(format!(
                            "installed package '{PACKAGE_NAME}' did not contain expected path '{SERVER_PATH}'",
                        ))?;
                    }
                }
                Err(error) => {
                    if !self.server_exists() {
                        Err(error)?;
                    }
                }
            }
        }
        Ok(SERVER_PATH.to_string())
    }
}

impl vector::Extension for HtmlExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &vector::Worktree,
    ) -> Result<vector::Command> {
        let server_path = if let Some(path) = worktree.which(BINARY_NAME) {
            return Ok(vector::Command {
                command: path,
                args: vec!["--stdio".to_string()],
                env: Default::default(),
            });
        } else {
            let server_path = self.server_script_path(language_server_id)?;
            env::current_dir()
                .unwrap()
                .join(&server_path)
                .to_string_lossy()
                .to_string()
        };
        self.cached_binary_path = Some(server_path.clone());

        Ok(vector::Command {
            command: vector::node_binary_path()?,
            args: vec![server_path, "--stdio".to_string()],
            env: Default::default(),
        })
    }

    fn language_server_workspace_configuration(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &vector::Worktree,
    ) -> Result<Option<vector::serde_json::Value>> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.settings)
            .unwrap_or_default();
        Ok(Some(settings))
    }

    fn language_server_initialization_options(
        &mut self,
        _: &LanguageServerId,
        _: &vector::Worktree,
    ) -> Result<Option<vector::serde_json::Value>> {
        let initialization_options = json!({"provideFormatter": true });
        Ok(Some(initialization_options))
    }
}

vector::register_extension!(HtmlExtension);
