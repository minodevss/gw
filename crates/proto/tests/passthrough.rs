#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "helpers outside #[test] fns are not covered by clippy.toml"
)]

use proto::Api;
use proto::features::Feature;
use proto::ir::Usage;
use proto::passthrough::{Peek, Tap, peek, splice_model};
use serde_json::{Value, json};

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture exists")
}

fn peek_view(peek: &Peek) -> Value {
    json!({
        "model": peek.model,
        "stream": peek.stream,
        "cache_key": peek.cache_key,
        "previous_response": peek.previous_response,
        "cache_ttl": peek.cache_ttl,
        "features": peek.features.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>(),
    })
}

const BODIES: [(&str, Api); 2] = [("claude", Api::Messages), ("codex", Api::Responses)];
const SCENARIOS: [&str; 3] = ["plain-1", "tool-1", "tool-2"];

#[test]
fn peek_snapshots_every_fixture() {
    for (dir, api) in BODIES {
        for scenario in SCENARIOS {
            let body = fixture(&format!("{dir}/{scenario}.body.json"));
            let peeked = peek(api, &body).expect("peeks");
            insta::assert_json_snapshot!(format!("peek_{dir}_{scenario}"), peek_view(&peeked));
        }
    }
}

fn splice_span(original: &[u8], spliced: &[u8], old_model: &str, new_model: &str) {
    let old = format!("\"model\":\"{old_model}\"");
    let new = format!("\"model\":\"{new_model}\"");
    let at = original
        .windows(old.len())
        .position(|w| w == old.as_bytes())
        .expect("top-level model present");
    let mut expected = original[..at].to_vec();
    expected.extend_from_slice(new.as_bytes());
    expected.extend_from_slice(&original[at + old.len()..]);
    assert_eq!(spliced, expected.as_slice());
}

#[test]
fn splice_preserves_every_other_byte_on_fixtures() {
    for (dir, api) in BODIES {
        for scenario in SCENARIOS {
            let body = fixture(&format!("{dir}/{scenario}.body.json"));
            let model = peek(api, &body).expect("peeks").model;
            let spliced = splice_model(&body, "other-model-1").expect("splices");
            splice_span(&body, &spliced, &model, "other-model-1");
            assert_eq!(peek(api, &spliced).expect("peeks").model, "other-model-1");
        }
    }
}

