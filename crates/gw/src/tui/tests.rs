use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use tokio::sync::{mpsc, oneshot};
use unicode_width::UnicodeWidthStr as _;

use super::action::{Action, Command, Effect};
use super::data::{KeyInfo, ModelInfo, Snapshot};
use super::flow::add_account::AddAccount;
use super::*;
use crate::account::{Account, Secret, State};
use crate::catalog::view::ModelView;
use crate::catalog::{Capabilities, Catalog};
use crate::ops::key;
use crate::provider::{Kind, Method, Prompt, Provider, Scope, Usage, Window};
use crate::store::{AccountUse, LogRow, LogSummary};
use view::View;

const NOW: i64 = 1_763_301_600;

fn account(name: &str, provider: Provider, method: Method, created: i64) -> Account {
    Account {
        name: name.to_owned(),
        kind: Kind::new(provider, method).unwrap(),
        secret: Secret::Key { key: String::new() },
        state: State::Active,
        usage: None,
        usage_at: None,
        created_at: created,
    }
}

fn with_usage(mut account: Account, windows: &[(&str, f64, i64)], age: i64) -> Account {
    account.usage = Some(Usage {
        windows: windows
            .iter()
            .map(|(label, used, reset)| Window {
                label: (*label).to_owned(),
                used: *used,
                reset_at: Some(NOW + reset),
            })
            .collect(),
    });
    account.usage_at = Some(NOW - age);
    account
}

fn healthy_accounts() -> Vec<Account> {
    vec![
        with_usage(
            account("claude-max-1", Provider::Anthropic, Method::Oauth, 1),
            &[("5h", 0.72, 4320), ("7d", 0.31, 3 * 86_400)],
            30,
        ),
        with_usage(
            account("claude-max-2", Provider::Anthropic, Method::Oauth, 2),
            &[("5h", 0.10, 9000)],
            30,
        ),
        account("anthropic-api", Provider::Anthropic, Method::Key, 3),
        with_usage(
            account("chatgpt-pro-1", Provider::OpenAi, Method::Codex, 4),
            &[("5h", 0.18, 10_920)],
            30,
        ),
        account("ollama", Provider::Custom, Method::Custom, 5),
    ]
}

fn problem_accounts() -> Vec<Account> {
    let mut accounts = healthy_accounts();
    accounts[1].state = State::Blocked {
        scope: Scope::Account,
        reset_at: NOW + 2460,
    };
    accounts.push(account("chatgpt-pro-2", Provider::OpenAi, Method::Codex, 6));
    accounts[5].state = State::Inactive {
        reason: "refresh failed 2025-11-16 14:02 UTC".to_owned(),
    };
    accounts[3] = with_usage(accounts[3].clone(), &[("5h", 0.18, 10_920)], 3 * 3600);
    accounts
}

fn catalog(accounts: &[Account]) -> Catalog {
    let mut catalog = Catalog::default();
    for account in accounts {
        let models: &[&str] = match account.kind.provider() {
            Provider::Anthropic => &["claude-opus-5-5", "claude-sonnet-5-5"],
            Provider::OpenAi => &["gpt-6.1-sol"],
            Provider::Custom => &["qwen3:32b"],
            Provider::Zai | Provider::Moonshot => &[],
        };
        for model in models {
            let caps = Capabilities {
                context: Some(if model.starts_with("gpt") {
                    400_000
                } else {
                    200_000
                }),
                fast: (account.name == "anthropic-api").then_some(false),
                ..Capabilities::default()
            };
            catalog.insert(&account.name, model, caps);
        }
    }
    catalog
}

fn log_row(id: i64, ago: i64, account: &str, status: u16, reason: &str) -> LogRow {
    LogRow {
        id,
        ts: NOW - ago,
        key: Some("macbook".to_owned()),
        session: Some("3f2a91c0aa".to_owned()),
        api: "messages".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        account: Some(account.to_owned()),
        reason: reason.to_owned(),
        status,
        input: Some(12_400),
        cache_read: Some(0),
        cache_write: Some(0),
        output: Some(812),
        duration_ms: 4100,
        ..LogRow::default()
    }
}

