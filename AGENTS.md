# AGENTS.md

gw is a personal LLM gateway in Rust. It serves the Anthropic Messages, OpenAI Responses and OpenAI Chat Completions APIs, routes each request to one of many upstream accounts, and translates between the APIs when client and upstream differ. User-facing docs are in `README.md`.

## Layout

- `crates/proto`: pure protocol code. No IO, async, clock or randomness.
  - `ir.rs`: the shared representation every API decodes into and encodes from.
  - `messages/`, `responses/`, `chat/`: `types.rs` (wire types), `decode.rs`, `encode.rs`, `stream.rs` per API.
  - `passthrough/`: `peek` (read fields), `splice` (replace only the top-level `model` bytes), `tap` (read usage and errors from responses).
  - `reject.rs`: rejection reasons; `capability_rejected: <class>` tokens for Claude Code.
  - `tests/`: integration tests, `tests/fixtures/` (sanitized real captures), `tests/snapshots/` (insta).
- `crates/gw`: the binary.
  - `serve.rs` (routing, auth, account selection loop), `forward.rs` (upstream send, stream relay and transcoding).
  - `select.rs` (candidate order), `session.rs` (conversation pinning), `store.rs` (SQLite).
  - `provider.rs` and `provider/`: everything provider-specific: endpoints, auth, login, refresh, model lists, limit signals.
  - `ops/`: actions shared by the CLI (`cli.rs`) and the TUI (`tui/`).

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All four must pass before a commit. Update snapshots with `INSTA_UPDATE=always cargo test -p proto` and inspect the diff before keeping it.

## Releases

Bump `version` in `crates/gw/Cargo.toml`, commit, then push a `v<version>` tag. `.github/workflows/release.yml` checks that the tag matches, publishes the Linux and macOS binaries with checksums and attestations, and pushes the `ghcr.io` image. After editing workflows, run `gh actions-lock` to update `.github/workflows/actions.lock`.

## Code style

Follow `docs/style/rust.md`. The rules that matter most:

- No comments except why-comments for external facts, with a source link. No `///` docs.
- No `unwrap`, `expect`, `panic!` or indexing outside tests. No `println!`; CLI output goes through `cli::out`.
- `foo.rs` + `foo/` modules, never `mod.rs`. `pub(crate)` in `gw`.
- Provider and API dispatch is `enum` + exhaustive `match`; never `_ =>` on project enums.
- Same input, same bytes: translation output must be deterministic so prompt caches keep hitting.

## Gateway invariants

Changes must keep these true:

- Same-API requests are forwarded byte for byte except credentials and the `model` value. Never re-serialize them.
- No guessing. Anything a target cannot express is rejected with a clear error, not dropped. For Claude Code clients, use the `capability_rejected` classes in `reject.rs` exactly.
- The only automatic action is moving to the next account on an upstream usage-limit signal (`provider/limit.rs`). Transient errors are returned to the client, which retries.
- gw sends no requests of its own except when the user asks (`gw model refresh`, TUI refresh).
- Provider facts (URLs, headers, error codes) live only under `provider/` and must come from primary sources (vendor docs, official client source). Cite the source in a why-comment when it is not obvious.

## Testing

- Add or update tests for every behaviour you change.
- Translation changes need fixture-based tests and reviewed snapshots in `crates/proto/tests`.
- Tests never touch the network. Use local fake servers on 127.0.0.1.
- When running real clients for manual checks, isolate them: Claude Code reads `~/.claude/settings.json`, whose `env` overrides the shell. Use a fresh `CLAUDE_CONFIG_DIR` or `--settings`, and a fresh `CODEX_HOME` for Codex.

## Security

- Never log, print or snapshot tokens, keys or request bodies. `Secret` hides its values in `Debug`; keep it that way.
- Secrets are read from prompts or stdin, never from command-line arguments.
- Sanitize captures before adding them to `tests/fixtures/` (paths, ids, tokens).

## Commits

- One commit per reason to change. Message format: `type: description`, lowercase, no period. Types: feat, fix, refactor, docs, test, chore, perf.
- Keep generated files (`target/`) and local notes out of commits.
