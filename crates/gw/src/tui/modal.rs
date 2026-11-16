use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::keymap::{self, Screen};
use super::widgets::{Ctx, Input, centered};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    RemoveAccount(String),
    RevokeKey(String),
    Refresh(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Closed<T> {
    Stay,
    Cancel,
    Done(T),
}

#[derive(Debug)]
enum Kind {
    TypeName { expected: String, input: Input },
    YesNo,
}

#[derive(Debug)]
pub(crate) struct Confirm {
    title: String,
    lines: Vec<String>,
    kind: Kind,
    action: Action,
    mismatch: bool,
}

impl Confirm {
    pub(crate) fn type_name(
        title: &str,
        lines: Vec<String>,
        expected: &str,
        action: Action,
    ) -> Self {
        Self {
            title: title.to_owned(),
            lines,
            kind: Kind::TypeName {
                expected: expected.to_owned(),
                input: Input::default(),
            },
            action,
            mismatch: false,
        }
    }

    pub(crate) fn yes_no(question: &str, action: Action) -> Self {
        Self {
            title: question.to_owned(),
            lines: Vec::new(),
            kind: Kind::YesNo,
            action,
            mismatch: false,
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.kind {
            Kind::TypeName { .. } => vec![("enter", "confirm"), ("esc", "cancel")],
            Kind::YesNo => vec![("y", "yes"), ("n/esc", "no")],
        }
    }

    pub(crate) fn handle(&mut self, key: &KeyEvent) -> Closed<Action> {
        if key.code == KeyCode::Esc {
            return Closed::Cancel;
        }
        match &mut self.kind {
            Kind::YesNo => {
                if matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
                    Closed::Done(self.action.clone())
                } else if matches!(key.code, KeyCode::Char('n' | 'N')) {
                    Closed::Cancel
                } else {
                    Closed::Stay
                }
            }
            Kind::TypeName { expected, input } => {
                if key.code == KeyCode::Enter {
                    if input.text() == expected {
                        return Closed::Done(self.action.clone());
                    }
                    self.mismatch = true;
                } else if input.handle(key) {
                    self.mismatch = false;
                }
                Closed::Stay
            }
        }
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        if let Kind::TypeName { input, .. } = &mut self.kind {
            input.paste(pasted);
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let mut lines: Vec<Line<'static>> = self.lines.iter().cloned().map(Line::from).collect();
        match &self.kind {
            Kind::TypeName { expected, input } => {
                lines.push(Line::default());
                lines.push(Line::from(format!("type {expected} to confirm")));
                lines.push(input.line(50, true, ctx.theme));
                if self.mismatch {
                    lines.push(Line::styled("name does not match", ctx.theme.bad()));
                }
            }
            Kind::YesNo => lines.push(Line::styled("y yes · n no", ctx.theme.dim())),
        }
        let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
        let rect = centered(area, 64, height);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(format!(" {} ", self.title))),
            rect,
        );
    }
}

#[derive(Debug)]
pub(crate) struct TextView {
    title: String,
    lines: Vec<Line<'static>>,
    scroll: u16,
}

impl TextView {
    pub(crate) fn new(title: String, lines: Vec<Line<'static>>) -> Self {
        Self {
            title,
            lines,
            scroll: 0,
        }
    }

    pub(crate) fn handle(&mut self, key: &KeyEvent) -> Closed<()> {
        let code = key.code;
        if matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            Closed::Cancel
        } else if matches!(code, KeyCode::Char('j') | KeyCode::Down) {
            let last = u16::try_from(self.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
            self.scroll = (self.scroll + 1).min(last);
            Closed::Stay
        } else if matches!(code, KeyCode::Char('k') | KeyCode::Up) {
            self.scroll = self.scroll.saturating_sub(1);
            Closed::Stay
        } else {
            Closed::Stay
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let rect = centered(
            area,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(self.lines.clone())
                .scroll((self.scroll, 0))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(format!(" {} ", self.title))),
            rect,
        );
    }
}

#[derive(Debug, Default)]
pub(crate) struct NewKey {
    input: Input,
}

impl NewKey {
    pub(crate) fn handle(&mut self, key: &KeyEvent) -> Closed<String> {
        if key.code == KeyCode::Esc {
            return Closed::Cancel;
        }
        if key.code == KeyCode::Enter {
            let name = self.input.text().trim().to_owned();
            return if name.is_empty() {
                Closed::Stay
            } else {
                Closed::Done(name)
            };
        }
        self.input.handle(key);
        Closed::Stay
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        self.input.paste(pasted);
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let rect = centered(area, 56, 5);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("name for the new key"),
                self.input.line(50, true, ctx.theme),
                Line::styled("enter create · esc cancel", ctx.theme.dim()),
            ])
            .block(Block::bordered().title(" Create key ")),
            rect,
        );
    }
}

pub(crate) fn render_help(frame: &mut Frame<'_>, area: Rect, screen: Screen, ctx: &Ctx) {
    let lines: Vec<Line<'static>> = keymap::help(screen)
        .into_iter()
        .map(|(keys, label)| {
            Line::from(vec![
                Span::styled(
                    format!("{keys:<12}"),
                    ctx.theme.accent().add_modifier(Modifier::BOLD),
                ),
                Span::raw(label),
            ])
        })
        .collect();
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let rect = centered(area, 56, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(format!(" Help · {} ", screen.title()))),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn removal_needs_the_exact_name() {
        let mut confirm = Confirm::type_name(
            "Remove",
            Vec::new(),
            "ab",
            Action::RemoveAccount("ab".to_owned()),
        );
        confirm.handle(&press(KeyCode::Char('a')));
        assert_eq!(confirm.handle(&press(KeyCode::Enter)), Closed::Stay);
        confirm.handle(&press(KeyCode::Char('b')));
        assert_eq!(
            confirm.handle(&press(KeyCode::Enter)),
            Closed::Done(Action::RemoveAccount("ab".to_owned()))
        );
    }

    #[test]
    fn escape_cancels_a_confirmation() {
        let mut confirm = Confirm::yes_no("refresh 2 accounts?", Action::Refresh(Vec::new()));
        assert_eq!(confirm.handle(&press(KeyCode::Esc)), Closed::Cancel);
    }

    #[test]
    fn yes_confirms_a_refresh() {
        let mut confirm =
            Confirm::yes_no("refresh 2 accounts?", Action::Refresh(vec!["a".to_owned()]));
        assert_eq!(
            confirm.handle(&press(KeyCode::Char('y'))),
            Closed::Done(Action::Refresh(vec!["a".to_owned()]))
        );
    }

    #[test]
    fn empty_key_name_is_not_submitted() {
        let mut modal = NewKey::default();
        assert_eq!(modal.handle(&press(KeyCode::Enter)), Closed::Stay);
        modal.handle(&press(KeyCode::Char('m')));
        assert_eq!(
            modal.handle(&press(KeyCode::Enter)),
            Closed::Done("m".to_owned())
        );
    }
}
