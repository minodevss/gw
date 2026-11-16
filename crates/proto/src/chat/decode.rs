use serde_json::Value;

use super::types::{
    CallType, Completion, ContentPart, FormatType, ImageUrl, Message, MessageRole, Request,
    ResponseFormat, StringOrList, ToolCall, ToolDef, Usage,
};
use crate::codec::DecodeError;
use crate::ir::{
    self, Document, Effort, Item, Media, Origin, Part, Reasoning, Speed, StopReason,
    StructuredOutput, Tool, ToolChoice, ToolKind,
};
use crate::raw::RawJson;
use crate::reject::Rejection;

const EMPTY_PARAMETERS: &str = r#"{"type":"object","properties":{}}"#;

pub fn decode_request(body: &[u8]) -> Result<ir::Request, Rejection> {
    let wire: Request = serde_json::from_slice(body)
        .map_err(|error| Rejection::unsupported(format!("invalid chat request: {error}")))?;
    reject_unsupported_fields(&wire)?;

    let max_output_tokens = match (wire.max_completion_tokens, wire.max_tokens) {
        (Some(_), Some(_)) => {
            return Err(Rejection::unsupported(
                "max_tokens together with max_completion_tokens",
            ));
        }
        (completion, legacy) => completion.or(legacy),
    };
    let (system, items) = decode_messages(wire.messages)?;
    let tools = wire
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(decode_tool)
        .collect::<Result<Vec<_>, _>>()?;
    let effort = match wire.reasoning_effort.as_deref() {
        Some(value) => {
            Some(Effort::parse(value).ok_or_else(|| Rejection::unsupported("reasoning_effort"))?)
        }
        None => None,
    };

    Ok(ir::Request {
        model: wire.model,
        system,
        items,
        tools,
        tool_choice: wire
            .tool_choice
            .as_ref()
            .map(decode_tool_choice)
            .transpose()?,
        parallel_tool_calls: wire.parallel_tool_calls,
        max_output_tokens,
        temperature: wire.temperature,
        top_p: wire.top_p,
        stop: match wire.stop {
            None => Vec::new(),
            Some(StringOrList::Text(text)) => vec![text],
            Some(StringOrList::List(list)) => list,
        },
        effort,
        thinking: None,
        reasoning_summary: false,
        speed: decode_service_tier(wire.service_tier.as_deref())?,
        structured_output: wire
            .response_format
            .map(decode_response_format)
            .transpose()?
            .flatten(),
        stream: wire.stream.unwrap_or(false),
        cache_key: wire.prompt_cache_key,
        user: wire.user,
    })
}

pub fn decode_response(body: &[u8]) -> Result<ir::Response, DecodeError> {
    let wire: Completion = serde_json::from_slice(body)?;
    let mut choices = wire.choices.into_iter();
    let (Some(choice), None) = (choices.next(), choices.next()) else {
        return Err(DecodeError::Event(
            "chat completion must have exactly one choice".to_owned(),
        ));
    };
    let finish = choice
        .finish_reason
        .as_deref()
        .ok_or_else(|| DecodeError::Event("choice without finish_reason".to_owned()))?;
    let mut stop = stop_from_wire(finish)?;

    let message = choice.message;
    let mut output = Vec::new();
    if let Some(text) = message.reasoning_content.filter(|text| !text.is_empty()) {
        output.push(plain_reasoning(text));
    }
    let mut parts = Vec::new();
    if let Some(text) = message.content.filter(|text| !text.is_empty()) {
        parts.push(Part::Text(text));
    }
    if let Some(refusal) = message.refusal.filter(|text| !text.is_empty()) {
        parts.push(Part::Text(refusal));
        if stop == StopReason::EndTurn {
            stop = StopReason::Refusal;
        }
    }
    if !parts.is_empty() {
        output.push(Item::Message {
            role: ir::Role::Assistant,
            parts,
        });
    }
    for call in message.tool_calls {
        output
            .push(Item::ToolCall(decode_tool_call(call).map_err(
                |rejection| DecodeError::Event(rejection.to_string()),
            )?));
    }

    Ok(ir::Response {
        id: wire.id,
        model: wire.model,
        output,
        stop,
        usage: wire.usage.map(usage_from_wire).unwrap_or_default(),
    })
}

pub(super) fn stop_from_wire(reason: &str) -> Result<StopReason, DecodeError> {
    Ok(match reason {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        // Z.ai adds "sensitive" and "model_context_window_exceeded" to the OpenAI reasons.
        // https://docs.z.ai/api-reference/llm/chat-completion
        "content_filter" | "sensitive" => StopReason::ContentFilter,
        "model_context_window_exceeded" => StopReason::ContextWindow,
        other => {
            return Err(DecodeError::Event(format!("unknown finish_reason {other}")));
        }
    })
}

pub(super) fn usage_from_wire(usage: Usage) -> ir::Usage {
    // Moonshot's usage schema also defines cached_tokens at the top level.
    // https://platform.kimi.ai/docs/api/chat
    let cached = usage
        .prompt_tokens_details
        .map(|details| details.cached_tokens)
        .or(usage.cached_tokens)
        .unwrap_or(0);
    ir::Usage {
        input_uncached: usage.prompt_tokens.saturating_sub(cached),
        cache_read: cached,
        cache_write: 0,
        output: usage.completion_tokens,
        reasoning: usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens),
    }
}

