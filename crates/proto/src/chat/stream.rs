use std::collections::BTreeMap;

use bytes::Bytes;
use serde::Serialize;

use super::decode::{stop_from_wire, usage_from_wire};
use super::encode::{stop_to_wire, usage_to_wire};
use super::types::{CallType, Chunk, ChunkChoice, Delta, DeltaFunction, DeltaToolCall};
use crate::api::Api;
use crate::codec::DecodeError;
use crate::error::{ErrorBody, ErrorKind};
use crate::ir::{Block, Event, StopReason, ToolKind, Usage};
use crate::sse;

const DONE: &str = "[DONE]";

#[derive(Debug, Default)]
pub struct StreamDecoder {
    started: bool,
    next_index: u32,
    text: Option<u32>,
    reasoning: Option<u32>,
    tools: BTreeMap<u32, u32>,
    open: Vec<u32>,
    pending_stop: Option<StopReason>,
}

impl StreamDecoder {
    pub fn push(&mut self, frame: &sse::Frame) -> Result<Vec<Event>, DecodeError> {
        let data = frame.data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        if data == DONE {
            return Ok(self
                .pending_stop
                .take()
                .map(Event::Stop)
                .into_iter()
                .collect());
        }
        let chunk: Chunk = serde_json::from_str(data)?;
        let mut events = Vec::new();
        if let Some(error) = chunk.error {
            let body = match error.kind.as_deref() {
                Some("invalid_request_error") => {
                    ErrorBody::new(400, ErrorKind::InvalidRequest, error.message)
                }
                Some("authentication_error") => {
                    ErrorBody::new(401, ErrorKind::Authentication, error.message)
                }
                Some("rate_limit_error") => {
                    ErrorBody::new(429, ErrorKind::RateLimit, error.message)
                }
                _ => ErrorBody::new(500, ErrorKind::Server, error.message),
            };
            events.push(Event::Error(body));
            return Ok(events);
        }
        if !self.started {
            self.started = true;
            events.push(Event::Start {
                id: chunk.id,
                model: chunk.model,
            });
        }

        let mut usage = chunk.usage;
        for choice in chunk.choices {
            if choice.index != 0 {
                return Err(DecodeError::Event(format!(
                    "unexpected choice index {}",
                    choice.index
                )));
            }
            self.delta(choice.delta, &mut events)?;
            if let Some(reason) = choice.finish_reason {
                self.close_all(&mut events);
                self.pending_stop = Some(stop_from_wire(&reason)?);
            }
            usage = usage.or(choice.usage);
        }
        if let Some(usage) = usage {
            events.push(Event::Usage(usage_from_wire(usage)));
            if let Some(stop) = self.pending_stop.take() {
                events.push(Event::Stop(stop));
            }
        }
        Ok(events)
    }

    fn delta(&mut self, delta: Delta, events: &mut Vec<Event>) -> Result<(), DecodeError> {
        if let Some(text) = delta.reasoning_content.filter(|text| !text.is_empty()) {
            self.close_slot(Slot::Text, events);
            let index = self.reasoning_block(events);
            events.push(Event::ReasoningDelta { index, text });
        }
        if let Some(text) = delta.content.filter(|text| !text.is_empty()) {
            self.close_slot(Slot::Reasoning, events);
            let index = self.text_block(events);
            events.push(Event::TextDelta { index, text });
        }
        for call in delta.tool_calls {
            if call.kind == Some(CallType::Custom) {
                return Err(DecodeError::Event("custom tool call".to_owned()));
            }
            let function = call.function.unwrap_or(DeltaFunction {
                name: None,
                arguments: None,
            });
            let index = if let Some(index) = self.tools.get(&call.index) {
                *index
            } else {
                let (Some(id), Some(name)) = (call.id, function.name) else {
                    return Err(DecodeError::Event(
                        "tool call started without id and name".to_owned(),
                    ));
                };
                self.close_slot(Slot::Text, events);
                self.close_slot(Slot::Reasoning, events);
                let index = self.open_block(
                    Block::ToolCall {
                        id,
                        name,
                        kind: ToolKind::Function,
                    },
                    events,
                );
                self.tools.insert(call.index, index);
                index
            };
            if let Some(json) = function.arguments.filter(|json| !json.is_empty()) {
                events.push(Event::ToolArgumentsDelta { index, json });
            }
        }
        Ok(())
    }

    fn open_block(&mut self, block: Block, events: &mut Vec<Event>) -> u32 {
        let index = self.next_index;
        self.next_index += 1;
        self.open.push(index);
        events.push(Event::BlockStart { index, block });
        index
    }

    fn text_block(&mut self, events: &mut Vec<Event>) -> u32 {
        if let Some(index) = self.text {
            return index;
        }
        let index = self.open_block(Block::Text, events);
        self.text = Some(index);
        index
    }

