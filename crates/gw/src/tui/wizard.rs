use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::Msg;
use super::keymap::{self, Nav};
use super::widgets::{Ctx, Input, Theme, centered, countdown};
use crate::account::{Account, Secret};
use crate::clock;
use crate::ops;
use crate::provider::{Billing, Kind, LoginSession, Method, Prompt, Provider};

const API_CHOICES: &[&str] = &["messages", "responses", "chat"];
const LOOPBACK_WAIT_SECS: i64 = 600;
const WIDTH: u16 = 76;

#[derive(Debug)]
pub(crate) struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) enum Event {
    Started {
        generation: u64,
        result: Result<Box<LoginSession>, String>,
    },
    Signed {
        generation: u64,
        result: Result<Secret, String>,
    },
    Saved {
        generation: u64,
        result: Result<String, String>,
    },
}

pub(crate) enum Output {
    Nothing,
    Close,
    Copy(String),
    BeginLogin(Kind),
    Complete {
        session: Box<LoginSession>,
        pasted: Option<String>,
    },
    Save {
        kind: Kind,
        name: String,
        secret: Secret,
        relogin: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Secret,
    Url,
    Api,
    OptionalKey,
    Code,
    Name,
}

#[derive(Debug)]
enum FieldKind {
    Text(Input),
    Choice(usize),
}

#[derive(Debug)]
struct Field {
    id: FieldId,
    label: &'static str,
    kind: FieldKind,
}

#[derive(Debug)]
struct Form {
    fields: Vec<Field>,
    focus: usize,
    error: Option<String>,
}

impl Form {
    fn new(fields: Vec<Field>) -> Self {
        Self {
            fields,
            focus: 0,
            error: None,
        }
    }

    fn text(&self, id: FieldId) -> String {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| match &f.kind {
                FieldKind::Text(input) => Some(input.text().trim().to_owned()),
                FieldKind::Choice(_) => None,
            })
            .unwrap_or_default()
    }

    fn choice(&self, id: FieldId) -> &'static str {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| match &f.kind {
                FieldKind::Choice(index) => API_CHOICES.get(*index).copied(),
                FieldKind::Text(_) => None,
            })
            .unwrap_or_default()
    }

    fn move_focus(&mut self, forward: bool) {
        let count = self.fields.len().max(1);
        self.focus = if forward {
            (self.focus + 1) % count
        } else {
            (self.focus + count - 1) % count
        };
    }

    fn is_last(&self) -> bool {
        self.focus + 1 >= self.fields.len()
    }

    fn focused_is_empty_text(&self) -> bool {
        self.fields
            .get(self.focus)
            .is_some_and(|field| match &field.kind {
                FieldKind::Text(input) => input.is_empty(),
                FieldKind::Choice(_) => false,
            })
    }

    fn type_key(&mut self, key: &KeyEvent, nav: Nav) {
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match &mut field.kind {
            FieldKind::Text(input) => {
                input.handle(key);
            }
            FieldKind::Choice(index) => {
                let previous = nav == Nav::Left || key.code == KeyCode::Char('h');
                let next = nav == Nav::Right || matches!(key.code, KeyCode::Char('l' | ' '));
                if previous {
                    *index = (*index + API_CHOICES.len() - 1) % API_CHOICES.len();
                } else if next {
                    *index = (*index + 1) % API_CHOICES.len();
                }
            }
        }
    }

    fn paste(&mut self, pasted: &str) {
        if let Some(Field {
            kind: FieldKind::Text(input),
            ..
        }) = self.fields.get_mut(self.focus)
        {
            input.paste(pasted);
        }
    }
}

#[derive(Debug)]
enum SignIn {
    Form(Form),
    Starting,
    Code {
        url: String,
        form: Form,
        session: Option<Box<LoginSession>>,
    },
    Waiting {
        prompt: Prompt,
        since: i64,
    },
    Saving,
    Failed(String),
}

#[derive(Debug)]
enum Step {
    Provider { cursor: usize },
    Method { provider: Provider, cursor: usize },
    SignIn { kind: Kind, state: SignIn },
    Done(Result<String, String>),
}

#[derive(Debug)]
pub(crate) struct Wizard {
    relogin: Option<String>,
    existing: Vec<Account>,
    step: Step,
    generation: u64,
    name: Option<String>,
    task: Option<AbortOnDrop>,
}

fn pick(cursor: &mut usize, nav: Nav, len: usize) -> bool {
    match nav {
        Nav::Down => *cursor = (*cursor + 1).min(len.saturating_sub(1)),
        Nav::Up => *cursor = cursor.saturating_sub(1),
        Nav::Enter => return true,
        Nav::Esc | Nav::Tab | Nav::BackTab | Nav::Left | Nav::Right | Nav::Other => {}
    }
    false
}

