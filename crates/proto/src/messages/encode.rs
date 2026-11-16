use std::borrow::Cow;

use bytes::Bytes;

use super::types::{
    BlockOut, CacheControl, FormatOut, MessageOut, MetadataOut, OutputConfigOut, RequestOut,
    ResponseOut, SourceOut, ThinkingOut, ToolChoiceOut, ToolInputOut, ToolOut, UsageOut,
};
use crate::codec::{EncodeOptions, EncodeResult, Encoded, Note};
use crate::envelope;
use crate::ir::{
    Document, Effort, Item, Media, Origin, Part, Reasoning, Request, Response, Role, Speed,
    StopReason, Thinking, Tool, ToolCall, ToolChoice, ToolKind, ToolResult, Usage,
};
use crate::raw::RawJson;
use crate::reject::Rejection;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    User,
    Assistant,
}

impl Side {
    fn role(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

pub fn encode_request(request: &Request, options: &EncodeOptions) -> EncodeResult {
    let max_tokens = request
        .max_output_tokens
        .or(options.default_max_output)
        .ok_or_else(|| {
            Rejection::unsupported(
                "max_tokens is required: set max_output for this model in catalog.override.toml",
            )
        })?;
    let effort = match request.effort {
        None => None,
        Some(Effort::None | Effort::Minimal) => return Err(Rejection::Effort),
        Some(
            effort @ (Effort::Low | Effort::Medium | Effort::High | Effort::Xhigh | Effort::Max),
        ) => Some(effort.as_str()),
    };
    let format = request.structured_output.as_ref().map(|output| FormatOut {
        kind: "json_schema",
        schema: &output.schema,
    });
    let output_config =
        (effort.is_some() || format.is_some()).then_some(OutputConfigOut { effort, format });

    let tools = encode_tools(&request.tools)?;
    let mut notes = Vec::new();
    let stream = request.stream || options.force_stream;
    if options.force_stream && !request.stream {
        notes.push(Note::StreamForced);
    }

    let body = RequestOut {
        model: &request.model,
        max_tokens,
        stream,
        thinking: encode_thinking(request),
        output_config,
        temperature: request.temperature,
        top_p: request.top_p,
        stop_sequences: &request.stop,
        speed: request.speed.map(|speed| match speed {
            Speed::Fast => "fast",
        }),
        metadata: request
            .user
            .as_deref()
            .map(|user_id| MetadataOut { user_id }),
        tool_choice: encode_tool_choice(request, !tools.is_empty())?,
        cache_control: CacheControl::ephemeral(),
        tools,
        system: encode_system(&request.system)?,
        messages: encode_messages(&request.items)?,
    };
    let body = serde_json::to_vec(&body)
        .map_err(|error| Rejection::unsupported(format!("serialize request: {error}")))?;
    Ok(Encoded {
        body: Bytes::from(body),
        notes,
    })
}

fn encode_thinking(request: &Request) -> Option<ThinkingOut> {
    match request.thinking {
        Some(Thinking::Adaptive) => Some(ThinkingOut::Adaptive),
        Some(Thinking::Enabled { budget_tokens }) => Some(ThinkingOut::Enabled { budget_tokens }),
        Some(Thinking::Disabled) => Some(ThinkingOut::Disabled),
        None => request.reasoning_summary.then_some(ThinkingOut::Adaptive),
    }
}

fn encode_tools(tools: &[Tool]) -> Result<Vec<ToolOut<'_>>, Rejection> {
    let mut out = tools
        .iter()
        .map(|tool| match tool {
            Tool::Function {
                name,
                description,
                parameters,
                strict,
            } => Ok(ToolOut {
                name,
                description: description.as_deref(),
                input_schema: parameters,
                strict: *strict,
                cache_control: None,
            }),
            Tool::Custom { .. } => Err(Rejection::unsupported("custom tool")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(last) = out.last_mut() {
        last.cache_control = Some(CacheControl::ephemeral());
    }
    Ok(out)
}

fn encode_tool_choice(
    request: &Request,
    has_tools: bool,
) -> Result<Option<ToolChoiceOut<'_>>, Rejection> {
    let disable = (request.parallel_tool_calls == Some(false)).then_some(true);
    let choice = request.tool_choice.as_ref().unwrap_or(&ToolChoice::Auto);
    if !has_tools {
        return match choice {
            ToolChoice::Auto | ToolChoice::None => Ok(None),
            ToolChoice::Required | ToolChoice::Named(_) => {
                Err(Rejection::unsupported("tool_choice without tools"))
            }
        };
    }
    Ok(match choice {
        ToolChoice::Auto => {
            (request.tool_choice.is_some() || disable.is_some()).then_some(ToolChoiceOut::Auto {
                disable_parallel_tool_use: disable,
            })
        }
        ToolChoice::None => Some(ToolChoiceOut::None),
        ToolChoice::Required => Some(ToolChoiceOut::Any {
            disable_parallel_tool_use: disable,
        }),
        ToolChoice::Named(name) => Some(ToolChoiceOut::Tool {
            name,
            disable_parallel_tool_use: disable,
        }),
    })
}

fn encode_system(parts: &[Part]) -> Result<Vec<BlockOut<'_>>, Rejection> {
    let mut blocks = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => {
                if let Some(block) = text_block(text) {
                    blocks.push(block);
                }
            }
            Part::Image(_) => return Err(Rejection::ImageBlock),
            Part::Document(_) => return Err(Rejection::DocumentBlock),
        }
    }
    if let Some(BlockOut::Text { cache_control, .. }) = blocks.last_mut() {
        *cache_control = Some(CacheControl::ephemeral());
    }
    Ok(blocks)
}

