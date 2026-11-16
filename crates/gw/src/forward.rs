use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode, header};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use proto::collect::Collector;
use proto::error::{ErrorBody, ErrorKind};
use proto::passthrough::{Peek, Tap};
use proto::reject::Rejection;
use proto::{Api, EncodeOptions, MaxTokensField, Note, ir, sse};
use serde_json::Value;

use crate::account::{Account, Secret, State};
use crate::catalog::Capabilities;
use crate::codec::{self, StreamDecoder, StreamEncoder};
use crate::provider::{Limit, Method, Provider, limit_signal, usage_from_headers};
use crate::serve::App;
use crate::store::LogRow;

const PING_EVERY: Duration = Duration::from_secs(15);
const ERROR_BODY_LIMIT: usize = 64 * 1024;

pub(crate) struct Target<'a> {
    pub(crate) account: &'a Account,
    pub(crate) caps: &'a Capabilities,
    pub(crate) model: &'a str,
    pub(crate) session: Option<&'a str>,
}

pub(crate) struct Inbound<'a> {
    pub(crate) api: Api,
    pub(crate) path: &'a str,
    pub(crate) headers: &'a HeaderMap,
    pub(crate) body: &'a Bytes,
    pub(crate) peek: &'a Peek,
}

pub(crate) enum Outcome {
    Limited(Limit),
    Unauthorized,
    Done(Response<Body>),
}

pub(crate) struct Trace {
    pub(crate) row: LogRow,
    pub(crate) started: Instant,
    pub(crate) gw_headers: Vec<(HeaderName, String)>,
}

struct Prepared {
    url: String,
    headers: HeaderMap,
    body: Bytes,
    upstream: Api,
    relay: bool,
    notes: Vec<Note>,
}

pub(crate) async fn forward(
    app: &App,
    inbound: &Inbound<'_>,
    target: &Target<'_>,
    mut trace: Trace,
) -> Outcome {
    let prepared = match prepare(inbound, target) {
        Ok(prepared) => prepared,
        Err(rejection) => {
            let body = ErrorBody::rejected(&rejection, inbound.api);
            return Outcome::Done(finish_error(app, inbound.api, &body, trace));
        }
    };
    let notes: Vec<&str> = prepared.notes.iter().map(|note| note.as_str()).collect();
    if !notes.is_empty() {
        let joined = notes.join("; ");
        trace.row.note = Some(joined.clone());
        trace.gw_headers.push((X_GW_NOTE, joined));
    }
    let request = app
        .http()
        .post(&prepared.url)
        .headers(prepared.headers)
        .body(prepared.body);
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let body = ErrorBody::new(
                502,
                ErrorKind::Server,
                format!("upstream unreachable: {error}"),
            );
            return Outcome::Done(finish_error(app, inbound.api, &body, trace));
        }
    };
    let status = response.status();
    let headers = response.headers().clone();
    let kind = target.account.kind;
    if status == StatusCode::UNAUTHORIZED && kind.refreshes() {
        return Outcome::Unauthorized;
    }
    if !status.is_success() {
        let bytes = read_limited(response).await;
        let json: Option<Value> = serde_json::from_slice(&bytes).ok();
        if let Some(limit) = limit_signal(kind, status.as_u16(), &headers, json.as_ref()) {
            return Outcome::Limited(limit);
        }
        trace.row.status = status.as_u16();
        trace.row.error = Some(excerpt(&bytes));
        let response = if prepared.relay {
            relay_error(status, &headers, bytes, &trace)
        } else {
            let body = translate_error(
                inbound.api,
                prepared.upstream,
                status,
                json.as_ref(),
                &bytes,
            );
            error_response(inbound.api, &body, &trace)
        };
        app.log(finish_row(trace.row, trace.started));
        return Outcome::Done(response);
    }
    if let Some(usage) = usage_from_headers(kind, &headers) {
        app.record_usage(&target.account.name, usage);
    }
    trace.row.status = status.as_u16();
    let client_stream = inbound.peek.stream;
    let upstream_stream = is_event_stream(&headers);
    let response = if prepared.relay {
        passthrough(app, inbound.api, status, &headers, response, target, trace)
    } else if client_stream {
        transcode_stream(app, inbound.api, prepared.upstream, response, target, trace)
    } else if upstream_stream {
        collect_stream(app, inbound.api, prepared.upstream, response, trace).await
    } else {
        translate_body(app, inbound.api, prepared.upstream, response, trace).await
    };
    Outcome::Done(response)
}

