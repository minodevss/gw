mod action;
mod data;
mod flow;
mod jobs;
mod keymap;
mod modal;
mod theme;
mod view;
mod widget;

#[cfg(test)]
mod tests;

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use futures_util::StreamExt as _;
use jiff::tz::TimeZone;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use tokio::sync::mpsc;

use self::action::{Action, Command, Reply, Tone, update};
use self::jobs::Jobs;
use self::keymap::Context;
use self::modal::Modal;
use self::theme::Theme;
use self::view::{Env, Filter, View, accounts, activity, keys, models};
use self::widget::tabs;
use crate::clock;
use crate::format::{self, Clock, truncate};
use crate::ops::snapshot::{Server, Snapshot};

const TINT: f32 = 0.08;
const TICK: Duration = Duration::from_millis(250);
const INFO_LIFETIME: Duration = Duration::from_secs(4);
const WIDE_COLUMNS: u16 = 110;
const MIN_COLUMNS: u16 = 60;
const MIN_ROWS: u16 = 16;
const COPY_PREVIEW_CELLS: usize = 48;
const CHANNEL_DEPTH: usize = 64;

struct Status {
    text: String,
    tone: Tone,
    since: Instant,
}

pub(crate) struct App {
    theme: Theme,
    clock: Clock,
    snapshot: Snapshot,
    loaded: bool,
    view: View,
    accounts: accounts::State,
    activity: activity::State,
    models: models::State,
    keys: keys::State,
    modals: Vec<Modal>,
    status: Option<Status>,
    db_error: Option<String>,
    jobs: Jobs,
    clipboard: Vec<String>,
    wide: bool,
    quit: bool,
}

impl App {
    fn new(jobs: Jobs, theme: Theme, zone: TimeZone) -> Self {
        Self {
            theme,
            clock: Clock::new(clock::now(), zone),
            snapshot: Snapshot::default(),
            loaded: false,
            view: View::Accounts,
            accounts: accounts::State::default(),
            activity: activity::State::default(),
            models: models::State::default(),
            keys: keys::State::default(),
            modals: Vec::new(),
            status: None,
            db_error: None,
            jobs,
            clipboard: Vec::new(),
            wide: false,
            quit: false,
        }
    }

