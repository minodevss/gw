use serde::Deserialize;
use serde::de::IgnoredAny;

use super::Peek;
use super::flexible::{Flexible, items};
use crate::codec::DecodeError;
use crate::features::{Feature, Features};
use crate::ir::Effort;

#[derive(Deserialize)]
struct Request {
    model: String,
    #[serde(default)]
    stream: bool,
    prompt_cache_key: Option<String>,
    max_tokens: Option<IgnoredAny>,
    max_completion_tokens: Option<IgnoredAny>,
    temperature: Option<IgnoredAny>,
    top_p: Option<IgnoredAny>,
    stop: Option<Flexible<IgnoredAny>>,
    user: Option<IgnoredAny>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    response_format: Option<ResponseFormat>,
    #[serde(default)]
    tools: Vec<Tool>,
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: Option<String>,
    function: Option<Function>,
}

#[derive(Deserialize)]
struct Function {
    strict: Option<bool>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: Option<Flexible<Part>>,
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: Option<String>,
}

pub(super) fn peek(body: &[u8]) -> Result<Peek, DecodeError> {
    let request: Request = serde_json::from_slice(body)?;
    let mut features = Features::default();
    if request.max_tokens.is_some() || request.max_completion_tokens.is_some() {
        features.insert(Feature::MaxOutputTokens);
    }
    if request.temperature.is_some() {
        features.insert(Feature::Temperature);
    }
    if request.top_p.is_some() {
        features.insert(Feature::TopP);
    }
    if request.stop.as_ref().is_some_and(Flexible::is_present) {
        features.insert(Feature::Stop);
    }
    if request.user.is_some() {
        features.insert(Feature::User);
    }
    if let Some(effort) = request.reasoning_effort.as_deref().and_then(Effort::parse) {
        features.insert(Feature::Effort(effort));
    }
    if request.service_tier.as_deref() == Some("priority") {
        features.insert(Feature::Fast);
    }
    let format = request.response_format.as_ref();
    if format.is_some_and(|format| format.kind.as_deref() == Some("json_schema")) {
        features.insert(Feature::StructuredOutput);
    }
    for tool in &request.tools {
        let strict = tool.function.as_ref().and_then(|function| function.strict);
        match tool.kind.as_deref() {
            Some("function") if strict == Some(true) => features.insert(Feature::StrictTools),
            Some("custom") => features.insert(Feature::CustomTools),
            _ => {}
        }
    }
    let mut leading = true;
    for message in &request.messages {
        if matches!(message.role.as_str(), "system" | "developer") {
            if !leading {
                features.insert(Feature::MidConversationSystem);
            }
        } else {
            leading = false;
        }
        for part in items(message.content.as_ref()) {
            match part.kind.as_deref() {
                Some("image_url") => features.insert(Feature::ImageInput),
                Some("file") => features.insert(Feature::DocumentInput),
                _ => {}
            }
        }
    }
    Ok(Peek {
        model: request.model,
        stream: request.stream,
        cache_key: request.prompt_cache_key,
        previous_response: false,
        cache_ttl: None,
        features,
    })
}
