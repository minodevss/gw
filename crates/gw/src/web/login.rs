use std::collections::HashMap;
use std::sync::MutexGuard;

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

use super::frame::{self, Ctx, Flash, Frame, Tab};
use super::{PREFIX, Web, accounts, html, redirect_with, see_other, segment};
use crate::account::Secret;
use crate::clock;
use crate::ops;
use crate::provider::{Kind, Prompt};

// Longer than every flow's own limit (loopback 10 minutes, device code 15), so a flow ends by
// itself before its entry is dropped.
const KEEP_SECS: i64 = 20 * 60;

pub(super) enum Target {
    New(String),
    Again(String),
}

impl Target {
    fn name(&self) -> &str {
        match self {
            Self::New(name) | Self::Again(name) => name,
        }
    }
}

pub(super) struct Login {
    created_at: i64,
    name: String,
    kind: Kind,
    again: bool,
    prompt: Prompt,
    paste: Option<oneshot::Sender<String>>,
    outcome: Outcome,
    task: Option<AbortHandle>,
}

impl Drop for Login {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Debug, Clone)]
enum Outcome {
    Waiting,
    Done(Flash),
    Failed(String),
}

#[derive(Template)]
#[template(path = "admin/sign_in.html")]
struct SignInPage {
    frame: Frame,
    started: String,
    waiting: bool,
    id: String,
    name: String,
    link: String,
    instruction: &'static str,
    field: Option<&'static str>,
    code: Option<String>,
    state: String,
}

#[derive(Template)]
#[template(path = "admin/sign_in_state.html")]
struct StatePart {
    id: String,
    name: String,
    path: String,
    phase: Phase,
}

enum Phase {
    Waiting { sent: bool },
    Done { message: String, warn: bool },
    Failed(String),
    Gone,
}

#[derive(Deserialize)]
pub(super) struct PasteForm {
    value: String,
}

pub(super) async fn start(web: &Web, kind: Kind, target: Target) -> anyhow::Result<String> {
    let session = ops::account::begin_login(&web.0.home, &web.0.http, kind).await?;
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    let id = URL_SAFE_NO_PAD.encode(bytes);
    let (sender, receiver) = oneshot::channel();
    {
        let now = clock::now();
        let mut logins = web.logins();
        logins.retain(|_, login| now - login.created_at < KEEP_SECS);
        logins.insert(
            id.clone(),
            Login {
                created_at: now,
                name: target.name().to_owned(),
                kind,
                again: matches!(target, Target::Again(_)),
                prompt: session.prompt().clone(),
                paste: Some(sender),
                outcome: Outcome::Waiting,
                task: None,
            },
        );
    }
    let task_web = web.clone();
    let task_id = id.clone();
    let task = tokio::spawn(async move {
        let outcome = match session.complete(async { receiver.await.ok() }).await {
            Ok(secret) => match finish(&task_web, kind, target, secret).await {
                Ok(flash) => Outcome::Done(flash),
                Err(error) => Outcome::Failed(format!("{error:#}")),
            },
            Err(error) => Outcome::Failed(error.to_string()),
        };
        if let Some(login) = task_web.logins().get_mut(&task_id) {
            login.outcome = outcome;
        }
    });
    if let Some(login) = web.logins().get_mut(&id) {
        login.task = Some(task.abort_handle());
    }
    Ok(id)
}

async fn finish(web: &Web, kind: Kind, target: Target, secret: Secret) -> anyhow::Result<Flash> {
    match target {
        Target::New(name) => {
            ops::account::add(&web.0.home, name.clone(), kind, secret).await?;
            Ok(accounts::added(web, &name).await)
        }
        Target::Again(name) => {
            ops::account::relogin(&web.0.home, &name, secret).await?;
            Ok(Flash::done(format!("{name} is signed in again.")))
        }
    }
}