fn log() -> Vec<LogRow> {
    let mut limit = log_row(3, 70, "claude-max-2", 429, "limit");
    limit.error = Some("claude-max-2 hit its 5h limit (resets 15:00)".to_owned());
    limit.input = None;
    let mut failed = log_row(4, 50, "chatgpt-pro-1", 502, "new");
    failed.error = Some("upstream connection reset".to_owned());
    failed.model = "gpt-6.1-sol".to_owned();
    failed.key = Some("desktop".to_owned());
    vec![
        log_row(1, 300, "claude-max-1", 200, "new"),
        log_row(2, 90, "claude-max-1", 200, "reused"),
        limit,
        failed,
        log_row(5, 20, "claude-max-1", 200, "reused"),
    ]
}

fn snapshot(accounts: Vec<Account>) -> Snapshot {
    let catalog = catalog(&accounts);
    let views = crate::catalog::view::models(&catalog, &accounts);
    let models = views
        .into_iter()
        .map(|view: ModelView| ModelInfo {
            next: crate::ops::model::next_in(&accounts, &catalog, &view.id, NOW),
            view,
        })
        .collect();
    Snapshot {
        now: NOW,
        models,
        catalog,
        keys: vec![
            KeyInfo {
                name: "macbook".to_owned(),
                created_at: NOW - 5 * 86_400,
                last_used: Some(NOW - 12),
            },
            KeyInfo {
                name: "desktop".to_owned(),
                created_at: NOW - 9 * 86_400,
                last_used: Some(NOW - 2 * 86_400),
            },
            KeyInfo {
                name: "t3code".to_owned(),
                created_at: NOW - 86_400,
                last_used: None,
            },
        ],
        log: log(),
        hour: LogSummary {
            requests: 182,
            limits: 3,
            errors: 1,
        },
        five_hours: vec![AccountUse {
            account: "claude-max-1".to_owned(),
            requests: 1204,
            input: 4_200_000,
            cached: 3_906_000,
            output: 210_000,
        }],
        heartbeat: Some((NOW - 2, "100.64.0.2:8787".to_owned())),
        accounts,
    }
}

struct Harness {
    app: App,
    _rx: mpsc::Receiver<Action>,
    width: u16,
    height: u16,
}

impl Harness {
    fn new(snapshot: Snapshot) -> Self {
        let (tx, rx) = mpsc::channel(8);
        let jobs = Jobs::new(PathBuf::from("/nonexistent"), tx);
        let mut app = App::new(jobs, Theme::new(false), TimeZone::UTC);
        app.apply(snapshot);
        app.sync_views();
        Self {
            app,
            _rx: rx,
            width: 120,
            height: 30,
        }
    }

    fn size(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self.app.wide = width >= WIDE_COLUMNS;
        self
    }

    fn view(mut self, view: View) -> Self {
        self.app.view = view;
        self
    }

    fn press(&mut self, code: KeyCode) -> &mut Self {
        self.app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        self
    }

    fn key(&mut self, c: char) -> &mut Self {
        self.press(KeyCode::Char(c))
    }

    fn type_text(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.key(c);
        }
        self
    }

    fn render(&mut self) -> String {
        let mut terminal = Terminal::new(TestBackend::new(self.width, self.height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut self.app)).unwrap();
        text_of(terminal.backend().buffer())
    }

    fn selected(&mut self) -> String {
        let buffer = self.buffer();
        (0..buffer.area.height)
            .filter(|y| {
                (0..buffer.area.width)
                    .any(|x| buffer[(x, *y)].modifier.contains(Modifier::REVERSED))
            })
            .map(|y| {
                (0..buffer.area.width)
                    .filter(|x| buffer[(*x, y)].modifier.contains(Modifier::REVERSED))
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn buffer(&mut self) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(self.width, self.height)).unwrap();
        terminal.draw(|frame| draw(frame, &mut self.app)).unwrap();
        terminal.backend().buffer().clone()
    }
}

fn text_of(buffer: &Buffer) -> String {
    let mut out = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        let mut x = 0;
        while x < buffer.area.width {
            let symbol = buffer[(x, y)].symbol();
            line.push_str(symbol);
            x += u16::try_from(symbol.width().max(1)).unwrap();
        }
        out.push(line.trim_end().to_owned());
    }
    out.join("\n")
}

