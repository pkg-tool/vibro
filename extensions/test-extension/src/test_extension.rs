use std::fs;
use vector::lsp::CompletionKind;
use vector::{CodeLabel, CodeLabelSpan, LanguageServerId};
use vector_extension_api::process::Command;
use vector_extension_api::{self as vector, Result};

struct TestExtension {
    cached_binary_path: Option<String>,
}

impl TestExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
    ) -> Result<String> {
        let echo_output = Command::new("echo").arg("hello!").output()?;

        println!("{}", String::from_utf8_lossy(&echo_output.stdout));

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).map_or(false, |stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        vector::set_language_server_installation_status(
            language_server_id,
            &vector::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = vector::latest_github_release(
            "gleam-lang/gleam",
            vector::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (platform, arch) = vector::current_platform();
        let asset_name = format!(
            "gleam-{version}-{arch}-{os}.tar.gz",
            version = release.version,
            arch = match arch {
                vector::Architecture::Aarch64 => "aarch64",
                vector::Architecture::X86 => "x86",
                vector::Architecture::X8664 => "x86_64",
            },
            os = match platform {
                vector::Os::Mac => "apple-darwin",
                vector::Os::Linux => "unknown-linux-musl",
                vector::Os::Windows => "pc-windows-msvc",
            },
        );

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("gleam-{}", release.version);
        let binary_path = format!("{version_dir}/gleam");

        if !fs::metadata(&binary_path).map_or(false, |stat| stat.is_file()) {
            vector::set_language_server_installation_status(
                language_server_id,
                &vector::LanguageServerInstallationStatus::Downloading,
            );

            vector::download_file(
                &asset.download_url,
                &version_dir,
                vector::DownloadedFileType::GzipTar,
            )
            .map_err(|e| format!("failed to download file: {e}"))?;

            let entries =
                fs::read_dir(".").map_err(|e| format!("failed to list working directory {e}"))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("failed to load directory entry {e}"))?;
                if entry.file_name().to_str() != Some(&version_dir) {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl vector::Extension for TestExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        _: &vector::Worktree,
    ) -> Result<vector::Command> {
        Ok(vector::Command {
            command: self.language_server_binary_path(language_server_id)?,
            args: vec!["lsp".to_string()],
            env: Default::default(),
        })
    }

    fn label_for_completion(
        &self,
        _language_server_id: &LanguageServerId,
        completion: vector::lsp::Completion,
    ) -> Option<vector::CodeLabel> {
        let name = &completion.label;
        let ty = strip_newlines_from_detail(&completion.detail?);
        let let_binding = "let a";
        let colon = ": ";
        let assignment = " = ";
        let call = match completion.kind? {
            CompletionKind::Function | CompletionKind::Constructor => "()",
            _ => "",
        };
        let code = format!("{let_binding}{colon}{ty}{assignment}{name}{call}");

        Some(CodeLabel {
            spans: vec![
                CodeLabelSpan::code_range({
                    let start = let_binding.len() + colon.len() + ty.len() + assignment.len();
                    start..start + name.len()
                }),
                CodeLabelSpan::code_range({
                    let start = let_binding.len();
                    start..start + colon.len()
                }),
                CodeLabelSpan::code_range({
                    let start = let_binding.len() + colon.len();
                    start..start + ty.len()
                }),
            ],
            filter_range: (0..name.len()).into(),
            code,
        })
    }
}

vector::register_extension!(TestExtension);

/// Removes newlines from the completion detail.
///
/// The Gleam LSP can return types containing newlines, which causes formatting
/// issues within the Vector completions menu.
fn strip_newlines_from_detail(detail: &str) -> String {
    let without_newlines = detail
        .replace("->\n  ", "-> ")
        .replace("\n  ", "")
        .replace(",\n", "");

    let comma_delimited_parts = without_newlines.split(',');
    comma_delimited_parts
        .map(|part| part.trim())
        .collect::<Vec<_>>()
        .join(", ")
}
