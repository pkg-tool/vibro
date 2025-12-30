use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub seq: u64,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub seq: u64,
    pub request_seq: u64,
    pub success: bool,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub seq: u64,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Message {
    Request(Request),
    Response(Response),
    Event(Event),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    #[serde(default)]
    pub error: Option<MessageDetails>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetails {
    #[serde(default)]
    pub id: Option<i64>,
    pub format: String,
    #[serde(default)]
    pub variables: Option<HashMap<String, String>>,
    #[serde(default)]
    pub show_user: Option<bool>,
}

