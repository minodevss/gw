use bytes::Bytes;
use serde_json::Value;

use super::decode::{
    completed_stop, error_body, incomplete_stop, join_reasoning_text, join_summary,
    reasoning_origin, usage, wire_error_body,
};
use super::encode::{State, item_id, response_object, sealed_origin, state_of};
use super::types::{
    Numbered, OutputItem, OutputPart, ReasoningTextPart, ResponseObject, ServerEvent, StreamEvent,
    SummaryPart, WireError,
};
use crate::api::Api;
use crate::codec::DecodeError;
use crate::error::ErrorBody;
use crate::ir::{Block, Event, Origin, StopReason, ToolKind, Usage};
use crate::sse::{self, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Text { output: u32, content: u32 },
    Item { output: u32 },
}

#[derive(Debug)]
struct Tracked {
    key: Key,
    index: u32,
    open: bool,
    received: bool,
    summary_parts: u32,
    separator_pending: bool,
}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    next_index: u32,
    blocks: Vec<Tracked>,
    tool_called: bool,
    refused: bool,
    finished: bool,
}

impl StreamDecoder {
    pub fn push(&mut self, frame: &Frame) -> Result<Vec<Event>, DecodeError> {
        if self.finished || frame.data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: StreamEvent = serde_json::from_str(&frame.data)?;
        let mut out = Vec::new();
        self.handle(event, &mut out)?;
        Ok(out)
    }