fn accounts_view(accounts: Vec<Account>) -> Harness {
    Harness::new(snapshot(accounts))
}

#[test]
fn accounts_wide() {
    insta::assert_snapshot!(accounts_view(healthy_accounts()).render());
}

#[test]
fn accounts_narrow_list() {
    insta::assert_snapshot!(accounts_view(healthy_accounts()).size(80, 24).render());
}

#[test]
fn accounts_narrow_detail_opens_with_right_and_closes_with_left() {
    let mut harness = accounts_view(healthy_accounts()).size(80, 24);
    harness.press(KeyCode::Right);
    insta::assert_snapshot!(harness.render());
    harness.press(KeyCode::Left);
    assert!(harness.render().contains("claude-max-2"));
}

#[test]
fn accounts_empty() {
    let mut snapshot = snapshot(Vec::new());
    snapshot.keys.clear();
    insta::assert_snapshot!(Harness::new(snapshot).render());
}

#[test]
fn accounts_problem_limited_selected() {
    let mut harness = accounts_view(problem_accounts());
    harness.press(KeyCode::Down);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn accounts_problem_signed_out_with_server_stopped() {
    let mut snapshot = snapshot(problem_accounts());
    snapshot.heartbeat = Some((NOW - 60, "100.64.0.2:8787".to_owned()));
    let mut harness = Harness::new(snapshot);
    harness.press(KeyCode::End).press(KeyCode::Up);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn accounts_problem_narrow() {
    let mut harness = accounts_view(problem_accounts()).size(80, 24);
    harness.press(KeyCode::End).press(KeyCode::Right);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn activity_normal() {
    insta::assert_snapshot!(
        accounts_view(healthy_accounts())
            .view(View::Activity)
            .render()
    );
}

#[test]
fn activity_error_selected() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Activity);
    harness.press(KeyCode::Up);
    harness.press(KeyCode::Up);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn activity_limit_selected_and_not_following() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Activity);
    for _ in 0..3 {
        harness.press(KeyCode::Up);
    }
    let mut next = snapshot(healthy_accounts());
    next.log.push(log_row(6, 5, "claude-max-1", 200, "reused"));
    next.log.push(log_row(7, 2, "claude-max-1", 200, "reused"));
    harness.app.apply(next);
    harness.app.sync_views();
    insta::assert_snapshot!(harness.render());
}

#[test]
fn activity_empty() {
    let mut snapshot = snapshot(healthy_accounts());
    snapshot.log.clear();
    insta::assert_snapshot!(Harness::new(snapshot).view(View::Activity).render());
}

#[test]
fn activity_narrow() {
    insta::assert_snapshot!(
        accounts_view(healthy_accounts())
            .view(View::Activity)
            .size(80, 24)
            .render()
    );
}

#[test]
fn activity_filter_searches_every_column() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Activity);
    harness.key('/').type_text("resets");
    let screen = harness.render();
    assert!(screen.contains("limit"));
    assert!(!screen.contains("502"));
    harness.press(KeyCode::Esc);
    assert!(harness.render().contains("502"));
}

#[test]
fn models_normal() {
    insta::assert_snapshot!(
        accounts_view(healthy_accounts())
            .view(View::Models)
            .render()
    );
}

#[test]
fn models_empty() {
    insta::assert_snapshot!(
        Harness::new(snapshot(Vec::new()))
            .view(View::Models)
            .render()
    );
}

