use std::path::PathBuf;

use fenestra_runtime::{RuntimeConfig, RuntimeMode, detect_runtime, user_runtime_path};
use stuk_fenestra::{
    BridgeCommandDescriptor, BridgeResponse, WebViewSecurity, WebViewWindow, WindowChrome,
    WindowRegion, run_installing_window_from_args, run_native_host_from_args,
};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if run_installing_window_from_args(&args) {
        return;
    }
    if run_native_host_from_args(&args) {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = RuntimeConfig {
        mode: RuntimeMode::SharedPreferred,
        allow_user_install: true,
        bundled_dir: Some(manifest_dir.clone()),
        ..RuntimeConfig::default()
    };
    let window = WebViewWindow::new()
        .title("Notes")
        .entry(manifest_dir.join("ui/index.html").display().to_string())
        .chrome(WindowChrome::Stuk)
        .glass()
        .blur_region(WindowRegion::adaptive_titlebar_sidebar(260, 38, 14))
        .opaque_region(WindowRegion::adaptive_content_after_sidebar(260, 38))
        .input_region(WindowRegion::adaptive_rounded_rect(14))
        .security(WebViewSecurity::default())
        .runtime(runtime.clone())
        .bridge_descriptor_handler(
            BridgeCommandDescriptor::new("notes.list").target("desktop"),
            |_| {
                Ok(BridgeResponse::json(serde_json::json!([
                    { "id": "product-notes", "title": "Product notes" },
                    { "id": "runtime-checklist", "title": "Runtime checklist" },
                    { "id": "design-pass", "title": "Design pass" }
                ])))
            },
        )
        .bridge_descriptor_handler(
            BridgeCommandDescriptor::new("notes.create").target("desktop"),
            |command| {
                let id = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                Ok(BridgeResponse::json(serde_json::json!({
                    "ok": true,
                    "id": format!("native-{id}"),
                    "params": command.params
                })))
            },
        );

    println!("Stuk + Fenestra notes example");
    println!("entry: {}", window.config.entry.as_deref().unwrap_or(""));
    println!(
        "user runtime dir: {}",
        user_runtime_path(runtime.engine).display()
    );
    println!("detected runtimes: {}", detect_runtime(&runtime).len());
    println!(
        "bridge commands: {}",
        window.config.bridge.commands().join(", ")
    );
    println!("resolving Fenestra web runtime");

    match window.launch_or_install() {
        Ok(process) => {
            println!("launched webview process {}", process.id());
            let _ = process.wait();
        }
        Err(error) => {
            eprintln!("failed to launch webview: {error}");
            std::process::exit(1);
        }
    }
}
