use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use crate::{
    icon_assets,
    source_assets::{self, StagedAssets},
    source_desktop,
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
pub struct SourceApp {
    pub id: String,
    pub name: String,
    pub source: PathBuf,
    pub command: Option<String>,
    pub icon: Option<PathBuf>,
    pub mime_types: Vec<String>,
    pub autostart: bool,
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

    let app = match options.target {
        Some(target) if Path::new(&target).exists() => {
            detect_source_app(Path::new(&target), None, None, None, false)?
        }
        Some(target) => read_registered_app(&target)?,
        None => detect_source_app(Path::new("."), None, None, None, false)?,
    };
    update_registered_app(&app)?;
    Ok(ExitCode::SUCCESS)
}

fn update_registered_app(app: &SourceApp) -> Result<(), String> {
    let app = detect_source_app(&app.source, Some(app.id.clone()), None, None, app.autostart)?;
    register_app(&app, true)?;
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
    let metadata = source_assets::metadata(&source);
    let package_name = package_name(&source.join("Cargo.toml"));
    let name = name
        .or(metadata.name)
        .or_else(|| package_name.clone())
        .ok_or_else(|| {
            "source app needs --name, app metadata, or Cargo.toml package metadata".to_string()
        })?;
    let id = id
        .or(metadata.id)
        .unwrap_or_else(|| format!("dev.fenestra.{}", sanitize_id(&name)));

    Ok(SourceApp {
        id: sanitize_id(&id),
        name,
        source,
        command: command.or(metadata.command),
        icon: metadata.icon,
        mime_types: metadata.mime_types,
        autostart,
    })
}

fn register_app(app: &SourceApp, desktop: bool) -> Result<(), String> {
    let app_dir = app_dir(&app.id)?;
    fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
    let assets = source_assets::stage(&app.source, &app_dir, app.icon.as_deref())?;
    let desktop_icon = icon_assets::install_user_icon(&app.id, assets.icon.as_deref())?;
    let wrapper = app_dir.join("launch.sh");
    fs::write(&wrapper, launcher_script(app, &app_dir, &assets))
        .map_err(|error| error.to_string())?;
    make_executable(&wrapper).map_err(|error| error.to_string())?;
    fs::write(
        app_dir.join("source-install.toml"),
        registry_record(app, &wrapper, &assets),
    )
    .map_err(|error| error.to_string())?;

    if desktop {
        let desktop_dir = applications_dir()?;
        fs::create_dir_all(&desktop_dir).map_err(|error| error.to_string())?;
        fs::write(
            desktop_dir.join(format!("{}.desktop", app.id)),
            source_desktop::entry(app, &wrapper, desktop_icon.as_deref()),
        )
        .map_err(|error| error.to_string())?;
        source_desktop::refresh_database(&desktop_dir);
    }
    if app.autostart {
        let autostart_dir = autostart_dir()?;
        fs::create_dir_all(&autostart_dir).map_err(|error| error.to_string())?;
        fs::write(
            autostart_dir.join(format!("{}.desktop", app.id)),
            source_desktop::entry(app, &wrapper, desktop_icon.as_deref()),
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
    let mime_types = registry_value(&text, "mime_types")
        .map(|value| {
            value
                .split(';')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let autostart = registry_value(&text, "autostart")
        .map(|value| value == "true")
        .unwrap_or(false);
    Ok(SourceApp {
        id,
        name,
        source,
        command,
        icon,
        mime_types,
        autostart,
    })
}

fn launcher_script(app: &SourceApp, app_dir: &Path, assets: &StagedAssets) -> String {
    let source = shell_quote(&app.source.display().to_string());
    let mut exports = vec![
        format!("export FENESTRA_APP_ID={}", shell_quote(&app.id)),
        format!(
            "export FENESTRA_APP_DIR={}",
            shell_quote(&app_dir.display().to_string())
        ),
        format!(
            "export FENESTRA_SOURCE_DIR={}",
            shell_quote(&app.source.display().to_string())
        ),
    ];
    if let Some(web_dir) = &assets.web_dir {
        exports.push(format!(
            "export FENESTRA_WEB_DIR={}",
            shell_quote(&web_dir.display().to_string())
        ));
    }
    if let Some(web_entry) = &assets.web_entry {
        exports.push(format!(
            "export FENESTRA_WEB_ENTRY={}",
            shell_quote(&web_entry.display().to_string())
        ));
    }
    let exports = exports.join("\n");
    match &app.command {
        Some(command) => format!(
            "#!/bin/sh\nset -e\n{exports}\ncd {source}\nexec sh -c {} sh \"$@\"\n",
            shell_quote(&format!("{command} \"$@\""))
        ),
        None => format!(
            "#!/bin/sh\nset -e\n{exports}\ncd {source}\nexec cargo run --manifest-path {} -- \"$@\"\n",
            shell_quote(&app.source.join("Cargo.toml").display().to_string())
        ),
    }
}

fn registry_record(app: &SourceApp, wrapper: &Path, assets: &StagedAssets) -> String {
    let command = app.command.clone().unwrap_or_default();
    let icon = app
        .icon
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let staged_icon = assets
        .icon
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let web_dir = assets
        .web_dir
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let web_entry = assets
        .web_entry
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mime_types = app.mime_types.join(";");
    format!(
        "id = \"{}\"\nname = \"{}\"\nsource = \"{}\"\ncommand = \"{}\"\nwrapper = \"{}\"\nicon = \"{}\"\nstaged_icon = \"{}\"\nweb_dir = \"{}\"\nweb_entry = \"{}\"\nmime_types = \"{}\"\nautostart = \"{}\"\n",
        quote_value(&app.id),
        quote_value(&app.name),
        quote_value(&app.source.display().to_string()),
        quote_value(&command),
        quote_value(&wrapper.display().to_string()),
        quote_value(&icon),
        quote_value(&staged_icon),
        quote_value(&web_dir),
        quote_value(&web_entry),
        quote_value(&mime_types),
        app.autostart
    )
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
        path.canonicalize().map_err(|error| error.to_string())
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
