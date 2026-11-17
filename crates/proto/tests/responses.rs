#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test helpers sit outside #[test] functions, which clippy.toml exempts"
)]

use bytes::Bytes;
use proto::codec::{EncodeOptions, Note};
use proto::envelope;
use proto::error::{ErrorBody, ErrorKind};
use proto::features::ThinkingKind;
use proto::ir::{
    Block, Document, Effort, Event, Item, Media, Origin, Part, Reasoning, Request, Response, Role,
    Speed, StopReason, StructuredOutput, Thinking, Tool, ToolCall, ToolChoice, ToolKind,
    ToolResult, Usage,
};
use proto::raw::RawJson;
use proto::reject::Rejection;
use proto::responses::{
    StreamDecoder, StreamEncoder, decode_request, decode_response, encode_request, encode_response,
};
use proto::sse;
use serde_json::{Value, json};

fn fixture(path: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn without_hosted_tools(body: &[u8]) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(body).unwrap();
    value["tools"]
        .as_array_mut()
        .unwrap()
        .retain(|tool| matches!(tool["type"].as_str(), Some("function" | "custom")));
    serde_json::to_vec(&value).unwrap()
}

fn outline(value: &mut Value) {
    match value {
        Value::String(text) if text.chars().count() > 48 => {
            let head: String = text.chars().take(48).collect();
            *text = format!("{head}...");
        }
        Value::Array(items) => items.iter_mut().for_each(outline),
        Value::Object(map) => map.values_mut().for_each(outline),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn pretty(body: &[u8]) -> String {
    let mut value: Value = serde_json::from_slice(body).unwrap();
    outline(&mut value);
    serde_json::to_string_pretty(&value).unwrap()
}

fn request(items: Vec<Item>) -> Request {
    Request {
        model: "gpt-5".to_owned(),
        system: vec![Part::Text("be brief".to_owned())],
        items,
        tools: Vec::new(),
        tool_choice: None,
        parallel_tool_calls: None,
        max_output_tokens: None,
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        effort: None,
        thinking: None,
        reasoning_summary: false,
        speed: None,
        structured_output: None,
        stream: false,
        cache_key: None,
        user: None,
    }
}

fn user(text: &str) -> Item {
    Item::Message {
        role: Role::User,
        parts: vec![Part::Text(text.to_owned())],
    }
}

fn raw(text: &str) -> RawJson {
    RawJson::parse(text.to_owned()).unwrap()
}

fn encoded(request: &Request) -> Vec<u8> {
    encode_request(request, &EncodeOptions::default())
        .unwrap()
        .body
        .to_vec()
}

fn rejection(body: &Value) -> Rejection {
    decode_request(body.to_string().as_bytes()).unwrap_err()
}

fn minimal(extra: &Value) -> Value {
    let mut base = json!({"model": "gpt-5", "input": [
        {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}
    ]});
    for (key, value) in extra.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

fn parse_frames(frames: &[Bytes]) -> Vec<sse::Frame> {
    let mut parser = sse::Parser::default();
    frames.iter().flat_map(|frame| parser.push(frame)).collect()
}

fn frames_text(frames: &[Bytes]) -> String {
    frames
        .iter()
        .map(|frame| String::from_utf8(frame.to_vec()).unwrap())
        .collect()
}

fn encode_events(events: &[Event]) -> Vec<Bytes> {
    let mut encoder = StreamEncoder::default();
    let mut frames: Vec<Bytes> = events.iter().flat_map(|e| encoder.push(e)).collect();
    frames.extend(encoder.finish());
    frames
}

fn decode_frames(frames: &[sse::Frame]) -> Vec<Event> {
    let mut decoder = StreamDecoder::default();
    frames
        .iter()
        .flat_map(|frame| decoder.push(frame).unwrap())
        .collect()
}

fn sample_events() -> Vec<Event> {
    let origin = Origin::OpenAi {
        id: Some("rs_up".to_owned()),
        encrypted: Some("enc".to_owned()),
    };
    vec![
        Event::Start {
            id: "resp_1".to_owned(),
            model: "gpt-5".to_owned(),
        },
        Event::BlockStart {
            index: 0,
            block: Block::Reasoning,
        },
        Event::ReasoningSummaryDelta {
            index: 0,
            text: "**Plan**".to_owned(),
        },
        Event::ReasoningDelta {
            index: 0,
            text: "raw thought".to_owned(),
        },
        Event::ReasoningOrigin { index: 0, origin },
        Event::BlockStop { index: 0 },
        Event::BlockStart {
            index: 1,
            block: Block::Text,
        },
        Event::TextDelta {
            index: 1,
            text: "Hel".to_owned(),
        },
        Event::TextDelta {
            index: 1,
            text: "lo".to_owned(),
        },
        Event::BlockStop { index: 1 },
        Event::BlockStart {
            index: 2,
            block: Block::ToolCall {
                id: "call_1".to_owned(),
                name: "exec_command".to_owned(),
                kind: ToolKind::Function,
            },
        },
        Event::ToolArgumentsDelta {
            index: 2,
            json: "{\"cmd\":".to_owned(),
        },
        Event::ToolArgumentsDelta {
            index: 2,
            json: "\"ls\"}".to_owned(),
        },
        Event::BlockStop { index: 2 },
        Event::BlockStart {
            index: 3,
            block: Block::ToolCall {
                id: "call_2".to_owned(),
                name: "apply_patch".to_owned(),
                kind: ToolKind::Custom,
            },
        },
        Event::ToolArgumentsDelta {
            index: 3,
            json: "*** Begin Patch".to_owned(),
        },
        Event::BlockStop { index: 3 },
        Event::Usage(Usage {
            input_uncached: 10,
            cache_read: 5,
            cache_write: 2,
            output: 7,
            reasoning: Some(3),
        }),
        Event::Stop(StopReason::ToolUse),
    ]
}

#[test]
fn decodes_every_codex_fixture_without_hosted_tools() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let body = without_hosted_tools(&fixture(&format!("codex/{name}.body.json")));
        let decoded = decode_request(&body).unwrap();
        assert_eq!(decoded.model, "gpt-5");
        assert!(decoded.stream);
        assert!(decoded.reasoning_summary);
        assert_eq!(decoded.system.len(), 3);
        assert_eq!(decoded.tool_choice, Some(ToolChoice::Auto));
        assert_eq!(decoded.parallel_tool_calls, Some(true));
        assert!(decoded.cache_key.is_some());
        assert_eq!(decoded.tools.len(), 7);
    }
}

#[test]
fn rejects_codex_fixtures_with_hosted_tools() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let error = decode_request(&fixture(&format!("codex/{name}.body.json"))).unwrap_err();
        assert_eq!(error, Rejection::unsupported("tool type namespace"));
    }
}

