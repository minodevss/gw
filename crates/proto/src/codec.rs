use bytes::Bytes;

use crate::reject::Rejection;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodeOptions {
    pub cache_key: Option<String>,
    pub default_max_output: Option<u32>,
    pub force_stream: bool,
    pub max_tokens_field: MaxTokensField,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaxTokensField {
    #[default]
    MaxCompletionTokens,
    MaxTokens,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub body: Bytes,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Note {
    CacheTtlDefault,
    ToolErrorFlagDropped,
    StreamForced,
}

impl Note {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CacheTtlDefault => "cache ttl → target default",
            Self::ToolErrorFlagDropped => "tool_result is_error → not representable, content kept",
            Self::StreamForced => "stream forced by upstream",
        }
    }
}

pub type EncodeResult = Result<Encoded, Rejection>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("invalid json: {0}")]
    Json(String),
    #[error("unexpected event: {0}")]
    Event(String),
}

impl From<serde_json::Error> for DecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}