    fn reasoning_block(&mut self, events: &mut Vec<Event>) -> u32 {
        if let Some(index) = self.reasoning {
            return index;
        }
        let index = self.open_block(Block::Reasoning, events);
        self.reasoning = Some(index);
        index
    }

    fn close_slot(&mut self, slot: Slot, events: &mut Vec<Event>) {
        let index = match slot {
            Slot::Text => self.text.take(),
            Slot::Reasoning => self.reasoning.take(),
        };
        if let Some(index) = index {
            self.open.retain(|open| *open != index);
            events.push(Event::BlockStop { index });
        }
    }

    fn close_all(&mut self, events: &mut Vec<Event>) {
        self.text = None;
        self.reasoning = None;
        self.tools.clear();
        events.extend(self.open.drain(..).map(|index| Event::BlockStop { index }));
    }
}

#[derive(Clone, Copy)]
enum Slot {
    Text,
    Reasoning,
}

#[derive(Debug, Default)]
pub struct StreamEncoder {
    id: String,
    model: String,
    tools: BTreeMap<u32, u32>,
    usage: Option<Usage>,
    finished: bool,
}

impl StreamEncoder {
    pub fn push(&mut self, event: &Event) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        match event {
            Event::Start { id, model } => {
                id.clone_into(&mut self.id);
                model.clone_into(&mut self.model);
                self.delta(
                    Delta {
                        role: Some("assistant".to_owned()),
                        content: Some(String::new()),
                        ..Delta::default()
                    },
                    None,
                )
            }
            Event::BlockStart {
                index,
                block: Block::ToolCall { id, name, .. },
            } => {
                let position = u32::try_from(self.tools.len()).unwrap_or(u32::MAX);
                self.tools.insert(*index, position);
                self.tool_delta(
                    position,
                    Some(id.clone()),
                    DeltaFunction {
                        name: Some(name.clone()),
                        arguments: Some(String::new()),
                    },
                )
            }
            Event::TextDelta { text, .. } => self.delta(
                Delta {
                    content: Some(text.clone()),
                    ..Delta::default()
                },
                None,
            ),
            Event::ReasoningDelta { text, .. } | Event::ReasoningSummaryDelta { text, .. } => self
                .delta(
                    Delta {
                        reasoning_content: Some(text.clone()),
                        ..Delta::default()
                    },
                    None,
                ),
            Event::ToolArgumentsDelta { index, json } => match self.tools.get(index) {
                Some(position) => self.tool_delta(
                    *position,
                    None,
                    DeltaFunction {
                        name: None,
                        arguments: Some(json.clone()),
                    },
                ),
                None => Vec::new(),
            },
            Event::Usage(usage) => {
                self.usage = Some(*usage);
                Vec::new()
            }
            Event::Stop(stop) => self.delta(Delta::default(), Some(stop_to_wire(*stop))),
            Event::Error(body) => {
                self.finished = true;
                vec![body.sse(Api::Chat)]
            }
            Event::BlockStart {
                block: Block::Text | Block::Reasoning,
                ..
            }
            | Event::ReasoningOrigin { .. }
            | Event::BlockStop { .. } => Vec::new(),
        }
    }

    pub fn finish(&mut self) -> Vec<Bytes> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut frames = Vec::new();
        if let Some(usage) = self.usage.take() {
            let chunk = Chunk {
                id: self.id.clone(),
                object: CHUNK_OBJECT.to_owned(),
                created: 0,
                model: self.model.clone(),
                choices: Vec::new(),
                usage: Some(usage_to_wire(usage)),
                error: None,
            };
            frames.extend(json_frame(&chunk));
        }
        frames.push(sse::frame(None, DONE));
        frames
    }

    fn tool_delta(&self, index: u32, id: Option<String>, function: DeltaFunction) -> Vec<Bytes> {
        let kind = id.is_some().then_some(CallType::Function);
        self.delta(
            Delta {
                tool_calls: vec![DeltaToolCall {
                    index,
                    id,
                    kind,
                    function: Some(function),
                }],
                ..Delta::default()
            },
            None,
        )
    }

    fn delta(&self, delta: Delta, finish_reason: Option<&str>) -> Vec<Bytes> {
        let chunk = Chunk {
            id: self.id.clone(),
            object: CHUNK_OBJECT.to_owned(),
            created: 0,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta,
                finish_reason: finish_reason.map(str::to_owned),
                usage: None,
            }],
            usage: None,
            error: None,
        };
        json_frame(&chunk).into_iter().collect()
    }
}

const CHUNK_OBJECT: &str = "chat.completion.chunk";

fn json_frame(value: &impl Serialize) -> Option<Bytes> {
    serde_json::to_string(value)
        .ok()
        .map(|json| sse::frame(None, &json))
}