#[test]
fn rejects_web_search_tool() {
    let error = rejection(&minimal(&json!({"tools": [{"type": "web_search"}]})));
    assert_eq!(error, Rejection::unsupported("tool type web_search"));
}

#[test]
fn codex_tool_round_trip_keeps_call_and_result() {
    let body = without_hosted_tools(&fixture("codex/tool-2.body.json"));
    let decoded = decode_request(&body).unwrap();
    let last_two: Vec<&Item> = decoded.items.iter().rev().take(2).collect();
    assert!(matches!(
        last_two.first(),
        Some(Item::ToolResult(ToolResult { call_id, kind: ToolKind::Function, .. })) if call_id == "call_fake001"
    ));
    assert!(matches!(
        last_two.get(1),
        Some(Item::ToolCall(ToolCall { id, arguments, .. }))
            if id == "call_fake001" && arguments == "{\"cmd\": \"echo hi\"}"
    ));
}

#[test]
fn request_survives_decode_encode_decode() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let body = without_hosted_tools(&fixture(&format!("codex/{name}.body.json")));
        let first = decode_request(&body).unwrap();
        let second = decode_request(&encoded(&first)).unwrap();
        assert_eq!(first, second);
    }
}

#[test]
fn snapshot_encode_codex_tool_2() {
    let body = without_hosted_tools(&fixture("codex/tool-2.body.json"));
    let decoded = decode_request(&body).unwrap();
    insta::assert_snapshot!(pretty(&encoded(&decoded)));
}

#[test]
fn snapshot_encode_codex_plain_1() {
    let body = without_hosted_tools(&fixture("codex/plain-1.body.json"));
    let decoded = decode_request(&body).unwrap();
    insta::assert_snapshot!(pretty(&encoded(&decoded)));
}

#[test]
fn decodes_custom_tool_and_calls() {
    let body = json!({
        "model": "gpt-5",
        "input": [
            {"type": "message", "role": "user", "content": "patch it"},
            {"type": "custom_tool_call", "call_id": "c1", "name": "apply_patch", "input": "*** Begin"},
            {"type": "custom_tool_call_output", "call_id": "c1", "output": "ok"}
        ],
        "tools": [{"type": "custom", "name": "apply_patch", "description": "d",
            "format": {"type": "grammar", "syntax": "lark", "definition": "start: x"}}],
        "tool_choice": {"type": "custom", "name": "apply_patch"}
    });
    let decoded = decode_request(body.to_string().as_bytes()).unwrap();
    assert!(
        matches!(&decoded.tools[..], [Tool::Custom { name, format: Some(_), .. }] if name == "apply_patch")
    );
    assert_eq!(
        decoded.tool_choice,
        Some(ToolChoice::Named("apply_patch".to_owned()))
    );
    assert!(matches!(
        &decoded.items[1],
        Item::ToolCall(ToolCall { kind: ToolKind::Custom, arguments, .. }) if arguments == "*** Begin"
    ));
    assert!(matches!(
        &decoded.items[2],
        Item::ToolResult(ToolResult {
            kind: ToolKind::Custom,
            ..
        })
    ));
    insta::assert_snapshot!(pretty(&encoded(&decoded)));
}

