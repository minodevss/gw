use super::App;
use super::jobs::{JobDone, Models, Output, Started};
use super::keymap::Context;
use super::modal::{Help, Modal};
use super::view::View;
use crate::account::Secret;
use crate::ops::snapshot::Snapshot;
use crate::provider::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Quit,
    NextView,
    PrevView,
    Goto(View),
    Help,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    Select,
    Back,
    Add,
    Filter,
    RefreshUsage,
    RefreshModels,
    Relogin,
    Rename,
    Remove,
    Copy,
    CopySettings,
    FilterAccount,
    FilterSession,
}

#[derive(Debug, Clone)]
pub(crate) enum Effect {
    StartLogin { kind: Kind },
    AddAccount { kind: Kind, secret: Secret },
    Relogin { name: String, secret: Secret },
    Rename { old: String, new: String },
    Remove(String),
    RefreshUsage(String),
    RefreshModels,
    CreateKey { name: String, base_url: String },
    Revoke(String),
    Copy(String),
}

pub(crate) enum Action {
    Command(Command),
    Data(Box<Snapshot>),
    LoadFailed(String),
    Job(Box<JobDone>),
    Paste(String),
    Reply(Reply),
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Info,
    Error,
}

pub(crate) enum Reply {
    None,
    Close,
    Push(Modal),
    Command(Command),
    Do(Effect),
    CloseDo(Effect),
    Toast(Tone, String),
}

pub(crate) fn update(app: &mut App, action: Action) {
    match action {
        Action::Tick => app.tick(),
        Action::Data(snapshot) => app.apply(*snapshot),
        Action::LoadFailed(message) => app.db_error = Some(message),
        Action::Paste(text) => app.paste(&text),
        Action::Job(done) => finish(app, *done),
        Action::Command(command) => command_of(app, command),
        Action::Reply(reply) => apply(app, reply),
    }
    app.sync_views();
}

fn command_of(app: &mut App, command: Command) {
    if app.modals.is_empty() {
        match command {
            Command::Quit => {
                app.quit = true;
                return;
            }
            Command::NextView => {
                app.view = app.view.step(1);
                return;
            }
            Command::PrevView => {
                app.view = app.view.step(-1);
                return;
            }
            Command::Goto(view) => {
                app.view = view;
                return;
            }
            Command::Help => {
                let context = Context::of(app.view);
                app.modals.push(Modal::Help(Help::new(context)));
                return;
            }
            Command::Up
            | Command::Down
            | Command::PageUp
            | Command::PageDown
            | Command::Top
            | Command::Bottom
            | Command::Left
            | Command::Right
            | Command::Select
            | Command::Back
            | Command::Add
            | Command::Filter
            | Command::RefreshUsage
            | Command::RefreshModels
            | Command::Relogin
            | Command::Rename
            | Command::Remove
            | Command::Copy
            | Command::CopySettings
            | Command::FilterAccount
            | Command::FilterSession => {}
        }
    }
    let reply = match app.modals.last_mut() {
        Some(modal) => modal.command(command, &app.snapshot),
        None => app.handle_view(command),
    };
    apply(app, reply);
}

fn apply(app: &mut App, reply: Reply) {
    match reply {
        Reply::None => {}
        Reply::Close => {
            app.modals.pop();
        }
        Reply::Push(mut modal) => {
            let start = modal.opened();
            app.modals.push(modal);
            if let Some(effect) = start {
                run(app, effect);
            }
        }
        Reply::Command(command) => {
            app.modals.pop();
            command_of(app, command);
        }
        Reply::Do(effect) => run(app, effect),
        Reply::CloseDo(effect) => {
            app.modals.pop();
            run(app, effect);
        }
        Reply::Toast(tone, text) => app.set_status(tone, text),
    }
}

fn run(app: &mut App, effect: Effect) {
    match &effect {
        Effect::Copy(text) => {
            app.copy(text);
            return;
        }
        Effect::RefreshUsage(name) => {
            app.set_status(Tone::Info, format!("Refreshing usage for {name}…"));
        }
        Effect::RefreshModels => app.set_status(Tone::Info, "Refreshing model lists…"),
        Effect::StartLogin { .. }
        | Effect::AddAccount { .. }
        | Effect::Relogin { .. }
        | Effect::Rename { .. }
        | Effect::Remove(_)
        | Effect::CreateKey { .. }
        | Effect::Revoke(_) => {}
    }
    if let Started::Login(handle) = app.jobs.start(effect)
        && let Some(Modal::AddAccount(wizard)) = app.modals.last_mut()
    {
        wizard.attach(handle);
    }
}

fn finish(app: &mut App, done: JobDone) {
    let mut done = Some(done);
    let mut reply = Reply::None;
    for modal in app.modals.iter_mut().rev() {
        if let Some(job) = done.take() {
            match modal.job(job) {
                Ok(next) => {
                    reply = next;
                    break;
                }
                Err(job) => done = Some(job),
            }
        }
    }
    if let Some(job) = done {
        reply = generic(job);
    }
    apply(app, reply);
}

fn generic(done: JobDone) -> Reply {
    let work = done.work;
    match done.result {
        Err(error) => Reply::Toast(Tone::Error, format!("{}: {error:#}", work.failure())),
        Ok(Output::Renamed { old, new }) => {
            Reply::Toast(Tone::Info, format!("Renamed {old} to {new}"))
        }
        Ok(Output::Removed(name)) => Reply::Toast(Tone::Info, format!("Removed {name}")),
        Ok(Output::UsageRefreshed(name)) => {
            Reply::Toast(Tone::Info, format!("Updated usage for {name}"))
        }
        Ok(Output::ModelsRefreshed { accounts, failed }) => match failed {
            Some(why) => Reply::Toast(Tone::Error, format!("Model list failed for {why}")),
            None => Reply::Toast(
                Tone::Info,
                format!("Updated model lists for {accounts} accounts"),
            ),
        },
        Ok(Output::Revoked(name)) => Reply::Toast(Tone::Info, format!("Revoked {name}")),
        Ok(Output::Saved { name, models }) => match models {
            Models::Count(count) => {
                Reply::Toast(Tone::Info, format!("Added {name}, {count} models"))
            }
            Models::Failed(why) => Reply::Toast(
                Tone::Error,
                format!("Added {name}, but no model list: {why}"),
            ),
        },
        Ok(Output::Relogged(name)) => {
            Reply::Toast(Tone::Info, format!("Signed in again as {name}"))
        }
        Ok(Output::KeyCreated { name, .. }) => Reply::Toast(Tone::Info, format!("Created {name}")),
        Ok(Output::Prompt { .. } | Output::Secret(_)) => Reply::None,
    }
}
