use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{Env, Filter, draw, empty, heading, panes, plural};
use crate::account::Account;
use crate::catalog::Capabilities;
use crate::display::{Health, health};
use crate::format;
use crate::ops::snapshot::ModelInfo;
use crate::provider::Billing;
use crate::select::NoCandidate;
use crate::tui::action::{Command, Effect, Reply};
use crate::tui::keymap::{Context, shortcut};
use crate::tui::modal::Modal;
use crate::tui::theme::Theme;
use crate::tui::widget::list::{ListState, Row};
use crate::tui::widget::menu::{Item, Menu};

const PAGE: isize = 10;

#[derive(Debug, Clone, Default)]
pub(crate) struct State {
    list: ListState,
    filter: Filter,
    detail: bool,
}

fn visible<'a>(env: &Env<'a>, filter: &Filter) -> Vec<&'a ModelInfo> {
    env.snapshot
        .models
        .iter()
        .filter(|info| filter.text().is_empty() || filter.matches(&info.view.id))
        .collect()
}

fn rows(models: &[&ModelInfo]) -> Vec<Row> {
    models
        .iter()
        .map(|info| Some(info.view.id.clone()))
        .collect()
}

fn context_text(tokens: Option<u64>) -> String {
    tokens.map_or_else(String::new, |tokens| {
        format::compact(i64::try_from(tokens).unwrap_or(i64::MAX))
    })
}

impl State {
    pub(crate) fn detail_open(&self) -> bool {
        self.detail
    }

    pub(crate) fn filter_mut(&mut self) -> &mut Filter {
        &mut self.filter
    }

    fn selected<'a>(&self, env: &Env<'a>) -> Option<&'a ModelInfo> {
        let id = self.list.selected()?;
        env.snapshot.models.iter().find(|info| info.view.id == id)
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
        match command {
            Command::Up => self.list.step(&rows, -1),
            Command::Down => self.list.step(&rows, 1),
            Command::PageUp => self.list.step(&rows, -PAGE),
            Command::PageDown => self.list.step(&rows, PAGE),
            Command::Top => self.list.top(&rows),
            Command::Bottom => self.list.bottom(&rows),
            Command::Right => self.detail = !env.wide && self.list.selected().is_some(),
            Command::Left => self.detail = false,
            Command::Back => {
                if self.detail {
                    self.detail = false;
                } else {
                    self.filter.clear();
                }
            }
            Command::Filter => self.filter.open(),
            Command::RefreshModels => return Reply::Do(Effect::RefreshModels),
            Command::Copy => {
                if let Some(info) = self.selected(env) {
                    return Reply::Do(Effect::Copy(info.view.id.clone()));
                }
            }
            Command::Select => return self.menu(env),
            Command::Quit
            | Command::NextView
            | Command::PrevView
            | Command::Goto(_)
            | Command::Help
            | Command::Add
            | Command::Relogin
            | Command::Rename
            | Command::Remove
            | Command::RefreshUsage
            | Command::CopySettings
            | Command::FilterAccount
            | Command::FilterSession => {}
        }
        Reply::None
    }

    fn menu(&self, env: &Env<'_>) -> Reply {
        let Some(info) = self.selected(env) else {
            return Reply::None;
        };
        let item = |label: &str, command| Item {
            label: label.to_owned(),
            command,
            shortcut: shortcut(Context::Models, command),
            unavailable: None,
        };
        Reply::Push(Modal::Menu(Menu::new(
            info.view.id.clone(),
            vec![
                item("Copy the model name", Command::Copy),
                item(
                    "Refresh model lists (asks every provider)",
                    Command::RefreshModels,
                ),
            ],
        )))
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, env: &Env<'_>) {
        let theme = env.theme;
        if env.snapshot.models.is_empty() {
            empty(
                frame,
                area,
                "No models yet.",
                "Add an account to see its models. Press r to fetch the lists.",
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
                    &format!("No model matches “{}”.", self.filter.text()),
                    "Press esc to clear the filter.",
                );
            } else {
                let ids = rows(&all);
                let range = self.list.visible(&ids, usize::from(list.height));
                let name_cells = all
                    .iter()
                    .map(|info| format::width(&info.view.id))
                    .max()
                    .unwrap_or(8)
                    .clamp(8, usize::from(list.width).saturating_sub(24).max(8));
                let lines: Vec<Line<'_>> = all
                    .iter()
                    .enumerate()
                    .skip(range.start)
                    .take(range.len())
                    .map(|(index, info)| {
                        let chosen = index == self.list.index();
                        let marker = " ";
                        let line = Line::from(vec![
                            Span::raw(format!(
                                " {marker} {}  ",
                                format::fit(&info.view.id, name_cells)
                            )),
                            Span::raw(format!(
                                "{:<12}",
                                plural(info.view.accounts.len(), "account", "accounts")
                            )),
                            Span::styled(context_text(info.view.context), Theme::dim()),
                        ]);
                        if chosen { Theme::select(line) } else { line }
                    })
                    .collect();
                draw(frame, list, lines);
            }
        }
        if let Some(detail) = panes.detail
            && let Some(info) = self.selected(env)
        {
            draw(
                frame,
                detail,
                detail_lines(info, env, usize::from(detail.width)),
            );
        }
    }
}

