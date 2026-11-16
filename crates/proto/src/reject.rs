use crate::api::Api;
use crate::features::ThinkingKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("mid-conversation system message")]
    MidConvSystem,
    #[error("cache_control field")]
    CacheControlField,
    #[error("reasoning from a different upstream")]
    ThinkingSignature,
    #[error("thinking type {}", .0.as_str())]
    ThinkingType(ThinkingKind),
    #[error("structured outputs")]
    StructuredOutputs,
    #[error("effort")]
    Effort,
    #[error("image input")]
    ImageBlock,
    #[error("document input")]
    DocumentBlock,
    #[error("prompt is too long")]
    PromptTooLong,
    #[error("input length and max_tokens exceed context limit")]
    MaxTokensContextOverflow,
    #[error("beta feature {0}")]
    BetaHeader(String),
    #[error("{0}")]
    Unsupported(String),
}

impl Rejection {
    pub fn unsupported(field: impl Into<String>) -> Self {
        Self::Unsupported(field.into())
    }

    pub fn class(&self) -> Option<String> {
        Some(match self {
            Self::MidConvSystem => "mid_conv_system".to_owned(),
            Self::CacheControlField => "cache_control_field".to_owned(),
            Self::ThinkingSignature => "thinking_signature".to_owned(),
            Self::ThinkingType(kind) => format!("thinking_type:{}", kind.as_str()),
            Self::StructuredOutputs => "structured_outputs_unsupported".to_owned(),
            Self::Effort => "effort_unsupported".to_owned(),
            Self::ImageBlock => "image_block".to_owned(),
            Self::DocumentBlock => "document_block".to_owned(),
            Self::PromptTooLong => "prompt_too_long".to_owned(),
            Self::MaxTokensContextOverflow => "max_tokens_context_overflow".to_owned(),
            Self::BetaHeader(value) => format!("beta_header:{value}"),
            Self::Unsupported(_) => return None,
        })
    }

    pub fn message(&self, client: Api) -> String {
        match (client, self.class()) {
            (Api::Messages, Some(class)) => format!("capability_rejected: {class}"),
            (Api::Messages | Api::Responses | Api::Chat, _) => format!("unsupported: {self}"),
        }
    }
}
