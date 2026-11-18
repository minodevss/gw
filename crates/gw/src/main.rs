mod account;
mod browser;
mod catalog;
mod cli;
mod clock;
mod codec;
mod forward;
mod home;
mod ops;
mod provider;
mod select;
mod serve;
mod session;
mod store;
mod tui;

use std::io::IsTerminal as _;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{CommandFactory as _, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "gw",
    version,
    about = "Personal LLM gateway. Run without a command to open the management TUI."
)]
struct Cli {
    #[arg(long, global = true, env = "GW_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "run the gateway server")]
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: String,
    },
    #[command(subcommand, about = "register and manage upstream accounts")]
    Account(cli::AccountCommand),
    #[command(subcommand, about = "issue and revoke client API keys")]
    Key(cli::KeyCommand),
    #[command(subcommand, about = "refresh and inspect the model catalog")]
    Model(cli::ModelCommand),
    #[command(about = "show the request log")]
    Log {
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        filter: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let home = home::resolve(cli.home);
    let Some(command) = cli.command else {
        if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
            Cli::command()
                .error(
                    ErrorKind::MissingSubcommand,
                    "no command given and not running in a terminal",
                )
                .exit();
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home::database(&home)?.with_file_name("tui.log"))?;
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(log))
            .init();
        return tui::run(&home).await;
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    match command {
        Command::Serve { listen } => serve::run(&home, &listen).await,
        Command::Account(command) => cli::account(&home, command).await,
        Command::Key(command) => cli::key(&home, command).await,
        Command::Model(command) => cli::model(&home, command).await,
        Command::Log { follow, filter } => cli::log(&home, follow, filter.as_deref()).await,
    }
}
