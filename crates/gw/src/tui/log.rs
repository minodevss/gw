use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::Request;
use super::data::LOG_PAGE;
use super::keymap::Command;
use super::widgets::{
    Ctx, FilterAction, FilterBox, Selection, Theme, compact, field_line, time_of_day, truncate,
};
use crate::clock;
use crate::ops::log::Filter;
use crate::store::LogRow;

const MAX_ROWS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Column {
    Time,
    Key,
    Session,
    Model,
    Account,
    Reason,
    Status,
    In,
    Out,
    Duration,
}

impl Column {
    fn title(self) -> &'static str {
        match self {
            Self::Time | Self::Duration => "TIME",
            Self::Key => "KEY",
            Self::Session => "SESSION",
            Self::Model => "MODEL",
            Self::Account => "ACCOUNT",
            Self::Reason => "REASON",
            Self::Status => "STATUS",
            Self::In => "IN (CACHE)",
            Self::Out => "OUT",
        }
    }

    fn constraint(self) -> Constraint {
        match self {
            Self::Time => Constraint::Length(8),
            Self::Key => Constraint::Length(9),
            Self::Session => Constraint::Length(7),
            Self::Model | Self::Account => Constraint::Fill(1),
            Self::Reason | Self::Status | Self::Out | Self::Duration => Constraint::Length(6),
            Self::In => Constraint::Length(13),
        }
    }
}

pub(crate) fn columns(terminal_width: u16) -> Vec<Column> {
    [
        (Column::Time, 0),
        (Column::Key, 105),
        (Column::Session, 120),
        (Column::Model, 0),
        (Column::Account, 0),
        (Column::Reason, 0),
        (Column::Status, 0),
        (Column::In, 0),
        (Column::Out, 82),
        (Column::Duration, 92),
    ]
    .into_iter()
    .filter(|(_, minimum)| terminal_width >= *minimum)
    .map(|(column, _)| column)
    .collect()
}

#[derive(Debug, Default)]
pub(crate) struct LogView {
    rows: Vec<LogRow>,
    loaded: bool,
    complete: bool,
    selection: Selection<i64>,
    follow: bool,
    filter: FilterBox,
    parsed: Filter,
    table: TableState,
}

fn tokens_in(row: &LogRow) -> Option<i64> {
    if row.input.is_none() && row.cache_read.is_none() && row.cache_write.is_none() {
        return None;
    }
    Some(
        row.input.unwrap_or_default()
            + row.cache_read.unwrap_or_default()
            + row.cache_write.unwrap_or_default(),
    )
}

fn duration(ms: i64) -> String {
    format!("{}.{}s", ms / 1000, ms % 1000 / 100)
}

fn short_session(row: &LogRow) -> String {
    row.session.as_deref().map_or_else(
        || "—".to_owned(),
        |session| {
            let prefix: String = session.chars().take(4).collect();
            if session.chars().count() > 4 {
                format!("{prefix}…")
            } else {
                prefix
            }
        },
    )
}

fn cell(column: Column, row: &LogRow, theme: Theme) -> Cell<'static> {
    let text = |value: Option<&str>| value.unwrap_or("—").to_owned();
    match column {
        Column::Time => Cell::from(time_of_day(row.ts)),
        Column::Key => Cell::from(truncate(&text(row.key.as_deref()), 9)),
        Column::Session => Cell::from(short_session(row)),
        Column::Model => Cell::from(row.model.clone()),
        Column::Account => Cell::from(text(row.account.as_deref())),
        Column::Reason => Cell::from(truncate(&row.reason, 6)),
        Column::Status => {
            let style = match row.status {
                500.. => theme.bad(),
                400..500 => theme.warn(),
                0..400 => theme.ok(),
            };
            Cell::from(ratatui::text::Span::styled(row.status.to_string(), style))
        }
        Column::In => Cell::from(tokens_in(row).map_or_else(
            || "—".to_owned(),
            |total| {
                format!(
                    "{} ({})",
                    compact(total),
                    compact(row.cache_read.unwrap_or_default())
                )
            },
        )),
        Column::Out => Cell::from(row.output.map_or_else(|| "—".to_owned(), compact)),
        Column::Duration => Cell::from(duration(row.duration_ms)),
    }
}

