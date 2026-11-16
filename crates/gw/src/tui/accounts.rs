use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use super::Request;
use super::keymap::Command;
use super::widgets::{
    Ctx, FilterAction, FilterBox, Selection, Theme, age, bar, compact, date, field_line,
    month_day_time, percent, truncate, until,
};
use crate::account::{Account, State};
use crate::ops::account::Detail;
use crate::provider::{Billing, Method, Window};

const BORDER: u16 = 2;
const FULL_WIDTH: u16 = 100;
const WITHOUT_WINDOW_TWO: u16 = 90;
const DETAIL_HEIGHT: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Column {
    Name,
    Method,
    State,
    Window1,
    Window2,
    Reset,
    AsOf,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Self::Name => "NAME",
            Self::Method => "METHOD",
            Self::State => "STATE",
            Self::Window1 => "WINDOW 1",
            Self::Window2 => "WINDOW 2",
            Self::Reset => "RESET",
            Self::AsOf => "AS OF",
        }
    }

    fn width(self) -> u16 {
        match self {
            Self::Name => 20,
            Self::Method => 11,
            Self::State => 8,
            Self::Window1 | Self::Window2 => 19,
            Self::Reset | Self::AsOf => 6,
        }
    }
}

pub(crate) fn columns(terminal_width: u16) -> Vec<Column> {
    let mut columns = vec![Column::Name, Column::Method, Column::State, Column::Window1];
    if terminal_width >= WITHOUT_WINDOW_TWO {
        columns.push(Column::Window2);
    }
    columns.push(Column::Reset);
    if terminal_width >= FULL_WIDTH {
        columns.push(Column::AsOf);
    }
    columns
}

#[derive(Debug, Default)]
pub(crate) struct AccountsView {
    accounts: Vec<Account>,
    detail: Option<(String, Detail)>,
    selection: Selection<String>,
    filter: FilterBox,
    table: TableState,
}

pub(crate) fn state_text(account: &Account, now: i64) -> &'static str {
    match &account.state {
        State::Active => "ok",
        State::Blocked { reset_at, .. } if *reset_at <= now => "ok",
        State::Blocked { .. } => "limited",
        State::Inactive { .. } => "inactive",
    }
}

pub(crate) fn needs_attention(accounts: &[Account], now: i64) -> (usize, usize) {
    let limited = accounts
        .iter()
        .filter(|a| state_text(a, now) == "limited")
        .count();
    let inactive = accounts
        .iter()
        .filter(|a| state_text(a, now) == "inactive")
        .count();
    (limited, inactive)
}

fn reset_at(account: &Account, now: i64) -> Option<i64> {
    match &account.state {
        State::Blocked { reset_at, .. } if *reset_at > now => Some(*reset_at),
        State::Active | State::Blocked { .. } => account.last_reset(),
        State::Inactive { .. } => None,
    }
}

fn window_cell(window: Option<&Window>, theme: Theme) -> Cell<'static> {
    let Some(window) = window else {
        return Cell::from("—");
    };
    let pct = percent(window.used);
    let style = match pct {
        90.. => theme.bad(),
        70..90 => theme.warn(),
        0..70 => theme.ok(),
    };
    Cell::from(Line::from(vec![
        Span::raw(format!("{:<2} ", truncate(&window.label, 2))),
        Span::styled(bar(window.used), style),
        Span::raw(format!(" {:>4}", format!("{pct}%"))),
    ]))
}

