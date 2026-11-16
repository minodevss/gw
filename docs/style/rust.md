# Rust 코드 스타일

## 원칙

- 코드가 설명한다. 이름과 타입으로 말하고, 주석은 마지막 수단이다.
- 필요 없는 것은 쓰지 않는다: 추상화, 의존성, 옵션, 주석, 매크로.
- 규칙은 도구가 강제한다(rustfmt, Clippy, CI). 도구가 못 잡는 것만 리뷰에서 본다.
- 불가능한 상태는 타입으로 표현 불가능하게 만든다. enum과 exhaustive `match`로 컴파일러가 누락을 잡게 한다.
- 공식 가이드(Rust Style Guide, API Guidelines)가 기본값이다. 어긋나는 규칙에는 이유를 적는다.

## 포맷

- rustfmt 기본값을 쓴다. `rustfmt.toml`은 만들지 않는다. edition은 Cargo.toml에서 읽히고 style edition이 따라간다.
  - `group_imports`, `imports_granularity`는 nightly 전용이라 쓰지 않는다.
- 4칸 공백, 최대 100열, trailing comma는 rustfmt 기본(Style Guide)이다. 손으로 정렬하지 않는다.
- CI: `cargo fmt --all --check`.
- Edition 2024, `rust-version`을 명시한다. resolver 3(MSRV 인지)이 기본이다.

```toml
[workspace.package]
edition = "2024"
rust-version = "1.98"
```

- edition 2024에서 let chain을 쓸 수 있다(1.88부터). 중첩 `if let`보다 let chain을 쓴다.

```rust
if let Some(a) = account && a.state.is_usable() { ... }
```

## 이름

API Guidelines C-CASE, C-CONV, C-GETTER, C-ITER, C-WORD-ORDER를 따른다.

- 타입, trait, enum variant는 `UpperCamelCase`. 함수, 메서드, 모듈, 변수는 `snake_case`. 상수는 `SCREAMING_SNAKE_CASE`.
- 약어는 한 단어로 쓴다: `OpenAi`, `Sse`, `Uuid`. `OpenAI`, `SSE` 아님.
- 변환 접두사: `as_`(공짜, 빌림→빌림), `to_`(비용 있음), `into_`(소유권 소비). `from_`, `try_from`은 `From`/`TryFrom`으로 구현한다.
- getter에 `get_`을 붙이지 않는다: `name()`. 예외는 키로 조회하는 `get(key)`뿐이다.
- iterator는 `iter`, `iter_mut`, `into_iter`. 반환 타입 이름은 메서드와 맞춘다.
- bool 반환은 `is_`, `has_`, `can_`. bool 파라미터는 enum으로 바꾼다(C-CUSTOM-TYPE).
- 생성자는 `new`, 변형은 `with_*`. 인자가 많으면 struct 리터럴을 쓰고 builder는 만들지 않는다.
- 단일 문자 이름은 `i`, `T`, `'a`와 짧은 클로저 인자만 허용한다.
- 모듈 이름에 `util`, `common`, `helpers`, `misc`를 쓰지 않는다. 하는 일로 이름 짓는다.
- 타입 이름에 `Manager`, `Handler`, `Helper`, `Impl`을 붙이지 않는다.
- 모듈 경로가 이미 말해 주는 접두사는 반복하지 않는다: `messages::Request`이지 `messages::MessagesRequest`가 아니다.
- 식별자와 주석은 영어로 쓴다(grep 가능성). 사용자에게 보이는 문구는 별도로 정한다.

## 주석과 문서

기본값은 주석 없음이다. 쓰기 전에 이름이나 타입을 바꿔서 해결되는지 먼저 본다.

- 무엇을 하는지 설명하는 주석을 쓰지 않는다. 이름이 그 일을 해야 한다.
- 허용 1: **왜** 그런지, 코드만 봐서는 알 수 없는 외부 사실. 프로토콜 동작, 업체 버그, workaround. 출처 URL을 반드시 단다.
- 허용 2: `unsafe` 블록의 `// SAFETY:` 주석. 단, 이 프로젝트는 `unsafe_code = "forbid"`라서 원칙적으로 쓸 일이 없다.
- `///`, `//!`는 쓰지 않는다. 이 workspace는 배포하지 않는 애플리케이션이라서 `missing_docs`를 켜지 않는다. API Guidelines C-CRATE-DOC, C-EXAMPLE과 의도적으로 다른 부분이다.
  - 예외: proto의 공개 진입점이 이름만으로 계약이 안 드러날 때 한두 줄의 `///`.
