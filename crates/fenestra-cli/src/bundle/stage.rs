use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{
    BundleFormat,
    config::BundleApp,
    metadata::{
        app_run, bundle_toml, desktop_entry, flatpak_manifest, info_plist, sanitize_path, web_toml,
        windows_manifest,
    },
};
use crate::{bundle::build_target_for_format, icon_assets};

#[derive(Debug)]
pub(super) struct StagedBundle {
    pub root: PathBuf,
    pub app_dir: PathBuf,
    pub executable: String,
}

pub(super) fn stage_bundle(
    app: &BundleApp,
    format: BundleFormat,
    binary: &Path,
    out: &Path,
    release: bool,
) -> Result<StagedBundle, String> {
    let executable = executable_name(app, format);
    let root = out.join(format.as_str()).join(sanitize_path(&app.id));
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let app_dir = match format {
        BundleFormat::Macos | BundleFormat::Dmg => {
            stage_macos(app, format, binary, &root, &executable)?
        }
        BundleFormat::Windows | BundleFormat::Msi | BundleFormat::Exe => {
            stage_windows(app, format, binary, &root, &executable)?
        }
        BundleFormat::AppImage => stage_appimage(app, format, binary, &root, &executable)?,
        BundleFormat::Linux | BundleFormat::Deb | BundleFormat::Rpm | BundleFormat::Portable => {
            stage_unix_root(app, format, binary, &root, &executable)?
        }
    };
    fs::write(
        root.join("fenestra-bundle.toml"),
        bundle_toml(app, format, &executable),
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        root.join("build-info.toml"),
        build_info(app, format, release),
    )
    .map_err(|error| error.to_string())?;
    Ok(StagedBundle {
        root,
        app_dir,
        executable,
    })
}

pub(super) fn binary_path(app: &BundleApp, format: BundleFormat, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    let file_name = executable_name(app, format);
    let rust_target = build_target_for_format(format).and_then(|target| target.rust_target());
    let mut candidates = app
        .source_dir
        .ancestors()
        .map(|ancestor| {
            let mut path = ancestor.join("target");
            if let Some(rust_target) = rust_target {
                path = path.join(rust_target);
            }
            path.join(profile).join(&file_name)
        })
        .collect::<Vec<_>>();
    if let Some(manifest_dir) = app.cargo_manifest.parent() {
        let mut path = manifest_dir.join("target");
        if let Some(rust_target) = rust_target {
            path = path.join(rust_target);
        }
        candidates.insert(0, path.join(profile).join(&file_name));
    }
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .unwrap_or_else(|| {
            candidates
                .into_iter()
                .next()
                .unwrap_or_else(|| app.source_dir.join("target").join(profile).join(file_name))
        })
}

fn stage_macos(
    app: &BundleApp,
    format: BundleFormat,
    binary: &Path,
    root: &Path,
    executable: &str,
) -> Result<PathBuf, String> {
    let app_dir = root.join(format!("{}.app", sanitize_path(&app.name)));
    let contents = app_dir.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).map_err(|error| error.to_string())?;
    fs::create_dir_all(&resources).map_err(|error| error.to_string())?;
    copy_binary(binary, &macos.join(executable))?;
    fs::write(contents.join("Info.plist"), info_plist(app, executable))
        .map_err(|error| error.to_string())?;
    stage_resources(app, format, &resources)?;
    Ok(app_dir)
}

fn stage_windows(
    app: &BundleApp,
    format: BundleFormat,
    binary: &Path,
    root: &Path,
    executable: &str,
) -> Result<PathBuf, String> {
    let app_dir = root.join(sanitize_path(&app.name));
    let resources = app_dir.join("resources");
    fs::create_dir_all(&resources).map_err(|error| error.to_string())?;
    copy_binary(binary, &app_dir.join(executable))?;
    fs::write(
        resources.join("windows-app-manifest.xml"),
        windows_manifest(app),
    )
    .map_err(|error| error.to_string())?;
    stage_resources(app, format, &resources)?;
    Ok(app_dir)
}

fn stage_appimage(
    app: &BundleApp,
    format: BundleFormat,
    binary: &Path,
    root: &Path,
    executable: &str,
) -> Result<PathBuf, String> {
    let app_dir = root.join("AppDir");
    let bin_dir = app_dir.join("usr/bin");
    fs::create_dir_all(&bin_dir).map_err(|error| error.to_string())?;
    copy_binary(binary, &bin_dir.join(executable))?;
    fs::write(app_dir.join("AppRun"), app_run(executable)).map_err(|error| error.to_string())?;
    make_executable(&app_dir.join("AppRun")).map_err(|error| error.to_string())?;
    let icon = stage_appimage_icon(app, &app_dir)?;
    fs::write(
        app_dir.join(format!("{}.desktop", app.id)),
        desktop_entry(app, executable, icon.as_deref()),
    )
    .map_err(|error| error.to_string())?;
    stage_resources(
        app,
        format,
        &app_dir.join("usr/share/fenestra").join(&app.id),
    )?;
    Ok(app_dir)
}

