use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path as AxumPath, State as AxumState};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode, header};
use axum::routing::{get, post};
use proto::Api;
use proto::error::{ErrorBody, ErrorKind};
use proto::passthrough::peek;
use proto::reject::Rejection;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::account::{Account, State};
use crate::catalog::{self, view};
use crate::clock;
use crate::forward::{
    self, Inbound, Outcome, Target, Trace, X_GW_ACCOUNT, X_GW_REASON, describe_limit,
    describe_state, error_response, finish_row,
};
use crate::ops::key;
use crate::provider::{self, Limit, ProviderError, Usage};
use crate::select::{self, NoCandidate, Query, Reason};
use crate::session::Sessions;
use crate::store::{self, LogRow, Store};

const DEFAULT_PIN_TTL: i64 = 300;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
const SESSION_HEADERS: [&str; 5] = [
    "x-claude-code-session-id",
    "session-id",
    "x-opencode-session-id",
    "x-session-affinity",
    "x-session-id",
];

#[derive(Clone)]
pub(crate) struct App(Arc<Inner>);

struct Inner {
    db: PathBuf,
    http: reqwest::Client,
    sessions: Sessions,
    writes: mpsc::Sender<Write>,
}

enum Write {
    Log(Box<LogRow>),
    Usage {
        account: String,
        usage: Usage,
        at: i64,
    },
    State {
        account: String,
        state: State,
    },
    Heartbeat {
        listen: String,
        at: i64,
    },
}

impl App {
    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.0.http
    }

    pub(crate) fn log(&self, row: LogRow) {
        self.write(Write::Log(Box::new(row)));
    }

    pub(crate) fn record_usage(&self, account: &str, usage: Usage) {
        self.write(Write::Usage {
            account: account.to_owned(),
            usage,
            at: clock::now(),
        });
    }

    pub(crate) fn record_limit(&self, account: &str, limit: &Limit) {
        if let Some(reset_at) = limit.reset_at {
            self.write(Write::State {
                account: account.to_owned(),
                state: State::Blocked {
                    scope: limit.scope.clone(),
                    reset_at,
                },
            });
        }
    }

    fn write(&self, write: Write) {
        if self.0.writes.try_send(write).is_err() {
            tracing::warn!("database write queue is full or closed");
        }
    }

    async fn db<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Store) -> store::Result<T> + Send + 'static,
    ) -> store::Result<T> {
        let db = self.0.db.clone();
        tokio::task::spawn_blocking(move || work(&mut Store::open(&db)?))
            .await
            .map_err(|error| store::Error::Invalid(error.to_string()))?
    }

    async fn refreshed(&self, account: Account, force: bool) -> Result<Account, String> {
        let kind = account.kind;
        let now = clock::now();
        if !kind.refreshes() || (!force && !provider::needs_refresh(kind, &account.secret, now)) {
            return Ok(account);
        }
        let app = self.clone();
        let name = account.name.clone();
        let stale = account.secret.clone();
        let handle = tokio::runtime::Handle::current();
        let outcome = self
            .db(move |store| {
                store.refresh_secret(&name, &stale, |secret| {
                    handle.block_on(provider::refresh(app.http(), kind, secret))
                })
            })
            .await
            .map_err(|error| error.to_string())?;
        match outcome {
            Ok(secret) => Ok(Account { secret, ..account }),
            Err(ProviderError::Expired) => {
                let reason = format!("refresh failed {}", clock::format_utc(now));
                self.write(Write::State {
                    account: account.name.clone(),
                    state: State::Inactive {
                        reason: reason.clone(),
                    },
                });
                Err(format!("{} inactive: {reason}", account.name))
            }
            Err(error) => Err(format!("{} refresh failed: {error}", account.name)),
        }
    }
}

