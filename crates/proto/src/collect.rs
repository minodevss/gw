use crate::codec::DecodeError;
use crate::ir::{
    Block, Event, Item, Origin, Part, Reasoning, Response, Role, StopReason, ToolCall, ToolKind,
    Usage,
};

#[derive(Debug, Default)]
pub struct Collector {
    start: Option<Start>,
    blocks: Vec<Slot>,
    usage: Usage,
    stop: Option<StopReason>,
    failure: Option<DecodeError>,
}

#[derive(Debug)]
struct Start {
    id: String,
    model: String,
}

#[derive(Debug)]
struct Slot {
    index: u32,
    content: Content,
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
        id: String,
        name: String,
        kind: ToolKind,
        arguments: String,
    },
}

impl Collector {
    pub fn push(&mut self, event: &Event) {
        if self.failure.is_some() {
            return;
        }
        let outcome = match event {
            Event::Start { id, model } => {
                self.start = Some(Start {
                    id: id.clone(),
                    model: model.clone(),
                });
                Ok(())
            }
            Event::BlockStart { index, block } => self.open(*index, block),
            Event::TextDelta { index, text } => self.edit(*index, |content| match content {
                Content::Text(buffer) => {
                    buffer.push_str(text);
                    true
                }
                Content::Reasoning { .. } | Content::Tool { .. } => false,
            }),
            Event::ReasoningDelta { index, text } => self.edit(*index, |content| match content {
                Content::Reasoning { text: buffer, .. } => {
                    buffer.push_str(text);
                    true
                }
                Content::Text(_) | Content::Tool { .. } => false,
            }),
            Event::ReasoningSummaryDelta { index, text } => {
                self.edit(*index, |content| match content {
                    Content::Reasoning {
                        summary: buffer, ..
                    } => {
                        buffer.push_str(text);
                        true
                    }
                    Content::Text(_) | Content::Tool { .. } => false,
                })
            }
            Event::ReasoningOrigin { index, origin } => {
                self.edit(*index, |content| match content {
                    Content::Reasoning { origin: slot, .. } => {
                        *slot = Some(origin.clone());
                        true
                    }
                    Content::Text(_) | Content::Tool { .. } => false,
                })
            }
            Event::ToolArgumentsDelta { index, json } => {
                self.edit(*index, |content| match content {
                    Content::Tool { arguments, .. } => {
                        arguments.push_str(json);
                        true
                    }
                    Content::Text(_) | Content::Reasoning { .. } => false,
                })
            }
            Event::BlockStop { .. } => Ok(()),
            Event::Usage(usage) => {
                self.usage = *usage;
                Ok(())
            }
            Event::Stop(reason) => {
                self.stop = Some(*reason);
                Ok(())
            }
            Event::Error(error) => Err(format!(
                "upstream error {}: {}",
                error.status, error.message
            )),
        };
        if let Err(message) = outcome {
            self.failure = Some(DecodeError::Event(message));
        }
    }

    pub fn finish(self) -> Result<Response, DecodeError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let start = self
            .start
            .ok_or_else(|| DecodeError::Event("stream ended without a start event".to_owned()))?;
        let stop = self
            .stop
            .ok_or_else(|| DecodeError::Event("stream ended without a stop reason".to_owned()))?;
        let mut output = Vec::new();
        for slot in self.blocks {
            match slot.content {
                Content::Text(text) => match output.last_mut() {
                    Some(Item::Message {
                        role: Role::Assistant,
                        parts,
                    }) if matches!(parts.last(), Some(Part::Text(_))) => {
                        parts.push(Part::Text(text));
                    }
                    _ => output.push(Item::Message {
                        role: Role::Assistant,
                        parts: vec![Part::Text(text)],
                    }),
                },
                Content::Reasoning {
                    text,
                    summary,
                    origin,
                } => output.push(Item::Reasoning(Reasoning {
                    text,
                    summary: if summary.is_empty() {
                        Vec::new()
                    } else {
                        vec![summary]
                    },
                    origin: origin.unwrap_or(Origin::Plain),
                })),
                Content::Tool {
                    id,
                    name,
                    kind,
                    arguments,
                } => output.push(Item::ToolCall(ToolCall {
                    id,
                    name,
                    kind,
                    arguments: if arguments.is_empty() {
                        "{}".to_owned()
                    } else {
                        arguments
                    },
                })),
            }
        }
        Ok(Response {
            id: start.id,
            model: start.model,
            output,
            stop,
            usage: self.usage,
        })
    }

    fn open(&mut self, index: u32, block: &Block) -> Result<(), String> {
        if self.blocks.iter().any(|slot| slot.index == index) {
            return Err(format!("block {index} started twice"));
        }
        let content = match block {
            Block::Text => Content::Text(String::new()),
            Block::Reasoning => Content::Reasoning {
                text: String::new(),
                summary: String::new(),
                origin: None,
            },
            Block::ToolCall { id, name, kind } => Content::Tool {
                id: id.clone(),
                name: name.clone(),
                kind: *kind,
                arguments: String::new(),
            },
        };
        self.blocks.push(Slot { index, content });
        Ok(())
    }

    fn edit(&mut self, index: u32, apply: impl FnOnce(&mut Content) -> bool) -> Result<(), String> {
        let slot = self
            .blocks
            .iter_mut()
            .find(|slot| slot.index == index)
            .ok_or_else(|| format!("delta for unknown block {index}"))?;
        if apply(&mut slot.content) {
            Ok(())
        } else {
            Err(format!("delta does not match the kind of block {index}"))
        }
    }
}
