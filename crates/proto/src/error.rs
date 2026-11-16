use bytes::Bytes;
use serde_json::{Value, json};

use crate::api::Api;
use crate::reject::Rejection;
use crate::sse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorBody {
    pub status: u16,
    pub kind: ErrorKind,
    pub message: String,
    pub resets_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidRequest,
    Authentication,
    Permission,
    NotFound,
    RateLimit,
    UsageLimit,
    Overloaded,
    Server,
}

impl ErrorBody {
    pub fn new(status: u16, kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
            resets_at: None,
        }
    }

    pub fn rejected(rejection: &Rejection, client: Api) -> Self {
        Self::new(400, ErrorKind::InvalidRequest, rejection.message(client))
    }

    pub fn json(&self, api: Api) -> Bytes {
        Bytes::from(self.value(api).to_string())
    }

    pub fn sse(&self, api: Api) -> Bytes {
        match api {
            Api::Messages => sse::frame(Some("error"), &self.value(api).to_string()),
            Api::Responses => {
                let event = json!({
                    "type": "error",
                    "code": self.openai_code(),
                    "message": self.message,
                });
                sse::frame(Some("error"), &event.to_string())
            }
            Api::Chat => sse::frame(None, &self.value(api).to_string()),
        }
    }

    fn value(&self, api: Api) -> Value {
        match api {
            Api::Messages => json!({
                "type": "error",
                "error": { "type": self.anthropic_type(), "message": self.message },
            }),
            Api::Responses | Api::Chat => {
                let mut error = json!({
                    "message": self.message,
                    "type": self.openai_type(),
                    "code": self.openai_code(),
                });
                if let (Some(resets_at), Some(map)) = (self.resets_at, error.as_object_mut()) {
                    map.insert("resets_at".to_owned(), json!(resets_at));
                }
                json!({ "error": error })
            }
        }
    }

    fn anthropic_type(&self) -> &'static str {
        match self.kind {
            ErrorKind::InvalidRequest => "invalid_request_error",
            ErrorKind::Authentication => "authentication_error",
            ErrorKind::Permission => "permission_error",
            ErrorKind::NotFound => "not_found_error",
            ErrorKind::RateLimit | ErrorKind::UsageLimit => "rate_limit_error",
            ErrorKind::Overloaded => "overloaded_error",
            ErrorKind::Server => "api_error",
        }
    }

    fn openai_type(&self) -> &'static str {
        match self.kind {
            ErrorKind::InvalidRequest | ErrorKind::NotFound => "invalid_request_error",
            ErrorKind::Authentication => "authentication_error",
            ErrorKind::Permission => "permission_error",
            ErrorKind::RateLimit => "rate_limit_error",
            ErrorKind::UsageLimit => "usage_limit_reached",
            ErrorKind::Overloaded | ErrorKind::Server => "server_error",
        }
    }

    fn openai_code(&self) -> &'static str {
        match self.kind {
            ErrorKind::InvalidRequest => "invalid_request",
            ErrorKind::Authentication => "invalid_api_key",
            ErrorKind::Permission => "permission_denied",
            ErrorKind::NotFound => "model_not_found",
            ErrorKind::RateLimit => "rate_limit_exceeded",
            ErrorKind::UsageLimit => "usage_limit_reached",
            ErrorKind::Overloaded => "server_is_overloaded",
            ErrorKind::Server => "server_error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_error_has_anthropic_shape() {
        let body = ErrorBody::new(429, ErrorKind::UsageLimit, "limit");
        let value: Value = serde_json::from_slice(&body.json(Api::Messages)).unwrap();
        assert_eq!(value["type"], "error");
        assert_eq!(value["error"]["type"], "rate_limit_error");
    }

    #[test]
    fn responses_usage_limit_carries_resets_at() {
        let body = ErrorBody {
            resets_at: Some(1_700_000_000),
            ..ErrorBody::new(429, ErrorKind::UsageLimit, "limit")
        };
        let value: Value = serde_json::from_slice(&body.json(Api::Responses)).unwrap();
        assert_eq!(value["error"]["type"], "usage_limit_reached");
        assert_eq!(value["error"]["resets_at"], 1_700_000_000);
    }

    #[test]
    fn rejection_message_is_the_bare_class_token_for_messages() {
        let body = ErrorBody::rejected(&Rejection::Effort, Api::Messages);
        assert_eq!(body.message, "capability_rejected: effort_unsupported");
    }
}
