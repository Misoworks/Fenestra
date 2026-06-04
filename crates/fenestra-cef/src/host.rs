use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const HOST_CMAKE: &str = include_str!("../host/linux/CMakeLists.txt");
const HOST_MAIN: &str = include_str!("../host/linux/main.cc");
const HOST_APP_H: &str = include_str!("../host/linux/app.h");
const HOST_APP_CC: &str = include_str!("../host/linux/app.cc");
const HOST_HANDLER_H: &str = include_str!("../host/linux/handler.h");
const HOST_HANDLER_CC: &str = include_str!("../host/linux/handler.cc");
const HOST_OSR_HANDLER_H: &str = include_str!("../host/linux/osr_handler.h");
const HOST_OSR_HANDLER_CC: &str = include_str!("../host/linux/osr_handler.cc");
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn ensure_cef_host(runtime_dir: &Path) -> Result<PathBuf, String> {
    let binary = runtime_dir.join("Release").join("fenestra-cef-host");
    let source_dir = runtime_dir.join(".fenestra-host-src");
    let build_dir = runtime_dir.join(".fenestra-host-build");
    let source_stamp = build_dir.join("fenestra-host-source.fnv");
    let expected_stamp = host_source_fingerprint();
    if binary.is_file()
        && std::fs::read_to_string(&source_stamp).is_ok_and(|stamp| stamp.trim() == expected_stamp)
    {
        return Ok(binary);
    }
    if !runtime_dir.join("include").is_dir()
        || !runtime_dir.join("libcef_dll").is_dir()
        || !runtime_dir.join("cmake").is_dir()
    {
        return Err(format!(
            "CEF runtime at {} is not a standard CEF distribution",
            runtime_dir.display()
        ));
    }

    std::fs::create_dir_all(&source_dir).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&build_dir).map_err(|error| error.to_string())?;
    write_host_source(&source_dir)?;

    let generator = if command_available("ninja") {
        "Ninja"
    } else {
        "Unix Makefiles"
    };
    run_checked(
        Command::new("cmake")
            .arg("-S")
            .arg(&source_dir)
            .arg("-B")
            .arg(&build_dir)
            .arg("-G")
            .arg(generator)
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg(format!("-DCEF_ROOT={}", runtime_dir.display())),
    )?;
    run_checked(
        Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .arg("--target")
            .arg("fenestra-cef-host")
            .arg("--parallel"),
    )?;

    if binary.is_file() {
        std::fs::write(source_stamp, expected_stamp).map_err(|error| error.to_string())?;
        Ok(binary)
    } else {
        Err(format!(
            "CEF host build did not create {}",
            binary.display()
        ))
    }
}

pub fn webview_cache_dir(title: &str, url: &str) -> PathBuf {
    user_cache_home()
        .join("fenestra")
        .join("webviews")
        .join(format!("{:016x}", stable_hash(&[title, url])))
        .join("instances")
        .join(instance_key())
}

pub fn ld_library_path(release_dir: &Path) -> String {
    let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    if existing.is_empty() {
        release_dir.display().to_string()
    } else {
        format!("{}:{existing}", release_dir.display())
    }
}

fn write_host_source(source_dir: &Path) -> Result<(), String> {
    for (name, body) in [
        ("CMakeLists.txt", HOST_CMAKE),
        ("main.cc", HOST_MAIN),
        ("app.h", HOST_APP_H),
        ("app.cc", HOST_APP_CC),
        ("handler.h", HOST_HANDLER_H),
        ("handler.cc", HOST_HANDLER_CC),
        ("osr_handler.h", HOST_OSR_HANDLER_H),
        ("osr_handler.cc", HOST_OSR_HANDLER_CC),
    ] {
        std::fs::write(source_dir.join(name), body).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn host_source_fingerprint() -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for body in [
        HOST_CMAKE,
        HOST_MAIN,
        HOST_APP_H,
        HOST_APP_CC,
        HOST_HANDLER_H,
        HOST_HANDLER_CC,
        HOST_OSR_HANDLER_H,
        HOST_OSR_HANDLER_CC,
    ] {
        for byte in body.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:016x}")
}

fn command_available(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

fn run_checked(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "command failed: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn user_cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
}

fn instance_key() -> String {
    let counter = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{counter}-{timestamp}", std::process::id())
}

fn stable_hash(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
