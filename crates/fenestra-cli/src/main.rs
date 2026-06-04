mod runtime;
mod source_install;
mod template;

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use runtime::RuntimeCommand;
use source_install::{InstallOptions, UpdateOptions};

#[derive(Debug, Parser)]
#[command(name = "fenestra", version, about = "Fenestra web runtime tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    New {
        name: String,
        #[arg(long, default_value = "notes")]
        template: String,
    },
    Runtime {
        #[command(subcommand)]
        command: RuntimeSubcommand,
    },
    Install {
        #[arg(default_value = ".")]
        source: PathBuf,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        autostart: bool,
        #[arg(long)]
        no_desktop: bool,
    },
    Update {
        target: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RuntimeSubcommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Install {
        engine: String,
    },
    Remove {
        engine: String,
        version: Option<String>,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::New { name, template } => template::new_app(&name, &template),
        Command::Runtime { command } => runtime::run_runtime(match command {
            RuntimeSubcommand::List { json } => RuntimeCommand::List { json },
            RuntimeSubcommand::Install { engine } => RuntimeCommand::Install { engine },
            RuntimeSubcommand::Remove { engine, version } => {
                RuntimeCommand::Remove { engine, version }
            }
            RuntimeSubcommand::Doctor { json } => RuntimeCommand::Doctor { json },
        }),
        Command::Install {
            source,
            id,
            name,
            command,
            autostart,
            no_desktop,
        } => match source_install::install(InstallOptions {
            source,
            id,
            name,
            command,
            autostart,
            desktop: !no_desktop,
        }) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
        Command::Update { target, all } => {
            match source_install::update(UpdateOptions { target, all }) {
                Ok(code) => code,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(1)
                }
            }
        }
    }
}
