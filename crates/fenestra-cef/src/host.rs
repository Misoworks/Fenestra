use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use fenestra_bridge::INSTALL_SCRIPT;

const HOST_CMAKE: &str = include_str!("../host/shared/CMakeLists.txt");
const HOST_MAIN: &str = include_str!("../host/shared/main.cc");
const HOST_APP_H: &str = include_str!("../host/shared/app.h");
const HOST_APP_CC: &str = include_str!("../host/shared/app.cc");
const HOST_HANDLER_H: &str = include_str!("../host/shared/handler.h");
const HOST_HANDLER_CC: &str = include_str!("../host/shared/handler.cc");
const HOST_OSR_HANDLER_H: &str = include_str!("../host/shared/osr_handler.h");
const HOST_OSR_HANDLER_CC: &str = include_str!("../host/shared/osr_handler.cc");
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);
const HOST_BUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(600);
const HOST_BUILD_LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

pub fn cef_host_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "fenestra-cef-host.exe"
    } else {
        "fenestra-cef-host"
    }
}

pub fn cef_host_release_binary(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("Release").join(cef_host_binary_name())
}

pub fn ensure_cef_host(runtime_dir: &Path) -> Result<PathBuf, String> {
    let binary = cef_host_release_binary(runtime_dir);
    let source_dir = runtime_dir.join(".fenestra-host-src");
    let build_dir = runtime_dir.join(".fenestra-host-build");
    let source_stamp = build_dir.join("fenestra-host-source.fnv");
    let expected_stamp = host_source_fingerprint();
    if binary.is_file()
        && std::fs::read_to_string(&source_stamp).is_ok_and(|stamp| stamp.trim() == expected_stamp)
    {
        return Ok(binary);
    }
    let _lock = HostBuildLock::acquire(runtime_dir)?;
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

    let generator = pick_cmake_generator();
    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&source_dir)
        .arg("-B")
        .arg(&build_dir);
    if !generator.is_empty() {
        configure.arg("-G").arg(generator);
    }
    configure
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg(format!("-DCEF_ROOT={}", runtime_dir.display()));
    run_checked(&mut configure)?;
    run_checked(
        Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .arg("--config")
            .arg("Release")
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

fn pick_cmake_generator() -> &'static str {
    if cfg!(target_os = "windows") {
        return "";
    }
    if command_available("ninja") {
        "Ninja"
    } else if cfg!(target_os = "macos") {
        "Unix Makefiles"
    } else {
        "Unix Makefiles"
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
    std::fs::write(source_dir.join("fenestra_bridge_js.h"), bridge_js_header())
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Generate the C++ header that embeds the canonical Fenestra bridge script
/// (kept in `crates/fenestra-bridge/src/web_bridge.js` and included as a
/// `&str` from `fenestra_bridge::INSTALL_SCRIPT`). The C++ host includes
/// this header and the C++ raw string literal
/// `FENESTRA_BRIDGE_JS_RAW` becomes a drop-in replacement for the
/// hand-maintained JS that previously lived in `handler.cc` and `app.cc`.
///
/// Keeping the C++ side generated means the JS body cannot drift from the
/// Rust `web_bridge.rs` install path.
fn bridge_js_header() -> String {
    let mut output = String::new();
    output.push_str(
        "// AUTO-GENERATED by fenestra-cef/src/host.rs from\n\
         // crates/fenestra-bridge/src/web_bridge.js. Do not edit by hand.\n\
         #pragma once\n\
         constexpr const char* FENESTRA_BRIDGE_JS_RAW = R\"js(",
    );
    // The content is embedded in a C++ raw string literal, so escape
    // sequences are NOT interpreted. We must NOT escape newlines/tabs/etc.
    // as `\\n`/`\\t` here, otherwise the generated JS source would contain
    // those as literal two-character sequences outside of any string and
    // V8 would fail to parse the script. The only sequence that actually
    // needs to be guarded against is the raw string terminator `)js"`.
    for byte in INSTALL_SCRIPT.as_bytes() {
        output.push(*byte as char);
    }
    output.push_str(")js\";\n");
    output
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
        INSTALL_SCRIPT,
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

struct HostBuildLock {
    path: PathBuf,
}

impl HostBuildLock {
    fn acquire(runtime_dir: &Path) -> Result<Self, String> {
        let path = runtime_dir.join(".fenestra-host-build.lock");
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    let _ = writeln!(file, "started={}", unix_timestamp_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= HOST_BUILD_LOCK_TIMEOUT {
                        return Err(format!(
                            "timed out waiting for Fenestra CEF host build lock at {}",
                            path.display()
                        ));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

impl Drop for HostBuildLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed >= HOST_BUILD_LOCK_STALE_AFTER)
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

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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
