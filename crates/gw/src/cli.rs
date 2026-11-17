use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::Path;

use anyhow::{Context as _, bail};
use clap::Subcommand;

use crate::account::{Account, Secret, State};
use crate::catalog::Capabilities;
use crate::clock;
use crate::ops::{self, log::Filter};
use crate::provider::{Kind, Method, Prompt, Provider};

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    #[command(about = "add an account (provider + method)")]
    Add {
        provider: String,
        #[arg(long)]
        method: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        api: Option<String>,
    },
    #[command(about = "sign an existing account in again")]
    Login { name: String },
    #[command(about = "list accounts in selection order")]
    List,
    #[command(about = "show one account with recent sessions and errors")]
    Show { name: String },
    #[command(about = "rename an account; past log rows keep the old name")]
    Rename { old: String, new: String },
    #[command(about = "remove an account")]
    Remove {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum KeyCommand {
    #[command(about = "issue a key and print client settings")]
    Create {
        name: String,
        #[arg(long)]
        url: Option<String>,
    },
    #[command(about = "list keys")]
    List,
    #[command(about = "revoke a key")]
    Revoke {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ModelCommand {
    #[command(about = "fetch model lists from upstreams")]
    Refresh {
        #[arg(long)]
        account: Option<String>,
    },
    #[command(about = "list models across accounts")]
    List,
    #[command(about = "show per-account capabilities of a model")]
    Show { id: String },
}

#[expect(clippy::print_stdout, reason = "the CLI writes its results to stdout")]
fn out(text: &str) {
    println!("{text}");
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder().build()?)
}

pub(crate) async fn account(home: &Path, command: AccountCommand) -> anyhow::Result<()> {
    match command {
        AccountCommand::Add {
            provider,
            method,
            name,
            url,
            api,
        } => add(home, &provider, method.as_deref(), name, url, api).await,
        AccountCommand::Login { name } => {
            let existing = ops::account::find(home, &name).await?;
            let secret = obtain_secret(home, existing.kind).await?;
            ops::account::relogin(home, &name, secret).await?;
            out(&format!("{name}: signed in again"));
            Ok(())
        }
        AccountCommand::List => {
            let now = clock::now();
            for account in ops::account::list(home).await? {
                out(&account_line(&account, now));
            }
            Ok(())
        }
        AccountCommand::Show { name } => {
            let account = ops::account::find(home, &name).await?;
            let detail = ops::account::detail(home, &name).await?;
            out(&account_line(&account, clock::now()));
            out(&format!(
                "models {} · requests 24h {} · 5h tokens {} in ({} cache) / {} out",
                detail.models,
                detail.requests_24h,
                detail.input_5h,
                detail.cache_5h,
                detail.output_5h
            ));
            for (session, key, model, ts) in detail.sessions {
                out(&format!(
                    "session {session} {key} {model} {}",
                    clock::format_utc(ts)
                ));
            }
            for (ts, error) in detail.errors {
                out(&format!("error {} {error}", clock::format_utc(ts)));
            }
            Ok(())
        }
        AccountCommand::Rename { old, new } => {
            ops::account::rename(home, &old, &new).await?;
            out(&format!("{old}: renamed to {new}"));
            Ok(())
        }
        AccountCommand::Remove { name, yes } => {
            confirm(&name, yes)?;
            ops::account::remove(home, &name).await?;
            out(&format!("{name}: removed"));
            Ok(())
        }
    }
}

async fn add(
    home: &Path,
    provider: &str,
    method: Option<&str>,
    name: Option<String>,
    url: Option<String>,
    api: Option<String>,
) -> anyhow::Result<()> {
    let provider: Provider = provider.parse()?;
    let method = match method {
        Some(method) => method.parse()?,
        None => choose_method(provider)?,
    };
    let kind = Kind::new(provider, method)?;
    let secret = if provider == Provider::Custom {
        let url = url.context("custom accounts need --url")?;
        let api = api.context("custom accounts need --api messages|responses|chat")?;
        if crate::forward::parse_api(&api).is_none() {
            bail!("--api must be messages, responses or chat");
        }
        let key = read_secret("key (empty for none): ")?;
        Secret::Custom {
            url,
            api,
            key: (!key.is_empty()).then_some(key),
        }
    } else {
        obtain_secret(home, kind).await?
    };
    let accounts = ops::account::list(home).await?;
    let name = name.unwrap_or_else(|| ops::account::default_name(&accounts, kind));
    let account = ops::account::add(home, name, kind, secret).await?;
    for refreshed in ops::model::refresh(home, &http()?, Some(&account.name)).await? {
        match refreshed.result {
            Ok(count) => out(&format!("{}: added, {count} models", refreshed.account)),
            Err(error) => out(&format!(
                "{}: added, model list failed: {error}",
                refreshed.account
            )),
        }
    }
    Ok(())
}

fn choose_method(provider: Provider) -> anyhow::Result<Method> {
    let methods = provider.methods();
    if let [only] = methods {
        return Ok(*only);
    }
    if !std::io::stdin().is_terminal() {
        let names: Vec<&str> = methods.iter().map(|m| m.as_str()).collect();
        bail!("--method is required: {}", names.join(", "));
    }
    for (index, method) in methods.iter().enumerate() {
        let kind = Kind::new(provider, *method)?;
        out(&format!("{}. {method} ({:?})", index + 1, kind.billing()));
    }
    let answer = read_line("method: ")?;
    let index: usize = answer.trim().parse().context("enter a number")?;
    methods
        .get(index.wrapping_sub(1))
        .copied()
        .context("no such method")
}

async fn obtain_secret(home: &Path, kind: Kind) -> anyhow::Result<Secret> {
    if ops::account::needs_paste(kind) {
        let label = match kind.method() {
            Method::SetupToken => "token from `claude setup-token`: ",
            Method::Oauth
            | Method::Siwc
            | Method::Codex
            | Method::Plan
            | Method::Key
            | Method::Custom => "key: ",
        };
        let value = read_secret(label)?;
        if value.is_empty() {
            bail!("empty value");
        }
        return Ok(ops::account::pasted_secret(kind, value, clock::now()));
    }
    let session = ops::account::begin_login(home, &http()?, kind).await?;
    let pasted = match session.prompt() {
        Prompt::PasteCode { url } => {
            out(&format!("open: {url}"));
            Some(read_line("code: ")?)
        }
        Prompt::Loopback { url, ssh_hint, .. } => {
            out(&format!("on your machine: {ssh_hint}"));
            out(&format!("then open: {url}"));
            out("waiting for the browser callback…");
            None
        }
        Prompt::Device {
            verification_uri,
            user_code,
            expires_at,
            ..
        } => {
            out(&format!("open: {verification_uri}"));
            out(&format!("enter: {user_code}"));
            out(&format!(
                "waiting for approval until {}",
                clock::format_utc(*expires_at)
            ));
            None
        }
    };
    Ok(session
        .complete(pasted.map(|value| value.trim().to_owned()))
        .await?)
}

fn read_line(prompt: &str) -> anyhow::Result<String> {
    let mut stdout = std::io::stdout();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn read_secret(prompt: &str) -> anyhow::Result<String> {
    if std::io::stdin().is_terminal() {
        Ok(rpassword::prompt_password(prompt)?.trim().to_owned())
    } else {
        Ok(read_line("")?.trim().to_owned())
    }
}

fn confirm(name: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("pass --yes to confirm in non-interactive use");
    }
    if read_line(&format!("type {name} to confirm: "))?.trim() != name {
        bail!("not confirmed");
    }
    Ok(())
}

fn account_line(account: &Account, now: i64) -> String {
    let state = match &account.state {
        State::Active => "ok".to_owned(),
        State::Blocked { reset_at, .. } if *reset_at <= now => "ok".to_owned(),
        State::Blocked { reset_at, .. } => {
            format!("limited until {}", clock::format_utc(*reset_at))
        }
        State::Inactive { reason } => format!("inactive ({reason})"),
    };
    let windows: Vec<String> = account
        .usage
        .iter()
        .flat_map(|usage| usage.windows.iter())
        .map(|w| format!("{} {:.0}%", w.label, w.used * 100.0))
        .collect();
    format!(
        "{:<22} {:<12} {:<4?} {:<28} {}",
        account.name,
        account.kind.method(),
        account.kind.billing(),
        state,
        windows.join("  ")
    )
}

pub(crate) async fn key(home: &Path, command: KeyCommand) -> anyhow::Result<()> {
    match command {
        KeyCommand::Create { name, url } => {
            let base = match url {
                Some(url) => url,
                None => server_url(home).await?,
            };
            let created = ops::with_store(home, move |store| {
                ops::key::create(store, &name, &base, clock::now())
                    .map_err(|error| crate::store::Error::Invalid(error.to_string()))
            })
            .await?;
            out(&format!("key (shown once): {}\n", created.key));
            for snippet in created.snippets {
                out(&format!("── {} ──\n{}", snippet.client, snippet.text));
            }
            Ok(())
        }
        KeyCommand::List => {
            for key in ops::with_store(home, |store| store.keys()).await? {
                out(&format!(
                    "{:<20} {}",
                    key.name,
                    clock::format_utc(key.created_at)
                ));
            }
            Ok(())
        }
        KeyCommand::Revoke { name, yes } => {
            confirm(&name, yes)?;
            let wanted = name.clone();
            if !ops::with_store(home, move |store| store.revoke_key(&wanted)).await? {
                bail!("no key named {name}");
            }
            out(&format!("{name}: revoked"));
            Ok(())
        }
    }
}

async fn server_url(home: &Path) -> anyhow::Result<String> {
    let heartbeat = ops::with_store(home, |store| store.last_heartbeat()).await?;
    let (_, listen) =
        heartbeat.context("server address unknown: start `gw serve` first or pass --url")?;
    Ok(format!("http://{listen}"))
}

pub(crate) async fn model(home: &Path, command: ModelCommand) -> anyhow::Result<()> {
    match command {
        ModelCommand::Refresh { account } => {
            for refreshed in ops::model::refresh(home, &http()?, account.as_deref()).await? {
                match refreshed.result {
                    Ok(count) => out(&format!("{}: {count} models", refreshed.account)),
                    Err(error) => out(&format!("{}: failed: {error}", refreshed.account)),
                }
                if let Some(balance) = refreshed.balance {
                    out(&format!(
                        "{}: balance ${:.2} (voucher ${:.2}, cash ${:.2})",
                        refreshed.account, balance.available, balance.voucher, balance.cash
                    ));
                }
            }
            Ok(())
        }
        ModelCommand::List => {
            for view in ops::model::views(home).await? {
                out(&format!(
                    "{:<32} ctx {:<9} efforts {:<28} {}",
                    view.id,
                    view.context
                        .map_or_else(|| "?".to_owned(), |c| c.to_string()),
                    view.efforts.map_or_else(|| "?".to_owned(), |e| e.join(",")),
                    view.accounts.join(" ")
                ));
            }
            Ok(())
        }
        ModelCommand::Show { id } => {
            for (account, caps) in ops::model::capabilities(home, &id).await? {
                out(&format!("{account}: {}", describe_caps(&caps)));
            }
            Ok(())
        }
    }
}

fn describe_caps(caps: &Capabilities) -> String {
    let known = |value: Option<String>| value.unwrap_or_else(|| "?".to_owned());
    format!(
        "context {} · max_output {} · efforts {} · fast {} · image {} · unsupported [{}]",
        known(caps.context.map(|v| v.to_string())),
        known(caps.max_output.map(|v| v.to_string())),
        known(caps.efforts.as_ref().map(|v| v.join(","))),
        known(caps.fast.map(|v| v.to_string())),
        known(caps.image.map(|v| v.to_string())),
        caps.unsupported.join(",")
    )
}

pub(crate) async fn log(home: &Path, follow: bool, filter: Option<&str>) -> anyhow::Result<()> {
    let filter = Filter::parse(filter.unwrap_or_default()).map_err(anyhow::Error::msg)?;
    let mut last = 0;
    loop {
        let after = last;
        let rows = ops::with_store(home, move |store| store.log_after(after, 200)).await?;
        for row in &rows {
            last = last.max(row.id);
            if filter.matches(row) {
                out(&format!(
                    "{} {:<10} {:<8} {:<10} {:<28} {:<22} {:<10} {} in {} (cache {}) out {} {}ms {} {}",
                    clock::format_utc(row.ts),
                    row.key.as_deref().unwrap_or("-"),
                    row.session
                        .as_deref()
                        .map_or("-", |s| s.get(..8).unwrap_or(s)),
                    row.api,
                    row.model,
                    row.account.as_deref().unwrap_or("-"),
                    row.reason,
                    row.status,
                    row.input.unwrap_or_default() + row.cache_read.unwrap_or_default(),
                    row.cache_read.unwrap_or_default(),
                    row.output.unwrap_or_default(),
                    row.duration_ms,
                    row.error.as_deref().unwrap_or_default(),
                    row.note.as_deref().unwrap_or_default(),
                ));
            }
        }
        if !follow {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
