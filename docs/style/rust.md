# Rust style

## Principles

- Code explains itself. Names and types carry the meaning; comments are the last resort.
- Write nothing that is not needed: abstractions, dependencies, options, comments, macros.
- Tools enforce the rules (rustfmt, Clippy, CI). Review covers only what tools cannot catch.
- Make invalid states unrepresentable. Use enums and exhaustive `match` so the compiler catches omissions.
- The official guides (Rust Style Guide, API Guidelines) are the default. Rules that differ say why.

## Formatting

- rustfmt defaults. No `rustfmt.toml`; the style edition follows the crate edition.
  - `group_imports` and `imports_granularity` are nightly-only, so they are not used.
- Four-space indent, 100 columns, trailing commas as rustfmt does them. No manual alignment.
- CI: `cargo fmt --all --check`.
- Edition 2024 with an explicit `rust-version`. Resolver 3 (MSRV-aware) is the default.

```toml
[workspace.package]
edition = "2024"
rust-version = "1.98"
```

- Prefer let chains (edition 2024, Rust 1.88+) over nested `if let`.

```rust
if let Some(a) = account && a.is_usable_for(model, now) { ... }
```

## Naming

Follow API Guidelines C-CASE, C-CONV, C-GETTER, C-ITER, C-WORD-ORDER.

- `UpperCamelCase` for types, traits and variants; `snake_case` for functions, methods, modules and variables; `SCREAMING_SNAKE_CASE` for constants.
- Acronyms are one word: `OpenAi`, `Sse`, `Uuid`.
- Conversions: `as_` (free, borrow to borrow), `to_` (costly), `into_` (consumes). Implement `From`/`TryFrom` instead of `from_` functions.
- Getters have no `get_` prefix: `name()`. The exception is keyed lookup, `get(key)`.
- Iterators: `iter`, `iter_mut`, `into_iter`.
- Boolean queries start with `is_`, `has_` or `can_`. Replace boolean parameters with enums (C-CUSTOM-TYPE).
- Constructors are `new`; variants are `with_*`. Use struct literals instead of builders.
- Single-letter names only for `i`, `T`, `'a` and short closure arguments.
- No `util`, `common`, `helpers` or `misc` modules. Name modules by what they do.
- No `Manager`, `Handler`, `Helper` or `Impl` suffixes.
- Do not repeat what the module path already says: `messages::Request`, not `messages::MessagesRequest`.
- Identifiers, comments and documentation are in English.

## Comments and documentation

The default is no comment. Before writing one, check whether a better name or type removes the need.

- Never describe what the code does. The name should.
- Allowed: why, when the reason is an external fact the code cannot show (protocol behaviour, vendor quirk, workaround). Link the source.
- Allowed: `// SAFETY:` on `unsafe` blocks. The workspace forbids `unsafe_code`, so this should not occur.
- No `///` or `//!`. This is an application, not a published library, so `missing_docs` is off. This deliberately differs from API Guidelines C-CRATE-DOC and C-EXAMPLE.
- No commented-out code, `TODO`, `FIXME`, section banners, change logs or author notes.
- Silence lints with `#[expect(lint, reason = "...")]`, never `#[allow]`. The compiler reports expectations that are no longer needed.

```rust
// bad
// checks whether the account can be used
fn check(a: &Account) -> bool { ... }

// good
fn is_usable_for(&self, model: &str, now: i64) -> bool { ... }

// good: why, with a source
// Anthropic rejects empty text blocks with a 400.
// https://platform.claude.com/docs/en/api/messages
parts.retain(|part| !part.is_empty_text());
```

## Modules and files

- Use `foo.rs` plus `foo/`, never `mod.rs`. The Rust Book calls `foo.rs` the most idiomatic layout and `mod.rs` the older style; many `mod.rs` files are hard to tell apart in an editor. `clippy::mod_module_files` enforces this.
- One concept per file. Split by responsibility, not by length.
- Visibility is private by default. In the `opengateway` binary crate use `pub(crate)`; plain `pub` is only for the `proto` crate's public API. `unreachable_pub` enforces this.
- `lib.rs` and `main.rs` hold module declarations, re-exports and the entry point only.
- Imports go at the top of the file. Glob imports only for `use super::*` in tests.
- A dependency cycle means the structure is wrong. `proto` does not know `opengateway`; `ops` does not know `tui` or `main`.
- `proto` has no IO and no async: no `tokio`, `reqwest`, `rusqlite`, `tracing`, files, clock or randomness. Pass times and ids in as arguments.

