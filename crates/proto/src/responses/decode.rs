use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_json::value::RawValue;

use super::types::{
    CallOutputItem, ContentPart, CustomCallItem, CustomTool, Extra, FunctionCallItem, FunctionTool,
    MessageItem, OutputItem, OutputPart, ReasoningConfig, ReasoningItem, ReasoningTextPart,
    Request, ResponseBody, SummaryPart, TextConfig, TypeHead, WireError, WireUsage,
};
use crate::codec::DecodeError;
use crate::envelope;
use crate::error::{ErrorBody, ErrorKind};
use crate::ir::{
    Effort, Item, Media, Origin, Part, Reasoning, Request as IrRequest, Response, Role, Speed,
    StopReason, StructuredOutput, Tool, ToolCall, ToolChoice, ToolKind, ToolResult, Usage,
};
use crate::raw::RawJson;
use crate::reject::Rejection;

const SEALED_PREFIX: &str = "gw1:";
const TRANSPORT_FIELDS: &[&str] = &["store", "stream_options", "client_metadata"];
const ENCRYPTED_REASONING: &str = "reasoning.encrypted_content";

enum Decoded {
    Developer(Vec<Part>),
    Item(Item),
}

pub fn decode_request(body: &[u8]) -> Result<IrRequest, Rejection> {
    let wire: Request = serde_json::from_slice(body).map_err(|error| invalid(&error))?;
    reject_extra(&wire.extra, TRANSPORT_FIELDS, "")?;
    if wire.previous_response_id.is_some() {
        return Err(Rejection::unsupported("previous_response_id"));
    }
    check_include(wire.include.as_deref())?;
    let (effort, reasoning_summary) = decode_reasoning_config(wire.reasoning)?;
    let tools = wire
        .tools
        .unwrap_or_default()
        .iter()
        .map(|raw| decode_tool(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let tool_choice = wire
        .tool_choice
        .as_ref()
        .map(decode_tool_choice)
        .transpose()?;
    let (system, items) = decode_input(&wire.input, wire.instructions)?;
    Ok(IrRequest {
        model: wire.model,
        system,
        items,
        tools,
        tool_choice,
        parallel_tool_calls: wire.parallel_tool_calls,
        max_output_tokens: wire.max_output_tokens,
        temperature: wire.temperature,
        top_p: wire.top_p,
        stop: Vec::new(),
        effort,
        thinking: None,
        reasoning_summary,
        speed: decode_speed(wire.service_tier.as_deref())?,
        structured_output: decode_text_config(wire.text)?,
        stream: wire.stream.unwrap_or(false),
        cache_key: wire.prompt_cache_key,
        user: wire.user,
    })
}

pub fn decode_response(body: &[u8]) -> Result<Response, DecodeError> {
    let wire: ResponseBody = serde_json::from_slice(body)?;
    match wire.status.as_deref() {
        None | Some("completed" | "incomplete") => {}
        Some("failed") => {
            let error = wire.error.as_ref();
            return Err(DecodeError::Event(format!(
                "response failed: {}",
                error
                    .and_then(|e| e.message.as_deref())
                    .unwrap_or("no message")
            )));
        }
        Some(other) => return Err(DecodeError::Event(format!("response status {other}"))),
    }
    let incomplete = wire.status.as_deref() == Some("incomplete");
    let reason = wire
        .incomplete_details
        .as_ref()
        .and_then(|details| details.reason.as_deref());
    let (output, refused, tool_called) = output_items(wire.output);
    let stop = if incomplete {
        incomplete_stop(reason)?
    } else {
        completed_stop(refused, tool_called)
    };
    Ok(Response {
        id: wire.id,
        model: wire.model,
        output,
        stop,
        usage: wire.usage.as_ref().map_or_default(usage),
    })
}

pub(super) fn usage(wire: &WireUsage) -> Usage {
    let details = wire.input_tokens_details.as_ref();
    let cached = details.map_or(0, |d| d.cached_tokens);
    let written = details.and_then(|d| d.cache_write_tokens).unwrap_or(0);
    Usage {
        input_uncached: wire
            .input_tokens
            .saturating_sub(cached)
            .saturating_sub(written),
        cache_read: cached,
        cache_write: written,
        output: wire.output_tokens,
        reasoning: wire
            .output_tokens_details
            .as_ref()
            .and_then(|d| d.reasoning_tokens),
    }
}

pub(super) fn completed_stop(refused: bool, tool_called: bool) -> StopReason {
    if refused {
        StopReason::Refusal
    } else if tool_called {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    }
}

pub(super) fn incomplete_stop(reason: Option<&str>) -> Result<StopReason, DecodeError> {
    match reason {
        Some("max_output_tokens") => Ok(StopReason::MaxTokens),
        Some("content_filter") => Ok(StopReason::ContentFilter),
        Some(other) => Err(DecodeError::Event(format!("incomplete reason {other}"))),
        None => Err(DecodeError::Event("incomplete without reason".to_owned())),
    }
}

pub(super) fn error_body(code: Option<&str>, message: Option<&str>) -> ErrorBody {
    let (status, kind) = match code {
        Some("rate_limit_exceeded") => (429, ErrorKind::RateLimit),
        Some("usage_limit_reached" | "insufficient_quota") => (429, ErrorKind::UsageLimit),
        Some("server_is_overloaded" | "slow_down") => (503, ErrorKind::Overloaded),
        Some("invalid_api_key") => (401, ErrorKind::Authentication),
        Some(
            "context_length_exceeded"
            | "invalid_prompt"
            | "invalid_request"
            | "invalid_request_error",
        ) => (400, ErrorKind::InvalidRequest),
        Some(_) | None => (500, ErrorKind::Server),
    };
    ErrorBody::new(status, kind, message.unwrap_or("upstream error"))
}

pub(super) fn wire_error_body(error: Option<&WireError>) -> ErrorBody {
    error_body(
        error.and_then(|e| e.code.as_deref()),
        error.and_then(|e| e.message.as_deref()),
    )
}

pub(super) fn reasoning_origin(id: &str, encrypted: Option<String>) -> Origin {
    Origin::OpenAi {
        id: Some(id.to_owned()).filter(|id| !id.is_empty()),
        encrypted,
    }
}

pub(super) fn join_summary(parts: &[SummaryPart]) -> String {
    parts
        .iter()
        .map(|SummaryPart::SummaryText { text }| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn join_reasoning_text(parts: &[ReasoningTextPart]) -> String {
    parts
        .iter()
        .map(|ReasoningTextPart::ReasoningText { text }| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn output_items(output: Vec<OutputItem>) -> (Vec<Item>, bool, bool) {
    let mut items = Vec::new();
    let mut refused = false;
    let mut tool_called = false;
    for item in output {
        match item {
            OutputItem::Message { content, .. } => {
                let mut parts = Vec::new();
                for part in content {
                    match part {
                        OutputPart::OutputText { text, .. } => parts.push(Part::Text(text)),
                        OutputPart::Refusal { refusal } => {
                            refused = true;
                            parts.push(Part::Text(refusal));
                        }
                    }
                }
                items.push(Item::Message {
                    role: Role::Assistant,
                    parts,
                });
            }
            OutputItem::Reasoning {
                id,
                summary,
                content,
                encrypted_content,
                ..
            } => {
                let text = content.as_deref().map_or_default(join_reasoning_text);
                items.push(Item::Reasoning(Reasoning {
                    text,
                    summary: summary
                        .into_iter()
                        .map(|SummaryPart::SummaryText { text }| text)
                        .collect(),
                    origin: reasoning_origin(&id, encrypted_content),
                }));
            }
            OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                tool_called = true;
                items.push(Item::ToolCall(ToolCall {
                    id: call_id,
                    name,
                    kind: ToolKind::Function,
                    arguments,
                }));
            }
            OutputItem::CustomToolCall {
                call_id,
                name,
                input,
                ..
            } => {
                tool_called = true;
                items.push(Item::ToolCall(ToolCall {
                    id: call_id,
                    name,
                    kind: ToolKind::Custom,
                    arguments: input,
                }));
            }
        }
    }
    (items, refused, tool_called)
}

fn invalid(error: &serde_json::Error) -> Rejection {
    Rejection::unsupported(format!("invalid request: {error}"))
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, Rejection> {
    serde_json::from_str(text).map_err(|error| invalid(&error))
}

fn reject_extra(extra: &Extra, ignored: &[&str], context: &str) -> Result<(), Rejection> {
    match extra.keys().find(|key| !ignored.contains(&key.as_str())) {
        Some(key) => Err(Rejection::unsupported(format!("{context}{key}"))),
        None => Ok(()),
    }
}

fn raw_json(raw: &RawValue) -> Result<RawJson, Rejection> {
    RawJson::parse(raw.get().to_owned()).map_err(|error| invalid(&error))
}

fn check_include(include: Option<&[String]>) -> Result<(), Rejection> {
    match include
        .unwrap_or_default()
        .iter()
        .find(|value| value.as_str() != ENCRYPTED_REASONING)
    {
        Some(value) => Err(Rejection::unsupported(format!("include {value}"))),
        None => Ok(()),
    }
}

fn decode_reasoning_config(
    config: Option<ReasoningConfig>,
) -> Result<(Option<Effort>, bool), Rejection> {
    let Some(config) = config else {
        return Ok((None, false));
    };
    reject_extra(&config.extra, &[], "reasoning.")?;
    let effort = match config.effort.as_deref() {
        Some(value) => Some(
            Effort::parse(value)
                .ok_or_else(|| Rejection::unsupported(format!("reasoning.effort {value}")))?,
        ),
        None => None,
    };
    let summary = config
        .summary
        .as_deref()
        .is_some_and(|value| value != "none");
    Ok((effort, summary))
}

fn decode_speed(tier: Option<&str>) -> Result<Option<Speed>, Rejection> {
    match tier {
        None | Some("auto" | "default") => Ok(None),
        Some("priority" | "fast") => Ok(Some(Speed::Fast)),
        Some(other) => Err(Rejection::unsupported(format!("service_tier {other}"))),
    }
}

fn decode_text_config(text: Option<TextConfig>) -> Result<Option<StructuredOutput>, Rejection> {
    let Some(text) = text else {
        return Ok(None);
    };
    reject_extra(&text.extra, &[], "text.")?;
    if text.verbosity.is_some() {
        return Err(Rejection::unsupported("text.verbosity"));
    }
    let Some(format) = text.format else {
        return Ok(None);
    };
    reject_extra(&format.extra, &[], "text.format.")?;
    match format.kind.as_str() {
        "text" => Ok(None),
        "json_schema" => {
            let schema = format
                .schema
                .as_deref()
                .ok_or_else(|| Rejection::unsupported("text.format.schema"))?;
            Ok(Some(StructuredOutput {
                name: format.name,
                schema: raw_json(schema)?,
                strict: format.strict,
            }))
        }
        other => Err(Rejection::unsupported(format!("text.format.type {other}"))),
    }
}

fn decode_tool(raw: &RawValue) -> Result<Tool, Rejection> {
    let head: TypeHead = parse(raw.get())?;
    match head.kind.as_deref() {
        Some("function") => {
            let tool: FunctionTool = parse(raw.get())?;
            reject_extra(&tool.extra, &["type"], "tools.function.")?;
            let parameters = tool
                .parameters
                .as_deref()
                .ok_or_else(|| Rejection::unsupported("tools.function.parameters"))?;
            Ok(Tool::Function {
                name: tool.name,
                description: tool.description,
                parameters: raw_json(parameters)?,
                strict: tool.strict,
            })
        }
        Some("custom") => {
            let tool: CustomTool = parse(raw.get())?;
            reject_extra(&tool.extra, &["type"], "tools.custom.")?;
            Ok(Tool::Custom {
                name: tool.name,
                description: tool.description,
                format: tool.format.as_deref().map(raw_json).transpose()?,
            })
        }
        Some(other) => Err(Rejection::unsupported(format!("tool type {other}"))),
        None => Err(Rejection::unsupported("tool without type")),
    }
}

fn decode_tool_choice(value: &Value) -> Result<ToolChoice, Rejection> {
    if let Some(mode) = value.as_str() {
        return match mode {
            "auto" => Ok(ToolChoice::Auto),
            "none" => Ok(ToolChoice::None),
            "required" => Ok(ToolChoice::Required),
            other => Err(Rejection::unsupported(format!("tool_choice {other}"))),
        };
    }
    if let Some(object) = value.as_object()
        && let Some(kind) = object.get("type").and_then(Value::as_str)
        && matches!(kind, "function" | "custom")
        && object.len() == 2
        && let Some(name) = object.get("name").and_then(Value::as_str)
    {
        return Ok(ToolChoice::Named(name.to_owned()));
    }
    Err(Rejection::unsupported("tool_choice"))
}

fn decode_input(
    input: &RawValue,
    instructions: Option<String>,
) -> Result<(Vec<Part>, Vec<Item>), Rejection> {
    let mut system: Vec<Part> = instructions
        .filter(|text| !text.is_empty())
        .map(Part::Text)
        .into_iter()
        .collect();
    if input.get().trim_start().starts_with('"') {
        let text: String = parse(input.get())?;
        let item = Item::Message {
            role: Role::User,
            parts: vec![Part::Text(text)],
        };
        return Ok((system, vec![item]));
    }
    let raws: Vec<Box<RawValue>> = parse(input.get())?;
    let mut items = Vec::new();
    let mut leading = true;
    for raw in &raws {
        match decode_item(raw)? {
            Decoded::Developer(parts) if leading => system.extend(parts),
            Decoded::Developer(parts) => items.push(Item::System { parts }),
            Decoded::Item(item) => {
                leading = false;
                items.push(item);
            }
        }
    }
    Ok((system, items))
}

fn decode_item(raw: &RawValue) -> Result<Decoded, Rejection> {
    let head: TypeHead = parse(raw.get())?;
    match head.kind.as_deref().unwrap_or("message") {
        "message" => {
            let message: MessageItem = parse(raw.get())?;
            reject_extra(&message.extra, &["type", "id", "status"], "input.message.")?;
            let parts = decode_content(&message.content)?;
            match message.role.as_str() {
                "user" => Ok(Decoded::Item(Item::Message {
                    role: Role::User,
                    parts,
                })),
                "assistant" => Ok(Decoded::Item(Item::Message {
                    role: Role::Assistant,
                    parts,
                })),
                "developer" | "system" => Ok(Decoded::Developer(parts)),
                other => Err(Rejection::unsupported(format!("message role {other}"))),
            }
        }
        "reasoning" => {
            let item: ReasoningItem = parse(raw.get())?;
            reject_extra(&item.extra, &["type", "status"], "input.reasoning.")?;
            decode_reasoning_item(item).map(Decoded::Item)
        }
        "function_call" => {
            let call: FunctionCallItem = parse(raw.get())?;
            reject_extra(
                &call.extra,
                &["type", "id", "status"],
                "input.function_call.",
            )?;
            Ok(Decoded::Item(Item::ToolCall(ToolCall {
                id: call.call_id,
                name: call.name,
                kind: ToolKind::Function,
                arguments: call.arguments,
            })))
        }
        "custom_tool_call" => {
            let call: CustomCallItem = parse(raw.get())?;
            reject_extra(
                &call.extra,
                &["type", "id", "status"],
                "input.custom_tool_call.",
            )?;
            Ok(Decoded::Item(Item::ToolCall(ToolCall {
                id: call.call_id,
                name: call.name,
                kind: ToolKind::Custom,
                arguments: call.input,
            })))
        }
        "function_call_output" => decode_call_output(raw, ToolKind::Function),
        "custom_tool_call_output" => decode_call_output(raw, ToolKind::Custom),
        other => Err(Rejection::unsupported(format!("input item type {other}"))),
    }
}

fn decode_call_output(raw: &RawValue, kind: ToolKind) -> Result<Decoded, Rejection> {
    let output: CallOutputItem = parse(raw.get())?;
    reject_extra(
        &output.extra,
        &["type", "id", "status"],
        "input.call_output.",
    )?;
    Ok(Decoded::Item(Item::ToolResult(ToolResult {
        call_id: output.call_id,
        kind,
        parts: decode_content(&output.output)?,
        is_error: false,
    })))
}

fn decode_reasoning_item(item: ReasoningItem) -> Result<Item, Rejection> {
    let sealed = item
        .encrypted_content
        .as_deref()
        .filter(|value| value.starts_with(SEALED_PREFIX));
    let origin = if let Some(value) = sealed {
        envelope::open(value).ok_or(Rejection::ThinkingSignature)?
    } else {
        Origin::OpenAi {
            id: item.id.filter(|id| !id.is_empty()),
            encrypted: item.encrypted_content,
        }
    };
    Ok(Item::Reasoning(Reasoning {
        text: item.content.as_deref().map_or_default(join_reasoning_text),
        summary: item
            .summary
            .into_iter()
            .map(|SummaryPart::SummaryText { text }| text)
            .collect(),
        origin,
    }))
}

fn decode_content(raw: &RawValue) -> Result<Vec<Part>, Rejection> {
    if raw.get().trim_start().starts_with('"') {
        return Ok(vec![Part::Text(parse(raw.get())?)]);
    }
    let parts: Vec<ContentPart> = parse(raw.get())?;
    parts.into_iter().map(decode_part).collect()
}

fn decode_part(part: ContentPart) -> Result<Part, Rejection> {
    match part {
        ContentPart::InputText { text, extra } => {
            reject_extra(&extra, &[], "input_text.")?;
            Ok(Part::Text(text))
        }
        ContentPart::OutputText {
            text,
            annotations,
            logprobs,
            extra,
        } => {
            reject_extra(&extra, &[], "output_text.")?;
            if annotations.is_some_and(|a| !a.is_empty()) {
                return Err(Rejection::unsupported("output_text.annotations"));
            }
            if logprobs.is_some_and(|l| !l.is_empty()) {
                return Err(Rejection::unsupported("output_text.logprobs"));
            }
            Ok(Part::Text(text))
        }
        ContentPart::InputImage {
            image_url,
            detail,
            file_id,
            extra,
        } => {
            reject_extra(&extra, &[], "input_image.")?;
            if file_id.is_some() {
                return Err(Rejection::unsupported("input_image.file_id"));
            }
            if let Some(detail) = detail
                && detail != "auto"
            {
                return Err(Rejection::unsupported(format!(
                    "input_image.detail {detail}"
                )));
            }
            let url = image_url.ok_or_else(|| Rejection::unsupported("input_image.image_url"))?;
            Ok(Part::Image(decode_media(&url)?))
        }
        ContentPart::InputFile {
            file_data,
            file_url,
            filename,
            file_id,
            extra,
        } => {
            reject_extra(&extra, &[], "input_file.")?;
            if file_id.is_some() {
                return Err(Rejection::unsupported("input_file.file_id"));
            }
            let media = match (file_data, file_url) {
                (Some(data), None) => decode_media(&data)?,
                (None, Some(url)) => decode_media(&url)?,
                (Some(_), Some(_)) | (None, None) => {
                    return Err(Rejection::unsupported("input_file source"));
                }
            };
            Ok(Part::Document(crate::ir::Document {
                media,
                title: filename,
            }))
        }
    }
}

fn decode_media(url: &str) -> Result<Media, Rejection> {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((media_type, data)) = rest.split_once(";base64,")
    {
        return Ok(Media::Base64 {
            media_type: media_type.to_owned(),
            data: data.to_owned(),
        });
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(Media::Url(url.to_owned()));
    }
    Err(Rejection::unsupported("media url scheme"))
}
