pub(crate) mod accounts;
pub(crate) mod activity;
pub(crate) mod keys;
pub(crate) mod models;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::theme::Theme;
use super::widget::text_field::TextField;
use crate::format::Clock;
use crate::ops::snapshot::Snapshot;

const LIST_WIDTH: u16 = 56;
const GAP: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Accounts,
    Activity,
    Models,
    Keys,
}

impl View {
    pub(crate) const ALL: [Self; 4] = [Self::Accounts, Self::Activity, Self::Models, Self::Keys];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Accounts => "Accounts",
            Self::Activity => "Activity",
            Self::Models => "Models",
            Self::Keys => "Keys",
        }
    }

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    pub(crate) fn step(self, delta: isize) -> Self {
        let count = Self::ALL.len().cast_signed();
        let next = (self.index().cast_signed() + delta).rem_euclid(count);
        Self::ALL
            .get(next.cast_unsigned())
            .copied()
            .unwrap_or(Self::Accounts)
    }
}

pub(crate) struct Env<'a> {
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) clock: &'a Clock,
    pub(crate) theme: Theme,
    pub(crate) wide: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Filter {
    field: TextField,
    editing: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            field: TextField::new().with_placeholder("type to filter"),
            editing: false,
        }
    }
}

impl Filter {
    pub(crate) fn text(&self) -> &str {
        self.field.value().trim()
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing
    }

    pub(crate) fn is_active(&self) -> bool {
        self.editing || !self.field.is_empty()
    }

    pub(crate) fn open(&mut self) {
        self.editing = true;
    }

    pub(crate) fn apply(&mut self) {
        self.editing = false;
    }

    pub(crate) fn clear(&mut self) {
        self.field.set_value("");
        self.editing = false;
    }

    pub(crate) fn set(&mut self, text: &str) {
        self.field.set_value(text);
        self.editing = false;
    }

    pub(crate) fn field_mut(&mut self) -> &mut TextField {
        &mut self.field
    }

    pub(crate) fn matches(&self, haystack: &str) -> bool {
        let text = self.text().to_lowercase();
        let haystack = haystack.to_lowercase();
        text.split_whitespace().all(|word| haystack.contains(word))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) -> Rect {
        if !self.is_active() || area.height == 0 {
            return area;
        }
        let [bar, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let [mark, input] =
            Layout::horizontal([Constraint::Length(2), Constraint::Fill(1)]).areas(bar);
        frame.render_widget(Paragraph::new(Span::styled("/ ", theme.accent())), mark);
        self.field.render(frame, input, theme, self.editing);
        rest
    }
}

pub(crate) struct Panes {
    pub(crate) list: Option<Rect>,
    pub(crate) detail: Option<Rect>,
}

pub(crate) fn panes(area: Rect, wide: bool, detail_open: bool) -> Panes {
    if wide {
        let list = LIST_WIDTH.min(area.width * 5 / 12);
        let [list, _, detail] = Layout::horizontal([
            Constraint::Length(list),
            Constraint::Length(GAP),
            Constraint::Fill(1),
        ])
        .areas(area);
        Panes {
            list: Some(list),
            detail: Some(detail),
        }
    } else if detail_open {
        Panes {
            list: None,
            detail: Some(area),
        }
    } else {
        Panes {
            list: Some(area),
            detail: None,
        }
    }
}

pub(crate) fn empty(frame: &mut Frame<'_>, area: Rect, title: &str, hint: &str) {
    let lines = vec![
        Line::styled(format!(" {title}"), Theme::bold()),
        Line::styled(format!(" {hint}"), Theme::dim()),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

pub(crate) fn draw(frame: &mut Frame<'_>, area: Rect, lines: Vec<Line<'_>>) {
    frame.render_widget(Paragraph::new(lines), area);
}

pub(crate) fn heading(text: &str) -> Line<'static> {
    Line::styled(format!(" {text}"), Theme::bold())
}

pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_cycle_in_both_directions() {
        assert_eq!(View::Keys.step(1), View::Accounts);
        assert_eq!(View::Accounts.step(-1), View::Keys);
        assert_eq!(View::Accounts.step(2), View::Models);
    }

    #[test]
    fn filter_matches_every_word_in_any_order() {
        let mut filter = Filter::default();
        filter.field_mut().set_value("max claude");
        assert!(filter.matches("Claude-Max-1"));
        assert!(!filter.matches("claude-api"));
    }
}