## Types and errors

- Wrap domain values in types when two strings could be confused (C-NEWTYPE).
- State and kind are enums. Put data in variants instead of combining several booleans and options.
- Never match your own enums with `_ =>`. Adding a variant must break the build. `wildcard_enum_match_arm` enforces this; provider and API dispatch depend on it.
- Dispatch over providers and APIs with `enum` + `match self`, not trait objects. No `async-trait`, no unnecessary `dyn`.
- Secrets (tokens, keys) implement `Debug` by hand to hide values and do not implement `Display` or `Serialize`. Storage converts them explicitly through a private type.
- Public types implement `Debug` (C-DEBUG). Derive only what is used.
- Take `&str` and `&[T]`, not `&String` and `&Vec<T>`.
- Pass bytes as `bytes::Bytes` without copying.
- Do not `clone()` to dodge the borrow checker; change the ownership structure.
- Return early and use `let ... else` to keep nesting shallow. Propagate with `?` (C-QUESTION-MARK).

**Errors**
- Callers that branch on the error get a typed `thiserror` enum. Callers that only report get `anyhow`.
- `proto`: `thiserror` enums. Rejections carry the reason the gateway turns into an HTTP error.
- `opengateway` request path (selection, forwarding, account state): typed errors mapped to HTTP status and reason.
- `opengateway` edges (`main`, CLI, `ops`, TUI jobs): `anyhow::Result` with `.context("...")`.
- Error messages start lowercase and have no trailing period. Chain causes with `#[source]`/`#[from]` (C-GOOD-ERR).
- Error types are `Send + Sync + 'static`.
- No `Box<dyn Error>` or `Result<_, String>` in APIs.
- No `unwrap`, `expect`, `panic!` or indexing `v[i]` outside tests. When an invariant truly holds, use `#[expect(clippy::..., reason = "...")]`.
- Do not swallow errors. A discarded `Result` must show in the code why discarding is correct.

## Async and IO

- Never block inside async code (Alice Ryhl, "What is blocking?").
- rusqlite is synchronous. Run it in `tokio::task::spawn_blocking` or on a dedicated thread. The server sends all writes through one channel to a single writer thread.
- Never hold a `std::sync::Mutex` guard across `.await` (tokio tutorial). Keep `await_holding_lock` on.
- Use a tokio `Mutex` only when the lock must span an await.
- Channels are bounded unless there is a stated reason for backpressure not to matter.
- Every branch of `tokio::select!` must be cancel-safe.
- Own spawned tasks through a `JoinHandle` or `JoinSet`; no fire-and-forget `spawn`.
- Dropping the client connection must cancel the upstream request. Do not detach that path with `spawn`.
- Relay streams chunk by chunk; never collect a whole stream. Tests guard against buffering.
- No `println!` or `eprintln!` (they break the TUI). Use `tracing` or an explicit writer.
- Logging uses `tracing`:
  - fixed messages, variable values as fields: `warn!(account = %name, "limit hit")`
  - `#[instrument(skip_all, fields(...))]` only; recording all arguments leaks secrets
  - never log request or response bodies, tokens or keys
  - while the TUI runs, logs go to `tui.log` in the data directory, not stderr

## serde and wire types

