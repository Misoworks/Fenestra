use std::process::ExitCode;

use fenestra_runtime::{
    RuntimeConfig, RuntimeEngine, RuntimePackage, detect_runtime, install_user_runtime,
    latest_install_plan, prune_user_runtimes, remove_user_runtime_version, resolve_runtime,
};

pub enum RuntimeCommand {
    List {
        json: bool,
    },
    Install {
        engine: String,
        package: String,
    },
    Remove {
        engine: String,
        version: Option<String>,
        package: String,
    },
    Prune {
        engine: String,
        keep: usize,
        package: String,
    },
    Doctor {
        json: bool,
    },
}

pub fn run_runtime(command: RuntimeCommand) -> ExitCode {
    match command {
        RuntimeCommand::List { json } => list_runtimes(json),
        RuntimeCommand::Install { engine, package } => install_runtime(&engine, &package),
        RuntimeCommand::Remove {
            engine,
            version,
            package,
        } => remove_runtime(&engine, version.as_deref(), &package),
        RuntimeCommand::Prune {
            engine,
            keep,
            package,
        } => prune_runtime(&engine, keep, &package),
        RuntimeCommand::Doctor { json } => doctor_runtime(json),
    }
}

fn list_runtimes(json: bool) -> ExitCode {
    let config = RuntimeConfig::default();
    let runtimes = detect_runtime(&config);

    if json {
        let entries = runtimes
            .iter()
            .map(|r| {
                let location_type = match &r.location {
                    fenestra_runtime::RuntimeLocation::System(_) => "system",
                    fenestra_runtime::RuntimeLocation::UserLocal(_) => "user",
                    fenestra_runtime::RuntimeLocation::Bundled(_) => "bundled",
                };
                format!(
                    "{{\"engine\":\"{}\",\"package\":\"{}\",\"version\":\"{}\",\"location_type\":\"{}\",\"path\":\"{}\"}}",
                    r.engine.id(),
                    r.package.as_str(),
                    r.version,
                    location_type,
                    r.location.path().display()
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        println!("{{\"runtimes\":[{entries}]}}");
    } else {
        if runtimes.is_empty() {
            println!("No CEF runtimes found.");
            println!("Run `fenestra runtime install cef` to install a user-local runtime.");
        } else {
            println!("CEF runtimes:");
            for runtime in &runtimes {
                let location_type = match &runtime.location {
                    fenestra_runtime::RuntimeLocation::System(_) => "system",
                    fenestra_runtime::RuntimeLocation::UserLocal(_) => "user",
                    fenestra_runtime::RuntimeLocation::Bundled(_) => "bundled",
                };
                println!(
                    "  {} {} {} {} {}",
                    runtime.version,
                    runtime.package.as_str(),
                    location_type,
                    runtime.engine.id(),
                    runtime.location.path().display()
                );
            }
        }
    }

    ExitCode::SUCCESS
}

fn install_runtime(engine: &str, package: &str) -> ExitCode {
    let Ok(config) = runtime_config(engine, package) else {
        return ExitCode::from(1);
    };
    if let Ok(runtime) = resolve_runtime(&config) {
        println!(
            "A compatible {engine} {package} runtime is already installed at {}.",
            runtime.location.path().display()
        );
        return ExitCode::SUCCESS;
    }

    match latest_install_plan(&config) {
        Ok(plan) => {
            println!(
                "Installing required {engine} {} runtime {}.",
                plan.package.as_str(),
                plan.version
            );
            println!("Download: {}", plan.url);
            println!("Destination: {}", plan.install_dir.display());
        }
        Err(error) => {
            eprintln!("failed to plan {engine} runtime install: {error}");
            return ExitCode::from(1);
        }
    }

    match install_user_runtime(&config) {
        Ok(runtime) => {
            println!(
                "Installed {engine} {} runtime {} at {}.",
                runtime.package.as_str(),
                runtime.version,
                runtime.location.path().display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to install {engine} runtime: {error}");
            ExitCode::from(1)
        }
    }
}

fn remove_runtime(engine: &str, version: Option<&str>, package: &str) -> ExitCode {
    let Ok(config) = runtime_config(engine, package) else {
        return ExitCode::from(1);
    };

    let Some(version) = version else {
        eprintln!("specify a version; run `fenestra runtime list` to see installed versions");
        return ExitCode::from(1);
    };

    match remove_user_runtime_version(&config, version) {
        Ok(true) => {
            println!("Removed {engine} {package} runtime {version}.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!("No user-local {engine} {package} runtime {version} found.");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("failed to remove {engine} {package} runtime {version}: {error}");
            ExitCode::from(1)
        }
    }
}

fn prune_runtime(engine: &str, keep: usize, package: &str) -> ExitCode {
    let Ok(config) = runtime_config(engine, package) else {
        return ExitCode::from(1);
    };

    match prune_user_runtimes(&config, keep) {
        Ok(0) => {
            println!("No stale {engine} {package} runtimes found.");
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!("Removed {removed} stale {engine} {package} runtime(s).");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to prune {engine} {package} runtimes: {error}");
            ExitCode::from(1)
        }
    }
}

fn doctor_runtime(json: bool) -> ExitCode {
    let config = RuntimeConfig::default();
    let runtimes = detect_runtime(&config);
    let resolved = resolve_runtime(&config).ok();
    let has_compatible = resolved.is_some();

    let status = if has_compatible {
        "ok"
    } else if runtimes.is_empty() {
        "missing"
    } else {
        "outdated"
    };

    if json {
        println!(
            "{{\"cef_status\":\"{status}\",\"runtimes\":[{}]}}",
            runtimes
                .iter()
                .map(|r| format!(
                    "{{\"version\":\"{}\",\"location\":\"{}\"}}",
                    r.version,
                    r.location.path().display()
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
    } else {
        match status {
            "ok" => println!("CEF runtime: ok"),
            "missing" => {
                println!("CEF runtime: not found");
                println!("  Install with: fenestra runtime install cef");
            }
            "outdated" => {
                println!("CEF runtime: outdated (found versions below minimum 126)");
                println!("  Update with: fenestra runtime install cef");
            }
            _ => {}
        }
    }

    if has_compatible {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn runtime_config(engine: &str, package: &str) -> Result<RuntimeConfig, ()> {
    let Some(engine) = RuntimeEngine::parse(engine) else {
        eprintln!("unknown engine `{engine}`; use cef");
        return Err(());
    };
    let Some(package) = RuntimePackage::parse(package) else {
        eprintln!("unknown runtime package `{package}`; use standard, client, or minimal");
        return Err(());
    };

    Ok(RuntimeConfig {
        engine,
        package,
        ..RuntimeConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_engine_parses_known_types() {
        assert!(RuntimeEngine::parse("cef").is_some());
        assert!(RuntimeEngine::parse("unknown").is_none());
    }
}