#[test]
fn leading_developer_items_become_system_and_later_ones_items() {
    let body = json!({
        "model": "gpt-5",
        "instructions": "base",
        "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "d1"}]},
            {"type": "message", "role": "system", "content": "d2"},
            {"type": "message", "role": "user", "content": "u"},
            {"type": "message", "role": "developer", "content": "late"}
        ]
    });
    let decoded = decode_request(body.to_string().as_bytes()).unwrap();
    assert_eq!(
        decoded.system,
        vec![
            Part::Text("base".to_owned()),
            Part::Text("d1".to_owned()),
            Part::Text("d2".to_owned())
        ]
    );
    assert!(matches!(
        &decoded.items[..],
        [Item::Message { .. }, Item::System { .. }]
    ));
    let encoded = pretty(&encoded(&decoded));
    assert!(!encoded.contains("\"system\""));
    assert_eq!(encoded.matches("\"developer\"").count(), 2);
}

#[test]
fn maps_effort_summary_tier_cache_key_and_sampling() {
    let decoded = decode_request(
        minimal(&json!({
            "reasoning": {"effort": "high", "summary": "detailed"},
            "service_tier": "priority",
            "prompt_cache_key": "k",
            "max_output_tokens": 99,
            "temperature": 0.5,
            "top_p": 0.9,
            "user": "u1",
            "store": true,
            "include": ["reasoning.encrypted_content"],
            "stream_options": {"include_obfuscation": false},
            "client_metadata": {"a": "b"}
        }))
        .to_string()
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(decoded.effort, Some(Effort::High));
    assert!(decoded.reasoning_summary);
    assert_eq!(decoded.speed, Some(Speed::Fast));
    assert_eq!(decoded.cache_key.as_deref(), Some("k"));
    assert_eq!(decoded.max_output_tokens, Some(99));
    assert_eq!(decoded.user.as_deref(), Some("u1"));
    assert!(!decoded.stream);
}

#[test]
fn auto_and_default_tier_mean_unspecified() {
    for tier in ["auto", "default"] {
        let decoded = decode_request(
            minimal(&json!({"service_tier": tier}))
                .to_string()
                .as_bytes(),
        );
        assert_eq!(decoded.unwrap().speed, None);
    }
}

#[test]
fn rejects_flex_and_scale_tier() {
    for tier in ["flex", "scale"] {
        let error = rejection(&minimal(&json!({"service_tier": tier})));
        assert_eq!(
            error,
            Rejection::unsupported(format!("service_tier {tier}"))
        );
    }
}

#[test]
fn rejects_previous_response_id() {
    let error = rejection(&minimal(&json!({"previous_response_id": "resp_1"})));
    assert_eq!(error, Rejection::unsupported("previous_response_id"));
}

#[test]
fn rejects_unknown_top_level_field() {
    let error = rejection(&minimal(&json!({"truncation": "auto"})));
    assert_eq!(error, Rejection::unsupported("truncation"));
}

#[test]
fn rejects_unknown_nested_field() {
    let mut body = minimal(&json!({}));
    body["input"][0]["phase"] = json!("commentary");
    assert_eq!(
        rejection(&body),
        Rejection::unsupported("input.message.phase")
    );
}

#[test]
fn rejects_untranslatable_transport_fields() {
    assert_eq!(
        rejection(&minimal(
            &json!({"include": ["message.output_text.logprobs"]})
        )),
        Rejection::unsupported("include message.output_text.logprobs")
    );
    assert_eq!(
        rejection(&minimal(&json!({"text": {"verbosity": "low"}}))),
        Rejection::unsupported("text.verbosity")
    );
    assert_eq!(
        rejection(&minimal(&json!({"prompt_cache_options": {"ttl": "1h"}}))),
        Rejection::unsupported("prompt_cache_options")
    );
}

#[test]
fn rejects_allowed_tools_choice() {
    let error = rejection(&minimal(
        &json!({"tool_choice": {"type": "allowed_tools", "mode": "auto", "tools": []}}),
    ));
    assert_eq!(error, Rejection::unsupported("tool_choice"));
}

#[test]
fn rejects_non_default_image_detail() {
    let mut body = minimal(&json!({}));
    body["input"][0]["content"] = json!([
        {"type": "input_image", "image_url": "https://x/y.png", "detail": "high"}
    ]);
    assert_eq!(
        rejection(&body),
        Rejection::unsupported("input_image.detail high")
    );
}

#[test]
fn decodes_structured_output_and_media() {
    let mut body = minimal(&json!({
        "text": {"format": {"type": "json_schema", "name": "out", "schema": {"type": "object"}, "strict": true}}
    }));
    body["input"][0]["content"] = json!([
        {"type": "input_image", "image_url": "data:image/png;base64,AAAA"},
        {"type": "input_file", "file_data": "data:application/pdf;base64,BBBB", "filename": "a.pdf"}
    ]);
    let decoded = decode_request(body.to_string().as_bytes()).unwrap();
    assert_eq!(
        decoded.structured_output,
        Some(StructuredOutput {
            name: Some("out".to_owned()),
            schema: raw("{\"type\":\"object\"}"),
            strict: Some(true),
        })
    );
    assert_eq!(
        decoded.items[0],
        Item::Message {
            role: Role::User,
            parts: vec![
                Part::Image(Media::Base64 {
                    media_type: "image/png".to_owned(),
                    data: "AAAA".to_owned()
                }),
                Part::Document(Document {
                    media: Media::Base64 {
                        media_type: "application/pdf".to_owned(),
                        data: "BBBB".to_owned()
                    },
                    title: Some("a.pdf".to_owned())
                }),
            ]
        }
    );
    assert_eq!(decode_request(&encoded(&decoded)).unwrap(), decoded);
}

#[test]
fn reasoning_items_restore_sealed_origin_or_wrap_openai() {
    let sealed = envelope::seal(&Origin::Anthropic {
        signature: "sig".to_owned(),
    });
    let body = json!({"model": "gpt-5", "input": [
        {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "s"}], "encrypted_content": sealed},
        {"type": "reasoning", "id": "rs_2", "summary": [], "encrypted_content": "opaque"}
    ]});
    let decoded = decode_request(body.to_string().as_bytes()).unwrap();
    assert!(matches!(
        &decoded.items[0],
        Item::Reasoning(Reasoning { origin: Origin::Anthropic { signature }, summary, .. })
            if signature == "sig" && summary == &["s".to_owned()]
    ));
    assert!(matches!(
        &decoded.items[1],
        Item::Reasoning(Reasoning { origin: Origin::OpenAi { id: Some(id), encrypted: Some(e) }, .. })
            if id == "rs_2" && e == "opaque"
    ));
}