pub(crate) fn row_lines(row: &LogRow, theme: Theme) -> Vec<Line<'static>> {
    let opt = |value: &Option<String>| value.clone().unwrap_or_else(|| "—".to_owned());
    let count = |value: Option<i64>| value.map_or_else(|| "—".to_owned(), |v| v.to_string());
    vec![
        field_line("id", row.id.to_string(), theme),
        field_line("time", clock::format_utc(row.ts), theme),
        field_line("key", opt(&row.key), theme),
        field_line("session", opt(&row.session), theme),
        field_line("api", row.api.clone(), theme),
        field_line("model", row.model.clone(), theme),
        field_line("account", opt(&row.account), theme),
        field_line("reason", row.reason.clone(), theme),
        field_line("status", row.status.to_string(), theme),
        field_line("error", opt(&row.error), theme),
        field_line("note", opt(&row.note), theme),
        field_line(
            "tokens",
            format!(
                "in {} · cache read {} · cache write {} · out {}",
                count(row.input),
                count(row.cache_read),
                count(row.cache_write),
                count(row.output)
            ),
            theme,
        ),
        field_line("duration", format!("{} ms", row.duration_ms), theme),
    ]
}

impl LogView {
    pub(crate) fn append(&mut self, fresh: Vec<LogRow>) {
        if !self.loaded {
            self.loaded = true;
            self.complete = fresh.len() < usize::try_from(LOG_PAGE).unwrap_or(usize::MAX);
            self.follow = true;
        }
        let last = self.last_id();
        self.rows
            .extend(fresh.into_iter().filter(|row| row.id > last));
        if self.rows.len() > MAX_ROWS {
            self.rows.drain(..self.rows.len() - MAX_ROWS);
            self.complete = false;
        }
        let ids = self.ids();
        if self.follow {
            self.selection.select_last(&ids);
        } else {
            self.selection.sync(&ids);
        }
    }

