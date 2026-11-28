use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear};

use crate::tui::theme::Theme;

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

pub(crate) fn open(frame: &mut Frame<'_>, area: Rect, title: &str, corner: &str) -> Rect {
    frame.render_widget(Clear, area);
    let mut block = Block::bordered()
        .border_style(Theme::dim())
        .title_top(Line::styled(format!(" {title} "), Theme::bold()));
    if !corner.is_empty() {
        block = block.title_top(
            Line::styled(format!(" {corner} "), Theme::dim()).alignment(Alignment::Right),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

pub(crate) fn page(frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Line::styled(format!(" {title}"), Theme::bold()),
        Rect { height: 1, ..area },
    );
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    }
}