- 주석 처리된 코드, `TODO`, `FIXME`, 구역 구분선(`// ---- helpers ----`), 변경 이력, 작성자 표시는 금지다. 할 일은 이슈로 둔다.
- lint를 끌 때는 `#[allow]`가 아니라 `#[expect(lint, reason = "...")]`을 쓴다. 더 이상 필요 없으면 컴파일러가 알려 준다.

```rust
// bad
// 계정이 사용 가능한지 확인한다
fn check(a: &Account) -> bool { ... }

// good
fn is_usable(&self) -> bool { ... }

// good: 왜
// Anthropic은 빈 text block을 400으로 거절한다.
// https://docs.anthropic.com/en/api/messages
parts.retain(|p| !p.is_empty_text());
```

## 모듈과 파일 구조

- `foo.rs` + `foo/` 방식을 쓴다. `mod.rs`는 만들지 않는다. Rust Book ch07-05와 Reference가 둘 다 허용하되 `foo.rs`를 현재 방식으로 소개하고, 에디터에서 `mod.rs`가 여러 개 열리면 구분이 안 된다. `clippy::mod_module_files`가 강제한다.
- 파일 하나는 개념 하나다. 길이로 쪼개지 않고 책임으로 쪼갠다.
- 기본 가시성은 private, 다음이 `pub(crate)`. `pub`은 crate 경계(proto의 외부 API)에만 쓴다. `unreachable_pub`가 잡아 준다.
- `lib.rs`와 `main.rs`는 `mod` 선언과 재export만 담는다.
- `use`는 파일 위에 모은다. 함수 안 `use`는 enum variant 전개(`use Provider::*`)처럼 범위가 좁을 때만 쓴다. glob import는 `use super::*`(tests)와 prelude에만 허용한다.
- 순환 의존이 생기면 구조가 틀린 것이다. proto는 gw를 모르고, gw의 `ops`는 `tui`, `main`을 모른다.
- proto는 IO와 async가 없다. `tokio`, `reqwest`, `rusqlite`, `tracing`, 파일, 시계, 난수를 의존성에 넣지 않는다. 시각이나 id가 필요하면 인자로 받는다.

## 타입과 에러

- 도메인 값은 newtype으로 감싼다: `AccountName`, `KeyHash`, `SessionKey`. `String` 두 개가 섞이는 버그를 컴파일러가 막는다(C-NEWTYPE).
- 상태와 종류는 enum이다. 여러 bool과 `Option` 조합 대신 variant에 데이터를 싣는다.
- 자기 enum을 `match`할 때 `_ =>`를 쓰지 않는다. variant가 추가되면 컴파일 에러가 나야 한다(design.md §11.4의 Provider 설계가 이것에 의존한다). `wildcard_enum_match_arm`이 강제한다.
- provider별 분기는 trait object가 아니라 `enum` + `match self`다. `async-trait`, `dyn`을 쓰지 않는다.
- 비밀값(`Secret`, token, key)은 `Debug`를 직접 구현해 값을 가리고, `Display`와 `Serialize`는 구현하지 않는다. 저장할 때만 명시적으로 꺼낸다.
- 공개 타입은 `Debug`를 구현한다(C-DEBUG). 기타 derive는 필요한 것만 한다.
- `&String`, `&Vec<T>` 대신 `&str`, `&[T]`. 소유가 필요할 때만 `String`, `Vec`.
- 바이트는 `bytes::Bytes`로 넘긴다. 복사하지 않는다.
- `clone()`을 borrow checker를 피하는 데 쓰지 않는다. 소유권 구조를 바꾼다.
- 조기 반환과 `let ... else`로 중첩을 줄인다. `?`로 전파한다(C-QUESTION-MARK).

