use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
};

#[derive(Debug)]
pub struct InstallOptions {
    pub source: PathBuf,
    pub id: Option<String>,
    pub name: Option<String>,
    pub command: Option<String>,
    pub desktop: bool,
    pub autostart: bool,
}

#[derive(Debug)]
pub struct UpdateOptions {
    pub target: Option<String>,
    pub all: bool,
}

#[derive(Clone, Debug)]
struct SourceApp {
    id: String,
    name: String,
    source: PathBuf,
    command: Option<String>,
    icon: Option<PathBuf>,
    autostart: bool,
}

pub fn install(options: InstallOptions) -> Result<ExitCode, String> {
    let app = detect_source_app(
        &options.source,
        options.id,
        options.name,
        options.command,
        options.autostart,
    )?;
    register_app(&app, options.desktop)?;
    println!("installed {} from {}", app.name, app.source.display());
    Ok(ExitCode::SUCCESS)
}

pub fn update(options: UpdateOptions) -> Result<ExitCode, String> {
    if options.all {
        let apps = registered_apps()?;
        if apps.is_empty() {
            println!("no source installs are registered");
            return Ok(ExitCode::SUCCESS);
        }
        for app in apps {
            update_registered_app(&app)?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let Some(target) = options.target else {
        return Err(
            "usage: fenestra update <id-or-source-path> or fenestra update --all".to_string(),
        );
    };
    let app = if Path::new(&target).exists() {
        detect_source_app(Path::new(&target), None, None, None, false)?
    } else {
        read_registered_app(&target)?
    };
    update_registered_app(&app)?;
    Ok(ExitCode::SUCCESS)
}

fn update_registered_app(app: &SourceApp) -> Result<(), String> {
    pull_source_if_git(&app.source)?;
    register_app(app, true)?;
    println!("updated {} from {}", app.name, app.source.display());
    Ok(())
}

fn detect_source_app(
    source: &Path,
    id: Option<String>,
    name: Option<String>,
    command: Option<String>,
    autostart: bool,
) -> Result<SourceApp, String> {
    let source = absolute_path(source)?;
    let manifest = source.join("Stuk.toml");
    let (manifest_id, manifest_name, manifest_icon) = if manifest.exists() {
        read_app_section(&manifest, &source)
    } else {
        (None, None, None)
    };
    let package_name = package_name(&source.join("Cargo.toml"));
    let name = name
        .or(manifest_name)
        .or_else(|| package_name.clone())
        .ok_or_else(|| "source app needs Cargo.toml package metadata".to_string())?;
    let id = id
        .or(manifest_id)
        .unwrap_or_else(|| format!("dev.fenestra.{}", sanitize_id(&name)));

    Ok(SourceApp {
        id: sanitize_id(&id),
        name,
        source,
        command,
        icon: manifest_icon,
        autostart,
    })
}

fn register_app(app: &SourceApp, desktop: bool) -> Result<(), String> {
    let app_dir = app_dir(&app.id)?;
    fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
    let wrapper = app_dir.join("launch.sh");
    fs::write(&wrapper, launcher_script(app)).map_err(|error| error.to_string())?;
    make_executable(&wrapper).map_err(|error| error.to_string())?;
    fs::write(
        app_dir.join("source-install.toml"),
        registry_record(app, &wrapper),
    )
    .map_err(|error| error.to_string())?;

    if desktop {
        let desktop_dir = applications_dir()?;
        fs::create_dir_all(&desktop_dir).map_err(|error| error.to_string())?;
        fs::write(
            desktop_dir.join(format!("{}.desktop", app.id)),
            desktop_entry(app, &wrapper),
        )
        .map_err(|error| error.to_string())?;
    }
    if app.autostart {
        let autostart_dir = autostart_dir()?;
        fs::create_dir_all(&autostart_dir).map_err(|error| error.to_string())?;
        fs::write(
            autostart_dir.join(format!("{}.desktop", app.id)),
            desktop_entry(app, &wrapper),
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn registered_apps() -> Result<Vec<SourceApp>, String> {
    let root = apps_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut apps = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let record = entry.path().join("source-install.toml");
        if record.exists() {
            apps.push(read_registry_record(&record)?);
        }
    }
    Ok(apps)
}

fn read_registered_app(id: &str) -> Result<SourceApp, String> {
    read_registry_record(&app_dir(&sanitize_id(id))?.join("source-install.toml"))
}

fn read_registry_record(path: &Path) -> Result<SourceApp, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let id =
        registry_value(&text, "id").ok_or_else(|| "registry record is missing id".to_string())?;
    let name = registry_value(&text, "name")
        .ok_or_else(|| "registry record is missing name".to_string())?;
    let source = registry_value(&text, "source")
        .map(PathBuf::from)
        .ok_or_else(|| "registry record is missing source".to_string())?;
    let command = registry_value(&text, "command").filter(|value| !value.is_empty());
    let icon = registry_value(&text, "icon")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let autostart = registry_value(&text, "autostart")
        .map(|value| value == "true")
        .unwrap_or(false);
    Ok(SourceApp {
        id,
        name,
        source,
        command,
        icon,
        autostart,
    })
}

fn pull_source_if_git(source: &Path) -> Result<(), String> {
    let Ok(output) = ProcessCommand::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "--show-toplevel"])
        .output()
    else {
        return Ok(());
    };
    if !output.status.success() {
        return Ok(());
    }
    let git_root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if git_root.is_empty() {
        return Ok(());
    }
    let status = ProcessCommand::new("git")
        .arg("-C")
        .arg(&git_root)
        .args(["pull", "--ff-only"])
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git pull --ff-only failed in {git_root}"))
    }
}