pub(super) async fn page(State(web): State<Web>, ctx: Ctx, Path(id): Path<String>) -> Response {
    let logins = web.logins();
    let Some(login) = logins.get(&id) else {
        drop(logins);
        return redirect_with(
            "/accounts",
            &Flash::failed("That sign-in is no longer running. Start it again from the account."),
        );
    };
    let (link, instruction, field, code) = match &login.prompt {
        Prompt::PasteCode { url } => (
            url.clone(),
            "Open the link and sign in, then paste the code you get.",
            Some("Code"),
            None,
        ),
        Prompt::Loopback { url } => (
            url.clone(),
            "Open the link and sign in. The browser then goes to a 127.0.0.1 address, which fails to load unless this browser runs on the gw machine. That is expected: copy that address from the address bar and paste it here.",
            Some("Address"),
            None,
        ),
        Prompt::Device {
            verification_uri,
            user_code,
            ..
        } => (
            verification_uri.clone(),
            "Open the link on any device and enter this code.",
            None,
            Some(user_code.clone()),
        ),
    };
    let page = SignInPage {
        frame: ctx.frame(Some(Tab::Accounts), format!("Sign in {}", login.name)),
        started: ctx.zone.clock(ctx.now).when(login.created_at),
        waiting: matches!(login.outcome, Outcome::Waiting),
        id: id.clone(),
        name: login.name.clone(),
        link,
        instruction,
        field: field.filter(|_| login.paste.is_some()),
        code,
        state: match state_part(&id, login).render() {
            Ok(state) => state,
            Err(error) => return super::failure(&error),
        },
    };
    drop(logins);
    html(StatusCode::OK, &page)
}

pub(super) async fn paste(
    State(web): State<Web>,
    Path(id): Path<String>,
    Form(form): Form<PasteForm>,
) -> Response {
    let value = form.value.trim().to_owned();
    if !value.is_empty()
        && let Some(sender) = web
            .logins()
            .get_mut(&id)
            .and_then(|login| login.paste.take())
    {
        // The flow may have ended by itself (callback or expiry); then there is nobody to tell.
        sender.send(value).unwrap_or_default();
    }
    see_other(&format!("/logins/{}", segment(&id)))
}

pub(super) async fn state(
    State(web): State<Web>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let logins = web.logins();
    let Some(login) = logins.get(&id) else {
        drop(logins);
        return html(
            StatusCode::OK,
            &StatePart {
                id,
                name: String::new(),
                path: String::new(),
                phase: Phase::Gone,
            },
        );
    };
    let part = state_part(&id, login);
    drop(logins);
    if let Phase::Done { message, warn } = &part.phase
        && headers.contains_key("hx-request")
    {
        let flash = if *warn {
            Flash::failed(message.clone())
        } else {
            Flash::done(message.clone())
        };
        let target = format!("{PREFIX}/account/{}", part.path);
        return frame::with_flash(
            ([("hx-redirect", target)], StatusCode::OK).into_response(),
            &flash,
        );
    }
    html(StatusCode::OK, &part)
}

fn state_part(id: &str, login: &Login) -> StatePart {
    StatePart {
        id: id.to_owned(),
        name: login.name.clone(),
        path: segment(&login.name),
        phase: match &login.outcome {
            Outcome::Waiting => Phase::Waiting {
                sent: login.paste.is_none(),
            },
            Outcome::Done(flash) => Phase::Done {
                message: flash.text.clone(),
                warn: flash.failed_tone(),
            },
            Outcome::Failed(message) => Phase::Failed(message.clone()),
        },
    }
}

pub(super) async fn cancel(State(web): State<Web>, Path(id): Path<String>) -> Response {
    let removed = web.logins().remove(&id);
    match removed {
        Some(login) if login.again => redirect_with(
            &format!("/account/{}", segment(&login.name)),
            &Flash::done("Cancelled the sign-in."),
        ),
        Some(_) => redirect_with("/accounts/new", &Flash::done("Cancelled the sign-in.")),
        None => redirect_with(
            "/accounts",
            &Flash::failed("That sign-in is no longer running."),
        ),
    }
}

pub(super) async fn retry(State(web): State<Web>, Path(id): Path<String>) -> Response {
    let found = web
        .logins()
        .remove(&id)
        .map(|login| (login.kind, login.again, login.name.clone()));
    let Some((kind, again, name)) = found else {
        return see_other("/accounts");
    };
    let target = if again {
        Target::Again(name)
    } else {
        Target::New(name)
    };
    match start(&web, kind, target).await {
        Ok(id) => see_other(&format!("/logins/{id}")),
        Err(error) => redirect_with(
            "/accounts",
            &Flash::failed(format!("Could not start the sign-in: {error:#}")),
        ),
    }
}

impl Web {
    fn logins(&self) -> MutexGuard<'_, HashMap<String, Login>> {
        super::lock(&self.0.logins)
    }
}
