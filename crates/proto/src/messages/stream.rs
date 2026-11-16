use std::collections::BTreeMap;

use bytes::Bytes;

use super::decode::{merge_usage, stop_reason};
use super::encode::{stop_reason_name, usage_out};
use super::types::{
    BlockStartIn, BlockStartOut, DeltaIn, DeltaOut, EmptyObject, ErrorIn, MessageDeltaOut,
    ResponseOut, StreamEventIn, StreamEventOut, UsageOut,
};
use crate::api::Api;
use crate::codec::DecodeError;
use crate::envelope;
use crate::error::{ErrorBody, ErrorKind};
use crate::ir::{Block, Event, Origin, StopReason, ToolKind, Usage};
use crate::sse;

#[derive(Debug, Default)]
pub struct StreamDecoder {
    usage: Usage,
    signatures: BTreeMap<u32, String>,
}

impl StreamDecoder {
    pub fn push(&mut self, frame: &sse::Frame) -> Result<Vec<Event>, DecodeError> {
        let event: StreamEventIn = serde_json::from_str(&frame.data)?;
        let mut events = Vec::new();
        match event {
            StreamEventIn::MessageStart { message } => {
                events.push(Event::Start {
                    id: message.id,
                    model: message.model,
                });
                if let Some(usage) = message.usage {
                    merge_usage(&mut self.usage, &usage);
                    events.push(Event::Usage(self.usage));
                }
            }
            StreamEventIn::ContentBlockStart {
                index,
                content_block,
            } => self.block_start(index, content_block, &mut events)?,
            StreamEventIn::ContentBlockDelta { index, delta } => {
                self.block_delta(index, delta, &mut events)?;
            }
            StreamEventIn::ContentBlockStop { index } => {
                if let Some(signature) = self.signatures.remove(&index) {
                    events.push(Event::ReasoningOrigin {
                        index,
                        origin: signature_origin(signature),
                    });
                }
                events.push(Event::BlockStop { index });
            }
            StreamEventIn::MessageDelta { delta, usage } => {
                if let Some(usage) = usage {
                    merge_usage(&mut self.usage, &usage);
                    events.push(Event::Usage(self.usage));
                }
                if let Some(reason) = delta.stop_reason {
                    events.push(Event::Stop(stop_reason(&reason)?));
                }
            }
            StreamEventIn::Error { error } => events.push(Event::Error(error_body(&error))),
            StreamEventIn::MessageStop | StreamEventIn::Ping | StreamEventIn::Unknown => {}
        }
        Ok(events)
    }

    fn block_start(
        &mut self,
        index: u32,
        block: BlockStartIn,
        events: &mut Vec<Event>,
    ) -> Result<(), DecodeError> {
        match block {
            BlockStartIn::Text { text } => {
                events.push(Event::BlockStart {
                    index,
                    block: Block::Text,
                });
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    events.push(Event::TextDelta { index, text });
                }
            }
            BlockStartIn::Thinking {
                thinking,
                signature,
            } => {
                events.push(Event::BlockStart {
                    index,
                    block: Block::Reasoning,
                });
                if let Some(text) = thinking.filter(|text| !text.is_empty()) {
                    events.push(Event::ReasoningDelta { index, text });
                }
                if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
                    self.signatures.insert(index, signature);
                }
            }
            BlockStartIn::RedactedThinking { data } => {
                events.push(Event::BlockStart {
                    index,
                    block: Block::Reasoning,
                });
                events.push(Event::ReasoningOrigin {
                    index,
                    origin: envelope::open(&data).unwrap_or(Origin::AnthropicRedacted { data }),
                });
            }
            BlockStartIn::ToolUse { id, name, input } => {
                events.push(Event::BlockStart {
                    index,
                    block: Block::ToolCall {
                        id,
                        name,
                        kind: ToolKind::Function,
                    },
                });
                if let Some(input) =
                    input.filter(|input| input.as_object().is_none_or(|map| !map.is_empty()))
                {
                    events.push(Event::ToolArgumentsDelta {
                        index,
                        json: input.to_string(),
                    });
                }
            }
            BlockStartIn::Unknown => {
                return Err(DecodeError::Event("unsupported content block".to_owned()));
            }
        }
        Ok(())
    }

    fn block_delta(
        &mut self,
        index: u32,
        delta: DeltaIn,
        events: &mut Vec<Event>,
    ) -> Result<(), DecodeError> {
        match delta {
            DeltaIn::Text { text } => events.push(Event::TextDelta { index, text }),
            DeltaIn::Thinking { thinking } => events.push(Event::ReasoningDelta {
                index,
                text: thinking,
            }),
            DeltaIn::Signature { signature } => {
                self.signatures
                    .entry(index)
                    .or_default()
                    .push_str(&signature);
            }
            DeltaIn::InputJson { partial_json } => events.push(Event::ToolArgumentsDelta {
                index,
                json: partial_json,
            }),
            DeltaIn::Citations {} => {}
            DeltaIn::Unknown => return Err(DecodeError::Event("unsupported delta".to_owned())),
        }
        Ok(())
    }
}

