use proto::DecodeError;
use proto::collect::Collector;
use proto::error::{ErrorBody, ErrorKind};
use proto::ir::{
    Block, Event, Item, Origin, Part, Reasoning, Role, StopReason, ToolCall, ToolKind, Usage,
};

fn start() -> Event {
    Event::Start {
        id: "resp_1".to_owned(),
        model: "m".to_owned(),
    }
}

fn run(events: &[Event]) -> Result<proto::ir::Response, DecodeError> {
    let mut collector = Collector::default();
    for event in events {
        collector.push(event);
    }
    collector.finish()
}

#[test]
fn folds_reasoning_text_and_tool_call() {
    let usage = Usage {
        input_uncached: 1,
        cache_read: 2,
        cache_write: 3,
        output: 4,
        reasoning: Some(1),
    };
    let response = run(&[
        start(),
        Event::BlockStart {
            index: 0,
            block: Block::Reasoning,
        },
        Event::ReasoningDelta {
            index: 0,
            text: "think".to_owned(),
        },
        Event::ReasoningDelta {
            index: 0,
            text: "ing".to_owned(),
        },
        Event::ReasoningSummaryDelta {
            index: 0,
            text: "sum".to_owned(),
        },
        Event::ReasoningOrigin {
            index: 0,
            origin: Origin::Anthropic {
                signature: "sig".to_owned(),
            },
        },
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
                name: "bash".to_owned(),
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
        Event::Usage(usage),
        Event::Stop(StopReason::ToolUse),
    ])
    .unwrap();
    assert_eq!(response.id, "resp_1");
    assert_eq!(response.model, "m");
    assert_eq!(response.stop, StopReason::ToolUse);
    assert_eq!(response.usage, usage);
    assert_eq!(
        response.output,
        vec![
            Item::Reasoning(Reasoning {
                text: "thinking".to_owned(),
                summary: vec!["sum".to_owned()],
                origin: Origin::Anthropic {
                    signature: "sig".to_owned()
                },
            }),
            Item::Message {
                role: Role::Assistant,
                parts: vec![Part::Text("Hello".to_owned())],
            },
            Item::ToolCall(ToolCall {
                id: "call_1".to_owned(),
                name: "bash".to_owned(),
                kind: ToolKind::Function,
                arguments: "{\"cmd\":\"ls\"}".to_owned(),
            }),
        ]
    );
}

#[test]
fn reasoning_without_origin_is_plain_and_empty_arguments_become_an_object() {
    let response = run(&[
        start(),
        Event::BlockStart {
            index: 0,
            block: Block::Reasoning,
        },
        Event::ReasoningDelta {
            index: 0,
            text: "r".to_owned(),
        },
        Event::BlockStart {
            index: 1,
            block: Block::ToolCall {
                id: "c".to_owned(),
                name: "n".to_owned(),
                kind: ToolKind::Custom,
            },
        },
        Event::Stop(StopReason::EndTurn),
    ])
    .unwrap();
    assert!(matches!(
        response.output.first(),
        Some(Item::Reasoning(Reasoning {
            origin: Origin::Plain,
            ..
        }))
    ));
    assert!(matches!(
        response.output.get(1),
        Some(Item::ToolCall(ToolCall { arguments, .. })) if arguments == "{}"
    ));
}

#[test]
fn adjacent_text_blocks_share_one_message() {
    let response = run(&[
        start(),
        Event::BlockStart {
            index: 0,
            block: Block::Text,
        },
        Event::TextDelta {
            index: 0,
            text: "a".to_owned(),
        },
        Event::BlockStart {
            index: 1,
            block: Block::Text,
        },
        Event::TextDelta {
            index: 1,
            text: "b".to_owned(),
        },
        Event::Stop(StopReason::EndTurn),
    ])
    .unwrap();
    assert_eq!(
        response.output,
        vec![Item::Message {
            role: Role::Assistant,
            parts: vec![Part::Text("a".to_owned()), Part::Text("b".to_owned())],
        }]
    );
}

#[test]
fn error_event_fails_the_collection() {
    let error = run(&[
        start(),
        Event::Error(ErrorBody::new(529, ErrorKind::Overloaded, "Overloaded")),
        Event::Stop(StopReason::EndTurn),
    ])
    .unwrap_err();
    assert_eq!(
        error,
        DecodeError::Event("upstream error 529: Overloaded".to_owned())
    );
}

#[test]
fn rejects_deltas_for_unknown_or_mismatched_blocks() {
    assert!(
        run(&[
            start(),
            Event::TextDelta {
                index: 3,
                text: "x".to_owned()
            }
        ])
        .is_err()
    );
    assert!(
        run(&[
            start(),
            Event::BlockStart {
                index: 0,
                block: Block::Text
            },
            Event::ToolArgumentsDelta {
                index: 0,
                json: "{}".to_owned()
            },
            Event::Stop(StopReason::EndTurn),
        ])
        .is_err()
    );
}

#[test]
fn requires_start_and_stop() {
    assert!(run(&[Event::Stop(StopReason::EndTurn)]).is_err());
    assert!(run(&[start()]).is_err());
}