pub(super) fn plain_reasoning(text: String) -> Item {
    Item::Reasoning(Reasoning {
        text,
        summary: Vec::new(),
        origin: Origin::Plain,
    })
}

pub(super) fn decode_tool_call(call: ToolCall) -> Result<ir::ToolCall, Rejection> {
    match (call.kind, call.function) {
        (CallType::Function, Some(function)) => Ok(ir::ToolCall {
            id: call.id,
            name: function.name,
            kind: ToolKind::Function,
            arguments: function.arguments,
        }),
        (CallType::Function, None) => Err(Rejection::unsupported(
            "function tool call without function",
        )),
        (CallType::Custom, _) => Err(Rejection::unsupported("custom tool call")),
    }
}

fn reject_unsupported_fields(wire: &Request) -> Result<(), Rejection> {
    if wire.n.is_some_and(|n| n != 1) {
        return Err(Rejection::unsupported("n other than 1"));
    }
    if wire.stream_options.is_some() && wire.stream != Some(true) {
        return Err(Rejection::unsupported("stream_options without stream"));
    }
    if wire.store == Some(true) {
        return Err(Rejection::unsupported("store"));
    }
    if wire.logprobs == Some(true) {
        return Err(Rejection::unsupported("logprobs"));
    }
    let present = [
        ("frequency_penalty", &wire.frequency_penalty),
        ("presence_penalty", &wire.presence_penalty),
        ("seed", &wire.seed),
        ("top_logprobs", &wire.top_logprobs),
        ("logit_bias", &wire.logit_bias),
        ("modalities", &wire.modalities),
        ("audio", &wire.audio),
        ("prediction", &wire.prediction),
        ("web_search_options", &wire.web_search_options),
        ("functions", &wire.functions),
        ("function_call", &wire.function_call),
        ("metadata", &wire.metadata),
        ("safety_identifier", &wire.safety_identifier),
        ("prompt_cache_retention", &wire.prompt_cache_retention),
        ("verbosity", &wire.verbosity),
    ];
    for (name, value) in present {
        if value.as_ref().is_some_and(|value| !value.is_null()) {
            return Err(Rejection::unsupported(name));
        }
    }
    Ok(())
}

fn decode_messages(messages: Vec<Message>) -> Result<(Vec<Part>, Vec<Item>), Rejection> {
    let mut system = Vec::new();
    let mut items = Vec::new();
    for message in messages {
        reject_unsupported_message_fields(&message)?;
        match message.role {
            MessageRole::System | MessageRole::Developer => {
                let parts = text_parts(message.content)?;
                if items.is_empty() {
                    system.extend(parts);
                } else {
                    items.push(Item::System { parts });
                }
            }
            MessageRole::User => {
                items.push(Item::Message {
                    role: ir::Role::User,
                    parts: user_parts(message.content)?,
                });
            }
            MessageRole::Assistant => {
                if let Some(text) = message.reasoning_content.filter(|text| !text.is_empty()) {
                    items.push(plain_reasoning(text));
                }
                let mut parts = text_parts(message.content)?;
                parts.retain(|part| !matches!(part, Part::Text(text) if text.is_empty()));
                if !parts.is_empty() {
                    items.push(Item::Message {
                        role: ir::Role::Assistant,
                        parts,
                    });
                }
                for call in message.tool_calls {
                    items.push(Item::ToolCall(decode_tool_call(call)?));
                }
            }
            MessageRole::Tool => {
                let call_id = message
                    .tool_call_id
                    .ok_or_else(|| Rejection::unsupported("tool message without tool_call_id"))?;
                items.push(Item::ToolResult(ir::ToolResult {
                    call_id,
                    kind: ToolKind::Function,
                    parts: text_parts(message.content)?,
                    is_error: false,
                }));
            }
            MessageRole::Function => return Err(Rejection::unsupported("function role message")),
        }
    }
    Ok((system, items))
}

fn reject_unsupported_message_fields(message: &Message) -> Result<(), Rejection> {
    let present = [
        ("message name", &message.name),
        ("message refusal", &message.refusal),
        ("message function_call", &message.function_call),
        ("message audio", &message.audio),
    ];
    for (name, value) in present {
        if value.as_ref().is_some_and(|value| !value.is_null()) {
            return Err(Rejection::unsupported(name));
        }
    }
    let assistant = message.role == MessageRole::Assistant;
    if !assistant && (!message.tool_calls.is_empty() || message.reasoning_content.is_some()) {
        return Err(Rejection::unsupported(
            "tool_calls or reasoning_content outside an assistant message",
        ));
    }
    if message.role != MessageRole::Tool && message.tool_call_id.is_some() {
        return Err(Rejection::unsupported(
            "tool_call_id outside a tool message",
        ));
    }
    Ok(())
}