#[test]
fn models_narrow_detail() {
    let mut harness = accounts_view(problem_accounts())
        .view(View::Models)
        .size(80, 24);
    harness.press(KeyCode::Right);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn models_every_account_limited() {
    let mut accounts = healthy_accounts();
    for (index, delay) in [(0, 3000), (1, 3600), (2, 4200)] {
        accounts[index].state = State::Blocked {
            scope: Scope::Account,
            reset_at: NOW + delay,
        };
    }
    insta::assert_snapshot!(accounts_view(accounts).view(View::Models).render());
}

#[test]
fn keys_normal() {
    insta::assert_snapshot!(accounts_view(healthy_accounts()).view(View::Keys).render());
}

#[test]
fn keys_empty() {
    let mut snapshot = snapshot(healthy_accounts());
    snapshot.keys.clear();
    insta::assert_snapshot!(Harness::new(snapshot).view(View::Keys).render());
}

#[test]
fn keys_narrow_detail_switches_client() {
    let mut harness = accounts_view(healthy_accounts())
        .view(View::Keys)
        .size(80, 24);
    harness.press(KeyCode::Right).press(KeyCode::Right);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn actions_menu_for_an_account() {
    let mut harness = accounts_view(problem_accounts());
    harness.press(KeyCode::Enter);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn actions_menu_explains_unavailable_items() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Enter);
    let screen = harness.render();
    assert!(screen.contains("Refresh usage (no usage API)"));
    assert!(
        harness
            .selected()
            .lines()
            .any(|line| line.starts_with("Rename"))
    );
}

#[test]
fn confirm_defaults_to_cancel() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('d');
    insta::assert_snapshot!(harness.render());
}

#[test]
fn help_lists_the_keys_of_the_screen() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('?');
    insta::assert_snapshot!(harness.render());
}

#[test]
fn rename_dialog() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('e');
    insta::assert_snapshot!(harness.render());
}

#[test]
fn terminal_too_small() {
    insta::assert_snapshot!(accounts_view(healthy_accounts()).size(50, 12).render());
}

fn wizard() -> Harness {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('a');
    harness
}

#[test]
fn wizard_service_step() {
    insta::assert_snapshot!(wizard().render());
}

#[test]
fn wizard_method_step() {
    let mut harness = wizard();
    harness.press(KeyCode::Enter);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn wizard_credential_step_masks_and_reveals() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    harness.type_text("sk-ant-secret");
    let hidden = harness.render();
    assert!(hidden.contains("•••••••••••••"));
    assert!(!hidden.contains("sk-ant-secret"));
    insta::assert_snapshot!(hidden);
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(harness.render().contains("sk-ant-secret"));
}

#[test]
fn wizard_custom_endpoint_step() {
    let mut harness = wizard();
    for _ in 0..4 {
        harness.press(KeyCode::Down);
    }
    harness.press(KeyCode::Enter);
    harness.press(KeyCode::Enter);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn wizard_rejects_a_bad_address_inline() {
    let mut harness = wizard();
    for _ in 0..4 {
        harness.press(KeyCode::Down);
    }
    harness
        .press(KeyCode::Enter)
        .type_text("localhost:11434")
        .press(KeyCode::Enter);
    let screen = harness.render();
    assert!(screen.contains("Enter a full http:// or https:// address"));
    assert!(screen.contains("2 / 3"));
}

#[tokio::test]
async fn wizard_back_keeps_every_value() {
    let mut harness = wizard();
    for _ in 0..4 {
        harness.press(KeyCode::Down);
    }
    harness.press(KeyCode::Enter);
    harness
        .type_text("http://localhost:11434")
        .press(KeyCode::Enter);
    harness.type_text("chat").press(KeyCode::Enter);
    harness.type_text("my-key");
    assert!(harness.render().contains("3 / 3"));
    harness.press(KeyCode::Esc);
    assert!(harness.render().contains("chat"));
    harness.press(KeyCode::Esc);
    let screen = harness.render();
    assert!(screen.contains("http://localhost:11434"));
    harness.press(KeyCode::Esc);
    let screen = harness.render();
    assert!(screen.contains("1 / 3"));
    assert!(
        harness
            .selected()
            .lines()
            .any(|line| line.starts_with("Other endpoint"))
    );
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Enter)
        .press(KeyCode::Enter);
    assert!(harness.render().contains("••••••"));
}

#[test]
fn wizard_method_choice_survives_going_back() {
    let mut harness = wizard();
    harness.press(KeyCode::Enter).press(KeyCode::Down);
    harness.press(KeyCode::Esc);
    harness.press(KeyCode::Enter);
    assert!(
        harness
            .selected()
            .lines()
            .any(|line| line.starts_with("Setup token"))
    );
}

#[test]
fn wizard_esc_on_the_first_step_cancels_and_saves_nothing() {
    let mut harness = wizard();
    harness.press(KeyCode::Esc);
    assert!(harness.app.modals.is_empty());
}

