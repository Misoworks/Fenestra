use std::path::PathBuf;

use fenestra_cef::{
    BridgeCommandDescriptor, BridgeResponse, FenestraLifecyclePolicy, FenestraWindow,
    FenestraWindowControlAction, RuntimeConfig, RuntimeMode, WindowRegion, WindowRegionRect,
    run_fenestra_host_from_args, user_runtime_path,
};

const APP_TITLEBAR_HEIGHT: i32 = 38;
const SIDEBAR_WIDTH: i32 = 260;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if run_fenestra_host_from_args(&args) {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = RuntimeConfig {
        mode: RuntimeMode::SharedPreferred,
        allow_user_install: true,
        bundled_dir: Some(manifest_dir.clone()),
        ..RuntimeConfig::default()
    };
    let mode = ExampleChromeMode::from_args(&args);
    let entry = mode.entry(&manifest_dir);
    let mut window = mode.apply(
        FenestraWindow::new()
            .title("Fenestra Notes")
            .size(900, 640)
            .entry(entry)
            .runtime(runtime.clone())
            .lifecycle_policy(FenestraLifecyclePolicy::browser_tab())
            .bridge_descriptor_handler(
                BridgeCommandDescriptor::new("notes.create").target("desktop"),
                |command| {
                    let id = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or_default();
                    Ok(BridgeResponse::json(serde_json::json!({
                        "ok": true,
                        "id": format!("fenestra-{id}"),
                        "params": command.params
                    })))
                },
            ),
    );
    if args.iter().any(|arg| arg == "--hidden") {
        window = window.hidden();
    }

    println!("Fenestra standalone notes example");
    println!("chrome mode: {}", mode.label());
    println!(
        "user runtime dir: {}",
        user_runtime_path(runtime.engine).display()
    );
    match window.launch_or_install() {
        Ok(process) => {
            println!("launched CEF process {}", process.id());
            let _ = process.wait();
        }
        Err(error) => {
            eprintln!("failed to launch CEF window: {error}");
            std::process::exit(1);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ExampleChromeMode {
    System,
    FenestraChrome,
    Frameless,
    Glass,
}

impl ExampleChromeMode {
    fn from_args(args: &[String]) -> Self {
        if args.iter().any(|arg| arg == "--system") {
            Self::System
        } else if args.iter().any(|arg| arg == "--fenestra-chrome") {
            Self::FenestraChrome
        } else if args.iter().any(|arg| arg == "--frameless") {
            Self::Frameless
        } else if args.iter().any(|arg| arg == "--glass") {
            Self::Glass
        } else {
            Self::Glass
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::System => "system CEF window",
            Self::FenestraChrome => "Fenestra chrome OSR window",
            Self::Frameless => "app-drawn frameless OSR window",
            Self::Glass => "app-drawn glass OSR window",
        }
    }

    fn entry(self, manifest_dir: &std::path::Path) -> String {
        let path = manifest_dir.join("ui/index.html");
        let suffix = if self.uses_app_chrome() {
            "?chrome=app"
        } else {
            ""
        };
        format!("{}{}", path.display(), suffix)
    }

    fn uses_app_chrome(self) -> bool {
        matches!(self, Self::Frameless | Self::Glass)
    }

    fn apply(self, window: FenestraWindow) -> FenestraWindow {
        match self {
            Self::System => window.system_chrome().opaque(),
            Self::FenestraChrome => window.fenestra_chrome().opaque(),
            Self::Frameless => app_chrome(
                window
                    .frameless()
                    .transparent(true)
                    .input_region(WindowRegion::adaptive_rounded_rect(14)),
            ),
            Self::Glass => app_chrome(
                window
                    .frameless()
                    .glass()
                    .blur_region(WindowRegion::adaptive_titlebar_sidebar(
                        SIDEBAR_WIDTH,
                        APP_TITLEBAR_HEIGHT,
                        14,
                    ))
                    .opaque_region(WindowRegion::adaptive_content_after_sidebar(
                        SIDEBAR_WIDTH,
                        APP_TITLEBAR_HEIGHT,
                    ))
                    .input_region(WindowRegion::adaptive_rounded_rect(14)),
            ),
        }
    }
}

fn app_chrome(window: FenestraWindow) -> FenestraWindow {
    window
        .drag_region(WindowRegionRect::new(0, 0, i32::MAX, APP_TITLEBAR_HEIGHT))
        .control_region(
            FenestraWindowControlAction::Minimize,
            WindowRegionRect::new(-100, 7, 24, 24),
        )
        .control_region(
            FenestraWindowControlAction::Maximize,
            WindowRegionRect::new(-68, 7, 24, 24),
        )
        .control_region(
            FenestraWindowControlAction::Close,
            WindowRegionRect::new(-36, 7, 24, 24),
        )
}