fn describe(method: Method) -> &'static str {
    match method {
        Method::Oauth => "browser login, paste the code back",
        Method::SetupToken => "token from `claude setup-token`",
        Method::Siwc => "browser login through an ssh tunnel",
        Method::Codex => "device code",
        Method::Plan => "subscription plan key",
        Method::Key => "API key",
        Method::Custom => "your own endpoint",
    }
}

fn billing_text(kind: Kind) -> &'static str {
    match kind.billing() {
        Billing::Sub => "sub",
        Billing::Api => "api",
    }
}

fn secret_field(kind: Kind) -> Field {
    Field {
        id: FieldId::Secret,
        label: if kind.method() == Method::SetupToken {
            "token"
        } else {
            "key"
        },
        kind: FieldKind::Text(Input::masked()),
    }
}

fn name_field(default: &str) -> Field {
    Field {
        id: FieldId::Name,
        label: "name",
        kind: FieldKind::Text(Input::plain(default)),
    }
}

impl Wizard {
    pub(crate) fn add(existing: Vec<Account>) -> Self {
        Self {
            relogin: None,
            existing,
            step: Step::Provider { cursor: 0 },
            generation: 0,
            name: None,
            task: None,
        }
    }

    pub(crate) fn relogin(account: &Account, existing: Vec<Account>) -> (Self, Output) {
        let mut wizard = Self {
            relogin: Some(account.name.clone()),
            existing,
            step: Step::Done(Err(String::new())),
            generation: 0,
            name: Some(account.name.clone()),
            task: None,
        };
        let output = wizard.start_sign_in(account.kind, Some(&account.secret));
        (wizard, output)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn set_task(&mut self, task: Option<AbortOnDrop>) {
        self.task = task;
    }

    fn default_name(&self, kind: Kind) -> String {
        ops::account::default_name(&self.existing, kind)
    }

    fn start_sign_in(&mut self, kind: Kind, previous: Option<&Secret>) -> Output {
        let adding = self.relogin.is_none();
        let state = if ops::account::needs_paste(kind) {
            let mut fields = vec![secret_field(kind)];
            if adding {
                fields.push(name_field(&self.default_name(kind)));
            }
            SignIn::Form(Form::new(fields))
        } else if kind.method() == Method::Custom {
            let (url, api) = match previous {
                Some(Secret::Custom { url, api, .. }) => (url.clone(), api.clone()),
                Some(Secret::Oauth { .. } | Secret::Token { .. } | Secret::Key { .. }) | None => {
                    (String::new(), String::new())
                }
            };
            let mut fields = vec![
                Field {
                    id: FieldId::Url,
                    label: "url",
                    kind: FieldKind::Text(Input::plain(&url)),
                },
                Field {
                    id: FieldId::Api,
                    label: "api",
                    kind: FieldKind::Choice(
                        API_CHOICES
                            .iter()
                            .position(|c| *c == api)
                            .unwrap_or_default(),
                    ),
                },
                Field {
                    id: FieldId::OptionalKey,
                    label: "key",
                    kind: FieldKind::Text(Input::masked()),
                },
            ];
            if adding {
                fields.push(name_field(&self.default_name(kind)));
            }
            SignIn::Form(Form::new(fields))
        } else {
            self.generation += 1;
            self.step = Step::SignIn {
                kind,
                state: SignIn::Starting,
            };
            return Output::BeginLogin(kind);
        };
        self.step = Step::SignIn { kind, state };
        Output::Nothing
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        if let Step::SignIn { state, .. } = &mut self.step {
            match state {
                SignIn::Form(form) | SignIn::Code { form, .. } => form.paste(pasted),
                SignIn::Starting | SignIn::Waiting { .. } | SignIn::Saving | SignIn::Failed(_) => {}
            }
        }
    }

    pub(crate) fn key(&mut self, key: &KeyEvent) -> Output {
        let nav = keymap::list_nav(key);
        match &mut self.step {
            Step::Provider { cursor } => {
                if nav == Nav::Esc {
                    return Output::Close;
                }
                if !pick(cursor, nav, Provider::ALL.len()) {
                    return Output::Nothing;
                }
                let Some(provider) = Provider::ALL.get(*cursor).copied() else {
                    return Output::Nothing;
                };
                if let [only] = provider.methods() {
                    return match Kind::new(provider, *only) {
                        Ok(kind) => self.start_sign_in(kind, None),
                        Err(_) => Output::Nothing,
                    };
                }
                self.step = Step::Method {
                    provider,
                    cursor: 0,
                };
                Output::Nothing
            }
            Step::Method { provider, cursor } => {
                let provider = *provider;
                let methods = provider.methods();
                if nav == Nav::Esc {
                    let index = Provider::ALL
                        .iter()
                        .position(|p| *p == provider)
                        .unwrap_or_default();
                    self.step = Step::Provider { cursor: index };
                    return Output::Nothing;
                }
                if !pick(cursor, nav, methods.len()) {
                    return Output::Nothing;
                }
                let Some(method) = methods.get(*cursor).copied() else {
                    return Output::Nothing;
                };
                match Kind::new(provider, method) {
                    Ok(kind) => self.start_sign_in(kind, None),
                    Err(_) => Output::Nothing,
                }
            }
            Step::SignIn { .. } => self.sign_in_key(key),
            Step::Done(_) => {
                if matches!(nav, Nav::Esc | Nav::Enter) {
                    Output::Close
                } else {
                    Output::Nothing
                }
            }
        }
    }

    fn sign_in_key(&mut self, key: &KeyEvent) -> Output {
        let Step::SignIn { kind, state } = &mut self.step else {
            return Output::Nothing;
        };
        let kind = *kind;
        let nav = keymap::nav(key);
        match state {
            SignIn::Saving => Output::Nothing,
            SignIn::Starting => {
                if nav == Nav::Esc {
                    Output::Close
                } else {
                    Output::Nothing
                }
            }
            SignIn::Failed(_) => match nav {
                Nav::Esc => Output::Close,
                Nav::Enter => self.start_sign_in(kind, None),
                Nav::Tab
                | Nav::BackTab
                | Nav::Up
                | Nav::Down
                | Nav::Left
                | Nav::Right
                | Nav::Other => Output::Nothing,
            },
            SignIn::Waiting { prompt, .. } => {
                if nav == Nav::Esc {
                    Output::Close
                } else if key.code == KeyCode::Char('y') {
                    match prompt {
                        Prompt::Device {
                            verification_uri, ..
                        } => Output::Copy(verification_uri.clone()),
                        Prompt::Loopback { url, .. } | Prompt::PasteCode { url } => {
                            Output::Copy(url.clone())
                        }
                    }
                } else if key.code == KeyCode::Char('s') {
                    match prompt {
                        Prompt::Loopback { ssh_hint, .. } => Output::Copy(ssh_hint.clone()),
                        Prompt::Device { .. } | Prompt::PasteCode { .. } => Output::Nothing,
                    }
                } else {
                    Output::Nothing
                }
            }
            SignIn::Code { url, form, session } => {
                let ctrl_y =
                    key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL);
                let bare_y = key.code == KeyCode::Char('y')
                    && form.focus == 0
                    && form.focused_is_empty_text();
                if ctrl_y || bare_y {
                    return Output::Copy(url.clone());
                }
                match nav {
                    Nav::Esc => Output::Close,
                    Nav::Tab | Nav::Down => {
                        form.move_focus(true);
                        Output::Nothing
                    }
                    Nav::BackTab | Nav::Up => {
                        form.move_focus(false);
                        Output::Nothing
                    }
                    Nav::Enter if !form.is_last() => {
                        form.move_focus(true);
                        Output::Nothing
                    }
                    Nav::Enter => {
                        let code = form.text(FieldId::Code);
                        if code.is_empty() {
                            form.error = Some("paste the code from the browser".to_owned());
                            return Output::Nothing;
                        }
                        let Some(session) = session.take() else {
                            return Output::Nothing;
                        };
                        let typed = form.text(FieldId::Name);
                        let url = url.clone();
                        self.name = Some(if typed.is_empty() {
                            ops::account::default_name(&self.existing, kind)
                        } else {
                            typed
                        });
                        self.generation += 1;
                        self.step = Step::SignIn {
                            kind,
                            state: SignIn::Waiting {
                                prompt: Prompt::PasteCode { url },
                                since: clock::now(),
                            },
                        };
                        Output::Complete {
                            session,
                            pasted: Some(code),
                        }
                    }
                    Nav::Left | Nav::Right | Nav::Other => {
                        form.type_key(key, nav);
                        Output::Nothing
                    }
                }
            }
            SignIn::Form(form) => match nav {
                Nav::Esc => Output::Close,
                Nav::Tab | Nav::Down => {
                    form.move_focus(true);
                    Output::Nothing
                }
                Nav::BackTab | Nav::Up => {
                    form.move_focus(false);
                    Output::Nothing
                }
                Nav::Enter if !form.is_last() => {
                    form.move_focus(true);
                    Output::Nothing
                }
                Nav::Enter => self.submit_form(kind),
                Nav::Left | Nav::Right | Nav::Other => {
                    form.type_key(key, nav);
                    Output::Nothing
                }
            },
        }
    }

