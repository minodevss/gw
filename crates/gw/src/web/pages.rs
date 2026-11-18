use askama::Template;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse as _, Response};

use super::{Web, failure, html, see_other};
use crate::account::Account;
use crate::clock;
use crate::display::{Health, PROVIDERS, health, kind_label, provider_title, rank, window_label};
use crate::format;
use crate::ops::snapshot::{self, ModelInfo, Server, Snapshot};
use crate::select::{self, NoCandidate};
use crate::store::LogRow;

const ACTIVITY_ROWS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Accounts,
    Activity,
    Models,
    Keys,
}

const TABS: [Tab; 4] = [Tab::Accounts, Tab::Activity, Tab::Models, Tab::Keys];

impl Tab {
    fn parse(path: &str) -> Option<Self> {
        TABS.into_iter().find(|tab| tab.path() == path)
    }

    fn path(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::Activity => "activity",
            Self::Models => "models",
            Self::Keys => "keys",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Accounts => "Accounts",
            Self::Activity => "Activity",
            Self::Models => "Models",
            Self::Keys => "Keys",
        }
    }

    fn live(self, snapshot: &Snapshot) -> askama::Result<String> {
        let status = Status::new(snapshot);
        match self {
            Self::Accounts => AccountsLive {
                status,
                groups: account_groups(snapshot),
            }
            .render(),
            Self::Activity => ActivityLive {
                status,
                rows: activity_rows(snapshot),
            }
            .render(),
            Self::Models => ModelsLive {
                status,
                rows: model_rows(snapshot),
            }
            .render(),
            Self::Keys => KeysLive {
                status,
                rows: key_rows(snapshot),
            }
            .render(),
        }
    }
}

pub(super) struct Link {
    pub(super) label: &'static str,
    pub(super) path: &'static str,
    pub(super) current: bool,
}

pub(super) fn nav(current: Tab) -> Vec<Link> {
    TABS.into_iter()
        .map(|tab| Link {
            label: tab.label(),
            path: tab.path(),
            current: tab == current,
        })
        .collect()
}

#[derive(Template)]
#[template(path = "admin/page.html")]
struct Page {
    title: &'static str,
    tabs: Vec<Link>,
    live: String,
}

struct Status {
    server: String,
    running: bool,
    attention: String,
}

impl Status {
    fn new(snapshot: &Snapshot) -> Self {
        let (server, running) = match snapshot.server() {
            Server::Running(listen) => (format!("running on {listen}"), true),
            Server::Stopped => ("not running".to_owned(), false),
        };
        let attention = snapshot
            .accounts
            .iter()
            .filter(|account| !matches!(health(account, snapshot.now), Health::Ready))
            .count();
        let attention = match attention {
            0 => String::new(),
            1 => "1 account needs attention".to_owned(),
            many => format!("{many} accounts need attention"),
        };
        Self {
            server,
            running,
            attention,
        }
    }
}

#[derive(Template)]
#[template(path = "admin/accounts.html")]
struct AccountsLive {
    status: Status,
    groups: Vec<AccountGroup>,
}

struct AccountGroup {
    title: &'static str,
    rows: Vec<AccountRow>,
}

pub(super) struct AccountRow {
    pub(super) name: String,
    pub(super) kind: &'static str,
    pub(super) state: String,
    pub(super) tone: &'static str,
    pub(super) windows: Vec<WindowCell>,
    pub(super) as_of: Stamp,
}

pub(super) struct WindowCell {
    pub(super) label: String,
    pub(super) percent: i64,
    pub(super) resets: String,
}

#[derive(Template)]
#[template(path = "admin/activity.html")]
struct ActivityLive {
    status: Status,
    rows: Vec<ActivityRow>,
}

struct ActivityRow {
    at: Stamp,
    key: String,
    model: String,
    account: String,
    reason: String,
    status: u16,
    failed: bool,
    input: String,
    output: String,
    took: String,
    error: String,
}

#[derive(Template)]
#[template(path = "admin/models.html")]
struct ModelsLive {
    status: Status,
    rows: Vec<ModelRow>,
}

struct ModelRow {
    id: String,
    accounts: String,
    context: String,
    max_output: String,
    efforts: String,
    next: String,
    blocked: bool,
}

#[derive(Template)]
#[template(path = "admin/keys.html")]
struct KeysLive {
    status: Status,
    rows: Vec<KeyRow>,
}

struct KeyRow {
    name: String,
    created: Stamp,
    last_used: Stamp,
}

// A relative time for reading at a glance and the exact UTC time on hover; the server's zone says
// nothing about where the reader is.
pub(super) struct Stamp {
    pub(super) text: String,
    pub(super) utc: String,
}

impl Stamp {
    pub(super) fn ago(at: Option<i64>, now: i64) -> Self {
        match at {
            Some(at) => Self {
                text: format::ago(now - at),
                utc: clock::format_utc(at),
            },
            None => Self {
                text: "never".to_owned(),
                utc: String::new(),
            },
        }
    }
}

pub(super) async fn home() -> Response {
    see_other("/accounts")
}

pub(super) async fn page(State(web): State<Web>, Path(tab): Path<String>) -> Response {
    let Some(tab) = Tab::parse(&tab) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let snapshot = match snapshot::load(&web.0.home).await {
        Ok(snapshot) => snapshot,
        Err(error) => return failure(&error),
    };
    match tab.live(&snapshot) {
        Ok(live) => html(
            StatusCode::OK,
            &Page {
                title: tab.label(),
                tabs: nav(tab),
                live,
            },
        ),
        Err(error) => failure(&error),
    }
}