fn prepare(inbound: &Inbound<'_>, target: &Target<'_>) -> Result<Prepared, Rejection> {
    let account = target.account;
    let upstream = upstream_api(account, inbound.api);
    let url = format!(
        "{}{}",
        base_url(account),
        path_for(account, upstream, inbound.path)
    );
    let mut headers = HeaderMap::new();
    let needs_stream = account.kind.method() == Method::Siwc && !inbound.peek.stream;
    let relay = upstream == inbound.api && !needs_stream;
    let (body, notes) = if relay {
        copy_request_headers(inbound.headers, &mut headers);
        let body = proto::passthrough::splice_model(inbound.body, target.model)
            .map_err(|error| Rejection::unsupported(error.to_string()))?;
        (body, Vec::new())
    } else {
        let mut request = codec::decode_request(inbound.api, inbound.body)?;
        target.model.clone_into(&mut request.model);
        let options = EncodeOptions {
            cache_key: target.session.map(str::to_owned),
            default_max_output: target.caps.max_output,
            force_stream: account.kind.method() == Method::Siwc,
            max_tokens_field: if account.kind.provider() == Provider::Zai {
                MaxTokensField::MaxTokens
            } else {
                MaxTokensField::MaxCompletionTokens
            },
        };
        let encoded = codec::encode_request(upstream, &request, &options)?;
        let mut notes = encoded.notes;
        if inbound.peek.cache_ttl.is_some() {
            notes.push(Note::CacheTtlDefault);
        }
        notes.sort_unstable();
        notes.dedup();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if upstream == Api::Messages {
            headers.insert(ANTHROPIC_VERSION, HeaderValue::from_static("2023-06-01"));
        }
        (encoded.body, notes)
    };
    account.kind.apply_auth(
        account.secret.bearer(),
        account.secret.account_id(),
        &mut headers,
    );
    Ok(Prepared {
        url,
        headers,
        body,
        upstream,
        relay,
        notes,
    })
}

pub(crate) fn upstream_api(account: &Account, client: Api) -> Api {
    if let Secret::Custom { api, .. } = &account.secret {
        return parse_api(api).unwrap_or(client);
    }
    let apis = account.kind.apis();
    if apis.contains(&client) {
        client
    } else {
        apis.first().copied().unwrap_or(client)
    }
}

pub(crate) fn parse_api(value: &str) -> Option<Api> {
    match value {
        "messages" => Some(Api::Messages),
        "responses" => Some(Api::Responses),
        "chat" => Some(Api::Chat),
        _ => None,
    }
}

fn base_url(account: &Account) -> String {
    match &account.secret {
        Secret::Custom { url, .. } => url.trim_end_matches('/').to_owned(),
        Secret::Oauth { .. } | Secret::Token { .. } | Secret::Key { .. } => {
            account.kind.base_url().to_owned()
        }
    }
}

fn path_for(account: &Account, upstream: Api, inbound_path: &str) -> String {
    let path = match &account.secret {
        Secret::Custom { .. } => upstream.path(),
        Secret::Oauth { .. } | Secret::Token { .. } | Secret::Key { .. } => {
            account.kind.path(upstream)
        }
    };
    match inbound_path.strip_prefix(Api::Messages.path()) {
        Some(rest) if !rest.is_empty() && upstream == Api::Messages => format!("{path}{rest}"),
        Some(_) | None => path.to_owned(),
    }
}

const ANTHROPIC_VERSION: HeaderName = HeaderName::from_static("anthropic-version");
pub(crate) const X_GW_ACCOUNT: HeaderName = HeaderName::from_static("x-gw-account");
pub(crate) const X_GW_REASON: HeaderName = HeaderName::from_static("x-gw-reason");
const X_GW_NOTE: HeaderName = HeaderName::from_static("x-gw-note");

const DROPPED_REQUEST_HEADERS: [&str; 12] = [
    "host",
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "accept-encoding",
    "authorization",
];

const DROPPED_RESPONSE_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

fn copy_request_headers(from: &HeaderMap, to: &mut HeaderMap) {
    for (name, value) in from {
        let lower = name.as_str();
        if !DROPPED_REQUEST_HEADERS.contains(&lower) && lower != "x-api-key" {
            to.append(name, value.clone());
        }
    }
}

fn copy_response_headers(from: &HeaderMap, to: &mut HeaderMap) {
    for (name, value) in from {
        if !DROPPED_RESPONSE_HEADERS.contains(&name.as_str()) {
            to.append(name, value.clone());
        }
    }
}

