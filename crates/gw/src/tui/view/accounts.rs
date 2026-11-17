use std::cmp::Ordering;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{Env, Filter, draw, empty, heading, panes};
use crate::account::{Account, State as AccountState};
use crate::provider::{Billing, Kind, Method, Provider, Scope, Usage, Window, can_query_usage};
use crate::select;
use crate::tui::action::{Command, Effect, Reply, Tone};
use crate::tui::flow::add_account::AddAccount;
use crate::tui::format;
use crate::tui::keymap::{Context, shortcut};
use crate::tui::modal::{Modal, Rename};
use crate::tui::theme::Theme;
use crate::tui::widget::confirm::Confirm;
use crate::tui::widget::gauge;
use crate::tui::widget::list::{ListState, Row};
use crate::tui::widget::menu::{Item, Menu};

const PROVIDERS: [Provider; 5] = [
    Provider::Anthropic,
    Provider::OpenAi,
    Provider::Zai,
    Provider::Moonshot,
    Provider::Custom,
];
const BAR_CELLS: usize = 10;
const STATUS_CELLS: usize = BAR_CELLS + 2 + 4 + 2 + 6;
const STALE_SECS: i64 = 3600;
const SESSION_WINDOW_SECS: i64 = 1800;
const PAGE: isize = 10;

pub(crate) fn provider_title(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "Anthropic",
        Provider::OpenAi => "OpenAI",
        Provider::Zai => "Z.ai",
        Provider::Moonshot => "Moonshot",
        Provider::Custom => "Custom",
    }
}

pub(crate) fn kind_label(kind: Kind) -> &'static str {
    match (kind.provider(), kind.method()) {
        (Provider::Anthropic, Method::Oauth) => "Claude subscription",
        (Provider::Anthropic, Method::SetupToken) => "Claude setup token",
        (Provider::OpenAi, Method::Siwc) => "ChatGPT subscription",
        (Provider::OpenAi, Method::Codex) => "ChatGPT subscription (device code)",
        (Provider::Zai, Method::Plan) => "GLM coding plan",
        (Provider::Moonshot, Method::Plan) => "Kimi coding plan",
        (Provider::Custom, _) => "Custom endpoint",
        (
            Provider::Anthropic | Provider::OpenAi | Provider::Zai | Provider::Moonshot,
            Method::Oauth
            | Method::SetupToken
            | Method::Siwc
            | Method::Codex
            | Method::Plan
            | Method::Key
            | Method::Custom,
        ) => match kind.billing() {
            Billing::Api => "API key",
            Billing::Sub => "Subscription",
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Health<'a> {
    Ready,
    Limited { reset_at: i64, scope: &'a Scope },
    SignedOut(&'a str),
}

pub(crate) fn health(account: &Account, now: i64) -> Health<'_> {
    match &account.state {
        AccountState::Active => Health::Ready,
        AccountState::Blocked { reset_at, .. } if *reset_at <= now => Health::Ready,
        AccountState::Blocked { scope, reset_at } => Health::Limited {
            reset_at: *reset_at,
            scope,
        },
        AccountState::Inactive { reason } => Health::SignedOut(reason),
    }
}

pub(crate) fn window_label(label: &str) -> String {
    let (span, family) = label
        .split_once('_')
        .map_or((label, None), |(span, family)| (span, Some(family)));
    let split = span
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(span.len());
    let (count, unit) = span.split_at(split);
    let unit = match (unit, count == "1") {
        ("s", true) => "second",
        ("s", false) => "seconds",
        ("m", true) => "minute",
        ("m", false) => "minutes",
        ("h", true) => "hour",
        ("h", false) => "hours",
        ("d", true) => "day",
        ("d", false) => "days",
        ("mo", true) => "month",
        ("mo", false) => "months",
        _ => return label.to_owned(),
    };
    let duration = format!("{count} {unit}");
    match family {
        Some(family) => format!("{duration} {family}"),
        None => duration,
    }
}

fn tightest(usage: &Usage) -> Option<&Window> {
    usage
        .windows
        .iter()
        .max_by(|a, b| a.used.partial_cmp(&b.used).unwrap_or(Ordering::Equal))
}

#[derive(Debug, Clone, Copy)]
enum Entry<'a> {
    Header(Provider),
    Spacer,
    Account(&'a Account),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    list: ListState,
    filter: Filter,
    detail: bool,
}

fn entries<'a>(env: &Env<'a>, filter: &Filter) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    for provider in PROVIDERS {
        let mut group: Vec<&Account> = env
            .snapshot
            .accounts
            .iter()
            .filter(|a| a.kind.provider() == provider)
            .filter(|a| {
                filter.text().is_empty()
                    || filter.matches(&format!(
                        "{} {} {}",
                        a.name,
                        provider_title(provider),
                        kind_label(a.kind)
                    ))
            })
            .collect();
        if group.is_empty() {
            continue;
        }
        group.sort_by(|a, b| {
            rank(a, env.clock.now)
                .cmp(&rank(b, env.clock.now))
                .then_with(|| select::order(a, b))
        });
        if !out.is_empty() {
            out.push(Entry::Spacer);
        }
        out.push(Entry::Header(provider));
        out.extend(group.into_iter().map(Entry::Account));
    }
    out
}