fn cell(column: Column, account: &Account, ctx: &Ctx) -> Cell<'static> {
    let window = |index: usize| {
        account
            .usage
            .as_ref()
            .and_then(|usage| usage.windows.get(index))
    };
    match column {
        Column::Name => Cell::from(truncate(&account.name, usize::from(column.width()))),
        Column::Method => Cell::from(account.kind.method().as_str()),
        Column::State => {
            let text = state_text(account, ctx.now);
            let style = match text {
                "limited" => ctx.theme.warn(),
                "inactive" => ctx.theme.bad(),
                _ => ctx.theme.ok(),
            };
            Cell::from(Span::styled(text, style))
        }
        Column::Window1 => window_cell(window(0), ctx.theme),
        Column::Window2 => window_cell(window(1), ctx.theme),
        Column::Reset => Cell::from(reset_at(account, ctx.now).map_or_else(
            || "—".to_owned(),
            |at| {
                if at <= ctx.now {
                    "now".to_owned()
                } else {
                    until(at - ctx.now)
                }
            },
        )),
        Column::AsOf => Cell::from(
            account
                .usage_at
                .map_or_else(|| "—".to_owned(), |at| age(ctx.now - at)),
        ),
    }
}

fn reset_label(reset: i64, now: i64) -> String {
    let full = month_day_time(reset);
    if reset - now < 86_400 {
        full.split(' ').nth(1).unwrap_or_default().to_owned()
    } else {
        full
    }
}

pub(crate) fn detail_lines(
    account: &Account,
    detail: Option<&Detail>,
    now: i64,
    theme: Theme,
) -> Vec<Line<'static>> {
    let billing = match account.kind.billing() {
        Billing::Sub => "sub",
        Billing::Api => "api",
    };
    let credential = if account.kind.refreshes() {
        "token auto refresh"
    } else {
        "static credential"
    };
    let mut lines = vec![Line::from(format!(
        "{} · {} · {billing} · added {} · {credential}",
        account.kind.provider(),
        account.kind.method(),
        date(account.created_at)
    ))];
    match &account.state {
        State::Active => {}
        State::Blocked { reset_at, .. } if *reset_at <= now => {}
        State::Blocked { reset_at, .. } => lines.push(field_line(
            "state",
            format!("limited until {}", month_day_time(*reset_at)),
            theme,
        )),
        State::Inactive { reason } => {
            lines.push(field_line("state", format!("inactive: {reason}"), theme));
        }
    }
    if account.kind.method() == Method::SetupToken
        && let Some(expires) = account.secret.expires_at()
    {
        lines.push(field_line("expires", date(expires), theme));
    }
    let limits: Vec<String> = account
        .usage
        .iter()
        .flat_map(|usage| usage.windows.iter())
        .map(|window| {
            let reset = window
                .reset_at
                .map_or_else(String::new, |at| format!(" ({})", reset_label(at, now)));
            format!("{} {}%{reset}", window.label, percent(window.used))
        })
        .collect();
    lines.push(field_line(
        "limits",
        if limits.is_empty() {
            "—".to_owned()
        } else {
            limits.join(" · ")
        },
        theme,
    ));
    let Some(detail) = detail else {
        lines.push(field_line("tokens", "loading…", theme));
        return lines;
    };
    lines.push(field_line(
        "tokens",
        format!(
            "5h {} in ({} cache) / {} out · 24h {} requests",
            compact(detail.input_5h),
            compact(detail.cache_5h),
            compact(detail.output_5h),
            detail.requests_24h
        ),
        theme,
    ));
    let sessions: Vec<String> = detail
        .sessions
        .iter()
        .map(|(session, key, model, ts)| {
            let prefix: String = session.chars().take(4).collect();
            let short = if session.chars().count() > 4 {
                format!("{prefix}…")
            } else {
                prefix
            };
            format!("{short} {key} {model} {} ago", age(now - ts))
        })
        .collect();
    lines.push(field_line(
        "sessions",
        if sessions.is_empty() {
            "none".to_owned()
        } else {
            sessions.join(" · ")
        },
        theme,
    ));
    let errors: Vec<String> = detail
        .errors
        .iter()
        .map(|(ts, error)| format!("{} ago {}", age(now - ts), truncate(error, 48)))
        .collect();
    lines.push(field_line(
        "errors",
        if errors.is_empty() {
            "none in 24h".to_owned()
        } else {
            errors.join(" · ")
        },
        theme,
    ));
    lines
}

impl AccountsView {
    pub(crate) fn update(&mut self, accounts: Vec<Account>, detail: Option<(String, Detail)>) {
        self.accounts = accounts;
        self.detail = detail;
        let names = self.names();
        self.selection.sync(&names);
    }

