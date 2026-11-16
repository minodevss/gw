# proto 테스트 fixture

설계 §11.4–11.5, 부록 A 용. 실제 API에는 요청을 보내지 않았다.

## 클라이언트 요청 캡처 (`claude/`, `codex/`)

- `<시나리오>-<n>.body.json`: 클라이언트가 보낸 body 바이트 (바이트 단위 passthrough 테스트용).
- `<시나리오>-<n>.request.json`: `{method, path, headers, body}`. `body`는 `.body.json`을 파싱한 값이고 둘은 항상 일치한다. headers에서 `authorization`, `x-api-key`, `cookie`, `host`를 뺐고 `content-length`는 정제 후 바이트 수로 맞췄다.
- 시나리오: `plain`(단순 프롬프트 1요청), `tool`(tool 호출 왕복, 요청 2개).
  - claude `tool-1`: Bash tool_use(`echo hi`)로 응답받기 전 요청, `tool-2`: tool_result를 담은 두 번째 요청.
  - codex `tool-1`/`tool-2`: 같은 구조(`exec_command` function_call → function_call_output).
- 만든 방법: `python3 -I`로 띄운 stdlib 가짜 업스트림(`/tmp/gwfix/fake.py`, 127.0.0.1)이 raw body를 기록하고 최소 SSE로 응답. 서버 로그로 로컬 도착을 확인했다.
  - claude 2.1.325: `claude -p`, `env -i`, 빈 `CLAUDE_CONFIG_DIR`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN=dummy`, `HTTPS_PROXY=http://127.0.0.1:9`. tool은 `--allowedTools Bash`.
  - codex 0.162.1: 빈 `CODEX_HOME`, custom provider(`wire_api="responses"`, `env_key`), git 디렉터리에서 `codex exec`. model은 `gpt-5`로 지정(metadata 경고 있음).
- 정제 (body와 headers에 같은 규칙으로 적용, body 바이트가 바뀜):
  - 임시 경로 → `/home/user/project`, `/home/user/.claude`, `/home/user/.codex`, 프로젝트 슬러그 `-home-user-project`
  - codex `installation_id` → `00000000-0000-4000-8000-000000000000`
  - authorization 등 헤더 제거. `/Users/mino`, 토큰은 원래 캡처에 없었다.
- 남은 값: session/turn id(UUID)는 임의 값이라 그대로 뒀다. 모델명 `claude-opus-5-5`는 claude 기본값이다.
- 참고: claude의 `count_tokens` 요청은 `-p` 실행에서 나오지 않아 캡처하지 못했다. claude 요청의 `messages` 안에 `role: "system"` 항목이 들어 있다(mid_conv_system 관련).

## 업스트림 스트림 예시 (`upstream/`)

공식 문서 페이지만 읽었다. SSE 와이어 형식(`event:`/`data:`, 빈 줄 구분).

- `anthropic-text.sse`, `anthropic-tool_use.sse`(input_json_delta), `anthropic-thinking.sse`(thinking_delta, signature_delta), `anthropic-error.sse`: 문서 원문 그대로. thinking 예시의 signature는 문서에서 `...`로 잘려 있고 그대로 뒀다. error 예시는 문서에 한 이벤트만 있다.
  - https://platform.claude.com/docs/en/build-with-claude/streaming
- `openai-responses-{text,function_call,reasoning,failed,error}.sse`: 문서는 이벤트별로 독립된 예시만 주고 완전한 스트림은 없다. 각 이벤트 예시를 순서대로 이어 붙였으므로 id와 `sequence_number`가 서로 맞지 않는다. data JSON은 한 줄로 압축했다(값은 그대로). `function_call`은 문서에 있는 arguments delta/done 두 이벤트뿐이고 function_call 타입의 `output_item.added`/`done` 예시는 문서에 없다.
  - https://developers.openai.com/api/reference/resources/responses/streaming-events
- `chat-text.sse`: 문서의 청크 3개(role 청크, "Hello", `finish_reason: stop`)에 `data: [DONE]`(문서 설명)을 붙임. 문서의 `....`(중간 생략)는 넣지 않았다. tool_calls delta와 최종 usage 청크는 공식 문서에 와이어 예시가 없어 만들지 않았다.
  - https://raw.githubusercontent.com/openai/openai-openapi/manual_spec/openapi.yaml (createChatCompletion 스트리밍 예시)
  - https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events
