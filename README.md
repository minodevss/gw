# gw

A personal LLM gateway. Register your subscriptions, API keys and local models once on one server, then use every model from Claude Code, Codex and other clients on any of your machines.

```
Claude Code ─┐                      ┌─ Claude subscriptions, Anthropic API keys
Codex       ─┼─▶ gw (your server) ──┼─ ChatGPT subscriptions, OpenAI API keys
opencode    ─┤                      ├─ GLM Coding Plan, Kimi Code
pi, …       ─┘                      └─ Ollama, LM Studio, vLLM, any compatible endpoint
```

- Speaks the Anthropic Messages, OpenAI Responses and OpenAI Chat Completions APIs.
- Picks an account for you and keeps a conversation on the same account so prompt caches stay warm.
- Moves to the next account when an upstream reports a usage limit.
- Translates between APIs, so Claude Code can use GPT models and Codex can use Claude models.

> [!WARNING]
> Using subscription credentials (Claude, ChatGPT, GLM Coding Plan, Kimi Code) through a gateway may break the provider's terms and can lead to account suspension. API keys are the sanctioned path. You are responsible for how you use gw.

## Design rules

- **Forward, don't rewrite.** When client and upstream speak the same API, only the credentials and the `model` value change; every other byte is passed through. Across APIs, requests follow fixed mapping rules.
- **No guessing.** gw acts only on what upstreams report and what you configure. Anything it cannot express is an error, not a silent drop.
- **One automatic action.** When an upstream says an account hit its usage limit, gw retries on the next account. Nothing else happens behind your back.
- **No background traffic.** gw sends no keep-alives, polling or retries. It contacts upstreams on its own only when you ask (model refresh).
- **Visible decisions.** Every response carries `x-gw-account` and `x-gw-reason`, and every request is logged.

## Install

**Docker** (Linux, amd64 and arm64). Publish the port only on an address your devices reach, such as a Tailscale IP:

```sh
docker run -d --name gw --restart unless-stopped \
  -p 100.64.0.2:8787:8787 -v gw:/data ghcr.io/minodevss/gw
docker exec -it gw gw          # management TUI
```

**Binary** for Linux (`x86_64` or `aarch64`, static) and macOS (`aarch64`) from [releases](https://github.com/minodevss/gw/releases):

```sh
curl -fsSLO https://github.com/minodevss/gw/releases/latest/download/gw-x86_64-unknown-linux-musl.tar.gz
gh attestation verify gw-x86_64-unknown-linux-musl.tar.gz --repo minodevss/gw
tar -xzf gw-x86_64-unknown-linux-musl.tar.gz gw && install gw ~/.local/bin/
```

**From source** with Rust 1.100:

```sh
cargo install --locked --path crates/gw
```

## Quick start

On the server (for example a home machine reachable over Tailscale):

```sh
gw serve --listen 100.64.0.2:8787
```

In another shell on the same server:

```sh
gw account add anthropic --method oauth     # Claude subscription
gw account add openai --method codex        # ChatGPT subscription
gw account add openai --method key          # OpenAI API key (prompted, never a flag)
gw account add custom --url http://localhost:11434 --api chat --name ollama
gw key create macbook                       # prints a client key and ready-to-paste settings
```

Paste the printed settings on each device. Nothing is installed on the clients.

In Docker, prefix these with `docker exec -it gw` and give `gw key create` the address clients use, since the container only knows its own: `--url http://100.64.0.2:8787`.

### Running as a service

Without Docker, run gw as a systemd user service so the CLI and the server share `~/.gw`. Save as `~/.config/systemd/user/gw.service`:

```ini
[Unit]
Description=gw

[Service]
ExecStart=%h/.local/bin/gw serve --listen 100.64.0.2:8787
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```sh
systemctl --user enable --now gw
loginctl enable-linger       # keep it running after you log out
```

## Accounts

| Provider | Method | What it is | Billing |
|---|---|---|---|
| `openai` | `siwc` | Sign in with ChatGPT | subscription |
| | `codex` | Codex login (device code) | subscription |
| | `key` | API key | pay as you go |
| `anthropic` | `oauth` | Claude subscription login | subscription |
| | `setup-token` | token from `claude setup-token` | subscription |
| | `key` | API key | pay as you go |
| `zai` | `plan` | GLM Coding Plan key | subscription |
| | `key` | API key | pay as you go |
| `moonshot` | `plan` | Kimi Code key | subscription |
| | `key` | Open Platform key | pay as you go |
| `custom` | | any Messages, Responses or Chat endpoint | subscription |

`--method` is optional on a terminal; gw lists the choices. Keys and tokens are read from a hidden prompt or stdin, never from arguments.

`siwc` sends the browser to `http://127.0.0.1:<port>/auth/callback` after sign-in. When the browser runs on the gw machine, gw picks that up by itself. Otherwise the page fails to load: paste its address into gw instead. This works in Docker too.

## Clients

`gw key create <name>` prints these with your server address and key filled in.

**Claude Code**: `~/.claude/settings.json` (environment set here overrides your shell):

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://100.64.0.2:8787",
    "ANTHROPIC_AUTH_TOKEN": "gw_…",
    "CLAUDE_CODE_PROMPT_CACHE_TTL": "1h"
  }
}
```

**Codex**: `~/.codex/config.toml`, plus `export GW_API_KEY=gw_…`:

```toml
model_provider = "gw"

