use bytes::Bytes;

use super::types::{
    CallType, Choice, Completion, ContentPart, FileInput, FormatType, FunctionDef, ImageUrl,
    JsonSchemaDef, Message, MessageRole, OutRequest, ResponseFormat, ResponseMessage,
    StreamOptions, StringOrList, ToolCall, ToolCallFunction, ToolChoiceOut, ToolDef, Usage,
};
use crate::codec::{EncodeOptions, EncodeResult, Encoded, MaxTokensField, Note};
use crate::ir::{
    self, Effort, Item, Media, Origin, Part, Reasoning, Speed, StopReason, Tool, ToolChoice,
    ToolKind,
};
use crate::reject::Rejection;

pub fn encode_request(request: &ir::Request, options: &EncodeOptions) -> EncodeResult {
    let mut notes = Vec::new();
    let streaming = request.stream || options.force_stream;
    if options.force_stream && !request.stream {
        notes.push(Note::StreamForced);
    }

    let tools = request
        .tools
        .iter()
        .map(encode_tool)
        .collect::<Result<Vec<_>, _>>()?;
    let parallel_tool_calls = if tools.is_empty() {
        None
    } else {
        request.parallel_tool_calls
    };

    let body = OutRequest {
        model: request.model.clone(),
        stream: streaming.then_some(true),
        stream_options: streaming.then_some(StreamOptions {
            include_usage: Some(true),
            include_obfuscation: None,
        }),
        max_completion_tokens: (options.max_tokens_field == MaxTokensField::MaxCompletionTokens)
            .then_some(request.max_output_tokens)
            .flatten(),
        max_tokens: (options.max_tokens_field == MaxTokensField::MaxTokens)
            .then_some(request.max_output_tokens)
            .flatten(),
        temperature: request.temperature,
        top_p: request.top_p,
        stop: (!request.stop.is_empty()).then(|| StringOrList::List(request.stop.clone())),
        reasoning_effort: request.effort.map(encode_effort).transpose()?,
        service_tier: request.speed.map(|Speed::Fast| "priority"),
        response_format: request
            .structured_output
            .as_ref()
            .map(encode_structured_output)
            .transpose()?,
        prompt_cache_key: options
            .cache_key
            .clone()
            .or_else(|| request.cache_key.clone()),
        user: request.user.clone(),
        tool_choice: request.tool_choice.as_ref().map(encode_tool_choice),
        parallel_tool_calls,
        tools,
        messages: encode_messages(request, &mut notes)?,
    };
    reject_thinking(request)?;

    notes.sort();
    notes.dedup();
    let body = serde_json::to_vec(&body)
        .map_err(|error| Rejection::unsupported(format!("chat request serialization: {error}")))?;
    Ok(Encoded {
        body: Bytes::from(body),
        notes,
    })
}

pub fn encode_response(response: &ir::Response) -> Bytes {
    let mut reasoning = Vec::new();
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for item in &response.output {
        match item {
            Item::Reasoning(item) => {
                if let Some(text) = reasoning_text(item) {
                    reasoning.push(text);
                }
            }
            Item::Message {
                role: ir::Role::Assistant,
                parts,
            } => {
                for part in parts {
                    if let Part::Text(text) = part {
                        content.push_str(text);
                    }
                }
            }
            Item::ToolCall(call) => tool_calls.push(wire_tool_call(call)),
            Item::Message {
                role: ir::Role::User,
                ..
            }
            | Item::System { .. }
            | Item::ToolResult(_) => {}
        }
    }
    let content = (!content.is_empty() || tool_calls.is_empty()).then_some(content);
    let completion = Completion {
        id: response.id.clone(),
        object: "chat.completion".to_owned(),
        created: 0,
        model: response.model.clone(),
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant".to_owned(),
                content,
                refusal: None,
                reasoning_content: (!reasoning.is_empty()).then(|| reasoning.join("\n\n")),
                tool_calls,
            },
            finish_reason: Some(stop_to_wire(response.stop).to_owned()),
        }],
        usage: Some(usage_to_wire(response.usage)),
    };
    Bytes::from(serde_json::to_vec(&completion).unwrap_or_default())
}

pub(super) fn stop_to_wire(stop: StopReason) -> &'static str {
    match stop {
        StopReason::EndTurn | StopReason::StopSequence | StopReason::Refusal => "stop",
        StopReason::MaxTokens | StopReason::ContextWindow => "length",
        StopReason::ToolUse => "tool_calls",
        StopReason::ContentFilter => "content_filter",
    }
}

