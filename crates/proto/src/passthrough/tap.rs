use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use crate::api::Api;
use crate::ir::Usage;
use crate::sse::{self, Frame};

#[derive(Debug)]
pub struct Tap {
    api: Api,
    mode: Mode,
    usage: Option<Usage>,
    error: Option<Value>,
}

#[derive(Debug)]
enum Mode {
    Stream(sse::Parser),
    Buffered(Vec<u8>),
}

impl Tap {
    pub fn new(api: Api, stream: bool) -> Self {
        let mode = if stream {
            Mode::Stream(sse::Parser::default())
        } else {
            Mode::Buffered(Vec::new())
        };
        Self {
            api,
            mode,
            usage: None,
            error: None,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        match &mut self.mode {
            Mode::Stream(parser) => {
                for frame in parser.push(chunk) {
                    self.observe(&frame);
                }
            }
            Mode::Buffered(body) => body.extend_from_slice(chunk),
        }
    }

    pub fn finish(&mut self) {
        match &mut self.mode {
            Mode::Stream(parser) => {
                for frame in parser.push(b"\n\n") {
                    self.observe(&frame);
                }
            }
            Mode::Buffered(body) => {
                let body = std::mem::take(body);
                self.observe_body(&body);
            }
        }
    }

    pub fn usage(&self) -> Option<Usage> {
        self.usage
    }

    pub fn error(&self) -> Option<Value> {
        self.error.clone()
    }

    fn observe(&mut self, frame: &Frame) {
        match self.api {
            Api::Messages => self.observe_messages(frame),
            Api::Responses => self.observe_responses(frame),
            Api::Chat => self.observe_chat(frame),
        }
    }

    fn observe_messages(&mut self, frame: &Frame) {
        let Ok(event) = serde_json::from_str::<MessagesEvent>(&frame.data) else {
            return;
        };
        let is_error =
            frame.event.as_deref() == Some("error") || event.kind.as_deref() == Some("error");
        if is_error {
            self.record_error(&frame.data);
            return;
        }
        let started = event.message.and_then(|message| message.usage);
        for usage in started.into_iter().chain(event.usage) {
            usage.apply(self.usage.get_or_insert_default());
        }
    }

    fn observe_responses(&mut self, frame: &Frame) {
        let Ok(event) = serde_json::from_str::<ResponsesEvent>(&frame.data) else {
            return;
        };
        let name = event.kind.as_deref().or(frame.event.as_deref());
        match name {
            Some("error" | "response.failed") => self.record_error(&frame.data),
            Some("response.completed" | "response.incomplete") => {
                if let Some(usage) = event.response.and_then(|response| response.usage) {
                    self.usage = Some(usage.normalize());
                }
            }
            _ => {}
        }
    }

    fn observe_chat(&mut self, frame: &Frame) {
        let Ok(chunk) = serde_json::from_str::<ChatChunk>(&frame.data) else {
            return;
        };
        if chunk.error.is_some() {
            self.record_error(&frame.data);
        }
        if let Some(usage) = chunk.usage {
            self.usage = Some(usage.normalize());
        }
    }

    fn observe_body(&mut self, body: &[u8]) {
        let Ok(text) = std::str::from_utf8(body) else {
            return;
        };
        match self.api {
            Api::Messages => {
                let Ok(parsed) = serde_json::from_str::<MessagesBody>(text) else {
                    return;
                };
                if let Some(usage) = parsed.usage {
                    usage.apply(self.usage.get_or_insert_default());
                }
                if parsed.error.is_some() {
                    self.record_error(text);
                }
            }
            Api::Responses | Api::Chat => {
                let Ok(parsed) = serde_json::from_str::<OpenAiBody>(text) else {
                    return;
                };
                if let Some(usage) = parsed.usage {
                    self.usage = Some(usage.normalize());
                }
                if parsed.error.is_some() {
                    self.record_error(text);
                }
            }
        }
    }

    fn record_error(&mut self, text: &str) {
        if let Ok(value) = serde_json::from_str(text) {
            self.error = Some(value);
        }
    }
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(rename = "input_tokens")]
    input: Option<u64>,
    #[serde(rename = "cache_read_input_tokens")]
    cache_read: Option<u64>,
    #[serde(rename = "cache_creation_input_tokens")]
    cache_write: Option<u64>,
    #[serde(rename = "output_tokens")]
    output: Option<u64>,
}

impl AnthropicUsage {
    fn apply(&self, usage: &mut Usage) {
        if let Some(value) = self.input {
            usage.input_uncached = value;
        }
        if let Some(value) = self.cache_read {
            usage.cache_read = value;
        }
        if let Some(value) = self.cache_write {
            usage.cache_write = value;
        }
        if let Some(value) = self.output {
            usage.output = value;
        }
    }
}

#[derive(Deserialize)]
struct MessagesEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    message: Option<MessagesMessage>,
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct MessagesMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct MessagesBody {
    usage: Option<AnthropicUsage>,
    error: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(alias = "prompt_tokens", default)]
    input_tokens: u64,
    #[serde(alias = "completion_tokens", default)]
    output_tokens: u64,
    #[serde(alias = "prompt_tokens_details")]
    input_tokens_details: Option<InputDetails>,
    #[serde(alias = "completion_tokens_details")]
    output_tokens_details: Option<OutputDetails>,
}

#[derive(Deserialize)]
struct InputDetails {
    cached_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct OutputDetails {
    reasoning_tokens: Option<u64>,
}

impl OpenAiUsage {
    fn normalize(&self) -> Usage {
        let cache_read = self
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .unwrap_or_default();
        let cache_write = self
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens)
            .unwrap_or_default();
        Usage {
            input_uncached: self
                .input_tokens
                .saturating_sub(cache_read)
                .saturating_sub(cache_write),
            cache_read,
            cache_write,
            output: self.output_tokens,
            reasoning: self
                .output_tokens_details
                .as_ref()
                .and_then(|details| details.reasoning_tokens),
        }
    }
}

#[derive(Deserialize)]
struct ResponsesEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    response: Option<ResponsesResponse>,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct ChatChunk {
    usage: Option<OpenAiUsage>,
    error: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct OpenAiBody {
    usage: Option<OpenAiUsage>,
    error: Option<IgnoredAny>,
}