fn text_parts(content: Option<StringOrList<ContentPart>>) -> Result<Vec<Part>, Rejection> {
    match content {
        None => Ok(Vec::new()),
        Some(StringOrList::Text(text)) => Ok(vec![Part::Text(text)]),
        Some(StringOrList::List(parts)) => parts
            .into_iter()
            .map(|part| match part {
                ContentPart::Text { text } => Ok(Part::Text(text)),
                ContentPart::ImageUrl { .. }
                | ContentPart::File { .. }
                | ContentPart::InputAudio { .. }
                | ContentPart::Refusal { .. } => {
                    Err(Rejection::unsupported("non-text part in this message role"))
                }
            })
            .collect(),
    }
}

fn user_parts(content: Option<StringOrList<ContentPart>>) -> Result<Vec<Part>, Rejection> {
    match content {
        None => Err(Rejection::unsupported("user message without content")),
        Some(StringOrList::Text(text)) => Ok(vec![Part::Text(text)]),
        Some(StringOrList::List(parts)) => parts.into_iter().map(user_part).collect(),
    }
}

fn user_part(part: ContentPart) -> Result<Part, Rejection> {
    match part {
        ContentPart::Text { text } => Ok(Part::Text(text)),
        ContentPart::ImageUrl {
            image_url: ImageUrl { url, detail },
        } => {
            if detail.is_some_and(|detail| detail != "auto") {
                return Err(Rejection::unsupported("image detail"));
            }
            Ok(Part::Image(media_from_url(&url)?))
        }
        ContentPart::File { file } => {
            if file.file_id.is_some() {
                return Err(Rejection::unsupported("file_id"));
            }
            let data = file
                .file_data
                .ok_or_else(|| Rejection::unsupported("file without file_data"))?;
            if !data.starts_with("data:") {
                return Err(Rejection::unsupported("file_data that is not a data url"));
            }
            Ok(Part::Document(Document {
                media: media_from_url(&data)?,
                title: file.filename,
            }))
        }
        ContentPart::InputAudio { .. } => Err(Rejection::unsupported("input_audio")),
        ContentPart::Refusal { .. } => Err(Rejection::unsupported("refusal part in user message")),
    }
}

fn media_from_url(url: &str) -> Result<Media, Rejection> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Ok(Media::Url(url.to_owned()));
    };
    let (header, data) = rest
        .split_once(',')
        .ok_or_else(|| Rejection::unsupported("malformed data url"))?;
    let media_type = header
        .strip_suffix(";base64")
        .ok_or_else(|| Rejection::unsupported("data url that is not base64"))?;
    Ok(Media::Base64 {
        media_type: media_type.to_owned(),
        data: data.to_owned(),
    })
}

fn decode_tool(tool: ToolDef) -> Result<Tool, Rejection> {
    let (CallType::Function, Some(function)) = (tool.kind, tool.function) else {
        return Err(Rejection::unsupported("custom tool"));
    };
    let parameters = match function.parameters {
        Some(parameters) => parameters,
        None => RawJson::parse(EMPTY_PARAMETERS.to_owned())
            .map_err(|error| Rejection::unsupported(error.to_string()))?,
    };
    Ok(Tool::Function {
        name: function.name,
        description: function.description,
        parameters,
        strict: function.strict,
    })
}

fn decode_tool_choice(value: &Value) -> Result<ToolChoice, Rejection> {
    match value {
        Value::String(choice) => match choice.as_str() {
            "auto" => Ok(ToolChoice::Auto),
            "none" => Ok(ToolChoice::None),
            "required" => Ok(ToolChoice::Required),
            _ => Err(Rejection::unsupported("tool_choice")),
        },
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str);
            let name = map
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str);
            match (kind, name, map.len()) {
                (Some("function"), Some(name), 2) => Ok(ToolChoice::Named(name.to_owned())),
                _ => Err(Rejection::unsupported("tool_choice")),
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {
            Err(Rejection::unsupported("tool_choice"))
        }
    }
}

fn decode_service_tier(tier: Option<&str>) -> Result<Option<Speed>, Rejection> {
    match tier {
        None | Some("auto" | "default") => Ok(None),
        Some("priority") => Ok(Some(Speed::Fast)),
        Some(_) => Err(Rejection::unsupported("service_tier")),
    }
}

fn decode_response_format(format: ResponseFormat) -> Result<Option<StructuredOutput>, Rejection> {
    match (format.kind, format.json_schema) {
        (FormatType::Text, None) => Ok(None),
        (FormatType::JsonObject, None) => Err(Rejection::StructuredOutputs),
        (FormatType::JsonSchema, Some(definition)) => {
            if definition.description.is_some() {
                return Err(Rejection::unsupported("response_format description"));
            }
            let schema = definition
                .schema
                .ok_or_else(|| Rejection::unsupported("response_format without schema"))?;
            Ok(Some(StructuredOutput {
                name: Some(definition.name),
                schema,
                strict: definition.strict,
            }))
        }
        (FormatType::Text | FormatType::JsonObject, Some(_)) | (FormatType::JsonSchema, None) => {
            Err(Rejection::unsupported("response_format"))
        }
    }
}
