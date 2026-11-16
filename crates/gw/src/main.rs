mod account;
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

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gw", version, about = "personal LLM gateway")]
struct Cli {
    #[arg(long, global = true, env = "GW_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "run the gateway server")]
    Serve {
        #[arg(long)]
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
    #[command(about = "open the management TUI")]
    Tui,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let home = home::resolve(cli.home);
    match cli.command {
        Command::Serve { listen } => serve::run(&home, &listen).await,
        Command::Account(command) => cli::account(&home, command).await,
        Command::Key(command) => cli::key(&home, command).await,
        Command::Model(command) => cli::model(&home, command).await,
        Command::Log { follow, filter } => cli::log(&home, follow, filter.as_deref()).await,
        Command::Tui => tui::run(&home).await,
    }
}