**에러**
- 에러 전략은 crate마다 다르다. Rust Book ch09와 thiserror, anyhow 문서가 나눈 기준이다. 호출자가 variant로 분기해야 하면 타입 있는 에러, 보고만 하면 anyhow다.
- proto: `thiserror` enum. decode의 reject 사유, 변환 불가 필드는 호출자가 HTTP 에러 형식으로 바꾸므로 variant로 구분한다.
- gw의 요청 처리 경로(select, forward, account 상태 전이)도 `thiserror` enum이다. reason과 HTTP status로 매핑해야 한다.
- gw의 바깥 가장자리(`main`, CLI `ops`, TUI 작업 결과)는 `anyhow::Result`와 `.context("...")`다.
- crate마다 `type Result<T> = std::result::Result<T, Error>`를 둔다.
- 에러 메시지는 소문자로 시작하고 마침표가 없다. 하위 에러는 `#[source]`/`#[from]`으로 잇고 메시지에 중복해서 붙이지 않는다(C-GOOD-ERR).
- 에러 타입은 `Send + Sync + 'static`이다(C-GOOD-ERR).
- `Box<dyn Error>`, `Result<_, String>`을 쓰지 않는다.
- 비-테스트 코드에서 `unwrap`, `expect`, `panic!`, 인덱싱 `v[i]`를 쓰지 않는다. 불변식이 증명되는 드문 경우는 `#[expect(clippy::..., reason = "...")]`로 이유를 남긴다.
- 에러를 삼키지 않는다. 의도적으로 버리는 `Result`는 `let _ =`로 쓰되, 왜 버려도 되는지가 코드에서 드러나야 한다(예: 종료 중인 로그 채널로의 전송).

## async와 IO 경계

- async 안에서 blocking 호출을 하지 않는다(Alice Ryhl, "What is blocking"). 한 await 사이에 수십 µs 넘는 일은 다른 task를 굶긴다.
- rusqlite는 동기 API다. `tokio::task::spawn_blocking`에서 실행하거나 전용 스레드에서 쓴다. design.md §11.2대로 로그 기록은 채널 하나를 받는 단일 writer가 맡는다.
- `std::sync::Mutex` 가드를 `.await` 너머로 들고 있지 않는다(tokio tutorial). 락 범위는 짧은 동기 구간으로 제한하고 `await_holding_lock`(Clippy 기본 경고)을 그대로 둔다.
- tokio `Mutex`는 락을 await 너머로 유지해야 할 때만 쓴다. 토큰 갱신 잠금이 이 경우다.
- channel은 bounded가 기본이다. unbounded는 backpressure가 필요 없다는 근거를 댈 수 있을 때만 쓴다.
- `tokio::select!`의 각 분기는 cancel-safe여야 한다. 아니면 분기 밖에서 future를 pin해서 재사용한다.
- task는 `JoinSet`이나 handle로 소유한다. 결과를 버리는 fire-and-forget `spawn`을 쓰지 않는다.
- 클라이언트가 연결을 끊으면 업스트림 요청도 drop으로 취소된다. 이 경로에서 `spawn`으로 떼어내지 않는다(design.md §11.5).
- 스트림은 청크 단위로 중계하고 전체를 모으지 않는다. 버퍼링은 테스트로 막는다.
- `println!`, `eprintln!`은 TUI 화면을 깬다. 출력은 `tracing`이나 명시적 writer로 한다.
- 로그는 `tracing`이다(`log` 크레이트 아님). `tracing-subscriber`는 gw 바이너리에서만 초기화한다.
  - 메시지는 고정 문구, 가변 값은 필드다: `warn!(account = %name, reason = ?r, "limit hit")`.
  - 함수 `#[instrument]`는 `skip_all`과 필요한 필드만 쓴다. 인자 전체를 기록하면 secret이 샌다.
  - 요청과 응답 본문, 토큰, key는 어떤 레벨에서도 기록하지 않는다.
  - `gw tui`가 켜져 있을 때 로그는 stderr가 아니라 파일로 간다.

## serde와 wire 타입