#[test]
fn broken_sealed_reasoning_is_rejected() {
    let body = json!({"model": "gpt-5", "input": [
        {"type": "reasoning", "summary": [], "encrypted_content": "gw1:@@@"}
    ]});
    assert_eq!(rejection(&body), Rejection::ThinkingSignature);
}

#[test]
fn encode_always_stateless_with_encrypted_reasoning() {
    let out: Value = serde_json::from_slice(&encoded(&request(vec![user("hi")]))).unwrap();
    assert_eq!(out["store"], false);
    assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(out["stream"], false);
    assert_eq!(out["input"][0]["role"], "developer");
    assert_eq!(out["input"][0]["content"][0]["text"], "be brief");
}

#[test]
fn force_stream_is_noted_only_when_it_changes_the_request() {
    let options = EncodeOptions {
        force_stream: true,
        ..EncodeOptions::default()
    };
    let result = encode_request(&request(vec![user("hi")]), &options).unwrap();
    assert_eq!(result.notes, vec![Note::StreamForced]);
    let body: Value = serde_json::from_slice(&result.body).unwrap();
    assert_eq!(body["stream"], true);
    let streaming = Request {
        stream: true,
        ..request(vec![user("hi")])
    };
    assert_eq!(encode_request(&streaming, &options).unwrap().notes, []);
}

#[test]
fn cache_key_prefers_options_and_falls_back_to_request() {
    let mut req = request(vec![user("hi")]);
    req.cache_key = Some("from-request".to_owned());
    let options = EncodeOptions {
        cache_key: Some("from-options".to_owned()),
        ..EncodeOptions::default()
    };
    let body: Value =
        serde_json::from_slice(&encode_request(&req, &options).unwrap().body).unwrap();
    assert_eq!(body["prompt_cache_key"], "from-options");
    let body: Value = serde_json::from_slice(&encoded(&req)).unwrap();
    assert_eq!(body["prompt_cache_key"], "from-request");
}

#[test]
fn encode_rejects_foreign_and_plain_reasoning() {
    for origin in [
        Origin::Anthropic {
            signature: "s".to_owned(),
        },
        Origin::AnthropicRedacted {
            data: "d".to_owned(),
        },
        Origin::Plain,
    ] {
        let req = request(vec![Item::Reasoning(Reasoning {
            text: "t".to_owned(),
            summary: Vec::new(),
            origin,
        })]);
        assert_eq!(
            encode_request(&req, &EncodeOptions::default()).unwrap_err(),
            Rejection::ThinkingSignature
        );
    }
}

#[test]
fn encode_keeps_openai_reasoning() {
    let req = request(vec![Item::Reasoning(Reasoning {
        text: String::new(),
        summary: vec!["s".to_owned()],
        origin: Origin::OpenAi {
            id: Some("rs_1".to_owned()),
            encrypted: Some("enc".to_owned()),
        },
    })]);
    let out: Value = serde_json::from_slice(&encoded(&req)).unwrap();
    assert_eq!(out["input"][1]["type"], "reasoning");
    assert_eq!(out["input"][1]["id"], "rs_1");
    assert_eq!(out["input"][1]["encrypted_content"], "enc");
    assert_eq!(out["input"][1]["summary"][0]["text"], "s");
}

#[test]
fn encode_rejects_stop_sequences() {
    let req = Request {
        stop: vec!["x".to_owned()],
        ..request(vec![user("hi")])
    };
    assert!(matches!(
        encode_request(&req, &EncodeOptions::default()),
        Err(Rejection::Unsupported(_))
    ));
}

