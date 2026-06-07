use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const ICON_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512, 1024];

pub fn install_user_icon(app_id: &str, icon: Option<&Path>) -> Result<Option<String>, String> {
    let Some(icon) = icon.filter(|icon| icon.is_file()) else {
        return Ok(None);
    };
    let hicolor = data_home()?.join("icons/hicolor");
    install_icon_set(app_id, icon, &hicolor)?;
    refresh_icon_cache(&hicolor);
    Ok(Some(app_id.to_string()))
}

pub fn stage_icon_set(app_id: &str, icon: &Path, destination: &Path) -> Result<(), String> {
    if !icon.is_file() {
        return Ok(());
    }
    install_icon_set(app_id, icon, destination)
}

fn install_icon_set(app_id: &str, icon: &Path, root: &Path) -> Result<(), String> {
    let extension = extension(icon)?;
    if extension.eq_ignore_ascii_case("svg") {
        copy_svg(app_id, icon, root)?;
        render_png_sizes(app_id, icon, root)?;
        return Ok(());
    }
    if extension.eq_ignore_ascii_case("png") {
        copy_resized_pngs(app_id, icon, root)?;
        return Ok(());
    }
    copy_named_icon(app_id, icon, root, &extension)
}

fn copy_svg(app_id: &str, icon: &Path, root: &Path) -> Result<(), String> {
    let destination = root.join("scalable/apps").join(format!("{app_id}.svg"));
    copy_file(icon, &destination)
}

fn render_png_sizes(app_id: &str, icon: &Path, root: &Path) -> Result<(), String> {
    if !command_exists("magick") {
        eprintln!(
            "warning: ImageMagick `magick` not found; SVG icon was installed without raster sizes"
        );
        return Ok(());
    }
    for size in ICON_SIZES {
        render_png(app_id, icon, root, *size)?;
    }
    Ok(())
}

fn copy_resized_pngs(app_id: &str, icon: &Path, root: &Path) -> Result<(), String> {
    if !command_exists("magick") {
        return copy_file(
            icon,
            &root.join("512x512/apps").join(format!("{app_id}.png")),
        );
    }
    for size in ICON_SIZES {
        render_png(app_id, icon, root, *size)?;
    }
    Ok(())
}

fn render_png(app_id: &str, icon: &Path, root: &Path, size: u32) -> Result<(), String> {
    let destination = root
        .join(format!("{size}x{size}/apps"))
        .join(format!("{app_id}.png"));
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let status = Command::new("magick")
        .arg(icon)
        .args(["-background", "none"])
        .args(["-resize", &format!("{size}x{size}")])
        .args(["-gravity", "center"])
        .args(["-extent", &format!("{size}x{size}")])
        .arg(&destination)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run ImageMagick for icon conversion: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "ImageMagick failed to convert icon {} to {}",
            icon.display(),
            destination.display()
        ))
    }
}

fn copy_named_icon(app_id: &str, icon: &Path, root: &Path, extension: &str) -> Result<(), String> {
    copy_file(
        icon,
        &root
            .join("512x512/apps")
            .join(format!("{app_id}.{extension}")),
    )
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(source, destination).map_err(|error| error.to_string())?;
    Ok(())
}

fn extension(path: &Path) -> Result<String, String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("icon path has no extension: {}", path.display()))
}

fn command_exists(name: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|path| {
            let candidate = path.join(name);
            candidate.is_file() && is_executable(&candidate)
        })
    })
}

fn refresh_icon_cache(root: &Path) {
    if !root.exists() {
        return;
    }
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-q", "-t"])
        .arg(root)
        .status();
}

fn data_home() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share"))
        .ok_or_else(|| "HOME is not set".to_string())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}
