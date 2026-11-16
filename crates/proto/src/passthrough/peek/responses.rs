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
    previous_response_id: Option<IgnoredAny>,
    prompt_cache_options: Option<CacheOptions>,
    max_output_tokens: Option<IgnoredAny>,
    temperature: Option<IgnoredAny>,
    top_p: Option<IgnoredAny>,
    user: Option<IgnoredAny>,
    reasoning: Option<Reasoning>,
    service_tier: Option<String>,
    text: Option<Text>,
    #[serde(default)]
    tools: Vec<Tool>,
    input: Option<Flexible<Item>>,
}

#[derive(Deserialize)]
struct CacheOptions {
    ttl: Option<String>,
}

#[derive(Deserialize)]
struct Reasoning {
    effort: Option<String>,
}

#[derive(Deserialize)]
struct Text {
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Format {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: Option<String>,
    strict: Option<bool>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    content: Option<Flexible<Part>>,
    output: Option<Flexible<Part>>,
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl Item {
    fn is_system_message(&self) -> bool {
        let is_message = matches!(self.kind.as_deref(), None | Some("message"));
        is_message && matches!(self.role.as_deref(), Some("developer" | "system"))
    }
}

fn insert_parts(features: &mut Features, parts: &[Part]) {
    for part in parts {
        match part.kind.as_deref() {
            Some("input_image") => features.insert(Feature::ImageInput),
            Some("input_file") => features.insert(Feature::DocumentInput),
            _ => {}
        }
    }
}

pub(super) fn peek(body: &[u8]) -> Result<Peek, DecodeError> {
    let request: Request = serde_json::from_slice(body)?;
    let mut features = Features::default();
    if request.max_output_tokens.is_some() {
        features.insert(Feature::MaxOutputTokens);
    }
    if request.temperature.is_some() {
        features.insert(Feature::Temperature);
    }
    if request.top_p.is_some() {
        features.insert(Feature::TopP);
    }
    if request.user.is_some() {
        features.insert(Feature::User);
    }
    if let Some(effort) = request
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.effort.as_deref())
        .and_then(Effort::parse)
    {
        features.insert(Feature::Effort(effort));
    }
    if request.service_tier.as_deref() == Some("priority") {
        features.insert(Feature::Fast);
    }
    let format = request.text.as_ref().and_then(|text| text.format.as_ref());
    if format.is_some_and(|format| format.kind.as_deref() == Some("json_schema")) {
        features.insert(Feature::StructuredOutput);
    }
    for tool in &request.tools {
        match tool.kind.as_deref() {
            Some("function") if tool.strict == Some(true) => features.insert(Feature::StrictTools),
            Some("custom") => features.insert(Feature::CustomTools),
            _ => {}
        }
    }
    let mut leading = true;
    for item in items(request.input.as_ref()) {
        if item.is_system_message() {
            if !leading {
                features.insert(Feature::MidConversationSystem);
            }
        } else {
            leading = false;
        }
        insert_parts(&mut features, items(item.content.as_ref()));
        insert_parts(&mut features, items(item.output.as_ref()));
    }
    Ok(Peek {
        model: request.model,
        stream: request.stream,
        cache_key: request.prompt_cache_key,
        previous_response: request.previous_response_id.is_some(),
        cache_ttl: request
            .prompt_cache_options
            .and_then(|options| options.ttl)
            .as_deref()
            .and_then(super::ttl_seconds),
        features,
    })
}