- Wire types live in `types.rs` and are never reused as IR.
- Put `#[serde(rename_all = "snake_case")]` on the container; rename individual fields only when they differ.
- Tagged unions use `#[serde(tag = "type")]`, matching the wire.
- Optional fields are `Option<T>` with `skip_serializing_if = "Option::is_none"`; empty vectors use `Vec::is_empty`.
- **Unknown fields are rejected.** `deny_unknown_fields` cannot be combined with `flatten`, and `flatten` buffers input in a way that breaks `RawValue`. Decode the top level into a map of raw values and reject leftovers, and use `deny_unknown_fields` on nested structs.
- **Byte preservation:** arbitrary JSON (tool schemas, tool inputs) is kept as raw JSON (`serde_json` `raw_value`). `Value` can change key order and number formatting. Types holding raw JSON get a round-trip test.
- Serialization order is struct field order. Reordering fields changes cache prefixes and is a snapshot-breaking change.
- No iteration over `HashMap` where it affects serialized output; use `Vec` or `BTreeMap`.
- The passthrough path never re-serializes. `peek` reads only the fields it needs.
- Avoid `#[serde(untagged)]`; its errors are useless.
- Treat input as untrusted; bound sizes before decoding.

## Tests

- Unit tests sit at the bottom of the file in `#[cfg(test)] mod tests { use super::*; ... }`.
- Integration tests live in each crate's `tests/` and use only the public API (`proto`) or process boundaries (fake upstreams).
- Test names state the behaviour: `rejects_unknown_field`, `splice_leaves_tool_schema_model_untouched`. No `test_` prefix.
- One behaviour per test.
- Pin translation output with `insta` snapshots and review them (`cargo insta review`, or `INSTA_UPDATE=always` then inspect the diff). Do not hand-write large expected strings.
- Fixtures are sanitized real captures in `tests/fixtures/`. Remove prompts and tokens before committing.
- Tests may use `unwrap` and `expect` (see `clippy.toml`).
- Async tests use `#[tokio::test]`; use a paused clock instead of real sleeps.
- No external network. Upstreams are local fake servers.

## Clippy

- The default groups stay as they are.
- `pedantic` is on as a group, minus the noisiest lints.
- `restriction` is never enabled as a group (the Clippy docs warn against it); individual lints are picked.
- `nursery` and `cargo` are off.

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
unreachable_pub = "warn"
unused_qualifications = "warn"
unused_must_use = "deny"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
too_many_lines = "allow"
similar_names = "allow"
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
# clippy.toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-indexing-slicing-in-tests = true
allow-print-in-tests = true
```

- CI: `cargo clippy --workspace --all-targets -- -D warnings`. Zero warnings before merge.
- CLI output goes through one function that carries `#[expect(clippy::print_stdout, reason = "...")]`.
- Fix the code rather than silencing a lint.

## Never

- Explanatory comments, `TODO`, commented-out code, `#[allow]`.
- `unwrap`, `expect`, `panic!`, `v[i]`, `println!`, `dbg!` outside tests.
- `unsafe`, `mod.rs`.
- `util`/`common`/`helpers` modules, `Manager`/`Handler` suffixes.
- `Box<dyn Error>`, `Result<_, String>`.
- `async-trait` or needless `dyn`.
- `_ =>` on your own enums.
- Blocking calls in async code; `std::sync::Mutex` guards across `.await`.
- Logging or printing secrets or bodies.
- `untagged`, or serialized output that depends on `HashMap` order.
- IO, async, clock, randomness or `tracing` in `proto`.
- Unused dependencies, features, abstractions or options.

## Sources

- Rust Style Guide: https://doc.rust-lang.org/style-guide/
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/checklist.html
- rustfmt configuration: https://github.com/rust-lang/rustfmt/blob/master/Configurations.md
- Clippy lints and configuration: https://doc.rust-lang.org/clippy/lints.html, https://doc.rust-lang.org/clippy/configuration.html
- Cargo workspace lints: https://doc.rust-lang.org/cargo/reference/workspaces.html#the-lints-table
- Edition 2024: https://doc.rust-lang.org/edition-guide/rust-2024/
- Module files: https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html
- Errors: https://doc.rust-lang.org/book/ch09-00-error-handling.html, https://docs.rs/thiserror, https://docs.rs/anyhow
- tokio: https://tokio.rs/tokio/tutorial/shared-state, https://ryhl.io/blog/async-what-is-blocking/
- serde: https://serde.rs/container-attrs.html, https://docs.rs/serde_json/latest/serde_json/value/struct.RawValue.html
- Tests: https://doc.rust-lang.org/book/ch11-03-test-organization.html, https://insta.rs/docs/