- wire 타입은 `types.rs`에만 둔다. IR과 같은 타입을 겸하지 않는다.
- 필드 이름은 Rust 관례로 쓰고 `#[serde(rename_all = "snake_case")]`를 컨테이너에 한 번 건다. 필드마다 `rename`을 붙이지 않는다. 이름이 다른 필드만 예외다(serde container attrs).
- tagged union은 `#[serde(tag = "type")]`이다. wire 이벤트가 `type` 필드를 쓰기 때문이다.
- 선택 필드는 `Option<T>` + `#[serde(skip_serializing_if = "Option::is_none")]`. 빈 `Vec`도 `Vec::is_empty`로 생략한다. `default`는 필요할 때만 건다.
- **모르는 필드:** 이 프로젝트는 `#[serde(flatten)] extra`로 받아서 decode에서 비어 있지 않으면 reject한다(design.md §11.4). `deny_unknown_fields`는 `flatten`과 같이 쓸 수 없다(serde 문서). 둘 중 하나만 쓴다.
- **바이트 보존:** tool schema, tool input 같은 임의 JSON은 `Box<serde_json::value::RawValue>`다(`raw_value` feature). `Value`는 키 순서와 숫자 표현을 바꿀 수 있다.
  - `RawValue`는 serde가 내용을 버퍼링하는 경로(`flatten`, `untagged`, 내부 tag enum)에서 깨질 수 있다. `RawValue` 필드가 있는 타입은 round-trip 테스트를 둔다.
- 직렬화 순서는 struct 필드 순서다. 필드 순서를 바꾸는 것은 캐시 안정성에 영향을 주므로 snapshot이 깨지는 변경이다.
- 순회가 직렬화에 영향을 주는 곳에 `HashMap`을 쓰지 않는다. 순서가 필요하면 `Vec`이나 `BTreeMap`.
- passthrough 경로는 serde를 거치지 않고 바이트를 직접 다룬다. `peek`는 필요한 필드만 가진 최소 struct로 읽는다.
- `#[serde(untagged)]`를 피한다. 에러 메시지가 쓸모없고 느리다. 판별 필드가 있으면 tag를 쓴다.
- 입력은 신뢰하지 않는다. 크기와 깊이 제한은 decode 앞단에서 건다.

## 테스트

- 단위 테스트는 같은 파일 아래쪽 `#[cfg(test)] mod tests { use super::*; ... }`에 둔다(Rust Book ch11-03).
- 통합 테스트는 crate의 `tests/`에 둔다. 공개 API(proto) 또는 프로세스 경계(gw의 가짜 업스트림)만 쓴다.
- 이름은 동작을 말한다: `rejects_unknown_field`, `splice_leaves_tool_schema_model_untouched`. `test_` 접두사를 붙이지 않는다.
- 테스트 하나는 동작 하나를 검증한다.
- 변환 결과는 `insta` snapshot으로 고정한다. `.snap`은 커밋하고 리뷰한다(`cargo insta review`). 손으로 쓴 거대한 expected 문자열을 만들지 않는다.
  - 사람이 읽는 텍스트나 JSON은 `assert_snapshot!`, 구조체는 `assert_json_snapshot!`을 쓴다.
- fixture는 정제한 실제 캡처이며 `tests/fixtures/`에 둔다. 프롬프트와 토큰은 커밋 전에 지운다.
- 테스트에서는 `unwrap`, `expect`를 허용한다(clippy.toml). 단 `unwrap` 대신 `expect("이유")`가 읽기 쉬우면 그걸 쓴다.
- async 테스트는 `#[tokio::test]`. 시간은 `start_paused = true`로 가짜 시계를 쓰고 `sleep`으로 기다리지 않는다.
- 외부 네트워크에 의존하지 않는다. 업스트림은 로컬 가짜 서버다.
- 속성이 있는 로직(변환의 round-trip, 순서)은 예시 몇 개보다 `proptest`가 낫지만, 의존성을 늘릴 만큼 가치가 있을 때만 추가한다.
- doctest는 쓰지 않는다(`///` 예제가 없다).

## Clippy 설정

근거:
- 기본 그룹(`correctness`, `suspicious`, `style`, `complexity`, `perf`)은 이미 기본 경고/거부다. 그대로 둔다.
- `pedantic`은 오탐이 있어도 대부분 실제 개선이다. 통째로 켜고 소음이 큰 것만 끈다.
- `restriction`은 그룹째로 켜지 않는다(Clippy 문서가 경고한다). 필요한 개별 lint만 고른다.
- `nursery`, `cargo`는 켜지 않는다. 불안정하거나 이 프로젝트에 효용이 낮다.
- `unsafe_code = "forbid"`: 의존성이 unsafe를 품고 있어도 우리 코드는 safe다.

