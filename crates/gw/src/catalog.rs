mod overrides;
pub(crate) mod view;

use std::collections::BTreeMap;

use proto::features::{Feature, ThinkingKind};
use proto::reject::Rejection;
use serde::{Deserialize, Serialize};

pub(crate) use overrides::Overrides;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Capabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_output: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) thinking: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fast: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) document: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) structured_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unsupported: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) overridden: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) instructions: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Support {
    Yes,
    No,
    Unknown,
}

impl Capabilities {
    pub(crate) fn support(&self, feature: Feature) -> Support {
        match feature {
            Feature::Effort(effort) => listed(self.efforts.as_deref(), effort.as_str()),
            Feature::Thinking(kind) => listed(self.thinking.as_deref(), kind.as_str()),
            Feature::Fast => flag(self.fast),
            Feature::ImageInput => flag(self.image),
            Feature::DocumentInput => flag(self.document),
            Feature::StructuredOutput => flag(self.structured_output),
            Feature::StrictTools
            | Feature::CustomTools
            | Feature::MidConversationSystem
            | Feature::Temperature
            | Feature::TopP
            | Feature::MaxOutputTokens
            | Feature::Stop
            | Feature::User => {
                if self
                    .unsupported
                    .iter()
                    .any(|name| name == feature_name(feature))
                {
                    Support::No
                } else {
                    Support::Unknown
                }
            }
        }
    }
}

pub(crate) fn feature_name(feature: Feature) -> &'static str {
    match feature {
        Feature::Effort(_) => "effort",
        Feature::Thinking(_) => "thinking",
        Feature::Fast => "fast",
        Feature::ImageInput => "image",
        Feature::DocumentInput => "document",
        Feature::StructuredOutput => "structured_output",
        Feature::StrictTools => "strict_tools",
        Feature::CustomTools => "custom_tools",
        Feature::MidConversationSystem => "mid_conversation_system",
        Feature::Temperature => "temperature",
        Feature::TopP => "top_p",
        Feature::MaxOutputTokens => "max_output_tokens",
        Feature::Stop => "stop",
        Feature::User => "user",
    }
}

pub(crate) fn rejection(feature: Feature) -> Rejection {
    match feature {
        Feature::Effort(_) => Rejection::Effort,
        Feature::Thinking(kind @ (ThinkingKind::Adaptive | ThinkingKind::Enabled)) => {
            Rejection::ThinkingType(kind)
        }
        Feature::ImageInput => Rejection::ImageBlock,
        Feature::DocumentInput => Rejection::DocumentBlock,
        Feature::StructuredOutput | Feature::StrictTools => Rejection::StructuredOutputs,
        Feature::MidConversationSystem => Rejection::MidConvSystem,
        Feature::Thinking(ThinkingKind::Disabled)
        | Feature::Fast
        | Feature::CustomTools
        | Feature::Temperature
        | Feature::TopP
        | Feature::MaxOutputTokens
        | Feature::Stop
        | Feature::User => Rejection::unsupported(feature_name(feature)),
    }
}

fn listed(values: Option<&[String]>, value: &str) -> Support {
    match values {
        Some(values) if values.iter().any(|v| v == value) => Support::Yes,
        Some(_) => Support::No,
        None => Support::Unknown,
    }
}

fn flag(value: Option<bool>) -> Support {
    match value {
        Some(true) => Support::Yes,
        Some(false) => Support::No,
        None => Support::Unknown,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Catalog {
    entries: BTreeMap<(String, String), Capabilities>,
}

impl Catalog {
    pub(crate) fn insert(&mut self, account: &str, model: &str, capabilities: Capabilities) {
        self.entries
            .insert((account.to_owned(), model.to_owned()), capabilities);
    }

    pub(crate) fn get(&self, account: &str, model: &str) -> Option<&Capabilities> {
        self.entries.get(&(account.to_owned(), model.to_owned()))
    }

    pub(crate) fn models(&self) -> impl Iterator<Item = (&str, &str, &Capabilities)> {
        self.entries
            .iter()
            .map(|((account, model), caps)| (account.as_str(), model.as_str(), caps))
    }
}

#[cfg(test)]
mod tests {
    use proto::ir::Effort;

    use super::*;

    #[test]
    fn unknown_lists_are_unknown_not_unsupported() {
        let caps = Capabilities::default();
        assert_eq!(
            caps.support(Feature::Effort(Effort::Xhigh)),
            Support::Unknown
        );
    }

    #[test]
    fn explicit_lists_decide() {
        let caps = Capabilities {
            efforts: Some(vec!["low".to_owned(), "high".to_owned()]),
            unsupported: vec!["temperature".to_owned()],
            ..Capabilities::default()
        };
        assert_eq!(caps.support(Feature::Effort(Effort::High)), Support::Yes);
        assert_eq!(caps.support(Feature::Effort(Effort::Xhigh)), Support::No);
        assert_eq!(caps.support(Feature::Temperature), Support::No);
        assert_eq!(caps.support(Feature::TopP), Support::Unknown);
    }
}