    fn env(&self) -> Env<'_> {
        Env {
            snapshot: &self.snapshot,
            clock: &self.clock,
            theme: self.theme,
            wide: self.wide,
        }
    }

    fn apply(&mut self, snapshot: Snapshot) {
        self.clock.now = snapshot.now;
        self.snapshot = snapshot;
        self.loaded = true;
        self.db_error = None;
    }

    fn tick(&mut self) {
        self.clock.now = clock::now();
        if self
            .status
            .as_ref()
            .is_some_and(|s| s.tone == Tone::Info && s.since.elapsed() >= INFO_LIFETIME)
        {
            self.status = None;
        }
    }

    fn set_status(&mut self, tone: Tone, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            tone,
            since: Instant::now(),
        });
    }

    fn copy(&mut self, text: &str) {
        self.clipboard.push(osc52(text));
        let first = text.lines().next().unwrap_or_default();
        let more = if text.lines().count() > 1 { " …" } else { "" };
        let preview = truncate(first, COPY_PREVIEW_CELLS);
        self.set_status(Tone::Info, format!("Copied {preview}{more}"));
    }

    fn paste(&mut self, text: &str) {
        if let Some(field) = self.field_mut() {
            field.paste(text);
        }
    }

    fn filter_mut(&mut self) -> &mut Filter {
        match self.view {
            View::Accounts => self.accounts.filter_mut(),
            View::Activity => self.activity.filter_mut(),
            View::Models => self.models.filter_mut(),
            View::Keys => self.keys.filter_mut(),
        }
    }

    fn is_filtering(&mut self) -> bool {
        self.modals.is_empty() && self.filter_mut().is_editing()
    }

    fn field_mut(&mut self) -> Option<&mut widget::text_field::TextField> {
        if !self.modals.is_empty() {
            return self.modals.last_mut().and_then(Modal::field_mut);
        }
        let filter = self.filter_mut();
        filter.is_editing().then(|| filter.field_mut())
    }

    fn detail_open(&self) -> bool {
        !self.wide
            && match self.view {
                View::Accounts => self.accounts.detail_open(),
                View::Models => self.models.detail_open(),
                View::Keys => self.keys.detail_open(),
                View::Activity => false,
            }
    }

    fn context(&mut self) -> Context {
        if let Some(modal) = self.modals.last() {
            return modal.context();
        }
        if self.is_filtering() {
            Context::Filter
        } else {
            Context::of(self.view)
        }
    }

    fn handle_view(&mut self, command: Command) -> Reply {
        if self.is_filtering() {
            let filter = self.filter_mut();
            if command == Command::Select {
                filter.apply();
            } else if command == Command::Back {
                filter.clear();
            }
            return Reply::None;
        }
        let env = Env {
            snapshot: &self.snapshot,
            clock: &self.clock,
            theme: self.theme,
            wide: self.wide,
        };
        match self.view {
            View::Accounts => self.accounts.handle(command, &env),
            View::Activity => self.activity.handle(command, &env),
            View::Models => self.models.handle(command, &env),
            View::Keys => self.keys.handle(command, &env),
        }
    }

    fn sync_views(&mut self) {
        let env = Env {
            snapshot: &self.snapshot,
            clock: &self.clock,
            theme: self.theme,
            wide: self.wide,
        };
        self.accounts.sync(&env);
        self.activity.sync(&env);
        self.models.sync(&env);
        self.keys.sync(&env);
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.status.as_ref().is_some_and(|s| s.tone == Tone::Error) {
            self.status = None;
        }
        let context = self.context();
        if let Some(field) = self.field_mut()
            && field.handle_key(&key)
        {
            self.sync_views();
            return;
        }
        if let Some(reply) = self.modals.last().and_then(|modal| modal.shortcut(&key)) {
            update(self, Action::Reply(reply));
            return;
        }
        if let Some(command) = keymap::lookup(context, &key) {
            update(self, Action::Command(command));
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Paste(text) => update(self, Action::Paste(text)),
            Event::Resize(width, _) => self.wide = width >= WIDE_COLUMNS,
            Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
        }
    }
}

fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text))
}

fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    app.wide = area.width >= WIDE_COLUMNS;
    if area.width < MIN_COLUMNS || area.height < MIN_ROWS {
        let message = format!(
            "Terminal too small. gw needs at least {MIN_COLUMNS}x{MIN_ROWS}, this is {}x{}.",
            area.width, area.height
        );
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: true }), area);
        return;
    }
    let theme = app.theme;
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, header, app);
    if app.loaded {
        draw_body(frame, body, app);
    }
    if !app.modals.is_empty() {
        frame.buffer_mut().set_style(body, Theme::backdrop());
    }
    let modals = std::mem::take(&mut app.modals);
    for modal in &modals {
        modal.render(frame, area, theme, &app.clock);
    }
    app.modals = modals;
    draw_footer(frame, footer, app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = app.theme;
    let titles: Vec<&str> = View::ALL.iter().map(|view| view.title()).collect();
    let mut spans = vec![Span::styled(" gw", theme.accent()), Span::raw("    ")];
    spans.extend(tabs::line(&titles, app.view.index()));
    let left = Line::from(spans);
    let right = if app.loaded {
        match app.snapshot.server() {
            Server::Running(addr) => Line::from(vec![
                Span::styled("running", theme.ok()),
                Span::styled(format!("  {addr} "), Theme::dim()),
            ]),
            Server::Stopped => Line::from(vec![Span::styled("stopped ", theme.error())]),
        }
    } else {
        Line::raw("")
    };
    let right_cells = u16::try_from(right.width()).unwrap_or(0);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_cells)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(Paragraph::new(right), right_area);
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = app.theme;
    let env = app.env();
    let mut body = area;
    if app.snapshot.server() == Server::Stopped {
        let [banner, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" Server is not running. ", theme.error()),
                Span::raw("Start it with: gw serve --listen <addr>"),
            ])),
            banner,
        );
        body = rest;
    }
    match app.view {
        View::Accounts => app.accounts.render(frame, body, &env),
        View::Activity => app.activity.render(frame, body, &env),
        View::Models => app.models.render(frame, body, &env),
        View::Keys => app.keys.render(frame, body, &env),
    }
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let theme = app.theme;
    let context = if app.modals.is_empty() && !app.is_filtering() && app.detail_open() {
        Context::Detail
    } else {
        app.context()
    };
    let hints = keymap::footer(context)
        .iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect::<Vec<_>>()
        .join("   ");
    let (note, style) = match (&app.status, &app.db_error) {
        (Some(status), _) => match status.tone {
            Tone::Info => (status.text.clone(), theme.ok()),
            Tone::Error => (status.text.clone(), theme.error()),
        },
        (None, Some(error)) => (format!("Can't read the database: {error}"), theme.error()),
        (None, None) => (String::new(), Theme::dim()),
    };
    let width = usize::from(area.width);
    let note = truncate(&note, width.saturating_sub(2));
    let note_cells = format::width(&note);
    let room = width.saturating_sub(note_cells + 3);
    let hints = truncate(&hints, room);
    let pad = width.saturating_sub(format::width(&hints) + note_cells + 2);
    let line = Line::from(vec![
        Span::styled(format!(" {hints}"), Theme::dim()),
        Span::raw(" ".repeat(pad)),
        Span::styled(note, style),
        Span::raw(" "),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

struct Screen {
    terminal: ratatui::DefaultTerminal,
}

impl Screen {
    fn enter() -> anyhow::Result<Self> {
        let terminal = ratatui::init();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(error) = execute!(std::io::stdout(), DisableBracketedPaste) {
                tracing::warn!(%error, "could not turn off bracketed paste");
            }
            previous(info);
        }));
        execute!(std::io::stdout(), EnableBracketedPaste)
            .context("could not turn on bracketed paste")?;
        Ok(Self { terminal })
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Err(error) = execute!(std::io::stdout(), DisableBracketedPaste) {
            tracing::warn!(%error, "could not turn off bracketed paste");
        }
        if let Err(error) = ratatui::try_restore() {
            tracing::warn!(%error, "could not restore the terminal");
        }
    }
}

