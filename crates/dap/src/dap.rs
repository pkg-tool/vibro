pub mod adapters;
pub mod client;
pub mod debugger_settings;
pub mod inline_value;
pub mod protocol;
mod registry;
pub mod transport;

use std::net::Ipv4Addr;

pub use dap_types::*;
pub use registry::{DapLocator, DapRegistry};
pub use task::DebugRequest;
use task::DebugScenario;

use gpui::App;
use serde::Serialize;
use settings::Settings as _;

pub type ScopeId = u64;
pub type VariableReference = u64;
pub type StackFrameId = u64;

#[cfg(any(test, feature = "test-support"))]
pub use adapters::FakeAdapter;
use task::TcpArgumentsTemplate;

pub async fn configure_tcp_connection(
    tcp_connection: TcpArgumentsTemplate,
) -> anyhow::Result<(Ipv4Addr, u16, Option<u64>)> {
    let host = tcp_connection.host();
    let timeout = tcp_connection.timeout;

    let port = if let Some(port) = tcp_connection.port {
        port
    } else {
        transport::TcpTransport::port(&tcp_connection).await?
    };

    Ok((host, port, timeout))
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetrySpawnLocation {
    Gutter,
    ScenarioList,
    Custom,
}

pub fn send_telemetry(scenario: &DebugScenario, location: TelemetrySpawnLocation, cx: &App) {
    let Some(adapter) = cx.global::<DapRegistry>().adapter(&scenario.adapter) else {
        return;
    };
    let dock = debugger_settings::DebuggerSettings::get_global(cx).dock;
    let config = scenario.config.clone();
    let with_build_task = scenario.build.is_some();
    let adapter_name = scenario.adapter.clone();
    cx.spawn(async move |_| {
        let kind = adapter.request_kind(&config).await.ok();
        let kind = kind.and_then(|kind| serde_json::to_value(kind).ok());

        telemetry::event!(
            "Debugger Session Started",
            spawn_location = location,
            with_build_task = with_build_task,
            kind = kind,
            adapter = adapter_name,
            dock_position = dock,
        );
    })
    .detach();
}
