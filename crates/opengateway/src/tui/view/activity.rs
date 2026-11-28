use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

use super::{Env, Filter, draw, empty};
use crate::format;
use crate::ops::log::Filter as LogFilter;
use crate::store::LogRow;
use crate::tui::action::Effect;
use crate::tui::action::{Command, Reply, Tone};
use crate::tui::keymap::{Context, shortcut};
use crate::tui::modal::Modal;
use crate::tui::theme::Theme;
use crate::tui::widget::list::{ListState, Row};
use crate::tui::widget::menu::{Item, Menu};
use crate::tui::widget::tabs;

const PAGE: isize = 10;
const RIGHT_CELLS: usize = 34;
const EXPLAIN_ROWS: u16 = 4;

#[derive(Debug, Clone)]
pub(crate) struct State {
    list: ListState,
    filter: Filter,
    following: bool,
    left_at: i64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            list: ListState::default(),
            filter: Filter::default(),
            following: true,
            left_at: 0,
        }
    }
}

fn visible<'a>(env: &Env<'a>, filter: &Filter) -> Vec<&'a LogRow> {
    let parsed = LogFilter::parse_loose(filter.text());
    env.snapshot
        .log
        .iter()
        .filter(|row| filter.text().is_empty() || parsed.matches(row))
        .collect()
}

fn rows(rows: &[&LogRow]) -> Vec<Row> {
    rows.iter().map(|row| Some(row.id.to_string())).collect()
}

fn is_limit(row: &LogRow) -> bool {
    row.reason == "limit"
}

fn is_error(row: &LogRow) -> bool {
    row.status >= 400 && !is_limit(row)
}

impl State {
    pub(crate) fn filter_mut(&mut self) -> &mut Filter {
        &mut self.filter
    }

    #[cfg(test)]
    pub(crate) fn is_following(&self) -> bool {
        self.following
    }

