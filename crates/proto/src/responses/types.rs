use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::raw::RawJson;

pub(super) type Extra = Map<String, Value>;

#[derive(Debug, Deserialize)]
pub(super) struct Request {
    pub model: String,
    pub instructions: Option<String>,
    pub input: Box<RawValue>,
    pub tools: Option<Vec<Box<RawValue>>>,
    pub tool_choice: Option<Value>,
    pub parallel_tool_calls: Option<bool>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning: Option<ReasoningConfig>,
    pub service_tier: Option<String>,
    pub text: Option<TextConfig>,
    pub stream: Option<bool>,
    pub include: Option<Vec<String>>,
    pub prompt_cache_key: Option<String>,
    pub previous_response_id: Option<Value>,
    pub user: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReasoningConfig {
    pub effort: Option<String>,
    pub summary: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct TextConfig {
    pub format: Option<TextFormat>,
    pub verbosity: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct TextFormat {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: Option<String>,
    pub schema: Option<Box<RawValue>>,
    pub strict: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct TypeHead {
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FunctionTool {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Box<RawValue>>,
    pub strict: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct CustomTool {
    pub name: String,
    pub description: Option<String>,
    pub format: Option<Box<RawValue>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentPart {
    InputText {
        text: String,
        #[serde(flatten)]
        extra: Extra,
    },
    OutputText {
        text: String,
        annotations: Option<Vec<Value>>,
        logprobs: Option<Vec<Value>>,
        #[serde(flatten)]
        extra: Extra,
    },
    InputImage {
        image_url: Option<String>,
        detail: Option<String>,
        file_id: Option<String>,
        #[serde(flatten)]
        extra: Extra,
    },
    InputFile {
        file_data: Option<String>,
        file_url: Option<String>,
        filename: Option<String>,
        file_id: Option<String>,
        #[serde(flatten)]
        extra: Extra,
    },
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageItem {
    pub role: String,
    pub content: Box<RawValue>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReasoningItem {
    pub id: Option<String>,
    #[serde(default)]
    pub summary: Vec<SummaryPart>,
    pub content: Option<Vec<ReasoningTextPart>>,
    pub encrypted_content: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct FunctionCallItem {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct CustomCallItem {
    pub call_id: String,
    pub name: String,
    pub input: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
pub(super) struct CallOutputItem {
    pub call_id: String,
    pub output: Box<RawValue>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SummaryPart {
    SummaryText { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ReasoningTextPart {
    ReasoningText { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OutputPart {
    OutputText {
        text: String,
        #[serde(default)]
        annotations: Vec<Value>,
    },
    Refusal {
        refusal: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum OutputItem {
    Message {
        #[serde(default)]
        id: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        content: Vec<OutputPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    Reasoning {
        #[serde(default)]
        id: String,
        #[serde(default)]
        summary: Vec<SummaryPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ReasoningTextPart>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    FunctionCall {
        #[serde(default)]
        id: String,
        call_id: String,
        name: String,
        arguments: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    CustomToolCall {
        #[serde(default)]
        id: String,
        call_id: String,
        name: String,
        input: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WireError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct IncompleteDetails {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WireUsage {
    pub input_tokens: u64,
    pub input_tokens_details: Option<InputTokensDetails>,
    pub output_tokens: u64,
    pub output_tokens_details: Option<OutputTokensDetails>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct InputTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OutputTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResponseBody {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    pub status: Option<String>,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    pub incomplete_details: Option<IncompleteDetails>,
    pub error: Option<WireError>,
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StreamEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub output_index: Option<u32>,
    pub content_index: Option<u32>,
    pub delta: Option<String>,
    pub item: Option<OutputItem>,
    pub part: Option<Value>,
    pub response: Option<ResponseBody>,
    pub arguments: Option<String>,
    pub input: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ResponseObject {
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub status: &'static str,
    pub error: Option<WireError>,
    pub incomplete_details: Option<IncompleteDetails>,
    pub model: String,
    pub output: Vec<OutputItem>,
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub(super) enum ServerEvent {
    #[serde(rename = "response.created")]
    Created { response: ResponseObject },
    #[serde(rename = "response.in_progress")]
    InProgress { response: ResponseObject },
    #[serde(rename = "response.completed")]
    Completed { response: ResponseObject },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponseObject },
    #[serde(rename = "response.failed")]
    Failed { response: ResponseObject },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: u32, item: OutputItem },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: u32, item: OutputItem },
    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: OutputPart,
    },
    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        part: OutputPart,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
        logprobs: Vec<Value>,
    },
    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
        logprobs: Vec<Value>,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        item_id: String,
        output_index: u32,
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        item_id: String,
        output_index: u32,
        name: String,
        arguments: String,
    },
    #[serde(rename = "response.custom_tool_call_input.delta")]
    CustomToolCallInputDelta {
        item_id: String,
        output_index: u32,
        delta: String,
    },
    #[serde(rename = "response.custom_tool_call_input.done")]
    CustomToolCallInputDone {
        item_id: String,
        output_index: u32,
        input: String,
    },
    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded {
        item_id: String,
        output_index: u32,
        summary_index: u32,
        part: SummaryPart,
    },
    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        item_id: String,
        output_index: u32,
        summary_index: u32,
        part: SummaryPart,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        item_id: String,
        output_index: u32,
        summary_index: u32,
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryTextDone {
        item_id: String,
        output_index: u32,
        summary_index: u32,
        text: String,
    },
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        item_id: String,
        output_index: u32,
        content_index: u32,
        delta: String,
    },
    #[serde(rename = "response.reasoning_text.done")]
    ReasoningTextDone {
        item_id: String,
        output_index: u32,
        content_index: u32,
        text: String,
    },
}

impl ServerEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Created { .. } => "response.created",
            Self::InProgress { .. } => "response.in_progress",
            Self::Completed { .. } => "response.completed",
            Self::Incomplete { .. } => "response.incomplete",
            Self::Failed { .. } => "response.failed",
            Self::OutputItemAdded { .. } => "response.output_item.added",
            Self::OutputItemDone { .. } => "response.output_item.done",
            Self::ContentPartAdded { .. } => "response.content_part.added",
            Self::ContentPartDone { .. } => "response.content_part.done",
            Self::OutputTextDelta { .. } => "response.output_text.delta",
            Self::OutputTextDone { .. } => "response.output_text.done",
            Self::FunctionCallArgumentsDelta { .. } => "response.function_call_arguments.delta",
            Self::FunctionCallArgumentsDone { .. } => "response.function_call_arguments.done",
            Self::CustomToolCallInputDelta { .. } => "response.custom_tool_call_input.delta",
            Self::CustomToolCallInputDone { .. } => "response.custom_tool_call_input.done",
            Self::ReasoningSummaryPartAdded { .. } => "response.reasoning_summary_part.added",
            Self::ReasoningSummaryPartDone { .. } => "response.reasoning_summary_part.done",
            Self::ReasoningSummaryTextDelta { .. } => "response.reasoning_summary_text.delta",
            Self::ReasoningSummaryTextDone { .. } => "response.reasoning_summary_text.done",
            Self::ReasoningTextDelta { .. } => "response.reasoning_text.delta",
            Self::ReasoningTextDone { .. } => "response.reasoning_text.done",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct Numbered<'a> {
    #[serde(flatten)]
    pub event: &'a ServerEvent,
    pub sequence_number: u64,
}

#[derive(Debug, Serialize)]
pub(super) struct RequestBody<'a> {
    pub model: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextOut<'a>>,
    pub store: bool,
    pub include: [&'static str; 1],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<&'a str>,
    pub stream: bool,
    pub input: Vec<InputItem<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToolOut<'a> {
    Function {
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        parameters: &'a RawJson,
        strict: bool,
    },
    Custom {
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<&'a RawJson>,
    },
}

#[derive(Debug, Serialize)]
pub(super) struct ReasoningOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub(super) struct TextOut<'a> {
    pub format: TextFormatOut<'a>,
}

#[derive(Debug, Serialize)]
pub(super) struct TextFormatOut<'a> {
    pub r#type: &'static str,
    pub name: &'a str,
    pub schema: &'a RawJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum InputItem<'a> {
    Message {
        role: &'static str,
        content: Vec<InputPart<'a>>,
    },
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<&'a str>,
        summary: Vec<SummaryPart>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ReasoningTextPart>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<&'a str>,
    },
    FunctionCall {
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
    },
    FunctionCallOutput {
        call_id: &'a str,
        output: Vec<InputPart<'a>>,
    },
    CustomToolCall {
        call_id: &'a str,
        name: &'a str,
        input: &'a str,
    },
    CustomToolCallOutput {
        call_id: &'a str,
        output: Vec<InputPart<'a>>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum InputPart<'a> {
    InputText {
        text: &'a str,
    },
    OutputText {
        text: &'a str,
    },
    InputImage {
        image_url: String,
    },
    InputFile {
        #[serde(skip_serializing_if = "Option::is_none")]
        file_data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_url: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<&'a str>,
    },
}