fn rank(account: &Account, now: i64) -> u8 {
    match health(account, now) {
        Health::Ready => 0,
        Health::Limited { .. } => 1,
        Health::SignedOut(_) => 2,
    }
}

fn rows(entries: &[Entry<'_>]) -> Vec<Row> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Account(account) => Some(account.name.clone()),
            Entry::Header(_) | Entry::Spacer => None,
        })
        .collect()
}

impl State {
    pub(crate) fn detail_open(&self) -> bool {
        self.detail
    }

    pub(crate) fn filter_mut(&mut self) -> &mut Filter {
        &mut self.filter
    }

    pub(crate) fn selected<'a>(&self, env: &Env<'a>) -> Option<&'a Account> {
        let name = self.list.selected()?;
        env.snapshot.account(name)
    }

    pub(crate) fn sync(&mut self, env: &Env<'_>) {
        let rows = rows(&entries(env, &self.filter));
        self.list.sync(&rows);
        if rows.iter().all(Option::is_none) {
            self.detail = false;
        }
    }

    pub(crate) fn handle(&mut self, command: Command, env: &Env<'_>) -> Reply {
        let rows = rows(&entries(env, &self.filter));
        match command {
            Command::Up => self.list.step(&rows, -1),
            Command::Down => self.list.step(&rows, 1),
            Command::PageUp => self.list.step(&rows, -PAGE),
            Command::PageDown => self.list.step(&rows, PAGE),
            Command::Top => self.list.top(&rows),
            Command::Bottom => self.list.bottom(&rows),
            Command::Right => self.detail = !env.wide && self.list.selected().is_some(),
            Command::Left => self.detail = false,
            Command::Back => {
                if self.detail {
                    self.detail = false;
                } else {
                    self.filter.clear();
                }
            }
            Command::Filter => self.filter.open(),
            Command::Add => {
                return Reply::Push(Modal::AddAccount(Box::new(AddAccount::new())));
            }
            Command::Select => return self.menu(env),
            Command::Relogin => return self.with_selected(env, relogin),
            Command::Rename => return self.with_selected(env, rename(env)),
            Command::Remove => return self.with_selected(env, remove),
            Command::RefreshUsage => return self.with_selected(env, refresh_usage),
            Command::Quit
            | Command::NextView
            | Command::PrevView
            | Command::Goto(_)
            | Command::Help
            | Command::Copy
            | Command::CopySettings
            | Command::RefreshModels
            | Command::FilterAccount
            | Command::FilterSession => {}
        }
        Reply::None
    }

    fn with_selected(&self, env: &Env<'_>, act: impl FnOnce(&Account) -> Reply) -> Reply {
        self.selected(env).map_or(Reply::None, act)
    }

    fn menu(&self, env: &Env<'_>) -> Reply {
        let Some(account) = self.selected(env) else {
            return Reply::None;
        };
        let item = |label: &str, command| Item {
            label: label.to_owned(),
            command,
            shortcut: shortcut(Context::Accounts, command),
            unavailable: None,
        };
        let refresh = if can_query_usage(account.kind) {
            item(
                &format!(
                    "Refresh usage (asks {})",
                    provider_title(account.kind.provider())
                ),
                Command::RefreshUsage,
            )
        } else {
            Item {
                unavailable: Some("no usage API".to_owned()),
                ..item("Refresh usage", Command::RefreshUsage)
            }
        };
        Reply::Push(Modal::Menu(Menu::new(
            account.name.clone(),
            if matches!(health(account, env.clock.now), Health::SignedOut(_)) {
                vec![
                    item("Sign in again", Command::Relogin),
                    refresh,
                    item("Rename", Command::Rename),
                    item("Remove", Command::Remove),
                ]
            } else if refresh.unavailable.is_none() {
                vec![
                    refresh,
                    item("Rename", Command::Rename),
                    item("Sign in again", Command::Relogin),
                    item("Remove", Command::Remove),
                ]
            } else {
                vec![
                    item("Rename", Command::Rename),
                    item("Sign in again", Command::Relogin),
                    refresh,
                    item("Remove", Command::Remove),
                ]
            },
        )))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, env: &Env<'_>) {
        let theme = env.theme;
        let entries = entries(env, &self.filter);
        if env.snapshot.accounts.is_empty() {
            empty(
                frame,
                area,
                "No accounts yet.",
                "Press a to add your first account.",
            );
            return;
        }
        let panes = panes(area, env.wide, self.detail);
        if let Some(list) = panes.list {
            let list = self.filter.render(frame, list, theme);
            if entries.is_empty() {
                empty(
                    frame,
                    list,
                    &format!("No account matches “{}”.", self.filter.text()),
                    "Press esc to clear the filter.",
                );
            } else {
                self.render_list(frame, list, &entries, env);
            }
        }
        if let Some(detail) = panes.detail
            && let Some(account) = self.selected(env)
        {
            draw(
                frame,
                detail,
                detail_lines(account, env, usize::from(detail.width)),
            );
        }
    }

    fn render_list(&self, frame: &mut Frame<'_>, area: Rect, entries: &[Entry<'_>], env: &Env<'_>) {
        let rows = rows(entries);
        let range = self.list.visible(&rows, usize::from(area.height));
        let widest = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Account(a) => Some(format::width(&a.name)),
                Entry::Header(_) | Entry::Spacer => None,
            })
            .max()
            .unwrap_or(8);
        let room = usize::from(area.width).saturating_sub(3 + 2 + STATUS_CELLS);
        let name_cells = widest.clamp(8, room.max(8));
        let lines: Vec<Line<'_>> = entries
            .iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(index, entry)| match entry {
                Entry::Header(provider) => {
                    Line::styled(format!(" {}", provider_title(*provider)), Theme::bold())
                }
                Entry::Spacer => Line::raw(""),
                Entry::Account(account) => {
                    let chosen = index == self.list.index();
                    account_line(account, env, name_cells, chosen)
                }
            })
            .collect();
        draw(frame, area, lines);
    }
}

