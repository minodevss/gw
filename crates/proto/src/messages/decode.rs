use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::types::{
    BlockHead, CacheControl, DocumentIn, ImageIn, MediaSourceIn, MessageIn, MetadataIn,
    OutputConfigIn, RedactedThinkingIn, ResponseIn, ResponseRedactedIn, ResponseTextIn,
    ResponseThinkingIn, ResponseToolUseIn, RoleIn, TextIn, ThinkingBlockIn, ThinkingIn,
    ToolChoiceIn, ToolHead, ToolIn, ToolResultIn, ToolUseIn, UsageIn,
};
use crate::codec::DecodeError;
use crate::envelope;
use crate::ir::{
    Document, Effort, Item, Media, Origin, Part, Reasoning, Request, Response, Role, Speed,
    StopReason, StructuredOutput, Thinking, Tool, ToolCall, ToolChoice, ToolKind, ToolResult,
    Usage,
};
use crate::reject::Rejection;

const KNOWN_FIELDS: [&str; 14] = [
    "model",
    "messages",
    "system",
    "tools",
    "tool_choice",
    "max_tokens",
    "temperature",
    "top_p",
    "stop_sequences",
    "thinking",
    "output_config",
    "speed",
    "metadata",
    "stream",
];

// Field and block to anthropic-beta pairs, from the Claude Code gateway document and the beta
// list in the Claude Code 2.1.325 binary.
// https://platform.claude.com/docs/en/api/beta-headers
const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
const MCP_SERVERS_BETA: &str = "mcp-servers-2025-12-04";
const PER_TURN_CONTROL_BETA: &str = "per-turn-control-2026-07-01";
const MID_CONVERSATION_TOOL_CHANGES_BETA: &str = "mid-conversation-tool-changes-2026-07-01";
const ADVANCED_TOOL_USE_BETA: &str = "advanced-tool-use-2025-11-20";
const TOOL_SEARCH_BETA: &str = "tool-search-tool-2025-10-19";
const PROMPT_CACHING_SCOPE_BETA: &str = "prompt-caching-scope-2026-01-05";
const TASK_BUDGETS_BETA: &str = "task-budgets-2026-03-13";
const FILES_API_BETA: &str = "files-api-2025-04-14";

fn beta(value: &str) -> Rejection {
    Rejection::BetaHeader(value.to_owned())
}

fn beta_for_field(key: &str) -> Option<&'static str> {
    match key {
        "context_management" => Some(CONTEXT_MANAGEMENT_BETA),
        "mcp_servers" => Some(MCP_SERVERS_BETA),
        _ => None,
    }
}

fn parse<T: DeserializeOwned>(text: &str, context: &str) -> Result<T, Rejection> {
    serde_json::from_str(text)
        .map_err(|error| Rejection::unsupported(format!("{context}: {error}")))
}

struct Fields(BTreeMap<String, Box<RawValue>>);

impl Fields {
    fn reject_unknown(&self) -> Result<(), Rejection> {
        if let Some(value) = self.0.keys().find_map(|key| beta_for_field(key)) {
            return Err(beta(value));
        }
        match self
            .0
            .keys()
            .find(|key| !KNOWN_FIELDS.contains(&key.as_str()))
        {
            Some(key) => Err(Rejection::unsupported(format!("unknown field {key}"))),
            None => Ok(()),
        }
    }

    fn take<T: DeserializeOwned>(&mut self, key: &str) -> Result<Option<T>, Rejection> {
        match self.0.remove(key) {
            Some(raw) if raw.get() != "null" => parse(raw.get(), key).map(Some),
            Some(_) | None => Ok(None),
        }
    }

    fn require<T: DeserializeOwned>(&mut self, key: &str) -> Result<T, Rejection> {
        self.take(key)?
            .ok_or_else(|| Rejection::unsupported(format!("missing field {key}")))
    }
}

enum Block {
    Part(Part),
    Reasoning(Reasoning),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
}

