use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Default)]
pub struct SourceMetadata {
    pub id: Option<String>,
    pub name: Option<String>,
    pub command: Option<String>,
    pub icon: Option<PathBuf>,
    pub mime_types: Vec<String>,
}

#[derive(Debug, Default)]
pub struct StagedAssets {
    pub web_dir: Option<PathBuf>,
    pub web_entry: Option<PathBuf>,
    pub icon: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct WebConfig {
    root: Option<PathBuf>,
    dist: Option<PathBuf>,
    entry: Option<PathBuf>,
    build: Option<String>,
    url: Option<String>,
    dev_url: Option<String>,
}

pub fn metadata(source: &Path) -> SourceMetadata {
    let fenestra = source.join("Fenestra.toml");
    let stuk = source.join("Stuk.toml");
    let mut metadata = SourceMetadata::default();
    if fenestra.exists() {
        merge_fenestra_metadata(source, &fenestra, &mut metadata);
    }
    if stuk.exists() {
        merge_stuk_metadata(source, &stuk, &mut metadata);
    }
    if metadata.icon.is_none() {
        metadata.icon = detect_icon(source);
    }
    metadata
}

pub fn stage(
    source: &Path,
    app_dir: &Path,
    configured_icon: Option<&Path>,
) -> Result<StagedAssets, String> {
    fs::create_dir_all(app_dir).map_err(|error| error.to_string())?;
    let web = stage_web(source, app_dir)?;
    let icon = stage_icon(source, app_dir, configured_icon)?;
    Ok(StagedAssets {
        web_dir: web
            .as_ref()
            .and_then(|entry| entry.parent().map(Path::to_path_buf)),
        web_entry: web,
        icon,
    })
}

fn stage_web(source: &Path, app_dir: &Path) -> Result<Option<PathBuf>, String> {
    let Some(config) = web_config(source) else {
        return Ok(None);
    };
    if let Some(command) = &config.build {
        println!("Building web assets: {command}");
        let status = shell_command(command)
            .current_dir(config.root.clone().unwrap_or_else(|| source.to_path_buf()))
            .stdin(Stdio::null())
            .status()
            .map_err(|error| format!("failed to run web build command `{command}`: {error}"))?;
        if !status.success() {
            return Err(format!("web build command failed: {command}"));
        }
    }

    let web_source = web_source_path(source, &config)?;
    let web_dir = app_dir.join("web");
    if web_dir.exists() {
        fs::remove_dir_all(&web_dir).map_err(|error| error.to_string())?;
    }
    copy_dir_recursive(&web_source, &web_dir).map_err(|error| error.to_string())?;
    let entry_name = config
        .entry
        .as_ref()
        .and_then(|entry| entry.file_name())
        .unwrap_or_default();
    let entry = if entry_name.is_empty() {
        web_dir.join("index.html")
    } else {
        web_dir.join(entry_name)
    };
    Ok(entry.is_file().then_some(entry))
}

fn stage_icon(
    source: &Path,
    app_dir: &Path,
    configured_icon: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let icon = configured_icon
        .map(Path::to_path_buf)
        .or_else(|| metadata(source).icon);
    let Some(icon) = icon.filter(|icon| icon.is_file()) else {
        return Ok(None);
    };
    let icons_dir = app_dir.join("icons");
    if icons_dir.exists() {
        fs::remove_dir_all(&icons_dir).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&icons_dir).map_err(|error| error.to_string())?;
    let destination = icons_dir.join(icon.file_name().unwrap_or_default());
    fs::copy(&icon, &destination).map_err(|error| error.to_string())?;
    Ok(Some(destination))
}

fn web_config(source: &Path) -> Option<WebConfig> {
    let mut config = WebConfig::default();
    read_web_config(source, &source.join("Fenestra.toml"), &mut config);
    read_web_config(source, &source.join("Stuk.toml"), &mut config);

    let has_remote_url = config.url.is_some() || config.dev_url.is_some();
    if config.root.is_none() && !has_remote_url {
        config.root = detect_package_root(source);
    }
    if config.entry.is_none() && !has_remote_url {
        config.entry = default_web_entry(source);
    }
    let has_web = config.root.is_some() || config.dist.is_some() || config.entry.is_some();
    if !has_web {
        return None;
    }
    let root = config.root.clone().unwrap_or_else(|| {
        config
            .entry
            .as_ref()
            .and_then(|entry| entry.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| source.to_path_buf())
    });
    if config.build.is_none() {
        config.build = detect_web_build_command(&root);
    }
    config.root = Some(root);
    Some(config)
}

fn web_source_path(source: &Path, config: &WebConfig) -> Result<PathBuf, String> {
    if let Some(dist) = &config.dist {
        if dist.exists() {
            return Ok(dist.clone());
        }
        return Err(format!("web dist path does not exist: {}", dist.display()));
    }
    let root = config.root.as_deref().unwrap_or(source);
    for candidate in [root.join("build"), root.join("dist"), root.join("public")] {
        if candidate.join("index.html").is_file() {
            return Ok(candidate);
        }
    }
    if let Some(entry) = &config.entry
        && entry.is_file()
        && let Some(parent) = entry.parent()
    {
        return Ok(parent.to_path_buf());
    }
    Err(format!(
        "could not find built web assets under {}",
        root.display()
    ))
}

fn read_web_config(source: &Path, path: &Path, config: &mut WebConfig) {
    let Ok(value) = read_toml(path) else {
        return;
    };
    if let Some(web) = value.get("web").and_then(toml::Value::as_table) {
        config.root = config
            .root
            .take()
            .or_else(|| string_value(web, "root").map(|path| source.join(path)));
        config.dist = config
            .dist
            .take()
            .or_else(|| string_value(web, "dist").map(|path| source.join(path)));
        config.entry = config
            .entry
            .take()
            .or_else(|| string_value(web, "entry").map(|path| source.join(path)));
        config.build = config.build.take().or_else(|| string_value(web, "build"));
        config.url = config.url.take().or_else(|| string_value(web, "url"));
        config.dev_url = config
            .dev_url
            .take()
            .or_else(|| string_value(web, "dev_url"));
    }
    if let Some(webview) = value.get("webview").and_then(toml::Value::as_table) {
        config.entry = config
            .entry
            .take()
            .or_else(|| string_value(webview, "entry").map(|path| source.join(path)));
    }
}

fn merge_fenestra_metadata(source: &Path, path: &Path, metadata: &mut SourceMetadata) {
    let Ok(value) = read_toml(path) else {
        return;
    };
    if let Some(install) = value.get("install").and_then(toml::Value::as_table) {
        metadata.command = metadata
            .command
            .take()
            .or_else(|| string_value(install, "command"));
    }
    if let Some(app) = value.get("app").and_then(toml::Value::as_table) {
        metadata.id = metadata.id.take().or_else(|| string_value(app, "id"));
        metadata.name = metadata.name.take().or_else(|| string_value(app, "name"));
        metadata.command = metadata
            .command
            .take()
            .or_else(|| string_value(app, "command"));
        metadata.icon = metadata
            .icon
            .take()
            .or_else(|| string_value(app, "icon").map(|path| source.join(path)));
        if metadata.mime_types.is_empty() {
            metadata.mime_types = string_array(app, "mime_types");
        }
    }
    if let Some(desktop) = value.get("desktop").and_then(toml::Value::as_table)
        && metadata.mime_types.is_empty()
    {
        metadata.mime_types = string_array(desktop, "mime_types");
    }
}

fn merge_stuk_metadata(source: &Path, path: &Path, metadata: &mut SourceMetadata) {
    merge_fenestra_metadata(source, path, metadata);
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    text.parse::<toml::Table>()
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn string_value(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

fn string_array(table: &toml::Table, key: &str) -> Vec<String> {
    table
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn detect_package_root(source: &Path) -> Option<PathBuf> {
    ["ui", "frontend", "web", "."]
        .iter()
        .map(|candidate| source.join(candidate))
        .find(|candidate| candidate.join("package.json").is_file())
}

fn default_web_entry(source: &Path) -> Option<PathBuf> {
    [
        "ui/index.html",
        "web/index.html",
        "frontend/index.html",
        "index.html",
    ]
    .iter()
    .map(|entry| source.join(entry))
    .find(|entry| entry.is_file())
}

fn detect_icon(source: &Path) -> Option<PathBuf> {
    [
        "static/icon.svg",
        "static/favicon.svg",
        "src/lib/assets/favicon.svg",
        "favicon.svg",
        "icon.svg",
        "icon.png",
        "icons/icon.svg",
        "icons/icon.png",
        "desktop/icons/icon.svg",
        "static/icon.png",
        "desktop/icons/icon.png",
    ]
    .iter()
    .map(|icon| source.join(icon))
    .find(|icon| icon.is_file())
}

fn detect_web_build_command(root: &Path) -> Option<String> {
    let package_json = root.join("package.json");
    let text = fs::read_to_string(package_json).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value
        .get("scripts")
        .and_then(|scripts| scripts.get("build"))
        .and_then(serde_json::Value::as_str)?;
    Some(format!("{} run build", package_manager(root, &value)))
}

fn package_manager(root: &Path, package: &serde_json::Value) -> &'static str {
    if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        return "bun";
    }
    if root.join("pnpm-lock.yaml").exists() {
        return "pnpm";
    }
    if root.join("yarn.lock").exists() {
        return "yarn";
    }
    if package
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.starts_with("bun@"))
    {
        return "bun";
    }
    if command_exists("bun") { "bun" } else { "npm" }
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
}

fn shell_command(command: &str) -> Command {
    let mut shell = if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.arg("/C");
        command
    } else {
        let mut command = Command::new("sh");
        command.arg("-c");
        command
    };
    shell.arg(command);
    shell
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}
