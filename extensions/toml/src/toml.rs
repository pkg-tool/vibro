use std::fs;
use vector::LanguageServerId;
use vector_extension_api::settings::LspSettings;
use vector_extension_api::{self as vector, Result};

struct TaploBinary {
    path: String,
    args: Option<Vec<String>>,
}

struct TomlExtension {
    cached_binary_path: Option<String>,
}

impl TomlExtension {
    fn language_server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &vector::Worktree,
    ) -> Result<TaploBinary> {
        let binary_settings = LspSettings::for_worktree("taplo", worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.binary);
        let binary_args = binary_settings
            .as_ref()
            .and_then(|binary_settings| binary_settings.arguments.clone());

        if let Some(path) = binary_settings.and_then(|binary_settings| binary_settings.path) {
            return Ok(TaploBinary {
                path,
                args: binary_args,
            });
        }

        if let Some(path) = worktree.which("taplo") {
            return Ok(TaploBinary {
                path,
                args: binary_args,
            });
        }

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).map_or(false, |stat| stat.is_file()) {
                return Ok(TaploBinary {
                    path: path.clone(),
                    args: binary_args,
                });
            }
        }

        vector::set_language_server_installation_status(
            language_server_id,
            &vector::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = vector::latest_github_release(
            "tamasfe/taplo",
            vector::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = vector::current_platform();
        let asset_name = format!(
            "taplo-{os}-{arch}.gz",
            arch = match arch {
                vector::Architecture::Aarch64 => "aarch64",
                vector::Architecture::X86 => "x86",
                vector::Architecture::X8664 => "x86_64",
            },
            os = match platform {
                vector::Os::Mac => "darwin",
                vector::Os::Linux => "linux",
                vector::Os::Windows => "windows",
            },
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("taplo-{}", release.version);
        fs::create_dir_all(&version_dir)
            .map_err(|err| format!("failed to create directory '{version_dir}': {err}"))?;

        let binary_path = format!(
            "{version_dir}/{bin_name}",
            bin_name = match platform {
                vector::Os::Windows => "taplo.exe",
                vector::Os::Mac | vector::Os::Linux => "taplo",
            }
        );

        if !fs::metadata(&binary_path).map_or(false, |stat| stat.is_file()) {
            vector::set_language_server_installation_status(
                language_server_id,
                &vector::LanguageServerInstallationStatus::Downloading,
            );

            vector::download_file(
                &asset.download_url,
                &binary_path,
                vector::DownloadedFileType::Gzip,
            )
            .map_err(|err| format!("failed to download file: {err}"))?;

            vector::make_file_executable(&binary_path)?;

            let entries = fs::read_dir(".")
                .map_err(|err| format!("failed to list working directory {err}"))?;
            for entry in entries {
                let entry = entry.map_err(|err| format!("failed to load directory entry {err}"))?;
                if entry.file_name().to_str() != Some(&version_dir) {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(TaploBinary {
            path: binary_path,
            args: binary_args,
        })
    }
}

impl vector::Extension for TomlExtension {
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
        let taplo_binary = self.language_server_binary(language_server_id, worktree)?;
        Ok(vector::Command {
            command: taplo_binary.path,
            args: taplo_binary
                .args
                .unwrap_or_else(|| vec!["lsp".to_string(), "stdio".to_string()]),
            env: Default::default(),
        })
    }
}

vector::register_extension!(TomlExtension);
