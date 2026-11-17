use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::{AbortHandle, JoinError, JoinSet};

use super::action::{Action, Effect};
use super::data;
use crate::account::Secret;
use crate::clock;
use crate::ops::{self, key::Issued};
use crate::provider::{Kind, Prompt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct JobId(u64);

impl JobId {
    #[cfg(test)]
    pub(crate) fn for_test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Work {
    Login,
    AddAccount,
    Relogin,
    Rename,
    Remove,
    RefreshUsage,
    RefreshModels,
    CreateKey,
    Revoke,
}

impl Work {
    pub(crate) fn failure(self) -> &'static str {
        match self {
            Self::Login => "Sign-in failed",
            Self::AddAccount => "Couldn't add the account",
            Self::Relogin => "Couldn't save the new login",
            Self::Rename => "Couldn't rename",
            Self::Remove => "Couldn't remove",
            Self::RefreshUsage => "Couldn't refresh usage",
            Self::RefreshModels => "Couldn't refresh the model lists",
            Self::CreateKey => "Couldn't create the key",
            Self::Revoke => "Couldn't revoke the key",
        }
    }
}

#[derive(Debug)]
pub(crate) enum Models {
    Count(usize),
    Failed(String),
}

pub(crate) enum Output {
    Prompt {
        prompt: Prompt,
        opened: bool,
    },
    Secret(Secret),
    Saved {
        name: String,
        models: Models,
    },
    Relogged(String),
    Renamed {
        old: String,
        new: String,
    },
    Removed(String),
    UsageRefreshed(String),
    ModelsRefreshed {
        accounts: usize,
        failed: Option<String>,
    },
    KeyCreated {
        name: String,
        issued: Issued,
    },
    Revoked(String),
}

pub(crate) struct JobDone {
    pub(crate) id: JobId,
    pub(crate) work: Work,
    pub(crate) result: anyhow::Result<Output>,
}

pub(crate) struct LoginHandle {
    pub(crate) id: JobId,
    code: Option<oneshot::Sender<String>>,
    abort: AbortHandle,
}

impl LoginHandle {
    pub(crate) fn submit(&mut self, code: String) {
        if let Some(sender) = self.code.take()
            && sender.send(code).is_err()
        {
            tracing::debug!("login task ended before the code arrived");
        }
    }
}

#[cfg(test)]
impl LoginHandle {
    pub(crate) fn for_test(id: JobId) -> (Self, oneshot::Receiver<String>) {
        let (sender, receiver) = oneshot::channel();
        let abort = tokio::spawn(std::future::pending::<()>()).abort_handle();
        let handle = Self {
            id,
            code: Some(sender),
            abort,
        };
        (handle, receiver)
    }
}

impl Drop for LoginHandle {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub(crate) enum Started {
    Login(LoginHandle),
    Plain,
}

pub(crate) struct Jobs {
    set: JoinSet<()>,
    tx: mpsc::Sender<Action>,
    home: PathBuf,
    http: reqwest::Client,
    reload: Arc<Notify>,
    next: u64,
}

impl Jobs {
    pub(crate) fn new(home: PathBuf, tx: mpsc::Sender<Action>) -> Self {
        Self {
            set: JoinSet::new(),
            tx,
            home,
            http: reqwest::Client::new(),
            reload: Arc::new(Notify::new()),
            next: 0,
        }
    }

    pub(crate) fn start_loader(&mut self) {
        data::spawn_loader(
            &mut self.set,
            self.home.clone(),
            self.tx.clone(),
            Arc::clone(&self.reload),
        );
    }

    pub(crate) async fn join_next(&mut self) -> Option<Result<(), JoinError>> {
        self.set.join_next().await
    }

    pub(crate) fn shutdown(&mut self) {
        self.set.abort_all();
    }

    fn next_id(&mut self) -> JobId {
        self.next += 1;
        JobId(self.next)
    }

    fn spawn(
        &mut self,
        work: Work,
        task: impl Future<Output = anyhow::Result<Output>> + Send + 'static,
    ) -> (JobId, AbortHandle) {
        let id = self.next_id();
        (id, self.spawn_as(id, work, task))
    }

    fn spawn_as(
        &mut self,
        id: JobId,
        work: Work,
        task: impl Future<Output = anyhow::Result<Output>> + Send + 'static,
    ) -> AbortHandle {
        let tx = self.tx.clone();
        let reload = Arc::clone(&self.reload);
        self.set.spawn(async move {
            let result = task.await;
            let done = Action::Job(Box::new(JobDone { id, work, result }));
            if tx.send(done).await.is_err() {
                tracing::debug!("event loop closed before a job finished");
            }
            reload.notify_one();
        })
    }

    pub(crate) fn start(&mut self, effect: Effect) -> Started {
        let home = self.home.clone();
        let http = self.http.clone();
        match effect {
            Effect::StartLogin { kind } => {
                let id = self.next_id();
                let (code_tx, code_rx) = oneshot::channel();
                let tx = self.tx.clone();
                let abort =
                    self.spawn_as(id, Work::Login, login(home, http, kind, id, tx, code_rx));
                return Started::Login(LoginHandle {
                    id,
                    code: Some(code_tx),
                    abort,
                });
            }
            Effect::AddAccount { kind, secret } => {
                self.spawn(Work::AddAccount, add_account(home, http, kind, secret));
            }
            Effect::Relogin { name, secret } => {
                self.spawn(Work::Relogin, async move {
                    ops::account::relogin(&home, &name, secret).await?;
                    Ok(Output::Relogged(name))
                });
            }
            Effect::Rename { old, new } => {
                self.spawn(Work::Rename, async move {
                    ops::account::rename(&home, &old, &new).await?;
                    Ok(Output::Renamed { old, new })
                });
            }
            Effect::Remove(name) => {
                self.spawn(Work::Remove, async move {
                    ops::account::remove(&home, &name).await?;
                    Ok(Output::Removed(name))
                });
            }
            Effect::RefreshUsage(name) => {
                self.spawn(Work::RefreshUsage, async move {
                    ops::account::refresh_usage(&home, &http, &name).await?;
                    Ok(Output::UsageRefreshed(name))
                });
            }
            Effect::RefreshModels => {
                self.spawn(Work::RefreshModels, async move {
                    let report = ops::model::refresh(&home, &http, None).await?;
                    let failed = report.iter().find_map(|r| {
                        r.result
                            .as_ref()
                            .err()
                            .map(|e| format!("{}: {e}", r.account))
                    });
                    Ok(Output::ModelsRefreshed {
                        accounts: report.len(),
                        failed,
                    })
                });
            }
            Effect::CreateKey { name, base_url } => {
                self.spawn(Work::CreateKey, async move {
                    let wanted = name.clone();
                    let issued = ops::with_store(&home, move |store| {
                        ops::key::create(store, &wanted, &base_url, clock::now())
                            .map_err(|error| crate::store::Error::Invalid(error.to_string()))
                    })
                    .await?;
                    Ok(Output::KeyCreated { name, issued })
                });
            }
            Effect::Revoke(name) => {
                self.spawn(Work::Revoke, async move {
                    let wanted = name.clone();
                    if !ops::with_store(&home, move |store| store.revoke_key(&wanted)).await? {
                        anyhow::bail!("no key named {name}");
                    }
                    Ok(Output::Revoked(name))
                });
            }
            Effect::Copy(_) => {}
        }
        Started::Plain
    }
}

async fn login(
    home: PathBuf,
    http: reqwest::Client,
    kind: Kind,
    id: JobId,
    tx: mpsc::Sender<Action>,
    code: oneshot::Receiver<String>,
) -> anyhow::Result<Output> {
    let session = ops::account::begin_login(&home, &http, kind).await?;
    let prompt = session.prompt().clone();
    let needs_code = matches!(prompt, Prompt::PasteCode { .. });
    let opened = crate::browser::open(prompt.url()).await;
    let announce = Action::Job(Box::new(JobDone {
        id,
        work: Work::Login,
        result: Ok(Output::Prompt { prompt, opened }),
    }));
    tx.send(announce)
        .await
        .map_err(|_| anyhow::anyhow!("the screen closed"))?;
    let pasted = if needs_code {
        Some(code.await.context("sign-in was cancelled")?)
    } else {
        None
    };
    Ok(Output::Secret(session.complete(pasted).await?))
}

async fn add_account(
    home: PathBuf,
    http: reqwest::Client,
    kind: Kind,
    secret: Secret,
) -> anyhow::Result<Output> {
    let existing = ops::account::list(&home).await?;
    let name = ops::account::default_name(&existing, kind);
    let account = ops::account::add(&home, name, kind, secret).await?;
    let report = ops::model::refresh(&home, &http, Some(&account.name)).await;
    let models = match report.map(|mut all| all.pop()) {
        Ok(Some(refreshed)) => match refreshed.result {
            Ok(count) => Models::Count(count),
            Err(error) => Models::Failed(error),
        },
        Ok(None) => Models::Count(0),
        Err(error) => Models::Failed(format!("{error:#}")),
    };
    Ok(Output::Saved {
        name: account.name,
        models,
    })
}
