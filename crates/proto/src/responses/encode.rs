use bytes::Bytes;
use serde_json::{Value, json};

use super::types::{
    IncompleteDetails, InputItem, InputPart, InputTokensDetails, OutputItem, OutputPart,
    OutputTokensDetails, ReasoningOut, ReasoningTextPart, RequestBody, ResponseObject, SummaryPart,
    TextFormatOut, TextOut, ToolOut, WireError, WireUsage,
};
use crate::codec::{EncodeOptions, EncodeResult, Encoded, Note};
use crate::envelope;
use crate::features::ThinkingKind;
use crate::ir::{
    Effort, Item, Media, Origin, Part, Reasoning, Request, Response, Role, Speed, StopReason,
    Thinking, Tool, ToolCall, ToolChoice, ToolKind, ToolResult, Usage,
};
use crate::reject::Rejection;

pub(super) enum State {
    InProgress,
    Completed,
    Incomplete(&'static str),
    Failed(WireError),
}

pub(super) fn state_of(stop: StopReason) -> State {
    match stop {
        StopReason::EndTurn
        | StopReason::ToolUse
        | StopReason::StopSequence
        | StopReason::Refusal => State::Completed,
        StopReason::MaxTokens => State::Incomplete("max_output_tokens"),
        StopReason::ContentFilter => State::Incomplete("content_filter"),
        StopReason::ContextWindow => State::Failed(WireError {
            code: Some("context_length_exceeded".to_owned()),
            message: Some("the context window was exceeded".to_owned()),
        }),
    }
}

pub(super) fn response_object(
    id: &str,
    model: &str,
    state: State,
    output: Vec<OutputItem>,
    usage: Option<&Usage>,
) -> ResponseObject {
    let (status, error, incomplete_details) = match state {
        State::InProgress => ("in_progress", None, None),
        State::Completed => ("completed", None, None),
        State::Incomplete(reason) => (
            "incomplete",
            None,
            Some(IncompleteDetails {
                reason: Some(reason.to_owned()),
            }),
        ),
        State::Failed(error) => ("failed", Some(error), None),
    };
    ResponseObject {
        id: id.to_owned(),
        object: "response",
        created_at: 0,
        status,
        error,
        incomplete_details,
        model: model.to_owned(),
        output,
        usage: usage.map(usage_to_wire),
    }
}

pub(super) fn usage_to_wire(usage: &Usage) -> WireUsage {
    WireUsage {
        input_tokens: usage.input_total(),
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: usage.cache_read,
            cache_write_tokens: Some(usage.cache_write).filter(|written| *written > 0),
        }),
        output_tokens: usage.output,
        output_tokens_details: usage.reasoning.map(|tokens| OutputTokensDetails {
            reasoning_tokens: Some(tokens),
        }),
        total_tokens: Some(usage.input_total() + usage.output),
    }
}

pub(super) fn sealed_origin(origin: Option<&Origin>) -> String {
    envelope::seal(origin.unwrap_or(&Origin::Plain))
}

pub(super) fn item_id(prefix: &str, output_index: impl std::fmt::Display) -> String {
    format!("{prefix}_{output_index}")
}

pub fn encode_response(response: &Response) -> Bytes {
    let output = response
        .output
        .iter()
        .filter_map(|item| match item {
            Item::Message {
                role: Role::Assistant,
                parts,
            } => Some(OutputItem::Message {
                id: String::new(),
                role: "assistant".to_owned(),
                content: parts
                    .iter()
                    .filter_map(|part| match part {
                        Part::Text(text) => Some(OutputPart::OutputText {
                            text: text.clone(),
                            annotations: Vec::new(),
                        }),
                        Part::Image(_) | Part::Document(_) => None,
                    })
                    .collect(),
                status: Some("completed".to_owned()),
            }),
            Item::Reasoning(reasoning) => Some(reasoning_output(reasoning)),
            Item::ToolCall(call) => Some(call_output(call)),
            Item::Message {
                role: Role::User, ..
            }
            | Item::System { .. }
            | Item::ToolResult(_) => None,
        })
        .enumerate()
        .map(|(index, item)| with_id(item, index))
        .collect();
    let object = response_object(
        &response.id,
        &response.model,
        state_of(response.stop),
        output,
        Some(&response.usage),
    );
    Bytes::from(serde_json::to_vec(&object).unwrap_or_default())
}

