use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::action::{Command, Effect, Reply, Tone};
use super::data::Snapshot;
use super::flow::add_account::AddAccount;
use super::flow::new_key::NewKey;
use super::format::{self, Clock};
use super::jobs::JobDone;
use super::keymap::{self, Context};
use super::theme::Theme;
use super::widget::confirm::{Choice, Confirm};
use super::widget::dialog;
use super::widget::menu::Menu;
use super::widget::text_field::TextField;
use crate::ops::name;

pub(crate) enum Modal {
    Menu(Menu),
    Confirm(Confirm),
    Help(Help),
    Rename(Rename),
    AddAccount(Box<AddAccount>),
    NewKey(Box<NewKey>),
}

#[derive(Debug, Clone)]
pub(crate) struct Help {
    rows: Vec<(String, &'static str)>,
}

impl Help {
    pub(crate) fn new(context: Context) -> Self {
        Self {
            rows: keymap::help(context),
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let keys = self
            .rows
            .iter()
            .map(|(keys, _)| format::width(keys))
            .max()
            .unwrap_or(0);
        let labels = self
            .rows
            .iter()
            .map(|(_, label)| format::width(label))
            .max()
            .unwrap_or(0);
        let width = u16::try_from(keys + labels + 8).unwrap_or(u16::MAX);
        let height = u16::try_from(self.rows.len() + 2).unwrap_or(u16::MAX);
        let outer = dialog::centered(area, width, height);
        let inner = dialog::open(frame, outer, "Keys on this screen", "");
        let lines: Vec<Line<'_>> = self
            .rows
            .iter()
            .map(|(keys_text, label)| {
                Line::from(vec![
                    Span::styled(format::fit(keys_text, keys), theme.accent()),
                    Span::raw(format!("  {label}")),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Rename {
    title: &'static str,
    old: String,
    field: TextField,
    taken: Vec<String>,
    make: fn(String, String) -> Effect,
}

impl Rename {
    pub(crate) fn new(
        title: &'static str,
        old: &str,
        taken: Vec<String>,
        make: fn(String, String) -> Effect,
    ) -> Self {
        Self {
            title,
            old: old.to_owned(),
            field: TextField::new().with_value(old),
            taken: taken.into_iter().filter(|name| name != old).collect(),
            make,
        }
    }

    fn submit(&mut self) -> Reply {
        let new = self.field.value().trim().to_owned();
        if new == self.old {
            return Reply::Close;
        }
        match name::check(&new, &self.taken) {
            Ok(()) => Reply::CloseDo((self.make)(self.old.clone(), new)),
            Err(error) => {
                self.field.set_error(format::capitalize(&error.to_string()));
                Reply::None
            }
        }
    }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let outer = dialog::centered(area, 52, 7);
        let inner = dialog::open(frame, outer, self.title, &self.old);
        frame.render_widget(
            Paragraph::new(Line::styled("New name", Theme::dim())),
            Rect { height: 1, ..inner },
        );
        let field = Rect {
            y: inner.y + 1,
            height: 2,
            ..inner
        };
        self.field.render(frame, field, theme, true);
    }
}

impl Modal {
    pub(crate) fn context(&self) -> Context {
        match self {
            Self::Menu(_) => Context::Menu,
            Self::Confirm(_) => Context::Confirm,
            Self::Help(_) => Context::Help,
            Self::Rename(_) => Context::Done,
            Self::AddAccount(wizard) => wizard.context(),
            Self::NewKey(flow) => flow.context(),
        }
    }

    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        match self {
            Self::Rename(rename) => Some(&mut rename.field),
            Self::AddAccount(wizard) => wizard.field_mut(),
            Self::NewKey(flow) => flow.field_mut(),
            Self::Menu(_) | Self::Confirm(_) | Self::Help(_) => None,
        }
    }

    pub(crate) fn opened(&mut self) -> Option<Effect> {
        match self {
            Self::AddAccount(wizard) => wizard.opened(),
            Self::Menu(_)
            | Self::Confirm(_)
            | Self::Help(_)
            | Self::Rename(_)
            | Self::NewKey(_) => None,
        }
    }

    pub(crate) fn shortcut(&self, key: &KeyEvent) -> Option<Reply> {
        let Self::Menu(menu) = self else {
            return None;
        };
        let KeyCode::Char(c) = key.code else {
            return None;
        };
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        menu.by_shortcut(c).map(|item| match &item.unavailable {
            Some(why) => Reply::Toast(Tone::Error, format!("{}: {why}", item.label)),
            None => Reply::Command(item.command),
        })
    }

    pub(crate) fn command(&mut self, command: Command, snapshot: &Snapshot) -> Reply {
        match self {
            Self::Menu(menu) => Self::menu_command(menu, command),
            Self::Confirm(confirm) => Self::confirm_command(confirm, command),
            Self::Help(_) => {
                if matches!(command, Command::Back | Command::Select) {
                    Reply::Close
                } else {
                    Reply::None
                }
            }
            Self::Rename(rename) => {
                if command == Command::Select {
                    rename.submit()
                } else if command == Command::Back {
                    Reply::Close
                } else {
                    Reply::None
                }
            }
            Self::AddAccount(wizard) => wizard.command(command, snapshot),
            Self::NewKey(flow) => flow.command(command, snapshot),
        }
    }

    fn menu_command(menu: &mut Menu, command: Command) -> Reply {
        if command == Command::Up {
            menu.step(-1);
        } else if command == Command::Down {
            menu.step(1);
        } else if command == Command::Back {
            return Reply::Close;
        } else if command == Command::Select {
            return match menu.current() {
                Some(item) => match &item.unavailable {
                    Some(why) => Reply::Toast(Tone::Error, format!("{}: {why}", item.label)),
                    None => Reply::Command(item.command),
                },
                None => Reply::Close,
            };
        }
        Reply::None
    }

    fn confirm_command(confirm: &mut Confirm, command: Command) -> Reply {
        if command == Command::Left {
            confirm.pick(Choice::Cancel);
        } else if command == Command::Right {
            confirm.pick(Choice::Confirm);
        } else if command == Command::Back {
            return Reply::Close;
        } else if command == Command::Select {
            return match confirm.choice() {
                Choice::Confirm => Reply::CloseDo(confirm.effect.clone()),
                Choice::Cancel => Reply::Close,
            };
        }
        Reply::None
    }

    pub(crate) fn job(&mut self, done: JobDone) -> Result<Reply, JobDone> {
        match self {
            Self::AddAccount(wizard) => wizard.job(done),
            Self::NewKey(flow) => flow.job(done),
            Self::Menu(_) | Self::Confirm(_) | Self::Help(_) | Self::Rename(_) => Err(done),
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, clock: &Clock) {
        match self {
            Self::Menu(menu) => menu.render(frame, area),
            Self::Confirm(confirm) => confirm.render(frame, area, theme),
            Self::Help(help) => help.render(frame, area, theme),
            Self::Rename(rename) => rename.render(frame, area, theme),
            Self::AddAccount(wizard) => wizard.render(frame, area, theme, clock),
            Self::NewKey(flow) => flow.render(frame, area, theme),
        }
    }
}