    fn handle(&mut self, event: StreamEvent, out: &mut Vec<Event>) -> Result<(), DecodeError> {
        match event.kind.as_str() {
            "response.created" => {
                if let Some(response) = event.response {
                    out.push(Event::Start {
                        id: response.id,
                        model: response.model,
                    });
                }
            }
            "response.output_item.added" => {
                let output = required(event.output_index, "output_index")?;
                if let Some(item) = event.item {
                    self.item_added(output, item, out);
                }
            }
            "response.content_part.added" => {
                let output = required(event.output_index, "output_index")?;
                let content = required(event.content_index, "content_index")?;
                let part_kind = event
                    .part
                    .as_ref()
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str);
                if part_kind == Some("refusal") {
                    self.refused = true;
                }
                if matches!(part_kind, Some("output_text" | "refusal")) {
                    self.open_block(Key::Text { output, content }, &Block::Text, out);
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let output = required(event.output_index, "output_index")?;
                let content = required(event.content_index, "content_index")?;
                if event.kind == "response.refusal.delta" {
                    self.refused = true;
                }
                let text = event.delta.unwrap_or_default();
                let pos = self.open_block(Key::Text { output, content }, &Block::Text, out);
                self.emit_with(pos, out, |index| Event::TextDelta { index, text });
            }
            "response.content_part.done" => {
                let output = required(event.output_index, "output_index")?;
                let content = required(event.content_index, "content_index")?;
                let full = event.part.as_ref().and_then(|part| {
                    part.get("text")
                        .or_else(|| part.get("refusal"))
                        .and_then(Value::as_str)
                });
                self.finish_text(output, content, full, out);
            }
            "response.reasoning_summary_part.added" => {
                let output = required(event.output_index, "output_index")?;
                let pos = self.open_block(Key::Item { output }, &Block::Reasoning, out);
                if let Some(tracked) = self.blocks.get_mut(pos) {
                    tracked.separator_pending = tracked.summary_parts > 0;
                    tracked.summary_parts += 1;
                }
            }
            "response.reasoning_summary_text.delta" => {
                let output = required(event.output_index, "output_index")?;
                let pos = self.open_block(Key::Item { output }, &Block::Reasoning, out);
                let mut text = event.delta.unwrap_or_default();
                if let Some(tracked) = self.blocks.get_mut(pos)
                    && tracked.separator_pending
                {
                    tracked.separator_pending = false;
                    text.insert_str(0, "\n\n");
                }
                self.emit_with(pos, out, |index| Event::ReasoningSummaryDelta {
                    index,
                    text,
                });
            }
            "response.reasoning_text.delta" => {
                let output = required(event.output_index, "output_index")?;
                let pos = self.open_block(Key::Item { output }, &Block::Reasoning, out);
                let text = event.delta.unwrap_or_default();
                self.emit_with(pos, out, |index| Event::ReasoningDelta { index, text });
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let output = required(event.output_index, "output_index")?;
                let json = event.delta.unwrap_or_default();
                let pos = self.find(Key::Item { output }).ok_or_else(|| {
                    DecodeError::Event(format!("{} before item added", event.kind))
                })?;
                self.emit_with(pos, out, |index| Event::ToolArgumentsDelta { index, json });
            }
            "response.function_call_arguments.done" => {
                let output = required(event.output_index, "output_index")?;
                self.fill_arguments(output, event.arguments.as_deref(), out);
            }
            "response.custom_tool_call_input.done" => {
                let output = required(event.output_index, "output_index")?;
                self.fill_arguments(output, event.input.as_deref(), out);
            }
            "response.output_item.done" => {
                let output = required(event.output_index, "output_index")?;
                if let Some(item) = event.item {
                    self.item_done(output, item, out);
                }
            }
            "response.completed" | "response.incomplete" => {
                let response = event.response.ok_or_else(|| {
                    DecodeError::Event(format!("{} without response", event.kind))
                })?;
                self.close_all(out);
                if let Some(wire) = &response.usage {
                    out.push(Event::Usage(usage(wire)));
                }
                let stop = if event.kind == "response.incomplete" {
                    let reason = response
                        .incomplete_details
                        .as_ref()
                        .and_then(|details| details.reason.as_deref());
                    incomplete_stop(reason)?
                } else {
                    completed_stop(self.refused, self.tool_called)
                };
                out.push(Event::Stop(stop));
                self.finished = true;
            }
            "response.failed" => {
                let error = event.response.and_then(|response| response.error);
                out.push(Event::Error(wire_error_body(error.as_ref())));
                self.finished = true;
            }
            "error" => {
                out.push(Event::Error(error_body(
                    event.code.as_deref(),
                    event.message.as_deref(),
                )));
                self.finished = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn find(&self, key: Key) -> Option<usize> {
        self.blocks.iter().position(|tracked| tracked.key == key)
    }

    fn received(&self, pos: usize) -> bool {
        self.blocks.get(pos).is_some_and(|tracked| tracked.received)
    }

    fn open_block(&mut self, key: Key, block: &Block, out: &mut Vec<Event>) -> usize {
        if let Some(pos) = self.find(key) {
            return pos;
        }
        let index = self.next_index;
        self.next_index += 1;
        out.push(Event::BlockStart {
            index,
            block: block.clone(),
        });
        self.blocks.push(Tracked {
            key,
            index,
            open: true,
            received: false,
            summary_parts: 0,
            separator_pending: false,
        });
        self.blocks.len() - 1
    }

    fn emit_with(&mut self, pos: usize, out: &mut Vec<Event>, build: impl FnOnce(u32) -> Event) {
        if let Some(tracked) = self.blocks.get_mut(pos) {
            tracked.received = true;
            out.push(build(tracked.index));
        }
    }

    fn close(&mut self, pos: usize, out: &mut Vec<Event>) {
        if let Some(tracked) = self.blocks.get_mut(pos)
            && tracked.open
        {
            tracked.open = false;
            out.push(Event::BlockStop {
                index: tracked.index,
            });
        }
    }

    fn close_all(&mut self, out: &mut Vec<Event>) {
        for pos in 0..self.blocks.len() {
            self.close(pos, out);
        }
    }

    fn item_added(&mut self, output: u32, item: OutputItem, out: &mut Vec<Event>) {
        match item {
            OutputItem::Message { .. } => {}
            OutputItem::Reasoning { .. } => {
                self.open_block(Key::Item { output }, &Block::Reasoning, out);
            }
            OutputItem::FunctionCall { call_id, name, .. } => {
                self.open_tool(output, call_id, name, ToolKind::Function, out);
            }
            OutputItem::CustomToolCall { call_id, name, .. } => {
                self.open_tool(output, call_id, name, ToolKind::Custom, out);
            }
        }
    }

    fn open_tool(
        &mut self,
        output: u32,
        id: String,
        name: String,
        kind: ToolKind,
        out: &mut Vec<Event>,
    ) -> usize {
        self.tool_called = true;
        self.open_block(
            Key::Item { output },
            &Block::ToolCall { id, name, kind },
            out,
        )
    }

    fn fill_arguments(&mut self, output: u32, full: Option<&str>, out: &mut Vec<Event>) {
        let Some(pos) = self.find(Key::Item { output }) else {
            return;
        };
        let received = self.received(pos);
        if let Some(json) = full.filter(|json| !received && !json.is_empty()) {
            let json = json.to_owned();
            self.emit_with(pos, out, |index| Event::ToolArgumentsDelta { index, json });
        }
    }

    fn finish_text(&mut self, output: u32, content: u32, full: Option<&str>, out: &mut Vec<Event>) {
        let pos = self.open_block(Key::Text { output, content }, &Block::Text, out);
        let received = self.received(pos);
        if let Some(text) = full.filter(|text| !received && !text.is_empty()) {
            let text = text.to_owned();
            self.emit_with(pos, out, |index| Event::TextDelta { index, text });
        }
        self.close(pos, out);
    }

    fn item_done(&mut self, output: u32, item: OutputItem, out: &mut Vec<Event>) {
        match item {
            OutputItem::Message { content, .. } => {
                for (position, part) in content.iter().enumerate() {
                    let content_index = u32::try_from(position).unwrap_or(u32::MAX);
                    let text = match part {
                        OutputPart::OutputText { text, .. } => text,
                        OutputPart::Refusal { refusal } => {
                            self.refused = true;
                            refusal
                        }
                    };
                    self.finish_text(output, content_index, Some(text), out);
                }
                for pos in 0..self.blocks.len() {
                    let belongs = self.blocks.get(pos).is_some_and(
                        |tracked| matches!(tracked.key, Key::Text { output: o, .. } if o == output),
                    );
                    if belongs {
                        self.close(pos, out);
                    }
                }
            }
            OutputItem::Reasoning {
                id,
                summary,
                content,
                encrypted_content,
                ..
            } => {
                let pos = self.open_block(Key::Item { output }, &Block::Reasoning, out);
                if !self.received(pos) {
                    let text = content.as_deref().map_or_default(join_reasoning_text);
                    if !text.is_empty() {
                        self.emit_with(pos, out, |index| Event::ReasoningDelta { index, text });
                    }
                    let text = join_summary(&summary);
                    if !text.is_empty() {
                        self.emit_with(pos, out, |index| Event::ReasoningSummaryDelta {
                            index,
                            text,
                        });
                    }
                }
                let origin = reasoning_origin(&id, encrypted_content);
                self.emit_with(pos, out, |index| Event::ReasoningOrigin { index, origin });
                self.close(pos, out);
            }
            OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                let pos = self.open_tool(output, call_id, name, ToolKind::Function, out);
                self.fill_arguments(output, Some(&arguments), out);
                self.close(pos, out);
            }
            OutputItem::CustomToolCall {
                call_id,
                name,
                input,
                ..
            } => {
                let pos = self.open_tool(output, call_id, name, ToolKind::Custom, out);
                self.fill_arguments(output, Some(&input), out);
                self.close(pos, out);
            }
        }
    }
}

fn required(value: Option<u32>, field: &str) -> Result<u32, DecodeError> {
    value.ok_or_else(|| DecodeError::Event(format!("missing {field}")))
}

#[derive(Debug)]
enum Content {
    Text(String),
    Reasoning {
        text: String,
        summary: String,
        origin: Option<Origin>,
    },
    Tool {
        call_id: String,
        name: String,
        kind: ToolKind,
        arguments: String,
    },
}

#[derive(Debug)]
struct Open {
    index: u32,
    output: u32,
    item_id: String,
    content: Content,
}

#[derive(Debug, Default)]
pub struct StreamEncoder {
    sequence: u64,
    id: String,
    model: String,
    started: bool,
    finished: bool,
    next_output: u32,
    open: Vec<Open>,
    done: Vec<(u32, OutputItem)>,
    usage: Option<Usage>,
    stop: Option<StopReason>,
}

impl StreamEncoder {
    pub fn push(&mut self, event: &Event) -> Vec<Bytes> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match event {
            Event::Start { id, model } => {
                if !self.started {
                    self.start(id, model, &mut out);
                }
            }
            Event::BlockStart { index, block } => {
                self.ensure_started(&mut out);
                self.block_start(*index, block, &mut out);
            }
            Event::TextDelta { index, text } => self.text_delta(*index, text, &mut out),
            Event::ReasoningDelta { index, text } => self.reasoning_delta(*index, text, &mut out),
            Event::ReasoningSummaryDelta { index, text } => {
                self.summary_delta(*index, text, &mut out);
            }
            Event::ReasoningOrigin { index, origin } => {
                if let Some(open) = self.find_mut(*index)
                    && let Content::Reasoning { origin: slot, .. } = &mut open.content
                {
                    *slot = Some(origin.clone());
                }
            }
            Event::ToolArgumentsDelta { index, json } => {
                self.arguments_delta(*index, json, &mut out);
            }
            Event::BlockStop { index } => self.block_stop(*index, &mut out),
            Event::Usage(usage) => self.usage = Some(*usage),
            Event::Stop(stop) => self.stop = Some(*stop),
            Event::Error(body) => self.fail(body, &mut out),
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        if self.finished || !self.started {
            self.finished = true;
            return out;
        }
        let indices: Vec<u32> = self.open.iter().map(|open| open.index).collect();
        for index in indices {
            self.block_stop(index, &mut out);
        }
        self.finished = true;
        let state = match self.stop {
            Some(stop) => state_of(stop),
            None => State::Failed(WireError {
                code: Some("server_error".to_owned()),
                message: Some("upstream stream ended before completion".to_owned()),
            }),
        };
        let build: fn(ResponseObject) -> ServerEvent = match &state {
            State::InProgress | State::Completed => |response| ServerEvent::Completed { response },
            State::Incomplete(_) => |response| ServerEvent::Incomplete { response },
            State::Failed(_) => |response| ServerEvent::Failed { response },
        };
        let response = response_object(
            &self.id,
            &self.model,
            state,
            self.sorted_output(),
            self.usage.as_ref(),
        );
        self.emit(&mut out, build(response));
        out
    }

    fn sorted_output(&self) -> Vec<OutputItem> {
        let mut done = self.done.clone();
        done.sort_by_key(|(output, _)| *output);
        done.into_iter().map(|(_, item)| item).collect()
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "call sites build the event inline"
    )]
    fn emit(&mut self, out: &mut Vec<Bytes>, event: ServerEvent) {
        let numbered = Numbered {
            event: &event,
            sequence_number: self.sequence,
        };
        self.sequence += 1;
        let json = serde_json::to_string(&numbered).unwrap_or_default();
        out.push(sse::frame(Some(event.name()), &json));
    }

    fn find_mut(&mut self, index: u32) -> Option<&mut Open> {
        self.open.iter_mut().find(|open| open.index == index)
    }

    fn start(&mut self, id: &str, model: &str, out: &mut Vec<Bytes>) {
        self.started = true;
        id.clone_into(&mut self.id);
        model.clone_into(&mut self.model);
        let created = response_object(id, model, State::InProgress, Vec::new(), None);
        self.emit(out, ServerEvent::Created { response: created });
        let progress = response_object(id, model, State::InProgress, Vec::new(), None);
        self.emit(out, ServerEvent::InProgress { response: progress });
    }

    fn ensure_started(&mut self, out: &mut Vec<Bytes>) {
        if !self.started {
            self.start("", "", out);
        }
    }

    fn block_start(&mut self, index: u32, block: &Block, out: &mut Vec<Bytes>) {
        let output = self.next_output;
        self.next_output += 1;
        let (item_id, content, item) = match block {
            Block::Text => {
                let id = item_id("msg", output);
                let item = OutputItem::Message {
                    id: id.clone(),
                    role: "assistant".to_owned(),
                    content: Vec::new(),
                    status: Some("in_progress".to_owned()),
                };
                (id, Content::Text(String::new()), item)
            }
            Block::Reasoning => {
                let id = item_id("rs", output);
                let item = OutputItem::Reasoning {
                    id: id.clone(),
                    summary: Vec::new(),
                    content: None,
                    encrypted_content: None,
                    status: Some("in_progress".to_owned()),
                };
                let content = Content::Reasoning {
                    text: String::new(),
                    summary: String::new(),
                    origin: None,
                };
                (id, content, item)
            }
            Block::ToolCall { id, name, kind } => {
                let (id_prefix, item) = match kind {
                    ToolKind::Function => (
                        "fc",
                        OutputItem::FunctionCall {
                            id: item_id("fc", output),
                            call_id: id.clone(),
                            name: name.clone(),
                            arguments: String::new(),
                            status: Some("in_progress".to_owned()),
                        },
                    ),
                    ToolKind::Custom => (
                        "ctc",
                        OutputItem::CustomToolCall {
                            id: item_id("ctc", output),
                            call_id: id.clone(),
                            name: name.clone(),
                            input: String::new(),
                            status: Some("in_progress".to_owned()),
                        },
                    ),
                };
                let content = Content::Tool {
                    call_id: id.clone(),
                    name: name.clone(),
                    kind: *kind,
                    arguments: String::new(),
                };
                (item_id(id_prefix, output), content, item)
            }
        };
        self.emit(
            out,
            ServerEvent::OutputItemAdded {
                output_index: output,
                item,
            },
        );
        if matches!(content, Content::Text(_)) {
            self.emit(
                out,
                ServerEvent::ContentPartAdded {
                    item_id: item_id.clone(),
                    output_index: output,
                    content_index: 0,
                    part: OutputPart::OutputText {
                        text: String::new(),
                        annotations: Vec::new(),
                    },
                },
            );
        }
        self.open.push(Open {
            index,
            output,
            item_id,
            content,
        });
    }

    fn text_delta(&mut self, index: u32, delta: &str, out: &mut Vec<Bytes>) {
        let Some(open) = self.find_mut(index) else {
            return;
        };
        let Content::Text(buffer) = &mut open.content else {
            return;
        };
        buffer.push_str(delta);
        let event = ServerEvent::OutputTextDelta {
            item_id: open.item_id.clone(),
            output_index: open.output,
            content_index: 0,
            delta: delta.to_owned(),
            logprobs: Vec::new(),
        };
        self.emit(out, event);
    }

    fn reasoning_delta(&mut self, index: u32, delta: &str, out: &mut Vec<Bytes>) {
        let Some(open) = self.find_mut(index) else {
            return;
        };
        let Content::Reasoning { text, .. } = &mut open.content else {
            return;
        };
        text.push_str(delta);
        let event = ServerEvent::ReasoningTextDelta {
            item_id: open.item_id.clone(),
            output_index: open.output,
            content_index: 0,
            delta: delta.to_owned(),
        };
        self.emit(out, event);
    }

    fn summary_delta(&mut self, index: u32, delta: &str, out: &mut Vec<Bytes>) {
        let Some(open) = self.find_mut(index) else {
            return;
        };
        let Content::Reasoning { summary, .. } = &mut open.content else {
            return;
        };
        let first = summary.is_empty();
        summary.push_str(delta);
        let item_id = open.item_id.clone();
        let output_index = open.output;
        if first {
            self.emit(
                out,
                ServerEvent::ReasoningSummaryPartAdded {
                    item_id: item_id.clone(),
                    output_index,
                    summary_index: 0,
                    part: SummaryPart::SummaryText {
                        text: String::new(),
                    },
                },
            );
        }
        self.emit(
            out,
            ServerEvent::ReasoningSummaryTextDelta {
                item_id,
                output_index,
                summary_index: 0,
                delta: delta.to_owned(),
            },
        );
    }

    fn arguments_delta(&mut self, index: u32, delta: &str, out: &mut Vec<Bytes>) {
        let Some(open) = self.find_mut(index) else {
            return;
        };
        let Content::Tool {
            kind, arguments, ..
        } = &mut open.content
        else {
            return;
        };
        arguments.push_str(delta);
        let item_id = open.item_id.clone();
        let output_index = open.output;
        let delta = delta.to_owned();
        let event = match kind {
            ToolKind::Function => ServerEvent::FunctionCallArgumentsDelta {
                item_id,
                output_index,
                delta,
            },
            ToolKind::Custom => ServerEvent::CustomToolCallInputDelta {
                item_id,
                output_index,
                delta,
            },
        };
        self.emit(out, event);
    }

    fn block_stop(&mut self, index: u32, out: &mut Vec<Bytes>) {
        let Some(pos) = self.open.iter().position(|open| open.index == index) else {
            return;
        };
        let Open {
            output,
            item_id,
            content,
            ..
        } = self.open.remove(pos);
        let item = match content {
            Content::Text(text) => self.text_done(output, item_id, text, out),
            Content::Reasoning {
                text,
                summary,
                origin,
            } => self.reasoning_done(output, item_id, text, summary, origin.as_ref(), out),
            Content::Tool {
                call_id,
                name,
                kind,
                arguments,
            } => self.tool_done(output, item_id, call_id, name, kind, arguments, out),
        };
        self.emit(
            out,
            ServerEvent::OutputItemDone {
                output_index: output,
                item: item.clone(),
            },
        );
        self.done.push((output, item));
    }

    fn text_done(
        &mut self,
        output: u32,
        item_id: String,
        text: String,
        out: &mut Vec<Bytes>,
    ) -> OutputItem {
        let part = OutputPart::OutputText {
            text: text.clone(),
            annotations: Vec::new(),
        };
        self.emit(
            out,
            ServerEvent::OutputTextDone {
                item_id: item_id.clone(),
                output_index: output,
                content_index: 0,
                text,
                logprobs: Vec::new(),
            },
        );
        self.emit(
            out,
            ServerEvent::ContentPartDone {
                item_id: item_id.clone(),
                output_index: output,
                content_index: 0,
                part: part.clone(),
            },
        );
        OutputItem::Message {
            id: item_id,
            role: "assistant".to_owned(),
            content: vec![part],
            status: Some("completed".to_owned()),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the fields of one wire item"
    )]
    fn tool_done(
        &mut self,
        output: u32,
        item_id: String,
        call_id: String,
        name: String,
        kind: ToolKind,
        arguments: String,
        out: &mut Vec<Bytes>,
    ) -> OutputItem {
        match kind {
            ToolKind::Function => {
                self.emit(
                    out,
                    ServerEvent::FunctionCallArgumentsDone {
                        item_id: item_id.clone(),
                        output_index: output,
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                );
                OutputItem::FunctionCall {
                    id: item_id,
                    call_id,
                    name,
                    arguments,
                    status: Some("completed".to_owned()),
                }
            }
            ToolKind::Custom => {
                self.emit(
                    out,
                    ServerEvent::CustomToolCallInputDone {
                        item_id: item_id.clone(),
                        output_index: output,
                        input: arguments.clone(),
                    },
                );
                OutputItem::CustomToolCall {
                    id: item_id,
                    call_id,
                    name,
                    input: arguments,
                    status: Some("completed".to_owned()),
                }
            }
        }
    }

    fn reasoning_done(
        &mut self,
        output: u32,
        item_id: String,
        text: String,
        summary: String,
        origin: Option<&Origin>,
        out: &mut Vec<Bytes>,
    ) -> OutputItem {
        let mut summary_parts = Vec::new();
        if !summary.is_empty() {
            self.emit(
                out,
                ServerEvent::ReasoningSummaryTextDone {
                    item_id: item_id.clone(),
                    output_index: output,
                    summary_index: 0,
                    text: summary.clone(),
                },
            );
            let part = SummaryPart::SummaryText { text: summary };
            self.emit(
                out,
                ServerEvent::ReasoningSummaryPartDone {
                    item_id: item_id.clone(),
                    output_index: output,
                    summary_index: 0,
                    part: part.clone(),
                },
            );
            summary_parts.push(part);
        }
        let content = if text.is_empty() {
            None
        } else {
            self.emit(
                out,
                ServerEvent::ReasoningTextDone {
                    item_id: item_id.clone(),
                    output_index: output,
                    content_index: 0,
                    text: text.clone(),
                },
            );
            Some(vec![ReasoningTextPart::ReasoningText { text }])
        };
        OutputItem::Reasoning {
            id: item_id,
            summary: summary_parts,
            content,
            encrypted_content: Some(sealed_origin(origin)),
            status: Some("completed".to_owned()),
        }
    }

    fn fail(&mut self, body: &ErrorBody, out: &mut Vec<Bytes>) {
        self.finished = true;
        if !self.started {
            out.push(body.sse(Api::Responses));
            return;
        }
        let value: Option<Value> = serde_json::from_slice(&body.json(Api::Responses)).ok();
        let code = value
            .as_ref()
            .and_then(|value| value.pointer("/error/code"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let error = WireError {
            code,
            message: Some(body.message.clone()),
        };
        let response = response_object(
            &self.id,
            &self.model,
            State::Failed(error),
            self.sorted_output(),
            self.usage.as_ref(),
        );
        self.emit(out, ServerEvent::Failed { response });
    }
}
