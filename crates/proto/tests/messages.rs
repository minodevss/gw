#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "helpers outside #[test] functions are not covered by clippy.toml test allowances"
)]

use std::fs;
use std::path::PathBuf;

use bytes::Bytes;
use insta::{assert_json_snapshot, assert_snapshot};
use proto::codec::{EncodeOptions, Note};
use proto::envelope;
use proto::error::{ErrorBody, ErrorKind};
use proto::ir::{
    Block, Document, Effort, Event, Item, Media, Origin, Part, Reasoning, Request, Response, Role,
    Speed, StopReason, StructuredOutput, Thinking, Tool, ToolCall, ToolChoice, ToolKind,
    ToolResult, Usage,
};
use proto::messages::{
    StreamDecoder, StreamEncoder, decode_request, decode_response, encode_request, encode_response,
    ping,
};
use proto::raw::RawJson;
use proto::reject::Rejection;
use proto::sse;
use serde_json::{Value, json};

const CONTEXT_MANAGEMENT: &str = "context-management-2025-06-27";
const PER_TURN_CONTROL: &str = "per-turn-control-2026-07-01";

fn fixture_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

fn claude_body(name: &str) -> Value {
    let bytes = fs::read(fixture_path(&format!("claude/{name}.body.json"))).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn claude_retry_body(name: &str) -> Vec<u8> {
    let mut body = claude_body(name);
    body.as_object_mut().unwrap().remove("context_management");
    for message in body["messages"].as_array_mut().unwrap() {
        message.as_object_mut().unwrap().remove("output_config");
    }
    body["tools"]
        .as_array_mut()
        .unwrap()
        .retain(|tool| tool.get("defer_loading").is_none());
    serde_json::to_vec(&body).unwrap()
}

fn raw(text: &str) -> RawJson {
    RawJson::parse(text.to_owned()).unwrap()
}

fn text_message(role: Role, text: &str) -> Item {
    Item::Message {
        role,
        parts: vec![Part::Text(text.to_owned())],
    }
}

fn request(items: Vec<Item>) -> Request {
    Request {
        model: "claude-opus-5-5".to_owned(),
        system: vec![Part::Text("be brief".to_owned())],
        items,
        tools: vec![
            Tool::Function {
                name: "Bash".to_owned(),
                description: Some("run a command".to_owned()),
                parameters: raw(r#"{"type":"object","properties":{"command":{"type":"string"}}}"#),
                strict: None,
            },
            Tool::Function {
                name: "Read".to_owned(),
                description: None,
                parameters: raw(r#"{"type":"object"}"#),
                strict: Some(true),
            },
        ],
        tool_choice: None,
        parallel_tool_calls: None,
        max_output_tokens: Some(1024),
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        effort: None,
        thinking: None,
        reasoning_summary: false,
        speed: None,
        structured_output: None,
        stream: true,
        cache_key: None,
        user: None,
    }
}

fn encode(request: &Request) -> String {
    let encoded = encode_request(request, &EncodeOptions::default()).unwrap();
    String::from_utf8(encoded.body.to_vec()).unwrap()
}

fn conversation(turns: usize) -> Vec<Item> {
    let mut items = Vec::new();
    for turn in 0..turns {
        items.push(text_message(Role::User, &format!("question {turn}")));
        items.push(Item::Reasoning(Reasoning {
            text: format!("thought {turn}"),
            summary: Vec::new(),
            origin: Origin::Anthropic {
                signature: format!("sig{turn}"),
            },
        }));
        items.push(Item::ToolCall(ToolCall {
            id: format!("toolu_{turn}"),
            name: "Bash".to_owned(),
            kind: ToolKind::Function,
            arguments: r#"{"command":"ls"}"#.to_owned(),
        }));
        items.push(Item::ToolResult(ToolResult {
            call_id: format!("toolu_{turn}"),
            kind: ToolKind::Function,
            parts: vec![Part::Text("file.txt".to_owned())],
            is_error: false,
        }));
        items.push(text_message(Role::Assistant, &format!("answer {turn}")));
    }
    items
}

fn outline(value: &Value) -> Value {
    match value {
        Value::String(text) if text.chars().count() > 48 => {
            Value::String(format!("{}...", text.chars().take(48).collect::<String>()))
        }
        Value::Array(items) => Value::Array(items.iter().map(outline).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = if key == "input_schema" {
                        Value::String("<schema>".to_owned())
                    } else {
                        outline(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.clone(),
    }
}

fn top_level_keys(body: &[u8]) -> Vec<String> {
    let map: serde_json::Map<String, Value> = serde_json::from_slice(body).unwrap();
    map.keys().cloned().collect()
}

#[test]
fn claude_fixtures_reject_context_management_beta() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let bytes = fs::read(fixture_path(&format!("claude/{name}.body.json"))).unwrap();
        assert_eq!(
            decode_request(&bytes),
            Err(Rejection::BetaHeader(CONTEXT_MANAGEMENT.to_owned())),
            "{name}"
        );
    }
}

#[test]
fn claude_fixtures_reject_per_turn_control_after_retry_without_context_management() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let mut body = claude_body(name);
        body.as_object_mut().unwrap().remove("context_management");
        let bytes = serde_json::to_vec(&body).unwrap();
        assert_eq!(
            decode_request(&bytes),
            Err(Rejection::BetaHeader(PER_TURN_CONTROL.to_owned())),
            "{name}"
        );
    }
}

#[test]
fn claude_fixtures_reject_deferred_tool_after_other_beta_fields_are_dropped() {
    let mut body: Value = serde_json::from_slice(&claude_retry_body("plain-1")).unwrap();
    body["tools"].as_array_mut().unwrap().push(json!({
        "name": "DeferredToolPlaceholder",
        "description": "d",
        "input_schema": {"type": "object"},
        "defer_loading": true
    }));
    assert_eq!(
        decode_request(&serde_json::to_vec(&body).unwrap()),
        Err(Rejection::BetaHeader(
            "advanced-tool-use-2025-11-20".to_owned()
        ))
    );
}

#[test]
fn claude_fixtures_decode_after_beta_fields_are_dropped() {
    for name in ["plain-1", "tool-1", "tool-2"] {
        let decoded = decode_request(&claude_retry_body(name)).unwrap();
        assert_eq!(decoded.model, "claude-opus-5-5");
        assert_eq!(decoded.system.len(), 3);
        assert_eq!(decoded.tools.len(), 11);
        assert_eq!(decoded.max_output_tokens, Some(128_000));
        assert_eq!(decoded.thinking, Some(Thinking::Adaptive));
        assert!(!decoded.reasoning_summary);
        assert_eq!(decoded.effort, Some(Effort::Medium));
        assert!(decoded.stream);
        assert_eq!(decoded.user, None);
        assert!(
            decoded
                .items
                .iter()
                .any(|item| matches!(item, Item::System { .. }))
        );
    }
}

#[test]
fn claude_tool_round_trip_decodes_to_call_and_result() {
    let decoded = decode_request(&claude_retry_body("tool-2")).unwrap();
    let kinds: Vec<&str> = decoded
        .items
        .iter()
        .map(|item| match item {
            Item::Message {
                role: Role::User, ..
            } => "user",
            Item::Message {
                role: Role::Assistant,
                ..
            } => "assistant",
            Item::System { .. } => "system",
            Item::Reasoning(_) => "reasoning",
            Item::ToolCall(_) => "call",
            Item::ToolResult(_) => "result",
        })
        .collect();
    assert_eq!(kinds, ["user", "system", "call", "result"]);
    let Some(Item::ToolCall(call)) = decoded.items.get(2) else {
        panic!("expected tool call");
    };
    assert_eq!(call.id, "toolu_fake001");
    assert_eq!(
        call.arguments,
        r#"{"command":"echo hi","description":"Echo hi"}"#
    );
}

#[test]
fn encodes_decoded_claude_fixture_without_mid_conversation_system() {
    let mut decoded = decode_request(&claude_retry_body("tool-2")).unwrap();
    assert_eq!(
        encode_request(&decoded, &EncodeOptions::default()),
        Err(Rejection::MidConvSystem)
    );
    decoded
        .items
        .retain(|item| !matches!(item, Item::System { .. }));
    decoded.effort = Some(Effort::High);
    let encoded = encode_request(&decoded, &EncodeOptions::default()).unwrap();
    let value: Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_json_snapshot!("encoded_claude_tool_2", outline(&value));
    assert_snapshot!(
        "encoded_claude_tool_2_key_order",
        top_level_keys(&encoded.body).join(",")
    );
}

#[test]
fn rejects_unknown_top_level_field() {
    let body = br#"{"model":"m","messages":[],"top_k":3}"#;
    assert_eq!(
        decode_request(body),
        Err(Rejection::Unsupported("unknown field top_k".to_owned()))
    );
}

#[test]
fn beta_field_wins_over_unknown_field() {
    let body = br#"{"model":"m","messages":[],"aaa":1,"mcp_servers":[]}"#;
    assert_eq!(
        decode_request(body),
        Err(Rejection::BetaHeader("mcp-servers-2025-12-04".to_owned()))
    );
}

#[test]
fn rejects_unknown_block_field() {
    let body = br#"{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"a","citations":[]}]}]}"#;
    assert!(matches!(
        decode_request(body),
        Err(Rejection::Unsupported(_))
    ));
}

#[test]
fn cache_control_scope_is_a_beta_field() {
    let body = br#"{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"a","cache_control":{"type":"ephemeral","scope":"global"}}]}]}"#;
    assert_eq!(
        decode_request(body),
        Err(Rejection::BetaHeader(
            "prompt-caching-scope-2026-01-05".to_owned()
        ))
    );
}

#[test]
fn drops_cache_control_ttl_and_decodes_text() {
    let body = br#"{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"a","cache_control":{"type":"ephemeral","ttl":"1h"}}]}]}"#;
    let decoded = decode_request(body).unwrap();
    assert_eq!(decoded.items, vec![text_message(Role::User, "a")]);
}

#[test]
fn decodes_every_request_field() {
    let schema = r#"{"type":"object","properties":{"a":{"type":"string"}}}"#;
    let body = json!({
        "model": "claude-opus-5-5",
        "max_tokens": 2048,
        "system": "be brief",
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
                {"type": "document", "title": "doc", "source": {"type": "url", "url": "https://example.com/a.pdf"}}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hm", "signature": "sig"},
                {"type": "redacted_thinking", "data": "red"},
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": "toolu_1", "name": "f", "input": {"b": 1,  "a": 2}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "done", "is_error": true},
                {"type": "text", "text": "thanks"}
            ]}
        ],
        "tools": [{"name": "f", "description": "d", "input_schema": serde_json::from_str::<Value>(schema).unwrap(), "strict": true}],
        "tool_choice": {"type": "tool", "name": "f", "disable_parallel_tool_use": true},
        "temperature": 0.5,
        "top_p": 0.9,
        "stop_sequences": ["END"],
        "thinking": {"type": "enabled", "budget_tokens": 2000},
        "output_config": {"effort": "high", "format": {"type": "json_schema", "schema": {"type": "object"}}},
        "speed": "fast",
        "metadata": {"user_id": "u1"},
        "stream": true
    });
    let decoded = decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(decoded.system, vec![Part::Text("be brief".to_owned())]);
    assert_eq!(decoded.tool_choice, Some(ToolChoice::Named("f".to_owned())));
    assert_eq!(decoded.parallel_tool_calls, Some(false));
    assert_eq!(decoded.max_output_tokens, Some(2048));
    assert_eq!(decoded.temperature, Some(0.5));
    assert_eq!(decoded.top_p, Some(0.9));
    assert_eq!(decoded.stop, vec!["END".to_owned()]);
    assert_eq!(decoded.effort, Some(Effort::High));
    assert_eq!(
        decoded.thinking,
        Some(Thinking::Enabled {
            budget_tokens: 2000
        })
    );
    assert_eq!(decoded.speed, Some(Speed::Fast));
    assert_eq!(
        decoded.structured_output,
        Some(StructuredOutput {
            name: None,
            schema: raw(r#"{"type":"object"}"#),
            strict: None,
        })
    );
    assert_eq!(decoded.user, None);
    assert_eq!(
        decoded.tools,
        vec![Tool::Function {
            name: "f".to_owned(),
            description: Some("d".to_owned()),
            parameters: raw(r#"{"properties":{"a":{"type":"string"}},"type":"object"}"#),
            strict: Some(true),
        }]
    );
    assert_eq!(
        decoded.items,
        vec![
            Item::Message {
                role: Role::User,
                parts: vec![
                    Part::Text("look".to_owned()),
                    Part::Image(Media::Base64 {
                        media_type: "image/png".to_owned(),
                        data: "AAAA".to_owned()
                    }),
                    Part::Document(Document {
                        media: Media::Url("https://example.com/a.pdf".to_owned()),
                        title: Some("doc".to_owned())
                    }),
                ]
            },
            Item::Reasoning(Reasoning {
                text: "hm".to_owned(),
                summary: Vec::new(),
                origin: Origin::Anthropic {
                    signature: "sig".to_owned()
                }
            }),
            Item::Reasoning(Reasoning {
                text: String::new(),
                summary: Vec::new(),
                origin: Origin::AnthropicRedacted {
                    data: "red".to_owned()
                }
            }),
            text_message(Role::Assistant, "ok"),
            Item::ToolCall(ToolCall {
                id: "toolu_1".to_owned(),
                name: "f".to_owned(),
                kind: ToolKind::Function,
                arguments: r#"{"a":2,"b":1}"#.to_owned()
            }),
            Item::ToolResult(ToolResult {
                call_id: "toolu_1".to_owned(),
                kind: ToolKind::Function,
                parts: vec![Part::Text("done".to_owned())],
                is_error: true
            }),
            text_message(Role::User, "thanks"),
        ]
    );
}

#[test]
fn opens_sealed_thinking_signature() {
    let origin = Origin::OpenAi {
        id: Some("rs_1".to_owned()),
        encrypted: Some("enc".to_owned()),
    };
    let body = json!({
        "model": "m",
        "messages": [{"role": "assistant", "content": [
            {"type": "thinking", "thinking": "t", "signature": envelope::seal(&origin)}
        ]}]
    });
    let decoded = decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    let Some(Item::Reasoning(reasoning)) = decoded.items.first() else {
        panic!("expected reasoning");
    };
    assert_eq!(reasoning.origin, origin);
}

#[test]
fn snapshot_encoded_conversation_with_tools() {
    let mut request = request(conversation(1));
    request.effort = Some(Effort::High);
    request.thinking = Some(Thinking::Enabled {
        budget_tokens: 4096,
    });
    request.tool_choice = Some(ToolChoice::Required);
    request.parallel_tool_calls = Some(false);
    request.temperature = Some(0.25);
    request.stop = vec!["END".to_owned()];
    request.speed = Some(Speed::Fast);
    request.user = Some("user-1".to_owned());
    request.structured_output = Some(StructuredOutput {
        name: Some("ignored".to_owned()),
        schema: raw(r#"{"type":"object"}"#),
        strict: Some(true),
    });
    assert_snapshot!("encoded_conversation_with_tools", encode(&request));
}

#[test]
fn snapshot_encoded_media_and_redacted_reasoning() {
    let mut request = request(vec![
        Item::Message {
            role: Role::User,
            parts: vec![
                Part::Text("see".to_owned()),
                Part::Text(String::new()),
                Part::Image(Media::Url("https://example.com/a.png".to_owned())),
                Part::Document(Document {
                    media: Media::Base64 {
                        media_type: "application/pdf".to_owned(),
                        data: "JVBER".to_owned(),
                    },
                    title: Some("spec".to_owned()),
                }),
            ],
        },
        Item::Reasoning(Reasoning {
            text: String::new(),
            summary: Vec::new(),
            origin: Origin::AnthropicRedacted {
                data: "red".to_owned(),
            },
        }),
        text_message(Role::Assistant, "done"),
    ]);
    request.tools.clear();
    request.reasoning_summary = true;
    request.stream = false;
    assert_snapshot!("encoded_media_and_redacted_reasoning", encode(&request));
}

#[test]
fn merges_consecutive_user_items_with_tool_results_first() {
    let request = request(vec![
        text_message(Role::User, "go"),
        Item::ToolCall(ToolCall {
            id: "t1".to_owned(),
            name: "Bash".to_owned(),
            kind: ToolKind::Function,
            arguments: "{}".to_owned(),
        }),
        text_message(Role::User, "note"),
        Item::ToolResult(ToolResult {
            call_id: "t1".to_owned(),
            kind: ToolKind::Function,
            parts: vec![Part::Text("out".to_owned())],
            is_error: true,
        }),
    ]);
    let value: Value = serde_json::from_str(&encode(&request)).unwrap();
    let last = &value["messages"][2]["content"];
    assert_eq!(last[0]["type"], "tool_result");
    assert_eq!(last[0]["is_error"], true);
    assert_eq!(last[1]["type"], "text");
}

#[test]
fn places_cache_breakpoints_on_last_tool_last_system_and_top_level() {
    let mut request = request(vec![text_message(Role::User, "hi")]);
    request.system = vec![Part::Text("a".to_owned()), Part::Text("b".to_owned())];
    let value: Value = serde_json::from_str(&encode(&request)).unwrap();
    assert_eq!(value["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(value["tools"][0].get("cache_control"), None);
    assert_eq!(
        value["tools"][1]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert_eq!(value["system"][0].get("cache_control"), None);
    assert_eq!(
        value["system"][1]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert_eq!(
        value["messages"][0]["content"][0].get("cache_control"),
        None
    );
}

#[test]
fn uses_default_max_output_when_request_has_none() {
    let mut request = request(vec![text_message(Role::User, "hi")]);
    request.max_output_tokens = None;
    let options = EncodeOptions {
        default_max_output: Some(64_000),
        ..EncodeOptions::default()
    };
    let encoded = encode_request(&request, &options).unwrap();
    let value: Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(value["max_tokens"], 64_000);
}

#[test]
fn rejects_without_any_max_tokens() {
    let mut request = request(vec![text_message(Role::User, "hi")]);
    request.max_output_tokens = None;
    assert!(matches!(
        encode_request(&request, &EncodeOptions::default()),
        Err(Rejection::Unsupported(_))
    ));
}

#[test]
fn rejects_unrepresentable_requests() {
    let base = request(vec![text_message(Role::User, "hi")]);
    let cases: Vec<(Request, Rejection)> = vec![
        (
            Request {
                effort: Some(Effort::None),
                ..base.clone()
            },
            Rejection::Effort,
        ),
        (
            Request {
                effort: Some(Effort::Minimal),
                ..base.clone()
            },
            Rejection::Effort,
        ),
        (
            Request {
                items: vec![Item::Reasoning(Reasoning {
                    text: "t".to_owned(),
                    summary: vec!["s".to_owned()],
                    origin: Origin::OpenAi {
                        id: None,
                        encrypted: Some("e".to_owned()),
                    },
                })],
                ..base.clone()
            },
            Rejection::ThinkingSignature,
        ),
        (
            Request {
                items: vec![Item::Reasoning(Reasoning {
                    text: "t".to_owned(),
                    summary: Vec::new(),
                    origin: Origin::Plain,
                })],
                ..base.clone()
            },
            Rejection::ThinkingSignature,
        ),
        (
            Request {
                items: vec![Item::System {
                    parts: vec![Part::Text("s".to_owned())],
                }],
                ..base.clone()
            },
            Rejection::MidConvSystem,
        ),
        (
            Request {
                tools: vec![Tool::Custom {
                    name: "c".to_owned(),
                    description: None,
                    format: None,
                }],
                ..base.clone()
            },
            Rejection::unsupported("custom tool"),
        ),
        (
            Request {
                items: vec![Item::ToolCall(ToolCall {
                    id: "t".to_owned(),
                    name: "n".to_owned(),
                    kind: ToolKind::Function,
                    arguments: "[1]".to_owned(),
                })],
                ..base.clone()
            },
            Rejection::unsupported("arguments of tool call t"),
        ),
        (
            Request {
                items: vec![Item::ToolCall(ToolCall {
                    id: "t".to_owned(),
                    name: "n".to_owned(),
                    kind: ToolKind::Custom,
                    arguments: "{}".to_owned(),
                })],
                ..base.clone()
            },
            Rejection::unsupported("custom tool call"),
        ),
        (
            Request {
                items: vec![Item::Message {
                    role: Role::Assistant,
                    parts: vec![Part::Image(Media::Url("u".to_owned()))],
                }],
                ..base.clone()
            },
            Rejection::ImageBlock,
        ),
        (
            Request {
                system: vec![Part::Image(Media::Url("u".to_owned()))],
                ..base.clone()
            },
            Rejection::ImageBlock,
        ),
        (
            Request {
                tools: Vec::new(),
                tool_choice: Some(ToolChoice::Required),
                ..base.clone()
            },
            Rejection::unsupported("tool_choice without tools"),
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(
            encode_request(&request, &EncodeOptions::default()),
            Err(expected)
        );
    }
}

#[test]
fn force_stream_sets_stream_and_note() {
    let mut request = request(vec![text_message(Role::User, "hi")]);
    request.stream = false;
    let options = EncodeOptions {
        force_stream: true,
        ..EncodeOptions::default()
    };
    let encoded = encode_request(&request, &options).unwrap();
    assert_eq!(encoded.notes, vec![Note::StreamForced]);
    let value: Value = serde_json::from_slice(&encoded.body).unwrap();
    assert_eq!(value["stream"], true);
}

#[test]
fn encode_is_deterministic() {
    let request = request(conversation(3));
    let first = encode_request(&request, &EncodeOptions::default()).unwrap();
    let second = encode_request(&request.clone(), &EncodeOptions::default()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn keeps_tool_schema_and_input_bytes() {
    let mut request = request(vec![
        text_message(Role::User, "hi"),
        Item::ToolCall(ToolCall {
            id: "t1".to_owned(),
            name: "Bash".to_owned(),
            kind: ToolKind::Function,
            arguments: r#"{"z": 1,  "a": [2.50]}"#.to_owned(),
        }),
    ]);
    request.tools = vec![Tool::Function {
        name: "Bash".to_owned(),
        description: None,
        parameters: raw(r#"{"type":"object",  "b":1.0,"a":2}"#),
        strict: None,
    }];
    let body = encode(&request);
    assert!(body.contains(r#""input_schema":{"type":"object",  "b":1.0,"a":2}"#));
    assert!(body.contains(r#""input":{"z": 1,  "a": [2.50]}"#));
}

#[test]
fn n_turn_encoding_is_a_byte_prefix_of_n_plus_one_turns() {
    for turns in 1..4 {
        let short = encode(&request(conversation(turns)));
        let long = encode(&request(conversation(turns + 1)));
        let prefix = short.strip_suffix("]}").unwrap();
        assert!(long.starts_with(prefix), "{turns} turns");
        assert!(long.len() > short.len());
    }
}

#[test]
fn claude_fixture_turn_one_is_a_prefix_of_turn_two() {
    let mut first = decode_request(&claude_retry_body("tool-1")).unwrap();
    let mut second = decode_request(&claude_retry_body("tool-2")).unwrap();
    for request in [&mut first, &mut second] {
        request
            .items
            .retain(|item| !matches!(item, Item::System { .. }));
    }
    let first = encode(&first);
    let second = encode(&second);
    let prefix = first.strip_suffix("]}").unwrap();
    assert!(second.starts_with(prefix));
}

fn sample_response() -> Response {
    Response {
        id: "msg_1".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        output: vec![
            Item::Reasoning(Reasoning {
                text: String::new(),
                summary: vec!["first".to_owned(), "second".to_owned()],
                origin: Origin::OpenAi {
                    id: Some("rs_1".to_owned()),
                    encrypted: Some("enc".to_owned()),
                },
            }),
            Item::Reasoning(Reasoning {
                text: "original".to_owned(),
                summary: vec!["ignored".to_owned()],
                origin: Origin::Anthropic {
                    signature: "sig".to_owned(),
                },
            }),
            Item::Reasoning(Reasoning {
                text: String::new(),
                summary: Vec::new(),
                origin: Origin::AnthropicRedacted {
                    data: "red".to_owned(),
                },
            }),
            text_message(Role::Assistant, "hello"),
            Item::ToolCall(ToolCall {
                id: "toolu_1".to_owned(),
                name: "Bash".to_owned(),
                kind: ToolKind::Function,
                arguments: r#"{"command":"ls"}"#.to_owned(),
            }),
        ],
        stop: StopReason::ToolUse,
        usage: Usage {
            input_uncached: 10,
            cache_read: 20,
            cache_write: 30,
            output: 40,
            reasoning: Some(5),
        },
    }
}

#[test]
fn snapshot_encoded_response() {
    let body = encode_response(&sample_response());
    assert_snapshot!(
        "encoded_response",
        String::from_utf8(body.to_vec()).unwrap()
    );
}

#[test]
fn sealed_reasoning_from_response_opens_on_the_next_request() {
    let body = encode_response(&sample_response());
    let value: Value = serde_json::from_slice(&body).unwrap();
    let signature = value["content"][0]["signature"].as_str().unwrap();
    assert!(signature.starts_with("gw1:"));
    assert_eq!(value["content"][0]["thinking"], "first\n\nsecond");
    assert_eq!(value["content"][1]["thinking"], "original");
    assert_eq!(value["content"][1]["signature"], "sig");
    assert_eq!(value["content"][2]["type"], "redacted_thinking");
    let reasoning = Item::Reasoning(Reasoning {
        text: "first\n\nsecond".to_owned(),
        summary: Vec::new(),
        origin: envelope::open(signature).unwrap(),
    });
    let Item::Reasoning(original) = &sample_response().output[0] else {
        panic!("expected reasoning");
    };
    let Item::Reasoning(opened) = reasoning else {
        panic!("expected reasoning");
    };
    assert_eq!(opened.origin, original.origin);
}

#[test]
fn encode_response_maps_every_stop_reason() {
    let cases = [
        (StopReason::EndTurn, "end_turn"),
        (StopReason::MaxTokens, "max_tokens"),
        (StopReason::ToolUse, "tool_use"),
        (StopReason::StopSequence, "stop_sequence"),
        (StopReason::Refusal, "refusal"),
        (StopReason::ContentFilter, "refusal"),
        (StopReason::ContextWindow, "model_context_window_exceeded"),
    ];
    for (stop, expected) in cases {
        let response = Response {
            stop,
            ..sample_response()
        };
        let value: Value = serde_json::from_slice(&encode_response(&response)).unwrap();
        assert_eq!(value["stop_reason"], expected);
    }
}

#[test]
fn decodes_upstream_response_with_normalized_usage() {
    let body = json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "hm", "signature": "sig"},
            {"type": "text", "text": "hi", "citations": null},
            {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}},
            {"type": "text", "text": "after"}
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {
            "input_tokens": 12,
            "cache_creation_input_tokens": 3,
            "cache_read_input_tokens": 7,
            "output_tokens": 9,
            "service_tier": "standard"
        }
    });
    let response = decode_response(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(response.id, "msg_01");
    assert_eq!(response.stop, StopReason::ToolUse);
    assert_eq!(
        response.usage,
        Usage {
            input_uncached: 12,
            cache_read: 7,
            cache_write: 3,
            output: 9,
            reasoning: None,
        }
    );
    assert_eq!(response.usage.input_total(), 22);
    assert_eq!(
        response.output,
        vec![
            Item::Reasoning(Reasoning {
                text: "hm".to_owned(),
                summary: Vec::new(),
                origin: Origin::Anthropic {
                    signature: "sig".to_owned()
                }
            }),
            text_message(Role::Assistant, "hi"),
            Item::ToolCall(ToolCall {
                id: "toolu_1".to_owned(),
                name: "Bash".to_owned(),
                kind: ToolKind::Function,
                arguments: r#"{"command":"ls"}"#.to_owned()
            }),
            text_message(Role::Assistant, "after"),
        ]
    );
}

#[test]
fn decode_response_errors_on_pause_turn_and_server_blocks() {
    let pause = br#"{"id":"m","model":"x","content":[],"stop_reason":"pause_turn","usage":{}}"#;
    assert!(decode_response(pause).is_err());
    let server = br#"{"id":"m","model":"x","content":[{"type":"server_tool_use"}],"stop_reason":"end_turn","usage":{}}"#;
    assert!(decode_response(server).is_err());
    assert!(decode_response(b"{").is_err());
}

#[test]
fn response_encode_then_decode_keeps_content() {
    let original = Response {
        output: vec![
            Item::Reasoning(Reasoning {
                text: "t".to_owned(),
                summary: Vec::new(),
                origin: Origin::Anthropic {
                    signature: "sig".to_owned(),
                },
            }),
            text_message(Role::Assistant, "hello"),
            Item::ToolCall(ToolCall {
                id: "toolu_1".to_owned(),
                name: "Bash".to_owned(),
                kind: ToolKind::Function,
                arguments: r#"{"command":"ls"}"#.to_owned(),
            }),
        ],
        usage: Usage {
            reasoning: None,
            ..sample_response().usage
        },
        ..sample_response()
    };
    let decoded = decode_response(&encode_response(&original)).unwrap();
    assert_eq!(decoded, original);
}

fn upstream_frames(name: &str) -> Vec<sse::Frame> {
    let bytes = fs::read(fixture_path(&format!("upstream/{name}.sse"))).unwrap();
    sse::Parser::default().push(&bytes)
}

fn decode_stream(name: &str) -> Vec<Event> {
    let mut decoder = StreamDecoder::default();
    upstream_frames(name)
        .iter()
        .flat_map(|frame| decoder.push(frame).unwrap())
        .collect()
}

fn encode_stream(events: &[Event]) -> Vec<Bytes> {
    let mut encoder = StreamEncoder::default();
    let mut out: Vec<Bytes> = events
        .iter()
        .flat_map(|event| encoder.push(event))
        .collect();
    out.extend(encoder.finish());
    out
}

fn frames_to_values(frames: &[Bytes]) -> Vec<Value> {
    let mut parser = sse::Parser::default();
    frames
        .iter()
        .flat_map(|frame| parser.push(frame))
        .map(|frame| {
            let value: Value = serde_json::from_str(&frame.data).unwrap();
            assert_eq!(frame.event.as_deref(), value["type"].as_str());
            value
        })
        .collect()
}

fn assert_valid_message_stream(values: &[Value]) {
    let types: Vec<&str> = values.iter().map(|v| v["type"].as_str().unwrap()).collect();
    assert_eq!(types.first(), Some(&"message_start"));
    assert_eq!(types.last(), Some(&"message_stop"));
    assert_eq!(types[types.len() - 2], "message_delta");
    let mut next = 0;
    let mut open: Option<u64> = None;
    for value in &values[1..values.len() - 2] {
        match value["type"].as_str().unwrap() {
            "content_block_start" => {
                assert_eq!(open, None);
                assert_eq!(value["index"], next);
                open = Some(next);
                next += 1;
            }
            "content_block_delta" => assert_eq!(open, value["index"].as_u64()),
            "content_block_stop" => {
                assert_eq!(open, value["index"].as_u64());
                open = None;
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(open, None);
}

#[test]
fn decodes_text_stream_into_ir_events() {
    let events = decode_stream("anthropic-text");
    assert_eq!(
        events.first(),
        Some(&Event::Start {
            id: "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY".to_owned(),
            model: "claude-opus-5-5".to_owned()
        })
    );
    let usages: Vec<Usage> = events
        .iter()
        .filter_map(|event| {
            if let Event::Usage(usage) = event {
                Some(*usage)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(usages.len(), 2);
    assert_eq!(usages[0].input_uncached, 25);
    assert_eq!(usages[1].input_uncached, 25);
    assert_eq!(usages[1].output, 15);
    assert_eq!(events.last(), Some(&Event::Stop(StopReason::EndTurn)));
}

#[test]
fn decodes_thinking_stream_with_signature_origin_before_block_stop() {
    let events = decode_stream("anthropic-thinking");
    let origin_at = events
        .iter()
        .position(|event| matches!(event, Event::ReasoningOrigin { .. }))
        .unwrap();
    assert!(matches!(
        events[origin_at + 1],
        Event::BlockStop { index: 0 }
    ));
    let Event::ReasoningOrigin { origin, .. } = &events[origin_at] else {
        panic!("expected origin");
    };
    assert!(matches!(origin, Origin::Anthropic { signature } if signature.starts_with("EqQB")));
}

#[test]
fn decodes_tool_use_stream_with_arguments() {
    let events = decode_stream("anthropic-tool_use");
    let arguments: String = events
        .iter()
        .filter_map(|event| {
            if let Event::ToolArgumentsDelta { json, .. } = event {
                Some(json.as_str())
            } else {
                None
            }
        })
        .collect();
    let value: Value = serde_json::from_str(&arguments).unwrap();
    assert!(value.is_object());
    assert!(events.iter().any(|event| matches!(
        event,
        Event::BlockStart {
            block: Block::ToolCall {
                kind: ToolKind::Function,
                ..
            },
            ..
        }
    )));
    assert_eq!(events.last(), Some(&Event::Stop(StopReason::ToolUse)));
}

#[test]
fn decodes_error_stream() {
    let events = decode_stream("anthropic-error");
    assert_eq!(
        events,
        vec![Event::Error(ErrorBody::new(
            529,
            ErrorKind::Overloaded,
            "Overloaded"
        ))]
    );
}

#[test]
fn round_trips_every_upstream_fixture_into_a_valid_stream() {
    for name in ["anthropic-text", "anthropic-thinking", "anthropic-tool_use"] {
        let values = frames_to_values(&encode_stream(&decode_stream(name)));
        assert_valid_message_stream(&values);
    }
}

#[test]
fn message_start_usage_is_zero_and_final_usage_is_in_message_delta() {
    let values = frames_to_values(&encode_stream(&decode_stream("anthropic-tool_use")));
    assert_eq!(
        values[0]["message"]["usage"],
        json!({"input_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 0})
    );
    let delta = &values[values.len() - 2];
    assert_eq!(delta["delta"]["stop_reason"], "tool_use");
    assert_eq!(delta["usage"]["input_tokens"], 472);
    assert_eq!(delta["usage"]["output_tokens"], 89);
}

#[test]
fn snapshot_thinking_stream() {
    let frames = encode_stream(&decode_stream("anthropic-thinking"));
    let text: String = frames
        .iter()
        .map(|frame| String::from_utf8(frame.to_vec()).unwrap())
        .collect();
    assert_snapshot!("stream_thinking", text);
}

#[test]
fn snapshot_tool_use_stream() {
    let frames = encode_stream(&decode_stream("anthropic-tool_use"));
    let text: String = frames
        .iter()
        .map(|frame| String::from_utf8(frame.to_vec()).unwrap())
        .collect();
    assert_snapshot!("stream_tool_use", text);
}

#[test]
fn stream_encoder_seals_foreign_reasoning_and_prefers_original_text() {
    let origin = Origin::OpenAi {
        id: Some("rs_1".to_owned()),
        encrypted: Some("enc".to_owned()),
    };
    let events = vec![
        Event::Start {
            id: "resp_1".to_owned(),
            model: "gpt".to_owned(),
        },
        Event::BlockStart {
            index: 7,
            block: Block::Reasoning,
        },
        Event::ReasoningSummaryDelta {
            index: 7,
            text: "summary".to_owned(),
        },
        Event::ReasoningDelta {
            index: 7,
            text: "ignored".to_owned(),
        },
        Event::ReasoningOrigin {
            index: 7,
            origin: origin.clone(),
        },
        Event::BlockStop { index: 7 },
        Event::BlockStart {
            index: 9,
            block: Block::Text,
        },
        Event::TextDelta {
            index: 9,
            text: "hi".to_owned(),
        },
        Event::Usage(Usage {
            input_uncached: 1,
            cache_read: 2,
            cache_write: 3,
            output: 4,
            reasoning: Some(1),
        }),
        Event::Stop(StopReason::EndTurn),
    ];
    let values = frames_to_values(&encode_stream(&events));
    assert_valid_message_stream(&values);
    assert_eq!(values[1]["content_block"]["type"], "thinking");
    assert_eq!(values[2]["delta"]["thinking"], "summary");
    assert_eq!(values[3]["delta"]["type"], "signature_delta");
    let signature = values[3]["delta"]["signature"].as_str().unwrap();
    assert_eq!(envelope::open(signature), Some(origin));
    assert_eq!(values[5]["index"], 1);
    let delta = &values[values.len() - 2];
    assert_eq!(
        delta["usage"],
        json!({"input_tokens": 1, "cache_creation_input_tokens": 3, "cache_read_input_tokens": 2, "output_tokens": 4})
    );
}

#[test]
fn stream_encoder_emits_redacted_thinking_block() {
    let events = vec![
        Event::Start {
            id: "m".to_owned(),
            model: "x".to_owned(),
        },
        Event::BlockStart {
            index: 0,
            block: Block::Reasoning,
        },
        Event::ReasoningOrigin {
            index: 0,
            origin: Origin::AnthropicRedacted {
                data: "red".to_owned(),
            },
        },
        Event::BlockStop { index: 0 },
        Event::Stop(StopReason::EndTurn),
    ];
    let values = frames_to_values(&encode_stream(&events));
    assert_valid_message_stream(&values);
    assert_eq!(
        values[1]["content_block"],
        json!({"type": "redacted_thinking", "data": "red"})
    );
    assert_eq!(values[2]["type"], "content_block_stop");
}

#[test]
fn stream_encoder_reports_errors_and_stops() {
    let mut encoder = StreamEncoder::default();
    let error = ErrorBody::new(429, ErrorKind::RateLimit, "slow down");
    let frames = encoder.push(&Event::Error(error.clone()));
    assert_eq!(frames, vec![error.sse(proto::Api::Messages)]);
    assert_eq!(
        encoder.push(&Event::Stop(StopReason::EndTurn)),
        [] as [Bytes; 0]
    );
    assert_eq!(encoder.finish(), [] as [Bytes; 0]);
}

#[test]
fn stream_encoder_finish_without_stop_emits_nothing() {
    let mut encoder = StreamEncoder::default();
    encoder.push(&Event::Start {
        id: "m".to_owned(),
        model: "x".to_owned(),
    });
    assert_eq!(encoder.finish(), [] as [Bytes; 0]);
}

#[test]
fn stream_decoder_ignores_ping_and_rejects_unknown_blocks() {
    let mut decoder = StreamDecoder::default();
    let ping_frame = sse::Frame {
        event: Some("ping".to_owned()),
        data: r#"{"type":"ping"}"#.to_owned(),
    };
    assert_eq!(decoder.push(&ping_frame), Ok(Vec::new()));
    let unknown = sse::Frame {
        event: None,
        data: r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"x","name":"y","input":{}}}"#.to_owned(),
    };
    assert!(decoder.push(&unknown).is_err());
    let garbage = sse::Frame {
        event: None,
        data: "nope".to_owned(),
    };
    assert!(decoder.push(&garbage).is_err());
}

#[test]
fn ping_frame_is_an_event_ping() {
    assert_eq!(
        ping(),
        Bytes::from_static(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
    );
}