pub(super) async fn live(State(web): State<Web>, Path(tab): Path<String>) -> Response {
    let Some(tab) = Tab::parse(&tab) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let rendered = match snapshot::load(&web.0.home).await {
        Ok(snapshot) => tab.live(&snapshot),
        Err(error) => return failure(&error),
    };
    match rendered {
        Ok(body) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response(),
        Err(error) => failure(&error),
    }
}

fn account_groups(snapshot: &Snapshot) -> Vec<AccountGroup> {
    PROVIDERS
        .into_iter()
        .filter_map(|provider| {
            let mut accounts: Vec<&Account> = snapshot
                .accounts
                .iter()
                .filter(|account| account.kind.provider() == provider)
                .collect();
            if accounts.is_empty() {
                return None;
            }
            accounts.sort_by(|a, b| {
                rank(a, snapshot.now)
                    .cmp(&rank(b, snapshot.now))
                    .then_with(|| select::order(a, b))
            });
            Some(AccountGroup {
                title: provider_title(provider),
                rows: accounts
                    .into_iter()
                    .map(|account| account_row(account, snapshot.now))
                    .collect(),
            })
        })
        .collect()
}

pub(super) fn account_row(account: &Account, now: i64) -> AccountRow {
    let (state, tone) = match health(account, now) {
        Health::Ready => ("Ready".to_owned(), "ok"),
        Health::Limited { reset_at, .. } => (
            format!("Limited, back in {}", format::remaining(reset_at - now)),
            "warn",
        ),
        Health::SignedOut(reason) => (format!("Sign in needed: {reason}"), "bad"),
    };
    let windows = account
        .usage
        .iter()
        .flat_map(|usage| &usage.windows)
        .map(|window| WindowCell {
            label: window_label(&window.label),
            percent: format::percent(window.used),
            resets: window.reset_at.map_or_else(String::new, |at| {
                format!("resets in {}", format::remaining(at - now))
            }),
        })
        .collect();
    AccountRow {
        name: account.name.clone(),
        kind: kind_label(account.kind),
        state,
        tone,
        windows,
        as_of: Stamp::ago(account.usage_at, now),
    }
}

fn activity_rows(snapshot: &Snapshot) -> Vec<ActivityRow> {
    snapshot
        .log
        .iter()
        .rev()
        .take(ACTIVITY_ROWS)
        .map(|row| activity_row(row, snapshot.now))
        .collect()
}

fn activity_row(row: &LogRow, now: i64) -> ActivityRow {
    let tokens = |value: Option<i64>| value.map_or_else(|| "—".to_owned(), format::compact);
    let input = match (row.input, row.cache_read) {
        (Some(input), Some(cached)) if cached > 0 => {
            format!(
                "{} ({} cached)",
                format::compact(input),
                format::compact(cached)
            )
        }
        (input, _) => tokens(input),
    };
    ActivityRow {
        at: Stamp::ago(Some(row.ts), now),
        key: row.key.clone().unwrap_or_default(),
        model: row.model.clone(),
        account: row.account.clone().unwrap_or_else(|| "—".to_owned()),
        reason: row.reason.clone(),
        status: row.status,
        failed: row.status >= 400,
        input,
        output: tokens(row.output),
        took: format!(
            "{:.1}s",
            f64::from(u32::try_from(row.duration_ms).unwrap_or(u32::MAX)) / 1000.0
        ),
        error: row.error.clone().unwrap_or_default(),
    }
}

fn model_rows(snapshot: &Snapshot) -> Vec<ModelRow> {
    snapshot
        .models
        .iter()
        .map(|info| model_row(info, snapshot.now))
        .collect()
}

fn model_row(info: &ModelInfo, now: i64) -> ModelRow {
    let view = &info.view;
    let (next, blocked) = match &info.next {
        Ok(choice) => (choice.account.clone(), false),
        Err(NoCandidate::Limited { reset_at }) => (
            reset_at.map_or_else(
                || "all accounts limited".to_owned(),
                |at| format!("all limited, back in {}", format::remaining(at - now)),
            ),
            true,
        ),
        Err(NoCandidate::UnknownModel | NoCandidate::Unsupported(_)) => {
            ("no account can serve it".to_owned(), true)
        }
    };
    ModelRow {
        id: view.id.clone(),
        accounts: view.accounts.join(", "),
        context: view.context.map_or_else(
            || "—".to_owned(),
            |v| format::compact(i64::try_from(v).unwrap_or(i64::MAX)),
        ),
        max_output: view
            .max_output
            .map_or_else(|| "—".to_owned(), |v| format::compact(i64::from(v))),
        efforts: view
            .efforts
            .as_ref()
            .map_or_else(|| "—".to_owned(), |efforts| efforts.join(" ")),
        next,
        blocked,
    }
}

fn key_rows(snapshot: &Snapshot) -> Vec<KeyRow> {
    snapshot
        .keys
        .iter()
        .map(|key| KeyRow {
            name: key.name.clone(),
            created: Stamp::ago(Some(key.created_at), snapshot.now),
            last_used: Stamp::ago(key.last_used, snapshot.now),
        })
        .collect()
}