#[test]
fn encode_thinking_modes() {
    let adaptive = Request {
        thinking: Some(Thinking::Adaptive),
        effort: Some(Effort::Low),
        ..request(vec![user("hi")])
    };
    let out: Value = serde_json::from_slice(&encoded(&adaptive)).unwrap();
    assert_eq!(
        out["reasoning"],
        json!({"effort": "low", "summary": "auto"})
    );
    let enabled = Request {
        thinking: Some(Thinking::Enabled { budget_tokens: 10 }),
        ..request(vec![user("hi")])
    };
    assert_eq!(
        encode_request(&enabled, &EncodeOptions::default()).unwrap_err(),
        Rejection::ThinkingType(ThinkingKind::Enabled)
    );
    let disabled = Request {
        thinking: Some(Thinking::Disabled),
        ..request(vec![user("hi")])
    };
    assert!(matches!(
        encode_request(&disabled, &EncodeOptions::default()),
        Err(Rejection::Unsupported(_))
    ));
}

#[test]
fn structured_output_requires_a_name() {
    let req = Request {
        structured_output: Some(StructuredOutput {
            name: None,
            schema: raw("{}"),
            strict: None,
        }),
        ..request(vec![user("hi")])
    };
    assert_eq!(
        encode_request(&req, &EncodeOptions::default()).unwrap_err(),
        Rejection::StructuredOutputs
    );
}

#[test]
fn tool_error_flag_is_dropped_with_a_note() {
    let req = request(vec![
        Item::ToolCall(ToolCall {
            id: "c".to_owned(),
            name: "n".to_owned(),
            kind: ToolKind::Function,
            arguments: "{}".to_owned(),
        }),
        Item::ToolResult(ToolResult {
            call_id: "c".to_owned(),
            kind: ToolKind::Function,
            parts: vec![Part::Text("boom".to_owned())],
            is_error: true,
        }),
    ]);
    let result = encode_request(&req, &EncodeOptions::default()).unwrap();
    assert_eq!(result.notes, vec![Note::ToolErrorFlagDropped]);
}

#[test]
fn base64_document_needs_a_filename() {
    let req = request(vec![Item::Message {
        role: Role::User,
        parts: vec![Part::Document(Document {
            media: Media::Base64 {
                media_type: "application/pdf".to_owned(),
                data: "AA".to_owned(),
            },
            title: None,
        })],
    }]);
    assert_eq!(
        encode_request(&req, &EncodeOptions::default()).unwrap_err(),
        Rejection::DocumentBlock
    );
}

#[test]
fn snapshot_encode_full_ir_request() {
    let req = Request {
        tools: vec![
            Tool::Function {
                name: "exec".to_owned(),
                description: Some("run".to_owned()),
                parameters: raw(
                    "{\"type\":\"object\",\"properties\":{\"cmd\":{\"type\":\"string\"}}}",
                ),
                strict: None,
            },
            Tool::Custom {
                name: "apply_patch".to_owned(),
                description: None,
                format: Some(raw("{\"type\":\"grammar\"}")),
            },
        ],
        tool_choice: Some(ToolChoice::Named("apply_patch".to_owned())),
        parallel_tool_calls: Some(false),
        max_output_tokens: Some(1000),
        temperature: Some(0.25),
        top_p: Some(0.5),
        effort: Some(Effort::Xhigh),
        reasoning_summary: true,
        speed: Some(Speed::Fast),
        structured_output: Some(StructuredOutput {
            name: Some("answer".to_owned()),
            schema: raw("{\"type\":\"object\"}"),
            strict: Some(true),
        }),
        stream: true,
        cache_key: Some("session".to_owned()),
        user: Some("u".to_owned()),
        ..request(vec![
            user("go"),
            Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Text("ok".to_owned())],
            },
            Item::System {
                parts: vec![Part::Text("note".to_owned())],
            },
            Item::ToolCall(ToolCall {
                id: "c1".to_owned(),
                name: "apply_patch".to_owned(),
                kind: ToolKind::Custom,
                arguments: "*** Begin".to_owned(),
            }),
            Item::ToolResult(ToolResult {
                call_id: "c1".to_owned(),
                kind: ToolKind::Custom,
                parts: vec![
                    Part::Text("done".to_owned()),
                    Part::Image(Media::Url("https://x/y.png".to_owned())),
                ],
                is_error: false,
            }),
        ])
    };
    insta::assert_snapshot!(pretty(&encoded(&req)));
}

#[test]
fn encoding_is_deterministic() {
    let body = without_hosted_tools(&fixture("codex/tool-2.body.json"));
    let decoded = decode_request(&body).unwrap();
    assert_eq!(encoded(&decoded), encoded(&decoded));
    assert_eq!(
        encode_response(&sample_response()),
        encode_response(&sample_response())
    );
    assert_eq!(
        frames_text(&encode_events(&sample_events())),
        frames_text(&encode_events(&sample_events()))
    );
}

#[test]
fn appending_items_keeps_the_previous_body_as_a_byte_prefix() {
    let body = without_hosted_tools(&fixture("codex/tool-1.body.json"));
    let base = decode_request(&body).unwrap();
    let mut longer = base.clone();
    longer.items.push(Item::Message {
        role: Role::Assistant,
        parts: vec![Part::Text("more".to_owned())],
    });
    longer.items.push(user("next"));
    let short = encoded(&base);
    let long = encoded(&longer);
    let short_prefix = short.strip_suffix(b"]}").unwrap();
    assert!(long.starts_with(short_prefix));
}

