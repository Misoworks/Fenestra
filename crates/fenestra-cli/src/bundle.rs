mod config;
mod metadata;
mod package;
mod stage;

use std::{
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
};

use config::{BundleApp, ConfigOverrides};
use package::package_bundle;
use stage::{binary_path, stage_bundle};

#[derive(Debug)]
pub struct BundleOptions {
    pub source: PathBuf,
    pub target: String,
    pub out: PathBuf,
    pub release: bool,
    pub no_build: bool,
    pub binary: Option<PathBuf>,
    pub no_web_build: bool,
    pub web_build: Option<String>,
    pub web_root: Option<PathBuf>,
    pub web_dist: Option<PathBuf>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub json: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BundleFormat {
    Linux,
    Portable,
    Deb,
    Rpm,
    AppImage,
    Windows,
    Exe,
    Msi,
    Macos,
    Dmg,
}

impl BundleFormat {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "linux" => Some(Self::Linux),
            "portable" | "tar" | "tar.gz" => Some(Self::Portable),
            "deb" => Some(Self::Deb),
            "rpm" => Some(Self::Rpm),
            "appimage" => Some(Self::AppImage),
            "windows" => Some(Self::Windows),
            "exe" | "nsis" => Some(Self::Exe),
            "msi" => Some(Self::Msi),
            "macos" | "app" => Some(Self::Macos),
            "dmg" => Some(Self::Dmg),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Portable => "portable",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::AppImage => "appimage",
            Self::Windows => "windows",
            Self::Exe => "exe",
            Self::Msi => "msi",
            Self::Macos => "macos",
            Self::Dmg => "dmg",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BuildTarget {
    Linux,
    Windows,
    Macos,
}

impl BuildTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Macos => "macos",
        }
    }

    pub(super) fn rust_target(self) -> Option<&'static str> {
        match self {
            Self::Linux if cfg!(target_os = "linux") => None,
            Self::Linux if cfg!(target_arch = "aarch64") => Some("aarch64-unknown-linux-gnu"),
            Self::Linux => Some("x86_64-unknown-linux-gnu"),
            Self::Windows => Some("x86_64-pc-windows-msvc"),
            Self::Macos if cfg!(target_arch = "aarch64") => Some("aarch64-apple-darwin"),
            Self::Macos => Some("x86_64-apple-darwin"),
        }
    }
}

pub fn bundle(options: BundleOptions) -> Result<ExitCode, String> {
    let Some(format) = BundleFormat::parse(&options.target) else {
        return Err("unknown bundle target; use linux, portable, deb, rpm, appimage, windows, exe, msi, macos, or dmg".to_string());
    };
    let app = config::resolve_app(
        &options.source,
        ConfigOverrides {
            id: options.id,
            name: options.name,
            version: options.version,
            web_build: options.web_build,
            web_root: options.web_root,
            web_dist: options.web_dist,
        },
    )?;

    if !options.no_web_build {
        build_web(&app)?;
    }
    if !options.no_build && options.binary.is_none() {
        build_rust(&app, format, options.release)?;
    }
    let binary = options
        .binary
        .map(absolute_path)
        .transpose()?
        .unwrap_or_else(|| binary_path(&app, format, options.release));
    if !binary.is_file() {
        return Err(format!(
            "built binary was not found at {}; pass --binary to package an existing executable or --no-build only when the default Cargo output already exists",
            binary.display()
        ));
    }

    let staged = stage_bundle(&app, format, &binary, &options.out, options.release)?;
    let packaged = package_bundle(&app, format, &staged)?;
    if options.json {
        println!("{}", bundle_json(&app, format, &staged, &packaged));
    } else {
        println!("Bundled {} to {}", app.name, staged.root.display());
        for artifact in &packaged.artifacts {
            println!("artifact: {}", artifact.display());
        }
        for note in &packaged.notes {
            println!("note: {note}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(super) fn build_target_for_format(format: BundleFormat) -> Option<BuildTarget> {
    match format {
        BundleFormat::Linux
        | BundleFormat::Portable
        | BundleFormat::Deb
        | BundleFormat::Rpm
        | BundleFormat::AppImage => Some(BuildTarget::Linux),
        BundleFormat::Windows | BundleFormat::Exe | BundleFormat::Msi => Some(BuildTarget::Windows),
        BundleFormat::Macos | BundleFormat::Dmg => Some(BuildTarget::Macos),
    }
}

fn build_web(app: &BundleApp) -> Result<(), String> {
    let Some(web) = &app.web else {
        return Ok(());
    };
    let Some(command) = &web.build_command else {
        return Ok(());
    };
    println!("Building web assets: {command}");
    let status = shell_command(command)
        .current_dir(&web.root)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run web build command `{command}`: {error}"))?;
    if !status.success() {
        return Err(format!("web build command failed: {command}"));
    }
    if !web.dist.exists() {
        return Err(format!(
            "web build completed but dist path does not exist: {}",
            web.dist.display()
        ));
    }
    Ok(())
}

fn build_rust(app: &BundleApp, format: BundleFormat, release: bool) -> Result<(), String> {
    let cargo_manifest = app.source_dir.join("Cargo.toml");
    if !cargo_manifest.is_file() {
        return Err(format!(
            "missing Cargo.toml at {}",
            cargo_manifest.display()
        ));
    }
    let target = build_target_for_format(format);
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(cargo_manifest);
    if release {
        command.arg("--release");
    }
    if let Some(rust_target) = target.and_then(BuildTarget::rust_target) {
        command.arg("--target").arg(rust_target);
    }
    command.env(
        "FENESTRA_BUILD_TARGET",
        target.map(BuildTarget::as_str).unwrap_or("native"),
    );
    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo build: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "cargo build failed for {}",
            target.map(BuildTarget::as_str).unwrap_or("native")
        ))
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut process = Command::new("cmd");
        process.args(["/C", command]);
        process
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut process = Command::new("sh");
        process.args(["-c", command]);
        process
    }
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path))
    }
}

fn bundle_json(
    app: &BundleApp,
    format: BundleFormat,
    staged: &stage::StagedBundle,
    packaged: &package::PackageResult,
) -> String {
    let artifacts = packaged
        .artifacts
        .iter()
        .map(|path| format!("\"{}\"", json(&path.display().to_string())))
        .collect::<Vec<_>>()
        .join(",");
    let notes = packaged
        .notes
        .iter()
        .map(|note| format!("\"{}\"", json(note)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"ok\":true,\"target\":\"{}\",\"app\":{{\"id\":\"{}\",\"name\":\"{}\",\"version\":\"{}\"}},\"path\":\"{}\",\"artifacts\":[{}],\"notes\":[{}]}}",
        format.as_str(),
        json(&app.id),
        json(&app.name),
        json(&app.version),
        json(&staged.root.display().to_string()),
        artifacts,
        notes
    )
}

fn json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