    fn selected<'a>(&self, env: &Env<'a>) -> Option<&'a LogRow> {
        let id: i64 = self.list.selected()?.parse().ok()?;
        env.snapshot.log.iter().find(|row| row.id == id)
    }

    pub(crate) fn sync(&mut self, env: &Env<'_>) {
        let rows = rows(&visible(env, &self.filter));
        if self.following {
            self.list.bottom(&rows);
        } else {
            self.list.sync(&rows);
        }
    }

    fn last_id(env: &Env<'_>, filter: &Filter) -> i64 {
        visible(env, filter).last().map_or(0, |row| row.id)
    }

    fn move_by(&mut self, env: &Env<'_>, delta: isize) {
        let rows = rows(&visible(env, &self.filter));
        self.list.step(&rows, delta);
        self.after_move(env, &rows);
    }

    fn after_move(&mut self, env: &Env<'_>, rows: &[Row]) {
        let at_end = self.list.selected() == rows.last().and_then(|row| row.as_deref());
        if at_end {
            self.following = true;
        } else if self.following {
            self.following = false;
            self.left_at = Self::last_id(env, &self.filter);
        }
    }

    pub(crate) fn handle(&mut self, command: Command, env: &Env<'_>) -> Reply {
        match command {
            Command::Up => self.move_by(env, -1),
            Command::Down => self.move_by(env, 1),
            Command::PageUp => self.move_by(env, -PAGE),
            Command::PageDown => self.move_by(env, PAGE),
            Command::Top => {
                let rows = rows(&visible(env, &self.filter));
                self.list.top(&rows);
                self.after_move(env, &rows);
            }
            Command::Bottom => {
                let rows = rows(&visible(env, &self.filter));
                self.list.bottom(&rows);
                self.following = true;
            }
            Command::Back | Command::Left => self.filter.clear(),
            Command::Filter => self.filter.open(),
            Command::Select => return self.menu(env),
            Command::FilterAccount => {
                if let Some(account) = self.selected(env).and_then(|row| row.account.as_deref()) {
                    self.filter.set(&format!("account:{account}"));
                }
            }
            Command::FilterSession => {
                if let Some(session) = self.selected(env).and_then(|row| row.session.as_deref()) {
                    self.filter.set(&format!("session:{session}"));
                }
            }
            Command::Copy => {
                return match self.selected(env) {
                    Some(row) => Reply::Do(Effect::Copy(explanation(row))),
                    None => Reply::Toast(Tone::Error, "Nothing selected".to_owned()),
                };
            }
            Command::Quit
            | Command::NextView
            | Command::PrevView
            | Command::Goto(_)
            | Command::Help
            | Command::Right
            | Command::Add
            | Command::Relogin
            | Command::Rename
            | Command::Remove
            | Command::RefreshUsage
            | Command::RefreshModels
            | Command::CopySettings => {}
        }
        Reply::None
    }

    fn menu(&self, env: &Env<'_>) -> Reply {
        let Some(row) = self.selected(env) else {
            return Reply::None;
        };
        let item = |label: &str, command, unavailable: Option<&str>| Item {
            label: label.to_owned(),
            command,
            shortcut: shortcut(Context::Activity, command),
            unavailable: unavailable.map(str::to_owned),
        };
        Reply::Push(Modal::Menu(Menu::new(
            format!("Request {}", env.clock.seconds(row.ts)),
            vec![
                item(
                    "Show only this account",
                    Command::FilterAccount,
                    row.account.is_none().then_some("no account was chosen"),
                ),
                item(
                    "Show only this session",
                    Command::FilterSession,
                    row.session.is_none().then_some("no session id"),
                ),
                item("Copy the details", Command::Copy, None),
            ],
        )))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, env: &Env<'_>) {
        let theme = env.theme;
        if env.snapshot.log.is_empty() {
            empty(
                frame,
                area,
                "No requests yet.",
                "Requests show up here as clients use OpenGateway.",
            );
            return;
        }
        let [summary, _, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        self.render_summary(frame, summary, env);
        let explain_rows = if body.height > 8 { EXPLAIN_ROWS } else { 0 };
        let [list, explain] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(explain_rows)]).areas(body);
        let list = self.filter.render(frame, list, theme);
        let all = visible(env, &self.filter);
        if all.is_empty() {
            empty(
                frame,
                list,
                &format!("No request matches “{}”.", self.filter.text()),
                "Press esc to clear the filter.",
            );
        } else {
            self.render_rows(frame, list, &all, env);
        }
        if explain_rows > 0
            && let Some(row) = self.selected(env)
        {
            let mut lines = vec![tabs::rule(usize::from(explain.width))];
            lines.extend(explain_lines(row, env, usize::from(explain.width)));
            draw(frame, explain, lines);
        }
    }

    fn render_summary(&self, frame: &mut Frame<'_>, area: Rect, env: &Env<'_>) {
        let theme = env.theme;
        let hour = &env.snapshot.hour;
        let mut parts = vec![format::noun(hour.requests, "request", "requests")];
        if hour.limits > 0 {
            parts.push(format::noun(hour.limits, "limit", "limits"));
        }
        if hour.errors > 0 {
            parts.push(format::noun(hour.errors, "error", "errors"));
        }
        let text = format!(" Last hour   {}", parts.join("   "));
        let status = if self.following {
            "following".to_owned()
        } else {
            let all = visible(env, &self.filter);
            let new = all.iter().filter(|row| row.id > self.left_at).count();
            format!("{new} new   End to follow")
        };
        let used = format::width(&text) + format::width(&status) + 1;
        let pad = usize::from(area.width).saturating_sub(used);
        let tail_style = if self.following {
            Theme::dim()
        } else {
            theme.accent()
        };
        let line = Line::from(vec![
            Span::styled(text, Theme::bold()),
            Span::raw(" ".repeat(pad)),
            Span::styled(status, tail_style),
        ]);
        draw(frame, area, vec![line]);
    }

    fn render_rows(&self, frame: &mut Frame<'_>, area: Rect, all: &[&LogRow], env: &Env<'_>) {
        let ids = rows(all);
        let range = self.list.visible(&ids, usize::from(area.height));
        let slice: Vec<&LogRow> = all
            .iter()
            .copied()
            .skip(range.start)
            .take(range.len())
            .collect();
        let widths = Widths::of(&slice, usize::from(area.width));
        let lines: Vec<Line<'_>> = slice
            .iter()
            .map(|row| {
                let chosen = self.list.selected() == Some(row.id.to_string().as_str());
                let line = row_line(row, env, &widths);
                if chosen { Theme::select(line) } else { line }
            })
            .collect();
        draw(frame, area, lines);
    }
}

struct Widths {
    key: usize,
    model: usize,
    account: usize,
}