pub fn decode_request(body: &[u8]) -> Result<Request, Rejection> {
    let map: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(body)
        .map_err(|error| Rejection::unsupported(format!("invalid json: {error}")))?;
    let mut fields = Fields(map);
    fields.reject_unknown()?;

    let model: String = fields.require("model")?;
    let system = decode_system(fields.take("system")?)?;
    let messages: Vec<MessageIn> = fields.require("messages")?;
    let mut items = Vec::new();
    for message in messages {
        items.extend(decode_message(&message)?);
    }
    let tools = fields
        .take::<Vec<Box<RawValue>>>("tools")?
        .unwrap_or_default()
        .iter()
        .map(|raw| decode_tool(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let (tool_choice, parallel_tool_calls) = decode_tool_choice(fields.take("tool_choice")?);
    let (thinking, reasoning_summary) = decode_thinking(fields.take("thinking")?);
    let (effort, structured_output) = decode_output_config(fields.take("output_config")?)?;
    let speed = decode_speed(fields.take::<String>("speed")?.as_deref())?;
    // Claude Code fills metadata.user_id with its device, account and session ids. It identifies the
    // client to Anthropic only, so it is not forwarded to other upstreams.
    fields.take::<MetadataIn>("metadata")?;
    let user = None;

    Ok(Request {
        model,
        system,
        items,
        tools,
        tool_choice,
        parallel_tool_calls,
        max_output_tokens: fields.take("max_tokens")?,
        temperature: fields.take("temperature")?,
        top_p: fields.take("top_p")?,
        stop: fields.take("stop_sequences")?.unwrap_or_default(),
        effort,
        thinking,
        reasoning_summary,
        speed,
        structured_output,
        stream: fields.take("stream")?.unwrap_or(false),
        cache_key: None,
        user,
    })
}

fn decode_system(raw: Option<Box<RawValue>>) -> Result<Vec<Part>, Rejection> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    content_blocks(&raw)?
        .into_iter()
        .map(|block| match block {
            Block::Part(part @ Part::Text(_)) => Ok(part),
            Block::Part(Part::Image(_) | Part::Document(_))
            | Block::Reasoning(_)
            | Block::ToolCall(_)
            | Block::ToolResult(_) => Err(Rejection::unsupported("system block must be text")),
        })
        .collect()
}

fn decode_message(message: &MessageIn) -> Result<Vec<Item>, Rejection> {
    if message.output_config.is_some() {
        return Err(beta(PER_TURN_CONTROL_BETA));
    }
    let blocks = content_blocks(&message.content)?;
    match message.role {
        RoleIn::User => user_items(blocks),
        RoleIn::Assistant => assistant_items(blocks),
        RoleIn::System => system_items(blocks),
    }
}

fn flush(items: &mut Vec<Item>, parts: &mut Vec<Part>, role: Role) {
    if !parts.is_empty() {
        items.push(Item::Message {
            role,
            parts: std::mem::take(parts),
        });
    }
}

fn user_items(blocks: Vec<Block>) -> Result<Vec<Item>, Rejection> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            Block::Part(part) => parts.push(part),
            Block::ToolResult(result) => {
                flush(&mut items, &mut parts, Role::User);
                items.push(Item::ToolResult(result));
            }
            Block::Reasoning(_) | Block::ToolCall(_) => {
                return Err(Rejection::unsupported("assistant block in user message"));
            }
        }
    }
    flush(&mut items, &mut parts, Role::User);
    Ok(items)
}

fn assistant_items(blocks: Vec<Block>) -> Result<Vec<Item>, Rejection> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            Block::Part(part @ Part::Text(_)) => parts.push(part),
            Block::Part(Part::Image(_) | Part::Document(_)) => {
                return Err(Rejection::unsupported("media in assistant message"));
            }
            Block::Reasoning(reasoning) => {
                flush(&mut items, &mut parts, Role::Assistant);
                items.push(Item::Reasoning(reasoning));
            }
            Block::ToolCall(call) => {
                flush(&mut items, &mut parts, Role::Assistant);
                items.push(Item::ToolCall(call));
            }
            Block::ToolResult(_) => {
                return Err(Rejection::unsupported("tool_result in assistant message"));
            }
        }
    }
    flush(&mut items, &mut parts, Role::Assistant);
    Ok(items)
}