fn signature_origin(signature: String) -> Origin {
    envelope::open(&signature).unwrap_or(Origin::Anthropic { signature })
}

fn error_body(error: &ErrorIn) -> ErrorBody {
    let (status, kind) = match error.kind.as_str() {
        "invalid_request_error" => (400, ErrorKind::InvalidRequest),
        "authentication_error" => (401, ErrorKind::Authentication),
        "permission_error" => (403, ErrorKind::Permission),
        "not_found_error" => (404, ErrorKind::NotFound),
        "request_too_large" => (413, ErrorKind::InvalidRequest),
        "rate_limit_error" => (429, ErrorKind::RateLimit),
        "overloaded_error" => (529, ErrorKind::Overloaded),
        _ => (500, ErrorKind::Server),
    };
    ErrorBody::new(status, kind, error.message.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningSource {
    Text,
    Summary,
}

#[derive(Debug)]
enum OpenBlock {
    Text,
    Tool,
    Reasoning {
        opened: bool,
        redacted: bool,
        source: Option<ReasoningSource>,
        origin: Option<Origin>,
    },
}

#[derive(Debug)]
struct Open {
    ir_index: u32,
    out_index: u32,
    block: OpenBlock,
}

#[derive(Debug, Default)]
pub struct StreamEncoder {
    started: bool,
    finished: bool,
    next_index: u32,
    open: Option<Open>,
    stop: Option<StopReason>,
    usage: Usage,
}

impl StreamEncoder {
    pub fn push(&mut self, event: &Event) -> Vec<Bytes> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        match event {
            Event::Start { id, model } => self.start(id, model, &mut out),
            Event::BlockStart { index, block } => {
                self.start("", "", &mut out);
                self.close_open(&mut out);
                self.open_block(*index, block, &mut out);
            }
            Event::TextDelta { index, text } => {
                if let Some(open) = self.open_for(*index)
                    && matches!(open.block, OpenBlock::Text)
                {
                    let out_index = open.out_index;
                    emit(
                        &mut out,
                        &StreamEventOut::ContentBlockDelta {
                            index: out_index,
                            delta: DeltaOut::Text { text },
                        },
                    );
                }
            }
            Event::ReasoningDelta { index, text } => {
                self.reasoning_delta(*index, text, ReasoningSource::Text, &mut out);
            }
            Event::ReasoningSummaryDelta { index, text } => {
                self.reasoning_delta(*index, text, ReasoningSource::Summary, &mut out);
            }
            Event::ReasoningOrigin { index, origin } => {
                self.reasoning_origin(*index, origin, &mut out);
            }
            Event::ToolArgumentsDelta { index, json } => {
                if let Some(open) = self.open_for(*index)
                    && matches!(open.block, OpenBlock::Tool)
                {
                    let out_index = open.out_index;
                    emit(
                        &mut out,
                        &StreamEventOut::ContentBlockDelta {
                            index: out_index,
                            delta: DeltaOut::InputJson { partial_json: json },
                        },
                    );
                }
            }
            Event::BlockStop { index } => {
                if self.open_for(*index).is_some() {
                    self.close_open(&mut out);
                }
            }
            Event::Usage(usage) => self.usage = *usage,
            Event::Stop(reason) => {
                self.close_open(&mut out);
                self.stop = Some(*reason);
            }
            Event::Error(body) => {
                out.push(body.sse(Api::Messages));
                self.finished = true;
            }
        }
        out
    }

    pub fn finish(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        if self.finished || !self.started {
            return out;
        }
        let Some(stop) = self.stop else {
            return out;
        };
        self.finished = true;
        emit(
            &mut out,
            &StreamEventOut::MessageDelta {
                delta: MessageDeltaOut {
                    stop_reason: stop_reason_name(stop),
                    stop_sequence: None,
                },
                usage: usage_out(&self.usage),
            },
        );
        emit(&mut out, &StreamEventOut::MessageStop);
        out
    }

    fn start(&mut self, id: &str, model: &str, out: &mut Vec<Bytes>) {
        if self.started {
            return;
        }
        self.started = true;
        emit(
            out,
            &StreamEventOut::MessageStart {
                message: ResponseOut {
                    id,
                    kind: "message",
                    role: "assistant",
                    model,
                    content: Vec::new(),
                    stop_reason: None,
                    stop_sequence: None,
                    usage: UsageOut {
                        input: 0,
                        cache_write: 0,
                        cache_read: 0,
                        output: 0,
                    },
                },
            },
        );
    }

    fn open_for(&self, ir_index: u32) -> Option<&Open> {
        self.open.as_ref().filter(|open| open.ir_index == ir_index)
    }

    fn open_block(&mut self, ir_index: u32, block: &Block, out: &mut Vec<Bytes>) {
        let out_index = self.next_index;
        self.next_index += 1;
        let open = match block {
            Block::Text => {
                emit(
                    out,
                    &StreamEventOut::ContentBlockStart {
                        index: out_index,
                        content_block: BlockStartOut::Text { text: "" },
                    },
                );
                OpenBlock::Text
            }
            Block::ToolCall { id, name, .. } => {
                emit(
                    out,
                    &StreamEventOut::ContentBlockStart {
                        index: out_index,
                        content_block: BlockStartOut::ToolUse {
                            id,
                            name,
                            input: EmptyObject {},
                        },
                    },
                );
                OpenBlock::Tool
            }
            Block::Reasoning => OpenBlock::Reasoning {
                opened: false,
                redacted: false,
                source: None,
                origin: None,
            },
        };
        self.open = Some(Open {
            ir_index,
            out_index,
            block: open,
        });
    }

    fn reasoning_delta(
        &mut self,
        ir_index: u32,
        text: &str,
        kind: ReasoningSource,
        out: &mut Vec<Bytes>,
    ) {
        let Some(open) = self.open.as_mut().filter(|open| open.ir_index == ir_index) else {
            return;
        };
        let out_index = open.out_index;
        let OpenBlock::Reasoning {
            opened,
            redacted,
            source,
            ..
        } = &mut open.block
        else {
            return;
        };
        if *redacted || text.is_empty() || *source.get_or_insert(kind) != kind {
            return;
        }
        open_thinking(out_index, opened, out);
        emit(
            out,
            &StreamEventOut::ContentBlockDelta {
                index: out_index,
                delta: DeltaOut::Thinking { thinking: text },
            },
        );
    }

    fn reasoning_origin(&mut self, ir_index: u32, new_origin: &Origin, out: &mut Vec<Bytes>) {
        let Some(open) = self.open.as_mut().filter(|open| open.ir_index == ir_index) else {
            return;
        };
        let out_index = open.out_index;
        let OpenBlock::Reasoning {
            opened,
            redacted,
            origin,
            ..
        } = &mut open.block
        else {
            return;
        };
        if let Origin::AnthropicRedacted { data } = new_origin
            && !*opened
        {
            emit(
                out,
                &StreamEventOut::ContentBlockStart {
                    index: out_index,
                    content_block: BlockStartOut::RedactedThinking { data },
                },
            );
            *opened = true;
            *redacted = true;
            return;
        }
        *origin = Some(new_origin.clone());
    }

    fn close_open(&mut self, out: &mut Vec<Bytes>) {
        let Some(open) = self.open.take() else {
            return;
        };
        let out_index = open.out_index;
        if let OpenBlock::Reasoning {
            mut opened,
            redacted: false,
            origin,
            ..
        } = open.block
        {
            open_thinking(out_index, &mut opened, out);
            let signature = match origin.unwrap_or(Origin::Plain) {
                Origin::Anthropic { signature } => signature,
                other @ (Origin::AnthropicRedacted { .. }
                | Origin::OpenAi { .. }
                | Origin::Plain) => envelope::seal(&other),
            };
            emit(
                out,
                &StreamEventOut::ContentBlockDelta {
                    index: out_index,
                    delta: DeltaOut::Signature {
                        signature: &signature,
                    },
                },
            );
        }
        emit(out, &StreamEventOut::ContentBlockStop { index: out_index });
    }
}

fn open_thinking(index: u32, opened: &mut bool, out: &mut Vec<Bytes>) {
    if *opened {
        return;
    }
    *opened = true;
    emit(
        out,
        &StreamEventOut::ContentBlockStart {
            index,
            content_block: BlockStartOut::Thinking {
                thinking: "",
                signature: "",
            },
        },
    );
}

fn emit(out: &mut Vec<Bytes>, event: &StreamEventOut<'_>) {
    let data = serde_json::to_string(event).unwrap_or_default();
    out.push(sse::frame(Some(event.name()), &data));
}

pub fn ping() -> Bytes {
    let mut out = Vec::new();
    emit(&mut out, &StreamEventOut::Ping);
    out.into_iter().next().unwrap_or_default()
}