fn is_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

async fn read_limited(response: reqwest::Response) -> Bytes {
    let mut out = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        out.extend_from_slice(&chunk);
        if out.len() >= ERROR_BODY_LIMIT {
            out.truncate(ERROR_BODY_LIMIT);
            break;
        }
    }
    Bytes::from(out)
}

fn excerpt(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(300).collect()
}

fn with_gw_headers(builder: http::response::Builder, trace: &Trace) -> http::response::Builder {
    trace.gw_headers.iter().fold(
        builder,
        |builder, (name, value)| match HeaderValue::from_str(value) {
            Ok(value) => builder.header(name, value),
            Err(_) => builder,
        },
    )
}

fn build(builder: http::response::Builder, body: Body) -> Response<Body> {
    builder.body(body).unwrap_or_else(|_| {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response
    })
}

pub(crate) fn error_response(api: Api, body: &ErrorBody, trace: &Trace) -> Response<Body> {
    let status = StatusCode::from_u16(body.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if body.status == 429 && api == Api::Messages {
        builder = builder.header("x-should-retry", "false");
    }
    build(with_gw_headers(builder, trace), Body::from(body.json(api)))
}

fn finish_error(app: &App, api: Api, body: &ErrorBody, mut trace: Trace) -> Response<Body> {
    trace.row.status = body.status;
    trace.row.error = Some(body.message.clone());
    let response = error_response(api, body, &trace);
    app.log(finish_row(trace.row, trace.started));
    response
}

fn relay_error(
    status: StatusCode,
    headers: &HeaderMap,
    bytes: Bytes,
    trace: &Trace,
) -> Response<Body> {
    let mut builder = Response::builder().status(status.as_u16());
    if let Some(map) = builder.headers_mut() {
        copy_response_headers(headers, map);
    }
    build(with_gw_headers(builder, trace), Body::from(bytes))
}

pub(crate) fn finish_row(mut row: LogRow, started: Instant) -> LogRow {
    row.duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    row
}

fn apply_usage(row: &mut LogRow, usage: Option<ir::Usage>) {
    if let Some(usage) = usage {
        let to_i64 = |value: u64| i64::try_from(value).ok();
        row.input = to_i64(usage.input_uncached);
        row.cache_read = to_i64(usage.cache_read);
        row.cache_write = to_i64(usage.cache_write);
        row.output = to_i64(usage.output);
    }
}

fn translate_error(
    client: Api,
    upstream: Api,
    status: StatusCode,
    json: Option<&Value>,
    raw: &[u8],
) -> ErrorBody {
    let error = json.and_then(|json| json.get("error"));
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map_or_else(|| excerpt(raw), str::to_owned);
    if client == Api::Messages
        && let Some(rejection) = classify(upstream, error)
    {
        return ErrorBody::rejected(&rejection, client);
    }
    let kind = match status.as_u16() {
        400 | 413 | 422 => ErrorKind::InvalidRequest,
        401 => ErrorKind::Authentication,
        403 => ErrorKind::Permission,
        404 => ErrorKind::NotFound,
        429 => ErrorKind::RateLimit,
        503 | 529 => ErrorKind::Overloaded,
        _ => ErrorKind::Server,
    };
    ErrorBody::new(status.as_u16(), kind, message)
}

fn classify(upstream: Api, error: Option<&Value>) -> Option<Rejection> {
    let error = error?;
    let code = error.get("code").and_then(|code| match code {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    })?;
    let param = error.get("param").and_then(Value::as_str);
    match (upstream, code.as_str(), param) {
        (Api::Responses | Api::Chat, "context_length_exceeded", _) | (Api::Chat, "1261", _) => {
            Some(Rejection::PromptTooLong)
        }
        (
            Api::Responses,
            "subscription_sharing_unsupported_capability",
            Some("reasoning.effort"),
        ) => Some(Rejection::Effort),
        _ => None,
    }
}

fn passthrough(
    app: &App,
    api: Api,
    status: StatusCode,
    headers: &HeaderMap,
    response: reqwest::Response,
    target: &Target<'_>,
    trace: Trace,
) -> Response<Body> {
    let mut builder = Response::builder().status(status.as_u16());
    if let Some(map) = builder.headers_mut() {
        copy_response_headers(headers, map);
    }
    let builder = with_gw_headers(builder, &trace);
    let mut finisher = Finisher::new(app, target, trace);
    finisher.tap = Some(Tap::new(api, is_event_stream(headers)));
    let stream = futures_util::stream::unfold(
        (response.bytes_stream(), finisher),
        |(mut upstream, mut finisher)| async move {
            match upstream.next().await {
                Some(Ok(chunk)) => {
                    if let Some(tap) = &mut finisher.tap {
                        tap.push(&chunk);
                    }
                    Some((Ok::<Bytes, std::io::Error>(chunk), (upstream, finisher)))
                }
                Some(Err(error)) => {
                    finisher.failure = Some(error.to_string());
                    finisher.finish();
                    None
                }
                None => {
                    finisher.finish();
                    None
                }
            }
        },
    );
    build(builder, Body::from_stream(stream))
}

struct Finisher {
    app: App,
    account: Account,
    trace: Option<Trace>,
    tap: Option<Tap>,
    usage: Option<ir::Usage>,
    error: Option<Value>,
    failure: Option<String>,
}

impl Finisher {
    fn new(app: &App, target: &Target<'_>, trace: Trace) -> Self {
        Self {
            app: app.clone(),
            account: target.account.clone(),
            trace: Some(trace),
            tap: None,
            usage: None,
            error: None,
            failure: None,
        }
    }

    fn finish(&mut self) {
        let Some(trace) = self.trace.take() else {
            return;
        };
        if let Some(tap) = &mut self.tap {
            tap.finish();
            self.usage = self.usage.or_else(|| tap.usage());
            self.error = self.error.take().or_else(|| tap.error());
        }
        let mut row = trace.row;
        apply_usage(&mut row, self.usage);
        if let Some(error) = &self.error {
            row.error = Some(error.to_string().chars().take(300).collect());
            if let Some(limit) =
                limit_signal(self.account.kind, 200, &HeaderMap::new(), Some(error))
            {
                self.app.record_limit(&self.account.name, &limit);
                row.reason = format!("{} (limit mid-stream)", row.reason);
            }
        }
        if let Some(failure) = self.failure.take() {
            row.error = Some(failure);
        }
        self.app.log(finish_row(row, trace.started));
    }
}

impl Drop for Finisher {
    fn drop(&mut self) {
        self.finish();
    }
}

fn transcode_stream(
    app: &App,
    client: Api,
    upstream: Api,
    response: reqwest::Response,
    target: &Target<'_>,
    trace: Trace,
) -> Response<Body> {
    let builder = with_gw_headers(
        Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache"),
        &trace,
    );
    let state = Transcode {
        upstream: Box::pin(response.bytes_stream()),
        parser: sse::Parser::default(),
        decoder: StreamDecoder::new(upstream),
        encoder: StreamEncoder::new(client),
        ticker: tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY),
        finisher: Finisher::new(app, target, trace),
        client,
    };
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        let chunk = state.next_chunk().await?;
        Some((Ok::<Bytes, std::io::Error>(chunk), state))
    })
    .filter(|item| futures_util::future::ready(item.as_ref().is_ok_and(|bytes| !bytes.is_empty())));
    build(builder, Body::from_stream(stream))
}

