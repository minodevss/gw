#![expect(
    clippy::expect_used,
    reason = "shared test helpers sit outside #[test] functions, where clippy.toml does not exempt expect"
)]

use std::fs;
use std::path::Path;

use insta::{assert_debug_snapshot, assert_snapshot};
use proto::chat::{
    StreamDecoder, StreamEncoder, decode_request, decode_response, encode_request, encode_response,
};
use proto::codec::{EncodeOptions, Note};
use proto::ir::{self, Event, Item, Part};
use proto::reject::Rejection;
use proto::sse;

const REQUEST: &str = r#"{
  "model": "gpt-4o",
  "messages": [
    {"role": "system", "content": "You are terse."},
    {"role": "developer", "content": [{"type": "text", "text": "Use metric units."}]},
    {"role": "user", "content": [
      {"type": "text", "text": "What is in this picture?"},
      {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA", "detail": "auto"}},
      {"type": "file", "file": {"file_data": "data:application/pdf;base64,BBBB", "filename": "a.pdf"}}
    ]},
    {"role": "assistant", "content": "Let me look.", "reasoning_content": "Check the image first.", "tool_calls": [
      {"id": "call_1", "type": "function", "function": {"name": "zoom", "arguments": "{\"x\":1}"}},
      {"id": "call_2", "type": "function", "function": {"name": "crop", "arguments": "{}"}}
    ]},
    {"role": "tool", "tool_call_id": "call_1", "content": "zoomed"},
    {"role": "tool", "tool_call_id": "call_2", "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]},
    {"role": "system", "content": "Late instruction."},
    {"role": "user", "content": "Thanks"}
  ],
  "tools": [
    {"type": "function", "function": {"name": "zoom", "description": "Zoom in", "parameters": {"type": "object",  "properties": {"x": {"type": "integer"}}}, "strict": true}},
    {"type": "function", "function": {"name": "crop"}}
  ],
  "tool_choice": {"type": "function", "function": {"name": "zoom"}},
  "parallel_tool_calls": false,
  "max_completion_tokens": 512,
  "temperature": 0.2,
  "top_p": 0.9,
  "stop": "END",
  "reasoning_effort": "high",
  "service_tier": "priority",
  "response_format": {"type": "json_schema", "json_schema": {"name": "answer", "schema": {"type": "object"}, "strict": true}},
  "prompt_cache_key": "session-1",
  "user": "u-1",
  "stream": true,
  "stream_options": {"include_usage": true}
}"#;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/upstream")
        .join(name);
    fs::read_to_string(path).expect("fixture exists")
}

fn frames(text: &str) -> Vec<sse::Frame> {
    sse::Parser::default().push(text.as_bytes())
}

fn decode_stream(frames: &[sse::Frame]) -> Vec<Event> {
    let mut decoder = StreamDecoder::default();
    frames
        .iter()
        .flat_map(|frame| decoder.push(frame).expect("frame decodes"))
        .collect()
}

fn encode_stream(events: &[Event]) -> String {
    let mut encoder = StreamEncoder::default();
    let mut out = Vec::new();
    for event in events {
        out.extend(encoder.push(event));
    }
    out.extend(encoder.finish());
    out.iter()
        .map(|bytes| String::from_utf8(bytes.to_vec()).expect("utf8"))
        .collect()
}

fn encoded_body(request: &ir::Request, options: &EncodeOptions) -> String {
    let encoded = encode_request(request, options).expect("encodes");
    String::from_utf8(encoded.body.to_vec()).expect("utf8")
}