    fn submit_form(&mut self, kind: Kind) -> Output {
        let Step::SignIn {
            state: SignIn::Form(form),
            ..
        } = &mut self.step
        else {
            return Output::Nothing;
        };
        let secret = if kind.method() == Method::Custom {
            let url = form.text(FieldId::Url);
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                form.error = Some("url must start with http:// or https://".to_owned());
                return Output::Nothing;
            }
            let key = form.text(FieldId::OptionalKey);
            Secret::Custom {
                url,
                api: form.choice(FieldId::Api).to_owned(),
                key: (!key.is_empty()).then_some(key),
            }
        } else {
            let value = form.text(FieldId::Secret);
            if value.is_empty() {
                form.error = Some("paste the value first".to_owned());
                return Output::Nothing;
            }
            ops::account::pasted_secret(kind, value, clock::now())
        };
        let typed = form.text(FieldId::Name);
        let name = match (&self.relogin, typed.is_empty()) {
            (Some(existing), _) => existing.clone(),
            (None, true) => ops::account::default_name(&self.existing, kind),
            (None, false) => typed,
        };
        self.save(kind, name, secret)
    }

    fn save(&mut self, kind: Kind, name: String, secret: Secret) -> Output {
        self.generation += 1;
        self.step = Step::SignIn {
            kind,
            state: SignIn::Saving,
        };
        Output::Save {
            kind,
            relogin: self.relogin.is_some(),
            name,
            secret,
        }
    }

    pub(crate) fn event(&mut self, event: Event) -> Output {
        match event {
            Event::Started { generation, result } if generation == self.generation => {
                let Step::SignIn {
                    kind,
                    state: state @ SignIn::Starting,
                } = &mut self.step
                else {
                    return Output::Nothing;
                };
                let kind = *kind;
                match result {
                    Err(error) => {
                        *state = SignIn::Failed(error);
                        Output::Nothing
                    }
                    Ok(session) => {
                        let prompt = session.prompt().clone();
                        match prompt {
                            Prompt::PasteCode { url } => {
                                let mut fields = vec![Field {
                                    id: FieldId::Code,
                                    label: "code",
                                    kind: FieldKind::Text(Input::masked()),
                                }];
                                if self.relogin.is_none() {
                                    fields.push(name_field(&ops::account::default_name(
                                        &self.existing,
                                        kind,
                                    )));
                                }
                                *state = SignIn::Code {
                                    url,
                                    form: Form::new(fields),
                                    session: Some(session),
                                };
                                Output::Nothing
                            }
                            Prompt::Loopback { .. } | Prompt::Device { .. } => {
                                *state = SignIn::Waiting {
                                    prompt,
                                    since: clock::now(),
                                };
                                self.generation += 1;
                                Output::Complete {
                                    session,
                                    pasted: None,
                                }
                            }
                        }
                    }
                }
            }
            Event::Signed { generation, result } if generation == self.generation => {
                let Step::SignIn {
                    kind,
                    state: state @ SignIn::Waiting { .. },
                } = &mut self.step
                else {
                    return Output::Nothing;
                };
                let kind = *kind;
                match result {
                    Err(error) => {
                        *state = SignIn::Failed(error);
                        Output::Nothing
                    }
                    Ok(secret) => {
                        let name = self
                            .name
                            .clone()
                            .unwrap_or_else(|| ops::account::default_name(&self.existing, kind));
                        self.save(kind, name, secret)
                    }
                }
            }
            Event::Saved { generation, result } if generation == self.generation => {
                self.task = None;
                self.step = Step::Done(result);
                Output::Nothing
            }
            Event::Started { .. } | Event::Signed { .. } | Event::Saved { .. } => Output::Nothing,
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.step {
            Step::Provider { .. } => vec![("j/k", "move"), ("enter", "select"), ("esc", "cancel")],
            Step::Method { .. } => vec![("j/k", "move"), ("enter", "select"), ("esc", "back")],
            Step::Done(_) => vec![("enter", "close")],
            Step::SignIn { state, .. } => match state {
                SignIn::Form(_) => vec![
                    ("tab", "next field"),
                    ("enter", "next/save"),
                    ("esc", "cancel"),
                ],
                SignIn::Code { .. } => {
                    vec![("y", "copy url"), ("enter", "submit"), ("esc", "cancel")]
                }
                SignIn::Waiting {
                    prompt: Prompt::Loopback { .. },
                    ..
                } => vec![("y", "copy url"), ("s", "copy ssh"), ("esc", "cancel")],
                SignIn::Waiting { .. } => vec![("y", "copy url"), ("esc", "cancel")],
                SignIn::Failed(_) => vec![("enter", "try again"), ("esc", "cancel")],
                SignIn::Starting => vec![("esc", "cancel")],
                SignIn::Saving => vec![("", "saving…")],
            },
        }
    }

    fn step_number(&self) -> usize {
        match &self.step {
            Step::Provider { .. } => 1,
            Step::Method { .. } => 2,
            Step::SignIn { .. } => 3,
            Step::Done(_) => 4,
        }
    }

    fn title(&self, theme: Theme) -> Line<'static> {
        let current = self.step_number();
        let mut spans = vec![Span::raw(match &self.relogin {
            Some(name) => format!(" Sign in again · {name} "),
            None => " Add account ".to_owned(),
        })];
        for (number, label) in [
            (1, "① provider"),
            (2, "② method"),
            (3, "③ sign in"),
            (4, "④ done"),
        ] {
            if self.relogin.is_some() && number < 3 {
                continue;
            }
            let style = if number == current {
                theme.selected()
            } else {
                theme.dim()
            };
            spans.push(Span::raw(" "));
            spans.push(Span::styled(label, style));
        }
        spans.push(Span::raw(" "));
        Line::from(spans)
    }

    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let theme = ctx.theme;
        let inner_width = usize::from(WIDTH.min(area.width)).saturating_sub(4).max(1);
        let lines = self.body(ctx, inner_width);
        let wrapped: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(inner_width).max(1))
            .sum();
        let height = u16::try_from(wrapped + 2).unwrap_or(u16::MAX);
        let rect = centered(area, WIDTH, height);
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(self.title(theme))),
            rect,
        );
    }

    fn body(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let theme = ctx.theme;
        match &self.step {
            Step::Provider { cursor } => Provider::ALL
                .iter()
                .enumerate()
                .map(|(index, provider)| {
                    let methods: Vec<&str> =
                        provider.methods().iter().map(|m| m.as_str()).collect();
                    list_line(
                        index == *cursor,
                        provider.as_str(),
                        &methods.join(" · "),
                        theme,
                    )
                })
                .collect(),
            Step::Method { provider, cursor } => provider
                .methods()
                .iter()
                .enumerate()
                .map(|(index, method)| {
                    let billing = Kind::new(*provider, *method).map_or("", billing_text);
                    list_line(
                        index == *cursor,
                        method.as_str(),
                        &format!("{billing:<4}{}", describe(*method)),
                        theme,
                    )
                })
                .collect(),
            Step::Done(Ok(message)) => vec![Line::styled(format!("✓ {message}"), theme.ok())],
            Step::Done(Err(message)) => vec![Line::styled(format!("✗ {message}"), theme.bad())],
            Step::SignIn { kind, state } => {
                let mut lines = vec![
                    Line::from(format!(
                        "{} · {} ({})",
                        kind.provider(),
                        kind.method(),
                        describe(kind.method())
                    )),
                    Line::default(),
                ];
                lines.extend(sign_in_lines(*kind, state, ctx, width));
                lines
            }
        }
    }
}

