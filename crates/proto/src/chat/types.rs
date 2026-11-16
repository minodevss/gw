use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::raw::RawJson;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum StringOrList<T> {
    Text(String),
    List(Vec<T>),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for StringOrList<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Shape<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for Shape<T> {
            type Value = StringOrList<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string or an array")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StringOrList::Text(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(StringOrList::Text(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(StringOrList::List(items))
            }
        }

        deserializer.deserialize_any(Shape(PhantomData))
    }
}

impl<T: Serialize> Serialize for StringOrList<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::List(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) model: String,
    pub(super) messages: Vec<Message>,
    pub(super) stream: Option<bool>,
    pub(super) stream_options: Option<StreamOptions>,
    pub(super) max_completion_tokens: Option<u32>,
    pub(super) max_tokens: Option<u32>,
    pub(super) temperature: Option<f64>,
    pub(super) top_p: Option<f64>,
    pub(super) stop: Option<StringOrList<String>>,
    pub(super) reasoning_effort: Option<String>,
    pub(super) service_tier: Option<String>,
    pub(super) response_format: Option<ResponseFormat>,
    pub(super) prompt_cache_key: Option<String>,
    pub(super) user: Option<String>,
    pub(super) tool_choice: Option<Value>,
    pub(super) parallel_tool_calls: Option<bool>,
    pub(super) tools: Option<Vec<ToolDef>>,
    pub(super) n: Option<u64>,
    pub(super) store: Option<bool>,
    pub(super) logprobs: Option<bool>,
    pub(super) frequency_penalty: Option<Value>,
    pub(super) presence_penalty: Option<Value>,
    pub(super) seed: Option<Value>,
    pub(super) top_logprobs: Option<Value>,
    pub(super) logit_bias: Option<Value>,
    pub(super) modalities: Option<Value>,
    pub(super) audio: Option<Value>,
    pub(super) prediction: Option<Value>,
    pub(super) web_search_options: Option<Value>,
    pub(super) functions: Option<Value>,
    pub(super) function_call: Option<Value>,
    pub(super) metadata: Option<Value>,
    pub(super) safety_identifier: Option<Value>,
    pub(super) prompt_cache_retention: Option<Value>,
    pub(super) verbosity: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct OutRequest {
    pub(super) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stop: Option<StringOrList<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) service_tier: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<ToolChoiceOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<ToolDef>,
    pub(super) messages: Vec<Message>,
}

#[derive(Debug)]
pub(super) enum ToolChoiceOut {
    Mode(&'static str),
    Named(String),
}

#[derive(Serialize)]
struct NamedChoice<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: NamedFunction<'a>,
}

#[derive(Serialize)]
struct NamedFunction<'a> {
    name: &'a str,
}

impl Serialize for ToolChoiceOut {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Mode(mode) => serializer.serialize_str(mode),
            Self::Named(name) => NamedChoice {
                kind: "function",
                function: NamedFunction { name },
            }
            .serialize(serializer),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) include_usage: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) include_obfuscation: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MessageRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
    Function,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Message {
    pub(super) role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<StringOrList<ContentPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) refusal: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) function_call: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) audio: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    File { file: FileInput },
    InputAudio { input_audio: Value },
    Refusal { refusal: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageUrl {
    pub(super) url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) file_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) filename: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CallType {
    Function,
    Custom,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolCall {
    pub(super) id: String,
    #[serde(rename = "type")]
    pub(super) kind: CallType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) function: Option<ToolCallFunction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) custom: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolCallFunction {
    pub(super) name: String,
    pub(super) arguments: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolDef {
    #[serde(rename = "type")]
    pub(super) kind: CallType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) function: Option<FunctionDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) custom: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FunctionDef {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parameters: Option<RawJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) strict: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FormatType {
    Text,
    JsonObject,
    JsonSchema,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseFormat {
    #[serde(rename = "type")]
    pub(super) kind: FormatType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) json_schema: Option<JsonSchemaDef>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JsonSchemaDef {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) schema: Option<RawJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) strict: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Completion {
    pub(super) id: String,
    #[serde(default)]
    pub(super) object: String,
    #[serde(default)]
    pub(super) created: u64,
    pub(super) model: String,
    pub(super) choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Choice {
    pub(super) index: u32,
    pub(super) message: ResponseMessage,
    pub(super) finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ResponseMessage {
    #[serde(default)]
    pub(super) role: String,
    pub(super) content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) refusal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Usage {
    #[serde(default)]
    pub(super) prompt_tokens: u64,
    #[serde(default)]
    pub(super) completion_tokens: u64,
    #[serde(default)]
    pub(super) total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) completion_tokens_details: Option<CompletionTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct PromptTokensDetails {
    #[serde(default)]
    pub(super) cached_tokens: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct CompletionTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Chunk {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) object: String,
    #[serde(default)]
    pub(super) created: u64,
    #[serde(default)]
    pub(super) model: String,
    #[serde(default)]
    pub(super) choices: Vec<ChunkChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) usage: Option<Usage>,
    #[serde(default, skip_serializing)]
    pub(super) error: Option<ChunkError>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ChunkChoice {
    pub(super) index: u32,
    #[serde(default)]
    pub(super) delta: Delta,
    pub(super) finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) usage: Option<Usage>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(super) struct Delta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tool_calls: Vec<DeltaToolCall>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DeltaToolCall {
    pub(super) index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub(super) kind: Option<CallType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) function: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct DeltaFunction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) arguments: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ChunkError {
    #[serde(default)]
    pub(super) message: String,
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
}
