use ratatui::text::{Line, Span};

use crate::tui::theme::Theme;

pub(crate) fn line<'a>(titles: &[&'a str], current: usize) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    for (index, title) in titles.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        let style = if index == current {
            Theme::current_tab()
        } else {
            Theme::dim()
        };
        spans.push(Span::styled(*title, style));
    }
    spans
}

pub(crate) fn rule(width: usize) -> Line<'static> {
    Line::styled("─".repeat(width), Theme::dim())
}