struct Transcode {
    upstream: std::pin::Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    parser: sse::Parser,
    decoder: StreamDecoder,
    encoder: StreamEncoder,
    ticker: tokio::time::Interval,
    finisher: Finisher,
    client: Api,
}

impl Transcode {
    async fn next_chunk(&mut self) -> Option<Bytes> {
        self.finisher.trace.as_ref()?;
        tokio::select! {
            item = self.upstream.next() => match item {
                Some(Ok(chunk)) => Some(self.convert(&chunk)),
                Some(Err(error)) => {
                    let body = ErrorBody::new(502, ErrorKind::Server, format!("upstream stream failed: {error}"));
                    let mut out = self.encoder.push(&ir::Event::Error(body)).concat();
                    out.extend(self.encoder.finish().concat());
                    self.finish(Some(error.to_string()));
                    Some(Bytes::from(out))
                }
                None => {
                    let out = self.encoder.finish().concat();
                    self.finish(None);
                    Some(Bytes::from(out))
                }
            },
            _ = self.ticker.tick() => Some(self.encoder.ping().unwrap_or_default()),
        }
    }

    fn convert(&mut self, chunk: &[u8]) -> Bytes {
        let mut out = Vec::new();
        for frame in self.parser.push(chunk) {
            match self.decoder.push(&frame) {
                Ok(events) => {
                    for event in events {
                        match &event {
                            ir::Event::Usage(usage) => self.finisher.usage = Some(*usage),
                            ir::Event::Error(body) => {
                                self.finisher.error =
                                    serde_json::from_slice(&body.json(self.client)).ok();
                            }
                            ir::Event::Start { .. }
                            | ir::Event::BlockStart { .. }
                            | ir::Event::TextDelta { .. }
                            | ir::Event::ReasoningDelta { .. }
                            | ir::Event::ReasoningSummaryDelta { .. }
                            | ir::Event::ReasoningOrigin { .. }
                            | ir::Event::ToolArgumentsDelta { .. }
                            | ir::Event::BlockStop { .. }
                            | ir::Event::Stop(_) => {}
                        }
                        for bytes in self.encoder.push(&event) {
                            out.extend_from_slice(&bytes);
                        }
                    }
                }
                Err(error) => {
                    let body =
                        ErrorBody::new(502, ErrorKind::Server, format!("upstream event: {error}"));
                    for bytes in self.encoder.push(&ir::Event::Error(body)) {
                        out.extend_from_slice(&bytes);
                    }
                }
            }
        }
        Bytes::from(out)
    }

