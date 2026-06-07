use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug)]
pub(super) struct BundleApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub icon: Option<PathBuf>,
    pub mime_types: Vec<String>,
    pub cargo_manifest: PathBuf,
    pub source_dir: PathBuf,
    pub cargo_package: String,
    pub web: Option<WebBundle>,
}

#[derive(Debug)]
pub(super) struct WebBundle {
    pub root: PathBuf,
    pub dist: PathBuf,
    pub entry: PathBuf,
    pub build_command: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct ConfigOverrides {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub web_build: Option<String>,
    pub web_root: Option<PathBuf>,
    pub web_dist: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct FenestraFile {
    #[serde(default)]
    app: AppSection,
    #[serde(default)]
    web: WebSection,
}

#[derive(Debug, Default, Deserialize)]
struct AppSection {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    icon: Option<String>,
    #[serde(default)]
    mime_types: Vec<String>,
    cargo_manifest: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WebSection {
    root: Option<String>,
    dist: Option<String>,
    entry: Option<String>,
    build: Option<String>,
}

pub(super) fn resolve_app(source: &Path, overrides: ConfigOverrides) -> Result<BundleApp, String> {
    let source_dir = absolute_path(source)?;
    let fenestra = read_fenestra_file(&source_dir)?;
    let stuk = read_stuk_file(&source_dir)?;
    let cargo_manifest = fenestra
        .app
        .cargo_manifest
        .as_ref()
        .map(|path| source_dir.join(path))
        .unwrap_or_else(|| source_dir.join("Cargo.toml"));
    let cargo_package = cargo_package_name(&cargo_manifest)
        .ok_or_else(|| format!("missing package name in {}", cargo_manifest.display()))?;
    let web = resolve_web(&source_dir, &fenestra.web, &stuk, &overrides)?;

    let name = overrides
        .name
        .or(fenestra.app.name)
        .or_else(|| stuk.app_name.clone())
        .unwrap_or_else(|| cargo_package.replace('_', " "));
    let id = overrides
        .id
        .or(fenestra.app.id)
        .or_else(|| stuk.app_id.clone())
        .unwrap_or_else(|| format!("dev.fenestra.{}", sanitize_id(&name)));
    let version = overrides
        .version
        .or(fenestra.app.version)
        .or_else(|| stuk.app_version.clone())
        .unwrap_or_else(|| {
            cargo_package_version(&cargo_manifest).unwrap_or_else(|| "0.1.0".to_string())
        });
    let icon = fenestra
        .app
        .icon
        .or_else(|| stuk.icon.clone())
        .map(|icon| source_dir.join(icon))
        .or_else(|| detect_icon(&source_dir));

    Ok(BundleApp {
        id: sanitize_id(&id),
        name,
        version,
        icon,
        mime_types: fenestra.app.mime_types,
        cargo_manifest,
        source_dir,
        cargo_package,
        web,
    })
}

fn resolve_web(
    source_dir: &Path,
    config: &WebSection,
    stuk: &StukFile,
    overrides: &ConfigOverrides,
) -> Result<Option<WebBundle>, String> {
    let configured_root = overrides
        .web_root
        .clone()
        .or_else(|| config.root.as_ref().map(PathBuf::from));
    let package_root = configured_root
        .as_deref()
        .map(|root| source_dir.join(root))
        .or_else(|| detect_package_root(source_dir));
    let entry = config
        .entry
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| stuk.webview_entry.as_ref().map(PathBuf::from))
        .or_else(|| default_web_entry(source_dir));
    let dist = overrides
        .web_dist
        .clone()
        .or_else(|| config.dist.as_ref().map(PathBuf::from))
        .map(|path| source_dir.join(path));

    if package_root.is_none() && entry.is_none() && dist.is_none() {
        return Ok(None);
    }

    let root = package_root
        .or_else(|| {
            entry
                .as_ref()
                .and_then(|entry| source_dir.join(entry).parent().map(Path::to_path_buf))
        })
        .unwrap_or_else(|| source_dir.join("ui"));
    let entry = entry
        .map(|entry| source_dir.join(entry))
        .unwrap_or_else(|| root.join("index.html"));
    let dist = dist.unwrap_or_else(|| {
        if root.join("dist").exists() || root.join("package.json").exists() {
            root.join("dist")
        } else {
            entry.parent().unwrap_or(&root).to_path_buf()
        }
    });
    let build_command = overrides
        .web_build
        .clone()
        .or_else(|| config.build.clone())
        .or_else(|| detect_web_build_command(&root));

    Ok(Some(WebBundle {
        root,
        dist,
        entry,
        build_command,
    }))
}

fn read_fenestra_file(source_dir: &Path) -> Result<FenestraFile, String> {
    let path = source_dir.join("Fenestra.toml");
    if !path.exists() {
        return Ok(FenestraFile::default());
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    toml::from_str(&text).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

#[derive(Default)]
struct StukFile {
    app_id: Option<String>,
    app_name: Option<String>,
    app_version: Option<String>,
    icon: Option<String>,
    webview_entry: Option<String>,
}

fn read_stuk_file(source_dir: &Path) -> Result<StukFile, String> {
    let path = source_dir.join("Stuk.toml");
    if !path.exists() {
        return Ok(StukFile::default());
    }
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let value = text
        .parse::<toml::Table>()
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    let app = value.get("app").and_then(toml::Value::as_table);
    let webview = value.get("webview").and_then(toml::Value::as_table);
    Ok(StukFile {
        app_id: app.and_then(|app| string_value(app, "id")),
        app_name: app.and_then(|app| string_value(app, "name")),
        app_version: app.and_then(|app| string_value(app, "version")),
        icon: app.and_then(|app| string_value(app, "icon")),
        webview_entry: webview.and_then(|webview| string_value(webview, "entry")),
    })
}

fn string_value(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

fn detect_package_root(source_dir: &Path) -> Option<PathBuf> {
    ["ui", "frontend", "web", "."]
        .iter()
        .map(|candidate| source_dir.join(candidate))
        .find(|candidate| candidate.join("package.json").is_file())
}

fn default_web_entry(source_dir: &Path) -> Option<PathBuf> {
    [
        "ui/index.html",
        "web/index.html",
        "frontend/index.html",
        "index.html",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|entry| source_dir.join(entry).is_file())
}

fn detect_icon(source_dir: &Path) -> Option<PathBuf> {
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
    .map(|icon| source_dir.join(icon))
    .find(|icon| icon.is_file())
}

fn detect_web_build_command(root: &Path) -> Option<String> {
    let package_json = root.join("package.json");
    if !package_json.is_file() {
        return None;
    }
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

fn cargo_package_name(path: &Path) -> Option<String> {
    cargo_package_value(path, "name")
}

fn cargo_package_version(path: &Path) -> Option<String> {
    cargo_package_value(path, "version")
}

fn cargo_package_value(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && trimmed.starts_with(key) {
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

fn sanitize_id(value: &str) -> String {
    let output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "app".to_string()
    } else {
        output
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("source path does not exist: {}", path.display()))
    }
}