fn system_items(blocks: Vec<Block>) -> Result<Vec<Item>, Rejection> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            Block::Part(part @ Part::Text(_)) => parts.push(part),
            Block::Part(Part::Image(_) | Part::Document(_))
            | Block::Reasoning(_)
            | Block::ToolCall(_)
            | Block::ToolResult(_) => {
                return Err(Rejection::unsupported("system message block must be text"));
            }
        }
    }
    Ok(if parts.is_empty() {
        Vec::new()
    } else {
        vec![Item::System { parts }]
    })
}

fn content_blocks(raw: &RawValue) -> Result<Vec<Block>, Rejection> {
    if raw.get().trim_start().starts_with('"') {
        let text: String = parse(raw.get(), "content")?;
        return Ok(vec![Block::Part(Part::Text(text))]);
    }
    let raws: Vec<Box<RawValue>> = parse(raw.get(), "content")?;
    raws.iter().map(|raw| parse_block(raw)).collect()
}

fn check_cache(cache_control: Option<&CacheControl>) -> Result<(), Rejection> {
    match cache_control {
        Some(CacheControl { scope: Some(_), .. }) => Err(beta(PROMPT_CACHING_SCOPE_BETA)),
        Some(_) | None => Ok(()),
    }
}

fn media(source: MediaSourceIn) -> Result<Media, Rejection> {
    match source {
        MediaSourceIn::Base64 { media_type, data } => Ok(Media::Base64 { media_type, data }),
        MediaSourceIn::Url { url } => Ok(Media::Url(url)),
        MediaSourceIn::File { .. } => Err(beta(FILES_API_BETA)),
    }
}

fn reasoning_origin(signature: String) -> Origin {
    envelope::open(&signature).unwrap_or(Origin::Anthropic { signature })
}

fn redacted_origin(data: String) -> Origin {
    envelope::open(&data).unwrap_or(Origin::AnthropicRedacted { data })
}

fn parse_block(raw: &RawValue) -> Result<Block, Rejection> {
    let text = raw.get();
    let head: BlockHead = parse(text, "content block")?;
    match head.kind.as_str() {
        "text" => {
            let block: TextIn = parse(text, "text block")?;
            check_cache(block.cache_control.as_ref())?;
            Ok(Block::Part(Part::Text(block.text)))
        }
        "image" => {
            let block: ImageIn = parse(text, "image block")?;
            check_cache(block.cache_control.as_ref())?;
            Ok(Block::Part(Part::Image(media(block.source)?)))
        }
        "document" => {
            let block: DocumentIn = parse(text, "document block")?;
            check_cache(block.cache_control.as_ref())?;
            Ok(Block::Part(Part::Document(Document {
                media: media(block.source)?,
                title: block.title,
            })))
        }
        "thinking" => {
            let block: ThinkingBlockIn = parse(text, "thinking block")?;
            Ok(Block::Reasoning(Reasoning {
                text: block.thinking,
                summary: Vec::new(),
                origin: reasoning_origin(block.signature),
            }))
        }
        "redacted_thinking" => {
            let block: RedactedThinkingIn = parse(text, "redacted_thinking block")?;
            Ok(Block::Reasoning(Reasoning {
                text: String::new(),
                summary: Vec::new(),
                origin: redacted_origin(block.data),
            }))
        }
        "tool_use" => {
            let block: ToolUseIn = parse(text, "tool_use block")?;
            check_cache(block.cache_control.as_ref())?;
            Ok(Block::ToolCall(ToolCall {
                id: block.id,
                name: block.name,
                kind: ToolKind::Function,
                arguments: block.input.as_str().to_owned(),
            }))
        }
        "tool_result" => {
            let block: ToolResultIn = parse(text, "tool_result block")?;
            check_cache(block.cache_control.as_ref())?;
            Ok(Block::ToolResult(ToolResult {
                call_id: block.tool_use_id,
                kind: ToolKind::Function,
                parts: tool_result_parts(block.content.as_deref())?,
                is_error: block.is_error.unwrap_or(false),
            }))
        }
        "tool_changes" => Err(beta(MID_CONVERSATION_TOOL_CHANGES_BETA)),
        "tool_reference" => Err(beta(TOOL_SEARCH_BETA)),
        other => Err(Rejection::unsupported(format!(
            "content block type {other}"
        ))),
    }
}

