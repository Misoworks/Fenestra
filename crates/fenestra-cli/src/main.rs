mod bundle;
mod icon_assets;
mod runtime;
mod source_assets;
mod source_desktop;
mod source_install;
mod template;

use std::{path::PathBuf, process::ExitCode};

use bundle::BundleOptions;
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
    Bundle {
        #[arg(default_value = ".")]
        source: PathBuf,
        #[arg(long, default_value = "linux")]
        target: String,
        #[arg(long, default_value = "dist")]
        out: PathBuf,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        no_build: bool,
        #[arg(long)]
        binary: Option<PathBuf>,
        #[arg(long)]
        no_web_build: bool,
        #[arg(long)]
        web_build: Option<String>,
        #[arg(long)]
        web_root: Option<PathBuf>,
        #[arg(long)]
        web_dist: Option<PathBuf>,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        json: bool,
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
        #[arg(long, default_value = "standard")]
        package: String,
    },
    Remove {
        engine: String,
        version: Option<String>,
        #[arg(long, default_value = "standard")]
        package: String,
    },
    Prune {
        engine: String,
        #[arg(long, default_value_t = 2)]
        keep: usize,
        #[arg(long, default_value = "standard")]
        package: String,
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
            RuntimeSubcommand::Install { engine, package } => {
                RuntimeCommand::Install { engine, package }
            }
            RuntimeSubcommand::Remove {
                engine,
                version,
                package,
            } => RuntimeCommand::Remove {
                engine,
                version,
                package,
            },
            RuntimeSubcommand::Prune {
                engine,
                keep,
                package,
            } => RuntimeCommand::Prune {
                engine,
                keep,
                package,
            },
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
        Command::Bundle {
            source,
            target,
            out,
            release,
            no_build,
            binary,
            no_web_build,
            web_build,
            web_root,
            web_dist,
            id,
            name,
            version,
            json,
        } => match bundle::bundle(BundleOptions {
            source,
            target,
            out,
            release,
            no_build,
            binary,
            no_web_build,
            web_build,
            web_root,
            web_dist,
            id,
            name,
            version,
            json,
        }) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(1)
            }
        },
    }
}
