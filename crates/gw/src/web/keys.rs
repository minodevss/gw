use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;
use url::Url;

use super::pages::{Link, Tab, nav};
use super::{Web, failure, html, origin, see_other};
use crate::clock;
use crate::ops;
use crate::ops::key::Snippet;

#[derive(Template)]
#[template(path = "admin/key_new.html")]
struct NewPage {
    title: &'static str,
    tabs: Vec<Link>,
    name: String,
    url: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/key_issued.html")]
struct IssuedPage {
    title: &'static str,
    tabs: Vec<Link>,
    name: String,
    key: String,
    snippets: Vec<Snippet>,
}

#[derive(Template)]
#[template(path = "admin/key_revoke.html")]
struct RevokePage {
    title: &'static str,
    tabs: Vec<Link>,
    name: String,
    error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct NewForm {
    name: String,
    url: String,
}

#[derive(Deserialize)]
pub(super) struct ConfirmForm {
    confirm: String,
}

pub(super) async fn new_form(headers: HeaderMap) -> Response {
    new_page(
        String::new(),
        origin(&headers).unwrap_or_default(),
        None,
        StatusCode::OK,
    )
}

pub(super) async fn create(State(web): State<Web>, Form(form): Form<NewForm>) -> Response {
    let name = form.name.trim().to_owned();
    let url = form.url.trim().trim_end_matches('/').to_owned();
    let invalid = |message: String| {
        new_page(
            name.clone(),
            url.clone(),
            Some(message),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    };
    let taken: Vec<String> = match ops::with_store(&web.0.home, |store| store.keys()).await {
        Ok(keys) => keys.into_iter().map(|key| key.name).collect(),
        Err(error) => return failure(&error),
    };
    if let Err(error) = ops::name::check(&name, &taken) {
        return invalid(format!("Name: {error}"));
    }
    if !Url::parse(&url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
        return invalid("Enter the http or https address clients use to reach gw.".to_owned());
    }
    let (key_name, base) = (name.clone(), url.clone());
    let issued = ops::with_store(&web.0.home, move |store| {
        ops::key::create(store, &key_name, &base, clock::now())
            .map_err(|error| crate::store::Error::Invalid(error.to_string()))
    })
    .await;
    match issued {
        Ok(issued) => html(
            StatusCode::OK,
            &IssuedPage {
                title: "Key created",
                tabs: nav(Tab::Keys),
                name,
                key: issued.key,
                snippets: issued.snippets,
            },
        ),
        Err(error) => invalid(format!("{error:#}")),
    }
}

pub(super) async fn confirm_revoke(Path(name): Path<String>) -> Response {
    revoke_page(name, None, StatusCode::OK)
}

pub(super) async fn revoke(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    if form.confirm.trim() != name {
        return revoke_page(
            name.clone(),
            Some(format!("Type {name} to revoke it.")),
            StatusCode::UNPROCESSABLE_ENTITY,
        );
    }
    let wanted = name.clone();
    match ops::with_store(&web.0.home, move |store| store.revoke_key(&wanted)).await {
        Ok(true) => see_other("/keys"),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => failure(&error),
    }
}

fn new_page(name: String, url: String, error: Option<String>, status: StatusCode) -> Response {
    html(
        status,
        &NewPage {
            title: "Create key",
            tabs: nav(Tab::Keys),
            name,
            url,
            error,
        },
    )
}

fn revoke_page(name: String, error: Option<String>, status: StatusCode) -> Response {
    html(
        status,
        &RevokePage {
            title: "Revoke key",
            tabs: nav(Tab::Keys),
            name,
            error,
        },
    )
}