fn stage_unix_root(
    app: &BundleApp,
    format: BundleFormat,
    binary: &Path,
    root: &Path,
    executable: &str,
) -> Result<PathBuf, String> {
    let app_dir = root.join("root");
    let bin_dir = app_dir.join("usr/bin");
    let desktop_dir = app_dir.join("usr/share/applications");
    let resources = app_dir.join("usr/share/fenestra").join(&app.id);
    fs::create_dir_all(&bin_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&desktop_dir).map_err(|error| error.to_string())?;
    copy_binary(binary, &bin_dir.join(executable))?;
    fs::write(
        desktop_dir.join(format!("{}.desktop", app.id)),
        desktop_entry(app, executable, linux_icon_path(app, format).as_deref()),
    )
    .map_err(|error| error.to_string())?;
    if format == BundleFormat::Linux {
        fs::write(
            root.join(format!("{}.flatpak.json", app.id)),
            flatpak_manifest(app, executable),
        )
        .map_err(|error| error.to_string())?;
    }
    stage_resources(app, format, &resources)?;
    Ok(app_dir)
}

fn linux_icon_path(app: &BundleApp, format: BundleFormat) -> Option<String> {
    if !matches!(
        format,
        BundleFormat::Linux | BundleFormat::Deb | BundleFormat::Rpm
    ) {
        return None;
    }
    let icon = app.icon.as_ref().filter(|icon| icon.is_file())?;
    if icon
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return Some(format!(
            "/usr/share/fenestra/{}/icons/scalable/apps/{}.svg",
            app.id, app.id
        ));
    }
    Some(format!(
        "/usr/share/fenestra/{}/icons/512x512/apps/{}.png",
        app.id, app.id
    ))
}

fn stage_appimage_icon(app: &BundleApp, app_dir: &Path) -> Result<Option<String>, String> {
    let Some(icon) = app.icon.as_ref().filter(|icon| icon.is_file()) else {
        return Ok(None);
    };
    let Some(extension) = icon.extension() else {
        return Ok(None);
    };
    let extension = extension.to_string_lossy();
    fs::copy(icon, app_dir.join(format!("{}.{}", app.id, extension)))
        .map_err(|error| error.to_string())?;
    icon_assets::stage_icon_set(&app.id, icon, &app_dir.join("usr/share/icons/hicolor"))?;
    Ok(Some(app.id.clone()))
}

fn stage_resources(app: &BundleApp, format: BundleFormat, resources: &Path) -> Result<(), String> {
    fs::create_dir_all(resources).map_err(|error| error.to_string())?;
    fs::write(
        resources.join("bundle.toml"),
        bundle_toml(app, format, &executable_name(app, format)),
    )
    .map_err(|error| error.to_string())?;
    if let Some(web) = web_toml(app) {
        fs::write(resources.join("web.toml"), web).map_err(|error| error.to_string())?;
    }
    if let Some(icon) = &app.icon
        && icon.is_file()
    {
        let name = icon.file_name().unwrap_or_default().to_os_string();
        fs::copy(icon, resources.join(name)).map_err(|error| error.to_string())?;
        icon_assets::stage_icon_set(&app.id, icon, &resources.join("icons"))?;
    }
    if let Some(web) = &app.web
        && web.has_local_assets
    {
        let web_source = if web.dist.exists() {
            web.dist.as_path()
        } else {
            web.entry.parent().unwrap_or(&web.root)
        };
        if web_source.exists() {
            copy_dir_recursive(web_source, &resources.join("web"))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn build_info(app: &BundleApp, format: BundleFormat, release: bool) -> String {
    format!(
        "format = \"{}\"\nrelease = {}\nsource = \"{}\"\ncargo_package = \"{}\"\n",
        format.as_str(),
        release,
        app.source_dir.display(),
        app.cargo_package
    )
}

fn executable_name(app: &BundleApp, format: BundleFormat) -> String {
    if matches!(
        format,
        BundleFormat::Windows | BundleFormat::Msi | BundleFormat::Exe
    ) {
        format!("{}.exe", app.cargo_package)
    } else {
        app.cargo_package.clone()
    }
}

fn copy_binary(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, destination).map_err(|error| error.to_string())?;
    make_executable(destination).map_err(|error| error.to_string())
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
