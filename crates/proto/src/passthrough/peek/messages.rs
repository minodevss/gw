use serde::Deserialize;
use serde::de::IgnoredAny;

use super::Peek;
use super::flexible::{Flexible, items};
use crate::codec::DecodeError;
use crate::features::{Feature, Features, ThinkingKind};
use crate::ir::Effort;

#[derive(Deserialize)]
struct Request {
    model: String,
    #[serde(default)]
    stream: bool,
    max_tokens: Option<IgnoredAny>,
    temperature: Option<IgnoredAny>,
    top_p: Option<IgnoredAny>,
    stop_sequences: Option<Vec<IgnoredAny>>,
    thinking: Option<Thinking>,
    output_config: Option<OutputConfig>,
    speed: Option<String>,
    cache_control: Option<CacheControl>,
    system: Option<Flexible<Block>>,
    #[serde(default)]
    tools: Vec<Tool>,
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Thinking {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct OutputConfig {
    effort: Option<String>,
    format: Option<IgnoredAny>,
}

#[derive(Deserialize)]
struct CacheControl {
    ttl: Option<String>,
}

#[derive(Deserialize)]
struct Tool {
    strict: Option<bool>,
    cache_control: Option<CacheControl>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: Option<Flexible<Block>>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    content: Option<Flexible<Block>>,
    cache_control: Option<CacheControl>,
}

#[derive(Default)]
struct Scan {
    features: Features,
    cache_ttl: Option<u64>,
}

impl Scan {
    fn control(&mut self, control: Option<&CacheControl>) {
        if let Some(ttl) = control
            .and_then(|control| control.ttl.as_deref())
            .and_then(super::ttl_seconds)
        {
            self.cache_ttl = Some(self.cache_ttl.map_or(ttl, |current| current.max(ttl)));
        }
    }

    fn blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block.kind.as_deref() {
                Some("image") => self.features.insert(Feature::ImageInput),
                Some("document") => self.features.insert(Feature::DocumentInput),
                _ => {}
            }
            self.control(block.cache_control.as_ref());
            self.blocks(items(block.content.as_ref()));
        }
    }
}

pub(super) fn peek(body: &[u8]) -> Result<Peek, DecodeError> {
    let request: Request = serde_json::from_slice(body)?;
    let mut scan = Scan::default();
    let features = &mut scan.features;
    if request.max_tokens.is_some() {
        features.insert(Feature::MaxOutputTokens);
    }
    if request.temperature.is_some() {
        features.insert(Feature::Temperature);
    }
    if request.top_p.is_some() {
        features.insert(Feature::TopP);
    }
    if request.stop_sequences.is_some_and(|stop| !stop.is_empty()) {
        features.insert(Feature::Stop);
    }
    if let Some(thinking) = &request.thinking {
        match thinking.kind.as_str() {
            "adaptive" => features.insert(Feature::Thinking(ThinkingKind::Adaptive)),
            "enabled" => features.insert(Feature::Thinking(ThinkingKind::Enabled)),
            "disabled" => features.insert(Feature::Thinking(ThinkingKind::Disabled)),
            _ => {}
        }
    }
    if let Some(config) = &request.output_config {
        if let Some(effort) = config.effort.as_deref().and_then(Effort::parse) {
            features.insert(Feature::Effort(effort));
        }
        if config.format.is_some() {
            features.insert(Feature::StructuredOutput);
        }
    }
    if request.speed.as_deref() == Some("fast") {
        features.insert(Feature::Fast);
    }
    if request.tools.iter().any(|tool| tool.strict == Some(true)) {
        features.insert(Feature::StrictTools);
    }
    if request
        .messages
        .iter()
        .any(|message| message.role == "system")
    {
        features.insert(Feature::MidConversationSystem);
    }
    scan.control(request.cache_control.as_ref());
    scan.blocks(items(request.system.as_ref()));
    for tool in &request.tools {
        scan.control(tool.cache_control.as_ref());
    }
    for message in &request.messages {
        scan.blocks(items(message.content.as_ref()));
    }
    Ok(Peek {
        model: request.model,
        stream: request.stream,
        cache_key: None,
        previous_response: false,
        cache_ttl: scan.cache_ttl,
        features: scan.features,
    })
}
