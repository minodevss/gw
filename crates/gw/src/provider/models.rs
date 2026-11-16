use std::collections::BTreeMap;

use http::HeaderMap;
use http::header::{HeaderName, HeaderValue};
use serde::Deserialize;

use super::oauth;
use super::{Method, Provider, ProviderError};
use crate::account::{Account, Secret};
use crate::catalog::Capabilities;

const CODEX_CLIENT_VERSION: &str = "0.162.1";
const ANTHROPIC_VERSION: HeaderName = HeaderName::from_static("anthropic-version");
const MAX_PAGES: usize = 50;
const EFFORT_ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

// https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
const SIWC_UNSUPPORTED: [&str; 4] = ["max_output_tokens", "temperature", "top_p", "user"];

type Models = Vec<(String, Capabilities)>;

pub(crate) async fn list_models(
    http: &reqwest::Client,
    account: &Account,
) -> Result<Models, ProviderError> {
    let kind = account.kind;
    let Some(path) = kind.models_path() else {
        return Ok(Vec::new());
    };
    let base = base_url(account)?;
    let headers = request_headers(account);
    match (kind.provider(), kind.method()) {
        (Provider::Zai, _) => Ok(Vec::new()),
        (Provider::Anthropic, _) => fetch_anthropic(http, &base, path, headers).await,
        (Provider::OpenAi, Method::Codex) => fetch_codex(http, &base, headers).await,
        (Provider::OpenAi, Method::Siwc) => fetch_siwc(http, &base, path, headers).await,
        (Provider::Moonshot, Method::Plan) => {
            let text = oauth::get_text(http, &format!("{base}{path}"), headers).await?;
            parse_moonshot_plan(&text)
        }
        (Provider::OpenAi | Provider::Moonshot | Provider::Custom, _) => {
            let text = oauth::get_text(http, &format!("{base}{path}"), headers).await?;
            parse_ids(&text)
        }
    }
}

fn base_url(account: &Account) -> Result<String, ProviderError> {
    match (&account.kind.provider(), &account.secret) {
        (Provider::Custom, Secret::Custom { url, .. }) => Ok(url.trim_end_matches('/').to_owned()),
        (Provider::Custom, _) => Err(ProviderError::Invalid(
            "custom account has no url".to_owned(),
        )),
        (Provider::Anthropic | Provider::OpenAi | Provider::Zai | Provider::Moonshot, _) => {
            Ok(account.kind.base_url().to_owned())
        }
    }
}

fn request_headers(account: &Account) -> HeaderMap {
    let mut headers = HeaderMap::new();
    account.kind.apply_auth(
        account.secret.bearer(),
        account.secret.account_id(),
        &mut headers,
    );
    if account.kind.provider() == Provider::Anthropic {
        headers.insert(ANTHROPIC_VERSION, HeaderValue::from_static("2023-06-01"));
    }
    headers
}

async fn fetch_anthropic(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    headers: HeaderMap,
) -> Result<Models, ProviderError> {
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!("{base}{path}");
        if let Some(id) = &after {
            let encoded: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();
            url.push_str("&after_id=");
            url.push_str(&encoded);
        }
        let text = oauth::get_text(http, &url, headers.clone()).await?;
        let page = parse_anthropic_page(&text)?;
        models.extend(page.models);
        match page.next_after {
            Some(id) => after = Some(id),
            None => return Ok(models),
        }
    }
    Err(ProviderError::Invalid(
        "model list has too many pages".to_owned(),
    ))
}

async fn fetch_codex(
    http: &reqwest::Client,
    base: &str,
    headers: HeaderMap,
) -> Result<Models, ProviderError> {
    let url = format!("{base}/models?client_version={CODEX_CLIENT_VERSION}");
    let text = oauth::get_text(http, &url, headers).await?;
    parse_codex(&text)
}

async fn fetch_siwc(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    headers: HeaderMap,
) -> Result<Models, ProviderError> {
    let text = oauth::get_text(http, &format!("{base}{path}"), headers).await?;
    parse_siwc(&text)
}

#[derive(Deserialize)]
struct Flag {
    supported: bool,
}

