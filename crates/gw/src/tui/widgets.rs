use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::keymap::Command;
use crate::clock;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    color: bool,
}

impl Theme {
    pub(crate) fn from_env() -> Self {
        Self::new(std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()))
    }

    pub(crate) fn new(color: bool) -> Self {
        Self { color }
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

    pub(crate) fn bad(self) -> Style {
        self.paint(Color::Red)
    }

    pub(crate) fn dim(self) -> Style {
        if self.color {
            Style::new().fg(Color::DarkGray)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        }
    }

    pub(crate) fn accent(self) -> Style {
        self.paint(Color::Cyan).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn selected(self) -> Style {
        if self.color {
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::REVERSED)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Ctx {
    pub(crate) now: i64,
    pub(crate) theme: Theme,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Input {
    text: String,
    masked: bool,
}

impl Input {
    pub(crate) fn plain(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            masked: false,
        }
    }

    pub(crate) fn masked() -> Self {
        Self {
            text: String::new(),
            masked: true,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn handle(&mut self, key: &KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if key.code == KeyCode::Char('u') && control {
            self.text.clear();
            return true;
        }
        if let KeyCode::Char(c) = key.code
            && !control
            && !alt
        {
            self.text.push(c);
            return true;
        }
        if key.code == KeyCode::Backspace {
            self.text.pop();
            return true;
        }
        false
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        self.text.extend(pasted.chars().filter(|c| !c.is_control()));
    }

    pub(crate) fn display(&self, width: usize) -> String {
        let shown: Vec<char> = if self.masked {
            self.text.chars().map(|_| '•').collect()
        } else {
            self.text.chars().collect()
        };
        let keep = width.saturating_sub(1);
        let start = shown.len().saturating_sub(keep);
        shown.iter().skip(start).collect()
    }

    pub(crate) fn line(&self, width: usize, focused: bool, theme: Theme) -> Line<'static> {
        let mut spans = vec![Span::raw(self.display(width))];
        if focused {
            spans.push(Span::styled(" ", theme.selected()));
        }
        Line::from(spans)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterAction {
    Pending,
    Submit(String),
    Cleared,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FilterBox {
    applied: String,
    editing: Option<Input>,
}

impl FilterBox {
    pub(crate) fn applied(&self) -> &str {
        &self.applied
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub(crate) fn start(&mut self) {
        self.editing = Some(Input::plain(&self.applied));
    }

    pub(crate) fn accept(&mut self, text: String) {
        self.applied = text;
        self.editing = None;
    }

    pub(crate) fn clear(&mut self) {
        self.applied.clear();
        self.editing = None;
    }

    pub(crate) fn handle(&mut self, key: &KeyEvent) -> FilterAction {
        let Some(input) = self.editing.as_mut() else {
            return FilterAction::Pending;
        };
        if key.code == KeyCode::Esc {
            self.clear();
            return FilterAction::Cleared;
        }
        if key.code == KeyCode::Enter {
            return FilterAction::Submit(input.text().trim().to_owned());
        }
        input.handle(key);
        FilterAction::Pending
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        if let Some(input) = self.editing.as_mut() {
            input.paste(pasted);
        }
    }

    pub(crate) fn line(&self, theme: Theme) -> Option<Line<'static>> {
        if let Some(input) = &self.editing {
            let mut spans = vec![Span::styled("/", theme.accent())];
            spans.extend(input.line(60, true, theme).spans);
            return Some(Line::from(spans));
        }
        (!self.applied.is_empty()).then(|| {
            Line::from(vec![
                Span::styled("filter ", theme.dim()),
                Span::raw(self.applied.clone()),
                Span::styled("  esc clears", theme.dim()),
            ])
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Selection<K> {
    key: Option<K>,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Self { key: None }
    }
}

impl<K: PartialEq + Clone> Selection<K> {
    pub(crate) fn index(&self, keys: &[K]) -> usize {
        self.key
            .as_ref()
            .and_then(|key| keys.iter().position(|candidate| candidate == key))
            .unwrap_or(0)
    }

    pub(crate) fn sync(&mut self, keys: &[K]) {
        self.key = keys.get(self.index(keys)).cloned();
    }

    pub(crate) fn select_last(&mut self, keys: &[K]) {
        self.key = keys.last().cloned();
    }

    pub(crate) fn is_last(&self, keys: &[K]) -> bool {
        !keys.is_empty() && self.index(keys) + 1 == keys.len()
    }

    pub(crate) fn apply(&mut self, keys: &[K], command: Command) {
        let current = self.index(keys);
        let last = keys.len().saturating_sub(1);
        let next = match command {
            Command::Down => (current + 1).min(last),
            Command::Up => current.saturating_sub(1),
            Command::Top => 0,
            Command::Bottom => last,
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
            | Command::Quit => current,
        };
        self.key = keys.get(next).cloned();
    }
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        cut.push('…');
    }
    cut
}

pub(crate) fn truncate_list(items: &[String], width: usize) -> String {
    for shown in (0..=items.len()).rev() {
        let hidden = items.len() - shown;
        let mut text = items
            .iter()
            .take(shown)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        if hidden > 0 {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push('+');
            text.push_str(&hidden.to_string());
        }
        if text.chars().count() <= width {
            return text;
        }
    }
    truncate(items.first().map_or("", String::as_str), width)
}

pub(crate) fn compact(count: i64) -> String {
    match count {
        ..1_000 => count.to_string(),
        1_000..100_000 => format!("{}.{}k", count / 1_000, count % 1_000 / 100),
        100_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{}.{}M", count / 1_000_000, count % 1_000_000 / 100_000),
    }
}

pub(crate) fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub(crate) fn age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        ..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

pub(crate) fn until(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else {
        format!("{hours}h{minutes:02}m")
    }
}

pub(crate) fn countdown(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

pub(crate) fn date(epoch: i64) -> String {
    clock::format_utc(epoch)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn month_day_time(epoch: i64) -> String {
    let full = clock::format_utc(epoch);
    let mut parts = full.split(' ');
    let day = parts
        .next()
        .and_then(|date| date.split_once('-'))
        .map_or("", |(_, rest)| rest);
    format!("{day} {}", parts.next().unwrap_or_default())
}

pub(crate) fn time_of_day(epoch: i64) -> String {
    let second = epoch.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        second / 3_600,
        second % 3_600 / 60,
        second % 60
    )
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=999 before the cast"
)]
pub(crate) fn percent(used: f64) -> u32 {
    (used * 100.0).round().clamp(0.0, 999.0) as u32
}

pub(crate) fn bar(used: f64) -> String {
    let filled = (percent(used) as usize / 10).min(10);
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

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

pub(crate) fn field_line(label: &str, text: impl Into<String>, theme: Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), theme.dim()),
        Span::raw(text.into()),
    ])
}

pub(crate) fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

#[cfg(test)]
pub(crate) mod testing {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    pub(crate) fn render(
        width: u16,
        height: u16,
        draw: impl FnOnce(&mut ratatui::Frame<'_>),
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut lines = Vec::new();
        for y in 0..height {
            let line: String = (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect();
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn truncates_account_lists_with_a_remainder_count() {
        let items = names(&["a", "bb", "ccc"]);
        assert_eq!(truncate_list(&items, 20), "a bb ccc");
        assert_eq!(truncate_list(&items, 7), "a bb +1");
        assert_eq!(truncate_list(&items, 3), "+3");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }

    #[test]
    fn osc52_wraps_base64_payload() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn compact_counts_match_the_mockup() {
        assert_eq!(compact(812), "812");
        assert_eq!(compact(12_400), "12.4k");
        assert_eq!(compact(98_000), "98.0k");
        assert_eq!(compact(210_000), "210k");
        assert_eq!(compact(4_200_000), "4.2M");
    }

    #[test]
    fn durations_use_the_mockup_formats() {
        assert_eq!(until(4_320), "1h12m");
        assert_eq!(until(2_460), "0h41m");
        assert_eq!(until(2 * 86_400 + 4 * 3_600), "2d4h");
        assert_eq!(age(20), "20s");
        assert_eq!(age(720), "12m");
        assert_eq!(age(2 * 86_400), "2d");
        assert_eq!(countdown(872), "14:32");
    }

    #[test]
    fn thousands_get_separators() {
        assert_eq!(thousands(1_204), "1,204");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn bar_fills_in_tenths() {
        assert_eq!(bar(0.72), "███████░░░");
        assert_eq!(bar(1.0), "██████████");
        assert_eq!(percent(0.31), 31);
    }

    #[test]
    fn masked_input_never_shows_the_text() {
        let mut input = Input::masked();
        input.paste("sk-secret");
        assert_eq!(input.display(40), "•••••••••");
        assert_eq!(input.text(), "sk-secret");
    }

    #[test]
    fn input_edits_and_clears() {
        let mut input = Input::default();
        for c in "abc".chars() {
            assert!(input.handle(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
        input.handle(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(input.text(), "ab");
        input.handle(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(input.is_empty());
    }

    #[test]
    fn selection_follows_the_key_not_the_row() {
        let mut selection = Selection::default();
        let before = names(&["a", "b", "c"]);
        selection.sync(&before);
        selection.apply(&before, Command::Down);
        assert_eq!(selection.index(&before), 1);
        let after = names(&["z", "a", "b", "c"]);
        assert_eq!(selection.index(&after), 2);
        selection.apply(&after, Command::Bottom);
        assert!(selection.is_last(&after));
    }

    #[test]
    fn filter_box_submits_and_clears() {
        let mut filter = FilterBox::default();
        filter.start();
        for c in "gpt".chars() {
            filter.handle(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let action = filter.handle(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, FilterAction::Submit("gpt".to_owned()));
        filter.accept("gpt".to_owned());
        assert_eq!(filter.applied(), "gpt");
        filter.start();
        assert_eq!(
            filter.handle(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            FilterAction::Cleared
        );
        assert_eq!(filter.applied(), "");
    }
}