fn user_request(items: Vec<Item>) -> ir::Request {
    ir::Request {
        items,
        ..decode_request(br#"{"model":"m","messages":[]}"#).expect("minimal request")
    }
}

fn reject(body: &str) -> Rejection {
    decode_request(body.as_bytes()).expect_err("rejected")
}

#[test]
fn decodes_full_request() {
    assert_debug_snapshot!(decode_request(REQUEST.as_bytes()).expect("decodes"));
}

#[test]
fn encodes_full_request() {
    let request = decode_request(REQUEST.as_bytes()).expect("decodes");
    let encoded = encode_request(&request, &EncodeOptions::default()).expect("encodes");
    assert_snapshot!(String::from_utf8(encoded.body.to_vec()).expect("utf8"));
    assert_eq!(encoded.notes, Vec::<Note>::new());
}

#[test]
fn request_round_trips_through_ir() {
    let request = decode_request(REQUEST.as_bytes()).expect("decodes");
    let again = decode_request(
        &encode_request(&request, &EncodeOptions::default())
            .unwrap()
            .body,
    )
    .expect("re-decodes");
    assert_eq!(request.items.len(), again.items.len());
    assert_eq!(request.tools, again.tools);
    assert_eq!(request.structured_output, again.structured_output);
    assert_eq!(request.system, again.system);
}

#[test]
fn encoding_is_deterministic() {
    let request = decode_request(REQUEST.as_bytes()).expect("decodes");
    let options = EncodeOptions::default();
    assert_eq!(
        encode_request(&request, &options).unwrap(),
        encode_request(&request, &options).unwrap()
    );
}

#[test]
fn cache_key_option_wins_and_stream_is_forced() {
    let mut request = decode_request(br#"{"model":"m","messages":[]}"#).expect("decodes");
    request.cache_key = Some("from-client".to_owned());
    let options = EncodeOptions {
        cache_key: Some("session".to_owned()),
        force_stream: true,
        ..EncodeOptions::default()
    };
    let encoded = encode_request(&request, &options).expect("encodes");
    assert_eq!(encoded.notes, vec![Note::StreamForced]);
    assert_snapshot!(String::from_utf8(encoded.body.to_vec()).expect("utf8"));
}

#[test]
fn encoded_prefix_is_stable_across_turns() {
    let turn = |text: &str| Item::Message {
        role: ir::Role::User,
        parts: vec![Part::Text(text.to_owned())],
    };
    let reply = |text: &str| Item::Message {
        role: ir::Role::Assistant,
        parts: vec![Part::Text(text.to_owned())],
    };
    let options = EncodeOptions {
        cache_key: Some("s".to_owned()),
        ..EncodeOptions::default()
    };
    let mut history = vec![turn("one")];
    let mut previous = encoded_body(&user_request(history.clone()), &options);
    for round in 0..3 {
        history.push(reply(&format!("reply {round}")));
        history.push(turn(&format!("turn {round}")));
        let next = encoded_body(&user_request(history.clone()), &options);
        let prefix = previous.strip_suffix("]}").expect("messages are last");
        assert!(next.starts_with(prefix), "{next} does not extend {prefix}");
        previous = next;
    }
}

#[test]
fn tool_error_flag_is_noted() {
    let request = user_request(vec![Item::ToolResult(ir::ToolResult {
        call_id: "c".to_owned(),
        kind: ir::ToolKind::Function,
        parts: vec![Part::Text("boom".to_owned())],
        is_error: true,
    })]);
    let encoded = encode_request(&request, &EncodeOptions::default()).expect("encodes");
    assert_eq!(encoded.notes, vec![Note::ToolErrorFlagDropped]);
}

#[test]
fn rejects_unsupported_request_fields() {
    let cases = [
        r#"{"model":"m","messages":[],"n":2}"#,
        r#"{"model":"m","messages":[],"seed":1}"#,
        r#"{"model":"m","messages":[],"frequency_penalty":0.5}"#,
        r#"{"model":"m","messages":[],"logprobs":true}"#,
        r#"{"model":"m","messages":[],"unknown_field":1}"#,
        r#"{"model":"m","messages":[],"tools":[{"type":"custom","custom":{"name":"x"}}]}"#,
        r#"{"model":"m","messages":[],"tool_choice":{"type":"allowed_tools"}}"#,
        r#"{"model":"m","messages":[],"service_tier":"flex"}"#,
        r#"{"model":"m","messages":[],"response_format":{"type":"json_object"}}"#,
        r#"{"model":"m","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"http://x","detail":"high"}}]}]}"#,
        r#"{"model":"m","messages":[{"role":"assistant","tool_calls":[{"id":"a","type":"custom","custom":{}}]}]}"#,
    ];
    let messages: Vec<String> = cases.iter().map(|case| reject(case).to_string()).collect();
    assert_debug_snapshot!(messages);
}

#[test]
fn rejects_what_chat_cannot_carry() {
    let image = Part::Image(ir::Media::Url("http://x".to_owned()));
    let tool_image = user_request(vec![Item::ToolResult(ir::ToolResult {
        call_id: "c".to_owned(),
        kind: ir::ToolKind::Function,
        parts: vec![image],
        is_error: false,
    })]);
    assert_eq!(
        encode_request(&tool_image, &EncodeOptions::default()),
        Err(Rejection::ImageBlock)
    );

    let signed = user_request(vec![Item::Reasoning(ir::Reasoning {
        text: "t".to_owned(),
        summary: Vec::new(),
        origin: ir::Origin::Anthropic {
            signature: "s".to_owned(),
        },
    })]);
    assert_eq!(
        encode_request(&signed, &EncodeOptions::default()),
        Err(Rejection::ThinkingSignature)
    );

    let mut unnamed = user_request(Vec::new());
    unnamed.structured_output = Some(ir::StructuredOutput {
        name: None,
        schema: proto::raw::RawJson::parse("{}".to_owned()).unwrap(),
        strict: None,
    });
    assert_eq!(
        encode_request(&unnamed, &EncodeOptions::default()),
        Err(Rejection::StructuredOutputs)
    );

    let mut effort = user_request(Vec::new());
    effort.effort = Some(ir::Effort::Max);
    assert_eq!(
        encode_request(&effort, &EncodeOptions::default()),
        Err(Rejection::Effort)
    );
}

const RESPONSE: &str = r#"{
  "id": "chatcmpl-1",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "kimi-k2",
  "choices": [{
    "index": 0,
    "message": {
      "role": "assistant",
      "content": "Checking.",
      "reasoning_content": "Need the weather.",
      "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]
    },
    "finish_reason": "tool_calls"
  }],
  "usage": {
    "prompt_tokens": 100,
    "completion_tokens": 30,
    "total_tokens": 130,
    "prompt_tokens_details": {"cached_tokens": 80},
    "completion_tokens_details": {"reasoning_tokens": 12}
  }
}"#;

#[test]
fn decodes_response_with_usage() {
    assert_debug_snapshot!(decode_response(RESPONSE.as_bytes()).expect("decodes"));
}

#[test]
fn encodes_response() {
    let response = decode_response(RESPONSE.as_bytes()).expect("decodes");
    let body = encode_response(&response);
    assert_snapshot!(String::from_utf8(body.to_vec()).expect("utf8"));
    assert_eq!(decode_response(&body).expect("re-decodes"), response);
}

#[test]
fn rejects_multiple_choices_in_response() {
    let body = r#"{"id":"a","model":"m","choices":[
      {"index":0,"message":{"role":"assistant","content":"a"},"finish_reason":"stop"},
      {"index":1,"message":{"role":"assistant","content":"b"},"finish_reason":"stop"}]}"#;
    assert!(decode_response(body.as_bytes()).is_err());
}

