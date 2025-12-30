use crate::{
    LocationLink,
    lsp_command::{LspCommand, location_link_from_lsp, location_links_from_lsp},
    lsp_store::LspStore,
    make_lsp_text_document_position, make_text_document_identifier,
};
use anyhow::Result;
use async_trait::async_trait;
use collections::HashMap;
use gpui::{App, AsyncApp, Entity};
use language::{Buffer, point_to_lsp};
use lsp::{LanguageServer, LanguageServerId};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use task::TaskTemplate;
use text::{BufferId, PointUtf16, ToPointUtf16};

pub enum LspExtExpandMacro {}

impl lsp::request::Request for LspExtExpandMacro {
    type Params = ExpandMacroParams;
    type Result = Option<ExpandedMacro>;
    const METHOD: &'static str = "rust-analyzer/expandMacro";
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExpandMacroParams {
    pub text_document: lsp::TextDocumentIdentifier,
    pub position: lsp::Position,
}

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExpandedMacro {
    pub name: String,
    pub expansion: String,
}

impl ExpandedMacro {
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.expansion.is_empty()
    }
}
#[derive(Debug)]
pub struct ExpandMacro {
    pub position: PointUtf16,
}

#[async_trait(?Send)]
impl LspCommand for ExpandMacro {
    type Response = ExpandedMacro;
    type LspRequest = LspExtExpandMacro;