fn sample_response() -> Response {
    Response {
        id: "resp_9".to_owned(),
        model: "gpt-5".to_owned(),
        output: vec![
            Item::Reasoning(Reasoning {
                text: "raw".to_owned(),
                summary: vec!["sum".to_owned()],
                origin: Origin::Anthropic {
                    signature: "sig".to_owned(),
                },
            }),
            Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Text("hello".to_owned())],
            },
            Item::ToolCall(ToolCall {
                id: "call_1".to_owned(),
                name: "exec".to_owned(),
                kind: ToolKind::Function,
                arguments: "{\"a\":1}".to_owned(),
            }),
            Item::ToolCall(ToolCall {
                id: "call_2".to_owned(),
                name: "apply_patch".to_owned(),
                kind: ToolKind::Custom,
                arguments: "patch".to_owned(),
            }),
        ],
        stop: StopReason::ToolUse,
        usage: Usage {
            input_uncached: 10,
            cache_read: 5,
            cache_write: 0,
            output: 7,
            reasoning: Some(2),
        },
    }
}

#[test]
fn snapshot_encode_response() {
    insta::assert_snapshot!(pretty(&encode_response(&sample_response())));
}

#[test]
fn response_round_trips_with_sealed_reasoning() {
    let original = sample_response();
    let decoded = decode_response(&encode_response(&original)).unwrap();
    assert_eq!(decoded.id, original.id);
    assert_eq!(decoded.stop, StopReason::ToolUse);
    assert_eq!(decoded.usage, original.usage);
    assert_eq!(decoded.output.len(), original.output.len());
    let Some(Item::Reasoning(Reasoning {
        origin: Origin::OpenAi {
            encrypted: Some(encrypted),
            ..
        },
        text,
        summary,
    })) = decoded.output.first()
    else {
        panic!("reasoning first");
    };
    assert_eq!(text, "raw");
    assert_eq!(summary, &["sum".to_owned()]);
    assert_eq!(
        envelope::open(encrypted),
        Some(Origin::Anthropic {
            signature: "sig".to_owned()
        })
    );
    assert_eq!(decoded.output[1..], original.output[1..]);
}

#[test]
fn encode_response_maps_stop_reasons() {
    let status = |stop| {
        let response = Response {
            stop,
            ..sample_response()
        };
        let value: Value = serde_json::from_slice(&encode_response(&response)).unwrap();
        (
            value["status"].as_str().unwrap().to_owned(),
            value["incomplete_details"]["reason"]
                .as_str()
                .map(str::to_owned),
        )
    };
    assert_eq!(status(StopReason::EndTurn).0, "completed");
    assert_eq!(
        status(StopReason::MaxTokens),
        (
            "incomplete".to_owned(),
            Some("max_output_tokens".to_owned())
        )
    );
    assert_eq!(
        status(StopReason::ContentFilter),
        ("incomplete".to_owned(), Some("content_filter".to_owned()))
    );
    assert_eq!(status(StopReason::ContextWindow).0, "failed");
}

#[test]
fn decode_response_subtracts_cached_tokens_from_input() {
    let body = json!({
        "id": "resp_1", "model": "gpt-5", "status": "completed",
        "output": [{"type": "message", "id": "m", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "hi", "annotations": []}]}],
        "usage": {"input_tokens": 100, "input_tokens_details": {"cached_tokens": 40},
            "output_tokens": 9, "output_tokens_details": {"reasoning_tokens": 4}, "total_tokens": 109}
    });
    let response = decode_response(body.to_string().as_bytes()).unwrap();
    assert_eq!(
        response.usage,
        Usage {
            input_uncached: 60,
            cache_read: 40,
            cache_write: 0,
            output: 9,
            reasoning: Some(4)
        }
    );
    assert_eq!(response.stop, StopReason::EndTurn);
}

#[test]
fn decode_response_maps_incomplete_and_failed() {
    let incomplete = json!({"id": "r", "model": "m", "status": "incomplete",
        "incomplete_details": {"reason": "max_output_tokens"}, "output": []});
    assert_eq!(
        decode_response(incomplete.to_string().as_bytes())
            .unwrap()
            .stop,
        StopReason::MaxTokens
    );
    let failed = json!({"id": "r", "model": "m", "status": "failed",
        "error": {"code": "server_error", "message": "bad"}, "output": []});
    assert!(decode_response(failed.to_string().as_bytes()).is_err());
}

#[test]
fn decode_response_rejects_unsupported_output_items() {
    let body = json!({"id": "r", "model": "m", "status": "completed",
        "output": [{"type": "web_search_call", "id": "w", "status": "completed"}]});
    assert!(decode_response(body.to_string().as_bytes()).is_err());
}

fn frames_of(path: &str) -> Vec<sse::Frame> {
    sse::Parser::default().push(&fixture(path))
}