    pub(crate) fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    fn visible(&self) -> Vec<&Account> {
        let needle = self.filter.applied().to_lowercase();
        self.accounts
            .iter()
            .filter(|account| {
                needle.is_empty()
                    || account.name.to_lowercase().contains(&needle)
                    || account.kind.method().as_str().contains(&needle)
            })
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.visible().iter().map(|a| a.name.clone()).collect()
    }

    pub(crate) fn selected(&self) -> Option<&Account> {
        let names = self.names();
        let name = names.get(self.selection.index(&names))?;
        self.accounts.iter().find(|a| &a.name == name)
    }

    pub(crate) fn selected_name(&self) -> Option<String> {
        self.selected().map(|a| a.name.clone())
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.filter.is_editing()
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        self.filter.paste(pasted);
    }

    pub(crate) fn edit_filter(&mut self, key: &KeyEvent) {
        match self.filter.handle(key) {
            FilterAction::Submit(text) => self.filter.accept(text),
            FilterAction::Pending | FilterAction::Cleared => {}
        }
        let names = self.names();
        self.selection.sync(&names);
    }

    pub(crate) fn command(&mut self, command: Command, now: i64, theme: Theme) -> Option<Request> {
        let names = self.names();
        match command {
            Command::Down | Command::Up | Command::Top | Command::Bottom => {
                self.selection.apply(&names, command);
                None
            }
            Command::Filter => {
                self.filter.start();
                None
            }
            Command::Back => {
                self.filter.clear();
                None
            }
            Command::New => Some(Request::AddAccount),
            Command::Login => self.selected().cloned().map(Request::Relogin),
            Command::Remove => self.selected_name().map(Request::RemoveAccount),
            Command::Refresh => self.selected_name().map(Request::RefreshAccount),
            Command::Copy => self.selected_name().map(Request::Copy),
            Command::Enter => {
                let account = self.selected()?;
                let detail = self
                    .detail
                    .as_ref()
                    .filter(|(name, _)| *name == account.name)
                    .map(|(_, detail)| detail);
                Some(Request::Lines {
                    title: account.name.clone(),
                    lines: detail_lines(account, detail, now, theme),
                })
            }
            Command::Goto(_) | Command::Help | Command::Quit => None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let filter_line = self.filter.line(ctx.theme);
        let [list, bar_area, detail] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(u16::from(filter_line.is_some())),
            Constraint::Length(DETAIL_HEIGHT),
        ])
        .areas(area);
        if let Some(line) = filter_line {
            frame.render_widget(Paragraph::new(line), bar_area);
        }
        self.render_table(frame, list, ctx);
        self.render_detail(frame, detail, ctx);
    }

    fn render_table(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let columns = columns(area.width + BORDER);
        let accounts = self.visible();
        let rows: Vec<Row<'_>> = accounts
            .iter()
            .map(|account| Row::new(columns.iter().map(|c| cell(*c, account, ctx))))
            .collect();
        let header = Row::new(columns.iter().map(|c| c.title()))
            .style(ctx.theme.dim().add_modifier(Modifier::BOLD));
        let names: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
        self.table.select(if names.is_empty() {
            None
        } else {
            Some(self.selection.index(&names))
        });
        let table = Table::new(rows, columns.iter().map(|c| Constraint::Length(c.width())))
            .header(header)
            .column_spacing(1)
            .highlight_symbol("▶")
            .row_highlight_style(Style::new().add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(table, area, &mut self.table);
    }

    fn render_detail(&self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let Some(account) = self.selected() else {
            frame.render_widget(
                Paragraph::new("no accounts · press n to add one")
                    .block(Block::new().borders(Borders::TOP)),
                area,
            );
            return;
        };
        let detail = self
            .detail
            .as_ref()
            .filter(|(name, _)| *name == account.name)
            .map(|(_, detail)| detail);
        frame.render_widget(
            Paragraph::new(detail_lines(account, detail, ctx.now, ctx.theme)).block(
                Block::new()
                    .borders(Borders::TOP)
                    .title(format!(" {} ", account.name)),
            ),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Secret;
    use crate::provider::{Kind, Provider, Scope, Usage};
    use crate::tui::widgets::testing;

    const NOW: i64 = 1_800_000_000;

    fn account(name: &str, provider: Provider, method: Method, state: State) -> Account {
        Account {
            name: name.to_owned(),
            kind: Kind::new(provider, method).unwrap(),
            secret: Secret::Key { key: String::new() },
            state,
            usage: None,
            usage_at: None,
            created_at: NOW - 86_400,
        }
    }

    fn fixture() -> AccountsView {
        let mut oauth = account(
            "anthropic-oauth-1",
            Provider::Anthropic,
            Method::Oauth,
            State::Active,
        );
        oauth.usage = Some(Usage {
            windows: vec![
                Window {
                    label: "5h".to_owned(),
                    used: 0.72,
                    reset_at: Some(NOW + 4_320),
                },
                Window {
                    label: "7d".to_owned(),
                    used: 0.31,
                    reset_at: Some(NOW + 3 * 86_400),
                },
            ],
        });
        oauth.usage_at = Some(NOW - 20);
        let key = account("openai-key-1", Provider::OpenAi, Method::Key, State::Active);
        let limited = account(
            "anthropic-oauth-2",
            Provider::Anthropic,
            Method::Oauth,
            State::Blocked {
                scope: Scope::Account,
                reset_at: NOW + 2_460,
            },
        );
        let inactive = account(
            "anthropic-oauth-3",
            Provider::Anthropic,
            Method::Oauth,
            State::Inactive {
                reason: "refresh failed".to_owned(),
            },
        );
        let mut view = AccountsView::default();
        let detail = Detail {
            requests_24h: 318,
            input_5h: 4_200_000,
            cache_5h: 3_900_000,
            output_5h: 210_000,
            sessions: vec![(
                "3f2a9c01".to_owned(),
                "macbook".to_owned(),
                "claude-opus-5-5".to_owned(),
                NOW - 20,
            )],
            ..Detail::default()
        };
        view.update(
            vec![oauth, key, limited, inactive],
            Some(("anthropic-oauth-1".to_owned(), detail)),
        );
        view
    }

    fn ctx() -> Ctx {
        Ctx {
            now: NOW,
            theme: Theme::new(false),
        }
    }

    #[test]
    fn hides_as_of_then_window_two_when_narrow() {
        assert_eq!(columns(120).len(), 7);
        assert!(!columns(99).contains(&Column::AsOf));
        assert!(columns(99).contains(&Column::Window2));
        assert!(!columns(89).contains(&Column::Window2));
        assert!(columns(80).contains(&Column::Reset));
    }

    #[test]
    fn counts_limited_and_inactive_accounts() {
        let view = fixture();
        assert_eq!(needs_attention(view.accounts(), NOW), (1, 1));
    }

    #[test]
    fn selection_survives_a_reordered_reload() {
        let mut view = fixture();
        view.command(Command::Down, NOW, Theme::new(false));
        assert_eq!(view.selected_name().as_deref(), Some("openai-key-1"));
        let mut reordered = view.accounts().to_vec();
        reordered.reverse();
        view.update(reordered, None);
        assert_eq!(view.selected_name().as_deref(), Some("openai-key-1"));
    }

    #[test]
    fn filter_narrows_by_name() {
        let mut view = fixture();
        view.filter.accept("key".to_owned());
        assert_eq!(view.names(), vec!["openai-key-1".to_owned()]);
    }

    #[test]
    fn renders_accounts_screen_wide() {
        let mut view = fixture();
        let text = testing::render(100, 22, |frame| view.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("accounts_wide", text);
    }

    #[test]
    fn renders_accounts_screen_narrow() {
        let mut view = fixture();
        let text = testing::render(80, 22, |frame| view.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("accounts_narrow", text);
    }
}