fn health_text(account: &Account, env: &Env<'_>) -> (String, ratatui::style::Style) {
    let theme = env.theme;
    match health(account, env.clock.now) {
        Health::Ready => ("ready".to_owned(), Theme::dim()),
        Health::Limited { reset_at, .. } => (
            format!("limited, {}", env.clock.until(reset_at)),
            theme.warn(),
        ),
        Health::SignedOut(_) => ("sign in needed".to_owned(), theme.error()),
    }
}

fn billing_word(billing: Billing) -> &'static str {
    match billing {
        Billing::Sub => "subscription",
        Billing::Api => "API key",
    }
}

fn why(info: &ModelInfo, env: &Env<'_>, chosen: &Account) -> &'static str {
    let serving: Vec<&Account> = info
        .view
        .accounts
        .iter()
        .filter_map(|name| env.snapshot.account(name))
        .collect();
    let peers = serving
        .iter()
        .filter(|a| a.kind.billing() == chosen.kind.billing())
        .count();
    if serving.len() == 1 {
        "only account"
    } else if peers > 1 && chosen.last_reset().is_some() {
        "resets soonest"
    } else if peers > 1 {
        "added first"
    } else if chosen.kind.billing() == Billing::Sub {
        "API keys come last"
    } else {
        "no subscription available"
    }
}

pub(crate) fn detail_lines(info: &ModelInfo, env: &Env<'_>, width: usize) -> Vec<Line<'static>> {
    let theme = env.theme;
    let clock = env.clock;
    let id = &info.view.id;
    let mut lines = vec![
        Line::styled(
            format!(" {}", format::truncate(id, width.saturating_sub(1))),
            theme.accent(),
        ),
        Line::raw(""),
        heading("Next request"),
    ];
    match &info.next {
        Ok(choice) => {
            let detail = env
                .snapshot
                .account(&choice.account)
                .map_or_else(String::new, |a| {
                    format!(
                        "  ({}, {})",
                        billing_word(a.kind.billing()),
                        why(info, env, a)
                    )
                });
            lines.push(Line::raw(format!("  {}{detail}", choice.account)));
        }
        Err(NoCandidate::Limited { reset_at }) => {
            let when = reset_at.map_or_else(String::new, |at| {
                format!(
                    " Earliest reset {} (in {}).",
                    clock.when(at),
                    clock.until(at)
                )
            });
            lines.push(Line::styled(
                format!("  Every account is limited.{when}"),
                theme.warn(),
            ));
        }
        Err(NoCandidate::UnknownModel | NoCandidate::Unsupported(_)) => {
            lines.push(Line::styled("  No account can take it", theme.error()));
        }
    }
    lines.push(Line::raw(""));
    lines.push(heading("Names"));
    let mut serving: Vec<&Account> = info
        .view
        .accounts
        .iter()
        .filter_map(|name| env.snapshot.account(name))
        .collect();
    serving.sort_by(|a, b| crate::select::order(a, b));
    let name_cells = format::width(id) + 4;
    lines.push(Line::raw(format!("  {id:<name_cells$}  any account")));
    if serving.iter().any(|a| a.kind.billing() == Billing::Sub) {
        lines.push(Line::raw(format!(
            "  {:<name_cells$}  subscriptions only",
            format!("{id}:sub")
        )));
    }
    if serving.iter().any(|a| a.kind.billing() == Billing::Api) {
        lines.push(Line::raw(format!(
            "  {:<name_cells$}  API keys only",
            format!("{id}:api")
        )));
    }
    lines.push(Line::raw(""));
    lines.push(heading("Accounts"));
    let account_cells = serving
        .iter()
        .map(|a| format::width(&a.name))
        .max()
        .unwrap_or(0);
    for account in &serving {
        let (text, style) = health_text(account, env);
        lines.push(Line::from(vec![
            Span::raw(format!("  {}  ", format::fit(&account.name, account_cells))),
            Span::styled(text, style),
        ]));
    }
    let limits = limit_lines(&serving, id, env);
    if !limits.is_empty() {
        lines.push(Line::raw(""));
        lines.push(heading("Limits"));
        lines.extend(limits);
    }
    lines
}

fn missing(caps: &Capabilities) -> Vec<String> {
    let mut missing = Vec::new();
    for (value, label) in [
        (caps.fast, "fast mode"),
        (caps.image, "images"),
        (caps.document, "documents"),
        (caps.structured_output, "structured output"),
    ] {
        if value == Some(false) {
            missing.push(label.to_owned());
        }
    }
    missing.extend(caps.unsupported.iter().map(|name| name.replace('_', " ")));
    missing
}

fn limit_lines(serving: &[&Account], model: &str, env: &Env<'_>) -> Vec<Line<'static>> {
    let per_account: Vec<(&str, Vec<String>)> = serving
        .iter()
        .filter_map(|account| {
            env.snapshot
                .catalog
                .get(&account.name, model)
                .map(|caps| (account.name.as_str(), missing(caps)))
        })
        .collect();
    let mut lines = Vec::new();
    for (name, features) in &per_account {
        let differing: Vec<&String> = features
            .iter()
            .filter(|feature| {
                per_account
                    .iter()
                    .any(|(other, theirs)| other != name && !theirs.contains(feature))
            })
            .collect();
        if !differing.is_empty() {
            let list: Vec<&str> = differing.iter().map(|f| f.as_str()).collect();
            lines.push(Line::raw(format!(
                "  {name} does not support: {}",
                list.join(", ")
            )));
        }
    }
    lines
}
