use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    BridgeHandlers, BridgeRuntime, WebViewConfig, WebViewError, WebViewProcess, WebViewResult,
    ld_library_path, prepare_bridge_command, spawn_bridge_dispatch, webview_cache_dir,
};

pub(crate) const OSR_HOST_ARG: &str = "--stuk-fenestra-osr-host";

pub(crate) fn run_from_args(args: &[String]) -> bool {
    let Some(index) = args.iter().position(|arg| arg == OSR_HOST_ARG) else {
        return false;
    };
    let Some(config_path) = args.get(index + 1).map(PathBuf::from) else {
        eprintln!("missing webview OSR host config path");
        std::process::exit(1);
    };
    if let Err(error) = crate::osr_host::run(config_path) {
        eprintln!("webview OSR host failed: {error}");
        std::process::exit(1);
    }
    true
}

pub(crate) fn launch_process(
    runtime_dir: &Path,
    config: &WebViewConfig,
    bridge_handlers: &BridgeHandlers,
    url: &str,
) -> WebViewResult<WebViewProcess> {
    #[cfg(target_os = "linux")]
    {
        let host_binary = fenestra_cef::ensure_cef_host(runtime_dir)
            .map_err(|message| WebViewError::CreationFailed { message })?;
        let host_config_path = std::env::temp_dir().join(format!(
            "stuk-fenestra-osr-{}.json",
            crate::webview_instance_key()
        ));
        let body = serde_json::json!({
            "runtime_dir": runtime_dir,
            "host_binary": host_binary,
            "url": url,
            "title": config.title,
            "width": config.width,
            "height": config.height,
            "min_width": config.min_width,
            "min_height": config.min_height,
            "resizable": config.resizable,
            "visible": config.visible,
            "active": config.active,
            "always_on_top": config.always_on_top,
            "transparent": config.transparent,
            "background_effect": config.background_effect.as_str(),
            "chrome": config.chrome.as_str(),
            "bridge_commands": config.bridge.commands(),
            "regions": crate::osr_protocol::regions_to_json(&config.regions),
        });
        std::fs::write(&host_config_path, body.to_string()).map_err(|error| {
            WebViewError::CreationFailed {
                message: format!("failed to write webview OSR host config: {error}"),
            }
        })?;

        let exe = std::env::current_exe().map_err(|error| WebViewError::CreationFailed {
            message: error.to_string(),
        })?;
        let mut command = Command::new(exe);
        command
            .arg(OSR_HOST_ARG)
            .arg(&host_config_path)
            .stderr(Stdio::inherit());
        prepare_bridge_command(&mut command, bridge_handlers);
        let mut child = command
            .spawn()
            .map_err(|error| WebViewError::CreationFailed {
                message: format!("failed to launch webview OSR host: {error}"),
            })?;
        let bridge_dispatch = spawn_bridge_dispatch(
            &mut child,
            BridgeRuntime::new(
                bridge_handlers.clone(),
                config.bridge.clone(),
                config.security.clone(),
            ),
        );
        Ok(WebViewProcess {
            cef: None,
            child: Some(child),
            bridge_thread: bridge_dispatch.thread,
            bridge_emitter: bridge_dispatch.emitter,
            desktop_services: None,
            desktop_event_thread: None,
            desktop_event_running: None,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (runtime_dir, config, bridge_handlers, url);
        Err(WebViewError::CreationFailed {
            message: "CEF OSR webview host is currently implemented for Linux".to_string(),
        })
    }
}

pub(crate) fn cef_osr_command(
    runtime_dir: &Path,
    host_binary: &Path,
    socket_path: &Path,
    config: &crate::osr_host::OsrHostConfig,
    width: u32,
    height: u32,
    scale: f64,
) -> Command {
    let release_dir = runtime_dir.join("Release");
    let cache_dir = webview_cache_dir(runtime_dir, &config.title, &config.url);
    let _ = std::fs::create_dir_all(&cache_dir);
    let mut command = Command::new(host_binary);
    command
        .arg(format!("--url={}", config.url))
        .arg("--fenestra-osr")
        .arg("--fenestra-ozone-platform=wayland")
        .arg(format!("--fenestra-osr-socket={}", socket_path.display()))
        .arg(format!("--fenestra-width={width}"))
        .arg(format!("--fenestra-height={height}"))
        .arg(format!("--fenestra-scale={scale:.4}"))
        .arg(format!(
            "--fenestra-bridge-commands={}",
            config.bridge_commands.join(",")
        ))
        .arg(format!("--root-cache-path={}", cache_dir.display()))
        .arg(format!(
            "--cache-path={}",
            cache_dir.join("browser").display()
        ))
        .arg("--ozone-platform=wayland")
        .arg("--enable-features=UseOzonePlatform")
        .arg("--disable-features=Vulkan,DefaultANGLEVulkan,VulkanFromANGLE")
        .arg("--disable-vulkan")
        .arg("--disable-gpu")
        .current_dir(&release_dir)
        .env("GDK_BACKEND", "wayland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("LD_LIBRARY_PATH", ld_library_path(&release_dir));
    if config.transparent {
        command
            .arg("--fenestra-transparent")
            .arg("--enable-transparent-visuals")
            .arg("--transparent-painting-enabled")
            .arg("--default-background-color=0x00000000");
    }
    if config.bridge_commands.is_empty() {
        command.stdin(Stdio::null()).stdout(Stdio::null());
    } else {
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
    }
    command
}