fn tool_result_parts(content: Option<&RawValue>) -> Result<Vec<Part>, Rejection> {
    let Some(raw) = content else {
        return Ok(Vec::new());
    };
    if raw.get().trim_start().starts_with('"') {
        let text: String = parse(raw.get(), "tool_result content")?;
        return Ok(if text.is_empty() {
            Vec::new()
        } else {
            vec![Part::Text(text)]
        });
    }
    content_blocks(raw)?
        .into_iter()
        .map(|block| match block {
            Block::Part(part) => Ok(part),
            Block::Reasoning(_) | Block::ToolCall(_) | Block::ToolResult(_) => Err(
                Rejection::unsupported("tool_result content must be text, image or document"),
            ),
        })
        .collect()
}

fn decode_tool(raw: &RawValue) -> Result<Tool, Rejection> {
    let head: ToolHead = parse(raw.get(), "tool")?;
    if let Some(kind) = head.kind
        && kind != "custom"
    {
        return Err(Rejection::unsupported(format!("server tool {kind}")));
    }
    let tool: ToolIn = parse(raw.get(), "tool")?;
    if tool.defer_loading.is_some()
        || tool.allowed_callers.is_some()
        || tool.input_examples.is_some()
    {
        return Err(beta(ADVANCED_TOOL_USE_BETA));
    }
    check_cache(tool.cache_control.as_ref())?;
    Ok(Tool::Function {
        name: tool.name,
        description: tool.description,
        parameters: tool.input_schema,
        strict: tool.strict,
    })
}

fn decode_tool_choice(choice: Option<ToolChoiceIn>) -> (Option<ToolChoice>, Option<bool>) {
    let Some(choice) = choice else {
        return (None, None);
    };
    let (choice, disable) = match choice {
        ToolChoiceIn::Auto {
            disable_parallel_tool_use,
        } => (ToolChoice::Auto, disable_parallel_tool_use),
        ToolChoiceIn::Any {
            disable_parallel_tool_use,
        } => (ToolChoice::Required, disable_parallel_tool_use),
        ToolChoiceIn::Tool {
            name,
            disable_parallel_tool_use,
        } => (ToolChoice::Named(name), disable_parallel_tool_use),
        ToolChoiceIn::None => (ToolChoice::None, None),
    };
    (Some(choice), disable.map(|disable| !disable))
}

fn decode_thinking(thinking: Option<ThinkingIn>) -> (Option<Thinking>, bool) {
    match thinking {
        None => (None, false),
        Some(ThinkingIn::Adaptive { display }) => (
            Some(Thinking::Adaptive),
            display.as_deref() != Some("omitted"),
        ),
        Some(ThinkingIn::Enabled { budget_tokens, .. }) => {
            (Some(Thinking::Enabled { budget_tokens }), false)
        }
        Some(ThinkingIn::Disabled) => (Some(Thinking::Disabled), false),
    }
}

fn decode_output_config(
    config: Option<OutputConfigIn>,
) -> Result<(Option<Effort>, Option<StructuredOutput>), Rejection> {
    let Some(config) = config else {
        return Ok((None, None));
    };
    if config.task_budget.is_some() {
        return Err(beta(TASK_BUDGETS_BETA));
    }
    let effort = config
        .effort
        .map(|value| {
            Effort::parse(&value).ok_or_else(|| Rejection::unsupported(format!("effort {value}")))
        })
        .transpose()?;
    let structured_output = config
        .format
        .map(|format| match format.kind.as_str() {
            "json_schema" => Ok(StructuredOutput {
                name: None,
                schema: format.schema,
                strict: None,
            }),
            other => Err(Rejection::unsupported(format!("output format {other}"))),
        })
        .transpose()?;
    Ok((effort, structured_output))
}