#[test]
fn wizard_pasted_key_emits_one_add_effect() {
    let snapshot = snapshot(healthy_accounts());
    let mut wizard = AddAccount::new();
    wizard.command(Command::Select, &snapshot);
    wizard.command(Command::Down, &snapshot);
    wizard.command(Command::Down, &snapshot);
    wizard.command(Command::Select, &snapshot);
    wizard.field_mut().unwrap().paste("  sk-ant-1\n");
    let reply = wizard.command(Command::Select, &snapshot);
    assert!(matches!(
        reply,
        Reply::Do(Effect::AddAccount { secret: Secret::Key { ref key }, .. }) if key == "sk-ant-1"
    ));
    assert_eq!(wizard.context(), Context::Busy);
}

#[test]
fn wizard_empty_secret_shows_an_inline_message() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness.press(KeyCode::Enter).press(KeyCode::Enter);
    assert!(harness.render().contains("Paste a value first"));
}

fn feed(harness: &mut Harness, work: jobs::Work, output: anyhow::Result<jobs::Output>) {
    let done = jobs::JobDone {
        id: jobs::JobId::for_test(0),
        work,
        result: output,
    };
    update(&mut harness.app, Action::Job(Box::new(done)));
}

#[tokio::test]
async fn wizard_done_step_offers_rename_with_the_default_filled_in() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness
        .press(KeyCode::Enter)
        .type_text("sk-ant-1")
        .press(KeyCode::Enter);
    feed(
        &mut harness,
        jobs::Work::AddAccount,
        Ok(jobs::Output::Saved {
            name: "anthropic-key-2".to_owned(),
            models: jobs::Models::Count(12),
        }),
    );
    insta::assert_snapshot!(harness.render());
    harness.press(KeyCode::Enter);
    assert!(harness.app.modals.is_empty());
}

#[tokio::test]
async fn wizard_done_rename_validates_then_emits_a_rename() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness
        .press(KeyCode::Enter)
        .type_text("k")
        .press(KeyCode::Enter);
    feed(
        &mut harness,
        jobs::Work::AddAccount,
        Ok(jobs::Output::Saved {
            name: "anthropic-key-2".to_owned(),
            models: jobs::Models::Failed("offline".to_owned()),
        }),
    );
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    harness.type_text("claude-max-1").press(KeyCode::Enter);
    assert!(harness.render().contains("That name is already used"));
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    harness.type_text("work-key").press(KeyCode::Enter);
    assert!(harness.app.modals.is_empty());
}

#[tokio::test]
async fn wizard_failed_save_returns_to_the_credential_step_with_the_reason() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness
        .press(KeyCode::Enter)
        .type_text("k")
        .press(KeyCode::Enter);
    feed(
        &mut harness,
        jobs::Work::AddAccount,
        Err(anyhow::anyhow!("database is locked")),
    );
    let screen = harness.render();
    assert!(screen.contains("database is locked"));
    assert!(screen.contains("3 / 3"));
}

#[tokio::test]
async fn one_time_key_screen() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Keys);
    harness.key('a').type_text("laptop").press(KeyCode::Enter);
    assert_eq!(
        harness.app.modals.last().map(Modal::context),
        Some(Context::Busy)
    );
    let issued = key::Issued {
        key: "gw_k3Zr9QeVJ1uH0w5LpA2xYdT8mNcB7fSgR4iOa6EbU".to_owned(),
        snippets: key::snippets(
            "http://100.64.0.2:8787",
            "gw_k3Zr9QeVJ1uH0w5LpA2xYdT8mNcB7fSgR4iOa6EbU",
        ),
    };
    feed(
        &mut harness,
        jobs::Work::CreateKey,
        Ok(jobs::Output::KeyCreated {
            name: "laptop".to_owned(),
            issued,
        }),
    );
    insta::assert_snapshot!(harness.render());
    harness.press(KeyCode::Esc);
    assert_eq!(
        harness.app.modals.len(),
        1,
        "esc must not close the one-time screen"
    );
    harness.key('c');
    assert!(harness.app.clipboard.last().unwrap().contains("]52;c;"));
    assert!(
        harness
            .render()
            .contains("Copied gw_k3Zr9QeVJ1uH0w5LpA2xYdT8mNcB7fSgR4iOa6EbU")
    );
    harness.press(KeyCode::Enter);
    assert!(harness.app.modals.is_empty());
}