    fn display_name(&self) -> &str {
        "Expand macro"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<ExpandMacroParams> {
        Ok(ExpandMacroParams {
            text_document: make_text_document_identifier(path)?,
            position: point_to_lsp(self.position),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<ExpandedMacro>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> anyhow::Result<ExpandedMacro> {
        Ok(message
            .map(|message| ExpandedMacro {
                name: message.name,
                expansion: message.expansion,
            })
            .unwrap_or_default())
    }
}

pub enum LspOpenDocs {}

impl lsp::request::Request for LspOpenDocs {
    type Params = OpenDocsParams;
    type Result = Option<DocsUrls>;
    const METHOD: &'static str = "experimental/externalDocs";
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocsParams {
    pub text_document: lsp::TextDocumentIdentifier,
    pub position: lsp::Position,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocsUrls {
    pub web: Option<String>,
    pub local: Option<String>,
}

impl DocsUrls {
    pub fn is_empty(&self) -> bool {
        self.web.is_none() && self.local.is_none()
    }
}

#[derive(Debug)]
pub struct OpenDocs {
    pub position: PointUtf16,
}

#[async_trait(?Send)]
impl LspCommand for OpenDocs {
    type Response = DocsUrls;
    type LspRequest = LspOpenDocs;

    fn display_name(&self) -> &str {
        "Open docs"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<OpenDocsParams> {
        Ok(OpenDocsParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: lsp::url_from_file_path(path).unwrap(),
            },
            position: point_to_lsp(self.position),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<DocsUrls>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> anyhow::Result<DocsUrls> {
        Ok(message
            .map(|message| DocsUrls {
                web: message.web,
                local: message.local,
            })
            .unwrap_or_default())
    }
}

pub enum LspSwitchSourceHeader {}

impl lsp::request::Request for LspSwitchSourceHeader {
    type Params = SwitchSourceHeaderParams;
    type Result = Option<SwitchSourceHeaderResult>;
    const METHOD: &'static str = "textDocument/switchSourceHeader";
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSourceHeaderParams(lsp::TextDocumentIdentifier);

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSourceHeaderResult(pub String);

#[derive(Default, Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SwitchSourceHeader;

#[derive(Debug)]
pub struct GoToParentModule {
    pub position: PointUtf16,
}

pub struct LspGoToParentModule {}

impl lsp::request::Request for LspGoToParentModule {
    type Params = lsp::TextDocumentPositionParams;
    type Result = Option<Vec<lsp::LocationLink>>;
    const METHOD: &'static str = "experimental/parentModule";
}

#[async_trait(?Send)]
impl LspCommand for SwitchSourceHeader {
    type Response = SwitchSourceHeaderResult;
    type LspRequest = LspSwitchSourceHeader;

    fn display_name(&self) -> &str {
        "Switch source header"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<SwitchSourceHeaderParams> {
        Ok(SwitchSourceHeaderParams(make_text_document_identifier(
            path,
        )?))
    }

    async fn response_from_lsp(
        self,
        message: Option<SwitchSourceHeaderResult>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> anyhow::Result<SwitchSourceHeaderResult> {
        Ok(message
            .map(|message| SwitchSourceHeaderResult(message.0))
            .unwrap_or_default())
    }
}

#[async_trait(?Send)]
impl LspCommand for GoToParentModule {
    type Response = Vec<LocationLink>;
    type LspRequest = LspGoToParentModule;

    fn display_name(&self) -> &str {
        "Go to parent module"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::TextDocumentPositionParams> {
        make_lsp_text_document_position(path, self.position)
    }

    async fn response_from_lsp(
        self,
        links: Option<Vec<lsp::LocationLink>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> anyhow::Result<Vec<LocationLink>> {
        location_links_from_lsp(
            links.map(lsp::GotoDefinitionResponse::Link),
            lsp_store,
            buffer,
            server_id,
            cx,
        )
        .await
    }
}

// https://rust-analyzer.github.io/book/contributing/lsp-extensions.html#runnables
// Taken from https://github.com/rust-lang/rust-analyzer/blob/a73a37a757a58b43a796d3eb86a1f7dfd0036659/crates/rust-analyzer/src/lsp/ext.rs#L425-L489
pub enum Runnables {}

impl lsp::request::Request for Runnables {
    type Params = RunnablesParams;
    type Result = Vec<Runnable>;
    const METHOD: &'static str = "experimental/runnables";
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RunnablesParams {
    pub text_document: lsp::TextDocumentIdentifier,
    #[serde(default)]
    pub position: Option<lsp::Position>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Runnable {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<lsp::LocationLink>,
    pub kind: RunnableKind,
    pub args: RunnableArgs,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum RunnableArgs {
    Cargo(CargoRunnableArgs),
    Shell(ShellRunnableArgs),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum RunnableKind {
    Cargo,
    Shell,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CargoRunnableArgs {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,
    pub cwd: PathBuf,
    /// Command to be executed instead of cargo
    #[serde(default)]
    pub override_cargo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    // command, --package and --lib stuff
    #[serde(default)]
    pub cargo_args: Vec<String>,
    // stuff after --
    #[serde(default)]
    pub executable_args: Vec<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ShellRunnableArgs {
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,
    pub cwd: PathBuf,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug)]
pub struct GetLspRunnables {
    pub buffer_id: BufferId,
    pub position: Option<text::Anchor>,
}

#[derive(Debug, Default)]
pub struct LspRunnables {
    pub runnables: Vec<(Option<LocationLink>, TaskTemplate)>,
}

#[async_trait(?Send)]
impl LspCommand for GetLspRunnables {
    type Response = LspRunnables;
    type LspRequest = Runnables;

    fn display_name(&self) -> &str {
        "LSP Runnables"
    }

    fn to_lsp(
        &self,
        path: &Path,
        buffer: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<RunnablesParams> {
        let url = match lsp::url_from_file_path(path) {
            Ok(url) => url,
            Err(()) => anyhow::bail!("Failed to parse path {path:?} as lsp::Url"),
        };
        Ok(RunnablesParams {
            text_document: lsp::TextDocumentIdentifier::new(url),
            position: self
                .position
                .map(|anchor| point_to_lsp(anchor.to_point_utf16(&buffer.snapshot()))),
        })
    }

    async fn response_from_lsp(
        self,
        lsp_runnables: Vec<Runnable>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<LspRunnables> {
        let mut runnables = Vec::with_capacity(lsp_runnables.len());

        for runnable in lsp_runnables {
            let location = match runnable.location {
                Some(location) => Some(
                    location_link_from_lsp(location, &lsp_store, &buffer, server_id, &mut cx)
                        .await?,
                ),
                None => None,
            };
            let mut task_template = TaskTemplate::default();
            task_template.label = runnable.label;
            match runnable.args {
                RunnableArgs::Cargo(cargo) => {
                    match cargo.override_cargo {
                        Some(override_cargo) => {
                            let mut override_parts =
                                override_cargo.split(" ").map(|s| s.to_string());
                            task_template.command = override_parts
                                .next()
                                .unwrap_or_else(|| override_cargo.clone());
                            task_template.args.extend(override_parts);
                        }
                        None => task_template.command = "cargo".to_string(),
                    };
                    task_template.env = cargo.environment;
                    task_template.cwd = Some(
                        cargo
                            .workspace_root
                            .unwrap_or(cargo.cwd)
                            .to_string_lossy()
                            .to_string(),
                    );
                    task_template.args.extend(cargo.cargo_args);
                    if !cargo.executable_args.is_empty() {
                        task_template.args.push("--".to_string());
                        task_template.args.extend(
                            cargo
                                .executable_args
                                .into_iter()
                                // rust-analyzer's doctest data may be smth. like
                                // ```
                                // command: "cargo",
                                // args: [
                                //     "test",
                                //     "--doc",
                                //     "--package",
                                //     "cargo-output-parser",
                                //     "--",
                                //     "X<T>::new",
                                //     "--show-output",
                                // ],
                                // ```
                                // and `X<T>::new` will cause troubles if not escaped properly, as later
                                // the task runs as `$SHELL -i -c "cargo test ..."`.
                                //
                                // We cannot escape all shell arguments unconditionally, as we use this for ssh commands, which may involve paths starting with `~`.
                                // That bit is not auto-expanded when using single quotes.
                                // Escape extra cargo args unconditionally as those are unlikely to contain `~`.
                                .flat_map(|extra_arg| {
                                    shlex::try_quote(&extra_arg).ok().map(|s| s.to_string())
                                }),
                        );
                    }
                }
                RunnableArgs::Shell(shell) => {
                    task_template.command = shell.program;
                    task_template.args = shell.args;
                    task_template.env = shell.environment;
                    task_template.cwd = Some(shell.cwd.to_string_lossy().to_string());
                }
            }

            runnables.push((location, task_template));
        }

        Ok(LspRunnables { runnables })
    }
}

#[derive(Debug)]
pub struct LspExtCancelFlycheck {}

#[derive(Debug)]
pub struct LspExtRunFlycheck {}

#[derive(Debug)]
pub struct LspExtClearFlycheck {}

impl lsp::notification::Notification for LspExtCancelFlycheck {
    type Params = ();
    const METHOD: &'static str = "rust-analyzer/cancelFlycheck";
}

impl lsp::notification::Notification for LspExtRunFlycheck {
    type Params = RunFlycheckParams;
    const METHOD: &'static str = "rust-analyzer/runFlycheck";
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunFlycheckParams {
    pub text_document: Option<lsp::TextDocumentIdentifier>,
}

impl lsp::notification::Notification for LspExtClearFlycheck {
    type Params = ();
    const METHOD: &'static str = "rust-analyzer/clearFlycheck";
}
