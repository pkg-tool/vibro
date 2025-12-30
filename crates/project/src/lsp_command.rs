mod signature_help;

use crate::{
    CodeAction, CompletionSource, CoreCompletion, DocumentHighlight, DocumentSymbol, Hover,
    HoverBlock, HoverBlockKind, InlayHint, InlayHintLabel, InlayHintLabelPart,
    InlayHintLabelPartTooltip, InlayHintTooltip, Location, LocationLink, LspAction, MarkupContent,
    PrepareRenameResponse, ProjectTransaction, ResolveState,
    lsp_store::{LocalLspStore, LspStore},
};
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use collections::HashSet;
use futures::future;
use gpui::{App, AsyncApp, Entity};
use language::{
    Anchor, Bias, Buffer, BufferSnapshot, CachedLspAdapter, CharKind, OffsetRangeExt, PointUtf16,
    ToOffset, ToPointUtf16, Transaction, Unclipped,
    language_settings::{InlayHintKind, LanguageSettings},
    point_from_lsp, point_to_lsp,
    range_from_lsp, range_to_lsp,
};
use lsp::{
    AdapterServerCapabilities, CodeActionKind, CodeActionOptions, CompletionContext,
    LanguageServer, LanguageServerId,
    LinkedEditingRangeServerCapabilities, OneOf, RenameOptions, ServerCapabilities,
};
use std::{cmp::Reverse, ops::Range, path::Path, sync::Arc};
use text::LineEnding;

pub use signature_help::SignatureHelp;

pub fn lsp_formatting_options(settings: &LanguageSettings) -> lsp::FormattingOptions {
    lsp::FormattingOptions {
        tab_size: settings.tab_size.into(),
        insert_spaces: !settings.hard_tabs,
        trim_trailing_whitespace: Some(settings.remove_trailing_whitespace_on_save),
        trim_final_newlines: Some(settings.ensure_final_newline_on_save),
        insert_final_newline: Some(settings.ensure_final_newline_on_save),
        ..lsp::FormattingOptions::default()
    }
}

pub(crate) fn file_path_to_lsp_url(path: &Path) -> Result<lsp::Url> {
    match lsp::url_from_file_path(path) {
        Ok(url) => Ok(url),
        Err(()) => anyhow::bail!("Invalid file path provided to LSP request: {path:?}"),
    }
}

pub(crate) fn make_text_document_identifier(path: &Path) -> Result<lsp::TextDocumentIdentifier> {
    Ok(lsp::TextDocumentIdentifier {
        uri: file_path_to_lsp_url(path)?,
    })
}

pub(crate) fn make_lsp_text_document_position(
    path: &Path,
    position: PointUtf16,
) -> Result<lsp::TextDocumentPositionParams> {
    Ok(lsp::TextDocumentPositionParams {
        text_document: make_text_document_identifier(path)?,
        position: point_to_lsp(position),
    })
}

#[async_trait(?Send)]
pub trait LspCommand: 'static + Sized + Send + std::fmt::Debug {
    type Response: 'static + Default + Send + std::fmt::Debug;
    type LspRequest: 'static + Send + lsp::request::Request;

    fn display_name(&self) -> &str;

    fn status(&self) -> Option<String> {
        None
    }

    fn to_lsp_params_or_response(
        &self,
        path: &Path,
        buffer: &Buffer,
        language_server: &Arc<LanguageServer>,
        cx: &App,
    ) -> Result<
        LspParamsOrResponse<<Self::LspRequest as lsp::request::Request>::Params, Self::Response>,
    > {
        if self.check_capabilities(language_server.adapter_server_capabilities()) {
            Ok(LspParamsOrResponse::Params(self.to_lsp(
                path,
                buffer,
                language_server,
                cx,
            )?))
        } else {
            Ok(LspParamsOrResponse::Response(Default::default()))
        }
    }

    /// When false, `to_lsp_params_or_response` default implementation will return the default response.
    fn check_capabilities(&self, _: AdapterServerCapabilities) -> bool {
        true
    }

    fn to_lsp(
        &self,
        path: &Path,
        buffer: &Buffer,
        language_server: &Arc<LanguageServer>,
        cx: &App,
    ) -> Result<<Self::LspRequest as lsp::request::Request>::Params>;

    async fn response_from_lsp(
        self,
        message: <Self::LspRequest as lsp::request::Request>::Result,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Self::Response>;
}

pub enum LspParamsOrResponse<P, R> {
    Params(P),
    Response(R),
}

#[derive(Debug)]
pub(crate) struct PrepareRename {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct PerformRename {
    pub position: PointUtf16,
    pub new_name: String,
    pub push_to_history: bool,
}

#[derive(Debug)]
pub struct GetDefinition {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetDeclaration {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetTypeDefinition {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetImplementation {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetReferences {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetDocumentHighlights {
    pub position: PointUtf16,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct GetDocumentSymbols;

#[derive(Clone, Debug)]
pub(crate) struct GetSignatureHelp {
    pub position: PointUtf16,
}

#[derive(Clone, Debug)]
pub(crate) struct GetHover {
    pub position: PointUtf16,
}

#[derive(Debug)]
pub(crate) struct GetCompletions {
    pub position: PointUtf16,
    pub context: CompletionContext,
}

#[derive(Clone, Debug)]
pub(crate) struct GetCodeActions {
    pub range: Range<Anchor>,
    pub kinds: Option<Vec<lsp::CodeActionKind>>,
}

#[derive(Debug)]
pub(crate) struct OnTypeFormatting {
    pub position: PointUtf16,
    pub trigger: String,
    pub options: lsp::FormattingOptions,
    pub push_to_history: bool,
}

#[derive(Debug)]
pub(crate) struct InlayHints {
    pub range: Range<Anchor>,
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct GetCodeLens;

impl GetCodeLens {
    pub(crate) fn can_resolve_lens(capabilities: &ServerCapabilities) -> bool {
        capabilities
            .code_lens_provider
            .as_ref()
            .and_then(|code_lens_options| code_lens_options.resolve_provider)
            .unwrap_or(false)
    }
}

#[derive(Debug)]
pub(crate) struct LinkedEditingRange {
    pub position: Anchor,
}

#[async_trait(?Send)]
impl LspCommand for PrepareRename {
    type Response = PrepareRenameResponse;
    type LspRequest = lsp::request::PrepareRenameRequest;

