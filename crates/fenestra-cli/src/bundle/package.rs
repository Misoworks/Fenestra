use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use super::{
    BundleFormat,
    config::BundleApp,
    metadata::{deb_control, nsis_script, rpm_spec, shell_script, wix_source},
    stage::StagedBundle,
};

#[derive(Default)]
pub(super) struct PackageResult {
    pub artifacts: Vec<PathBuf>,
    pub notes: Vec<String>,
}

pub(super) fn package_bundle(
    app: &BundleApp,
    format: BundleFormat,
    staged: &StagedBundle,
) -> Result<PackageResult, String> {
    let mut result = PackageResult::default();
    match format {
        BundleFormat::Portable
        | BundleFormat::Linux
        | BundleFormat::Windows
        | BundleFormat::Macos => {
            tar_gz(
                &staged.root,
                &artifact_path(app, staged, format, "tar.gz"),
                &mut result,
            )?;
        }
        BundleFormat::Deb => package_deb(app, staged, &mut result)?,
        BundleFormat::Rpm => package_rpm(app, staged, &mut result)?,
        BundleFormat::AppImage => package_appimage(app, staged, &mut result)?,
        BundleFormat::Dmg => package_dmg(app, staged, &mut result)?,
        BundleFormat::Msi => package_msi(app, staged, &mut result)?,
        BundleFormat::Exe => package_exe(app, staged, &mut result)?,
    }
    Ok(result)
}

fn package_deb(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let debian = staged.app_dir.join("DEBIAN");
    fs::create_dir_all(&debian).map_err(|error| error.to_string())?;
    fs::write(
        debian.join("control"),
        deb_control(app, dir_size_kb(&staged.app_dir)?),
    )
    .map_err(|error| error.to_string())?;
    let artifact = artifact_path(app, staged, BundleFormat::Deb, "deb");
    if command_exists("dpkg-deb") {
        ensure_parent(&artifact)?;
        run(Command::new("dpkg-deb")
            .arg("--build")
            .arg(&staged.app_dir)
            .arg(&artifact))?;
        result.artifacts.push(artifact);
    } else {
        write_script(
            &staged.root.join("build-deb.sh"),
            &shell_script(&[
                &mkdir_parent_line(&artifact),
                &format!(
                    "dpkg-deb --build '{}' '{}'",
                    staged.app_dir.display(),
                    artifact.display()
                ),
            ]),
        )?;
        result
            .notes
            .push("dpkg-deb not found; wrote build-deb.sh".to_string());
    }
    Ok(())
}

fn package_rpm(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let spec = staged.root.join(format!("{}.spec", app.id));
    fs::write(&spec, rpm_spec(app, &staged.executable)).map_err(|error| error.to_string())?;
    write_script(
        &staged.root.join("build-rpm.sh"),
        &shell_script(&[&format!(
            "rpmbuild -bb '{}' --buildroot '{}'",
            spec.display(),
            staged.app_dir.display()
        )]),
    )?;
    result
        .notes
        .push("wrote RPM spec and build-rpm.sh".to_string());
    Ok(())
}

fn package_appimage(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let artifact = artifact_path(app, staged, BundleFormat::AppImage, "AppImage");
    if command_exists("appimagetool") {
        ensure_parent(&artifact)?;
        run(Command::new("appimagetool")
            .arg(&staged.app_dir)
            .arg(&artifact))?;
        result.artifacts.push(artifact);
    } else {
        write_script(
            &staged.root.join("build-appimage.sh"),
            &shell_script(&[
                &mkdir_parent_line(&artifact),
                &format!(
                    "appimagetool '{}' '{}'",
                    staged.app_dir.display(),
                    artifact.display()
                ),
            ]),
        )?;
        result
            .notes
            .push("appimagetool not found; wrote build-appimage.sh".to_string());
    }
    Ok(())
}

fn package_dmg(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let artifact = artifact_path(app, staged, BundleFormat::Dmg, "dmg");
    if command_exists("hdiutil") {
        ensure_parent(&artifact)?;
        run(Command::new("hdiutil")
            .arg("create")
            .arg("-volname")
            .arg(&app.name)
            .arg("-srcfolder")
            .arg(&staged.app_dir)
            .arg("-ov")
            .arg("-format")
            .arg("UDZO")
            .arg(&artifact))?;
        result.artifacts.push(artifact);
    } else {
        tar_gz(
            &staged.root,
            &artifact_path(app, staged, BundleFormat::Dmg, "app.tar.gz"),
            result,
        )?;
        write_script(
            &staged.root.join("build-dmg.sh"),
            &shell_script(&[
                &mkdir_parent_line(&artifact),
                &format!(
                    "hdiutil create -volname '{}' -srcfolder '{}' -ov -format UDZO '{}'",
                    app.name,
                    staged.app_dir.display(),
                    artifact.display()
                ),
            ]),
        )?;
        result
            .notes
            .push("hdiutil not found; wrote build-dmg.sh and app tarball".to_string());
    }
    Ok(())
}

