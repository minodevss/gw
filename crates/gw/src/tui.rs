mod accounts;
mod data;
mod keymap;
mod keys;
mod log;
mod modal;
mod models;
mod widgets;
mod wizard;

use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind,
};
use futures_util::StreamExt as _;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use self::accounts::AccountsView;
use self::data::{LoadRequest, Snapshot};
use self::keymap::{Command, Screen};
use self::keys::{KeysView, SnippetTabs};
use self::log::LogView;
use self::modal::{Action, Closed, Confirm, NewKey, TextView};
use self::models::ModelsView;
use self::widgets::{Ctx, Theme, truncate};
use self::wizard::Wizard;
use crate::account::Account;
use crate::clock;
use crate::ops;

const TICK: Duration = Duration::from_millis(250);
const RELOAD: Duration = Duration::from_secs(1);
const HEARTBEAT_FRESH_SECS: i64 = 10;
const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;

pub(crate) enum Request {
    Lines {
        title: String,
        lines: Vec<Line<'static>>,
    },
    AddAccount,
    Relogin(Account),
    RemoveAccount(String),
    RefreshAccount(String),
    NewKey,
    KeySnippets(String),
    RevokeKey {
        name: String,
        last_used: String,
    },
    RefreshAll,
    Copy(String),
    Error(String),
}

pub(crate) enum Msg {
    Loaded(Box<anyhow::Result<Snapshot>>),
    Wizard(Box<wizard::Event>),
    KeyIssued {
        name: String,
        result: Result<ops::key::Issued, String>,
    },
    Finished(Result<String, String>),
    RefreshStep {
        account: String,
        result: Result<usize, String>,
    },
    RefreshDone,
}

enum Overlay {
    Help,
    Lines(TextView),
    Confirm(Confirm),
    NewKey(NewKey),
    Snippets(SnippetTabs),
    Wizard(Box<Wizard>),
}

enum Reaction {
    Keep,
    Close,
    Run(Action),
    CreateKey(String),
    Copy(String),
    Wizard(wizard::Output),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Ok,
    Error,
}

#[derive(Debug, Clone)]
struct Status {
    text: String,
    tone: Tone,
}

#[derive(Debug)]
struct Refresh {
    total: usize,
    done: usize,
    failures: Vec<String>,
}

#[derive(Debug, Default)]
struct Header {
    limited: usize,
    inactive: usize,
    heartbeat: Option<(i64, String)>,
}