    fn display_name(&self) -> &str {
        "Prepare rename"
    }

    fn to_lsp_params_or_response(
        &self,
        path: &Path,
        buffer: &Buffer,
        language_server: &Arc<LanguageServer>,
        cx: &App,
    ) -> Result<LspParamsOrResponse<lsp::TextDocumentPositionParams, PrepareRenameResponse>> {
        let rename_provider = language_server
            .adapter_server_capabilities()
            .server_capabilities
            .rename_provider;
        match rename_provider {
            Some(lsp::OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                ..
            })) => Ok(LspParamsOrResponse::Params(self.to_lsp(
                path,
                buffer,
                language_server,
                cx,
            )?)),
            Some(lsp::OneOf::Right(_)) => Ok(LspParamsOrResponse::Response(
                PrepareRenameResponse::OnlyUnpreparedRenameSupported,
            )),
            Some(lsp::OneOf::Left(true)) => Ok(LspParamsOrResponse::Response(
                PrepareRenameResponse::OnlyUnpreparedRenameSupported,
            )),
            _ => anyhow::bail!("Rename not supported"),
        }
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
        message: Option<lsp::PrepareRenameResponse>,
        _: Entity<LspStore>,
        buffer: Entity<Buffer>,
        _: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<PrepareRenameResponse> {
        buffer.read_with(&mut cx, |buffer, _| match message {
            Some(lsp::PrepareRenameResponse::Range(range))
            | Some(lsp::PrepareRenameResponse::RangeWithPlaceholder { range, .. }) => {
                let Range { start, end } = range_from_lsp(range);
                if buffer.clip_point_utf16(start, Bias::Left) == start.0
                    && buffer.clip_point_utf16(end, Bias::Left) == end.0
                {
                    Ok(PrepareRenameResponse::Success(
                        buffer.anchor_after(start)..buffer.anchor_before(end),
                    ))
                } else {
                    Ok(PrepareRenameResponse::InvalidPosition)
                }
            }
            Some(lsp::PrepareRenameResponse::DefaultBehavior { .. }) => {
                let snapshot = buffer.snapshot();
                let (range, _) = snapshot.surrounding_word(self.position);
                let range = snapshot.anchor_after(range.start)..snapshot.anchor_before(range.end);
                Ok(PrepareRenameResponse::Success(range))
            }
            None => Ok(PrepareRenameResponse::InvalidPosition),
        })?
    }
}

#[async_trait(?Send)]
impl LspCommand for PerformRename {
    type Response = ProjectTransaction;
    type LspRequest = lsp::request::Rename;

