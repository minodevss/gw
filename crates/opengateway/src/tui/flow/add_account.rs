use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use url::Url;

use crate::account::{Account, Secret};
use crate::clock;
use crate::display::{method_text, provider_title};
use crate::format::{self, Clock};
use crate::ops::snapshot::Snapshot;
use crate::ops::{self, name};
use crate::provider::{Billing, Kind, Method, Prompt, Provider};
use crate::tui::action::{Command, Effect, Reply, Tone};
use crate::tui::jobs::{JobDone, LoginHandle, Models, Output, Work};
use crate::tui::keymap::Context;
use crate::tui::theme::Theme;
use crate::tui::widget::dialog;
use crate::tui::widget::text_field::TextField;

const SERVICES: [(&str, &str, Provider); 5] = [
    ("Claude", "Anthropic", Provider::Anthropic),
    ("ChatGPT / OpenAI", "", Provider::OpenAi),
    ("GLM", "Z.ai", Provider::Zai),
    ("Kimi", "Moonshot", Provider::Moonshot),
    (
        "Other endpoint",
        "Ollama, LM Studio, vLLM, any compatible API",
        Provider::Custom,
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Service,
    Method,
    Endpoint,
    Credential,
    Login,
    Saving,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Url,
    Api,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    New,
    Again {
        name: String,
        kind: Kind,
        endpoint: Option<(String, String)>,
    },
}

struct Session {
    handle: LoginHandle,
    prompt: Option<Prompt>,
    opened: bool,
    sent: bool,
}

pub(crate) struct AddAccount {
    mode: Mode,
    step: Step,
    service: usize,
    method: usize,
    focus: Focus,
    secret: TextField,
    url: TextField,
    api: TextField,
    code: TextField,
    name: TextField,
    session: Option<Session>,
    login_error: Option<String>,
    saved: Option<String>,
    summary: String,
}

impl AddAccount {
    pub(crate) fn new() -> Self {
        Self {
            mode: Mode::New,
            step: Step::Service,
            service: 0,
            method: 0,
            focus: Focus::Url,
            secret: TextField::new().masked().with_placeholder("paste here"),
            url: TextField::new().with_placeholder("http://localhost:11434"),
            api: TextField::new().with_placeholder("chat, responses or messages"),
            code: TextField::new().with_placeholder("paste the code"),
            name: TextField::new(),
            session: None,
            login_error: None,
            saved: None,
            summary: String::new(),
        }
    }

    pub(crate) fn again(account: &Account) -> Self {
        let endpoint = match &account.secret {
            Secret::Custom { url, api, .. } => Some((url.clone(), api.clone())),
            Secret::Oauth { .. } | Secret::Token { .. } | Secret::Key { .. } => None,
        };
        let step = if ops::account::needs_paste(account.kind) || endpoint.is_some() {
            Step::Credential
        } else {
            Step::Login
        };
        Self {
            mode: Mode::Again {
                name: account.name.clone(),
                kind: account.kind,
                endpoint,
            },
            step,
            ..Self::new()
        }
    }

    fn provider(&self) -> Provider {
        match &self.mode {
            Mode::New => SERVICES
                .get(self.service)
                .map_or(Provider::Anthropic, |(_, _, provider)| *provider),
            Mode::Again { kind, .. } => kind.provider(),
        }
    }

    fn kind(&self) -> Option<Kind> {
        match &self.mode {
            Mode::Again { kind, .. } => Some(*kind),
            Mode::New => {
                let provider = self.provider();
                let method = *provider.methods().get(self.method)?;
                Kind::new(provider, method).ok()
            }
        }
    }

    fn is_custom(&self) -> bool {
        self.provider() == Provider::Custom
    }

    pub(crate) fn opened(&mut self) -> Option<Effect> {
        (self.step == Step::Login && self.session.is_none())
            .then(|| self.kind())
            .flatten()
            .map(|kind| Effect::StartLogin { kind })
    }

    pub(crate) fn attach(&mut self, handle: LoginHandle) {
        self.login_error = None;
        self.session = Some(Session {
            handle,
            prompt: None,
            opened: false,
            sent: false,
        });
    }

    pub(crate) fn context(&self) -> Context {
        match self.step {
            Step::Service | Step::Method => Context::Choice,
            Step::Endpoint | Step::Credential => Context::Input,
            Step::Login if self.login_error.is_some() => Context::Failed,
            Step::Login => match &self.session {
                Some(Session {
                    prompt: Some(Prompt::PasteCode { .. } | Prompt::Loopback { .. }),
                    sent: false,
                    ..
                }) => Context::Code,
                Some(_) | None => Context::Waiting,
            },
            Step::Saving => Context::Busy,
            Step::Done => Context::Done,
        }
    }

    pub(crate) fn field_mut(&mut self) -> Option<&mut TextField> {
        match self.step {
            Step::Endpoint => Some(match self.focus {
                Focus::Url => &mut self.url,
                Focus::Api => &mut self.api,
            }),
            Step::Credential => Some(&mut self.secret),
            Step::Login if self.context() == Context::Code => Some(&mut self.code),
            Step::Done if self.mode == Mode::New => Some(&mut self.name),
            Step::Service | Step::Method | Step::Login | Step::Saving | Step::Done => None,
        }
    }

    fn leave_login(&mut self) {
        self.session = None;
        self.login_error = None;
    }

    fn back(&mut self) -> Reply {
        match self.step {
            Step::Service | Step::Done => return Reply::Close,
            Step::Method => self.step = Step::Service,
            Step::Endpoint => match self.focus {
                Focus::Api => self.focus = Focus::Url,
                Focus::Url => self.step = Step::Service,
            },
            Step::Credential => {
                if self.mode != Mode::New {
                    return Reply::Close;
                }
                self.step = if self.is_custom() {
                    Step::Endpoint
                } else {
                    Step::Method
                };
                self.focus = Focus::Api;
            }
            Step::Login => {
                self.leave_login();
                if self.mode != Mode::New {
                    return Reply::Close;
                }
                self.step = Step::Method;
            }
            Step::Saving => {}
        }
        Reply::None
    }

    fn pick(&mut self, delta: isize) {
        match self.step {
            Step::Service => {
                let last = SERVICES.len() - 1;
                self.service = self.service.saturating_add_signed(delta).min(last);
                self.method = 0;
            }
            Step::Method => {
                let last = self.provider().methods().len().saturating_sub(1);
                self.method = self.method.saturating_add_signed(delta).min(last);
            }
            Step::Endpoint | Step::Credential | Step::Login | Step::Saving | Step::Done => {}
        }
    }

    pub(crate) fn command(&mut self, command: Command, snapshot: &Snapshot) -> Reply {
        match command {
            Command::Up => self.pick(-1),
            Command::Down => self.pick(1),
            Command::Back => return self.back(),
            Command::Select => return self.advance(snapshot),
            Command::Copy => return self.copy(),
            Command::Quit
            | Command::NextView
            | Command::PrevView
            | Command::Goto(_)
            | Command::Help
            | Command::PageUp
            | Command::PageDown
            | Command::Top
            | Command::Bottom
            | Command::Left
            | Command::Right
            | Command::Add
            | Command::Filter
            | Command::RefreshUsage
            | Command::RefreshModels
            | Command::Relogin
            | Command::Rename
            | Command::Remove
            | Command::CopySettings
            | Command::FilterAccount
            | Command::FilterSession => {}
        }
        Reply::None
    }

    fn copy(&self) -> Reply {
        let text = match self.session.as_ref().and_then(|s| s.prompt.as_ref()) {
            Some(Prompt::Device { user_code, .. }) => user_code.clone(),
            Some(Prompt::PasteCode { url } | Prompt::Loopback { url, .. }) => url.clone(),
            None => return Reply::None,
        };
        Reply::Do(Effect::Copy(text))
    }

    fn advance(&mut self, snapshot: &Snapshot) -> Reply {
        match self.step {
            Step::Service => {
                self.step = if self.is_custom() {
                    Step::Endpoint
                } else {
                    Step::Method
                };
                self.focus = Focus::Url;
                Reply::None
            }
            Step::Method => self.choose_method(),
            Step::Endpoint => self.submit_endpoint(),
            Step::Credential => self.submit_credential(),
            Step::Login => self.submit_login(),
            Step::Saving => Reply::None,
            Step::Done => self.finish(snapshot),
        }
    }

    fn choose_method(&mut self) -> Reply {
        let Some(kind) = self.kind() else {
            return Reply::None;
        };
        if ops::account::needs_paste(kind) {
            self.step = Step::Credential;
            return Reply::None;
        }
        self.step = Step::Login;
        Reply::Do(Effect::StartLogin { kind })
    }

    fn submit_endpoint(&mut self) -> Reply {
        match self.focus {
            Focus::Url => {
                let text = self.url.value().trim().to_owned();
                match Url::parse(&text) {
                    Ok(url) if matches!(url.scheme(), "http" | "https") => {
                        self.url.set_value(&text);
                        self.focus = Focus::Api;
                    }
                    Ok(_) | Err(_) => self
                        .url
                        .set_error("Enter a full http:// or https:// address"),
                }
            }
            Focus::Api => {
                let text = self.api.value().trim().to_lowercase();
                if crate::forward::parse_api(&text).is_some() {
                    self.api.set_value(&text);
                    self.step = Step::Credential;
                } else {
                    self.api.set_error("Use chat, responses or messages");
                }
            }
        }
        Reply::None
    }

    fn submit_credential(&mut self) -> Reply {
        let Some(kind) = self.kind() else {
            return Reply::None;
        };
        let value = self.secret.value().trim().to_owned();
        let secret = match (&self.mode, self.is_custom()) {
            (
                Mode::Again {
                    endpoint: Some((url, api)),
                    ..
                },
                _,
            ) => Secret::Custom {
                url: url.clone(),
                api: api.clone(),
                key: (!value.is_empty()).then_some(value),
            },
            (Mode::New | Mode::Again { endpoint: None, .. }, true) => Secret::Custom {
                url: self.url.value().trim().to_owned(),
                api: self.api.value().trim().to_owned(),
                key: (!value.is_empty()).then_some(value),
            },
            (Mode::New | Mode::Again { endpoint: None, .. }, false) => {
                if value.is_empty() {
                    self.secret.set_error("Paste a value first");
                    return Reply::None;
                }
                ops::account::pasted_secret(kind, value, clock::now())
            }
        };
        self.step = Step::Saving;
        Reply::Do(self.save(kind, secret))
    }

    fn save(&self, kind: Kind, secret: Secret) -> Effect {
        match &self.mode {
            Mode::New => Effect::AddAccount { kind, secret },
            Mode::Again { name, .. } => Effect::Relogin {
                name: name.clone(),
                secret,
            },
        }
    }

    fn submit_login(&mut self) -> Reply {
        if self.login_error.is_some() {
            self.login_error = None;
            self.session = None;
            return self
                .kind()
                .map_or(Reply::None, |kind| Reply::Do(Effect::StartLogin { kind }));
        }
        if self.context() != Context::Code {
            return Reply::None;
        }
        let code = self.code.value().trim().to_owned();
        if code.is_empty() {
            let missing = match self.session.as_ref().and_then(|s| s.prompt.as_ref()) {
                Some(Prompt::Loopback { .. }) => "Paste the address first",
                Some(Prompt::PasteCode { .. } | Prompt::Device { .. }) | None => {
                    "Paste the code first"
                }
            };
            self.code.set_error(missing);
            return Reply::None;
        }
        if let Some(session) = self.session.as_mut() {
            session.handle.submit(code);
            session.sent = true;
        }
        Reply::None
    }

    fn finish(&mut self, snapshot: &Snapshot) -> Reply {
        let (Mode::New, Some(saved)) = (&self.mode, &self.saved) else {
            return Reply::Close;
        };
        let new = self.name.value().trim().to_owned();
        if &new == saved {
            return Reply::Close;
        }
        let taken: Vec<String> = snapshot
            .account_names()
            .into_iter()
            .filter(|existing| existing != saved)
            .collect();
        match name::check(&new, &taken) {
            Ok(()) => Reply::CloseDo(Effect::Rename {
                old: saved.clone(),
                new,
            }),
            Err(error) => {
                self.name.set_error(format::capitalize(&error.to_string()));
                Reply::None
            }
        }
    }

    pub(crate) fn job(&mut self, done: JobDone) -> Result<Reply, JobDone> {
        let mine = match done.work {
            Work::Login => self
                .session
                .as_ref()
                .is_some_and(|s| s.handle.id == done.id),
            Work::AddAccount | Work::Relogin => self.step == Step::Saving,
            Work::Rename
            | Work::Remove
            | Work::RefreshUsage
            | Work::RefreshModels
            | Work::CreateKey
            | Work::Revoke => false,
        };
        if !mine {
            return Err(done);
        }
        Ok(match done.result {
            Ok(Output::Prompt { prompt, opened }) => {
                let placeholder = match prompt {
                    Prompt::Loopback { .. } => "paste the address",
                    Prompt::PasteCode { .. } | Prompt::Device { .. } => "paste the code",
                };
                self.code = TextField::new().with_placeholder(placeholder);
                if let Some(session) = self.session.as_mut() {
                    session.prompt = Some(prompt);
                    session.opened = opened;
                }
                Reply::None
            }
            Ok(Output::Secret(secret)) => match self.kind() {
                Some(kind) => {
                    self.session = None;
                    self.step = Step::Saving;
                    Reply::Do(self.save(kind, secret))
                }
                None => Reply::None,
            },
            Ok(Output::Saved { name, models }) => {
                self.summary = match models {
                    Models::Count(count) => format!(
                        "Added {name}. {} available.",
                        format::noun(i64::try_from(count).unwrap_or(0), "model", "models")
                    ),
                    Models::Failed(why) => format!("Added {name}. The model list failed: {why}"),
                };
                self.name.set_value(&name);
                self.saved = Some(name);
                self.step = Step::Done;
                Reply::None
            }
            Ok(Output::Relogged(name)) => {
                self.summary = format!("{name} is signed in again.");
                self.step = Step::Done;
                Reply::None
            }
            Ok(
                Output::Renamed { .. }
                | Output::Removed(_)
                | Output::UsageRefreshed(_)
                | Output::ModelsRefreshed { .. }
                | Output::KeyCreated { .. }
                | Output::Revoked(_),
            ) => Reply::None,
            Err(error) => self.failed(done.work, &error),
        })
    }

    fn failed(&mut self, work: Work, error: &anyhow::Error) -> Reply {
        let text = format!("{error:#}");
        match work {
            Work::Login => {
                self.login_error = Some(text);
                self.session = None;
                Reply::None
            }
            Work::AddAccount | Work::Relogin => {
                self.step = Step::Credential;
                self.secret.set_error(text);
                Reply::Toast(Tone::Error, work.failure().to_owned())
            }
            Work::Rename
            | Work::Remove
            | Work::RefreshUsage
            | Work::RefreshModels
            | Work::CreateKey
            | Work::Revoke => Reply::None,
        }
    }

    fn height(&self) -> u16 {
        match self.step {
            Step::Service | Step::Credential | Step::Done => 12,
            Step::Method => 10,
            Step::Endpoint => 13,
            Step::Login => self.login_height(),
            Step::Saving => 8,
        }
    }

    fn login_height(&self) -> u16 {
        if self.login_error.is_some() {
            return 11;
        }
        match self.session.as_ref().and_then(|s| s.prompt.as_ref()) {
            None => 8,
            Some(Prompt::Device { .. }) => 13,
            Some(Prompt::Loopback { url }) => 14 + url_rows(url, 70),
            Some(Prompt::PasteCode { url }) => 12 + url_rows(url, 70),
        }
    }

    fn corner(&self) -> String {
        match (&self.mode, self.step) {
            (Mode::Again { name, .. }, _) => name.clone(),
            (Mode::New, Step::Service) => "1 / 3".to_owned(),
            (Mode::New, Step::Method | Step::Endpoint) => "2 / 3".to_owned(),
            (Mode::New, Step::Credential | Step::Login | Step::Saving) => "3 / 3".to_owned(),
            (Mode::New, Step::Done) => String::new(),
        }
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, clock: &Clock) {
        let outer = dialog::centered(area, 74, self.height());
        let title = match self.mode {
            Mode::New => "Add account",
            Mode::Again { .. } => "Sign in again",
        };
        let inner = dialog::open(frame, outer, title, &self.corner());
        let body = Rect {
            y: inner.y + 1,
            height: inner.height.saturating_sub(3),
            ..inner
        };
        match self.step {
            Step::Service => self.render_services(frame, body),
            Step::Method => self.render_methods(frame, body),
            Step::Endpoint => self.render_endpoint(frame, body, theme),
            Step::Credential => self.render_credential(frame, body, theme),
            Step::Login => self.render_login(frame, body, theme, clock),
            Step::Saving => text(frame, body, vec![Line::raw("Saving…")]),
            Step::Done => self.render_done(frame, body, theme),
        }
    }

    fn render_services(&self, frame: &mut Frame<'_>, area: Rect) {
        let rows: Vec<(String, String)> = SERVICES
            .iter()
            .map(|(name, note, _)| ((*name).to_owned(), (*note).to_owned()))
            .collect();
        choices(frame, area, "Service", &rows, self.service);
    }

    fn render_methods(&self, frame: &mut Frame<'_>, area: Rect) {
        let provider = self.provider();
        let rows: Vec<(String, String)> = provider
            .methods()
            .iter()
            .map(|method| {
                let (label, note) = method_text(provider, *method);
                (label.to_owned(), note.to_owned())
            })
            .collect();
        let question = format!("{} sign-in", provider_title(provider));
        choices(frame, area, &question, &rows, self.method);
    }

    fn render_endpoint(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let mut lines = vec![Line::raw("Endpoint"), Line::raw("")];
        lines.push(Line::styled("Address", Theme::dim()));
        draw_head(frame, area, lines);
        let url = Rect {
            y: area.y + 3,
            height: 2,
            ..area
        };
        self.url.render(frame, url, theme, self.focus == Focus::Url);
        let label = Rect {
            y: area.y + 5,
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::styled("API it speaks", Theme::dim())),
            label,
        );
        let api = Rect {
            y: area.y + 6,
            height: 2,
            ..area
        };
        self.api.render(frame, api, theme, self.focus == Focus::Api);
    }

    fn render_credential(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let (question, note) = match (self.kind(), self.is_custom()) {
            (_, true) => (
                "API key".to_owned(),
                "Leave it empty if the endpoint needs none.",
            ),
            (Some(kind), false) if kind.method() == Method::SetupToken => (
                "Paste the token from claude setup-token".to_owned(),
                "Run claude setup-token in another terminal first.",
            ),
            (Some(kind), false) => (
                format!("Paste your {} key", provider_title(kind.provider())),
                match kind.billing() {
                    Billing::Api => "Billed per use by the provider.",
                    Billing::Sub => "From your subscription plan.",
                },
            ),
            (None, false) => ("Paste your key".to_owned(), ""),
        };
        let lines = vec![
            Line::raw(question),
            Line::styled(note, Theme::dim()),
            Line::raw(""),
        ];
        draw_head(frame, area, lines);
        let field = Rect {
            y: area.y + 3,
            height: 2,
            ..area
        };
        self.secret.render(frame, field, theme, true);
        let reveal = if self.secret.is_revealed() {
            "ctrl-r hide"
        } else {
            "ctrl-r show"
        };
        let hint = Rect {
            y: area.y + 5,
            height: 1,
            ..area
        };
        frame.render_widget(Paragraph::new(Line::styled(reveal, Theme::dim())), hint);
    }

    fn render_paste(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: Theme,
        intro: Vec<Line<'static>>,
        (url, label): (&str, &'static str),
    ) {
        let sent = self.session.as_ref().is_some_and(|session| session.sent);
        let top = u16::try_from(intro.len()).unwrap_or(u16::MAX);
        let mut lines = intro;
        lines.push(Line::raw(""));
        lines.push(Line::styled(url.to_owned(), theme.accent()));
        if sent {
            lines.push(Line::raw(""));
            lines.push(Line::raw("Signing in…"));
            text(frame, area, lines);
            return;
        }
        text(frame, area, lines);
        let used = url_rows(url, area.width);
        let label_area = Rect {
            y: area.y + top + 2 + used,
            height: 1,
            ..area
        };
        frame.render_widget(
            Paragraph::new(Line::styled(label, Theme::dim())),
            label_area,
        );
        let field = Rect {
            y: area.y + top + 3 + used,
            height: 2,
            ..area
        };
        self.code.render(frame, field, theme, true);
    }

    fn render_login(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme, clock: &Clock) {
        if let Some(error) = &self.login_error {
            let lines = vec![
                Line::styled(format!("Sign-in failed: {error}"), theme.error()),
                Line::raw(""),
                Line::styled("Press enter to try again, or esc to go back.", Theme::dim()),
            ];
            text(frame, area, lines);
            return;
        }
        let Some(session) = &self.session else {
            text(frame, area, vec![Line::raw("Starting sign-in…")]);
            return;
        };
        match &session.prompt {
            None => text(frame, area, vec![Line::raw("Starting sign-in…")]),
            Some(Prompt::PasteCode { url }) => {
                let intro = vec![Line::raw(if session.opened {
                    "Sign in in the browser window that opened, then paste the code you get."
                } else {
                    "Open this link, sign in, then paste the code you get."
                })];
                self.render_paste(frame, area, theme, intro, (url, "Code"));
            }
            Some(Prompt::Loopback { url }) => {
                let intro = vec![
                    Line::raw(if session.opened {
                        "Sign in in the browser window that opened."
                    } else {
                        "Open this link and sign in."
                    }),
                    Line::raw("If the next page does not load, paste its address here."),
                    Line::raw("If it loads, OpenGateway continues by itself."),
                ];
                self.render_paste(frame, area, theme, intro, (url, "Address"));
            }
            Some(Prompt::Device {
                verification_uri,
                user_code,
                expires_at,
                ..
            }) => {
                let spaced: String = user_code
                    .chars()
                    .flat_map(|c| [c, ' '])
                    .collect::<String>()
                    .trim_end()
                    .to_owned();
                let lines = vec![
                    Line::raw(if session.opened {
                        "Enter this code in the browser window that opened."
                    } else {
                        "Open this link on any device and enter the code."
                    }),
                    Line::raw(""),
                    Line::styled(verification_uri.clone(), theme.accent()),
                    Line::raw(""),
                    Line::styled(format!("  {spaced}"), theme.accent()),
                    Line::raw(""),
                    Line::styled(
                        format!("Waiting for approval, {} left", clock.until(*expires_at)),
                        Theme::dim(),
                    ),
                ];
                text(frame, area, lines);
            }
        }
    }

    fn render_done(&self, frame: &mut Frame<'_>, area: Rect, theme: Theme) {
        let lines = vec![Line::raw(self.summary.clone()), Line::raw("")];
        if self.mode != Mode::New {
            text(frame, area, lines);
            return;
        }
        let mut lines = lines;
        lines.push(Line::styled("Name (you can change it)", Theme::dim()));
        draw_head(frame, area, lines);
        let field = Rect {
            y: area.y + 3,
            height: 2,
            ..area
        };
        self.name.render(frame, field, theme, true);
    }
}

fn url_rows(url: &str, width: u16) -> u16 {
    let rows = format::wrap(url, usize::from(width)).len();
    u16::try_from(rows).unwrap_or(1)
}

fn draw_head(frame: &mut Frame<'_>, area: Rect, lines: Vec<Line<'_>>) {
    frame.render_widget(Paragraph::new(lines), area);
}

fn text(frame: &mut Frame<'_>, area: Rect, lines: Vec<Line<'_>>) {
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn choices(
    frame: &mut Frame<'_>,
    area: Rect,
    question: &str,
    rows: &[(String, String)],
    selected: usize,
) {
    let label_cells = rows
        .iter()
        .map(|(label, _)| format::width(label))
        .max()
        .unwrap_or(0)
        + 3;
    let mut lines = vec![Line::raw(question.to_owned()), Line::raw("")];
    for (index, (label, note)) in rows.iter().enumerate() {
        let marker = " ";
        let line = Line::from(vec![
            Span::raw(format!("{marker} {}", format::fit(label, label_cells))),
            Span::styled(
                note.clone(),
                if index == selected {
                    Theme::selected()
                } else {
                    Theme::dim()
                },
            ),
        ]);
        lines.push(if index == selected {
            Theme::select(line)
        } else {
            line
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}
