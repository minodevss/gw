use std::borrow::Cow;

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::raw::RawJson;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CacheKind {
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CacheControl {
    #[serde(rename = "type")]
    pub(super) kind: CacheKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) ttl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) scope: Option<String>,
}

impl CacheControl {
    pub(super) fn ephemeral() -> Self {
        Self {
            kind: CacheKind::Ephemeral,
            ttl: None,
            scope: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RoleIn {
    User,
    Assistant,
    System,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageIn {
    pub(super) role: RoleIn,
    pub(super) content: Box<RawValue>,
    pub(super) output_config: Option<IgnoredAny>,
}

#[derive(Deserialize)]
pub(super) struct BlockHead {
    #[serde(rename = "type")]
    pub(super) kind: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum MediaSourceIn {
    Base64 {
        media_type: String,
        data: String,
    },
    Url {
        url: String,
    },
    File {
        #[serde(rename = "file_id")]
        _file_id: IgnoredAny,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TextIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) text: String,
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) source: MediaSourceIn,
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) source: MediaSourceIn,
    pub(super) title: Option<String>,
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThinkingBlockIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) thinking: String,
    pub(super) signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RedactedThinkingIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) data: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolUseIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: RawJson,
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolResultIn {
    #[serde(rename = "type")]
    _kind: IgnoredAny,
    pub(super) tool_use_id: String,
    pub(super) content: Option<Box<RawValue>>,
    pub(super) is_error: Option<bool>,
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
pub(super) struct ToolHead {
    #[serde(rename = "type")]
    pub(super) kind: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolIn {
    #[serde(rename = "type")]
    _kind: Option<IgnoredAny>,
    pub(super) name: String,
    pub(super) description: Option<String>,
    pub(super) input_schema: RawJson,
    pub(super) strict: Option<bool>,
    pub(super) cache_control: Option<CacheControl>,
    pub(super) defer_loading: Option<IgnoredAny>,
    pub(super) allowed_callers: Option<IgnoredAny>,
    pub(super) input_examples: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ToolChoiceIn {
    Auto {
        disable_parallel_tool_use: Option<bool>,
    },
    Any {
        disable_parallel_tool_use: Option<bool>,
    },
    Tool {
        name: String,
        disable_parallel_tool_use: Option<bool>,
    },
    None,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ThinkingIn {
    Adaptive {
        display: Option<String>,
    },
    Enabled {
        budget_tokens: u32,
        #[serde(rename = "display")]
        _display: Option<IgnoredAny>,
    },
    Disabled,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FormatIn {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) schema: RawJson,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OutputConfigIn {
    pub(super) effort: Option<String>,
    pub(super) format: Option<FormatIn>,
    pub(super) task_budget: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MetadataIn {
    #[serde(rename = "user_id")]
    pub(super) _user_id: Option<IgnoredAny>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct UsageIn {
    #[serde(rename = "input_tokens")]
    pub(super) input: Option<u64>,
    #[serde(rename = "output_tokens")]
    pub(super) output: Option<u64>,
    #[serde(rename = "cache_creation_input_tokens")]
    pub(super) cache_write: Option<u64>,
    #[serde(rename = "cache_read_input_tokens")]
    pub(super) cache_read: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct ResponseIn {
    pub(super) id: String,
    pub(super) model: String,
    pub(super) content: Vec<Box<RawValue>>,
    pub(super) stop_reason: Option<String>,
    #[serde(default)]
    pub(super) usage: UsageIn,
}

#[derive(Deserialize)]
pub(super) struct ResponseTextIn {
    pub(super) text: String,
}

#[derive(Deserialize)]
pub(super) struct ResponseThinkingIn {
    pub(super) thinking: String,
    pub(super) signature: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ResponseRedactedIn {
    pub(super) data: String,
}

#[derive(Deserialize)]
pub(super) struct ResponseToolUseIn {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) input: RawJson,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StreamEventIn {
    MessageStart {
        message: MessageStartIn,
    },
    ContentBlockStart {
        index: u32,
        content_block: BlockStartIn,
    },
    ContentBlockDelta {
        index: u32,
        delta: DeltaIn,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaIn,
        usage: Option<UsageIn>,
    },
    MessageStop,
    Ping,
    Error {
        error: ErrorIn,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct MessageStartIn {
    pub(super) id: String,
    pub(super) model: String,
    pub(super) usage: Option<UsageIn>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BlockStartIn {
    Text {
        text: Option<String>,
    },
    Thinking {
        thinking: Option<String>,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Option<Value>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum DeltaIn {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "citations_delta")]
    Citations {},
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(super) struct MessageDeltaIn {
    pub(super) stop_reason: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ErrorIn {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) message: String,
}

#[derive(Serialize)]
pub(super) struct EmptyObject {}

pub(super) enum ToolInputOut {
    Raw(RawJson),
    Empty,
}

impl Serialize for ToolInputOut {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Raw(raw) => raw.serialize(serializer),
            Self::Empty => EmptyObject {}.serialize(serializer),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SourceOut<'a> {
    Base64 { media_type: &'a str, data: &'a str },
    Url { url: &'a str },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BlockOut<'a> {
    Text {
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: SourceOut<'a>,
    },
    Document {
        source: SourceOut<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<&'a str>,
    },
    Thinking {
        thinking: Cow<'a, str>,
        signature: Cow<'a, str>,
    },
    RedactedThinking {
        data: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: ToolInputOut,
    },
    ToolResult {
        tool_use_id: &'a str,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        content: Vec<BlockOut<'a>>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

impl BlockOut<'_> {
    pub(super) fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }
}

#[derive(Serialize)]
pub(super) struct MessageOut<'a> {
    pub(super) role: &'static str,
    pub(super) content: Vec<BlockOut<'a>>,
}

#[derive(Serialize)]
pub(super) struct ToolOut<'a> {
    pub(super) name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<&'a str>,
    pub(super) input_schema: &'a RawJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToolChoiceOut<'a> {
    Auto {
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    Any {
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    Tool {
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    None,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ThinkingOut {
    Adaptive,
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(Serialize)]
pub(super) struct FormatOut<'a> {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) schema: &'a RawJson,
}

#[derive(Serialize)]
pub(super) struct OutputConfigOut<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) format: Option<FormatOut<'a>>,
}

#[derive(Serialize)]
pub(super) struct MetadataOut<'a> {
    pub(super) user_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct RequestOut<'a> {
    pub(super) model: &'a str,
    pub(super) max_tokens: u32,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thinking: Option<ThinkingOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output_config: Option<OutputConfigOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f64>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub(super) stop_sequences: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) speed: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) metadata: Option<MetadataOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<ToolChoiceOut<'a>>,
    pub(super) cache_control: CacheControl,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<ToolOut<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) system: Vec<BlockOut<'a>>,
    pub(super) messages: Vec<MessageOut<'a>>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct UsageOut {
    #[serde(rename = "input_tokens")]
    pub(super) input: u64,
    #[serde(rename = "cache_creation_input_tokens")]
    pub(super) cache_write: u64,
    #[serde(rename = "cache_read_input_tokens")]
    pub(super) cache_read: u64,
    #[serde(rename = "output_tokens")]
    pub(super) output: u64,
}

#[derive(Serialize)]
pub(super) struct ResponseOut<'a> {
    pub(super) id: &'a str,
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) role: &'static str,
    pub(super) model: &'a str,
    pub(super) content: Vec<BlockOut<'a>>,
    pub(super) stop_reason: Option<&'static str>,
    pub(super) stop_sequence: Option<&'a str>,
    pub(super) usage: UsageOut,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BlockStartOut<'a> {
    Text {
        text: &'static str,
    },
    Thinking {
        thinking: &'static str,
        signature: &'static str,
    },
    RedactedThinking {
        data: &'a str,
    },
    ToolUse {
        id: &'a str,
        name: &'a str,
        input: EmptyObject,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum DeltaOut<'a> {
    #[serde(rename = "text_delta")]
    Text { text: &'a str },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: &'a str },
    #[serde(rename = "signature_delta")]
    Signature { signature: &'a str },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: &'a str },
}

#[derive(Serialize)]
pub(super) struct MessageDeltaOut {
    pub(super) stop_reason: &'static str,
    pub(super) stop_sequence: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StreamEventOut<'a> {
    MessageStart {
        message: ResponseOut<'a>,
    },
    ContentBlockStart {
        index: u32,
        content_block: BlockStartOut<'a>,
    },
    ContentBlockDelta {
        index: u32,
        delta: DeltaOut<'a>,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaOut,
        usage: UsageOut,
    },
    MessageStop,
    Ping,
}

impl StreamEventOut<'_> {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::MessageStart { .. } => "message_start",
            Self::ContentBlockStart { .. } => "content_block_start",
            Self::ContentBlockDelta { .. } => "content_block_delta",
            Self::ContentBlockStop { .. } => "content_block_stop",
            Self::MessageDelta { .. } => "message_delta",
            Self::MessageStop => "message_stop",
            Self::Ping => "ping",
        }
    }
}