fn server_text(heartbeat: Option<&(i64, String)>, now: i64) -> String {
    match heartbeat {
        Some((at, listen)) if now - at <= HEARTBEAT_FRESH_SECS => {
            format!("server: running {listen}")
        }
        Some(_) | None => "server: not running".to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Run,
    Quit,
}

struct App {
    home: PathBuf,
    http: reqwest::Client,
    tx: mpsc::Sender<Msg>,
    theme: Theme,
    now: i64,
    screen: Screen,
    accounts: AccountsView,
    keys: KeysView,
    models: ModelsView,
    log: LogView,
    overlay: Option<Overlay>,
    status: Option<Status>,
    db_busy: bool,
    header: Header,
    jobs: JoinSet<()>,
    loading: bool,
    last_load: Instant,
    reload_now: bool,
    refresh: Option<Refresh>,
    flow: Flow,
}

impl App {
    fn new(home: PathBuf, http: reqwest::Client, tx: mpsc::Sender<Msg>, theme: Theme) -> Self {
        Self {
            home,
            http,
            tx,
            theme,
            now: clock::now(),
            screen: Screen::Accounts,
            accounts: AccountsView::default(),
            keys: KeysView::default(),
            models: ModelsView::default(),
            log: LogView::default(),
            overlay: None,
            status: None,
            db_busy: false,
            header: Header::default(),
            jobs: JoinSet::new(),
            loading: false,
            last_load: Instant::now(),
            reload_now: true,
            refresh: None,
            flow: Flow::Run,
        }
    }

    fn ctx(&self) -> Ctx {
        Ctx {
            now: self.now,
            theme: self.theme,
        }
    }

    fn set_status(&mut self, tone: Tone, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            tone,
        });
    }

    fn server_url(&self) -> Option<String> {
        self.header
            .heartbeat
            .as_ref()
            .map(|(_, listen)| format!("http://{listen}"))
    }

    fn apply(&mut self, snapshot: Snapshot) {
        self.db_busy = false;
        self.now = snapshot.now;
        let (limited, inactive) = accounts::needs_attention(&snapshot.accounts, snapshot.now);
        self.header = Header {
            limited,
            inactive,
            heartbeat: snapshot.heartbeat,
        };
        self.accounts.update(snapshot.accounts, snapshot.detail);
        self.log.append(snapshot.log);
        let log = &self.log;
        self.keys
            .update(snapshot.keys, |name| log.last_used(name), log.is_complete());
        self.models.update(snapshot.models, snapshot.capabilities);
    }

    fn spawn_load(&mut self) {
        self.loading = true;
        let request = LoadRequest {
            account: self.accounts.selected_name(),
            model: self.models.selected_id(),
            log_after: self.log.last_id(),
        };
        let home = self.home.clone();
        let tx = self.tx.clone();
        self.jobs.spawn(async move {
            let result = data::load(&home, request).await;
            if tx.send(Msg::Loaded(Box::new(result))).await.is_err() {
                tracing::debug!("event loop closed before a load finished");
            }
        });
    }

    fn on_tick(&mut self) {
        while let Some(joined) = self.jobs.try_join_next() {
            if joined.is_err() {
                self.loading = false;
                self.set_status(Tone::Error, "a background task failed");
            }
        }
        if !self.loading && (self.reload_now || self.last_load.elapsed() >= RELOAD) {
            self.reload_now = false;
            self.spawn_load();
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Loaded(result) => {
                self.loading = false;
                self.last_load = Instant::now();
                match *result {
                    Ok(snapshot) => self.apply(snapshot),
                    Err(error) => {
                        tracing::debug!(%error, "database read failed");
                        self.db_busy = true;
                    }
                }
            }
            Msg::Wizard(event) => {
                let output = match self.overlay.as_mut() {
                    Some(Overlay::Wizard(wizard)) => wizard.event(*event),
                    Some(
                        Overlay::Help
                        | Overlay::Lines(_)
                        | Overlay::Confirm(_)
                        | Overlay::NewKey(_)
                        | Overlay::Snippets(_),
                    )
                    | None => wizard::Output::Nothing,
                };
                self.drive(output);
            }
            Msg::KeyIssued { name, result } => match result {
                Ok(issued) => {
                    self.overlay = Some(Overlay::Snippets(SnippetTabs::issued(
                        &name,
                        &issued.key,
                        issued.snippets,
                    )));
                    self.reload_now = true;
                }
                Err(error) => self.set_status(Tone::Error, error),
            },
            Msg::Finished(result) => {
                match result {
                    Ok(text) => self.set_status(Tone::Ok, text),
                    Err(error) => self.set_status(Tone::Error, error),
                }
                self.reload_now = true;
            }
            Msg::RefreshStep { account, result } => {
                let Some(refresh) = self.refresh.as_mut() else {
                    return;
                };
                refresh.done += 1;
                if let Err(reason) = result {
                    refresh.failures.push(format!("{account}: {reason}"));
                }
                let text = format!("refreshing {}/{} · {account}", refresh.done, refresh.total);
                self.set_status(Tone::Info, text);
            }
            Msg::RefreshDone => {
                if let Some(refresh) = self.refresh.take() {
                    if refresh.failures.is_empty() {
                        self.set_status(Tone::Ok, format!("refreshed {} accounts", refresh.total));
                    } else {
                        let first = refresh.failures.first().cloned().unwrap_or_default();
                        self.set_status(
                            Tone::Error,
                            format!(
                                "refreshed {}: {} failed · {first}",
                                refresh.total,
                                refresh.failures.len()
                            ),
                        );
                    }
                }
                self.reload_now = true;
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Paste(text) => self.on_paste(text),
            Event::Key(_)
            | Event::FocusGained
            | Event::FocusLost
            | Event::Mouse(_)
            | Event::Resize(..) => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        match self.overlay.as_mut() {
            Some(Overlay::Wizard(wizard)) => wizard.paste(text),
            Some(Overlay::Confirm(confirm)) => confirm.paste(text),
            Some(Overlay::NewKey(new_key)) => new_key.paste(text),
            Some(Overlay::Help | Overlay::Lines(_) | Overlay::Snippets(_)) => {}
            None => match self.screen {
                Screen::Accounts => self.accounts.paste(text),
                Screen::Models => self.models.paste(text),
                Screen::Log => self.log.paste(text),
                Screen::Keys => {}
            },
        }
    }

    fn editing_filter(&self) -> bool {
        match self.screen {
            Screen::Accounts => self.accounts.is_editing(),
            Screen::Models => self.models.is_editing(),
            Screen::Log => self.log.is_editing(),
            Screen::Keys => false,
        }
    }

    fn on_key(&mut self, key: &KeyEvent) {
        if keymap::is_interrupt(key) {
            self.flow = Flow::Quit;
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        if self.editing_filter() {
            let request = match self.screen {
                Screen::Accounts => {
                    self.accounts.edit_filter(key);
                    None
                }
                Screen::Models => {
                    self.models.edit_filter(key);
                    None
                }
                Screen::Log => self.log.edit_filter(key),
                Screen::Keys => None,
            };
            if let Some(request) = request {
                self.request(request);
            }
            return;
        }
        let Some(command) = keymap::lookup(self.screen, key) else {
            return;
        };
        match command {
            Command::Goto(screen) => self.screen = screen,
            Command::Quit => self.flow = Flow::Quit,
            Command::Help => self.overlay = Some(Overlay::Help),
            Command::Down
            | Command::Up
            | Command::Top
            | Command::Bottom
            | Command::Enter
            | Command::Back
            | Command::Filter
            | Command::New
            | Command::Login
            | Command::Remove
            | Command::Copy
            | Command::Refresh => {
                let request = match self.screen {
                    Screen::Accounts => self.accounts.command(command, self.now, self.theme),
                    Screen::Keys => self.keys.command(command, self.now),
                    Screen::Models => self.models.command(command, self.theme),
                    Screen::Log => self.log.command(command, self.theme),
                };
                if let Some(request) = request {
                    self.request(request);
                }
            }
        }
    }

    fn overlay_key(&mut self, key: &KeyEvent) {
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        let reaction = match &mut overlay {
            Overlay::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?' | 'q')
                ) {
                    Reaction::Close
                } else {
                    Reaction::Keep
                }
            }
            Overlay::Lines(view) => match view.handle(key) {
                Closed::Cancel | Closed::Done(()) => Reaction::Close,
                Closed::Stay => Reaction::Keep,
            },
            Overlay::Confirm(confirm) => match confirm.handle(key) {
                Closed::Done(action) => Reaction::Run(action),
                Closed::Cancel => Reaction::Close,
                Closed::Stay => Reaction::Keep,
            },
            Overlay::NewKey(new_key) => match new_key.handle(key) {
                Closed::Done(name) => Reaction::CreateKey(name),
                Closed::Cancel => Reaction::Close,
                Closed::Stay => Reaction::Keep,
            },
            Overlay::Snippets(tabs) => match tabs.handle(key) {
                Closed::Done(text) => Reaction::Copy(text),
                Closed::Cancel => Reaction::Close,
                Closed::Stay => Reaction::Keep,
            },
            Overlay::Wizard(wizard) => Reaction::Wizard(wizard.key(key)),
        };
        match reaction {
            Reaction::Keep => self.overlay = Some(overlay),
            Reaction::Close => {}
            Reaction::Run(action) => self.run(action),
            Reaction::CreateKey(name) => self.create_key(name),
            Reaction::Copy(text) => {
                self.overlay = Some(overlay);
                self.copy(&text);
            }
            Reaction::Wizard(output) => {
                self.overlay = Some(overlay);
                self.drive(output);
            }
        }
    }

    fn drive(&mut self, output: wizard::Output) {
        match output {
            wizard::Output::Nothing => {}
            wizard::Output::Close => {
                self.overlay = None;
                self.reload_now = true;
            }
            wizard::Output::Copy(text) => self.copy(&text),
            wizard::Output::BeginLogin(_)
            | wizard::Output::Complete { .. }
            | wizard::Output::Save { .. } => {
                if let Some(Overlay::Wizard(wizard)) = self.overlay.as_mut() {
                    let task = wizard::launch(
                        output,
                        wizard.generation(),
                        &self.home,
                        &self.http,
                        &self.tx,
                    );
                    wizard.set_task(task);
                }
            }
        }
    }

    fn request(&mut self, request: Request) {
        match request {
            Request::Lines { title, lines } => {
                self.overlay = Some(Overlay::Lines(TextView::new(title, lines)));
            }
            Request::AddAccount => {
                let existing = self.accounts.accounts().to_vec();
                self.overlay = Some(Overlay::Wizard(Box::new(Wizard::add(existing))));
            }
            Request::Relogin(account) => {
                let existing = self.accounts.accounts().to_vec();
                let (wizard, output) = Wizard::relogin(&account, existing);
                self.overlay = Some(Overlay::Wizard(Box::new(wizard)));
                self.drive(output);
            }
            Request::RemoveAccount(name) => {
                self.overlay = Some(Overlay::Confirm(Confirm::type_name(
                    "Remove account",
                    vec![format!(
                        "{name} and its model list are deleted from the database"
                    )],
                    &name.clone(),
                    Action::RemoveAccount(name),
                )));
            }
            Request::RefreshAccount(name) => self.start_refresh(vec![name]),
            Request::RefreshAll => {
                let names: Vec<String> = self
                    .accounts
                    .accounts()
                    .iter()
                    .map(|a| a.name.clone())
                    .collect();
                if names.is_empty() {
                    self.set_status(Tone::Error, "no accounts to refresh");
                    return;
                }
                let question = format!("refresh {} accounts?", names.len());
                self.overlay = Some(Overlay::Confirm(Confirm::yes_no(
                    &question,
                    Action::Refresh(names),
                )));
            }
            Request::NewKey => {
                if self.server_url().is_none() {
                    self.set_status(
                        Tone::Error,
                        "server address unknown: start `gw serve` once first",
                    );
                } else {
                    self.overlay = Some(Overlay::NewKey(NewKey::default()));
                }
            }
            Request::KeySnippets(name) => {
                let base = self
                    .server_url()
                    .unwrap_or_else(|| "http://<server>".to_owned());
                self.overlay = Some(Overlay::Snippets(SnippetTabs::placeholder(
                    &name,
                    ops::key::snippets(&base, "<key>"),
                )));
            }
            Request::RevokeKey { name, last_used } => {
                self.overlay = Some(Overlay::Confirm(Confirm::type_name(
                    "Revoke key",
                    vec![
                        format!("LAST USED  {last_used}"),
                        "clients using this key stop working at once".to_owned(),
                    ],
                    &name.clone(),
                    Action::RevokeKey(name),
                )));
            }
            Request::Copy(text) => self.copy(&text),
            Request::Error(message) => self.set_status(Tone::Error, message),
        }
    }

    fn run(&mut self, action: Action) {
        let home = self.home.clone();
        let tx = self.tx.clone();
        match action {
            Action::RemoveAccount(name) => {
                self.jobs.spawn(async move {
                    let result = ops::account::remove(&home, &name)
                        .await
                        .map(|()| format!("{name}: removed"))
                        .map_err(|error| format!("{error:#}"));
                    finish(&tx, result).await;
                });
            }
            Action::RevokeKey(name) => {
                self.jobs.spawn(async move {
                    let wanted = name.clone();
                    let result = ops::with_store(&home, move |store| store.revoke_key(&wanted))
                        .await
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|found| {
                            if found {
                                Ok(format!("{name}: revoked"))
                            } else {
                                Err(format!("no key named {name}"))
                            }
                        });
                    finish(&tx, result).await;
                });
            }
            Action::Refresh(names) => self.start_refresh(names),
        }
    }

    fn create_key(&mut self, name: String) {
        let Some(base) = self.server_url() else {
            self.set_status(Tone::Error, "server address unknown");
            return;
        };
        let home = self.home.clone();
        let tx = self.tx.clone();
        self.jobs.spawn(async move {
            let wanted = name.clone();
            let result = ops::with_store(&home, move |store| {
                ops::key::create(store, &wanted, &base, clock::now())
                    .map_err(|error| crate::store::Error::Invalid(error.to_string()))
            })
            .await
            .map_err(|error| format!("{error:#}"));
            if tx.send(Msg::KeyIssued { name, result }).await.is_err() {
                tracing::debug!("event loop closed before a key was issued");
            }
        });
    }

    fn start_refresh(&mut self, names: Vec<String>) {
        if self.refresh.is_some() {
            self.set_status(Tone::Error, "a refresh is already running");
            return;
        }
        self.refresh = Some(Refresh {
            total: names.len(),
            done: 0,
            failures: Vec::new(),
        });
        self.set_status(Tone::Info, format!("refreshing 0/{}", names.len()));
        let home = self.home.clone();
        let http = self.http.clone();
        let tx = self.tx.clone();
        self.jobs.spawn(async move {
            for account in names {
                let result = match ops::model::refresh(&home, &http, Some(&account)).await {
                    Ok(report) => report
                        .into_iter()
                        .next()
                        .map_or(Err("no result".to_owned()), |r| r.result),
                    Err(error) => Err(format!("{error:#}")),
                };
                if tx.send(Msg::RefreshStep { account, result }).await.is_err() {
                    return;
                }
            }
            if tx.send(Msg::RefreshDone).await.is_err() {
                tracing::debug!("event loop closed before a refresh finished");
            }
        });
    }

    fn copy(&mut self, text: &str) {
        let mut out = std::io::stdout();
        let written = out
            .write_all(widgets::osc52(text).as_bytes())
            .and_then(|()| out.flush());
        match written {
            Ok(()) => self.set_status(Tone::Ok, "copied (OSC 52)"),
            Err(error) => self.set_status(Tone::Error, format!("copy failed: {error}")),
        }
    }

    fn hints(&self) -> String {
        match &self.overlay {
            Some(Overlay::Help | Overlay::Lines(_)) => "esc close".to_owned(),
            Some(Overlay::Confirm(confirm)) => keymap::hints_text(&confirm.hints()),
            Some(Overlay::NewKey(_)) => "enter create  esc cancel".to_owned(),
            Some(Overlay::Snippets(tabs)) => keymap::hints_text(&tabs.hints()),
            Some(Overlay::Wizard(wizard)) => keymap::hints_text(&wizard.hints()),
            None if self.editing_filter() => "enter apply  esc clear".to_owned(),
            None => keymap::footer_text(self.screen),
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            frame.render_widget(
                Paragraph::new(format!(
                    "terminal too small (need {MIN_WIDTH}×{MIN_HEIGHT})"
                )),
                area,
            );
            return;
        }
        let ctx = self.ctx();
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);
        self.render_header(frame, header);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_style(ctx.theme.dim());
        let inner = block.inner(body);
        frame.render_widget(block, body);
        match self.screen {
            Screen::Accounts => self.accounts.render(frame, inner, &ctx),
            Screen::Keys => self.keys.render(frame, inner, &ctx),
            Screen::Models => self.models.render(frame, inner, &ctx),
            Screen::Log => self.log.render(frame, inner, &ctx),
        }
        match &self.overlay {
            Some(Overlay::Help) => modal::render_help(frame, body, self.screen, &ctx),
            Some(Overlay::Lines(view)) => view.render(frame, body),
            Some(Overlay::Confirm(confirm)) => confirm.render(frame, body, &ctx),
            Some(Overlay::NewKey(new_key)) => new_key.render(frame, body, &ctx),
            Some(Overlay::Snippets(tabs)) => tabs.render(frame, body, &ctx),
            Some(Overlay::Wizard(wizard)) => wizard.render(frame, body, &ctx),
            None => {}
        }
        self.render_footer(frame, footer);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let theme = self.theme;
        let mut spans = vec![Span::styled(" gw ", theme.accent())];
        for screen in Screen::ALL {
            let label = format!(" {} {} ", screen.number(), screen.title());
            let style = if screen == self.screen {
                theme.selected()
            } else {
                Style::new()
            };
            spans.push(Span::styled(label, style));
        }
        let right = format!(
            "{} limited · {} inactive    {} ",
            self.header.limited,
            self.header.inactive,
            server_text(self.header.heartbeat.as_ref(), self.now)
        );
        let width = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
        frame.render_widget(Paragraph::new(Line::from(spans)), left_area);
        let attention = self.header.limited + self.header.inactive > 0;
        let style = if attention {
            theme.warn()
        } else {
            Style::new()
        };
        frame.render_widget(Paragraph::new(Span::styled(right, style)), right_area);
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let theme = self.theme;
        let mut parts: Vec<Span<'static>> = Vec::new();
        if let Some(status) = &self.status {
            let (mark, style) = match status.tone {
                Tone::Info => ("", Style::new()),
                Tone::Ok => ("✓ ", theme.ok()),
                Tone::Error => ("✗ ", theme.bad()),
            };
            parts.push(Span::styled(
                truncate(
                    &format!("{mark}{}", status.text),
                    usize::from(area.width / 2),
                ),
                style,
            ));
        }
        if self.db_busy {
            if !parts.is_empty() {
                parts.push(Span::raw(" · "));
            }
            parts.push(Span::styled("db busy", theme.warn()));
        }
        parts.push(Span::raw(" "));
        let width: usize = parts.iter().map(Span::width).sum();
        let [hints, status] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(u16::try_from(width).unwrap_or(u16::MAX)),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {}", self.hints()), theme.dim())),
            hints,
        );
        frame.render_widget(Paragraph::new(Line::from(parts)), status);
    }
}