    pub(crate) fn last_id(&self) -> i64 {
        self.rows.last().map_or(0, |row| row.id)
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn last_used(&self, key: &str) -> Option<i64> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.key.as_deref() == Some(key))
            .map(|row| row.ts)
    }

    fn visible(&self) -> Vec<&LogRow> {
        self.rows
            .iter()
            .filter(|row| self.parsed.matches(row))
            .collect()
    }

    fn ids(&self) -> Vec<i64> {
        self.visible().iter().map(|row| row.id).collect()
    }

    fn selected(&self) -> Option<&LogRow> {
        let ids = self.ids();
        let id = ids.get(self.selection.index(&ids))?;
        self.rows.iter().find(|row| row.id == *id)
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.filter.is_editing()
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        self.filter.paste(pasted);
    }

    pub(crate) fn edit_filter(&mut self, key: &KeyEvent) -> Option<Request> {
        let mut request = None;
        match self.filter.handle(key) {
            FilterAction::Submit(text) => match Filter::parse(&text) {
                Ok(parsed) => {
                    self.parsed = parsed;
                    self.filter.accept(text);
                }
                Err(error) => request = Some(Request::Error(error)),
            },
            FilterAction::Cleared => self.parsed = Filter::default(),
            FilterAction::Pending => {}
        }
        let ids = self.ids();
        if self.follow {
            self.selection.select_last(&ids);
        } else {
            self.selection.sync(&ids);
        }
        request
    }

    pub(crate) fn command(&mut self, command: Command, theme: Theme) -> Option<Request> {
        let ids = self.ids();
        match command {
            Command::Down | Command::Up | Command::Top | Command::Bottom => {
                self.selection.apply(&ids, command);
                match command {
                    Command::Up | Command::Top => self.follow = false,
                    Command::Down => self.follow = self.selection.is_last(&ids),
                    Command::Bottom => self.follow = true,
                    Command::Goto(_)
                    | Command::Enter
                    | Command::Back
                    | Command::Filter
                    | Command::New
                    | Command::Login
                    | Command::Remove
                    | Command::Copy
                    | Command::Refresh
                    | Command::Help
                    | Command::Quit => {}
                }
                None
            }
            Command::Filter => {
                self.filter.start();
                None
            }
            Command::Back => {
                self.filter.clear();
                self.parsed = Filter::default();
                None
            }
            Command::Enter => self.selected().map(|row| Request::Lines {
                title: format!("request {}", row.id),
                lines: row_lines(row, theme),
            }),
            Command::Copy => self.selected().map(|row| {
                Request::Copy(
                    row_lines(row, theme)
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            }),
            Command::Goto(_)
            | Command::New
            | Command::Login
            | Command::Remove
            | Command::Refresh
            | Command::Help
            | Command::Quit => None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let [list, status] =
            Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).areas(area);
        let [left, right] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(26)]).areas(status);
        if let Some(line) = self.filter.line(ctx.theme) {
            frame.render_widget(Paragraph::new(line), left);
        }
        let (text, style) = if self.follow {
            ("following", ctx.theme.ok())
        } else {
            ("paused · G to follow", ctx.theme.warn())
        };
        frame.render_widget(
            Paragraph::new(Line::styled(text, style)).right_aligned(),
            right,
        );
        let columns = columns(area.width + 2);
        let rows = self.visible();
        let table_rows: Vec<Row<'_>> = rows
            .iter()
            .map(|row| Row::new(columns.iter().map(|c| cell(*c, row, ctx.theme))))
            .collect();
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        self.table.select(if ids.is_empty() {
            None
        } else {
            Some(self.selection.index(&ids))
        });
        let table = Table::new(table_rows, columns.iter().map(|c| c.constraint()))
            .header(
                Row::new(columns.iter().map(|c| c.title()))
                    .style(ctx.theme.dim().add_modifier(Modifier::BOLD)),
            )
            .column_spacing(1)
            .highlight_symbol("▶")
            .row_highlight_style(Style::new().add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(table, list, &mut self.table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::widgets::testing;

    fn row(id: i64, account: &str, status: u16) -> LogRow {
        LogRow {
            id,
            ts: 1_800_000_000 + id,
            key: Some("macbook".to_owned()),
            session: Some("3f2a9c01".to_owned()),
            api: "messages".to_owned(),
            model: "claude-opus-5-5".to_owned(),
            account: Some(account.to_owned()),
            reason: "reused".to_owned(),
            status,
            input: Some(500),
            cache_read: Some(11_900),
            cache_write: Some(0),
            output: Some(812),
            duration_ms: 4_100,
            ..LogRow::default()
        }
    }

    fn fixture() -> LogView {
        let mut view = LogView::default();
        view.append(vec![
            row(1, "anthropic-oauth-1", 200),
            row(2, "anthropic-oauth-2", 429),
            row(3, "openai-codex-1", 200),
        ]);
        view
    }

    #[test]
    fn follows_new_rows_until_scrolled_up() {
        let mut view = fixture();
        view.append(vec![row(4, "a", 200)]);
        assert_eq!(view.selected().map(|r| r.id), Some(4));
        view.command(Command::Up, Theme::new(false));
        assert!(!view.follow);
        view.append(vec![row(5, "a", 200)]);
        assert_eq!(view.selected().map(|r| r.id), Some(3));
        view.command(Command::Bottom, Theme::new(false));
        assert!(view.follow);
        assert_eq!(view.selected().map(|r| r.id), Some(5));
    }

    #[test]
    fn filter_applies_to_existing_and_new_rows() {
        let mut view = fixture();
        view.parsed = Filter::parse("status:429").unwrap();
        assert_eq!(view.ids(), vec![2]);
        view.append(vec![row(4, "a", 429), row(5, "a", 200)]);
        assert_eq!(view.ids(), vec![2, 4]);
        assert!(view.follow);
        assert_eq!(view.selected().map(|r| r.id), Some(4));
    }

    #[test]
    fn duplicate_ids_are_not_appended_twice() {
        let mut view = fixture();
        view.append(vec![row(3, "x", 200), row(4, "x", 200)]);
        assert_eq!(view.ids(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn small_log_counts_as_complete() {
        assert!(fixture().is_complete());
    }

    #[test]
    fn hides_columns_on_narrow_terminals() {
        assert_eq!(columns(130).len(), 10);
        assert!(!columns(110).contains(&Column::Session));
        assert!(!columns(100).contains(&Column::Key));
        assert!(!columns(90).contains(&Column::Duration));
        assert!(!columns(80).contains(&Column::Out));
        assert!(columns(80).contains(&Column::Status));
    }

    #[test]
    fn detail_shows_reason_error_and_note() {
        let mut full = row(9, "a", 429);
        full.error = Some("429 rate limited".to_owned());
        full.note = Some("cache ttl".to_owned());
        let text: String = row_lines(&full, Theme::new(false))
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("429 rate limited"));
        assert!(text.contains("cache ttl"));
        assert!(text.contains("reused"));
    }

    #[test]
    fn renders_log_screen() {
        let mut view = fixture();
        let ctx = Ctx {
            now: 1_800_000_100,
            theme: Theme::new(false),
        };
        let text = testing::render(120, 10, |frame| view.render(frame, frame.area(), &ctx));
        insta::assert_snapshot!("log", text);
    }
}