fn relogin(account: &Account) -> Reply {
    Reply::Push(Modal::AddAccount(Box::new(AddAccount::again(account))))
}

fn rename(env: &Env<'_>) -> impl FnOnce(&Account) -> Reply {
    let taken = env.snapshot.account_names();
    move |account| {
        Reply::Push(Modal::Rename(Rename::new(
            "Rename account",
            &account.name,
            taken,
            |old, new| Effect::Rename { old, new },
        )))
    }
}

fn remove(account: &Account) -> Reply {
    Reply::Push(Modal::Confirm(Confirm::new(
        format!("Remove {}?", account.name),
        "gw forgets its login. You can add it again later.",
        "Remove",
        Effect::Remove(account.name.clone()),
    )))
}

fn refresh_usage(account: &Account) -> Reply {
    if can_query_usage(account.kind) {
        Reply::Do(Effect::RefreshUsage(account.name.clone()))
    } else {
        Reply::Toast(
            Tone::Error,
            "Refresh usage: no usage API for this account".to_owned(),
        )
    }
}

fn account_line<'a>(
    account: &'a Account,
    env: &Env<'_>,
    name_cells: usize,
    chosen: bool,
) -> Line<'a> {
    let theme = env.theme;
    let marker = " ";
    let mut spans = vec![
        Span::raw(format!(" {marker} ")),
        Span::raw(format::fit(&account.name, name_cells)),
        Span::raw("  "),
    ];
    spans.extend(status_spans(account, env, theme));
    let line = Line::from(spans);
    if chosen { Theme::select(line) } else { line }
}