pub(crate) async fn run(home: &Path, listen: &str) -> anyhow::Result<()> {
    let db = crate::home::database(home)?;
    Store::open(&db)?;
    let (writes, receiver) = mpsc::channel(4096);
    let writer = std::thread::spawn({
        let db = db.clone();
        move || write_loop(&db, receiver)
    });
    let app = App(Arc::new(Inner {
        db,
        http: reqwest::Client::builder().build()?,
        sessions: Sessions::default(),
        writes,
    }));
    let heartbeat = tokio::spawn({
        let app = app.clone();
        let listen = listen.to_owned();
        async move {
            let mut ticker = tokio::time::interval(HEARTBEAT_EVERY);
            loop {
                ticker.tick().await;
                app.write(Write::Heartbeat {
                    listen: listen.clone(),
                    at: clock::now(),
                });
            }
        }
    });
    let router = Router::new()
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/models", get(models))
        .route("/v1/models/{id}", get(model))
        .route("/codex/models", get(codex_models))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(listen, "gw listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown())
        .await?;
    heartbeat.abort();
    if writer.join().is_err() {
        tracing::warn!("database writer panicked");
    }
    Ok(())
}

// Containers and service managers stop processes with SIGTERM; terminals send SIGINT.
async fn shutdown() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}

fn write_loop(db: &Path, mut receiver: mpsc::Receiver<Write>) {
    let store = match Store::open(db) {
        Ok(store) => store,
        Err(error) => {
            tracing::error!(%error, "database writer could not open the database");
            return;
        }
    };
    while let Some(write) = receiver.blocking_recv() {
        let result = match &write {
            Write::Log(row) => store.insert_log(row),
            Write::Usage { account, usage, at } => store.set_usage(account, usage, *at),
            Write::State { account, state } => store.set_state(account, state),
            Write::Heartbeat { listen, at } => store.heartbeat(listen, *at),
        };
        if let Err(error) = result {
            tracing::warn!(%error, "database write failed");
        }
    }
}

async fn messages(
    AxumState(app): AxumState<App>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    generate(&app, Api::Messages, Api::Messages.path(), &headers, &body).await
}

async fn count_tokens(
    AxumState(app): AxumState<App>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    generate(
        &app,
        Api::Messages,
        "/v1/messages/count_tokens",
        &headers,
        &body,
    )
    .await
}

async fn responses(
    AxumState(app): AxumState<App>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    generate(&app, Api::Responses, Api::Responses.path(), &headers, &body).await
}

async fn chat(AxumState(app): AxumState<App>, headers: HeaderMap, body: Bytes) -> Response<Body> {
    generate(&app, Api::Chat, Api::Chat.path(), &headers, &body).await
}

async fn models(AxumState(app): AxumState<App>, headers: HeaderMap) -> Response<Body> {
    let api = models_api(&headers);
    catalog_response(&app, &headers, api, |views| {
        Ok(match api {
            Api::Messages => view::anthropic(views),
            Api::Responses | Api::Chat => view::openai(views),
        })
    })
    .await
}

async fn model(
    AxumState(app): AxumState<App>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let api = models_api(&headers);
    catalog_response(&app, &headers, api, |views| {
        let found = views.iter().find(|m| m.id == id).ok_or_else(|| {
            ErrorBody::new(404, ErrorKind::NotFound, format!("model not found: {id}"))
        })?;
        Ok(match api {
            Api::Messages => view::anthropic_model(found),
            Api::Responses | Api::Chat => view::openai_model(found),
        })
    })
    .await
}

fn models_api(headers: &HeaderMap) -> Api {
    if headers.contains_key("anthropic-version") {
        Api::Messages
    } else {
        Api::Chat
    }
}

async fn codex_models(AxumState(app): AxumState<App>, headers: HeaderMap) -> Response<Body> {
    catalog_response(&app, &headers, Api::Responses, |views| {
        Ok(view::codex(views))
    })
    .await
}