fn launcher_script(app: &SourceApp) -> String {
    let source = shell_quote(&app.source.display().to_string());
    match &app.command {
        Some(command) => format!(
            "#!/bin/sh\nset -e\ncd {source}\nexec sh -c {} sh \"$@\"\n",
            shell_quote(&format!("{command} \"$@\""))
        ),
        None => format!(
            "#!/bin/sh\nset -e\ncd {source}\nexec cargo run --manifest-path {} -- \"$@\"\n",
            shell_quote(&app.source.join("Cargo.toml").display().to_string())
        ),
    }
}

fn registry_record(app: &SourceApp, wrapper: &Path) -> String {
    let command = app.command.clone().unwrap_or_default();
    let icon = app
        .icon
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    format!(
        "id = \"{}\"\nname = \"{}\"\nsource = \"{}\"\ncommand = \"{}\"\nwrapper = \"{}\"\nicon = \"{}\"\nautostart = \"{}\"\n",
        quote_value(&app.id),
        quote_value(&app.name),
        quote_value(&app.source.display().to_string()),
        quote_value(&command),
        quote_value(&wrapper.display().to_string()),
        quote_value(&icon),
        app.autostart
    )
}

fn desktop_entry(app: &SourceApp, wrapper: &Path) -> String {
    let icon = app
        .icon
        .as_ref()
        .filter(|path| path.exists())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| app.id.clone());
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={} %U\nIcon={}\nTerminal=false\nCategories=Development;Utility;\nStartupNotify=true\n",
        desktop_value(&app.name),
        desktop_exec(wrapper),
        desktop_value(&icon)
    )
}

fn read_app_section(
    path: &Path,
    source: &Path,
) -> (Option<String>, Option<String>, Option<PathBuf>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (None, None, None);
    };
    let mut in_app = false;
    let mut id = None;
    let mut name = None;
    let mut icon = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_app = trimmed == "[app]";
            continue;
        }
        if !in_app {
            continue;
        }
        if trimmed.starts_with("id") {
            id = toml_string_value(trimmed);
        } else if trimmed.starts_with("name") {
            name = toml_string_value(trimmed);
        } else if trimmed.starts_with("icon") {
            icon = toml_string_value(trimmed).map(|path| source.join(path));
        }
    }
    (id, name, icon)
}

fn package_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && trimmed.starts_with("name") {
            return toml_string_value(trimmed);
        }
    }
    None
}

fn toml_string_value(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(ToOwned::to_owned)
}

fn registry_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((line_key, _value)) = trimmed.split_once('=') else {
            continue;
        };
        if line_key.trim() == key {
            return toml_string_value(trimmed).map(unquote_value);
        }
    }
    None
}

fn unquote_value(value: String) -> String {
    let mut output = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            output.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }
    output
}

fn quote_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn desktop_value(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

fn desktop_exec(path: &Path) -> String {
    path.display().to_string().replace(' ', "\\ ")
}

fn sanitize_id(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push('-');
        }
    }
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "app".to_string()
    } else {
        output
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| error.to_string())
            .map(|cwd| cwd.join(path))?
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("source path does not exist: {}", path.display()))
    }
}

fn apps_root() -> Result<PathBuf, String> {
    Ok(data_home()?.join("fenestra/apps"))
}

fn app_dir(id: &str) -> Result<PathBuf, String> {
    Ok(apps_root()?.join(id))
}

fn applications_dir() -> Result<PathBuf, String> {
    Ok(data_home()?.join("applications"))
}

fn autostart_dir() -> Result<PathBuf, String> {
    Ok(config_home()?.join("autostart"))
}

fn data_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    home_dir()
        .map(|home| home.join(".local/share"))
        .ok_or_else(|| "HOME is not set".to_string())
}

fn config_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    home_dir()
        .map(|home| home.join(".config"))
        .ok_or_else(|| "HOME is not set".to_string())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
