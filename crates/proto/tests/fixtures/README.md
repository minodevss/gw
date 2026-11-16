# proto test fixtures

No request in this directory was sent to a real API.

## Client request captures (`claude/`, `codex/`)

- `<scenario>-<n>.body.json`: the exact body bytes the client sent, for byte-exact passthrough tests.
- `<scenario>-<n>.request.json`: `{method, path, headers, body}`. `body` is the parsed `.body.json`; the two always match. `authorization`, `x-api-key`, `cookie` and `host` are removed and `content-length` matches the sanitized body.
- Scenarios: `plain` (one request), `tool` (a tool round trip, two requests).
  - claude `tool-1` is the request before a Bash `tool_use` (`echo hi`); `tool-2` carries the `tool_result`.
  - codex `tool-1`/`tool-2` have the same shape (`exec_command` function_call, then function_call_output).
- How they were made: a stdlib Python fake upstream on 127.0.0.1 recorded raw bodies and answered with minimal SSE.
  - Claude Code 2.1.325: `claude -p` under `env -i`, an empty `CLAUDE_CONFIG_DIR`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN=dummy`, `HTTPS_PROXY=http://127.0.0.1:9`, `--allowedTools Bash`.
  - Codex 0.162.1: an empty `CODEX_HOME`, a custom provider (`wire_api = "responses"`, `env_key`), `codex exec` in a git directory, model `gpt-5`.
- Sanitization (applied to bodies and headers alike, so body bytes differ from the raw capture):
  - temporary paths became `/home/user/project`, `/home/user/.claude`, `/home/user/.codex`, project slug `-home-user-project`
  - codex `installation_id` became `00000000-0000-4000-8000-000000000000`
- Session and turn ids are random UUIDs and were kept. `claude-opus-5-5` is Claude Code's default model.
- Claude Code did not send `count_tokens` in `-p` mode, so it is not captured. Claude Code requests contain `role: "system"` entries inside `messages`.

## Upstream stream examples (`upstream/`)

SSE wire format (`event:` / `data:` lines separated by blank lines).

- `anthropic-{text,tool_use,thinking,error}.sse`: copied from the Anthropic streaming docs. The thinking signature is truncated with `...` in the docs and kept that way.
  - https://platform.claude.com/docs/en/build-with-claude/streaming
- `openai-responses-{text,function_call,reasoning,failed,error}.sse`: the docs only give one example per event, so the examples are chained in order; ids and `sequence_number` do not line up. `function_call` has only the arguments delta and done events.
  - https://developers.openai.com/api/reference/resources/responses/streaming-events
- `chat-text.sse`: the three chunks from the OpenAPI spec plus `data: [DONE]`.
  - https://raw.githubusercontent.com/openai/openai-openapi/manual_spec/openapi.yaml
- `chat-{tool_call,usage,reasoning}.sse`: hand-built from the OpenAPI stream schemas (`ChatCompletionStreamResponseDelta`, `CreateChatCompletionStreamResponse`). `chat-reasoning.sse` follows the `reasoning_content` delta and usage shapes in the Moonshot chat API reference.
  - https://platform.kimi.ai/docs/api/chat
