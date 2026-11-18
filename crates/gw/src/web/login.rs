use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tokio::sync::oneshot;

use super::pages::{Link, Tab, nav};
use super::{Web, accounts, html, see_other};
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
    prompt: Prompt,
    paste: Option<oneshot::Sender<String>>,
    outcome: Outcome,
}

#[derive(Debug, Clone)]
enum Outcome {
    Waiting,
    Done(String),
    Failed(String),
}

#[derive(Template)]
#[template(path = "admin/sign_in.html")]
struct SignInPage {
    title: &'static str,
    tabs: Vec<Link>,
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
    waiting: bool,
    done: Option<String>,
    failed: Option<String>,
    sent: bool,
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
                prompt: session.prompt().clone(),
                paste: Some(sender),
                outcome: Outcome::Waiting,
            },
        );
    }
    let task_web = web.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        let outcome = match session.complete(async { receiver.await.ok() }).await {
            Ok(secret) => match finish(&task_web, kind, target, secret).await {
                Ok(message) => Outcome::Done(message),
                Err(error) => Outcome::Failed(format!("{error:#}")),
            },
            Err(error) => Outcome::Failed(error.to_string()),
        };
        if let Some(login) = task_web.logins().get_mut(&task_id) {
            login.outcome = outcome;
        }
    });
    Ok(id)
}

async fn finish(web: &Web, kind: Kind, target: Target, secret: Secret) -> anyhow::Result<String> {
    match target {
        Target::New(name) => {
            ops::account::add(&web.0.home, name.clone(), kind, secret).await?;
            Ok(accounts::added(web, &name).await)
        }
        Target::Again(name) => {
            ops::account::relogin(&web.0.home, &name, secret).await?;
            Ok(format!("{name} is signed in again."))
        }
    }
}

pub(super) async fn page(State(web): State<Web>, Path(id): Path<String>) -> Response {
    let logins = web.logins();
    let Some(login) = logins.get(&id) else {
        return see_other("/accounts");
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
            "Open the link and sign in. If the page you land on does not load, paste its address here.",
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
        title: "Sign in",
        tabs: nav(Tab::Accounts),
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
    see_other(&format!("/logins/{id}"))
}

pub(super) async fn state(State(web): State<Web>, Path(id): Path<String>) -> Response {
    let logins = web.logins();
    let Some(login) = logins.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let part = state_part(&id, login);
    drop(logins);
    html(StatusCode::OK, &part)
}

fn state_part(id: &str, login: &Login) -> StatePart {
    let (waiting, done, failed) = match &login.outcome {
        Outcome::Waiting => (true, None, None),
        Outcome::Done(message) => (false, Some(message.clone()), None),
        Outcome::Failed(message) => (false, None, Some(message.clone())),
    };
    StatePart {
        id: id.to_owned(),
        name: login.name.clone(),
        waiting,
        done,
        failed,
        sent: login.paste.is_none(),
    }
}

impl Web {
    fn logins(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Login>> {
        // A panic while holding the lock leaves the map itself intact, so keep using it.
        self.0
            .logins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
