use ratatui::style::Style;
use ratatui::text::Span;

use crate::tui::theme::Theme;

const FILLED: &str = "━";
const TRACK: &str = "─";
const WARN_AT: f64 = 0.9;
const FULL_AT: f64 = 1.0;

pub(crate) fn spans(used: f64, cells: usize, stale: bool, theme: Theme) -> [Span<'static>; 2] {
    let filled = filled(used, cells);
    let style = if stale {
        Theme::dim()
    } else if used >= FULL_AT {
        theme.error()
    } else if used >= WARN_AT {
        theme.warn()
    } else {
        Style::new()
    };
    [
        Span::styled(FILLED.repeat(filled), style),
        Span::styled(TRACK.repeat(cells - filled), Theme::backdrop()),
    ]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the value is clamped to 0..=cells before the cast"
)]
fn filled(used: f64, cells: usize) -> usize {
    ((used.clamp(0.0, 1.0) * cells as f64).round() as usize).min(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(used: f64, cells: usize) -> (String, String) {
        let [filled, track] = spans(used, cells, false, Theme::new(true));
        (filled.content.into_owned(), track.content.into_owned())
    }

    #[test]
    fn fills_proportionally_and_clamps() {
        assert_eq!(text(0.72, 10), ("━".repeat(7), "─".repeat(3)));
        assert_eq!(text(0.0, 4), (String::new(), "─".repeat(4)));
        assert_eq!(text(1.7, 4), ("━".repeat(4), String::new()));
        assert_eq!(text(-1.0, 4), (String::new(), "─".repeat(4)));
    }

    #[test]
    fn colour_only_marks_trouble() {
        let theme = Theme::new(true);
        assert_eq!(spans(0.5, 4, false, theme)[0].style, Style::new());
        assert_eq!(spans(0.95, 4, false, theme)[0].style, theme.warn());
        assert_eq!(spans(1.0, 4, false, theme)[0].style, theme.error());
    }
}