// Anthropic rejects empty text blocks with a 400.
// https://platform.claude.com/docs/en/api/errors
fn text_block(text: &str) -> Option<BlockOut<'_>> {
    (!text.is_empty()).then_some(BlockOut::Text {
        text,
        cache_control: None,
    })
}

fn encode_messages(items: &[Item]) -> Result<Vec<MessageOut<'_>>, Rejection> {
    let mut messages: Vec<(Side, Vec<BlockOut<'_>>)> = Vec::new();
    for item in items {
        let (side, blocks) = encode_item(item)?;
        if blocks.is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some((last, existing)) if *last == side => existing.extend(blocks),
            Some(_) | None => messages.push((side, blocks)),
        }
    }
    Ok(messages
        .into_iter()
        .map(|(side, mut content)| {
            content.sort_by_key(|block| !block.is_tool_result());
            MessageOut {
                role: side.role(),
                content,
            }
        })
        .collect())
}

fn encode_item(item: &Item) -> Result<(Side, Vec<BlockOut<'_>>), Rejection> {
    match item {
        Item::Message { role, parts } => {
            let (side, text_only) = match role {
                Role::User => (Side::User, false),
                Role::Assistant => (Side::Assistant, true),
            };
            Ok((side, encode_parts(parts, text_only)?))
        }
        Item::System { .. } => Err(Rejection::MidConvSystem),
        Item::Reasoning(reasoning) => Ok((Side::Assistant, vec![encode_reasoning(reasoning)?])),
        Item::ToolCall(call) => Ok((Side::Assistant, vec![encode_tool_call(call)?])),
        Item::ToolResult(result) => Ok((Side::User, vec![encode_tool_result(result)?])),
    }
}

fn encode_parts(parts: &[Part], text_only: bool) -> Result<Vec<BlockOut<'_>>, Rejection> {
    let mut blocks = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => blocks.extend(text_block(text)),
            Part::Image(_) if text_only => return Err(Rejection::ImageBlock),
            Part::Document(_) if text_only => return Err(Rejection::DocumentBlock),
            Part::Image(media) => blocks.push(BlockOut::Image {
                source: source(media),
            }),
            Part::Document(Document { media, title }) => blocks.push(BlockOut::Document {
                source: source(media),
                title: title.as_deref(),
            }),
        }
    }
    Ok(blocks)
}

fn source(media: &Media) -> SourceOut<'_> {
    match media {
        Media::Base64 { media_type, data } => SourceOut::Base64 { media_type, data },
        Media::Url(url) => SourceOut::Url { url },
    }
}