fn package_msi(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let wxs = staged.root.join("installer.wxs");
    let artifact = artifact_path(app, staged, BundleFormat::Msi, "msi");
    fs::write(
        &wxs,
        wix_source(
            app,
            &staged
                .app_dir
                .join(&staged.executable)
                .display()
                .to_string(),
        ),
    )
    .map_err(|error| error.to_string())?;
    if command_exists("wix") {
        ensure_parent(&artifact)?;
        run(Command::new("wix")
            .arg("build")
            .arg(&wxs)
            .arg("-o")
            .arg(&artifact))?;
        result.artifacts.push(artifact);
    } else {
        write_script(
            &staged.root.join("build-msi.sh"),
            &shell_script(&[
                &mkdir_parent_line(&artifact),
                &format!("wix build '{}' -o '{}'", wxs.display(), artifact.display()),
            ]),
        )?;
        result
            .notes
            .push("WiX not found; wrote installer.wxs and build-msi.sh".to_string());
    }
    Ok(())
}

fn package_exe(
    app: &BundleApp,
    staged: &StagedBundle,
    result: &mut PackageResult,
) -> Result<(), String> {
    let script = staged.root.join("installer.nsi");
    let artifact = artifact_path(app, staged, BundleFormat::Exe, "exe");
    fs::write(
        &script,
        nsis_script(
            app,
            &staged.app_dir.display().to_string(),
            &staged.executable,
            &artifact.display().to_string(),
        ),
    )
    .map_err(|error| error.to_string())?;
    if command_exists("makensis") {
        ensure_parent(&artifact)?;
        run(Command::new("makensis")
            .current_dir(&staged.root)
            .arg(&script))?;
        if artifact.is_file() {
            result.artifacts.push(artifact);
        } else {
            result.notes.push(
                "ran makensis; setup exe was not found at the expected artifact path".to_string(),
            );
        }
    } else {
        write_script(
            &staged.root.join("build-exe.sh"),
            &shell_script(&[
                &mkdir_parent_line(&artifact),
                &format!("makensis '{}'", script.display()),
            ]),
        )?;
        result
            .notes
            .push("makensis not found; wrote installer.nsi and build-exe.sh".to_string());
    }
    Ok(())
}

fn tar_gz(source: &Path, artifact: &Path, result: &mut PackageResult) -> Result<(), String> {
    if command_exists("tar") {
        if let Some(parent) = artifact.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let source_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("invalid artifact source {}", source.display()))?;
        let parent = source.parent().unwrap_or_else(|| Path::new("."));
        run(Command::new("tar")
            .arg("-C")
            .arg(parent)
            .arg("-czf")
            .arg(artifact)
            .arg(source_name))?;
        result.artifacts.push(artifact.to_path_buf());
    } else {
        result
            .notes
            .push("tar not found; staged directory only".to_string());
    }
    Ok(())
}

fn artifact_path(
    app: &BundleApp,
    staged: &StagedBundle,
    format: BundleFormat,
    extension: &str,
) -> PathBuf {
    staged
        .root
        .parent()
        .unwrap_or(&staged.root)
        .join("artifacts")
        .join(format!(
            "{}-{}-{}.{}",
            app.id,
            app.version,
            format.as_str(),
            extension
        ))
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn mkdir_parent_line(path: &Path) -> String {
    path.parent()
        .map(|parent| format!("mkdir -p '{}'", parent.display()))
        .unwrap_or_else(|| "true".to_string())
}

fn dir_size_kb(path: &Path) -> Result<u64, String> {
    fn walk(path: &Path, total: &mut u64) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                walk(&entry.path(), total)?;
            } else {
                *total += metadata.len();
            }
        }
        Ok(())
    }
    let mut bytes = 0;
    walk(path, &mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.div_ceil(1024))
}

fn write_script(path: &Path, script: &str) -> Result<(), String> {
    fs::write(path, script).map_err(|error| error.to_string())?;
    make_executable(path).map_err(|error| error.to_string())
}

fn run(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("failed to run packaging tool: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("packaging tool failed".to_string())
    }
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|path| path.join(name).is_file()))
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
