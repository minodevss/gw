use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::tui::theme::Theme;

const MASK: &str = "•";

enum Edit {
    Changed,
    Moved,
    Ignored,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TextField {
    value: String,
    cursor: usize,
    mask: bool,
    revealed: bool,
    placeholder: &'static str,
    error: Option<String>,
    scroll: Cell<usize>,
}

impl TextField {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn masked(mut self) -> Self {
        self.mask = true;
        self
    }

    pub(crate) fn with_placeholder(mut self, placeholder: &'static str) -> Self {
        self.placeholder = placeholder;
        self
    }

    pub(crate) fn with_value(mut self, value: &str) -> Self {
        self.set_value(value);
        self
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub(crate) fn set_value(&mut self, value: &str) {
        value.clone_into(&mut self.value);
        self.cursor = self.value.len();
        self.error = None;
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn is_revealed(&self) -> bool {
        self.revealed
    }

    pub(crate) fn cursor_cells(&self) -> usize {
        let head = self.value.get(..self.cursor).unwrap_or_default();
        if self.is_hidden() {
            head.graphemes(true).count()
        } else {
            head.width()
        }
    }

    fn is_hidden(&self) -> bool {
        self.mask && !self.revealed
    }

    fn before(&self) -> &str {
        self.value.get(..self.cursor).unwrap_or_default()
    }

    fn after(&self) -> &str {
        self.value.get(self.cursor..).unwrap_or_default()
    }

    fn is_word(grapheme: &str) -> bool {
        grapheme.chars().any(|c| c.is_alphanumeric() || c == '_')
    }

    fn left_one(&self) -> usize {
        self.before()
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn right_one(&self) -> usize {
        self.cursor + self.after().graphemes(true).next().map_or(0, str::len)
    }

    fn word_left(&self) -> usize {
        let mut position = self.cursor;
        let mut in_word = false;
        for (index, grapheme) in self.before().grapheme_indices(true).rev() {
            let word = Self::is_word(grapheme);
            if in_word && !word {
                break;
            }
            in_word |= word;
            position = index;
        }
        position
    }

    fn word_right(&self) -> usize {
        let mut position = self.cursor;
        let mut in_word = false;
        for grapheme in self.after().graphemes(true) {
            let word = Self::is_word(grapheme);
            if in_word && !word {
                break;
            }
            in_word |= word;
            position += grapheme.len();
        }
        position
    }

    fn remove(&mut self, from: usize, to: usize) {
        self.value.replace_range(from..to, "");
        self.cursor = from;
    }

    fn insert(&mut self, text: &str) {
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub(crate) fn paste(&mut self, text: &str) {
        let cleaned: String = text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            self.insert(cleaned);
            self.error = None;
        }
    }

    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let edit = match key.code {
            KeyCode::Char(c) if ctrl && !alt => self.control(c),
            KeyCode::Char(c) if alt && !ctrl => self.meta(c),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.insert(c.encode_utf8(&mut [0; 4]));
                Edit::Changed
            }
            KeyCode::Left if ctrl || alt => self.go(self.word_left()),
            KeyCode::Right if ctrl || alt => self.go(self.word_right()),
            KeyCode::Left => self.go(self.left_one()),
            KeyCode::Right => self.go(self.right_one()),
            KeyCode::Home => self.go(0),
            KeyCode::End => self.go(self.value.len()),
            KeyCode::Backspace if alt => self.cut(self.word_left(), self.cursor),
            KeyCode::Backspace => self.cut(self.left_one(), self.cursor),
            KeyCode::Delete => self.cut(self.cursor, self.right_one()),
            KeyCode::Enter
            | KeyCode::Esc
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Insert
            | KeyCode::F(_)
            | KeyCode::Char(_)
            | KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => Edit::Ignored,
        };
        match edit {
            Edit::Changed => {
                self.error = None;
                true
            }
            Edit::Moved => true,
            Edit::Ignored => false,
        }
    }

    fn go(&mut self, position: usize) -> Edit {
        self.cursor = position;
        Edit::Moved
    }

    fn cut(&mut self, from: usize, to: usize) -> Edit {
        self.remove(from, to);
        Edit::Changed
    }

    fn control(&mut self, c: char) -> Edit {
        match c.to_ascii_lowercase() {
            'b' => self.go(self.left_one()),
            'f' => self.go(self.right_one()),
            'a' => self.go(0),
            'e' => self.go(self.value.len()),
            'h' => self.cut(self.left_one(), self.cursor),
            'd' => self.cut(self.cursor, self.right_one()),
            'w' => self.cut(self.word_left(), self.cursor),
            'u' => self.cut(0, self.cursor),
            'k' => self.cut(self.cursor, self.value.len()),
            'r' => {
                self.revealed = !self.revealed;
                Edit::Moved
            }
            _ => Edit::Ignored,
        }
    }

    fn meta(&mut self, c: char) -> Edit {
        match c.to_ascii_lowercase() {
            'b' => self.go(self.word_left()),
            'f' => self.go(self.word_right()),
            'd' => self.cut(self.cursor, self.word_right()),
            _ => Edit::Ignored,
        }
    }

    fn shown(&self) -> String {
        if self.is_hidden() {
            MASK.repeat(self.value.graphemes(true).count())
        } else {
            self.value.clone()
        }
    }

    fn window(&self, width: usize) -> String {
        let cells = self.cursor_cells();
        let mut scroll = self.scroll.get();
        if cells < scroll {
            scroll = cells;
        } else if cells >= scroll + width {
            scroll = cells + 1 - width;
        }
        self.scroll.set(scroll);
        let mut out = String::new();
        let mut position = 0;
        for grapheme in self.shown().graphemes(true) {
            let step = grapheme.width();
            if position >= scroll && position + step <= scroll + width {
                out.push_str(grapheme);
            }
            position += step;
        }
        out
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let width = usize::from(area.width);
        let line = if self.value.is_empty() {
            let hint: String = self.placeholder.chars().take(width).collect();
            let pad = width.saturating_sub(hint.width());
            Line::from(vec![
                Span::styled(hint, Theme::dim().patch(Theme::field())),
                Span::styled(" ".repeat(pad), Theme::field()),
            ])
        } else {
            let visible = self.window(width);
            let pad = width.saturating_sub(visible.width());
            Line::from(vec![
                Span::styled(visible, Theme::field()),
                Span::styled(" ".repeat(pad), Theme::field()),
            ])
        };
        frame.render_widget(Paragraph::new(line), Rect { height: 1, ..area });
        if let Some(error) = &self.error
            && area.height > 1
        {
            let message = crate::tui::format::truncate(error, width.saturating_sub(2));
            let line = Line::from(vec![Span::styled(message, theme.error())]);
            let below = Rect {
                y: area.y + 1,
                height: 1,
                ..area
            };
            frame.render_widget(Paragraph::new(line), below);
        }
        if focused {
            let offset = self.cursor_cells().saturating_sub(self.scroll.get());
            let x = area.x
                + u16::try_from(offset)
                    .unwrap_or(u16::MAX)
                    .min(area.width - 1);
            frame.set_cursor_position(Position::new(x, area.y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(field: &mut TextField, code: KeyCode, modifiers: KeyModifiers) -> bool {
        field.handle_key(&KeyEvent::new(code, modifiers))
    }

    fn ctrl(field: &mut TextField, c: char) {
        press(field, KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    fn alt(field: &mut TextField, c: char) {
        press(field, KeyCode::Char(c), KeyModifiers::ALT);
    }

    fn typed(text: &str) -> TextField {
        let mut field = TextField::new();
        for c in text.chars() {
            press(&mut field, KeyCode::Char(c), KeyModifiers::NONE);
        }
        field
    }

    fn state(field: &TextField) -> (String, usize) {
        (field.value().to_owned(), field.cursor_cells())
    }

    #[test]
    fn typing_inserts_at_the_cursor() {
        let mut field = typed("ac");
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        press(&mut field, KeyCode::Char('b'), KeyModifiers::SHIFT);
        assert_eq!(state(&field), ("abc".to_owned(), 2));
    }

    #[test]
    fn left_right_and_ctrl_b_f_move_one_grapheme() {
        let mut field = typed("abc");
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 2);
        ctrl(&mut field, 'b');
        assert_eq!(field.cursor_cells(), 1);
        press(&mut field, KeyCode::Right, KeyModifiers::NONE);
        ctrl(&mut field, 'f');
        assert_eq!(field.cursor_cells(), 3);
        press(&mut field, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 3);
    }

    #[test]
    fn word_moves_cover_alt_ctrl_arrows_and_alt_b_f() {
        let mut field = typed("one two  three");
        press(&mut field, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(field.cursor_cells(), 9);
        press(&mut field, KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(field.cursor_cells(), 4);
        alt(&mut field, 'b');
        assert_eq!(field.cursor_cells(), 0);
        press(&mut field, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(field.cursor_cells(), 3);
        press(&mut field, KeyCode::Right, KeyModifiers::CONTROL);
        assert_eq!(field.cursor_cells(), 7);
        alt(&mut field, 'f');
        assert_eq!(field.cursor_cells(), 14);
    }

    #[test]
    fn home_end_ctrl_a_e_jump_to_the_edges() {
        let mut field = typed("abc");
        press(&mut field, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 0);
        press(&mut field, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 3);
        ctrl(&mut field, 'a');
        assert_eq!(field.cursor_cells(), 0);
        ctrl(&mut field, 'e');
        assert_eq!(field.cursor_cells(), 3);
    }

    #[test]
    fn backspace_and_ctrl_h_delete_before_delete_and_ctrl_d_after() {
        let mut field = typed("abcd");
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        ctrl(&mut field, 'h');
        assert_eq!(state(&field), ("ab".to_owned(), 2));
        press(&mut field, KeyCode::Home, KeyModifiers::NONE);
        press(&mut field, KeyCode::Delete, KeyModifiers::NONE);
        ctrl(&mut field, 'd');
        assert_eq!(state(&field), (String::new(), 0));
        assert!(press(&mut field, KeyCode::Backspace, KeyModifiers::NONE));
    }

    #[test]
    fn word_deletion_covers_ctrl_w_alt_backspace_and_alt_d() {
        let mut field = typed("one two three");
        ctrl(&mut field, 'w');
        assert_eq!(field.value(), "one two ");
        press(&mut field, KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(field.value(), "one ");
        press(&mut field, KeyCode::Home, KeyModifiers::NONE);
        alt(&mut field, 'd');
        assert_eq!(field.value(), " ");
    }

    #[test]
    fn line_deletion_covers_ctrl_u_and_ctrl_k() {
        let mut field = typed("hello world");
        for _ in 0..5 {
            press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        }
        ctrl(&mut field, 'k');
        assert_eq!(field.value(), "hello ");
        ctrl(&mut field, 'u');
        assert_eq!(state(&field), (String::new(), 0));
    }

    #[test]
    fn korean_cursor_uses_two_cells_per_syllable() {
        let mut field = typed("한글a");
        assert_eq!(field.cursor_cells(), 5);
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 2);
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(state(&field), ("글a".to_owned(), 0));
    }

    #[test]
    fn composed_korean_jamo_delete_as_one_grapheme() {
        let mut field = typed("\u{1112}\u{1161}\u{11ab}");
        assert_eq!(field.cursor_cells(), 2);
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(field.is_empty());
    }

    #[test]
    fn emoji_graphemes_move_and_delete_whole() {
        let mut field = typed("a👨‍👩‍👧b🇰🇷");
        assert_eq!(field.value().graphemes(true).count(), 4);
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(field.value(), "a👨‍👩‍👧b");
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(field.cursor_cells(), 3);
        press(&mut field, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(field.value(), "a👨‍👩‍👧");
        press(&mut field, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(field.value(), "a");
    }

    #[test]
    fn paste_strips_newlines_and_trims() {
        let mut field = typed("x");
        field.paste("  sk-abc\r\ndef \n");
        assert_eq!(field.value(), "xsk-abcdef");
        field.paste("\n\n");
        assert_eq!(field.value(), "xsk-abcdef");
    }

    #[test]
    fn paste_inserts_at_the_cursor() {
        let mut field = typed("ad");
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        field.paste("bc");
        assert_eq!(state(&field), ("abcd".to_owned(), 3));
    }

    #[test]
    fn mask_hides_until_ctrl_r_reveals() {
        let mut field = TextField::new().masked();
        field.paste("secret");
        assert_eq!(field.shown(), "••••••");
        ctrl(&mut field, 'r');
        assert!(field.is_revealed());
        assert_eq!(field.shown(), "secret");
        ctrl(&mut field, 'r');
        assert_eq!(field.shown(), "••••••");
    }

    #[test]
    fn masked_korean_cursor_counts_one_cell_each() {
        let mut field = TextField::new().masked();
        field.paste("한글");
        assert_eq!(field.cursor_cells(), 2);
        ctrl(&mut field, 'r');
        assert_eq!(field.cursor_cells(), 4);
    }

    #[test]
    fn enter_escape_and_release_are_not_consumed() {
        let mut field = typed("a");
        assert!(!press(&mut field, KeyCode::Enter, KeyModifiers::NONE));
        assert!(!press(&mut field, KeyCode::Esc, KeyModifiers::NONE));
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(!field.handle_key(&release));
        assert_eq!(field.value(), "a");
    }

    #[test]
    fn editing_clears_the_validation_message() {
        let mut field = typed("a");
        field.set_error("bad");
        press(&mut field, KeyCode::Left, KeyModifiers::NONE);
        assert!(field.error().is_some());
        press(&mut field, KeyCode::Char('b'), KeyModifiers::NONE);
        assert!(field.error().is_none());
    }

    #[test]
    fn window_scrolls_to_keep_the_cursor_visible() {
        let field = typed("abcdefghij");
        assert_eq!(field.window(5), "ghij");
        let mut field = field;
        press(&mut field, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(field.window(5), "abcde");
    }

    #[test]
    fn window_never_splits_a_wide_character() {
        let field = typed("가나다라마");
        assert_eq!(field.window(5), "라마");
    }
}