fn encode_reasoning(reasoning: &Reasoning) -> Result<BlockOut<'_>, Rejection> {
    match &reasoning.origin {
        Origin::Anthropic { signature } => Ok(BlockOut::Thinking {
            thinking: Cow::Borrowed(&reasoning.text),
            signature: Cow::Borrowed(signature),
        }),
        Origin::AnthropicRedacted { data } => Ok(BlockOut::RedactedThinking { data }),
        Origin::OpenAi { .. } | Origin::Plain => Err(Rejection::ThinkingSignature),
    }
}

fn tool_input(call: &ToolCall) -> Result<RawJson, Rejection> {
    let invalid = || Rejection::unsupported(format!("arguments of tool call {}", call.id));
    if !call.arguments.trim_start().starts_with('{') {
        return Err(invalid());
    }
    RawJson::parse(call.arguments.clone()).map_err(|_| invalid())
}

fn encode_tool_call(call: &ToolCall) -> Result<BlockOut<'_>, Rejection> {
    match call.kind {
        ToolKind::Function => Ok(BlockOut::ToolUse {
            id: &call.id,
            name: &call.name,
            input: ToolInputOut::Raw(tool_input(call)?),
        }),
        ToolKind::Custom => Err(Rejection::unsupported("custom tool call")),
    }
}

fn encode_tool_result(result: &ToolResult) -> Result<BlockOut<'_>, Rejection> {
    match result.kind {
        ToolKind::Function => Ok(BlockOut::ToolResult {
            tool_use_id: &result.call_id,
            content: encode_parts(&result.parts, false)?,
            is_error: result.is_error,
        }),
        ToolKind::Custom => Err(Rejection::unsupported("custom tool result")),
    }
}

pub fn encode_response(response: &Response) -> Bytes {
    let mut content = Vec::new();
    for item in &response.output {
        match item {
            Item::Message { parts, .. } => {
                for part in parts {
                    if let Part::Text(text) = part {
                        content.push(BlockOut::Text {
                            text,
                            cache_control: None,
                        });
                    }
                }
            }
            Item::Reasoning(reasoning) => content.push(client_reasoning(reasoning)),
            Item::ToolCall(call) => content.push(BlockOut::ToolUse {
                id: &call.id,
                name: &call.name,
                input: client_tool_input(&call.arguments),
            }),
            Item::System { .. } | Item::ToolResult(_) => {}
        }
    }
    let body = ResponseOut {
        id: &response.id,
        kind: "message",
        role: "assistant",
        model: &response.model,
        content,
        stop_reason: Some(stop_reason_name(response.stop)),
        stop_sequence: None,
        usage: usage_out(&response.usage),
    };
    Bytes::from(serde_json::to_vec(&body).unwrap_or_default())
}

fn client_tool_input(arguments: &str) -> ToolInputOut {
    if !arguments.trim_start().starts_with('{') {
        return ToolInputOut::Empty;
    }
    RawJson::parse(arguments.to_owned()).map_or(ToolInputOut::Empty, ToolInputOut::Raw)
}

fn client_reasoning(reasoning: &Reasoning) -> BlockOut<'_> {
    let thinking = if reasoning.text.is_empty() {
        Cow::Owned(reasoning.summary.join("\n\n"))
    } else {
        Cow::Borrowed(reasoning.text.as_str())
    };
    match &reasoning.origin {
        Origin::Anthropic { signature } => BlockOut::Thinking {
            thinking,
            signature: Cow::Borrowed(signature),
        },
        Origin::AnthropicRedacted { data } => BlockOut::RedactedThinking { data },
        Origin::OpenAi { .. } | Origin::Plain => BlockOut::Thinking {
            thinking,
            signature: Cow::Owned(envelope::seal(&reasoning.origin)),
        },
    }
}

pub(super) fn stop_reason_name(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::ToolUse => "tool_use",
        StopReason::StopSequence => "stop_sequence",
        StopReason::Refusal | StopReason::ContentFilter => "refusal",
        StopReason::ContextWindow => "model_context_window_exceeded",
    }
}

pub(super) fn usage_out(usage: &Usage) -> UsageOut {
    UsageOut {
        input: usage.input_uncached,
        cache_write: usage.cache_write,
        cache_read: usage.cache_read,
        output: usage.output,
    }
}