async fn catalog_response(
    app: &App,
    headers: &HeaderMap,
    api: Api,
    render: impl FnOnce(&[view::ModelView]) -> Result<serde_json::Value, ErrorBody>,
) -> Response<Body> {
    let trace = blank_trace();
    if authenticate(app, headers).await.is_none() {
        return error_response(api, &unauthorized(), &trace);
    }
    match app
        .db(|store| Ok((store.accounts()?, store.catalog()?)))
        .await
    {
        Ok((accounts, catalog)) => {
            let body = match render(&view::models(&catalog, &accounts)) {
                Ok(body) => body.to_string(),
                Err(error) => return error_response(api, &error, &trace),
            };
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap_or_else(|_| Response::new(Body::empty()))
        }
        Err(error) => error_response(api, &internal(&error), &trace),
    }
}

async fn generate(
    app: &App,
    api: Api,
    path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response<Body> {
    let started = Instant::now();
    let now = clock::now();
    let Some(key_name) = authenticate(app, headers).await else {
        return error_response(api, &unauthorized(), &blank_trace());
    };
    let peeked = match peek(api, body) {
        Ok(peeked) => peeked,
        Err(error) => {
            let body = ErrorBody::new(400, ErrorKind::InvalidRequest, error.to_string());
            return error_response(api, &body, &blank_trace());
        }
    };
    let session = session_key(headers).or_else(|| peeked.cache_key.clone());
    let base = LogRow {
        ts: now,
        key: Some(key_name),
        session: session.clone(),
        api: api_name(api).to_owned(),
        model: peeked.model.clone(),
        ..LogRow::default()
    };
    if peeked.previous_response {
        let rejection = Rejection::unsupported("previous_response_id");
        return reject(
            app,
            api,
            &ErrorBody::rejected(&rejection, api),
            base,
            started,
        );
    }
    let (model, suffix) = select::parse_model(&peeked.model);
    let ttl = peeked
        .cache_ttl
        .and_then(|ttl| i64::try_from(ttl).ok())
        .unwrap_or(DEFAULT_PIN_TTL);
    let inbound = Inbound {
        api,
        path,
        headers,
        body,
        peek: &peeked,
    };
    let mut skip: Vec<String> = Vec::new();
    let mut moved_because: Option<String> = None;
    let mut retried_auth: Vec<String> = Vec::new();
    loop {
        let loaded = app
            .db(|store| Ok((store.accounts()?, store.catalog()?)))
            .await;
        let (accounts, catalog) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => return reject(app, api, &internal(&error), base, started),
        };
        let mut failure = None;
        let choice = app
            .0
            .sessions
            .resolve(session.as_deref(), model, now, ttl, |pinned| {
                let query = Query {
                    model,
                    suffix,
                    features: &peeked.features,
                    pinned,
                    skip: &skip,
                    now,
                };
                match select::select(query, &accounts, &catalog) {
                    Ok(choice) => Some((choice.account.clone(), choice)),
                    Err(error) => {
                        failure = Some(error);
                        None
                    }
                }
            });
        let Some(choice) = choice else {
            let body = no_candidate(api, &peeked.model, failure.as_ref());
            return reject(app, api, &body, base, started);
        };
        let Some(account) = accounts.iter().find(|a| a.name == choice.account).cloned() else {
            return reject(
                app,
                api,
                &internal(&"selected account vanished"),
                base,
                started,
            );
        };
        let account = match app.refreshed(account, false).await {
            Ok(account) => account,
            Err(reason) => {
                skip.push(choice.account.clone());
                moved_because = Some(reason);
                continue;
            }
        };
        let upstream = forward::upstream_api(&account, api);
        if path.ends_with("/count_tokens") && upstream != Api::Messages {
            let body = ErrorBody::new(
                404,
                ErrorKind::NotFound,
                format!("count_tokens is not available for {model}"),
            );
            return reject(app, api, &body, base, started);
        }
        let reason = reason_text(&choice.reason, moved_because.as_deref(), &accounts);
        let caps = catalog
            .get(&account.name, model)
            .cloned()
            .unwrap_or_default();
        let trace = Trace {
            row: LogRow {
                account: Some(account.name.clone()),
                reason: reason.clone(),
                ..base.clone()
            },
            started,
            gw_headers: vec![(X_GW_ACCOUNT, account.name.clone()), (X_GW_REASON, reason)],
        };
        let target = Target {
            account: &account,
            caps: &caps,
            model,
            session: session.as_deref(),
        };
        match forward::forward(app, &inbound, &target, trace).await {
            Outcome::Done(response) => return response,
            Outcome::Unauthorized if !retried_auth.contains(&account.name) => {
                retried_auth.push(account.name.clone());
                if let Err(reason) = app.refreshed(account.clone(), true).await {
                    skip.push(account.name.clone());
                    moved_because = Some(reason);
                }
            }
            Outcome::Unauthorized => {
                skip.push(account.name.clone());
                moved_because = Some(format!("{} unauthorized after refresh", account.name));
            }
            Outcome::Limited(limit) => {
                app.record_limit(&account.name, &limit);
                let because = describe_limit(&account.name, &limit);
                app.log(finish_row(
                    LogRow {
                        account: Some(account.name.clone()),
                        reason: "limit".to_owned(),
                        status: 429,
                        error: Some(because.clone()),
                        ..base.clone()
                    },
                    started,
                ));
                skip.push(account.name.clone());
                moved_because = Some(because);
            }
        }
    }
}