fn status_spans(account: &Account, env: &Env<'_>, theme: Theme) -> Vec<Span<'static>> {
    let clock = env.clock;
    match health(account, clock.now) {
        Health::SignedOut(_) => vec![Span::styled("sign in needed", theme.error())],
        Health::Limited { reset_at, .. } => vec![
            Span::styled(format::fit("limited", BAR_CELLS + 2 + 4), theme.warn()),
            Span::raw("  "),
            Span::raw(clock.until(reset_at)),
        ],
        Health::Ready => match account.usage.as_ref().and_then(tightest) {
            Some(window) => {
                let stale = account
                    .usage_at
                    .is_some_and(|at| clock.now - at > STALE_SECS);
                let remaining = window
                    .reset_at
                    .map(|at| clock.until(at))
                    .unwrap_or_default();
                let [filled, track] = gauge::spans(window.used, BAR_CELLS, stale, theme);
                vec![
                    filled,
                    track,
                    Span::raw(format!(
                        "  {:>4}",
                        format!("{}%", format::percent(window.used))
                    )),
                    Span::raw(format!("  {remaining}")),
                ]
            }
            None => match (account.kind.billing(), account.kind.method()) {
                (Billing::Api, _) => vec![Span::styled("pay as you go", Theme::dim())],
                (Billing::Sub, _) => vec![Span::styled("ready", Theme::dim())],
            },
        },
    }
}

pub(crate) fn detail_lines(account: &Account, env: &Env<'_>, width: usize) -> Vec<Line<'static>> {
    let theme = env.theme;
    let clock = env.clock;
    let mut lines = Vec::new();
    match health(account, clock.now) {
        Health::SignedOut(reason) => {
            lines.extend(problem(
                &format!("Signed out ({reason}). Press enter to sign in again."),
                width,
                theme.error(),
            ));
            lines.push(Line::raw(""));
        }
        Health::Limited { reset_at, scope } => {
            let what = match scope {
                Scope::Account => "Limited".to_owned(),
                Scope::Family(family) => format!("Limited for {family} models"),
            };
            lines.extend(problem(
                &format!(
                    "{what} until {}, in {}",
                    clock.when(reset_at),
                    clock.until(reset_at)
                ),
                width,
                theme.warn(),
            ));
            lines.push(Line::raw(""));
        }
        Health::Ready => {}
    }
    lines.push(Line::styled(
        format!(
            " {}",
            format::truncate(&account.name, width.saturating_sub(1))
        ),
        theme.accent(),
    ));
    lines.push(Line::styled(
        format!(" {}", kind_label(account.kind)),
        Theme::dim(),
    ));
    if let Some(usage) = account.usage.as_ref().filter(|u| !u.windows.is_empty()) {
        lines.push(Line::raw(""));
        lines.push(heading("Usage"));
        let stale = account.usage_at.filter(|at| clock.now - at > STALE_SECS);
        let label_cells = usage
            .windows
            .iter()
            .map(|window| format::width(&window_label(&window.label)))
            .max()
            .unwrap_or(0)
            + 2;
        for window in &usage.windows {
            let [filled, track] = gauge::spans(window.used, BAR_CELLS, stale.is_some(), theme);
            let resets = window
                .reset_at
                .map(|at| format!("   resets {}", clock.when(at)))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::raw(format!(
                    "  {}",
                    format::fit(&window_label(&window.label), label_cells)
                )),
                filled,
                track,
                Span::raw(format!(
                    "  {:>4}",
                    format!("{}%", format::percent(window.used))
                )),
                Span::styled(resets, Theme::dim()),
            ]));
        }
        if let Some(at) = stale {
            lines.push(Line::styled(
                format!("  updated {}", clock.since(at)),
                Theme::dim(),
            ));
        }
    }
    if let Some(used) = env
        .snapshot
        .five_hours
        .iter()
        .find(|used| used.account == account.name && used.requests > 0)
    {
        lines.push(Line::raw(""));
        lines.push(heading("Last 5 hours"));
        lines.push(Line::raw(format!(
            "  {}",
            format::noun(used.requests, "request", "requests")
        )));
        let cached = if used.input > 0 {
            format!(
                ", {}% cached",
                format::percent(ratio(used.cached, used.input))
            )
        } else {
            String::new()
        };
        lines.push(Line::raw(format!(
            "  {} tokens in{cached}",
            format::compact(used.input)
        )));
        lines.push(Line::raw(format!(
            "  {} tokens out",
            format::compact(used.output)
        )));
    }
    let sessions = recent_sessions(account, env);
    if !sessions.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading("Active sessions"));
        lines.extend(sessions);
    }
    let errors = recent_errors(account, env, width);
    if !errors.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading("Recent errors"));
        lines.extend(errors);
    }
    lines
}

