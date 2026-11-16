mod chat;
mod flexible;
mod messages;
mod responses;

use crate::api::Api;
use crate::codec::DecodeError;
use crate::features::Features;

#[derive(Debug, Clone, PartialEq)]
pub struct Peek {
    pub model: String,
    pub stream: bool,
    pub cache_key: Option<String>,
    pub previous_response: bool,
    pub cache_ttl: Option<u64>,
    pub features: Features,
}

pub fn peek(api: Api, body: &[u8]) -> Result<Peek, DecodeError> {
    match api {
        Api::Messages => messages::peek(body),
        Api::Responses => responses::peek(body),
        Api::Chat => chat::peek(body),
    }
}

pub(super) fn ttl_seconds(value: &str) -> Option<u64> {
    let split = value.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = value.split_at(split);
    let number: u64 = number.parse().ok()?;
    match unit {
        "s" => Some(number),
        "m" => number.checked_mul(60),
        "h" => number.checked_mul(3600),
        _ => None,
    }
}