fn with_id(item: OutputItem, index: usize) -> OutputItem {
    match item {
        OutputItem::Message {
            role,
            content,
            status,
            ..
        } => OutputItem::Message {
            id: item_id("msg", index),
            role,
            content,
            status,
        },
        OutputItem::Reasoning {
            summary,
            content,
            encrypted_content,
            status,
            ..
        } => OutputItem::Reasoning {
            id: item_id("rs", index),
            summary,
            content,
            encrypted_content,
            status,
        },
        OutputItem::FunctionCall {
            call_id,
            name,
            arguments,
            status,
            ..
        } => OutputItem::FunctionCall {
            id: item_id("fc", index),
            call_id,
            name,
            arguments,
            status,
        },
        OutputItem::CustomToolCall {
            call_id,
            name,
            input,
            status,
            ..
        } => OutputItem::CustomToolCall {
            id: item_id("ctc", index),
            call_id,
            name,
            input,
            status,
        },
    }
}

fn reasoning_output(reasoning: &Reasoning) -> OutputItem {
    OutputItem::Reasoning {
        id: String::new(),
        summary: reasoning
            .summary
            .iter()
            .map(|text| SummaryPart::SummaryText { text: text.clone() })
            .collect(),
        content: reasoning_text(&reasoning.text),
        encrypted_content: Some(sealed_origin(Some(&reasoning.origin))),
        status: Some("completed".to_owned()),
    }
}

fn call_output(call: &ToolCall) -> OutputItem {
    match call.kind {
        ToolKind::Function => OutputItem::FunctionCall {
            id: String::new(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            status: Some("completed".to_owned()),
        },
        ToolKind::Custom => OutputItem::CustomToolCall {
            id: String::new(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            input: call.arguments.clone(),
            status: Some("completed".to_owned()),
        },
    }
}

pub fn encode_request(request: &Request, options: &EncodeOptions) -> EncodeResult {
    if !request.stop.is_empty() {
        return Err(Rejection::unsupported("stop sequences"));
    }
    let adaptive = match request.thinking {
        None => false,
        Some(Thinking::Adaptive) => true,
        Some(thinking @ Thinking::Enabled { .. }) => {
            return Err(Rejection::ThinkingType(thinking.kind()));
        }
        Some(Thinking::Disabled) => {
            return Err(Rejection::unsupported(format!(
                "thinking type {}",
                ThinkingKind::Disabled.as_str()
            )));
        }
    };
    let mut notes = Vec::new();
    if options.force_stream && !request.stream {
        notes.push(Note::StreamForced);
    }
    let input = encode_input(request, &mut notes)?;
    let summary = (adaptive || request.reasoning_summary).then_some("auto");
    let effort = request.effort.map(Effort::as_str);
    let body = RequestBody {
        model: &request.model,
        tools: request.tools.iter().map(encode_tool).collect(),
        tool_choice: request
            .tool_choice
            .as_ref()
            .map(|choice| encode_tool_choice(choice, &request.tools)),
        parallel_tool_calls: request.parallel_tool_calls,
        reasoning: (effort.is_some() || summary.is_some())
            .then_some(ReasoningOut { effort, summary }),
        text: encode_text(request)?,
        store: false,
        include: ["reasoning.encrypted_content"],
        service_tier: request.speed.map(|speed| match speed {
            Speed::Fast => "priority",
        }),
        max_output_tokens: request.max_output_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        prompt_cache_key: options
            .cache_key
            .as_deref()
            .or(request.cache_key.as_deref()),
        user: request.user.as_deref(),
        stream: request.stream || options.force_stream,
        input,
    };
    let bytes = serde_json::to_vec(&body)
        .map_err(|error| Rejection::unsupported(format!("serialize request: {error}")))?;
    Ok(Encoded {
        body: Bytes::from(bytes),
        notes,
    })
}

fn encode_text(request: &Request) -> Result<Option<TextOut<'_>>, Rejection> {
    let Some(output) = &request.structured_output else {
        return Ok(None);
    };
    let name = output.name.as_deref().ok_or(Rejection::StructuredOutputs)?;
    Ok(Some(TextOut {
        format: TextFormatOut {
            r#type: "json_schema",
            name,
            schema: &output.schema,
            strict: output.strict,
        },
    }))
}

fn encode_tool(tool: &Tool) -> ToolOut<'_> {
    match tool {
        Tool::Function {
            name,
            description,
            parameters,
            strict,
        } => ToolOut::Function {
            name,
            description: description.as_deref(),
            parameters,
            // The Responses API defaults function tool `strict` to true, while Messages and
            // Chat tools without the flag are non-strict.
            // https://platform.openai.com/docs/api-reference/responses/create#responses-create-tools
            strict: strict.unwrap_or(false),
        },
        Tool::Custom {
            name,
            description,
            format,
        } => ToolOut::Custom {
            name,
            description: description.as_deref(),
            format: format.as_ref(),
        },
    }
}

