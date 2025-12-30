use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const MODEL_REQUESTS_USAGE_LIMIT_HEADER_NAME: &str = "x-vector-model-requests-usage-limit";
pub const MODEL_REQUESTS_USAGE_AMOUNT_HEADER_NAME: &str = "x-vector-model-requests-usage-amount";

pub const CLIENT_SUPPORTS_EXA_WEB_SEARCH_PROVIDER_HEADER_NAME: &str =
    "x-vector-client-supports-exa-web-search-provider";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionMode {
    Normal,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionIntent {
    UserPrompt,
    ToolResults,
    ThreadSummarization,
    ThreadContextSummarization,
    InlineAssist,
    TerminalInlineAssist,
    GenerateGitCommitMessage,
    CreateFile,
    EditFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum CompletionRequestStatus {
    Queued { position: usize },
    Started,
    Failed {
        code: String,
        message: String,
        request_id: String,
    },
    UsageUpdated { amount: u32, limit: UsageLimit },
    ToolUseLimitReached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum UsageLimit {
    Limited { limit: u32 },
    Unlimited,
}

impl fmt::Display for UsageLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageLimit::Limited { limit } => write!(f, "{limit}"),
            UsageLimit::Unlimited => write!(f, "unlimited"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum UsageLimitParseError {
    #[error("invalid usage limit: {0}")]
    Invalid(String),
}

impl FromStr for UsageLimit {
    type Err = UsageLimitParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("unlimited") {
            return Ok(UsageLimit::Unlimited);
        }

        let s = s.strip_prefix("limited:").unwrap_or(s);
        let limit = s
            .parse::<u32>()
            .map_err(|_| UsageLimitParseError::Invalid(s.to_string()))?;
        Ok(UsageLimit::Limited { limit })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchBody {
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchResponse {
    pub results: Vec<WebSearchResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
}