fn list_line(selected: bool, name: &str, note: &str, theme: Theme) -> Line<'static> {
    let marker = if selected { "▶" } else { " " };
    let style = if selected {
        ratatui::style::Style::new().add_modifier(Modifier::BOLD)
    } else {
        ratatui::style::Style::new()
    };
    Line::from(vec![
        Span::styled(format!("{marker} {name:<12}"), style),
        Span::styled(note.to_owned(), theme.dim()),
    ])
}

fn form_lines(form: &Form, width: usize, theme: Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, field) in form.fields.iter().enumerate() {
        let focused = index == form.focus;
        let marker = if focused { "▶" } else { " " };
        let mut spans = vec![Span::raw(format!("{marker} {:<7}", field.label))];
        match &field.kind {
            FieldKind::Text(input) => {
                spans.extend(input.line(width.saturating_sub(10), focused, theme).spans);
            }
            FieldKind::Choice(selected) => {
                for (position, option) in API_CHOICES.iter().enumerate() {
                    let style = if position == *selected {
                        theme.selected()
                    } else {
                        theme.dim()
                    };
                    spans.push(Span::styled(format!(" {option} "), style));
                    spans.push(Span::raw(" "));
                }
            }
        }
        lines.push(Line::from(spans));
    }
    if let Some(error) = &form.error {
        lines.push(Line::default());
        lines.push(Line::styled(error.clone(), theme.bad()));
    }
    lines
}