#[derive(Deserialize)]
struct AnthropicPage {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    #[serde(default)]
    max_input_tokens: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    capabilities: Option<AnthropicCapabilities>,
}

#[derive(Deserialize)]
struct AnthropicCapabilities {
    #[serde(default)]
    effort: Option<AnthropicEffort>,
    #[serde(default)]
    thinking: Option<AnthropicThinking>,
    #[serde(default)]
    image_input: Option<Flag>,
    #[serde(default)]
    pdf_input: Option<Flag>,
    #[serde(default)]
    structured_outputs: Option<Flag>,
}

#[derive(Deserialize)]
struct AnthropicEffort {
    supported: bool,
    #[serde(flatten)]
    levels: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct AnthropicThinking {
    supported: bool,
    #[serde(default)]
    types: BTreeMap<String, Flag>,
}

struct Page {
    models: Models,
    next_after: Option<String>,
}

fn parse_anthropic_page(body: &str) -> Result<Page, ProviderError> {
    let page: AnthropicPage = serde_json::from_str(body)?;
    let next_after = if page.has_more { page.last_id } else { None };
    let models = page
        .data
        .into_iter()
        .map(|model| {
            let capabilities = anthropic_capabilities(&model);
            (model.id, capabilities)
        })
        .collect();
    Ok(Page { models, next_after })
}

fn anthropic_capabilities(model: &AnthropicModel) -> Capabilities {
    let flag = |flag: Option<&Flag>| flag.map(|flag| flag.supported);
    let capabilities = model.capabilities.as_ref();
    Capabilities {
        context: model.max_input_tokens,
        max_output: model.max_tokens,
        efforts: capabilities
            .and_then(|caps| caps.effort.as_ref())
            .and_then(anthropic_efforts),
        thinking: capabilities
            .and_then(|caps| caps.thinking.as_ref())
            .map(anthropic_thinking),
        image: flag(capabilities.and_then(|caps| caps.image_input.as_ref())),
        document: flag(capabilities.and_then(|caps| caps.pdf_input.as_ref())),
        structured_output: flag(capabilities.and_then(|caps| caps.structured_outputs.as_ref())),
        ..Capabilities::default()
    }
}

fn anthropic_efforts(effort: &AnthropicEffort) -> Option<Vec<String>> {
    if !effort.supported {
        return Some(Vec::new());
    }
    let mut levels: Vec<String> = effort
        .levels
        .iter()
        .filter(|(_, value)| {
            value
                .get("supported")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .map(|(name, _)| name.clone())
        .collect();
    levels.sort_by_key(|name| {
        EFFORT_ORDER
            .iter()
            .position(|known| known == name)
            .unwrap_or(EFFORT_ORDER.len())
    });
    (!levels.is_empty()).then_some(levels)
}

fn anthropic_thinking(thinking: &AnthropicThinking) -> Vec<String> {
    let mut kinds = vec!["disabled".to_owned()];
    if thinking.supported {
        kinds.extend(
            thinking
                .types
                .iter()
                .filter(|(_, flag)| flag.supported)
                .map(|(name, _)| name.clone()),
        );
    }
    kinds
}

#[derive(Deserialize)]
struct CodexList {
    #[serde(default)]
    models: Vec<CodexModel>,
    #[serde(default)]
    data: Vec<CodexModel>,
}

#[derive(Deserialize)]
struct CodexModel {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<ReasoningLevel>>,
    #[serde(default)]
    service_tiers: Vec<ServiceTier>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    base_instructions: Option<String>,
    #[serde(default)]
    model_messages: Option<ModelMessages>,
}

#[derive(Deserialize)]
struct ModelMessages {
    #[serde(default)]
    instructions_template: Option<String>,
}

#[derive(Deserialize)]
struct ReasoningLevel {
    effort: String,
}

#[derive(Deserialize)]
struct ServiceTier {
    id: String,
}

impl CodexModel {
    fn name(&self) -> Option<&str> {
        self.slug.as_deref().or(self.id.as_deref())
    }
}

fn codex_entries(body: &str) -> Result<Vec<CodexModel>, ProviderError> {
    let list: CodexList = serde_json::from_str(body)?;
    Ok(if list.models.is_empty() {
        list.data
    } else {
        list.models
    })
}

fn parse_codex(body: &str) -> Result<Models, ProviderError> {
    Ok(codex_entries(body)?
        .into_iter()
        .filter_map(|model| {
            let name = model.name()?.to_owned();
            let capabilities = Capabilities {
                context: model.context_window,
                efforts: model.supported_reasoning_levels.map(|levels| {
                    levels
                        .into_iter()
                        .map(|level| level.effort)
                        .collect::<Vec<_>>()
                }),
                fast: Some(model.service_tiers.iter().any(|tier| tier.id == "priority")),
                image: model
                    .input_modalities
                    .map(|modalities| modalities.iter().any(|kind| kind == "image")),
                instructions: model
                    .model_messages
                    .and_then(|messages| messages.instructions_template)
                    .or(model.base_instructions),
                ..Capabilities::default()
            };
            Some((name, capabilities))
        })
        .collect())
}

// Entries without a visibility field are kept so a response shape change does not empty the list.
// https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations
fn parse_siwc(body: &str) -> Result<Models, ProviderError> {
    Ok(codex_entries(body)?
        .into_iter()
        .filter(|model| {
            model
                .visibility
                .as_deref()
                .is_none_or(|value| value == "list")
        })
        .filter_map(|model| {
            let capabilities = Capabilities {
                unsupported: SIWC_UNSUPPORTED
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
                ..Capabilities::default()
            };
            Some((model.name()?.to_owned(), capabilities))
        })
        .collect())
}

#[derive(Deserialize)]
struct IdList {
    data: Vec<IdEntry>,
}

#[derive(Deserialize)]
struct IdEntry {
    id: String,
}

fn parse_ids(body: &str) -> Result<Models, ProviderError> {
    let list: IdList = serde_json::from_str(body)?;
    Ok(list
        .data
        .into_iter()
        .map(|entry| (entry.id, Capabilities::default()))
        .collect())
}

#[derive(Deserialize)]
struct MoonshotList {
    data: Vec<MoonshotModel>,
}

#[derive(Deserialize)]
struct MoonshotModel {
    id: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    supports_image_in: Option<bool>,
}

fn parse_moonshot_plan(body: &str) -> Result<Models, ProviderError> {
    let list: MoonshotList = serde_json::from_str(body)?;
    Ok(list
        .data
        .into_iter()
        .map(|model| {
            let capabilities = Capabilities {
                context: model.context_length,
                image: model.supports_image_in,
                ..Capabilities::default()
            };
            (model.id, capabilities)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::State;
    use crate::provider::Kind;
    use crate::provider::oauth::fake::{FakeServer, Reply};

    const ANTHROPIC_SAMPLE: &str = r#"{
      "data": [
        {
          "id": "claude-opus-4-6",
          "type": "model",
          "display_name": "Claude Opus 4.6",
          "created_at": "2026-02-04T00:00:00Z",
          "max_input_tokens": 1000000,
          "max_tokens": 128000,
          "capabilities": {
            "batch": {"supported": true},
            "citations": {"supported": true},
            "effort": {
              "supported": true,
              "max": {"supported": true},
              "high": {"supported": true},
              "medium": {"supported": true},
              "low": {"supported": true}
            },
            "image_input": {"supported": true},
            "pdf_input": {"supported": true},
            "structured_outputs": {"supported": true},
            "thinking": {
              "supported": true,
              "types": {"adaptive": {"supported": true}, "enabled": {"supported": false}}
            }
          }
        },
        {
          "id": "claude-haiku-3",
          "type": "model",
          "max_input_tokens": 200000,
          "max_tokens": 4096,
          "capabilities": {
            "effort": {"supported": false},
            "image_input": {"supported": false},
            "thinking": {"supported": false, "types": {}}
          }
        }
      ],
      "first_id": "claude-opus-4-6",
      "has_more": true,
      "last_id": "claude-haiku-3"
    }"#;

    const CODEX_SAMPLE: &str = r#"{
      "models": [
        {
          "slug": "gpt-5.5",
          "display_name": "GPT-5.5",
          "visibility": "list",
          "context_window": 272000,
          "supported_reasoning_levels": [
            {"effort": "low", "description": "Fast"},
            {"effort": "medium", "description": "Balanced"},
            {"effort": "high", "description": "Deep"}
          ],
          "service_tiers": [{"id": "priority", "name": "Fast", "description": "x"}],
          "input_modalities": ["text", "image"],
          "apply_patch_tool_type": "freeform"
        },
        {
          "slug": "gpt-5.5-mini",
          "visibility": "hide",
          "context_window": 128000,
          "supported_reasoning_levels": [],
          "service_tiers": [],
          "input_modalities": ["text"]
        }
      ]
    }"#;

    fn caps(models: &Models, name: &str) -> Capabilities {
        models
            .iter()
            .find(|(id, _)| id == name)
            .map(|(_, caps)| caps.clone())
            .unwrap()
    }

    #[test]
    fn anthropic_maps_capabilities() {
        let page = parse_anthropic_page(ANTHROPIC_SAMPLE).unwrap();
        assert_eq!(page.next_after.as_deref(), Some("claude-haiku-3"));
        let opus = caps(&page.models, "claude-opus-4-6");
        assert_eq!(opus.context, Some(1_000_000));
        assert_eq!(opus.max_output, Some(128_000));
        assert_eq!(
            opus.efforts,
            Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into()
            ])
        );
        assert_eq!(
            opus.thinking,
            Some(vec!["disabled".into(), "adaptive".into()])
        );
        assert_eq!(
            (opus.image, opus.document, opus.structured_output),
            (Some(true), Some(true), Some(true))
        );
    }

    #[test]
    fn anthropic_unsupported_effort_is_explicit_and_missing_is_unknown() {
        let page = parse_anthropic_page(ANTHROPIC_SAMPLE).unwrap();
        let haiku = caps(&page.models, "claude-haiku-3");
        assert_eq!(haiku.efforts, Some(Vec::new()));
        assert_eq!(haiku.thinking, Some(vec!["disabled".into()]));
        assert_eq!((haiku.image, haiku.document), (Some(false), None));
    }

    #[test]
    fn anthropic_last_page_has_no_cursor() {
        let page = parse_anthropic_page(r#"{"data":[],"has_more":false,"last_id":"x"}"#).unwrap();
        assert_eq!(page.next_after, None);
    }

    #[test]
    fn codex_maps_context_efforts_fast_and_image() {
        let models = parse_codex(CODEX_SAMPLE).unwrap();
        assert_eq!(models.len(), 2);
        let full = caps(&models, "gpt-5.5");
        assert_eq!(full.context, Some(272_000));
        assert_eq!(
            full.efforts,
            Some(vec!["low".into(), "medium".into(), "high".into()])
        );
        assert_eq!((full.fast, full.image), (Some(true), Some(true)));
        let mini = caps(&models, "gpt-5.5-mini");
        assert_eq!((mini.fast, mini.image), (Some(false), Some(false)));
        assert_eq!(mini.efforts, Some(Vec::new()));
    }

    #[test]
    fn siwc_keeps_listed_models_and_marks_unsupported_fields() {
        let models = parse_siwc(CODEX_SAMPLE).unwrap();
        let names: Vec<_> = models.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(names, ["gpt-5.5"]);
        assert_eq!(
            caps(&models, "gpt-5.5").unsupported,
            ["max_output_tokens", "temperature", "top_p", "user"]
        );
    }

    #[test]
    fn plain_lists_return_ids_only() {
        let models =
            parse_ids(r#"{"object":"list","data":[{"id":"gpt-5","object":"model"},{"id":"o4"}]}"#)
                .unwrap();
        assert_eq!(
            models,
            vec![
                ("gpt-5".to_owned(), Capabilities::default()),
                ("o4".to_owned(), Capabilities::default())
            ]
        );
    }

    #[test]
    fn moonshot_plan_reads_context_and_image() {
        let models = parse_moonshot_plan(
            r#"{"data":[{"id":"kimi-for-coding","context_length":262144,"supports_reasoning":true,"supports_image_in":true,"supports_video_in":false}]}"#,
        )
        .unwrap();
        let kimi = caps(&models, "kimi-for-coding");
        assert_eq!((kimi.context, kimi.image), (Some(262_144), Some(true)));
        assert_eq!((kimi.thinking, kimi.efforts), (None, None));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(parse_ids("nope"), Err(ProviderError::Json(_))));
    }

    fn account(kind: Kind, secret: Secret) -> Account {
        Account {
            name: "a".to_owned(),
            kind,
            secret,
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at: 0,
        }
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            body: body.to_owned(),
        }
    }

    #[tokio::test]
    async fn anthropic_follows_pagination_with_after_id() {
        let server = FakeServer::start(vec![
            reply(200, ANTHROPIC_SAMPLE),
            reply(
                200,
                r#"{"data":[{"id":"claude-sonnet-5-5"}],"has_more":false,"last_id":"claude-sonnet-5-5"}"#,
            ),
        ])
        .await;
        let models = fetch_anthropic(
            &reqwest::Client::new(),
            &server.base_url,
            "/v1/models?limit=1000",
            HeaderMap::new(),
        )
        .await
        .unwrap();
        assert_eq!(models.len(), 3);
        let requests = server.requests().await;
        assert!(requests[0].starts_with("GET /v1/models?limit=1000 "));
        assert!(requests[1].starts_with("GET /v1/models?limit=1000&after_id=claude-haiku-3 "));
    }

    #[tokio::test]
    async fn codex_request_carries_client_version() {
        let server = FakeServer::start(vec![reply(200, CODEX_SAMPLE)]).await;
        fetch_codex(&reqwest::Client::new(), &server.base_url, HeaderMap::new())
            .await
            .unwrap();
        let requests = server.requests().await;
        assert!(requests[0].starts_with("GET /models?client_version=0.162.1 "));
    }

    #[tokio::test]
    async fn custom_account_uses_its_url_and_bearer_key() {
        let server = FakeServer::start(vec![reply(200, r#"{"data":[{"id":"local-1"}]}"#)]).await;
        let acct = account(
            Kind::new(Provider::Custom, Method::Custom).unwrap(),
            Secret::Custom {
                url: format!("{}/", server.base_url),
                api: "chat".to_owned(),
                key: Some("sk-local".to_owned()),
            },
        );
        let models = list_models(&reqwest::Client::new(), &acct).await.unwrap();
        assert_eq!(models[0].0, "local-1");
        let requests = server.requests().await;
        assert!(requests[0].starts_with("GET /v1/models "));
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer sk-local")
        );
    }

    #[tokio::test]
    async fn upstream_errors_carry_status_and_excerpt() {
        let server = FakeServer::start(vec![reply(401, r#"{"error":"bad key"}"#)]).await;
        let acct = account(
            Kind::new(Provider::Custom, Method::Custom).unwrap(),
            Secret::Custom {
                url: server.base_url.clone(),
                api: "chat".to_owned(),
                key: None,
            },
        );
        let error = list_models(&reqwest::Client::new(), &acct)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Status { status: 401, ref body_excerpt } if body_excerpt.contains("bad key")
        ));
    }

    #[tokio::test]
    async fn zai_has_no_model_endpoint() {
        let acct = account(
            Kind::new(Provider::Zai, Method::Key).unwrap(),
            Secret::Key {
                key: "k".to_owned(),
            },
        );
        assert!(
            list_models(&reqwest::Client::new(), &acct)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn anthropic_requests_carry_version_and_oauth_beta() {
        let acct = account(
            Kind::new(Provider::Anthropic, Method::Oauth).unwrap(),
            Secret::Token {
                token: "tok".to_owned(),
                expires_at: None,
            },
        );
        let headers = request_headers(&acct);
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert_eq!(headers["authorization"], "Bearer tok");
        assert_eq!(headers["anthropic-beta"], "oauth-2025-04-20");
    }
}