fn encode_tool_choice(choice: &ToolChoice, tools: &[Tool]) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Named(name) => {
            let custom = tools
                .iter()
                .any(|tool| matches!(tool, Tool::Custom { name: n, .. } if n == name));
            json!({ "type": if custom { "custom" } else { "function" }, "name": name })
        }
    }
}

fn encode_input<'a>(
    request: &'a Request,
    notes: &mut Vec<Note>,
) -> Result<Vec<InputItem<'a>>, Rejection> {
    let mut input = Vec::with_capacity(request.items.len() + 1);
    if !request.system.is_empty() {
        input.push(developer_message(&request.system)?);
    }
    for item in &request.items {
        input.push(match item {
            Item::Message {
                role: Role::User,
                parts,
            } => InputItem::Message {
                role: "user",
                content: parts.iter().map(input_part).collect::<Result<_, _>>()?,
            },
            Item::Message {
                role: Role::Assistant,
                parts,
            } => InputItem::Message {
                role: "assistant",
                content: parts.iter().map(output_part).collect::<Result<_, _>>()?,
            },
            Item::System { parts } => developer_message(parts)?,
            Item::Reasoning(reasoning) => encode_reasoning(reasoning)?,
            Item::ToolCall(call) => match call.kind {
                ToolKind::Function => InputItem::FunctionCall {
                    call_id: &call.id,
                    name: &call.name,
                    arguments: &call.arguments,
                },
                ToolKind::Custom => InputItem::CustomToolCall {
                    call_id: &call.id,
                    name: &call.name,
                    input: &call.arguments,
                },
            },
            Item::ToolResult(result) => encode_result(result, notes)?,
        });
    }
    Ok(input)
}

fn developer_message(parts: &[Part]) -> Result<InputItem<'_>, Rejection> {
    let content = parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => Ok(InputPart::InputText { text }),
            Part::Image(_) => Err(Rejection::ImageBlock),
            Part::Document(_) => Err(Rejection::DocumentBlock),
        })
        .collect::<Result<_, _>>()?;
    Ok(InputItem::Message {
        role: "developer",
        content,
    })
}

fn encode_reasoning(reasoning: &Reasoning) -> Result<InputItem<'_>, Rejection> {
    let Origin::OpenAi { id, encrypted } = &reasoning.origin else {
        return Err(Rejection::ThinkingSignature);
    };
    Ok(InputItem::Reasoning {
        id: id.as_deref(),
        summary: reasoning
            .summary
            .iter()
            .map(|text| SummaryPart::SummaryText { text: text.clone() })
            .collect(),
        content: reasoning_text(&reasoning.text),
        encrypted_content: encrypted.as_deref(),
    })
}

fn encode_result<'a>(
    result: &'a ToolResult,
    notes: &mut Vec<Note>,
) -> Result<InputItem<'a>, Rejection> {
    if result.is_error && !notes.contains(&Note::ToolErrorFlagDropped) {
        notes.push(Note::ToolErrorFlagDropped);
    }
    let output = result
        .parts
        .iter()
        .map(input_part)
        .collect::<Result<_, _>>()?;
    Ok(match result.kind {
        ToolKind::Function => InputItem::FunctionCallOutput {
            call_id: &result.call_id,
            output,
        },
        ToolKind::Custom => InputItem::CustomToolCallOutput {
            call_id: &result.call_id,
            output,
        },
    })
}

fn output_part(part: &Part) -> Result<InputPart<'_>, Rejection> {
    match part {
        Part::Text(text) => Ok(InputPart::OutputText { text }),
        Part::Image(_) => Err(Rejection::ImageBlock),
        Part::Document(_) => Err(Rejection::DocumentBlock),
    }
}

fn input_part(part: &Part) -> Result<InputPart<'_>, Rejection> {
    match part {
        Part::Text(text) => Ok(InputPart::InputText { text }),
        Part::Image(media) => Ok(InputPart::InputImage {
            image_url: media_url(media),
        }),
        Part::Document(document) => match &document.media {
            Media::Base64 { .. } => Ok(InputPart::InputFile {
                file_data: Some(media_url(&document.media)),
                file_url: None,
                filename: Some(document.title.as_deref().ok_or(Rejection::DocumentBlock)?),
            }),
            Media::Url(url) => Ok(InputPart::InputFile {
                file_data: None,
                file_url: Some(url),
                filename: document.title.as_deref(),
            }),
        },
    }
}

fn media_url(media: &Media) -> String {
    match media {
        Media::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        Media::Url(url) => url.clone(),
    }
}

fn reasoning_text(text: &str) -> Option<Vec<ReasoningTextPart>> {
    (!text.is_empty()).then(|| {
        vec![ReasoningTextPart::ReasoningText {
            text: text.to_owned(),
        }]
    })
}