#[test]
fn new_key_name_step() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Keys);
    harness.key('a').type_text("mac book").press(KeyCode::Enter);
    insta::assert_snapshot!(harness.render());
}

#[test]
fn keymap_footer_matches_the_screen() {
    let mut harness = accounts_view(healthy_accounts());
    let screen = harness.render();
    let footer = screen.lines().last().unwrap();
    for (key, label) in keymap::footer(Context::Accounts) {
        assert!(footer.contains(&format!("{key} {label}")), "{footer}");
    }
}

#[test]
fn modal_on_top_receives_the_keys_and_blocks_the_screen_beneath() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Enter);
    harness.key('q');
    assert!(!harness.app.quit);
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.view, View::Accounts);
    harness.key('3');
    assert_eq!(harness.app.view, View::Accounts);
    harness.key('j');
    assert!(
        harness
            .selected()
            .lines()
            .any(|line| line.starts_with("Sign in again"))
    );
    harness.press(KeyCode::Esc);
    assert!(harness.app.modals.is_empty());
    harness.key('q');
    assert!(harness.app.quit);
}

#[test]
fn only_the_top_modal_handles_input() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Enter);
    harness.key('d');
    assert_eq!(harness.app.modals.len(), 1);
    assert!(matches!(harness.app.modals.last(), Some(Modal::Confirm(_))));
    harness.press(KeyCode::Esc);
    assert_eq!(harness.app.modals.len(), 0);
}

#[test]
fn menu_shortcut_letters_run_the_item() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Enter).key('e');
    assert!(matches!(harness.app.modals.last(), Some(Modal::Rename(_))));
}

#[tokio::test]
async fn confirm_requires_moving_to_the_action() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('d').press(KeyCode::Enter);
    assert!(harness.app.modals.is_empty());
    harness.key('d').press(KeyCode::Right);
    assert!(harness.render().contains("[ Remove ]"));
    harness.press(KeyCode::Enter);
    assert!(harness.app.modals.is_empty());
}

#[test]
fn typing_in_a_filter_does_not_trigger_shortcuts() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('/').type_text("q1a2");
    assert!(!harness.app.quit);
    assert_eq!(harness.app.view, View::Accounts);
    harness.press(KeyCode::Esc);
    harness.key('q');
    assert!(harness.app.quit);
}

#[test]
fn filter_narrows_the_list_and_keeps_the_selection_valid() {
    let mut harness = accounts_view(healthy_accounts());
    harness.key('/').type_text("chatgpt").press(KeyCode::Enter);
    let screen = harness.render();
    assert!(screen.contains("chatgpt-pro-1"));
    assert!(!screen.contains("claude-max-1"));
    harness.press(KeyCode::Esc);
    assert!(harness.render().contains("claude-max-1"));
}

#[test]
fn selection_follows_the_account_when_the_order_changes() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Down);
    let mut next = healthy_accounts();
    next[0].created_at = 10;
    harness.app.apply(snapshot(next));
    harness.app.sync_views();
    harness.press(KeyCode::Enter);
    let screen = harness.render();
    assert!(screen.contains("claude-max-2"), "{screen}");
}

#[test]
fn selection_moves_to_a_neighbour_when_the_account_disappears() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Down);
    let mut next = healthy_accounts();
    next.remove(1);
    harness.app.apply(snapshot(next));
    harness.app.sync_views();
    assert!(harness.app.accounts.selected(&harness.app.env()).is_some());
}

#[test]
fn activity_follows_new_rows_until_the_user_scrolls_up() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Activity);
    assert!(harness.app.activity.is_following());
    harness.press(KeyCode::Up);
    assert!(!harness.app.activity.is_following());
    harness.press(KeyCode::End);
    assert!(harness.app.activity.is_following());
}

#[test]
fn ctrl_c_quits_even_inside_a_text_field() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Down)
        .press(KeyCode::Down)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(harness.app.quit);
}