pub(super) fn usage_to_wire(usage: ir::Usage) -> Usage {
    let prompt_tokens = usage.input_total();
    Usage {
        prompt_tokens,
        completion_tokens: usage.output,
        total_tokens: prompt_tokens + usage.output,
        prompt_tokens_details: Some(super::types::PromptTokensDetails {
            cached_tokens: usage.cache_read,
        }),
        completion_tokens_details: usage.reasoning.map(|reasoning_tokens| {
            super::types::CompletionTokensDetails {
                reasoning_tokens: Some(reasoning_tokens),
            }
        }),
        cached_tokens: None,
    }
}

fn reject_thinking(request: &ir::Request) -> Result<(), Rejection> {
    match request.thinking {
        None | Some(ir::Thinking::Adaptive) => Ok(()),
        Some(thinking @ ir::Thinking::Enabled { .. }) => {
            Err(Rejection::ThinkingType(thinking.kind()))
        }
        Some(ir::Thinking::Disabled) => Err(Rejection::unsupported("thinking disabled")),
    }
}

fn encode_effort(effort: Effort) -> Result<&'static str, Rejection> {
    match effort {
        Effort::Max => Err(Rejection::Effort),
        Effort::None
        | Effort::Minimal
        | Effort::Low
        | Effort::Medium
        | Effort::High
        | Effort::Xhigh => Ok(effort.as_str()),
    }
}

fn encode_structured_output(output: &ir::StructuredOutput) -> Result<ResponseFormat, Rejection> {
    let name = output.name.clone().ok_or(Rejection::StructuredOutputs)?;
    Ok(ResponseFormat {
        kind: FormatType::JsonSchema,
        json_schema: Some(JsonSchemaDef {
            name,
            description: None,
            schema: Some(output.schema.clone()),
            strict: output.strict,
        }),
    })
}

fn encode_tool(tool: &Tool) -> Result<ToolDef, Rejection> {
    match tool {
        Tool::Function {
            name,
            description,
            parameters,
            strict,
        } => Ok(ToolDef {
            kind: CallType::Function,
            function: Some(FunctionDef {
                name: name.clone(),
                description: description.clone(),
                parameters: Some(parameters.clone()),
                strict: *strict,
            }),
            custom: None,
        }),
        Tool::Custom { .. } => Err(Rejection::unsupported("custom tool")),
    }
}

fn encode_tool_choice(choice: &ToolChoice) -> ToolChoiceOut {
    match choice {
        ToolChoice::Auto => ToolChoiceOut::Mode("auto"),
        ToolChoice::None => ToolChoiceOut::Mode("none"),
        ToolChoice::Required => ToolChoiceOut::Mode("required"),
        ToolChoice::Named(name) => ToolChoiceOut::Named(name.clone()),
    }
}

fn wire_tool_call(call: &ir::ToolCall) -> ToolCall {
    ToolCall {
        id: call.id.clone(),
        kind: CallType::Function,
        function: Some(ToolCallFunction {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }),
        custom: None,
    }
}

fn reasoning_text(reasoning: &Reasoning) -> Option<String> {
    let text = if reasoning.text.is_empty() {
        reasoning.summary.join("\n\n")
    } else {
        reasoning.text.clone()
    };
    (!text.is_empty()).then_some(text)
}

#[derive(Default)]
struct AssistantDraft {
    reasoning: Vec<String>,
    parts: Vec<Part>,
    tool_calls: Vec<ToolCall>,
}

impl AssistantDraft {
    fn is_empty(&self) -> bool {
        self.reasoning.is_empty() && self.parts.is_empty() && self.tool_calls.is_empty()
    }

    fn has_output(&self) -> bool {
        !self.parts.is_empty() || !self.tool_calls.is_empty()
    }

    fn into_message(self) -> Result<Message, Rejection> {
        let content = if self.parts.is_empty() && !self.tool_calls.is_empty() {
            None
        } else {
            Some(text_content(&self.parts)?)
        };
        Ok(Message {
            role: MessageRole::Assistant,
            content,
            reasoning_content: (!self.reasoning.is_empty()).then(|| self.reasoning.join("\n\n")),
            tool_calls: self.tool_calls,
            tool_call_id: None,
            name: None,
            refusal: None,
            function_call: None,
            audio: None,
        })
    }
}

