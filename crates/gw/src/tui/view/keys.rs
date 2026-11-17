use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{Env, Filter, draw, empty, heading, panes};
use crate::ops::key;
use crate::tui::action::{Command, Effect, Reply};
use crate::tui::data::KeyInfo;
use crate::tui::flow::new_key::NewKey;
use crate::tui::format;
use crate::tui::keymap::{Context, shortcut};
use crate::tui::modal::Modal;
use crate::tui::theme::Theme;
use crate::tui::widget::confirm::Confirm;
use crate::tui::widget::list::{ListState, Row};
use crate::tui::widget::menu::{Item, Menu};
use crate::tui::widget::tabs;

const PAGE: isize = 10;
pub(crate) const PLACEHOLDER: &str = "<your key>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Client {
    pub(crate) name: &'static str,
    pub(crate) text: String,
}

pub(crate) fn clients(snippets: Vec<key::Snippet>) -> Vec<Client> {
    let mut clients = Vec::new();
    let mut other = Vec::new();
    for snippet in snippets {
        match snippet.client {
            "Claude Code" | "Codex" => clients.push(Client {
                name: snippet.client,
                text: snippet.text,
            }),
            _ => other.push(snippet.text),
        }
    }
    clients.push(Client {
        name: "Other",
        text: other.join("\n"),
    });
    clients
}

pub(crate) fn render_client(frame: &mut Frame<'_>, area: Rect, clients: &[Client], current: usize) {
    let titles: Vec<&str> = clients.iter().map(|c| c.name).collect();
    let mut lines = vec![
        Line::from(
            std::iter::once(Span::raw(" "))
                .chain(tabs::line(&titles, current))
                .chain(std::iter::once(Span::styled("     ←→", Theme::dim())))
                .collect::<Vec<_>>(),
        ),
        Line::raw(""),
    ];
    if let Some(client) = clients.get(current) {
        lines.extend(client.text.lines().map(|text| {
            Line::raw(format::truncate(
                &format!(" {text}"),
                usize::from(area.width),
            ))
        }));
    }
    draw(frame, area, lines);
}

#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    list: ListState,
    filter: Filter,
    detail: bool,
    client: usize,
}

fn visible<'a>(env: &Env<'a>, filter: &Filter) -> Vec<&'a KeyInfo> {
    env.snapshot
        .keys
        .iter()
        .filter(|key| filter.text().is_empty() || filter.matches(&key.name))
        .collect()
}

fn rows(keys: &[&KeyInfo]) -> Vec<Row> {
    keys.iter().map(|key| Some(key.name.clone())).collect()
}

fn used_text(key: &KeyInfo, env: &Env<'_>) -> String {
    key.last_used.map_or_else(
        || "never used".to_owned(),
        |at| format!("used {}", env.clock.since(at)),
    )
}

fn client_list(env: &Env<'_>) -> Vec<Client> {
    clients(key::snippets(&env.snapshot.base_url(), PLACEHOLDER))
}

impl State {
    pub(crate) fn detail_open(&self) -> bool {
        self.detail
    }

    pub(crate) fn filter_mut(&mut self) -> &mut Filter {
        &mut self.filter
    }

