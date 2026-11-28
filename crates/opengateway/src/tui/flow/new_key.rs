use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ops::name;
use crate::ops::snapshot::Snapshot;
use crate::tui::action::{Command, Effect, Reply};
use crate::tui::jobs::{JobDone, Output, Work};
use crate::tui::keymap::Context;
use crate::tui::theme::Theme;
use crate::tui::view::keys::{Client, clients, render_client};
use crate::tui::widget::dialog;
use crate::tui::widget::text_field::TextField;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Name,
    Creating,
    Show,
}

struct Shown {
    key: String,
    clients: Vec<Client>,
    current: usize,
}

pub(crate) struct NewKey {
    base_url: String,
    step: Step,
    name: TextField,
    created: String,
    shown: Option<Shown>,
}

impl NewKey {
    pub(crate) fn new(base_url: String) -> Self {
        Self {
            base_url,
            step: Step::Name,
            name: TextField::new().with_placeholder("for example: macbook"),
            created: String::new(),
            shown: None,
        }
    }

    pub(crate) fn context(&self) -> Context {
        match self.step {
            Step::Name => Context::Done,
            Step::Creating => Context::Busy,
            Step::Show => Context::OneTime,
        }
    }

    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        (self.step == Step::Name).then_some(&mut self.name)
    }

    pub(crate) fn command(&mut self, command: Command, snapshot: &Snapshot) -> Reply {
        match (self.step, command) {
            (Step::Name, Command::Back) | (Step::Show, Command::Select) => Reply::Close,
            (Step::Name, Command::Select) => {
                let taken: Vec<String> = snapshot.keys.iter().map(|k| k.name.clone()).collect();
                let value = self.name.value().trim().to_owned();
                match name::check(&value, &taken) {
                    Ok(()) => {
                        self.step = Step::Creating;
                        self.created.clone_from(&value);
                        Reply::Do(Effect::CreateKey {
                            name: value,
                            base_url: self.base_url.clone(),
                        })
                    }
                    Err(error) => {
                        self.name
                            .set_error(crate::format::capitalize(&error.to_string()));
                        Reply::None
                    }
                }
            }
            (Step::Show, Command::Copy) => self.shown.as_ref().map_or(Reply::None, |shown| {
                Reply::Do(Effect::Copy(shown.key.clone()))
            }),
            (Step::Show, Command::CopySettings) => self
                .shown
                .as_ref()
                .and_then(|shown| shown.clients.get(shown.current))
                .map_or(Reply::None, |client| {
                    Reply::Do(Effect::Copy(client.text.clone()))
                }),
            (Step::Show, Command::Left) => self.switch(-1),
            (Step::Show, Command::Right) => self.switch(1),
            (_, _) => Reply::None,
        }
    }

    fn switch(&mut self, delta: isize) -> Reply {
        if let Some(shown) = self.shown.as_mut() {
            let last = shown.clients.len().saturating_sub(1);
            shown.current = shown.current.saturating_add_signed(delta).min(last);
        }
        Reply::None
    }

    pub(crate) fn job(&mut self, done: JobDone) -> Result<Reply, JobDone> {
        if done.work != Work::CreateKey || self.step != Step::Creating {
            return Err(done);
        }
        match done.result {
            Ok(Output::KeyCreated { issued, .. }) => {
                self.shown = Some(Shown {
                    key: issued.key,
                    clients: clients(issued.snippets),
                    current: 0,
                });
                self.step = Step::Show;
            }
            Ok(
                Output::Prompt { .. }
                | Output::Secret(_)
                | Output::Saved { .. }
                | Output::Relogged(_)
                | Output::Renamed { .. }
                | Output::Removed(_)
                | Output::UsageRefreshed(_)
                | Output::ModelsRefreshed { .. }
                | Output::Revoked(_),
            ) => {}
            Err(error) => {
                self.step = Step::Name;
                self.name.set_error(format!("{error:#}"));
            }
        }
        Ok(Reply::None)
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        if let Some(shown) = &self.shown {
            self.render_shown(frame, area, theme, shown);
            return;
        }
        let outer = dialog::centered(area, 52, 8);
        let inner = dialog::open(frame, outer, "New key", "");
        let label = if self.step == Step::Creating {
            format!("Creating {}…", self.created)
        } else {
            "Device name".to_owned()
        };
        frame.render_widget(
            Paragraph::new(Line::raw(label)),
            Rect { height: 1, ..inner },
        );
        if self.step == Step::Name {
            let field = Rect {
                y: inner.y + 2,
                height: 2,
                ..inner
            };
            self.name.render(frame, field, theme, true);
        }
    }

    fn render_shown(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, shown: &Shown) {
        let inner = dialog::page(frame, area, &format!("New key {}", self.created));
        let head = vec![
            Line::raw(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(shown.key.clone(), theme.accent()),
            ]),
            Line::raw(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("Shown once. Copy it now.", theme.warn()),
            ]),
            Line::raw(""),
        ];
        let used = u16::try_from(head.len()).unwrap_or(5);
        frame.render_widget(Paragraph::new(head), inner);
        let below = Rect {
            y: inner.y + used,
            height: inner.height.saturating_sub(used + 1),
            ..inner
        };
        render_client(frame, below, &shown.clients, shown.current);
    }
}