fn sign_in_lines(kind: Kind, state: &SignIn, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
    let theme = ctx.theme;
    match state {
        SignIn::Form(form) => {
            let mut lines = Vec::new();
            if kind.method() == Method::SetupToken {
                lines.push(Line::from(
                    "run `claude setup-token` on any machine, then paste the token",
                ));
                lines.push(Line::default());
            }
            lines.extend(form_lines(form, width, theme));
            lines
        }
        SignIn::Starting => vec![Line::styled("contacting the provider…", theme.dim())],
        SignIn::Code { url, form, .. } => {
            let mut lines = vec![
                Line::from(format!("1. open   {url}")),
                Line::styled(
                    "          y copies the url while the code is empty",
                    theme.dim(),
                ),
                Line::from("2. sign in, then paste the code the browser shows"),
                Line::default(),
            ];
            lines.extend(form_lines(form, width, theme));
            lines
        }
        SignIn::Waiting { prompt, since } => match prompt {
            Prompt::PasteCode { .. } => vec![Line::styled("signing in…", theme.dim())],
            Prompt::Device {
                verification_uri,
                user_code,
                expires_at,
                ..
            } => vec![
                Line::from(format!("1. open   {verification_uri}    y copy")),
                Line::from(format!("2. enter  {user_code}")),
                Line::default(),
                Line::styled(
                    format!(
                        "waiting for approval…  expires in {}",
                        countdown(expires_at - ctx.now)
                    ),
                    theme.dim(),
                ),
            ],
            Prompt::Loopback { url, ssh_hint, .. } => vec![
                Line::from("1. on your machine run  (s copies)"),
                Line::from(format!("   {ssh_hint}")),
                Line::from(format!("2. open   {url}    y copy")),
                Line::default(),
                Line::styled(
                    format!(
                        "waiting for the browser callback…  {} left",
                        countdown(since + LOOPBACK_WAIT_SECS - ctx.now)
                    ),
                    theme.dim(),
                ),
            ],
        },
        SignIn::Saving => vec![Line::styled(
            "saving and fetching the model list…",
            theme.dim(),
        )],
        SignIn::Failed(message) => vec![
            Line::styled(format!("✗ {message}"), theme.bad()),
            Line::default(),
            Line::from("enter starts sign-in again · esc cancels"),
        ],
    }
}