#[test]
fn splice_leaves_tool_schema_model_untouched() {
    let body = br#"{"tools":[{"input_schema":{"properties":{"model":{"type":"string"}},"model":"keep"}}],"messages":[{"role":"user","content":"{\"model\":\"text\"}"}], "model" : "gpt-5" ,"stream":true}"#;
    let spliced = splice_model(body, "claude-x").unwrap();
    let expected = String::from_utf8(body.to_vec())
        .unwrap()
        .replace(r#""model" : "gpt-5""#, r#""model" : "claude-x""#);
    assert_eq!(spliced.as_ref(), expected.as_bytes());
}

#[test]
fn splice_escapes_the_new_value() {
    let spliced = splice_model(br#"{"model":"a"}"#, "q\"u\\o\nte").unwrap();
    let value: Value = serde_json::from_slice(&spliced).unwrap();
    assert_eq!(value["model"], "q\"u\\o\nte");
}

#[test]
fn splice_errors_without_top_level_model() {
    assert!(splice_model(br#"{"tools":[{"model":"x"}]}"#, "m").is_err());
}

fn features(api: Api, body: &Value) -> Vec<Feature> {
    peek(api, body.to_string().as_bytes())
        .expect("peeks")
        .features
        .iter()
        .collect()
}

#[test]
fn messages_features_follow_the_ir_meaning() {
    let body = json!({
        "model": "m", "max_tokens": 10, "temperature": 0, "top_p": 1,
        "stop_sequences": ["x"], "metadata": {"user_id": "u"},
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "output_config": {"effort": "high", "format": {"type": "json_schema"}},
        "speed": "fast",
        "tools": [{"name": "t", "strict": true, "input_schema": {}}],
        "messages": [
            {"role": "user", "content": [
                {"type": "image", "source": {}},
                {"type": "tool_result", "content": [{"type": "document", "source": {}}]}
            ]},
            {"role": "system", "content": "mid"}
        ]
    });
    let found = features(Api::Messages, &body);
    for feature in [
        Feature::MaxOutputTokens,
        Feature::Temperature,
        Feature::TopP,
        Feature::Stop,
        Feature::Thinking(proto::features::ThinkingKind::Enabled),
        Feature::Effort(proto::ir::Effort::High),
        Feature::StructuredOutput,
        Feature::Fast,
        Feature::StrictTools,
        Feature::ImageInput,
        Feature::DocumentInput,
        Feature::MidConversationSystem,
    ] {
        assert!(found.contains(&feature), "{feature:?}");
    }
    assert_eq!(found.len(), 12);
}

#[test]
fn messages_cache_ttl_is_found_in_nested_blocks() {
    let ttl = |body: Value| {
        peek(Api::Messages, body.to_string().as_bytes())
            .unwrap()
            .cache_ttl
    };
    let nested = json!({"model": "m", "messages": [{"role": "user", "content": [
        {"type": "tool_result", "content": [{"type": "text", "cache_control": {"type": "ephemeral", "ttl": "1h"}}]}
    ]}]});
    assert_eq!(ttl(nested), Some(3600));
    let plain =
        json!({"model": "m", "system": [{"type": "text", "cache_control": {"type": "ephemeral"}}]});
    assert_eq!(ttl(plain), None);
    let top = json!({"model": "m", "cache_control": {"type": "ephemeral", "ttl": "5m"}});
    assert_eq!(ttl(top), Some(300));
}

#[test]
fn responses_summary_alone_is_not_thinking_and_system_items_split() {
    let body = json!({
        "model": "m", "reasoning": {"summary": "auto"}, "previous_response_id": "r",
        "prompt_cache_key": "k", "prompt_cache_options": {"ttl": "12h"},
        "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "a"}]},
            {"type": "message", "role": "user", "content": [{"type": "input_image", "image_url": "u"}]},
            {"type": "function_call_output", "output": [{"type": "input_file"}]},
            {"type": "message", "role": "developer", "content": "late"}
        ]
    });
    let peeked = peek(Api::Responses, body.to_string().as_bytes()).unwrap();
    assert!(peeked.previous_response);
    assert_eq!(peeked.cache_ttl, Some(43_200));
    assert_eq!(peeked.cache_key.as_deref(), Some("k"));
    let found: Vec<_> = peeked.features.iter().collect();
    assert_eq!(
        found,
        vec![
            Feature::ImageInput,
            Feature::DocumentInput,
            Feature::MidConversationSystem
        ]
    );
}

#[test]
fn responses_leading_developer_items_are_not_mid_conversation() {
    let body = json!({"model": "m", "input": [
        {"role": "developer", "content": "a"}, {"role": "system", "content": "b"},
        {"role": "user", "content": "c"}
    ]});
    assert_eq!(features(Api::Responses, &body), []);
}

#[test]
fn chat_features() {
    let body = json!({
        "model": "m", "prompt_cache_key": "k", "stop": "x", "max_completion_tokens": 5,
        "reasoning_effort": "low", "user": "u",
        "tools": [{"type": "function", "function": {"name": "f", "strict": true}}],
        "messages": [
            {"role": "system", "content": "s"},
            {"role": "user", "content": [{"type": "image_url", "image_url": {}}]},
            {"role": "developer", "content": "late"}
        ]
    });
    let peeked = peek(Api::Chat, body.to_string().as_bytes()).unwrap();
    assert_eq!(peeked.cache_key.as_deref(), Some("k"));
    let found: Vec<_> = peeked.features.iter().collect();
    for feature in [
        Feature::Stop,
        Feature::MaxOutputTokens,
        Feature::Effort(proto::ir::Effort::Low),
        Feature::User,
        Feature::StrictTools,
        Feature::ImageInput,
        Feature::MidConversationSystem,
    ] {
        assert!(found.contains(&feature), "{feature:?}");
    }
    assert_eq!(found.len(), 7);
}

#[test]
fn peek_rejects_invalid_or_modelless_bodies() {
    assert!(peek(Api::Messages, b"{").is_err());
    assert!(peek(Api::Responses, b"{}").is_err());
}

fn tap_stream(api: Api, bytes: &[u8], chunk: usize) -> Tap {
    let mut tap = Tap::new(api, true);
    for part in bytes.chunks(chunk) {
        tap.push(part);
    }
    tap.finish();
    tap
}

fn tap_view(tap: &Tap) -> Value {
    json!({
        "usage": tap.usage().map(|u| json!({
            "input_uncached": u.input_uncached, "cache_read": u.cache_read,
            "cache_write": u.cache_write, "output": u.output, "reasoning": u.reasoning,
        })),
        "error": tap.error(),
    })
}

const STREAMS: [(&str, Api); 10] = [
    ("anthropic-text", Api::Messages),
    ("anthropic-tool_use", Api::Messages),
    ("anthropic-thinking", Api::Messages),
    ("anthropic-error", Api::Messages),
    ("chat-text", Api::Chat),
    ("openai-responses-text", Api::Responses),
    ("openai-responses-function_call", Api::Responses),
    ("openai-responses-reasoning", Api::Responses),
    ("openai-responses-failed", Api::Responses),
    ("openai-responses-error", Api::Responses),
];

#[test]
fn tap_results_do_not_depend_on_chunk_boundaries() {
    for (name, api) in STREAMS {
        let bytes = fixture(&format!("upstream/{name}.sse"));
        let whole = tap_view(&tap_stream(api, &bytes, bytes.len()));
        for chunk in [1, 2, 3, 7, 13, 64, 251] {
            assert_eq!(
                tap_view(&tap_stream(api, &bytes, chunk)),
                whole,
                "{name} {chunk}"
            );
        }
        insta::assert_json_snapshot!(format!("tap_{name}"), whole);
    }
}

#[test]
fn tap_messages_usage_merges_start_and_delta() {
    let bytes = fixture("upstream/anthropic-text.sse");
    let usage = tap_stream(Api::Messages, &bytes, 5).usage().unwrap();
    assert_eq!(
        usage,
        Usage {
            input_uncached: 25,
            output: 15,
            ..Usage::default()
        }
    );
}

#[test]
fn tap_flushes_a_final_frame_without_blank_line() {
    let mut tap = Tap::new(Api::Chat, true);
    tap.push(b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4,\"prompt_tokens_details\":{\"cached_tokens\":6},\"completion_tokens_details\":{\"reasoning_tokens\":3}}}");
    tap.finish();
    assert_eq!(
        tap.usage(),
        Some(Usage {
            input_uncached: 4,
            cache_read: 6,
            cache_write: 0,
            output: 4,
            reasoning: Some(3)
        })
    );
}

#[test]
fn tap_responses_splits_cached_and_written_input() {
    let event = json!({"type": "response.completed", "response": {"usage": {
        "input_tokens": 100, "output_tokens": 7,
        "input_tokens_details": {"cached_tokens": 60, "cache_write_tokens": 30},
        "output_tokens_details": {"reasoning_tokens": 2}
    }}});
    let frame = format!("event: response.completed\ndata: {event}\n\n");
    let usage = tap_stream(Api::Responses, frame.as_bytes(), 9)
        .usage()
        .unwrap();
    assert_eq!(usage.input_uncached, 10);
    assert_eq!(usage.cache_read, 60);
    assert_eq!(usage.cache_write, 30);
    assert_eq!(usage.input_total(), 100);
}

#[test]
fn tap_buffers_non_stream_bodies() {
    let body = br#"{"id":"msg","usage":{"input_tokens":3,"cache_read_input_tokens":4,"cache_creation_input_tokens":5,"output_tokens":6}}"#;
    let mut tap = Tap::new(Api::Messages, false);
    for part in body.chunks(11) {
        tap.push(part);
    }
    assert_eq!(tap.usage(), None);
    tap.finish();
    assert_eq!(
        tap.usage(),
        Some(Usage {
            input_uncached: 3,
            cache_read: 4,
            cache_write: 5,
            output: 6,
            reasoning: None
        })
    );
    assert_eq!(tap.error(), None);
}

#[test]
fn tap_reports_non_stream_error_body() {
    let body = br#"{"error":{"type":"usage_limit_reached","resets_at":1}}"#;
    let mut tap = Tap::new(Api::Responses, false);
    tap.push(body);
    tap.finish();
    assert_eq!(tap.error().unwrap()["error"]["resets_at"], 1);
    assert_eq!(tap.usage(), None);
}