#[test]
fn stream_decoder_reads_the_text_fixture() {
    let events = decode_frames(&frames_of("upstream/openai-responses-text.sse"));
    assert!(matches!(events.first(), Some(Event::Start { model, .. }) if model == "gpt-6-astra"));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::BlockStart {
            block: Block::Text,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TextDelta { text, .. } if text == "In"))
    );
    assert_eq!(events.last(), Some(&Event::Stop(StopReason::EndTurn)));
    insta::assert_debug_snapshot!(events);
}

#[test]
fn stream_decoder_reads_the_reasoning_fixture() {
    let events = decode_frames(&frames_of("upstream/openai-responses-reasoning.sse"));
    assert!(matches!(
        events.first(),
        Some(Event::BlockStart {
            block: Block::Reasoning,
            ..
        })
    ));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::ReasoningSummaryDelta { .. }))
            .count(),
        1
    );
}

#[test]
fn stream_decoder_maps_the_failed_and_error_fixtures() {
    let failed = decode_frames(&frames_of("upstream/openai-responses-failed.sse"));
    assert!(matches!(&failed[..], [Event::Error(body)]
        if body.status == 500 && body.kind == ErrorKind::Server && body.message == "The model failed to generate a response."));
    let error = decode_frames(&frames_of("upstream/openai-responses-error.sse"));
    assert!(matches!(&error[..], [Event::Error(body)] if body.message == "Something went wrong"));
}

#[test]
fn stream_decoder_requires_the_item_before_function_call_arguments() {
    let mut decoder = StreamDecoder::default();
    let frames = frames_of("upstream/openai-responses-function_call.sse");
    assert!(decoder.push(&frames[0]).is_err());
}

fn frame(value: &Value) -> sse::Frame {
    sse::Frame {
        event: value["type"].as_str().map(str::to_owned),
        data: value.to_string(),
    }
}

#[test]
fn stream_decoder_handles_tool_call_streams_and_usage() {
    let frames = [
        json!({"type": "response.created", "response": {"id": "r1", "model": "m"}}),
        json!({"type": "response.output_item.added", "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "exec", "arguments": ""}}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_1", "delta": "{\"a\""}),
        json!({"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_1", "delta": ":1}"}),
        json!({"type": "response.function_call_arguments.done", "output_index": 0, "item_id": "fc_1", "arguments": "{\"a\":1}"}),
        json!({"type": "response.output_item.done", "output_index": 0,
            "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "exec", "arguments": "{\"a\":1}"}}),
        json!({"type": "response.output_item.added", "output_index": 1,
            "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_2", "name": "apply_patch", "input": ""}}),
        json!({"type": "response.custom_tool_call_input.delta", "output_index": 1, "item_id": "ctc_1", "delta": "patch"}),
        json!({"type": "response.output_item.done", "output_index": 1,
            "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_2", "name": "apply_patch", "input": "patch"}}),
        json!({"type": "response.completed", "response": {"id": "r1", "model": "m", "status": "completed", "output": [],
            "usage": {"input_tokens": 50, "input_tokens_details": {"cached_tokens": 20}, "output_tokens": 5}}}),
    ];
    let frames: Vec<sse::Frame> = frames.iter().map(frame).collect();
    let events = decode_frames(&frames);
    insta::assert_debug_snapshot!(events);
}

#[test]
fn stream_decoder_fills_in_streams_that_only_send_done_items() {
    let frames = [
        json!({"type": "response.output_item.done", "output_index": 0,
            "item": {"type": "function_call", "call_id": "c", "name": "n", "arguments": "{}"}}),
        json!({"type": "response.output_item.done", "output_index": 1,
            "item": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "hi", "annotations": []}]}}),
        json!({"type": "response.completed", "response": {"id": "r", "model": "m", "output": []}}),
    ];
    let frames: Vec<sse::Frame> = frames.iter().map(frame).collect();
    let events = decode_frames(&frames);
    assert!(events.contains(&Event::ToolArgumentsDelta {
        index: 0,
        json: "{}".to_owned()
    }));
    assert!(events.contains(&Event::TextDelta {
        index: 1,
        text: "hi".to_owned()
    }));
    assert_eq!(events.last(), Some(&Event::Stop(StopReason::ToolUse)));
}

#[test]
fn stream_decoder_inserts_a_separator_between_summary_parts() {
    let frames = [
        json!({"type": "response.reasoning_summary_part.added", "output_index": 0, "summary_index": 0}),
        json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "summary_index": 0, "delta": "a"}),
        json!({"type": "response.reasoning_summary_part.added", "output_index": 0, "summary_index": 1}),
        json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "summary_index": 1, "delta": "b"}),
    ];
    let frames: Vec<sse::Frame> = frames.iter().map(frame).collect();
    let texts: Vec<String> = decode_frames(&frames)
        .into_iter()
        .filter_map(|event| match event {
            Event::ReasoningSummaryDelta { text, .. } => Some(text),
            Event::Start { .. }
            | Event::BlockStart { .. }
            | Event::TextDelta { .. }
            | Event::ReasoningDelta { .. }
            | Event::ReasoningOrigin { .. }
            | Event::ToolArgumentsDelta { .. }
            | Event::BlockStop { .. }
            | Event::Usage(_)
            | Event::Stop(_)
            | Event::Error(_) => None,
        })
        .collect();
    assert_eq!(texts, ["a", "\n\nb"]);
}

