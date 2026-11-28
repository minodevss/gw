use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::dialog;
use crate::format;
use crate::tui::action::Command;
use crate::tui::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) command: Command,
    pub(crate) shortcut: Option<char>,
    pub(crate) unavailable: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Menu {
    pub(crate) title: String,
    pub(crate) items: Vec<Item>,
    selected: usize,
}

impl Menu {
    pub(crate) fn new(title: impl Into<String>, items: Vec<Item>) -> Self {
        Self {
            title: title.into(),
            items,
            selected: 0,
        }
    }

    pub(crate) fn step(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub(crate) fn current(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    pub(crate) fn by_shortcut(&self, key: char) -> Option<&Item> {
        self.items.iter().find(|item| item.shortcut == Some(key))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let widest = self
            .items
            .iter()
            .map(|item| {
                format::width(&item.label)
                    + item
                        .unavailable
                        .as_ref()
                        .map_or(0, |why| format::width(why) + 3)
            })
            .max()
            .unwrap_or(0);
        let width = u16::try_from(widest + 12).unwrap_or(u16::MAX).max(30);
        let height = u16::try_from(self.items.len() + 2).unwrap_or(u16::MAX);
        let outer = dialog::centered(area, width, height);
        let inner = dialog::open(frame, outer, &self.title, "");
        let cells = usize::from(inner.width);
        let lines: Vec<Line<'_>> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let marker = " ";
                let shortcut = item.shortcut.map_or_default(String::from);
                let mut label = item.label.clone();
                if let Some(why) = &item.unavailable {
                    label = format!("{label} ({why})");
                }
                let text = format::fit(&label, cells.saturating_sub(5));
                let style = if item.unavailable.is_some() {
                    Theme::dim()
                } else {
                    Theme::default_style()
                };
                let style = if index == self.selected {
                    style.patch(Theme::selected())
                } else {
                    style
                };
                Line::from(vec![Span::styled(
                    format!("{marker} {text} {shortcut:>1} "),
                    style,
                )])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        let item = |label: &str, shortcut| Item {
            label: label.to_owned(),
            command: Command::Rename,
            shortcut: Some(shortcut),
            unavailable: None,
        };
        Menu::new("x", vec![item("One", 'o'), item("Two", 't')])
    }

    #[test]
    fn stepping_stays_inside_the_items() {
        let mut menu = menu();
        menu.step(-1);
        assert_eq!(menu.current().map(|i| i.label.as_str()), Some("One"));
        menu.step(5);
        assert_eq!(menu.current().map(|i| i.label.as_str()), Some("Two"));
    }

    #[test]
    fn shortcuts_find_items() {
        assert_eq!(
            menu().by_shortcut('t').map(|i| i.label.as_str()),
            Some("Two")
        );
        assert!(menu().by_shortcut('z').is_none());
    }
}
