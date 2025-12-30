use std::sync::Arc;

use anyhow::{Ok, Result};
use dap::{
    Capabilities, ContinueArguments, ExceptionFilterOptions, InitializeRequestArguments,
    InitializeRequestArgumentsPathFormat, NextArguments, SetVariableResponse, SourceBreakpoint,
    StepInArguments, StepOutArguments, SteppingGranularity, Variable, VariablesArgumentsFilter,
    requests::{Continue, Next},
};
use serde_json::Value;

pub trait LocalDapCommand: 'static + Send + Sync + std::fmt::Debug {
    type Response: 'static + Send + std::fmt::Debug;
    type DapRequest: 'static + Send + dap::requests::Request;

    fn is_supported(_capabilities: &Capabilities) -> bool {
        true
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments;

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response>;
}

pub trait DapCommand: LocalDapCommand {
    const CACHEABLE: bool = false;
}

impl<T: LocalDapCommand> LocalDapCommand for Arc<T> {
    type Response = T::Response;
    type DapRequest = T::DapRequest;

    fn is_supported(capabilities: &Capabilities) -> bool {
        T::is_supported(capabilities)
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        T::to_dap(self)
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        T::response_from_dap(self, message)
    }
}

impl<T: DapCommand> DapCommand for Arc<T> {
    const CACHEABLE: bool = T::CACHEABLE;
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct StepCommand {
    pub thread_id: u64,
    pub granularity: Option<SteppingGranularity>,
    pub single_thread: Option<bool>,
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct NextCommand {
    pub inner: StepCommand,
}

impl LocalDapCommand for NextCommand {
    type Response = <Next as dap::requests::Request>::Response;
    type DapRequest = Next;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        NextArguments {
            thread_id: self.inner.thread_id,
            single_thread: self.inner.single_thread,
            granularity: self.inner.granularity,
        }
    }
    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for NextCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct StepInCommand {
    pub inner: StepCommand,
}

impl LocalDapCommand for StepInCommand {
    type Response = <dap::requests::StepIn as dap::requests::Request>::Response;
    type DapRequest = dap::requests::StepIn;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        StepInArguments {
            thread_id: self.inner.thread_id,
            single_thread: self.inner.single_thread,
            target_id: None,
            granularity: self.inner.granularity,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for StepInCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct StepOutCommand {
    pub inner: StepCommand,
}

impl LocalDapCommand for StepOutCommand {
    type Response = <dap::requests::StepOut as dap::requests::Request>::Response;
    type DapRequest = dap::requests::StepOut;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        StepOutArguments {
            thread_id: self.inner.thread_id,
            single_thread: self.inner.single_thread,
            granularity: self.inner.granularity,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for StepOutCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct StepBackCommand {
    pub inner: StepCommand,
}
impl LocalDapCommand for StepBackCommand {
    type Response = <dap::requests::StepBack as dap::requests::Request>::Response;
    type DapRequest = dap::requests::StepBack;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_step_back.unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::StepBackArguments {
            thread_id: self.inner.thread_id,
            single_thread: self.inner.single_thread,
            granularity: self.inner.granularity,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for StepBackCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct ContinueCommand {
    pub thread_id: u64,
    pub single_thread: Option<bool>,
}

impl LocalDapCommand for ContinueCommand {
    type Response = <Continue as dap::requests::Request>::Response;
    type DapRequest = Continue;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        ContinueArguments {
            thread_id: self.thread_id,
            single_thread: self.single_thread,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

impl DapCommand for ContinueCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct PauseCommand {
    pub thread_id: u64,
}

impl LocalDapCommand for PauseCommand {
    type Response = <dap::requests::Pause as dap::requests::Request>::Response;
    type DapRequest = dap::requests::Pause;
    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::PauseArguments {
            thread_id: self.thread_id,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for PauseCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct DisconnectCommand {
    pub restart: Option<bool>,
    pub terminate_debuggee: Option<bool>,
    pub suspend_debuggee: Option<bool>,
}

impl LocalDapCommand for DisconnectCommand {
    type Response = <dap::requests::Disconnect as dap::requests::Request>::Response;
    type DapRequest = dap::requests::Disconnect;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::DisconnectArguments {
            restart: self.restart,
            terminate_debuggee: self.terminate_debuggee,
            suspend_debuggee: self.suspend_debuggee,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for DisconnectCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct TerminateThreadsCommand {
    pub thread_ids: Option<Vec<u64>>,
}

impl LocalDapCommand for TerminateThreadsCommand {
    type Response = <dap::requests::TerminateThreads as dap::requests::Request>::Response;
    type DapRequest = dap::requests::TerminateThreads;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities
            .supports_terminate_threads_request
            .unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::TerminateThreadsArguments {
            thread_ids: self.thread_ids.clone(),
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for TerminateThreadsCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct TerminateCommand {
    pub restart: Option<bool>,
}

impl LocalDapCommand for TerminateCommand {
    type Response = <dap::requests::Terminate as dap::requests::Request>::Response;
    type DapRequest = dap::requests::Terminate;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_terminate_request.unwrap_or_default()
    }
    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::TerminateArguments {
            restart: self.restart,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for TerminateCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct RestartCommand {
    pub raw: serde_json::Value,
}

impl LocalDapCommand for RestartCommand {
    type Response = <dap::requests::Restart as dap::requests::Request>::Response;
    type DapRequest = dap::requests::Restart;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_restart_request.unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::RestartArguments {
            raw: self.raw.clone(),
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for RestartCommand {}

#[derive(Debug, Hash, PartialEq, Eq)]
pub struct VariablesCommand {
    pub variables_reference: u64,
    pub filter: Option<VariablesArgumentsFilter>,
    pub start: Option<u64>,
    pub count: Option<u64>,
}

impl LocalDapCommand for VariablesCommand {
    type Response = Vec<Variable>;
    type DapRequest = dap::requests::Variables;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::VariablesArguments {
            variables_reference: self.variables_reference,
            filter: self.filter,
            start: self.start,
            count: self.count,
            format: None,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.variables)
    }
}

impl DapCommand for VariablesCommand {
    const CACHEABLE: bool = true;
}

#[derive(Debug, Hash, PartialEq, Eq)]
pub(crate) struct SetVariableValueCommand {
    pub name: String,
    pub value: String,
    pub variables_reference: u64,
}
impl LocalDapCommand for SetVariableValueCommand {
    type Response = SetVariableResponse;
    type DapRequest = dap::requests::SetVariable;
    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_set_variable.unwrap_or_default()
    }
    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::SetVariableArguments {
            format: None,
            name: self.name.clone(),
            value: self.value.clone(),
            variables_reference: self.variables_reference,
        }
    }
    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

impl DapCommand for SetVariableValueCommand {}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct RestartStackFrameCommand {
    pub stack_frame_id: u64,
}

impl LocalDapCommand for RestartStackFrameCommand {
    type Response = <dap::requests::RestartFrame as dap::requests::Request>::Response;
    type DapRequest = dap::requests::RestartFrame;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_restart_frame.unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::RestartFrameArguments {
            frame_id: self.stack_frame_id,
        }
    }

    fn response_from_dap(
        &self,
        _message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(())
    }
}

impl DapCommand for RestartStackFrameCommand {}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ModulesCommand;

impl LocalDapCommand for ModulesCommand {
    type Response = Vec<dap::Module>;
    type DapRequest = dap::requests::Modules;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities.supports_modules_request.unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::ModulesArguments {
            start_module: None,
            module_count: None,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.modules)
    }
}

impl DapCommand for ModulesCommand {
    const CACHEABLE: bool = true;
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct LoadedSourcesCommand;

impl LocalDapCommand for LoadedSourcesCommand {
    type Response = Vec<dap::Source>;
    type DapRequest = dap::requests::LoadedSources;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities
            .supports_loaded_sources_request
            .unwrap_or_default()
    }
    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::LoadedSourcesArguments {}
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.sources)
    }
}

impl DapCommand for LoadedSourcesCommand {
    const CACHEABLE: bool = true;
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct StackTraceCommand {
    pub thread_id: u64,
    pub start_frame: Option<u64>,
    pub levels: Option<u64>,
}

impl LocalDapCommand for StackTraceCommand {
    type Response = Vec<dap::StackFrame>;
    type DapRequest = dap::requests::StackTrace;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::StackTraceArguments {
            thread_id: self.thread_id,
            start_frame: self.start_frame,
            levels: self.levels,
            format: None,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.stack_frames)
    }
}

impl DapCommand for StackTraceCommand {
    const CACHEABLE: bool = true;
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ScopesCommand {
    pub stack_frame_id: u64,
}

impl LocalDapCommand for ScopesCommand {
    type Response = Vec<dap::Scope>;
    type DapRequest = dap::requests::Scopes;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::ScopesArguments {
            frame_id: self.stack_frame_id,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.scopes)
    }
}

impl DapCommand for ScopesCommand {
    const CACHEABLE: bool = true;
}

impl LocalDapCommand for super::session::CompletionsQuery {
    type Response = dap::CompletionsResponse;
    type DapRequest = dap::requests::Completions;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::CompletionsArguments {
            text: self.query.clone(),
            frame_id: self.frame_id,
            column: self.column,
            line: None,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities
            .supports_completions_request
            .unwrap_or_default()
    }
}

impl DapCommand for super::session::CompletionsQuery {
    const CACHEABLE: bool = true;
}

#[derive(Debug, Clone)]
pub(crate) struct EvaluateCommand {
    pub expression: String,
    pub frame_id: Option<u64>,
    pub context: Option<dap::EvaluateArgumentsContext>,
}

impl LocalDapCommand for EvaluateCommand {
    type Response = dap::EvaluateResponse;
    type DapRequest = dap::requests::Evaluate;
    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::EvaluateArguments {
            expression: self.expression.clone(),
            frame_id: self.frame_id,
            context: self.context.clone(),
            format: None,
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}
impl DapCommand for EvaluateCommand {}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct ThreadsCommand;

impl LocalDapCommand for ThreadsCommand {
    type Response = Vec<dap::Thread>;
    type DapRequest = dap::requests::Threads;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        ()
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.threads)
    }
}

impl DapCommand for ThreadsCommand {
    const CACHEABLE: bool = true;
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub(super) struct Initialize {
    pub(super) adapter_id: String,
}

fn dap_client_capabilities(adapter_id: String) -> InitializeRequestArguments {
    InitializeRequestArguments {
        client_id: Some("vector".to_owned()),
        client_name: Some("Vector".to_owned()),
        adapter_id,
        locale: Some("en-US".to_owned()),
        path_format: Some(InitializeRequestArgumentsPathFormat::Path),
        supports_variable_type: Some(true),
        supports_variable_paging: Some(false),
        supports_run_in_terminal_request: Some(true),
        supports_memory_references: Some(true),
        supports_progress_reporting: Some(false),
        supports_invalidated_event: Some(false),
        lines_start_at1: Some(true),
        columns_start_at1: Some(true),
        supports_memory_event: Some(false),
        supports_args_can_be_interpreted_by_shell: Some(false),
        supports_start_debugging_request: Some(true),
    }
}

impl LocalDapCommand for Initialize {
    type Response = Capabilities;
    type DapRequest = dap::requests::Initialize;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap_client_capabilities(self.adapter_id.clone())
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub(super) struct ConfigurationDone {}

impl LocalDapCommand for ConfigurationDone {
    type Response = ();
    type DapRequest = dap::requests::ConfigurationDone;

    fn is_supported(capabilities: &Capabilities) -> bool {
        capabilities
            .supports_configuration_done_request
            .unwrap_or_default()
    }

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::ConfigurationDoneArguments {}
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub(super) struct Launch {
    pub(super) raw: Value,
}

impl LocalDapCommand for Launch {
    type Response = ();
    type DapRequest = dap::requests::Launch;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::LaunchRequestArguments {
            raw: self.raw.clone(),
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

#[derive(Clone, Debug, Hash, PartialEq)]
pub(super) struct Attach {
    pub(super) raw: Value,
}

impl LocalDapCommand for Attach {
    type Response = ();
    type DapRequest = dap::requests::Attach;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::AttachRequestArguments {
            raw: self.raw.clone(),
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message)
    }
}

#[derive(Clone, Debug)]
pub(super) struct SetBreakpoints {
    pub(super) source: dap::Source,
    pub(super) breakpoints: Vec<SourceBreakpoint>,
    pub(super) source_modified: Option<bool>,
}

impl LocalDapCommand for SetBreakpoints {
    type Response = Vec<dap::Breakpoint>;
    type DapRequest = dap::requests::SetBreakpoints;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        dap::SetBreakpointsArguments {
            lines: None,
            source_modified: self.source_modified,
            source: self.source.clone(),
            breakpoints: Some(self.breakpoints.clone()),
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.breakpoints)
    }
}
#[derive(Clone, Debug)]
pub(super) enum SetExceptionBreakpoints {
    Plain {
        filters: Vec<String>,
    },
    WithOptions {
        filters: Vec<ExceptionFilterOptions>,
    },
}

impl LocalDapCommand for SetExceptionBreakpoints {
    type Response = Vec<dap::Breakpoint>;
    type DapRequest = dap::requests::SetExceptionBreakpoints;

    fn to_dap(&self) -> <Self::DapRequest as dap::requests::Request>::Arguments {
        match self {
            SetExceptionBreakpoints::Plain { filters } => dap::SetExceptionBreakpointsArguments {
                filters: filters.clone(),
                exception_options: None,
                filter_options: None,
            },
            SetExceptionBreakpoints::WithOptions { filters } => {
                dap::SetExceptionBreakpointsArguments {
                    filters: vec![],
                    filter_options: Some(filters.clone()),
                    exception_options: None,
                }
            }
        }
    }

    fn response_from_dap(
        &self,
        message: <Self::DapRequest as dap::requests::Request>::Response,
    ) -> Result<Self::Response> {
        Ok(message.breakpoints.unwrap_or_default())
    }
}