fn decode_speed(speed: Option<&str>) -> Result<Option<Speed>, Rejection> {
    match speed {
        None | Some("standard") => Ok(None),
        Some("fast") => Ok(Some(Speed::Fast)),
        Some(other) => Err(Rejection::unsupported(format!("speed {other}"))),
    }
}

pub fn decode_response(body: &[u8]) -> Result<Response, DecodeError> {
    let wire: ResponseIn = serde_json::from_slice(body)?;
    let stop = wire
        .stop_reason
        .as_deref()
        .ok_or_else(|| DecodeError::Event("missing stop_reason".to_owned()))
        .and_then(stop_reason)?;
    let mut output = Vec::new();
    let mut parts = Vec::new();
    for raw in &wire.content {
        match response_block(raw)? {
            ResponseBlock::Text(text) => parts.push(Part::Text(text)),
            ResponseBlock::Item(item) => {
                flush(&mut output, &mut parts, Role::Assistant);
                output.push(item);
            }
        }
    }
    flush(&mut output, &mut parts, Role::Assistant);
    Ok(Response {
        id: wire.id,
        model: wire.model,
        output,
        stop,
        usage: usage(&wire.usage),
    })
}

enum ResponseBlock {
    Text(String),
    Item(Item),
}

fn response_block(raw: &RawValue) -> Result<ResponseBlock, DecodeError> {
    let text = raw.get();
    let head: BlockHead = serde_json::from_str(text)?;
    Ok(match head.kind.as_str() {
        "text" => ResponseBlock::Text(serde_json::from_str::<ResponseTextIn>(text)?.text),
        "thinking" => {
            let block: ResponseThinkingIn = serde_json::from_str(text)?;
            ResponseBlock::Item(Item::Reasoning(Reasoning {
                text: block.thinking,
                summary: Vec::new(),
                origin: reasoning_origin(block.signature.unwrap_or_default()),
            }))
        }
        "redacted_thinking" => {
            let block: ResponseRedactedIn = serde_json::from_str(text)?;
            ResponseBlock::Item(Item::Reasoning(Reasoning {
                text: String::new(),
                summary: Vec::new(),
                origin: redacted_origin(block.data),
            }))
        }
        "tool_use" => {
            let block: ResponseToolUseIn = serde_json::from_str(text)?;
            ResponseBlock::Item(Item::ToolCall(ToolCall {
                id: block.id,
                name: block.name,
                kind: ToolKind::Function,
                arguments: block.input.as_str().to_owned(),
            }))
        }
        other => {
            return Err(DecodeError::Event(format!(
                "unsupported content block {other}"
            )));
        }
    })
}

pub(super) fn stop_reason(value: &str) -> Result<StopReason, DecodeError> {
    match value {
        "end_turn" => Ok(StopReason::EndTurn),
        "max_tokens" => Ok(StopReason::MaxTokens),
        "tool_use" => Ok(StopReason::ToolUse),
        "stop_sequence" => Ok(StopReason::StopSequence),
        "refusal" => Ok(StopReason::Refusal),
        "model_context_window_exceeded" => Ok(StopReason::ContextWindow),
        other => Err(DecodeError::Event(format!("stop_reason {other}"))),
    }
}

pub(super) fn usage(wire: &UsageIn) -> Usage {
    let mut usage = Usage::default();
    merge_usage(&mut usage, wire);
    usage
}

pub(super) fn merge_usage(usage: &mut Usage, wire: &UsageIn) {
    if let Some(value) = wire.input {
        usage.input_uncached = value;
    }
    if let Some(value) = wire.cache_read {
        usage.cache_read = value;
    }
    if let Some(value) = wire.cache_write {
        usage.cache_write = value;
    }
    if let Some(value) = wire.output {
        usage.output = value;
    }
}