#[test]
fn decodes_text_stream() {
    assert_debug_snapshot!(decode_stream(&frames(&fixture("chat-text.sse"))));
}

#[test]
fn decodes_tool_call_stream() {
    assert_debug_snapshot!(decode_stream(&frames(&fixture("chat-tool_call.sse"))));
}

#[test]
fn decodes_usage_stream() {
    assert_debug_snapshot!(decode_stream(&frames(&fixture("chat-usage.sse"))));
}

#[test]
fn decodes_reasoning_stream_with_usage_in_choice() {
    assert_debug_snapshot!(decode_stream(&frames(&fixture("chat-reasoning.sse"))));
}

#[test]
fn encodes_tool_call_stream() {
    let events = decode_stream(&frames(&fixture("chat-tool_call.sse")));
    assert_snapshot!(encode_stream(&events));
}

#[test]
fn stream_round_trips_through_encoder() {
    for name in [
        "chat-text.sse",
        "chat-tool_call.sse",
        "chat-usage.sse",
        "chat-reasoning.sse",
    ] {
        let events = decode_stream(&frames(&fixture(name)));
        let wire = encode_stream(&events);
        assert!(wire.ends_with("data: [DONE]\n\n"), "{name}");
        assert_eq!(decode_stream(&frames(&wire)), events, "{name}");
    }
}

#[test]
fn stream_survives_arbitrary_byte_splits() {
    let text = fixture("chat-tool_call.sse");
    let whole = decode_stream(&frames(&text));
    let mut parser = sse::Parser::default();
    let mut decoder = StreamDecoder::default();
    let mut events = Vec::new();
    for chunk in text.as_bytes().chunks(7) {
        for frame in parser.push(chunk) {
            events.extend(decoder.push(&frame).expect("frame decodes"));
        }
    }
    assert_eq!(events, whole);
}

#[test]
fn error_chunk_becomes_error_event() {
    let frame = sse::Frame {
        event: None,
        data: r#"{"error":{"message":"slow down","type":"rate_limit_error"}}"#.to_owned(),
    };
    let events = StreamDecoder::default().push(&frame).expect("decodes");
    assert!(matches!(events.as_slice(), [Event::Error(body)] if body.status == 429));
}

#[test]
fn stream_encoder_stops_after_error() {
    let mut encoder = StreamEncoder::default();
    let body = proto::error::ErrorBody::new(500, proto::error::ErrorKind::Server, "x");
    assert_eq!(encoder.push(&Event::Error(body)).len(), 1);
    assert_eq!(encoder.finish(), [] as [bytes::Bytes; 0]);
}