[model_providers.gw]
name = "gw"
base_url = "http://100.64.0.2:8787/v1"
wire_api = "responses"
env_key = "GW_API_KEY"
model_catalog_url = "http://100.64.0.2:8787/codex/models"
```

Codex sends hosted tools (`web_search`, `multi_agent`) that only OpenAI understands. For other models, add `~/.codex/no-web-search.config.toml` and run `codex -p no-web-search -m <model>`:

```toml
model_provider = "gw"
web_search = "disabled"

[features]
multi_agent = false
```

**opencode, pi and others**: point an OpenAI-compatible provider at `http://<server>/v1` with the key.

## Choosing models

Use the upstream model id, optionally with a billing suffix:

| Name | Uses |
|---|---|
| `gpt-6.1-sol` | any account serving it, subscriptions first |
| `gpt-6.1-sol:sub` | subscription accounts only |
| `gpt-6.1-sol:api` | API key accounts only |

For each request, gw keeps the account already used by the conversation. Otherwise it picks subscriptions before API keys, and among those the account whose usage window resets soonest. Clients identify conversations with their own session headers; requests without one are not pinned.

Model capabilities come from the upstreams' model lists. Fill gaps in `~/.gw/catalog.override.toml`, keyed by provider or account name, then run `gw model refresh`:

```toml
[zai."glm-5.3"]
context = 200000
max_output = 32000
```

## Management

```
gw serve [--listen <addr>]            # default 127.0.0.1:8787; GET /healthz needs no key
gw account add | login | list | show | rename | remove
gw key create | list | revoke
gw model refresh | list | show
gw log [--follow] [--filter "account:x status:429"]
gw admin token [--rotate]             # token for the admin web page
gw                          # management TUI
```

Running `gw` with no command opens the management TUI. It shows accounts with their usage windows and reset times, keys, models with per-account support, and a live request log. It runs over SSH and copies with OSC 52.

### Admin web page

Where a terminal is awkward, as in Kubernetes, open `http://<gw address>/admin` in a browser. It is on the same port as the API.

- **Overview** says whether every account can take requests, lists what needs attention with the fix next to it, and shows where the next request for each model you use would go.
- **Usage** charts each account's usage windows (such as 5 hours, 7 days and per-model weekly windows) with resets, reported limits and a projection to the reset, the capacity left across accounts, and how much each account had left at its last resets. gw saves a reading whenever a response reports a change, so history starts once you run this version.
- **Traffic** breaks requests and tokens down by key, model, account or outcome, with cache use, request times and a weekday-by-hour view.
- **Activity** is the request log, with the same search as `gw log --filter`, a page per request and per conversation.
- **Accounts**, **Models** and **Keys** let you add accounts and sign in to them, rename or remove them, refresh usage and model lists, and create or revoke keys.
- Pages refresh themselves while they are open. Times are in your browser's time zone.
- Sign-ins work from any browser, because the page asks for the code or address you get after signing in.

Sign in with the token from `gw admin token` (`docker exec gw gw admin token`, `kubectl exec <pod> -- gw admin token`). The command creates the token on first use and prints the same one after that. `--rotate` replaces it and signs out every browser. Until a token exists the page only explains how to create one.

The page is protected by the token alone. To keep it off the internet while the API is public, block `/admin` at your proxy.

## Data

Everything lives in `~/.gw` (or `GW_HOME`), created with mode 0700:

- `gw.db`: accounts, client key hashes, model catalog, request log. Request and response bodies are never stored.
- `catalog.override.toml`: your catalog corrections.

Client keys are stored as SHA-256 hashes and shown once. Upstream credentials are stored as-is; protect the server with disk encryption.

## Development

See [AGENTS.md](AGENTS.md) for build and test commands and [docs/style/rust.md](docs/style/rust.md) for code style.

## License

[MIT](LICENSE)
