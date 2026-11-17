use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

static SELECTION_TINT: OnceLock<Option<Color>> = OnceLock::new();

fn selection_tint() -> Option<Color> {
    SELECTION_TINT.get().copied().flatten()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    color: bool,
}

impl Theme {
    pub(crate) fn detect() -> Self {
        Self {
            color: std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()),
        }
    }

    #[cfg(test)]
    pub(crate) const fn new(color: bool) -> Self {
        Self { color }
    }

    pub(crate) fn is_colored(self) -> bool {
        self.color
    }

    fn paint(self, color: Color) -> Style {
        if self.color {
            Style::new().fg(color)
        } else {
            Style::new()
        }
    }

    pub(crate) fn ok(self) -> Style {
        self.paint(Color::Green)
    }

    pub(crate) fn warn(self) -> Style {
        self.paint(Color::Yellow)
    }

    pub(crate) fn error(self) -> Style {
        self.paint(Color::Red).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn accent(self) -> Style {
        self.paint(Color::Cyan).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn dim() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    pub(crate) fn bold() -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }

    pub(crate) fn set_selection_tint(tint: Option<Color>) {
        SELECTION_TINT.get_or_init(|| tint);
    }

    pub(crate) fn selected() -> Style {
        match selection_tint() {
            Some(tint) => Style::new().bg(tint),
            None => Style::new().add_modifier(Modifier::REVERSED),
        }
    }

    pub(crate) fn select(line: Line<'_>) -> Line<'_> {
        if selection_tint().is_some() {
            return line.style(Self::selected());
        }
        let spans = line
            .spans
            .into_iter()
            .map(|span| {
                let style = span.style.fg(Color::Reset);
                span.style(style)
            })
            .collect::<Vec<_>>();
        Line::from(spans).style(Self::selected())
    }

    pub(crate) fn backdrop() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    pub(crate) fn current_tab() -> Style {
        Self::bold().add_modifier(Modifier::UNDERLINED)
    }

    pub(crate) fn field() -> Style {
        Style::new().add_modifier(Modifier::UNDERLINED)
    }
}

impl Theme {
    pub(crate) fn default_style() -> Style {
        Style::new()
    }
}
