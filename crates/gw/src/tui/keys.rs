use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, TableState, Tabs, Wrap};

use super::Request;
use super::data::KeyStat;
use super::keymap::Command;
use super::modal::Closed;
use super::widgets::{Ctx, Selection, age, centered, date, thousands};
use crate::ops::key::Snippet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastUsed {
    At(i64),
    Never,
    Older,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    stat: KeyStat,
    last_used: LastUsed,
}

impl LastUsed {
    fn text(self, now: i64) -> String {
        match self {
            Self::At(at) => format!("{} ago", age(now - at)),
            Self::Never => "never".to_owned(),
            Self::Older => "older".to_owned(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct KeysView {
    entries: Vec<Entry>,
    selection: Selection<String>,
    table: TableState,
}

impl KeysView {
    pub(crate) fn update(
        &mut self,
        stats: Vec<KeyStat>,
        last_in_log: impl Fn(&str) -> Option<i64>,
        log_is_complete: bool,
    ) {
        self.entries = stats
            .into_iter()
            .map(|stat| {
                let seen = stat.last_24h.max(last_in_log(&stat.name));
                let last_used = match seen {
                    Some(at) => LastUsed::At(at),
                    None if log_is_complete => LastUsed::Never,
                    None => LastUsed::Older,
                };
                Entry { stat, last_used }
            })
            .collect();
        let names = self.names();
        self.selection.sync(&names);
    }

    fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.stat.name.clone()).collect()
    }

    fn selected(&self) -> Option<&Entry> {
        let names = self.names();
        self.entries.get(self.selection.index(&names))
    }

    pub(crate) fn command(&mut self, command: Command, now: i64) -> Option<Request> {
        let names = self.names();
        match command {
            Command::Down | Command::Up | Command::Top | Command::Bottom => {
                self.selection.apply(&names, command);
                None
            }
            Command::New => Some(Request::NewKey),
            Command::Enter => self
                .selected()
                .map(|e| Request::KeySnippets(e.stat.name.clone())),
            Command::Copy => self.selected().map(|e| Request::Copy(e.stat.name.clone())),
            Command::Remove => self.selected().map(|e| Request::RevokeKey {
                name: e.stat.name.clone(),
                last_used: e.last_used.text(now),
            }),
            Command::Goto(_)
            | Command::Back
            | Command::Filter
            | Command::Login
            | Command::Refresh
            | Command::Help
            | Command::Quit => None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let rows: Vec<Row<'_>> = self
            .entries
            .iter()
            .map(|entry| {
                Row::new(vec![
                    entry.stat.name.clone(),
                    date(entry.stat.created_at),
                    entry.last_used.text(ctx.now),
                    thousands(entry.stat.requests_24h),
                ])
            })
            .collect();
        let names = self.names();
        self.table.select(if names.is_empty() {
            None
        } else {
            Some(self.selection.index(&names))
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(22),
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Length(14),
            ],
        )
        .header(
            Row::new(["NAME", "CREATED", "LAST USED", "REQUESTS 24H"])
                .style(ctx.theme.dim().add_modifier(Modifier::BOLD)),
        )
        .column_spacing(1)
        .highlight_symbol("▶")
        .row_highlight_style(Style::new().add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(table, area, &mut self.table);
        if self.entries.is_empty() {
            let hint = Rect {
                y: area.y + 2,
                height: 1,
                ..area
            };
            frame.render_widget(Paragraph::new("no keys · press n to create one"), hint);
        }
    }
}

#[derive(Debug)]
pub(crate) struct SnippetTabs {
    title: String,
    tabs: Vec<(String, String)>,
    current: usize,
    escape_closes: bool,
}

impl SnippetTabs {
    pub(crate) fn issued(name: &str, key: &str, snippets: Vec<Snippet>) -> Self {
        let mut tabs = vec![(
            "Key".to_owned(),
            format!(
                "{key}\n\nshown once: only its hash is stored, so copy it now.\nenter closes this window."
            ),
        )];
        tabs.extend(snippets.into_iter().map(|s| (s.client.to_owned(), s.text)));
        Self {
            title: format!("New key {name}"),
            tabs,
            current: 0,
            escape_closes: false,
        }
    }

    pub(crate) fn placeholder(name: &str, snippets: Vec<Snippet>) -> Self {
        Self {
            title: format!("Setup for {name}"),
            tabs: snippets
                .into_iter()
                .map(|s| (s.client.to_owned(), s.text))
                .collect(),
            current: 0,
            escape_closes: true,
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.escape_closes {
            vec![("tab", "next"), ("y", "copy"), ("esc", "close")]
        } else {
            vec![("tab", "next"), ("y", "copy"), ("enter", "close")]
        }
    }

    pub(crate) fn handle(&mut self, key: &KeyEvent) -> Closed<String> {
        let count = self.tabs.len().max(1);
        let code = key.code;
        if code == KeyCode::Enter || (code == KeyCode::Esc && self.escape_closes) {
            Closed::Cancel
        } else if matches!(code, KeyCode::Tab | KeyCode::Right | KeyCode::Char('l')) {
            self.current = (self.current + 1) % count;
            Closed::Stay
        } else if matches!(code, KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h')) {
            self.current = (self.current + count - 1) % count;
            Closed::Stay
        } else if code == KeyCode::Char('y') {
            self.tabs
                .get(self.current)
                .map_or(Closed::Stay, |(_, text)| Closed::Done(text.clone()))
        } else {
            Closed::Stay
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let rect = centered(
            area,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        frame.render_widget(Clear, rect);
        let block = Block::bordered().title(format!(" {} ", self.title));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let [tabs, body] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        frame.render_widget(
            Tabs::new(self.tabs.iter().map(|(title, _)| title.as_str()))
                .select(self.current)
                .highlight_style(ctx.theme.selected()),
            tabs,
        );
        let text = self
            .tabs
            .get(self.current)
            .map_or("", |(_, text)| text.as_str());
        frame.render_widget(
            Paragraph::new(text.lines().map(Line::from).collect::<Vec<_>>())
                .wrap(Wrap { trim: false }),
            body,
        );
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;
    use crate::tui::widgets::{Theme, testing};

    const NOW: i64 = 1_800_000_000;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn stat(name: &str, requests: usize, last: Option<i64>) -> KeyStat {
        KeyStat {
            name: name.to_owned(),
            created_at: NOW - 86_400,
            requests_24h: requests,
            last_24h: last,
        }
    }

    fn fixture() -> KeysView {
        let mut view = KeysView::default();
        view.update(
            vec![
                stat("macbook", 1_204, Some(NOW - 12)),
                stat("desktop", 0, None),
                stat("t3code", 0, None),
            ],
            |name| (name == "desktop").then_some(NOW - 2 * 86_400),
            true,
        );
        view
    }

    fn ctx() -> Ctx {
        Ctx {
            now: NOW,
            theme: Theme::new(false),
        }
    }

    fn snippets() -> Vec<Snippet> {
        crate::ops::key::snippets("http://100.64.0.2:8787", "<key>")
    }

    #[test]
    fn last_use_falls_back_to_the_log_then_never() {
        let mut view = fixture();
        view.command(Command::Down, NOW);
        let Some(Request::RevokeKey { name, last_used }) = view.command(Command::Remove, NOW)
        else {
            panic!("expected a revoke request");
        };
        assert_eq!((name.as_str(), last_used.as_str()), ("desktop", "2d ago"));
        view.command(Command::Down, NOW);
        let Some(Request::RevokeKey { last_used, .. }) = view.command(Command::Remove, NOW) else {
            panic!("expected a revoke request");
        };
        assert_eq!(last_used, "never");
    }

    #[test]
    fn issued_modal_closes_only_with_enter() {
        let mut tabs = SnippetTabs::issued("macbook", "gw_secret", snippets());
        assert_eq!(tabs.handle(&press(KeyCode::Esc)), Closed::Stay);
        assert_eq!(tabs.handle(&press(KeyCode::Char('q'))), Closed::Stay);
        assert_eq!(tabs.handle(&press(KeyCode::Enter)), Closed::Cancel);
    }

    #[test]
    fn copy_returns_the_visible_tab() {
        let mut tabs = SnippetTabs::issued("macbook", "gw_secret", snippets());
        let Closed::Done(key) = tabs.handle(&press(KeyCode::Char('y'))) else {
            panic!("expected text to copy");
        };
        assert!(key.starts_with("gw_secret"));
        tabs.handle(&press(KeyCode::Tab));
        let Closed::Done(text) = tabs.handle(&press(KeyCode::Char('y'))) else {
            panic!("expected text to copy");
        };
        assert!(text.contains("ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn placeholder_view_keeps_the_key_slot_empty() {
        let mut tabs = SnippetTabs::placeholder("macbook", snippets());
        let Closed::Done(text) = tabs.handle(&press(KeyCode::Char('y'))) else {
            panic!("expected text to copy");
        };
        assert!(text.contains("<key>"));
        assert_eq!(tabs.handle(&press(KeyCode::Esc)), Closed::Cancel);
    }

    #[test]
    fn renders_keys_screen() {
        let mut view = fixture();
        let text = testing::render(90, 8, |frame| view.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("keys_list", text);
    }

    #[test]
    fn renders_issued_key_modal() {
        let tabs = SnippetTabs::issued("macbook", "gw_secret", snippets());
        let text = testing::render(90, 16, |frame| tabs.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("keys_issued", text);
    }
}