async fn send(tx: &mpsc::Sender<Msg>, event: Event) {
    if tx.send(Msg::Wizard(Box::new(event))).await.is_err() {
        tracing::debug!("event loop closed before a wizard task finished");
    }
}

pub(crate) fn launch(
    output: Output,
    generation: u64,
    home: &Path,
    http: &reqwest::Client,
    tx: &mpsc::Sender<Msg>,
) -> Option<AbortOnDrop> {
    let tx = tx.clone();
    let home = home.to_path_buf();
    let http = http.clone();
    let handle = match output {
        Output::Nothing | Output::Close | Output::Copy(_) => return None,
        Output::BeginLogin(kind) => tokio::spawn(async move {
            let result = ops::account::begin_login(&home, &http, kind)
                .await
                .map(Box::new)
                .map_err(|error| format!("{error:#}"));
            send(&tx, Event::Started { generation, result }).await;
        }),
        Output::Complete { session, pasted } => tokio::spawn(async move {
            let result = (*session)
                .complete(pasted)
                .await
                .map_err(|error| error.to_string());
            send(&tx, Event::Signed { generation, result }).await;
        }),
        Output::Save {
            kind,
            name,
            secret,
            relogin,
        } => tokio::spawn(async move {
            let result = save(&home, &http, kind, name, secret, relogin).await;
            send(&tx, Event::Saved { generation, result }).await;
        }),
    };
    Some(AbortOnDrop(handle))
}