    fn display_name(&self) -> &str {
        "Rename"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::RenameParams> {
        Ok(lsp::RenameParams {
            text_document_position: make_lsp_text_document_position(path, self.position)?,
            new_name: self.new_name.clone(),
            work_done_progress_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::WorkspaceEdit>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<ProjectTransaction> {
        if let Some(edit) = message {
            let (lsp_adapter, lsp_server) =
                language_server_for_buffer(&lsp_store, &buffer, server_id, &mut cx)?;
            LocalLspStore::deserialize_workspace_edit(
                lsp_store,
                edit,
                self.push_to_history,
                lsp_adapter,
                lsp_server,
                &mut cx,
            )
            .await
        } else {
            Ok(ProjectTransaction::default())
        }
    }
}

#[async_trait(?Send)]
impl LspCommand for GetDefinition {
    type Response = Vec<LocationLink>;
    type LspRequest = lsp::request::GotoDefinition;

    fn display_name(&self) -> &str {
        "Get definition"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .definition_provider
            .is_some()
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::GotoDefinitionParams> {
        Ok(lsp::GotoDefinitionParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::GotoDefinitionResponse>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<LocationLink>> {
        location_links_from_lsp(message, lsp_store, buffer, server_id, cx).await
    }
}

#[async_trait(?Send)]
impl LspCommand for GetDeclaration {
    type Response = Vec<LocationLink>;
    type LspRequest = lsp::request::GotoDeclaration;

    fn display_name(&self) -> &str {
        "Get declaration"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .declaration_provider
            .is_some()
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::GotoDeclarationParams> {
        Ok(lsp::GotoDeclarationParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::GotoDeclarationResponse>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<LocationLink>> {
        location_links_from_lsp(message, lsp_store, buffer, server_id, cx).await
    }
}

#[async_trait(?Send)]
impl LspCommand for GetImplementation {
    type Response = Vec<LocationLink>;
    type LspRequest = lsp::request::GotoImplementation;

    fn display_name(&self) -> &str {
        "Get implementation"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::GotoImplementationParams> {
        Ok(lsp::GotoImplementationParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::GotoImplementationResponse>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<LocationLink>> {
        location_links_from_lsp(message, lsp_store, buffer, server_id, cx).await
    }
}

#[async_trait(?Send)]
impl LspCommand for GetTypeDefinition {
    type Response = Vec<LocationLink>;
    type LspRequest = lsp::request::GotoTypeDefinition;

    fn display_name(&self) -> &str {
        "Get type definition"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        !matches!(
            &capabilities.server_capabilities.type_definition_provider,
            None | Some(lsp::TypeDefinitionProviderCapability::Simple(false))
        )
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::GotoTypeDefinitionParams> {
        Ok(lsp::GotoTypeDefinitionParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::GotoTypeDefinitionResponse>,
        project: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<LocationLink>> {
        location_links_from_lsp(message, project, buffer, server_id, cx).await
    }
}

fn language_server_for_buffer(
    lsp_store: &Entity<LspStore>,
    buffer: &Entity<Buffer>,
    server_id: LanguageServerId,
    cx: &mut AsyncApp,
) -> Result<(Arc<CachedLspAdapter>, Arc<LanguageServer>)> {
    lsp_store
        .update(cx, |lsp_store, cx| {
            buffer.update(cx, |buffer, cx| {
                lsp_store
                    .language_server_for_local_buffer(buffer, server_id, cx)
                    .map(|(adapter, server)| (adapter.clone(), server.clone()))
            })
        })?
        .context("no language server found for buffer")
}

pub async fn location_links_from_lsp(
    message: Option<lsp::GotoDefinitionResponse>,
    lsp_store: Entity<LspStore>,
    buffer: Entity<Buffer>,
    server_id: LanguageServerId,
    mut cx: AsyncApp,
) -> Result<Vec<LocationLink>> {
    let message = match message {
        Some(message) => message,
        None => return Ok(Vec::new()),
    };

    let mut unresolved_links = Vec::new();
    match message {
        lsp::GotoDefinitionResponse::Scalar(loc) => {
            unresolved_links.push((None, loc.uri, loc.range));
        }

        lsp::GotoDefinitionResponse::Array(locs) => {
            unresolved_links.extend(locs.into_iter().map(|l| (None, l.uri, l.range)));
        }

        lsp::GotoDefinitionResponse::Link(links) => {
            unresolved_links.extend(links.into_iter().map(|l| {
                (
                    l.origin_selection_range,
                    l.target_uri,
                    l.target_selection_range,
                )
            }));
        }
    }

    let (lsp_adapter, language_server) =
        language_server_for_buffer(&lsp_store, &buffer, server_id, &mut cx)?;
    let mut definitions = Vec::new();
    for (origin_range, target_uri, target_range) in unresolved_links {
        let target_buffer_handle = lsp_store
            .update(&mut cx, |this, cx| {
                this.open_local_buffer_via_lsp(
                    target_uri,
                    language_server.server_id(),
                    lsp_adapter.name.clone(),
                    cx,
                )
            })?
            .await?;

        cx.update(|cx| {
            let origin_location = origin_range.map(|origin_range| {
                let origin_buffer = buffer.read(cx);
                let origin_start =
                    origin_buffer.clip_point_utf16(point_from_lsp(origin_range.start), Bias::Left);
                let origin_end =
                    origin_buffer.clip_point_utf16(point_from_lsp(origin_range.end), Bias::Left);
                Location {
                    buffer: buffer.clone(),
                    range: origin_buffer.anchor_after(origin_start)
                        ..origin_buffer.anchor_before(origin_end),
                }
            });

            let target_buffer = target_buffer_handle.read(cx);
            let target_start =
                target_buffer.clip_point_utf16(point_from_lsp(target_range.start), Bias::Left);
            let target_end =
                target_buffer.clip_point_utf16(point_from_lsp(target_range.end), Bias::Left);
            let target_location = Location {
                buffer: target_buffer_handle,
                range: target_buffer.anchor_after(target_start)
                    ..target_buffer.anchor_before(target_end),
            };

            definitions.push(LocationLink {
                origin: origin_location,
                target: target_location,
            })
        })?;
    }
    Ok(definitions)
}

pub async fn location_link_from_lsp(
    link: lsp::LocationLink,
    lsp_store: &Entity<LspStore>,
    buffer: &Entity<Buffer>,
    server_id: LanguageServerId,
    cx: &mut AsyncApp,
) -> Result<LocationLink> {
    let (lsp_adapter, language_server) =
        language_server_for_buffer(&lsp_store, &buffer, server_id, cx)?;

    let (origin_range, target_uri, target_range) = (
        link.origin_selection_range,
        link.target_uri,
        link.target_selection_range,
    );

    let target_buffer_handle = lsp_store
        .update(cx, |lsp_store, cx| {
            lsp_store.open_local_buffer_via_lsp(
                target_uri,
                language_server.server_id(),
                lsp_adapter.name.clone(),
                cx,
            )
        })?
        .await?;

    cx.update(|cx| {
        let origin_location = origin_range.map(|origin_range| {
            let origin_buffer = buffer.read(cx);
            let origin_start =
                origin_buffer.clip_point_utf16(point_from_lsp(origin_range.start), Bias::Left);
            let origin_end =
                origin_buffer.clip_point_utf16(point_from_lsp(origin_range.end), Bias::Left);
            Location {
                buffer: buffer.clone(),
                range: origin_buffer.anchor_after(origin_start)
                    ..origin_buffer.anchor_before(origin_end),
            }
        });

        let target_buffer = target_buffer_handle.read(cx);
        let target_start =
            target_buffer.clip_point_utf16(point_from_lsp(target_range.start), Bias::Left);
        let target_end =
            target_buffer.clip_point_utf16(point_from_lsp(target_range.end), Bias::Left);
        let target_location = Location {
            buffer: target_buffer_handle,
            range: target_buffer.anchor_after(target_start)
                ..target_buffer.anchor_before(target_end),
        };

        LocationLink {
            origin: origin_location,
            target: target_location,
        }
    })
}

#[async_trait(?Send)]
impl LspCommand for GetReferences {
    type Response = Vec<Location>;
    type LspRequest = lsp::request::References;

    fn display_name(&self) -> &str {
        "Find all references"
    }

    fn status(&self) -> Option<String> {
        Some("Finding references...".to_owned())
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        match &capabilities.server_capabilities.references_provider {
            Some(OneOf::Left(has_support)) => *has_support,
            Some(OneOf::Right(_)) => true,
            None => false,
        }
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::ReferenceParams> {
        Ok(lsp::ReferenceParams {
            text_document_position: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp::ReferenceContext {
                include_declaration: true,
            },
        })
    }

    async fn response_from_lsp(
        self,
        locations: Option<Vec<lsp::Location>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<Vec<Location>> {
        let mut references = Vec::new();
        let (lsp_adapter, language_server) =
            language_server_for_buffer(&lsp_store, &buffer, server_id, &mut cx)?;

        if let Some(locations) = locations {
            for lsp_location in locations {
                let target_buffer_handle = lsp_store
                    .update(&mut cx, |lsp_store, cx| {
                        lsp_store.open_local_buffer_via_lsp(
                            lsp_location.uri,
                            language_server.server_id(),
                            lsp_adapter.name.clone(),
                            cx,
                        )
                    })?
                    .await?;

                target_buffer_handle
                    .clone()
                    .read_with(&mut cx, |target_buffer, _| {
                        let target_start = target_buffer
                            .clip_point_utf16(point_from_lsp(lsp_location.range.start), Bias::Left);
                        let target_end = target_buffer
                            .clip_point_utf16(point_from_lsp(lsp_location.range.end), Bias::Left);
                        references.push(Location {
                            buffer: target_buffer_handle,
                            range: target_buffer.anchor_after(target_start)
                                ..target_buffer.anchor_before(target_end),
                        });
                    })?;
            }
        }

        Ok(references)
    }
}

#[async_trait(?Send)]
impl LspCommand for GetDocumentHighlights {
    type Response = Vec<DocumentHighlight>;
    type LspRequest = lsp::request::DocumentHighlightRequest;

    fn display_name(&self) -> &str {
        "Get document highlights"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .document_highlight_provider
            .is_some()
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::DocumentHighlightParams> {
        Ok(lsp::DocumentHighlightParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        lsp_highlights: Option<Vec<lsp::DocumentHighlight>>,
        _: Entity<LspStore>,
        buffer: Entity<Buffer>,
        _: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<Vec<DocumentHighlight>> {
        buffer.read_with(&mut cx, |buffer, _| {
            let mut lsp_highlights = lsp_highlights.unwrap_or_default();
            lsp_highlights.sort_unstable_by_key(|h| (h.range.start, Reverse(h.range.end)));
            lsp_highlights
                .into_iter()
                .map(|lsp_highlight| {
                    let start = buffer
                        .clip_point_utf16(point_from_lsp(lsp_highlight.range.start), Bias::Left);
                    let end = buffer
                        .clip_point_utf16(point_from_lsp(lsp_highlight.range.end), Bias::Left);
                    DocumentHighlight {
                        range: buffer.anchor_after(start)..buffer.anchor_before(end),
                        kind: lsp_highlight
                            .kind
                            .unwrap_or(lsp::DocumentHighlightKind::READ),
                    }
                })
                .collect()
        })
    }
}

#[async_trait(?Send)]
impl LspCommand for GetDocumentSymbols {
    type Response = Vec<DocumentSymbol>;
    type LspRequest = lsp::request::DocumentSymbolRequest;

    fn display_name(&self) -> &str {
        "Get document symbols"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .document_symbol_provider
            .is_some()
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::DocumentSymbolParams> {
        Ok(lsp::DocumentSymbolParams {
            text_document: make_text_document_identifier(path)?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        lsp_symbols: Option<lsp::DocumentSymbolResponse>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> Result<Vec<DocumentSymbol>> {
        let Some(lsp_symbols) = lsp_symbols else {
            return Ok(Vec::new());
        };

        let symbols: Vec<_> = match lsp_symbols {
            lsp::DocumentSymbolResponse::Flat(symbol_information) => symbol_information
                .into_iter()
                .map(|lsp_symbol| DocumentSymbol {
                    name: lsp_symbol.name,
                    kind: lsp_symbol.kind,
                    range: range_from_lsp(lsp_symbol.location.range),
                    selection_range: range_from_lsp(lsp_symbol.location.range),
                    children: Vec::new(),
                })
                .collect(),
            lsp::DocumentSymbolResponse::Nested(nested_responses) => {
                fn convert_symbol(lsp_symbol: lsp::DocumentSymbol) -> DocumentSymbol {
                    DocumentSymbol {
                        name: lsp_symbol.name,
                        kind: lsp_symbol.kind,
                        range: range_from_lsp(lsp_symbol.range),
                        selection_range: range_from_lsp(lsp_symbol.selection_range),
                        children: lsp_symbol
                            .children
                            .map(|children| {
                                children.into_iter().map(convert_symbol).collect::<Vec<_>>()
                            })
                            .unwrap_or_default(),
                    }
                }
                nested_responses.into_iter().map(convert_symbol).collect()
            }
        };
        Ok(symbols)
    }
}

#[async_trait(?Send)]
impl LspCommand for GetSignatureHelp {
    type Response = Option<SignatureHelp>;
    type LspRequest = lsp::SignatureHelpRequest;

    fn display_name(&self) -> &str {
        "Get signature help"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .signature_help_provider
            .is_some()
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _cx: &App,
    ) -> Result<lsp::SignatureHelpParams> {
        Ok(lsp::SignatureHelpParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            context: None,
            work_done_progress_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::SignatureHelp>,
        _: Entity<LspStore>,
        _: Entity<Buffer>,
        _: LanguageServerId,
        _: AsyncApp,
    ) -> Result<Self::Response> {
        Ok(message.and_then(SignatureHelp::new))
    }
}

#[async_trait(?Send)]
impl LspCommand for GetHover {
    type Response = Option<Hover>;
    type LspRequest = lsp::request::HoverRequest;

    fn display_name(&self) -> &str {
        "Get hover"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        match capabilities.server_capabilities.hover_provider {
            Some(lsp::HoverProviderCapability::Simple(enabled)) => enabled,
            Some(lsp::HoverProviderCapability::Options(_)) => true,
            None => false,
        }
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::HoverParams> {
        Ok(lsp::HoverParams {
            text_document_position_params: make_lsp_text_document_position(path, self.position)?,
            work_done_progress_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::Hover>,
        _: Entity<LspStore>,
        buffer: Entity<Buffer>,
        _: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<Self::Response> {
        let Some(hover) = message else {
            return Ok(None);
        };

        let (language, range) = buffer.read_with(&mut cx, |buffer, _| {
            (
                buffer.language().cloned(),
                hover.range.map(|range| {
                    let token_start =
                        buffer.clip_point_utf16(point_from_lsp(range.start), Bias::Left);
                    let token_end = buffer.clip_point_utf16(point_from_lsp(range.end), Bias::Left);
                    buffer.anchor_after(token_start)..buffer.anchor_before(token_end)
                }),
            )
        })?;

        fn hover_blocks_from_marked_string(marked_string: lsp::MarkedString) -> Option<HoverBlock> {
            let block = match marked_string {
                lsp::MarkedString::String(content) => HoverBlock {
                    text: content,
                    kind: HoverBlockKind::Markdown,
                },
                lsp::MarkedString::LanguageString(lsp::LanguageString { language, value }) => {
                    HoverBlock {
                        text: value,
                        kind: HoverBlockKind::Code { language },
                    }
                }
            };
            if block.text.is_empty() {
                None
            } else {
                Some(block)
            }
        }

        let contents = match hover.contents {
            lsp::HoverContents::Scalar(marked_string) => {
                hover_blocks_from_marked_string(marked_string)
                    .into_iter()
                    .collect()
            }
            lsp::HoverContents::Array(marked_strings) => marked_strings
                .into_iter()
                .filter_map(hover_blocks_from_marked_string)
                .collect(),
            lsp::HoverContents::Markup(markup_content) => vec![HoverBlock {
                text: markup_content.value,
                kind: if markup_content.kind == lsp::MarkupKind::Markdown {
                    HoverBlockKind::Markdown
                } else {
                    HoverBlockKind::PlainText
                },
            }],
        };

        Ok(Some(Hover {
            contents,
            range,
            language,
        }))
    }
}

#[async_trait(?Send)]
impl LspCommand for GetCompletions {
    type Response = Vec<CoreCompletion>;
    type LspRequest = lsp::request::Completion;

    fn display_name(&self) -> &str {
        "Get completion"
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::CompletionParams> {
        Ok(lsp::CompletionParams {
            text_document_position: make_lsp_text_document_position(path, self.position)?,
            context: Some(self.context.clone()),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        completions: Option<lsp::CompletionResponse>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<Self::Response> {
        let mut completions = if let Some(completions) = completions {
            match completions {
                lsp::CompletionResponse::Array(completions) => completions,
                lsp::CompletionResponse::List(mut list) => {
                    std::mem::take(&mut list.items)
                }
            }
        } else {
            Vec::new()
        };

        let language_server_adapter = lsp_store
            .read_with(&mut cx, |lsp_store, _| {
                lsp_store.language_server_adapter_for_id(server_id)
            })?
            .with_context(|| format!("no language server with id {server_id}"))?;

        let mut completion_edits = Vec::new();
        buffer.update(&mut cx, |buffer, _cx| {
            let snapshot = buffer.snapshot();
            let clipped_position = buffer.clip_point_utf16(Unclipped(self.position), Bias::Left);

            let mut range_for_token = None;
            completions.retain(|lsp_completion| {
                let lsp_edit = lsp_completion.text_edit.clone();

                let edit = match lsp_edit {
                    // If the language server provides a range to overwrite, then
                    // check that the range is valid.
                    Some(completion_text_edit) => {
                        match parse_completion_text_edit(&completion_text_edit, &snapshot) {
                            Some(edit) => edit,
                            None => return false,
                        }
                    }
                    // If the language server does not provide a range, then infer
                    // the range based on the syntax tree.
                    None => {
                        if self.position != clipped_position {
                            log::info!("completion out of expected range");
                            return false;
                        }

                        let range = range_for_token
                            .get_or_insert_with(|| {
                                let offset = self.position.to_offset(&snapshot);
                                let (range, kind) = snapshot.surrounding_word(offset);
                                let range = if kind == Some(CharKind::Word) {
                                    range
                                } else {
                                    offset..offset
                                };

                                snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end)
                            })
                            .clone();

                        // We already know text_edit is None here
                        let text = lsp_completion
                            .insert_text
                            .as_ref()
                            .unwrap_or(&lsp_completion.label)
                            .clone();

                        ParsedCompletionEdit {
                            replace_range: range,
                            insert_range: None,
                            new_text: text,
                        }
                    }
                };

                completion_edits.push(edit);
                true
            });
        })?;

        language_server_adapter
            .process_completions(&mut completions)
            .await;

        Ok(completions
            .into_iter()
            .zip(completion_edits)
            .map(|(lsp_completion, mut edit)| {
                LineEnding::normalize(&mut edit.new_text);
                CoreCompletion {
                    replace_range: edit.replace_range,
                    new_text: edit.new_text,
                    source: CompletionSource::Lsp {
                        insert_range: edit.insert_range,
                        server_id,
                        lsp_completion: Box::new(lsp_completion),
                        resolved: false,
                    },
                }
            })
            .collect())
    }
}

pub struct ParsedCompletionEdit {
    pub replace_range: Range<Anchor>,
    pub insert_range: Option<Range<Anchor>>,
    pub new_text: String,
}

pub(crate) fn parse_completion_text_edit(
    edit: &lsp::CompletionTextEdit,
    snapshot: &BufferSnapshot,
) -> Option<ParsedCompletionEdit> {
    let (replace_range, insert_range, new_text) = match edit {
        lsp::CompletionTextEdit::Edit(edit) => (edit.range, None, &edit.new_text),
        lsp::CompletionTextEdit::InsertAndReplace(edit) => {
            (edit.replace, Some(edit.insert), &edit.new_text)
        }
    };

    let replace_range = {
        let range = range_from_lsp(replace_range);
        let start = snapshot.clip_point_utf16(range.start, Bias::Left);
        let end = snapshot.clip_point_utf16(range.end, Bias::Left);
        if start != range.start.0 || end != range.end.0 {
            log::info!("completion out of expected range");
            return None;
        }
        snapshot.anchor_before(start)..snapshot.anchor_after(end)
    };

    let insert_range = match insert_range {
        None => None,
        Some(insert_range) => {
            let range = range_from_lsp(insert_range);
            let start = snapshot.clip_point_utf16(range.start, Bias::Left);
            let end = snapshot.clip_point_utf16(range.end, Bias::Left);
            if start != range.start.0 || end != range.end.0 {
                log::info!("completion (insert) out of expected range");
                return None;
            }
            Some(snapshot.anchor_before(start)..snapshot.anchor_after(end))
        }
    };

    Some(ParsedCompletionEdit {
        insert_range: insert_range,
        replace_range: replace_range,
        new_text: new_text.clone(),
    })
}

#[async_trait(?Send)]
impl LspCommand for GetCodeActions {
    type Response = Vec<CodeAction>;
    type LspRequest = lsp::request::CodeActionRequest;

    fn display_name(&self) -> &str {
        "Get code actions"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        match &capabilities.server_capabilities.code_action_provider {
            None => false,
            Some(lsp::CodeActionProviderCapability::Simple(false)) => false,
            _ => {
                // If we do know that we want specific code actions AND we know that
                // the server only supports specific code actions, then we want to filter
                // down to the ones that are supported.
                if let Some((requested, supported)) = self
                    .kinds
                    .as_ref()
                    .zip(Self::supported_code_action_kinds(capabilities))
                {
                    let server_supported = supported.into_iter().collect::<HashSet<_>>();
                    requested.iter().any(|kind| server_supported.contains(kind))
                } else {
                    true
                }
            }
        }
    }

    fn to_lsp(
        &self,
        path: &Path,
        buffer: &Buffer,
        language_server: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::CodeActionParams> {
        let mut relevant_diagnostics = Vec::new();
        for entry in buffer
            .snapshot()
            .diagnostics_in_range::<_, language::PointUtf16>(self.range.clone(), false)
        {
            relevant_diagnostics.push(entry.to_lsp_diagnostic_stub()?);
        }

        let supported =
            Self::supported_code_action_kinds(language_server.adapter_server_capabilities());

        let only = if let Some(requested) = &self.kinds {
            if let Some(supported_kinds) = supported {
                let server_supported = supported_kinds.into_iter().collect::<HashSet<_>>();

                let filtered = requested
                    .iter()
                    .filter(|kind| server_supported.contains(kind))
                    .cloned()
                    .collect();
                Some(filtered)
            } else {
                Some(requested.clone())
            }
        } else {
            supported
        };

        Ok(lsp::CodeActionParams {
            text_document: make_text_document_identifier(path)?,
            range: range_to_lsp(self.range.to_point_utf16(buffer))?,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp::CodeActionContext {
                diagnostics: relevant_diagnostics,
                only,
                ..lsp::CodeActionContext::default()
            },
        })
    }

    async fn response_from_lsp(
        self,
        actions: Option<lsp::CodeActionResponse>,
        lsp_store: Entity<LspStore>,
        _: Entity<Buffer>,
        server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<CodeAction>> {
        let requested_kinds_set = if let Some(kinds) = self.kinds {
            Some(kinds.into_iter().collect::<HashSet<_>>())
        } else {
            None
        };

        let language_server = cx.update(|cx| {
            lsp_store
                .read(cx)
                .language_server_for_id(server_id)
                .with_context(|| {
                    format!("Missing the language server that just returned a response {server_id}")
                })
        })??;

        let server_capabilities = language_server.capabilities();
        let available_commands = server_capabilities
            .execute_command_provider
            .as_ref()
            .map(|options| options.commands.as_slice())
            .unwrap_or_default();
        Ok(actions
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| {
                let (lsp_action, resolved) = match entry {
                    lsp::CodeActionOrCommand::CodeAction(lsp_action) => {
                        if let Some(command) = lsp_action.command.as_ref() {
                            if !available_commands.contains(&command.command) {
                                return None;
                            }
                        }
                        (LspAction::Action(Box::new(lsp_action)), false)
                    }
                    lsp::CodeActionOrCommand::Command(command) => {
                        if available_commands.contains(&command.command) {
                            (LspAction::Command(command), true)
                        } else {
                            return None;
                        }
                    }
                };

                if let Some((requested_kinds, kind)) =
                    requested_kinds_set.as_ref().zip(lsp_action.action_kind())
                {
                    if !requested_kinds.contains(&kind) {
                        return None;
                    }
                }

                Some(CodeAction {
                    server_id,
                    range: self.range.clone(),
                    lsp_action,
                    resolved,
                })
            })
            .collect())
    }
}

impl GetCodeActions {
    fn supported_code_action_kinds(
        capabilities: AdapterServerCapabilities,
    ) -> Option<Vec<CodeActionKind>> {
        match capabilities.server_capabilities.code_action_provider {
            Some(lsp::CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(supported_action_kinds),
                ..
            })) => Some(supported_action_kinds.clone()),
            _ => capabilities.code_action_kinds,
        }
    }

    pub fn can_resolve_actions(capabilities: &ServerCapabilities) -> bool {
        capabilities
            .code_action_provider
            .as_ref()
            .and_then(|options| match options {
                lsp::CodeActionProviderCapability::Simple(_is_supported) => None,
                lsp::CodeActionProviderCapability::Options(options) => options.resolve_provider,
            })
            .unwrap_or(false)
    }
}

#[async_trait(?Send)]
impl LspCommand for OnTypeFormatting {
    type Response = Option<Transaction>;
    type LspRequest = lsp::request::OnTypeFormatting;

    fn display_name(&self) -> &str {
        "Formatting on typing"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        let Some(on_type_formatting_options) = &capabilities
            .server_capabilities
            .document_on_type_formatting_provider
        else {
            return false;
        };
        on_type_formatting_options
            .first_trigger_character
            .contains(&self.trigger)
            || on_type_formatting_options
                .more_trigger_character
                .iter()
                .flatten()
                .any(|chars| chars.contains(&self.trigger))
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::DocumentOnTypeFormattingParams> {
        Ok(lsp::DocumentOnTypeFormattingParams {
            text_document_position: make_lsp_text_document_position(path, self.position)?,
            ch: self.trigger.clone(),
            options: self.options.clone(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<Vec<lsp::TextEdit>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> Result<Option<Transaction>> {
        if let Some(edits) = message {
            let (lsp_adapter, lsp_server) =
                language_server_for_buffer(&lsp_store, &buffer, server_id, &mut cx)?;
            LocalLspStore::deserialize_text_edits(
                lsp_store,
                buffer,
                edits,
                self.push_to_history,
                lsp_adapter,
                lsp_server,
                &mut cx,
            )
            .await
        } else {
            Ok(None)
        }
    }
}

impl InlayHints {
    pub async fn lsp_to_project_hint(
        lsp_hint: lsp::InlayHint,
        buffer_handle: &Entity<Buffer>,
        server_id: LanguageServerId,
        resolve_state: ResolveState,
        force_no_type_left_padding: bool,
        cx: &mut AsyncApp,
    ) -> anyhow::Result<InlayHint> {
        let kind = lsp_hint.kind.and_then(|kind| match kind {
            lsp::InlayHintKind::TYPE => Some(InlayHintKind::Type),
            lsp::InlayHintKind::PARAMETER => Some(InlayHintKind::Parameter),
            _ => None,
        });

        let position = buffer_handle.read_with(cx, |buffer, _| {
            let position = buffer.clip_point_utf16(point_from_lsp(lsp_hint.position), Bias::Left);
            if kind == Some(InlayHintKind::Parameter) {
                buffer.anchor_before(position)
            } else {
                buffer.anchor_after(position)
            }
        })?;
        let label = Self::lsp_inlay_label_to_project(lsp_hint.label, server_id)
            .await
            .context("lsp to project inlay hint conversion")?;
        let padding_left = if force_no_type_left_padding && kind == Some(InlayHintKind::Type) {
            false
        } else {
            lsp_hint.padding_left.unwrap_or(false)
        };

        Ok(InlayHint {
            position,
            padding_left,
            padding_right: lsp_hint.padding_right.unwrap_or(false),
            label,
            kind,
            tooltip: lsp_hint.tooltip.map(|tooltip| match tooltip {
                lsp::InlayHintTooltip::String(s) => InlayHintTooltip::String(s),
                lsp::InlayHintTooltip::MarkupContent(markup_content) => {
                    InlayHintTooltip::MarkupContent(MarkupContent {
                        kind: match markup_content.kind {
                            lsp::MarkupKind::PlainText => HoverBlockKind::PlainText,
                            lsp::MarkupKind::Markdown => HoverBlockKind::Markdown,
                        },
                        value: markup_content.value,
                    })
                }
            }),
            resolve_state,
        })
    }

    async fn lsp_inlay_label_to_project(
        lsp_label: lsp::InlayHintLabel,
        server_id: LanguageServerId,
    ) -> anyhow::Result<InlayHintLabel> {
        let label = match lsp_label {
            lsp::InlayHintLabel::String(s) => InlayHintLabel::String(s),
            lsp::InlayHintLabel::LabelParts(lsp_parts) => {
                let mut parts = Vec::with_capacity(lsp_parts.len());
                for lsp_part in lsp_parts {
                    parts.push(InlayHintLabelPart {
                        value: lsp_part.value,
                        tooltip: lsp_part.tooltip.map(|tooltip| match tooltip {
                            lsp::InlayHintLabelPartTooltip::String(s) => {
                                InlayHintLabelPartTooltip::String(s)
                            }
                            lsp::InlayHintLabelPartTooltip::MarkupContent(markup_content) => {
                                InlayHintLabelPartTooltip::MarkupContent(MarkupContent {
                                    kind: match markup_content.kind {
                                        lsp::MarkupKind::PlainText => HoverBlockKind::PlainText,
                                        lsp::MarkupKind::Markdown => HoverBlockKind::Markdown,
                                    },
                                    value: markup_content.value,
                                })
                            }
                        }),
                        location: Some(server_id).zip(lsp_part.location),
                    });
                }
                InlayHintLabel::LabelParts(parts)
            }
        };

        Ok(label)
    }

    pub fn project_to_lsp_hint(hint: InlayHint, snapshot: &BufferSnapshot) -> lsp::InlayHint {
        lsp::InlayHint {
            position: point_to_lsp(hint.position.to_point_utf16(snapshot)),
            kind: hint.kind.map(|kind| match kind {
                InlayHintKind::Type => lsp::InlayHintKind::TYPE,
                InlayHintKind::Parameter => lsp::InlayHintKind::PARAMETER,
            }),
            text_edits: None,
            tooltip: hint.tooltip.and_then(|tooltip| {
                Some(match tooltip {
                    InlayHintTooltip::String(s) => lsp::InlayHintTooltip::String(s),
                    InlayHintTooltip::MarkupContent(markup_content) => {
                        lsp::InlayHintTooltip::MarkupContent(lsp::MarkupContent {
                            kind: match markup_content.kind {
                                HoverBlockKind::PlainText => lsp::MarkupKind::PlainText,
                                HoverBlockKind::Markdown => lsp::MarkupKind::Markdown,
                                HoverBlockKind::Code { .. } => return None,
                            },
                            value: markup_content.value,
                        })
                    }
                })
            }),
            label: match hint.label {
                InlayHintLabel::String(s) => lsp::InlayHintLabel::String(s),
                InlayHintLabel::LabelParts(label_parts) => lsp::InlayHintLabel::LabelParts(
                    label_parts
                        .into_iter()
                        .map(|part| lsp::InlayHintLabelPart {
                            value: part.value,
                            tooltip: part.tooltip.and_then(|tooltip| {
                                Some(match tooltip {
                                    InlayHintLabelPartTooltip::String(s) => {
                                        lsp::InlayHintLabelPartTooltip::String(s)
                                    }
                                    InlayHintLabelPartTooltip::MarkupContent(markup_content) => {
                                        lsp::InlayHintLabelPartTooltip::MarkupContent(
                                            lsp::MarkupContent {
                                                kind: match markup_content.kind {
                                                    HoverBlockKind::PlainText => {
                                                        lsp::MarkupKind::PlainText
                                                    }
                                                    HoverBlockKind::Markdown => {
                                                        lsp::MarkupKind::Markdown
                                                    }
                                                    HoverBlockKind::Code { .. } => return None,
                                                },
                                                value: markup_content.value,
                                            },
                                        )
                                    }
                                })
                            }),
                            location: part.location.map(|(_, location)| location),
                            command: None,
                        })
                        .collect(),
                ),
            },
            padding_left: Some(hint.padding_left),
            padding_right: Some(hint.padding_right),
            data: match hint.resolve_state {
                ResolveState::CanResolve(_, data) => data,
                ResolveState::Resolving | ResolveState::Resolved => None,
            },
        }
    }

    pub fn can_resolve_inlays(capabilities: &ServerCapabilities) -> bool {
        capabilities
            .inlay_hint_provider
            .as_ref()
            .and_then(|options| match options {
                OneOf::Left(_is_supported) => None,
                OneOf::Right(capabilities) => match capabilities {
                    lsp::InlayHintServerCapabilities::Options(o) => o.resolve_provider,
                    lsp::InlayHintServerCapabilities::RegistrationOptions(o) => {
                        o.inlay_hint_options.resolve_provider
                    }
                },
            })
            .unwrap_or(false)
    }
}

#[async_trait(?Send)]
impl LspCommand for InlayHints {
    type Response = Vec<InlayHint>;
    type LspRequest = lsp::InlayHintRequest;

    fn display_name(&self) -> &str {
        "Inlay hints"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        let Some(inlay_hint_provider) = &capabilities.server_capabilities.inlay_hint_provider
        else {
            return false;
        };
        match inlay_hint_provider {
            lsp::OneOf::Left(enabled) => *enabled,
            lsp::OneOf::Right(inlay_hint_capabilities) => match inlay_hint_capabilities {
                lsp::InlayHintServerCapabilities::Options(_) => true,
                lsp::InlayHintServerCapabilities::RegistrationOptions(_) => false,
            },
        }
    }

    fn to_lsp(
        &self,
        path: &Path,
        buffer: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::InlayHintParams> {
        Ok(lsp::InlayHintParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: file_path_to_lsp_url(path)?,
            },
            range: range_to_lsp(self.range.to_point_utf16(buffer))?,
            work_done_progress_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<Vec<lsp::InlayHint>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> anyhow::Result<Vec<InlayHint>> {
        let (lsp_adapter, lsp_server) =
            language_server_for_buffer(&lsp_store, &buffer, server_id, &mut cx)?;
        // `typescript-language-server` adds padding to the left for type hints, turning
        // `const foo: boolean` into `const foo : boolean` which looks odd.
        // `rust-analyzer` does not have the padding for this case, and we have to accommodate both.
        //
        // We could trim the whole string, but being pessimistic on par with the situation above,
        // there might be a hint with multiple whitespaces at the end(s) which we need to display properly.
        // Hence let's use a heuristic first to handle the most awkward case and look for more.
        let force_no_type_left_padding =
            lsp_adapter.name.0.as_ref() == "typescript-language-server";

        let hints = message.unwrap_or_default().into_iter().map(|lsp_hint| {
            let resolve_state = if InlayHints::can_resolve_inlays(&lsp_server.capabilities()) {
                ResolveState::CanResolve(lsp_server.server_id(), lsp_hint.data.clone())
            } else {
                ResolveState::Resolved
            };

            let buffer = buffer.clone();
            cx.spawn(async move |cx| {
                InlayHints::lsp_to_project_hint(
                    lsp_hint,
                    &buffer,
                    server_id,
                    resolve_state,
                    force_no_type_left_padding,
                    cx,
                )
                .await
            })
        });
        future::join_all(hints)
            .await
            .into_iter()
            .collect::<anyhow::Result<_>>()
            .context("lsp to project inlay hints conversion")
    }
}

#[async_trait(?Send)]
impl LspCommand for GetCodeLens {
    type Response = Vec<CodeAction>;
    type LspRequest = lsp::CodeLensRequest;

    fn display_name(&self) -> &str {
        "Code Lens"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        capabilities
            .server_capabilities
            .code_lens_provider
            .as_ref()
            .map_or(false, |code_lens_options| {
                code_lens_options.resolve_provider.unwrap_or(false)
            })
    }

    fn to_lsp(
        &self,
        path: &Path,
        _: &Buffer,
        _: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::CodeLensParams> {
        Ok(lsp::CodeLensParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: file_path_to_lsp_url(path)?,
            },
            work_done_progress_params: lsp::WorkDoneProgressParams::default(),
            partial_result_params: lsp::PartialResultParams::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<Vec<lsp::CodeLens>>,
        lsp_store: Entity<LspStore>,
        buffer: Entity<Buffer>,
        server_id: LanguageServerId,
        mut cx: AsyncApp,
    ) -> anyhow::Result<Vec<CodeAction>> {
        let snapshot = buffer.read_with(&mut cx, |buffer, _| buffer.snapshot())?;
        let language_server = cx.update(|cx| {
            lsp_store
                .read(cx)
                .language_server_for_id(server_id)
                .with_context(|| {
                    format!("Missing the language server that just returned a response {server_id}")
                })
        })??;
        let server_capabilities = language_server.capabilities();
        let available_commands = server_capabilities
            .execute_command_provider
            .as_ref()
            .map(|options| options.commands.as_slice())
            .unwrap_or_default();
        Ok(message
            .unwrap_or_default()
            .into_iter()
            .filter(|code_lens| {
                code_lens
                    .command
                    .as_ref()
                    .is_none_or(|command| available_commands.contains(&command.command))
            })
            .map(|code_lens| {
                let code_lens_range = range_from_lsp(code_lens.range);
                let start = snapshot.clip_point_utf16(code_lens_range.start, Bias::Left);
                let end = snapshot.clip_point_utf16(code_lens_range.end, Bias::Right);
                let range = snapshot.anchor_before(start)..snapshot.anchor_after(end);
                CodeAction {
                    server_id,
                    range,
                    lsp_action: LspAction::CodeLens(code_lens),
                    resolved: false,
                }
            })
            .collect())
    }
}

#[async_trait(?Send)]
impl LspCommand for LinkedEditingRange {
    type Response = Vec<Range<Anchor>>;
    type LspRequest = lsp::request::LinkedEditingRange;

    fn display_name(&self) -> &str {
        "Linked editing range"
    }

    fn check_capabilities(&self, capabilities: AdapterServerCapabilities) -> bool {
        let Some(linked_editing_options) = &capabilities
            .server_capabilities
            .linked_editing_range_provider
        else {
            return false;
        };
        if let LinkedEditingRangeServerCapabilities::Simple(false) = linked_editing_options {
            return false;
        }
        true
    }

    fn to_lsp(
        &self,
        path: &Path,
        buffer: &Buffer,
        _server: &Arc<LanguageServer>,
        _: &App,
    ) -> Result<lsp::LinkedEditingRangeParams> {
        let position = self.position.to_point_utf16(&buffer.snapshot());
        Ok(lsp::LinkedEditingRangeParams {
            text_document_position_params: make_lsp_text_document_position(path, position)?,
            work_done_progress_params: Default::default(),
        })
    }

    async fn response_from_lsp(
        self,
        message: Option<lsp::LinkedEditingRanges>,
        _: Entity<LspStore>,
        buffer: Entity<Buffer>,
        _server_id: LanguageServerId,
        cx: AsyncApp,
    ) -> Result<Vec<Range<Anchor>>> {
        if let Some(lsp::LinkedEditingRanges { mut ranges, .. }) = message {
            ranges.sort_by_key(|range| range.start);

            buffer.read_with(&cx, |buffer, _| {
                ranges
                    .into_iter()
                    .map(|range| {
                        let start =
                            buffer.clip_point_utf16(point_from_lsp(range.start), Bias::Left);
                        let end = buffer.clip_point_utf16(point_from_lsp(range.end), Bias::Left);
                        buffer.anchor_before(start)..buffer.anchor_after(end)
                    })
                    .collect()
            })
        } else {
            Ok(vec![])
        }
    }
}