#[test]
fn stream_decoder_maps_incomplete_stop() {
    let frame = frame(&json!({"type": "response.incomplete", "response": {
        "id": "r", "model": "m", "incomplete_details": {"reason": "content_filter"}}}));
    let events = StreamDecoder::default().push(&frame).unwrap();
    assert_eq!(events, vec![Event::Stop(StopReason::ContentFilter)]);
}

#[test]
fn snapshot_stream_encoder_lifecycle() {
    insta::assert_snapshot!(frames_text(&encode_events(&sample_events())));
}

#[test]
fn stream_encoder_sequence_numbers_increase_by_one() {
    let frames = parse_frames(&encode_events(&sample_events()));
    let numbers: Vec<u64> = frames
        .iter()
        .map(|f| {
            serde_json::from_str::<Value>(&f.data).unwrap()["sequence_number"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert!(numbers.len() > 20);
    assert!(numbers.iter().copied().eq(0..numbers.len() as u64));
    assert!(frames.iter().all(|f| {
        let value: Value = serde_json::from_str(&f.data).unwrap();
        f.event.as_deref() == value["type"].as_str()
    }));
}

#[test]
fn stream_round_trips_through_encoder_and_decoder() {
    let frames = parse_frames(&encode_events(&sample_events()));
    let decoded = decode_frames(&frames);
    let mut expected = sample_events();
    for event in &mut expected {
        if let Event::ReasoningOrigin { origin, .. } = event {
            *origin = Origin::OpenAi {
                id: Some("rs_0".to_owned()),
                encrypted: Some(envelope::seal(origin)),
            };
        }
    }
    assert_eq!(decoded, expected);
}

#[test]
fn stream_encoder_completed_event_carries_output_and_usage() {
    let frames = parse_frames(&encode_events(&sample_events()));
    let last: Value = serde_json::from_str(&frames.last().unwrap().data).unwrap();
    assert_eq!(last["type"], "response.completed");
    assert_eq!(last["response"]["output"].as_array().unwrap().len(), 4);
    assert_eq!(last["response"]["usage"]["input_tokens"], 17);
    assert_eq!(
        last["response"]["usage"]["input_tokens_details"]["cached_tokens"],
        5
    );
    assert_eq!(
        last["response"]["output"][2]["arguments"],
        "{\"cmd\":\"ls\"}"
    );
}

#[test]
fn stream_encoder_without_stop_fails_the_response() {
    let events = &sample_events()[..8];
    let frames = parse_frames(&encode_events(events));
    let last: Value = serde_json::from_str(&frames.last().unwrap().data).unwrap();
    assert_eq!(last["type"], "response.failed");
    assert_eq!(last["response"]["error"]["code"], "server_error");
}

#[test]
fn stream_encoder_max_tokens_is_incomplete() {
    let events = [
        Event::Start {
            id: "r".to_owned(),
            model: "m".to_owned(),
        },
        Event::Stop(StopReason::MaxTokens),
    ];
    let frames = parse_frames(&encode_events(&events));
    let last: Value = serde_json::from_str(&frames.last().unwrap().data).unwrap();
    assert_eq!(last["type"], "response.incomplete");
    assert_eq!(
        last["response"]["incomplete_details"]["reason"],
        "max_output_tokens"
    );
}

#[test]
fn stream_encoder_error_after_start_becomes_response_failed() {
    let events = [
        Event::Start {
            id: "r".to_owned(),
            model: "m".to_owned(),
        },
        Event::Error(ErrorBody::new(429, ErrorKind::RateLimit, "slow down")),
    ];
    let frames = parse_frames(&encode_events(&events));
    let last: Value = serde_json::from_str(&frames.last().unwrap().data).unwrap();
    assert_eq!(last["type"], "response.failed");
    assert_eq!(last["response"]["error"]["code"], "rate_limit_exceeded");
    assert_eq!(last["response"]["error"]["message"], "slow down");
}

#[test]
fn stream_encoder_error_before_start_is_a_bare_error_event() {
    let events = [Event::Error(ErrorBody::new(500, ErrorKind::Server, "boom"))];
    let frames = parse_frames(&encode_events(&events));
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].event.as_deref(), Some("error"));
    let value: Value = serde_json::from_str(&frames[0].data).unwrap();
    assert_eq!(value["type"], "error");
}

#[test]
fn stream_encoder_ignores_events_after_the_terminal_event() {
    let mut encoder = StreamEncoder::default();
    for event in sample_events() {
        encoder.push(&event);
    }
    assert_ne!(encoder.finish(), [] as [Bytes; 0]);
    assert_eq!(encoder.finish(), [] as [Bytes; 0]);
    assert_eq!(
        encoder.push(&Event::TextDelta {
            index: 1,
            text: "x".to_owned()
        }),
        [] as [Bytes; 0]
    );
}

#[test]
fn stream_encoder_is_silent_when_nothing_arrived() {
    assert_eq!(StreamEncoder::default().finish(), [] as [Bytes; 0]);
}