async fn save(
    home: &Path,
    http: &reqwest::Client,
    kind: Kind,
    name: String,
    secret: Secret,
    relogin: bool,
) -> Result<String, String> {
    if relogin {
        return ops::account::relogin(home, &name, secret)
            .await
            .map(|()| format!("{name}: signed in again"))
            .map_err(|error| format!("{error:#}"));
    }
    let account = ops::account::add(home, name, kind, secret)
        .await
        .map_err(|error| format!("{error:#}"))?;
    let models = match ops::model::refresh(home, http, Some(&account.name)).await {
        Ok(report) => report
            .into_iter()
            .next()
            .map_or(Err("no model list returned".to_owned()), |r| r.result),
        Err(error) => Err(format!("{error:#}")),
    };
    Ok(match models {
        Ok(count) => format!("added {} · {count} models", account.name),
        Err(reason) => format!("added {} · model list failed: {reason}", account.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::widgets::testing;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(wizard: &mut Wizard, text: &str) {
        for c in text.chars() {
            wizard.key(&press(KeyCode::Char(c)));
        }
    }

    fn go_to(wizard: &mut Wizard, provider: Provider, method: usize) -> Output {
        let index = Provider::ALL.iter().position(|p| *p == provider).unwrap();
        for _ in 0..index {
            wizard.key(&press(KeyCode::Down));
        }
        let output = wizard.key(&press(KeyCode::Enter));
        if matches!(wizard.step, Step::Method { .. }) {
            for _ in 0..method {
                wizard.key(&press(KeyCode::Down));
            }
            return wizard.key(&press(KeyCode::Enter));
        }
        output
    }

    fn ctx() -> Ctx {
        Ctx {
            now: 1_800_000_000,
            theme: Theme::new(false),
        }
    }

    #[test]
    fn provider_then_method_then_sign_in_form() {
        let mut wizard = Wizard::add(Vec::new());
        assert!(matches!(wizard.step, Step::Provider { .. }));
        let output = go_to(&mut wizard, Provider::OpenAi, 2);
        assert!(matches!(output, Output::Nothing));
        assert!(matches!(
            wizard.step,
            Step::SignIn {
                state: SignIn::Form(_),
                ..
            }
        ));
    }

    #[test]
    fn escape_steps_back_then_closes() {
        let mut wizard = Wizard::add(Vec::new());
        wizard.key(&press(KeyCode::Enter));
        assert!(matches!(wizard.step, Step::Method { .. }));
        assert!(matches!(wizard.key(&press(KeyCode::Esc)), Output::Nothing));
        assert!(matches!(wizard.step, Step::Provider { .. }));
        assert!(matches!(wizard.key(&press(KeyCode::Esc)), Output::Close));
    }

    #[test]
    fn key_form_saves_a_pasted_key_with_the_default_name() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::OpenAi, 2);
        wizard.paste("sk-test");
        wizard.key(&press(KeyCode::Enter));
        let Output::Save {
            name,
            secret,
            relogin,
            ..
        } = wizard.key(&press(KeyCode::Enter))
        else {
            panic!("expected a save");
        };
        assert_eq!(name, "openai-key-1");
        assert_eq!(
            secret,
            Secret::Key {
                key: "sk-test".to_owned()
            }
        );
        assert!(!relogin);
        assert!(matches!(
            wizard.step,
            Step::SignIn {
                state: SignIn::Saving,
                ..
            }
        ));
    }

    #[test]
    fn empty_secret_is_rejected_inline() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::OpenAi, 2);
        wizard.key(&press(KeyCode::Tab));
        wizard.key(&press(KeyCode::Enter));
        assert!(matches!(
            &wizard.step,
            Step::SignIn { state: SignIn::Form(form), .. } if form.error.is_some()
        ));
    }

    #[test]
    fn setup_token_gets_an_expiry() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::Anthropic, 1);
        wizard.paste("tok");
        wizard.key(&press(KeyCode::Enter));
        let Output::Save { secret, .. } = wizard.key(&press(KeyCode::Enter)) else {
            panic!("expected a save");
        };
        assert!(matches!(
            secret,
            Secret::Token {
                expires_at: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn custom_form_collects_url_api_and_optional_key() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::Custom, 0);
        type_text(&mut wizard, "http://localhost:11434");
        wizard.key(&press(KeyCode::Tab));
        wizard.key(&press(KeyCode::Right));
        wizard.key(&press(KeyCode::Tab));
        wizard.key(&press(KeyCode::Tab));
        let Output::Save { secret, name, .. } = wizard.key(&press(KeyCode::Enter)) else {
            panic!("expected a save");
        };
        assert_eq!(name, "custom-1");
        assert_eq!(
            secret,
            Secret::Custom {
                url: "http://localhost:11434".to_owned(),
                api: "responses".to_owned(),
                key: None,
            }
        );
    }

    #[test]
    fn custom_url_must_be_http() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::Custom, 0);
        type_text(&mut wizard, "localhost");
        for _ in 0..3 {
            wizard.key(&press(KeyCode::Tab));
        }
        assert!(matches!(
            wizard.key(&press(KeyCode::Enter)),
            Output::Nothing
        ));
        assert!(matches!(
            &wizard.step,
            Step::SignIn { state: SignIn::Form(form), .. } if form.error.is_some()
        ));
    }

    #[test]
    fn browser_methods_begin_a_login_and_ignore_stale_results() {
        let mut wizard = Wizard::add(Vec::new());
        let output = go_to(&mut wizard, Provider::Anthropic, 0);
        assert!(matches!(output, Output::BeginLogin(_)));
        let stale = wizard.generation() - 1;
        wizard.event(Event::Started {
            generation: stale,
            result: Err("late".to_owned()),
        });
        assert!(matches!(
            wizard.step,
            Step::SignIn {
                state: SignIn::Starting,
                ..
            }
        ));
        wizard.event(Event::Started {
            generation: wizard.generation(),
            result: Err("network down".to_owned()),
        });
        assert!(matches!(
            &wizard.step,
            Step::SignIn { state: SignIn::Failed(message), .. } if message == "network down"
        ));
        let retry = wizard.key(&press(KeyCode::Enter));
        assert!(matches!(retry, Output::BeginLogin(_)));
    }

    #[test]
    fn device_login_result_is_saved_then_done() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::OpenAi, 1);
        wizard.step = Step::SignIn {
            kind: Kind::new(Provider::OpenAi, Method::Codex).unwrap(),
            state: SignIn::Waiting {
                prompt: Prompt::Device {
                    verification_uri: "https://auth.openai.com/codex/device".to_owned(),
                    user_code: "ABCD-EFGH".to_owned(),
                    expires_at: 0,
                    interval: 5,
                },
                since: 0,
            },
        };
        let secret = Secret::Key {
            key: "k".to_owned(),
        };
        let output = wizard.event(Event::Signed {
            generation: wizard.generation(),
            result: Ok(secret),
        });
        let Output::Save { name, .. } = output else {
            panic!("expected a save");
        };
        assert_eq!(name, "openai-codex-1");
        wizard.event(Event::Saved {
            generation: wizard.generation(),
            result: Ok("added openai-codex-1 · 3 models".to_owned()),
        });
        assert!(matches!(wizard.step, Step::Done(Ok(_))));
        assert!(matches!(wizard.key(&press(KeyCode::Enter)), Output::Close));
    }

    #[test]
    fn copy_keys_return_url_and_ssh_hint() {
        let mut wizard = Wizard::add(Vec::new());
        wizard.step = Step::SignIn {
            kind: Kind::new(Provider::OpenAi, Method::Siwc).unwrap(),
            state: SignIn::Waiting {
                prompt: Prompt::Loopback {
                    url: "https://login".to_owned(),
                    port: 1455,
                    ssh_hint: "ssh -L 1455:127.0.0.1:1455 host".to_owned(),
                },
                since: 0,
            },
        };
        assert!(
            matches!(wizard.key(&press(KeyCode::Char('y'))), Output::Copy(url) if url == "https://login")
        );
        assert!(
            matches!(wizard.key(&press(KeyCode::Char('s'))), Output::Copy(h) if h.starts_with("ssh -L"))
        );
        assert!(matches!(wizard.key(&press(KeyCode::Esc)), Output::Close));
    }

    #[test]
    fn relogin_keeps_the_account_name() {
        let account = Account {
            name: "openai-key-7".to_owned(),
            kind: Kind::new(Provider::OpenAi, Method::Key).unwrap(),
            secret: Secret::Key {
                key: "old".to_owned(),
            },
            state: crate::account::State::Active,
            usage: None,
            usage_at: None,
            created_at: 0,
        };
        let (mut wizard, output) = Wizard::relogin(&account, vec![account.clone()]);
        assert!(matches!(output, Output::Nothing));
        wizard.paste("new");
        let Output::Save {
            name,
            relogin,
            secret,
            ..
        } = wizard.key(&press(KeyCode::Enter))
        else {
            panic!("expected a save");
        };
        assert_eq!(name, "openai-key-7");
        assert!(relogin);
        assert_eq!(
            secret,
            Secret::Key {
                key: "new".to_owned()
            }
        );
    }

    #[test]
    fn renders_provider_step() {
        let wizard = Wizard::add(Vec::new());
        let text = testing::render(90, 16, |frame| wizard.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("wizard_provider", text);
    }

    #[test]
    fn renders_masked_key_form() {
        let mut wizard = Wizard::add(Vec::new());
        go_to(&mut wizard, Provider::OpenAi, 2);
        wizard.paste("sk-very-secret");
        let text = testing::render(90, 16, |frame| wizard.render(frame, frame.area(), &ctx()));
        assert!(!text.contains("sk-very-secret"));
        insta::assert_snapshot!("wizard_key_form", text);
    }

    #[test]
    fn renders_device_code_waiting_screen() {
        let mut wizard = Wizard::add(Vec::new());
        wizard.step = Step::SignIn {
            kind: Kind::new(Provider::OpenAi, Method::Codex).unwrap(),
            state: SignIn::Waiting {
                prompt: Prompt::Device {
                    verification_uri: "https://auth.openai.com/codex/device".to_owned(),
                    user_code: "ABCD-EFGH".to_owned(),
                    expires_at: 1_800_000_872,
                    interval: 5,
                },
                since: 1_800_000_000,
            },
        };
        let text = testing::render(90, 16, |frame| wizard.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("wizard_device", text);
    }
}