    fn selected<'a>(&self, env: &Env<'a>) -> Option<&'a KeyInfo> {
        let name = self.list.selected()?;
        env.snapshot.keys.iter().find(|key| key.name == name)
    }

    pub(crate) fn sync(&mut self, env: &Env<'_>) {
        let rows = rows(&visible(env, &self.filter));
        self.list.sync(&rows);
        if rows.is_empty() {
            self.detail = false;
        }
    }

    pub(crate) fn handle(&mut self, command: Command, env: &Env<'_>) -> Reply {
        let rows = rows(&visible(env, &self.filter));
        let count = client_list(env).len();
        match command {
            Command::Up => self.list.step(&rows, -1),
            Command::Down => self.list.step(&rows, 1),
            Command::PageUp => self.list.step(&rows, -PAGE),
            Command::PageDown => self.list.step(&rows, PAGE),
            Command::Top => self.list.top(&rows),
            Command::Bottom => self.list.bottom(&rows),
            Command::Right => {
                if env.wide || self.detail {
                    self.client = (self.client + 1).min(count.saturating_sub(1));
                } else {
                    self.detail = self.list.selected().is_some();
                }
            }
            Command::Left => {
                if self.client > 0 {
                    self.client -= 1;
                } else {
                    self.detail = false;
                }
            }
            Command::Back => {
                if self.detail {
                    self.detail = false;
                } else {
                    self.filter.clear();
                }
            }
            Command::Filter => self.filter.open(),
            Command::Add => {
                return Reply::Push(Modal::NewKey(Box::new(NewKey::new(
                    env.snapshot.base_url(),
                ))));
            }
            Command::Copy => {
                if let Some(client) = client_list(env).get(self.client) {
                    return Reply::Do(Effect::Copy(client.text.clone()));
                }
            }
            Command::Remove => return self.revoke(env),
            Command::Select => return self.menu(env),
            Command::Quit
            | Command::NextView
            | Command::PrevView
            | Command::Goto(_)
            | Command::Help
            | Command::Relogin
            | Command::Rename
            | Command::RefreshUsage
            | Command::RefreshModels
            | Command::CopySettings
            | Command::FilterAccount
            | Command::FilterSession => {}
        }
        Reply::None
    }

    fn revoke(&self, env: &Env<'_>) -> Reply {
        self.selected(env).map_or(Reply::None, |key| {
            Reply::Push(Modal::Confirm(Confirm::new(
                format!("Revoke {}?", key.name),
                "Devices using this key stop working. You can issue a new key later.",
                "Revoke",
                Effect::Revoke(key.name.clone()),
            )))
        })
    }

    fn menu(&self, env: &Env<'_>) -> Reply {
        let Some(key) = self.selected(env) else {
            return Reply::None;
        };
        let item = |label: &str, command| Item {
            label: label.to_owned(),
            command,
            shortcut: shortcut(Context::Keys, command),
            unavailable: None,
        };
        Reply::Push(Modal::Menu(Menu::new(
            key.name.clone(),
            vec![
                item("Copy client settings", Command::Copy),
                item("Revoke", Command::Remove),
            ],
        )))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, env: &Env<'_>) {
        let theme = env.theme;
        if env.snapshot.keys.is_empty() {
            empty(
                frame,
                area,
                "No keys yet.",
                "Press a to create the key for your first device.",
            );
            return;
        }
        let panes = panes(area, env.wide, self.detail);
        let all = visible(env, &self.filter);
        if let Some(list) = panes.list {
            let list = self.filter.render(frame, list, theme);
            if all.is_empty() {
                empty(
                    frame,
                    list,
                    &format!("No key matches “{}”.", self.filter.text()),
                    "Press esc to clear the filter.",
                );
            } else {
                let ids = rows(&all);
                let range = self.list.visible(&ids, usize::from(list.height));
                let name_cells = all
                    .iter()
                    .map(|key| format::width(&key.name))
                    .max()
                    .unwrap_or(8)
                    .clamp(8, usize::from(list.width).saturating_sub(22).max(8));
                let lines: Vec<Line<'_>> = all
                    .iter()
                    .enumerate()
                    .skip(range.start)
                    .take(range.len())
                    .map(|(index, key)| {
                        let chosen = index == self.list.index();
                        let marker = " ";
                        let line = Line::from(vec![
                            Span::raw(format!(
                                " {marker} {}  ",
                                format::fit(&key.name, name_cells)
                            )),
                            Span::styled(used_text(key, env), Theme::dim()),
                        ]);
                        if chosen { Theme::select(line) } else { line }
                    })
                    .collect();
                draw(frame, list, lines);
            }
        }
        if let Some(detail) = panes.detail
            && let Some(key) = self.selected(env)
        {
            self.render_detail(frame, detail, key, env);
        }
    }

    fn render_detail(&self, frame: &mut Frame<'_>, area: Rect, key: &KeyInfo, env: &Env<'_>) {
        let theme = env.theme;
        let head = vec![
            Line::styled(format!(" {}", key.name), theme.accent()),
            Line::styled(
                format!(" created {}", env.clock.when(key.created_at)),
                Theme::dim(),
            ),
            Line::raw(""),
            heading("Connect a client"),
        ];
        let used = u16::try_from(head.len()).unwrap_or(4);
        draw(frame, area, head);
        let below = Rect {
            y: area.y + used,
            height: area.height.saturating_sub(used),
            ..area
        };
        render_client(frame, below, &client_list(env), self.client);
    }
}