```toml
# Cargo.toml (workspace root)
[workspace.lints.rust]
unsafe_code = "forbid"
unreachable_pub = "warn"
unused_qualifications = "warn"
unused_must_use = "deny"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
# pedantic에서 끄는 것
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
too_many_lines = "allow"
similar_names = "allow"
# restriction에서 고른 것
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
indexing_slicing = "deny"
todo = "deny"
unimplemented = "deny"
dbg_macro = "deny"
print_stdout = "deny"
print_stderr = "deny"
mod_module_files = "deny"
wildcard_enum_match_arm = "warn"
allow_attributes = "warn"
allow_attributes_without_reason = "warn"
undocumented_unsafe_blocks = "deny"
string_slice = "warn"
let_underscore_must_use = "warn"
```

```toml
# 각 crate의 Cargo.toml
[lints]
workspace = true
```

```toml
# clippy.toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-indexing-slicing-in-tests = true
allow-print-in-tests = true
```

- CI: `cargo clippy --workspace --all-targets -- -D warnings`. 경고는 병합 전에 0이어야 한다.
- `main.rs`에서 CLI가 사용자에게 출력하는 부분은 별도의 `Output` 쓰기 함수 한 곳으로 모으고 거기에만 `#[expect(clippy::print_stdout, reason = "CLI 출력")]`을 둔다.
- lint를 끄는 것보다 코드를 고친다. `#[expect]`는 줄 단위로, reason을 쓴다.
- proto에는 추가로 `clippy.toml`의 `disallowed-methods`/`disallowed-types`로 `std::time::SystemTime::now`, `std::fs::*`, `std::thread::sleep`을 막을 수 있다.

## 금지 목록

- 설명 주석, `TODO`, 주석 처리된 코드, `#[allow]`(`#[expect]`만).
- 비-테스트 코드의 `unwrap`, `expect`, `panic!`, `v[i]`, `println!`, `dbg!`.
- `unsafe`.
- `mod.rs`.
- `util`, `common`, `helpers` 모듈. `Manager`, `Handler` 접미사.
- `Box<dyn Error>`, `Result<_, String>`.
- `async-trait`, 불필요한 `dyn`. provider와 API 분기는 enum.
- 자기 enum에 대한 `_ =>` 분기.
- async 안의 blocking 호출, `.await` 너머 `std::sync::Mutex` 가드.
- secret의 `Debug`, `Display`, `Serialize`, 로그 출력. 본문 로그.
- `untagged`, 순회하는 `HashMap`으로 만든 wire 직렬화.
- proto에서 IO, async, 시계, 난수, `tracing`.
- 쓰이지 않는 의존성, feature, 추상화, 설정 옵션.

## 출처

- Rust Style Guide: https://doc.rust-lang.org/style-guide/
- Rust API Guidelines (C-CASE, C-CONV, C-GETTER, C-ITER, C-GOOD-ERR, C-QUESTION-MARK, C-NEWTYPE, C-CUSTOM-TYPE, C-DEBUG, C-CRATE-DOC): https://rust-lang.github.io/api-guidelines/checklist.html
- rustfmt 설정(기본값, nightly 전용 옵션): https://github.com/rust-lang/rustfmt/blob/master/Configurations.md
- Clippy lint 그룹과 설정: https://doc.rust-lang.org/clippy/lints.html, https://doc.rust-lang.org/clippy/configuration.html, https://rust-lang.github.io/rust-clippy/master/
- Cargo `[lints]`와 workspace lints: https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
- rustc lint 그룹: https://doc.rust-lang.org/rustc/lints/groups.html
- Edition 2024 가이드: https://doc.rust-lang.org/edition-guide/rust-2024/
- 모듈 파일 배치: https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html, https://doc.rust-lang.org/reference/items/modules.html
- 에러 처리: https://doc.rust-lang.org/book/ch09-00-error-handling.html, https://docs.rs/thiserror, https://docs.rs/anyhow
- tokio: https://tokio.rs/tokio/tutorial/shared-state, https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html, https://ryhl.io/blog/async-what-is-blocking/
- serde: https://serde.rs/container-attrs.html, https://serde.rs/field-attrs.html, https://docs.rs/serde_json/latest/serde_json/value/struct.RawValue.html
- 테스트: https://doc.rust-lang.org/book/ch11-03-test-organization.html, https://insta.rs/docs/
- 로깅: https://docs.rs/tracing
- 프로젝트 설계: docs/design.md §11