fn encode_messages(
    request: &ir::Request,
    notes: &mut Vec<Note>,
) -> Result<Vec<Message>, Rejection> {
    let mut messages = Vec::new();
    system_messages(&request.system, &mut messages)?;
    let mut draft = AssistantDraft::default();
    for item in &request.items {
        match item {
            Item::Reasoning(reasoning) => {
                if reasoning.origin != Origin::Plain {
                    return Err(Rejection::ThinkingSignature);
                }
                if draft.has_output() {
                    flush(&mut draft, &mut messages)?;
                }
                draft.reasoning.extend(reasoning_text(reasoning));
            }
            Item::Message {
                role: ir::Role::Assistant,
                parts,
            } => {
                if !draft.tool_calls.is_empty() {
                    flush(&mut draft, &mut messages)?;
                }
                draft.parts.extend(parts.iter().cloned());
            }
            Item::ToolCall(call) => {
                if call.kind == ToolKind::Custom {
                    return Err(Rejection::unsupported("custom tool call"));
                }
                draft.tool_calls.push(wire_tool_call(call));
            }
            Item::Message {
                role: ir::Role::User,
                parts,
            } => {
                flush(&mut draft, &mut messages)?;
                messages.push(plain_message(MessageRole::User, user_content(parts)?));
            }
            Item::System { parts } => {
                flush(&mut draft, &mut messages)?;
                system_messages(parts, &mut messages)?;
            }
            Item::ToolResult(result) => {
                flush(&mut draft, &mut messages)?;
                if result.kind == ToolKind::Custom {
                    return Err(Rejection::unsupported("custom tool result"));
                }
                if result.is_error {
                    notes.push(Note::ToolErrorFlagDropped);
                }
                let mut message = plain_message(MessageRole::Tool, text_content(&result.parts)?);
                message.tool_call_id = Some(result.call_id.clone());
                messages.push(message);
            }
        }
    }
    flush(&mut draft, &mut messages)?;
    Ok(messages)
}

fn flush(draft: &mut AssistantDraft, messages: &mut Vec<Message>) -> Result<(), Rejection> {
    if !draft.is_empty() {
        messages.push(std::mem::take(draft).into_message()?);
    }
    Ok(())
}

fn system_messages(parts: &[Part], messages: &mut Vec<Message>) -> Result<(), Rejection> {
    for part in parts {
        match part {
            Part::Text(text) => messages.push(plain_message(
                MessageRole::System,
                StringOrList::Text(text.clone()),
            )),
            Part::Image(_) => return Err(Rejection::ImageBlock),
            Part::Document(_) => return Err(Rejection::DocumentBlock),
        }
    }
    Ok(())
}

fn plain_message(role: MessageRole, content: StringOrList<ContentPart>) -> Message {
    Message {
        role,
        content: Some(content),
        reasoning_content: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
        refusal: None,
        function_call: None,
        audio: None,
    }
}

fn text_content(parts: &[Part]) -> Result<StringOrList<ContentPart>, Rejection> {
    let mut texts = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => texts.push(text.as_str()),
            Part::Image(_) => return Err(Rejection::ImageBlock),
            Part::Document(_) => return Err(Rejection::DocumentBlock),
        }
    }
    Ok(match texts.as_slice() {
        [] => StringOrList::Text(String::new()),
        [single] => StringOrList::Text((*single).to_owned()),
        many => StringOrList::List(
            many.iter()
                .map(|text| ContentPart::Text {
                    text: (*text).to_owned(),
                })
                .collect(),
        ),
    })
}

fn user_content(parts: &[Part]) -> Result<StringOrList<ContentPart>, Rejection> {
    if let [Part::Text(text)] = parts {
        return Ok(StringOrList::Text(text.clone()));
    }
    if parts.is_empty() {
        return Ok(StringOrList::Text(String::new()));
    }
    parts
        .iter()
        .map(user_part)
        .collect::<Result<Vec<_>, _>>()
        .map(StringOrList::List)
}

fn user_part(part: &Part) -> Result<ContentPart, Rejection> {
    match part {
        Part::Text(text) => Ok(ContentPart::Text { text: text.clone() }),
        Part::Image(media) => Ok(ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: media_url(media),
                detail: None,
            },
        }),
        Part::Document(document) => match (&document.media, &document.title) {
            (Media::Base64 { .. }, Some(title)) => Ok(ContentPart::File {
                file: FileInput {
                    file_data: Some(media_url(&document.media)),
                    file_id: None,
                    filename: Some(title.clone()),
                },
            }),
            (Media::Base64 { .. }, None) | (Media::Url(_), _) => Err(Rejection::DocumentBlock),
        },
    }
}

fn media_url(media: &Media) -> String {
    match media {
        Media::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
        Media::Url(url) => url.clone(),
    }
}
