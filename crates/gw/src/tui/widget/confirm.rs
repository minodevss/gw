use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::dialog;
use crate::tui::action::Effect;
use crate::tui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    Cancel,
    Confirm,
}

#[derive(Debug, Clone)]
pub(crate) struct Confirm {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) label: String,
    pub(crate) effect: Effect,
    choice: Choice,
}

impl Confirm {
    pub(crate) fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        label: impl Into<String>,
        effect: Effect,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            label: label.into(),
            effect,
            choice: Choice::Cancel,
        }
    }

    pub(crate) fn choice(&self) -> Choice {
        self.choice
    }

    pub(crate) fn pick(&mut self, choice: Choice) {
        self.choice = choice;
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let body_width = crate::tui::format::width(&self.body).clamp(36, 60);
        let width = u16::try_from(body_width + 4).unwrap_or(u16::MAX);
        let rows = crate::tui::format::wrap(&self.body, body_width).len();
        let height = u16::try_from(rows + 4).unwrap_or(u16::MAX);
        let outer = dialog::centered(area, width, height);
        let inner = dialog::open(frame, outer, &self.title, "");
        let body_height = u16::try_from(rows).unwrap_or(u16::MAX);
        frame.render_widget(
            Paragraph::new(self.body.as_str()).wrap(Wrap { trim: false }),
            Rect {
                height: body_height,
                ..inner
            },
        );
        let button = |label: &str, active: bool, danger: bool| {
            let text = format!("[ {label} ]");
            let style = match (active, danger) {
                (true, _) => Theme::selected(),
                (false, true) => theme.error(),
                (false, false) => Theme::default_style(),
            };
            Span::styled(text, style)
        };
        let buttons = Line::from(vec![
            button("Cancel", self.choice == Choice::Cancel, false),
            Span::raw("   "),
            button(&self.label, self.choice == Choice::Confirm, true),
        ])
        .alignment(Alignment::Right);
        let row = Rect {
            y: inner.y + inner.height.saturating_sub(1),
            height: 1,
            ..inner
        };
        frame.render_widget(Paragraph::new(buttons), row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_cancel() {
        let confirm = Confirm::new("t", "b", "Remove", Effect::RefreshModels);
        assert_eq!(confirm.choice(), Choice::Cancel);
    }
}