async fn finish(tx: &mpsc::Sender<Msg>, result: Result<String, String>) {
    if tx.send(Msg::Finished(result)).await.is_err() {
        tracing::debug!("event loop closed before an action finished");
    }
}

struct TerminalGuard {
    terminal: DefaultTerminal,
}

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        let terminal = ratatui::try_init()?;
        if let Err(error) = crossterm::execute!(std::io::stdout(), EnableBracketedPaste) {
            ratatui::restore();
            return Err(error);
        }
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(error) = crossterm::execute!(std::io::stdout(), DisableBracketedPaste) {
            tracing::warn!(%error, "could not disable bracketed paste");
        }
        ratatui::restore();
    }
}

pub(crate) async fn run(home: &Path) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        bail!(
            "gw tui needs an interactive terminal; use the CLI instead: \
             gw account list, gw key list, gw model list, gw log --follow"
        );
    }
    let http = reqwest::Client::builder().build()?;
    let (tx, mut rx) = mpsc::channel(64);
    let mut app = App::new(home.to_path_buf(), http, tx, Theme::from_env());
    let mut guard = TerminalGuard::enter().context("cannot start the terminal UI")?;
    event_loop(&mut guard.terminal, &mut app, &mut rx).await
}

async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::Receiver<Msg>,
) -> anyhow::Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    loop {
        app.now = clock::now();
        terminal.draw(|frame| app.render(frame))?;
        tokio::select! {
            event = events.next() => match event {
                Some(event) => app.on_event(&event?),
                None => break,
            },
            _ = tick.tick() => app.on_tick(),
            Some(msg) = rx.recv() => app.on_msg(msg),
        }
        if app.flow == Flow::Quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::account::{Secret, State};
    use crate::provider::{Kind, Method, Provider};
    use crate::tui::widgets::testing;

    const NOW: i64 = 1_800_000_000;

    fn press(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn app() -> App {
        let (tx, _rx) = mpsc::channel(8);
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            reqwest::Client::new(),
            tx,
            Theme::new(false),
        );
        let account = |name: &str, state: State| Account {
            name: name.to_owned(),
            kind: Kind::new(Provider::OpenAi, Method::Key).unwrap(),
            secret: Secret::Key { key: String::new() },
            state,
            usage: None,
            usage_at: None,
            created_at: NOW,
        };
        app.apply(Snapshot {
            now: NOW,
            accounts: vec![
                account("openai-key-1", State::Active),
                account(
                    "openai-key-2",
                    State::Inactive {
                        reason: "refresh failed".to_owned(),
                    },
                ),
            ],
            detail: None,
            keys: Vec::new(),
            models: Vec::new(),
            capabilities: None,
            heartbeat: Some((NOW - 3, "100.64.0.2:8787".to_owned())),
            log: Vec::new(),
        });
        app
    }

    #[test]
    fn server_status_goes_stale_after_ten_seconds() {
        let beat = (NOW - 10, "100.64.0.2:8787".to_owned());
        assert_eq!(
            server_text(Some(&beat), NOW),
            "server: running 100.64.0.2:8787"
        );
        assert_eq!(server_text(Some(&beat), NOW + 1), "server: not running");
        assert_eq!(server_text(None, NOW), "server: not running");
    }

    #[test]
    fn digits_switch_screens_and_q_quits() {
        let mut app = app();
        app.on_event(&press(KeyCode::Char('4')));
        assert_eq!(app.screen, Screen::Log);
        app.on_event(&press(KeyCode::Char('q')));
        assert_eq!(app.flow, Flow::Quit);
    }

    #[test]
    fn typing_q_into_the_remove_prompt_does_not_quit() {
        let mut app = app();
        app.on_event(&press(KeyCode::Char('d')));
        assert!(matches!(app.overlay, Some(Overlay::Confirm(_))));
        app.on_event(&press(KeyCode::Char('q')));
        assert_eq!(app.flow, Flow::Run);
        app.on_event(&press(KeyCode::Esc));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn ctrl_c_quits_even_inside_a_modal() {
        let mut app = app();
        app.on_event(&press(KeyCode::Char('n')));
        assert!(matches!(app.overlay, Some(Overlay::Wizard(_))));
        app.on_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(app.flow, Flow::Quit);
    }

    #[test]
    fn creating_a_key_needs_a_known_server_address() {
        let mut app = app();
        app.header.heartbeat = None;
        app.on_event(&press(KeyCode::Char('2')));
        app.on_event(&press(KeyCode::Char('n')));
        assert!(app.overlay.is_none());
        assert_eq!(app.status.as_ref().map(|s| s.tone), Some(Tone::Error));
    }

    #[test]
    fn failed_load_keeps_the_last_frame_and_flags_db_busy() {
        let mut app = app();
        app.on_msg(Msg::Loaded(Box::new(Err(anyhow::anyhow!("locked")))));
        assert!(app.db_busy);
        assert_eq!(app.accounts.accounts().len(), 2);
    }

    #[test]
    fn small_terminal_shows_only_the_size_message() {
        let mut app = app();
        let text = testing::render(79, 23, |frame| app.render(frame));
        assert_eq!(text.trim(), "terminal too small (need 80×24)");
    }

    #[test]
    fn renders_full_frame_with_header_and_footer() {
        let mut app = app();
        app.set_status(Tone::Ok, "copied (OSC 52)");
        let text = testing::render(110, 28, |frame| app.render(frame));
        insta::assert_snapshot!("frame_accounts", text);
    }

    #[test]
    fn renders_help_overlay() {
        let mut app = app();
        app.on_event(&press(KeyCode::Char('?')));
        let text = testing::render(100, 26, |frame| app.render(frame));
        insta::assert_snapshot!("frame_help", text);
    }
}