impl Widths {
    fn of(rows: &[&LogRow], width: usize) -> Self {
        let longest = |text: fn(&LogRow) -> &str| {
            rows.iter()
                .map(|row| format::width(text(row)))
                .max()
                .unwrap_or(1)
        };
        let key = longest(|row| row.key.as_deref().unwrap_or("-")).min(10);
        let model_max = longest(|row| row.model.as_str());
        let account_max = longest(|row| row.account.as_deref().unwrap_or("-"));
        let flex = width.saturating_sub(3 + 8 + 8 + RIGHT_CELLS + key);
        let model = model_max.min(flex * 3 / 5).max(6);
        let account = account_max.min(flex.saturating_sub(model)).max(6);
        Self {
            key,
            model,
            account,
        }
    }
}

fn tokens(row: &LogRow) -> i64 {
    row.input.unwrap_or_default()
        + row.cache_read.unwrap_or_default()
        + row.cache_write.unwrap_or_default()
}

fn seconds(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{}.{}s", ms / 1000, ms % 1000 / 100)
    }
}

fn row_line<'a>(row: &'a LogRow, env: &Env<'_>, widths: &Widths) -> Line<'a> {
    let theme = env.theme;
    let marker = " ";
    let mut spans = vec![
        Span::raw(format!(" {marker} {}  ", env.clock.seconds(row.ts))),
        Span::raw(format::fit(row.key.as_deref().unwrap_or("-"), widths.key)),
        Span::raw("  "),
        Span::raw(format::fit(&row.model, widths.model)),
        Span::raw("  "),
        Span::raw(format::fit(
            row.account.as_deref().unwrap_or("-"),
            widths.account,
        )),
        Span::raw("  "),
    ];
    spans.extend(outcome(row, theme, RIGHT_CELLS));
    Line::from(spans)
}

fn outcome(row: &LogRow, theme: Theme, cells: usize) -> Vec<Span<'static>> {
    if is_limit(row) {
        return vec![Span::styled("limit", theme.warn())];
    }
    if is_error(row) {
        let head = row.status.to_string();
        let room = cells.saturating_sub(format::width(&head) + 2);
        let why = format::truncate(row.error.as_deref().unwrap_or_default(), room);
        return vec![
            Span::styled(head, theme.error()),
            Span::raw(format!("  {why}")),
        ];
    }
    let usage = if row.input.is_some() {
        format!(
            "  {:>6} in  {:>6} out",
            format::compact(tokens(row)),
            format::compact(row.output.unwrap_or_default())
        )
    } else {
        String::new()
    };
    vec![Span::raw(format!(
        "{}{usage}  {:>6}",
        row.status,
        seconds(row.duration_ms)
    ))]
}

fn sentence(row: &LogRow) -> String {
    let account = row.account.as_deref().unwrap_or("no account");
    if is_limit(row) {
        return row
            .error
            .clone()
            .unwrap_or_else(|| format!("{account} reached its usage limit."));
    }
    if is_error(row) {
        return row
            .error
            .clone()
            .unwrap_or_else(|| format!("The request failed with status {}.", row.status));
    }
    match row.reason.as_str() {
        "new" => format!("Served by {account}, the next account in line."),
        "reused" => format!("Served by {account}, which already has this session."),
        other => {
            let mut text = other.to_owned();
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("Served by {account}. {text}.")
        }
    }
}

fn detail_line(row: &LogRow) -> String {
    let mut parts = Vec::new();
    if let Some(session) = &row.session {
        parts.push(format!(
            "session {}",
            session.chars().take(8).collect::<String>()
        ));
    }
    if let Some(key) = &row.key {
        parts.push(format!("client {key}"));
    }
    parts.push(row.api.clone());
    if let Some(note) = &row.note {
        parts.push(note.clone());
    }
    parts.join("   ")
}

fn explain_lines(row: &LogRow, env: &Env<'_>, width: usize) -> Vec<Line<'static>> {
    let theme = env.theme;
    let when = env.clock.when(row.ts);
    let first = format!("{when}  {}", sentence(row));
    let style = if is_limit(row) {
        theme.warn()
    } else if is_error(row) {
        theme.error()
    } else {
        Theme::default_style()
    };
    let wrapped = format::wrap(&first, width.saturating_sub(1));
    let mut lines: Vec<Line<'static>> = wrapped
        .into_iter()
        .take(2)
        .map(|text| Line::styled(format!(" {text}"), style))
        .collect();
    lines.push(Line::styled(
        format::truncate(&format!(" {}", detail_line(row)), width),
        Theme::dim(),
    ));
    lines
}

fn explanation(row: &LogRow) -> String {
    format!("{}\n{}", sentence(row), detail_line(row))
}
