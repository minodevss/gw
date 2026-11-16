use bytes::Bytes;
use proto::reject::Rejection;
use proto::{Api, DecodeError, EncodeOptions, EncodeResult, chat, ir, messages, responses, sse};

pub(crate) fn decode_request(api: Api, body: &[u8]) -> Result<ir::Request, Rejection> {
    match api {
        Api::Messages => messages::decode_request(body),
        Api::Responses => responses::decode_request(body),
        Api::Chat => chat::decode_request(body),
    }
}

pub(crate) fn encode_request(
    api: Api,
    request: &ir::Request,
    options: &EncodeOptions,
) -> EncodeResult {
    match api {
        Api::Messages => messages::encode_request(request, options),
        Api::Responses => responses::encode_request(request, options),
        Api::Chat => chat::encode_request(request, options),
    }
}

pub(crate) fn decode_response(api: Api, body: &[u8]) -> Result<ir::Response, DecodeError> {
    match api {
        Api::Messages => messages::decode_response(body),
        Api::Responses => responses::decode_response(body),
        Api::Chat => chat::decode_response(body),
    }
}

pub(crate) fn encode_response(api: Api, response: &ir::Response) -> Bytes {
    match api {
        Api::Messages => messages::encode_response(response),
        Api::Responses => responses::encode_response(response),
        Api::Chat => chat::encode_response(response),
    }
}

#[derive(Debug)]
pub(crate) enum StreamDecoder {
    Messages(messages::StreamDecoder),
    Responses(responses::StreamDecoder),
    Chat(chat::StreamDecoder),
}

impl StreamDecoder {
    pub(crate) fn new(api: Api) -> Self {
        match api {
            Api::Messages => Self::Messages(messages::StreamDecoder::default()),
            Api::Responses => Self::Responses(responses::StreamDecoder::default()),
            Api::Chat => Self::Chat(chat::StreamDecoder::default()),
        }
    }

    pub(crate) fn push(&mut self, frame: &sse::Frame) -> Result<Vec<ir::Event>, DecodeError> {
        match self {
            Self::Messages(decoder) => decoder.push(frame),
            Self::Responses(decoder) => decoder.push(frame),
            Self::Chat(decoder) => decoder.push(frame),
        }
    }
}

#[derive(Debug)]
pub(crate) enum StreamEncoder {
    Messages(messages::StreamEncoder),
    Responses(responses::StreamEncoder),
    Chat(chat::StreamEncoder),
}

impl StreamEncoder {
    pub(crate) fn new(api: Api) -> Self {
        match api {
            Api::Messages => Self::Messages(messages::StreamEncoder::default()),
            Api::Responses => Self::Responses(responses::StreamEncoder::default()),
            Api::Chat => Self::Chat(chat::StreamEncoder::default()),
        }
    }

    pub(crate) fn push(&mut self, event: &ir::Event) -> Vec<Bytes> {
        match self {
            Self::Messages(encoder) => encoder.push(event),
            Self::Responses(encoder) => encoder.push(event),
            Self::Chat(encoder) => encoder.push(event),
        }
    }

    pub(crate) fn finish(&mut self) -> Vec<Bytes> {
        match self {
            Self::Messages(encoder) => encoder.finish(),
            Self::Responses(encoder) => encoder.finish(),
            Self::Chat(encoder) => encoder.finish(),
        }
    }

    pub(crate) fn ping(&self) -> Option<Bytes> {
        match self {
            Self::Messages(_) => Some(messages::ping()),
            Self::Responses(_) | Self::Chat(_) => None,
        }
    }
}