fn reason_text(reason: &Reason, because: Option<&str>, accounts: &[Account]) -> String {
    match reason {
        Reason::Reused => "reused".to_owned(),
        Reason::New => "new".to_owned(),
        Reason::Moved { from } => {
            let why = because.map(str::to_owned).or_else(|| {
                accounts
                    .iter()
                    .find(|a| &a.name == from)
                    .and_then(describe_state)
            });
            format!("moved ({})", why.unwrap_or_else(|| from.clone()))
        }
    }
}

fn no_candidate(api: Api, model: &str, failure: Option<&NoCandidate>) -> ErrorBody {
    match failure {
        Some(NoCandidate::Limited { reset_at }) => {
            let reset_at = *reset_at;
            let message = match reset_at {
                Some(reset_at) => format!(
                    "all accounts for {model} are limited, resets {}",
                    clock::format_utc(reset_at)
                ),
                None => format!("all accounts for {model} are limited"),
            };
            ErrorBody {
                resets_at: reset_at.and_then(|at| u64::try_from(at).ok()),
                ..ErrorBody::new(429, ErrorKind::UsageLimit, message)
            }
        }
        Some(NoCandidate::Unsupported(feature)) => {
            ErrorBody::rejected(&catalog::rejection(*feature), api)
        }
        Some(NoCandidate::UnknownModel) | None => ErrorBody::new(
            404,
            ErrorKind::NotFound,
            format!("model not found: {model}"),
        ),
    }
}

fn reject(app: &App, api: Api, body: &ErrorBody, base: LogRow, started: Instant) -> Response<Body> {
    let trace = Trace {
        row: LogRow {
            reason: "rejected".to_owned(),
            status: body.status,
            error: Some(body.message.clone()),
            ..base
        },
        started,
        gw_headers: Vec::new(),
    };
    let response = error_response(api, body, &trace);
    app.log(finish_row(trace.row, trace.started));
    response
}

fn blank_trace() -> Trace {
    Trace {
        row: LogRow::default(),
        started: Instant::now(),
        gw_headers: Vec::new(),
    }
}

fn unauthorized() -> ErrorBody {
    ErrorBody::new(401, ErrorKind::Authentication, "invalid gw API key")
}

fn internal(error: &dyn std::fmt::Display) -> ErrorBody {
    ErrorBody::new(500, ErrorKind::Server, error.to_string())
}

async fn authenticate(app: &App, headers: &HeaderMap) -> Option<String> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })?;
    let hash = key::hash(presented.trim());
    app.db(move |store| store.key_name(&hash))
        .await
        .ok()
        .flatten()
}

fn session_key(headers: &HeaderMap) -> Option<String> {
    SESSION_HEADERS.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value: &HeaderValue| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn api_name(api: Api) -> &'static str {
    match api {
        Api::Messages => "messages",
        Api::Responses => "responses",
        Api::Chat => "chat",
    }
}