fn problem(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    format::wrap(text, width.saturating_sub(2))
        .into_iter()
        .map(|line| Line::styled(format!(" {line}"), style))
        .collect()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "token counts far below 2^52 keep their ratio"
)]
fn ratio(part: i64, whole: i64) -> f64 {
    part as f64 / whole as f64
}

fn recent_sessions(account: &Account, env: &Env<'_>) -> Vec<Line<'static>> {
    let mut seen: Vec<&str> = Vec::new();
    let mut lines = Vec::new();
    for row in env.snapshot.log.iter().rev() {
        if lines.len() == 3 || env.clock.now - row.ts > SESSION_WINDOW_SECS {
            break;
        }
        let Some(session) = row.session.as_deref() else {
            continue;
        };
        if row.account.as_deref() != Some(account.name.as_str()) || seen.contains(&session) {
            continue;
        }
        seen.push(session);
        lines.push(Line::raw(format!(
            "  {}   {}   {}",
            row.key.as_deref().unwrap_or("client"),
            row.model,
            env.clock.since(row.ts)
        )));
    }
    lines
}

fn recent_errors(account: &Account, env: &Env<'_>, width: usize) -> Vec<Line<'static>> {
    env.snapshot
        .log
        .iter()
        .rev()
        .filter(|row| row.account.as_deref() == Some(account.name.as_str()))
        .filter(|row| row.status >= 400 && row.reason != "limit")
        .take(3)
        .map(|row| {
            let when = env.clock.when(row.ts);
            let text = row.error.as_deref().unwrap_or("request failed");
            let room = width.saturating_sub(format::width(&when) + 4);
            Line::from(vec![
                Span::raw(format!("  {when}  ")),
                Span::styled(format::truncate(text, room), Style::new()),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::window_label;

    #[test]
    fn spells_out_reported_windows() {
        assert_eq!(window_label("5h"), "5 hours");
        assert_eq!(window_label("7d"), "7 days");
        assert_eq!(window_label("7d_fable"), "7 days fable");
        assert_eq!(window_label("1mo"), "1 month");
        assert_eq!(window_label("primary"), "primary");
    }
}