#[test]
fn key_release_events_are_ignored() {
    let mut harness = accounts_view(healthy_accounts());
    harness.app.on_key(KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert!(!harness.app.quit);
}

#[test]
fn paste_goes_to_the_focused_field_with_newlines_removed() {
    let mut harness = wizard();
    harness
        .press(KeyCode::Enter)
        .press(KeyCode::Down)
        .press(KeyCode::Down);
    harness.press(KeyCode::Enter);
    update(
        &mut harness.app,
        Action::Paste("\n sk-ant-\nabc \n".to_owned()),
    );
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    assert!(harness.render().contains("sk-ant-abc"));
}

#[test]
fn copy_writes_osc52_and_shows_the_copied_text() {
    let mut harness = accounts_view(healthy_accounts()).view(View::Models);
    harness.key('c');
    let sequence = harness.app.clipboard.last().unwrap().clone();
    assert!(sequence.starts_with("\x1b]52;c;"));
    assert!(sequence.contains("Y2xhdWRlLW9wdXMtNS01"));
    assert!(harness.render().contains("Copied claude-opus-5-5"));
}

#[test]
fn tab_and_number_keys_switch_screens() {
    let mut harness = accounts_view(healthy_accounts());
    harness.press(KeyCode::Tab);
    assert_eq!(harness.app.view, View::Activity);
    harness.key('4');
    assert_eq!(harness.app.view, View::Keys);
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(harness.app.view, View::Models);
}

#[test]
fn current_tab_is_underlined_and_the_selected_row_is_reversed() {
    let mut harness = accounts_view(healthy_accounts());
    let buffer = harness.buffer();
    let tab = (0..buffer.area.width)
        .find(|x| buffer[(*x, 0)].symbol() == "A")
        .unwrap();
    assert!(buffer[(tab, 0)].modifier.contains(Modifier::UNDERLINED));
    let selected = harness.selected();
    assert_eq!(selected.lines().count(), 1, "{selected}");
}

#[test]
fn no_screen_uses_a_middle_dot_separator() {
    for view in View::ALL {
        let mut harness = accounts_view(problem_accounts()).view(view);
        assert!(!harness.render().contains('·'), "{view:?}");
    }
}

#[test]
fn no_color_theme_paints_nothing() {
    let theme = Theme::new(false);
    assert_eq!(theme.ok().fg, None);
    assert_eq!(theme.warn().fg, None);
    assert_eq!(theme.error().fg, None);
    assert_eq!(theme.accent().fg, None);
    assert!(Theme::new(true).ok().fg.is_some());
}

#[test]
fn problems_always_carry_a_word_or_glyph() {
    let mut harness = accounts_view(problem_accounts());
    let screen = harness.render();
    assert!(screen.contains("limited"));
    assert!(screen.contains("sign in needed"));
}

#[tokio::test]
async fn key_flow_creates_through_the_store() {
    let dir = std::env::temp_dir().join(format!("gw-tui-flow-{}", std::process::id()));
    let (tx, mut rx) = mpsc::channel(8);
    let mut jobs = Jobs::new(dir.clone(), tx);
    jobs.start(Effect::CreateKey {
        name: "laptop".to_owned(),
        base_url: "http://x".to_owned(),
    });
    let Some(Action::Job(done)) = rx.recv().await else {
        panic!("expected a finished job");
    };
    assert!(matches!(done.result, Ok(jobs::Output::KeyCreated { .. })));
    jobs.start(Effect::Revoke("laptop".to_owned()));
    let Some(Action::Job(done)) = rx.recv().await else {
        panic!("expected a finished job");
    };
    assert!(matches!(done.result, Ok(jobs::Output::Revoked(_))));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn account_jobs_rename_and_remove_through_the_store() {
    let dir = std::env::temp_dir().join(format!("gw-tui-accounts-{}", std::process::id()));
    crate::ops::account::add(
        &dir,
        "old".to_owned(),
        Kind::new(Provider::Custom, Method::Custom).unwrap(),
        Secret::Custom {
            url: "http://127.0.0.1:1".to_owned(),
            api: "chat".to_owned(),
            key: None,
        },
    )
    .await
    .unwrap();
    let (tx, mut rx) = mpsc::channel(8);
    let mut jobs = Jobs::new(dir.clone(), tx);
    jobs.start(Effect::Rename {
        old: "old".to_owned(),
        new: "new".to_owned(),
    });
    let Some(Action::Job(done)) = rx.recv().await else {
        panic!("expected a finished job");
    };
    assert!(matches!(done.result, Ok(jobs::Output::Renamed { .. })));
    let snapshot = data::load(&dir).await.unwrap();
    assert_eq!(snapshot.account_names(), vec!["new".to_owned()]);
    jobs.start(Effect::Remove("new".to_owned()));
    let Some(Action::Job(done)) = rx.recv().await else {
        panic!("expected a finished job");
    };
    assert!(matches!(done.result, Ok(jobs::Output::Removed(_))));
    std::fs::remove_dir_all(dir).unwrap();
}

fn login_wizard(
    downs: usize,
    method_downs: usize,
    prompt: Prompt,
) -> (Harness, oneshot::Receiver<String>) {
    let snapshot = snapshot(healthy_accounts());
    let mut wizard = AddAccount::new();
    for _ in 0..downs {
        wizard.command(Command::Down, &snapshot);
    }
    wizard.command(Command::Select, &snapshot);
    for _ in 0..method_downs {
        wizard.command(Command::Down, &snapshot);
    }
    let started = wizard.command(Command::Select, &snapshot);
    assert!(matches!(started, Reply::Do(Effect::StartLogin { .. })));
    let id = jobs::JobId::for_test(7);
    let (handle, code) = jobs::LoginHandle::for_test(id);
    wizard.attach(handle);
    let done = jobs::JobDone {
        id,
        work: jobs::Work::Login,
        result: Ok(jobs::Output::Prompt {
            prompt,
            opened: false,
        }),
    };
    assert!(wizard.job(done).is_ok());
    let mut harness = accounts_view(healthy_accounts());
    harness.app.modals.push(Modal::AddAccount(Box::new(wizard)));
    (harness, code)
}

#[tokio::test]
async fn wizard_device_code_step() {
    let (mut harness, _code) = login_wizard(
        1,
        1,
        Prompt::Device {
            verification_uri: "https://auth.openai.com/codex/device".to_owned(),
            user_code: "ABCD-1234".to_owned(),
            expires_at: NOW + 840,
            interval: 5,
        },
    );
    insta::assert_snapshot!(harness.render());
    harness.key('c');
    assert!(harness.app.clipboard.last().unwrap().contains("]52;c;"));
    assert!(harness.render().contains("Copied ABCD-1234"));
}

#[tokio::test]
async fn wizard_paste_code_step_sends_the_code_to_the_login_task() {
    let (mut harness, code) = login_wizard(
        0,
        0,
        Prompt::PasteCode {
            url: "https://claude.com/cai/oauth/authorize?code=true".to_owned(),
        },
    );
    insta::assert_snapshot!(harness.render());
    harness.press(KeyCode::Enter);
    assert!(harness.render().contains("Paste the code first"));
    update(&mut harness.app, Action::Paste("abc#def\n".to_owned()));
    harness.press(KeyCode::Enter);
    assert_eq!(code.await.unwrap(), "abc#def");
    assert!(harness.render().contains("Signing in…"));
}

#[tokio::test]
async fn wizard_loopback_step() {
    let (mut harness, _code) = login_wizard(
        1,
        0,
        Prompt::Loopback {
            url: "https://auth.openai.com/oauth/authorize?x=1".to_owned(),
            port: 1455,
            ssh_hint: "ssh -L 1455:localhost:1455 host".to_owned(),
        },
    );
    insta::assert_snapshot!(harness.render());
}

#[tokio::test]
async fn wizard_login_failure_offers_retry_and_back() {
    let (mut harness, _code) = login_wizard(
        1,
        1,
        Prompt::Device {
            verification_uri: "https://x".to_owned(),
            user_code: "A".to_owned(),
            expires_at: NOW + 60,
            interval: 5,
        },
    );
    update(
        &mut harness.app,
        Action::Job(Box::new(jobs::JobDone {
            id: jobs::JobId::for_test(7),
            work: jobs::Work::Login,
            result: Err(anyhow::anyhow!("the code expired")),
        })),
    );
    insta::assert_snapshot!(harness.render());
    harness.press(KeyCode::Esc);
    assert!(harness.render().contains("2 / 3"));
}