pub(crate) async fn run(home: &Path) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(CHANNEL_DEPTH);
    let jobs = Jobs::new(home.to_path_buf(), tx);
    let theme = Theme::detect();
    Theme::set_selection_tint(theme.is_colored().then(selection_tint).flatten());
    let mut app = App::new(jobs, theme, TimeZone::system());
    app.jobs.start_loader();
    let mut screen = Screen::enter()?;
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while !app.quit {
        screen.terminal.draw(|frame| draw(frame, &mut app))?;
        for sequence in app.clipboard.drain(..) {
            screen
                .terminal
                .backend_mut()
                .write_all(sequence.as_bytes())?;
        }
        screen.terminal.backend_mut().flush()?;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(event)) => app.on_event(event),
                Some(Err(error)) => return Err(error).context("terminal input failed"),
                None => break,
            },
            Some(action) = rx.recv() => update(&mut app, action),
            Some(joined) = app.jobs.join_next() => {
                if let Err(error) = joined && !error.is_cancelled() {
                    tracing::error!(%error, "a background task failed");
                    app.set_status(Tone::Error, "A background task stopped unexpectedly");
                }
            }
            _ = ticker.tick() => update(&mut app, Action::Tick),
        }
    }
    app.jobs.shutdown();
    Ok(())
}

fn selection_tint() -> Option<ratatui::style::Color> {
    use terminal_colorsaurus::{QueryOptions, ThemeMode, color_palette};
    let palette = color_palette(QueryOptions::default()).ok()?;
    let truecolor =
        std::env::var("COLORTERM").is_ok_and(|value| value == "truecolor" || value == "24bit");
    if truecolor {
        let mix = |bg: u16, fg: u16| {
            let (bg, fg) = (f32::from(bg >> 8), f32::from(fg >> 8));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the blend stays within 0..=255"
            )]
            let channel = (bg + (fg - bg) * TINT).round() as u8;
            channel
        };
        let (bg, fg) = (palette.background, palette.foreground);
        Some(ratatui::style::Color::Rgb(
            mix(bg.r, fg.r),
            mix(bg.g, fg.g),
            mix(bg.b, fg.b),
        ))
    } else {
        Some(ratatui::style::Color::Indexed(match palette.theme_mode() {
            ThemeMode::Dark => 236,
            ThemeMode::Light => 254,
        }))
    }
}