    fn finish(&mut self, failure: Option<String>) {
        self.finisher.failure = failure;
        self.finisher.finish();
    }
}

async fn collect_stream(
    app: &App,
    client: Api,
    upstream: Api,
    response: reqwest::Response,
    mut trace: Trace,
) -> Response<Body> {
    let mut parser = sse::Parser::default();
    let mut decoder = StreamDecoder::new(upstream);
    let mut collector = Collector::default();
    let mut stream = response.bytes_stream();
    while let Some(item) = stream.next().await {
        let Ok(chunk) = item else {
            let body = ErrorBody::new(502, ErrorKind::Server, "upstream stream failed");
            return finish_error(app, client, &body, trace);
        };
        for frame in parser.push(&chunk) {
            match decoder.push(&frame) {
                Ok(events) => events.iter().for_each(|event| collector.push(event)),
                Err(error) => {
                    let body =
                        ErrorBody::new(502, ErrorKind::Server, format!("upstream event: {error}"));
                    return finish_error(app, client, &body, trace);
                }
            }
        }
    }
    match collector.finish() {
        Ok(response) => {
            apply_usage(&mut trace.row, Some(response.usage));
            json_response(app, client, &response, trace)
        }
        Err(error) => {
            let body = ErrorBody::new(502, ErrorKind::Server, format!("upstream stream: {error}"));
            finish_error(app, client, &body, trace)
        }
    }
}

async fn translate_body(
    app: &App,
    client: Api,
    upstream: Api,
    response: reqwest::Response,
    mut trace: Trace,
) -> Response<Body> {
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            let body = ErrorBody::new(502, ErrorKind::Server, format!("upstream body: {error}"));
            return finish_error(app, client, &body, trace);
        }
    };
    match codec::decode_response(upstream, &bytes) {
        Ok(response) => {
            apply_usage(&mut trace.row, Some(response.usage));
            json_response(app, client, &response, trace)
        }
        Err(error) => {
            let body = ErrorBody::new(502, ErrorKind::Server, format!("upstream body: {error}"));
            finish_error(app, client, &body, trace)
        }
    }
}

fn json_response(app: &App, client: Api, response: &ir::Response, trace: Trace) -> Response<Body> {
    let builder = with_gw_headers(
        Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "application/json"),
        &trace,
    );
    let body = codec::encode_response(client, response);
    app.log(finish_row(trace.row, trace.started));
    build(builder, Body::from(body))
}

pub(crate) fn describe_limit(account: &str, limit: &Limit) -> String {
    let scope = match &limit.scope {
        crate::provider::Scope::Account => "limit".to_owned(),
        crate::provider::Scope::Family(family) => format!("{family} limit"),
    };
    match limit.reset_at {
        Some(reset_at) => format!(
            "{account} {scope}, reset {}",
            crate::clock::format_utc(reset_at)
        ),
        None => format!("{account} {scope}"),
    }
}

pub(crate) fn describe_state(account: &Account) -> Option<String> {
    match &account.state {
        State::Active => None,
        State::Blocked { scope, reset_at } => Some(describe_limit(
            &account.name,
            &Limit {
                scope: scope.clone(),
                reset_at: Some(*reset_at),
            },
        )),
        State::Inactive { reason } => Some(format!("{} inactive: {reason}", account.name)),
    }
}
