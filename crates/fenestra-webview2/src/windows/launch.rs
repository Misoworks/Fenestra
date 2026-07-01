// WebView2 launch flow — Windows-only.
//
// The launch flow on a real Windows host is:
//
// 1. Build a winit `EventLoop` and create a `Window` (frameless if the
//    user asked for one). Extract the `HWND` via `raw_window_handle`.
// 2. Apply DWM backdrop / glass if the user asked for a glass effect
//    (see `host_controls::apply_dwm_backdrop`).
// 3. Create a per-window `WebView2` user data directory under
//    `%LOCALAPPDATA%/fenestra/webviews/<stable hash>/<instance>`.
// 4. Drive the COM-style async env creation: build a
//    `CreateCoreWebView2EnvironmentCompletedHandler` whose callback
//    sends the result to an `mpsc::channel`, kick off
//    `CreateCoreWebView2EnvironmentWithOptions`, and call
//    `webview2_com::wait_with_pump` on the channel. This blocks the
//    UI thread but pumps Win32 messages, which is exactly what the
//    COM apartment expects.
// 5. From the env, do the same dance with
//    `CreateCoreWebView2ControllerCompletedHandler` to obtain a
//    `ICoreWebView2Controller`.
// 6. Wire up the event handlers on the controller's `ICoreWebView2`:
//    - `add_NavigationStarting` — intercept `fenestra://bridge/...`
//      and `fenestra://window/...` URLs.
//    - `add_WebMessageReceived` — receive `postMessage(...)` from the
//      page so plain-text window commands work without a navigation.
//    - `AddScriptToExecuteOnDocumentCreated` — install the canonical
//      Fenestra bridge script (`fenestra_bridge::install_script`)
//      into every main-frame document.
// 7. If the entry URL is `http(s)://`, probe the dev server with
//    short TCP connect timeouts before `Navigate` (so a Vite-style
//    dev server has a chance to start).
// 8. `webview.Navigate(url)`. WebView2 repaints itself; winit only
//    drives window events.
// 9. Run the winit event loop. Window and WebView2 creation happen in
//    `ApplicationHandler::can_create_surfaces` — winit 0.31 does not call
//    `resumed` on Windows. The loop processes `winit::Event`s plus user
//    plus user events delivered via an `mpsc` channel
//    (`WebView2UserEvent`) that the app's `about_to_wait` callback
//    drains. The winit 0.31 API does not support typed user events
//    on the `EventLoopProxy` (only a bare `wake_up`), so the channel
//    is the only safe way to talk to the UI thread from a bridge
//    handler or activity emitter running on a worker thread.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

use fenestra_bridge::{ActivityHostUpdate, LaunchMetrics};
use fenestra_runtime::RuntimeInfo;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowAttributes, WindowId, WindowLevel},
};

use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
    },
};
use webview2_com_sys::Microsoft::Web::WebView2::Win32 as SysWin32;

use crate::{
    WebView2Config, WebView2Error, WebView2Process, WebView2ProcessInner, WebView2Result,
    WebView2Window, windows::bridge,
};

pub(crate) fn launch(
    window: WebView2Window,
    runtime: RuntimeInfo,
) -> WebView2Result<WebView2Process> {
    let metrics = LaunchMetrics::new(metrics_label(&window.config));
    metrics.mark("launch.start");

    let url = entry_url(&window.config)?;
    metrics.mark("entry_url.ready");

    let event_loop = EventLoop::new()
        .map_err(|error| WebView2Error::Backend(format!("winit event loop: {error}")))?;
    metrics.mark("event_loop.ready");

    let (event_tx, event_rx) = std::sync::mpsc::channel::<WebView2UserEvent>();

    let bridge_runtime = fenestra_bridge::BridgeRuntime::new(
        window.bridge_handlers.clone(),
        window.config.bridge.clone(),
        window.config.security.clone(),
    );
    metrics.mark("bridge_runtime.ready");

    let activity = fenestra_bridge::ActivityRegistry::default();
    let command_allowlist =
        fenestra_bridge::bridge_commands_with_internal(window.config.bridge.commands());

    let emitter = Arc::new(crate::WebView2ActivityEmitter {
        sender: event_tx.clone(),
        activity: activity.clone(),
    });

    let inner = {
        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(WebView2ProcessInner {
            hwnd: std::sync::atomic::AtomicIsize::new(0),
            controller: Mutex::new(None),
            webview: Mutex::new(None),
            bridge_runtime: Mutex::new(Some(bridge_runtime)),
            activity: activity.clone(),
            emitter: emitter.clone(),
            metrics: metrics.clone(),
            event_sender: event_tx.clone(),
            runtime: runtime.clone(),
            background_frame_rate: window.config.lifecycle.background_frame_rate,
            command_allowlist: command_allowlist.clone(),
        })
    };

    let state = LaunchState {
        config: window.config,
        url,
        inner: inner.clone(),
        event_rx,
        window: None,
    };
    let app = LaunchApp { state };
    event_loop
        .run_app(app)
        .map_err(|error| WebView2Error::Backend(format!("winit run_app: {error}")))?;
    metrics.mark("event_loop.exit");

    Ok(WebView2Process { inner })
}

struct LaunchState {
    config: WebView2Config,
    url: String,
    inner: Arc<WebView2ProcessInner>,
    event_rx: Receiver<WebView2UserEvent>,
    window: Option<Box<dyn Window>>,
}

struct LaunchApp {
    state: LaunchState,
}

impl ApplicationHandler for LaunchApp {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self
            .state
            .inner
            .hwnd
            .load(std::sync::atomic::Ordering::Relaxed)
            != 0
        {
            return;
        }
        let mut attributes = WindowAttributes::default()
            .with_title(self.state.config.title.clone())
            .with_surface_size(PhysicalSize::new(
                self.state.config.width.max(1) as f64,
                self.state.config.height.max(1) as f64,
            ))
            .with_visible(self.state.config.visible)
            .with_resizable(self.state.config.resizable)
            .with_min_surface_size(PhysicalSize::new(
                self.state.config.min_width.max(1) as f64,
                self.state.config.min_height.max(1) as f64,
            ))
            .with_decorations(self.state.config.chrome.uses_native_decorations())
            .with_transparent(self.state.config.transparent);
        if self.state.config.always_on_top {
            attributes = attributes.with_window_level(WindowLevel::AlwaysOnTop);
        }
        let window: Box<dyn Window> = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("fenestra: failed to create winit window: {error}");
                event_loop.exit();
                return;
            }
        };
        let hwnd = match window.window_handle() {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::Win32(handle) => handle.hwnd.get(),
                _ => {
                    eprintln!("fenestra: winit did not return a Win32 handle");
                    event_loop.exit();
                    return;
                }
            },
            Err(error) => {
                eprintln!("fenestra: failed to extract HWND: {error}");
                event_loop.exit();
                return;
            }
        };
        self.state
            .inner
            .hwnd
            .store(hwnd, std::sync::atomic::Ordering::Relaxed);
        self.state.inner.metrics.mark("hwnd.ready");

        let _ = super::host_controls::apply_dwm_backdrop(hwnd, &self.state.config);

        match create_webview2(
            hwnd,
            &self.state.config,
            &self.state.url,
            self.state.inner.clone(),
        ) {
            Ok(()) => self.state.inner.metrics.mark("controller.ready"),
            Err(error) => {
                eprintln!("fenestra: WebView2 controller failed: {error}");
                event_loop.exit();
                return;
            }
        }

        self.state.window = Some(window);
    }

    fn window_event(
        &mut self,
        _event_loop: &dyn ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if let WindowEvent::CloseRequested = event {
            let _ = self.state.inner.event_sender.send(WebView2UserEvent::Exit);
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        loop {
            match self.state.event_rx.try_recv() {
                Ok(event) => self.handle_user_event(event_loop, event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    event_loop.exit();
                    break;
                }
            }
        }
    }
}

impl LaunchApp {
    fn handle_user_event(&mut self, event_loop: &dyn ActiveEventLoop, event: WebView2UserEvent) {
        let hwnd = self
            .state
            .inner
            .hwnd
            .load(std::sync::atomic::Ordering::Relaxed);
        match event {
            WebView2UserEvent::BridgeEvent { name, payload } => {
                if let Some(webview) = self.state.inner.webview.lock().unwrap().clone() {
                    bridge::execute_bridge_emit(&webview, &name, &payload);
                }
            }
            WebView2UserEvent::Activity { update } => {
                emit_activity_event(self.state.inner.clone(), update);
            }
            WebView2UserEvent::SetVisible(visible) => {
                if visible {
                    let _ = super::host_controls::show_window(hwnd);
                } else {
                    let _ = super::host_controls::hide_window(hwnd);
                }
            }
            WebView2UserEvent::Show => {
                let _ = super::host_controls::show_window(hwnd);
            }
            WebView2UserEvent::Hide => {
                let _ = super::host_controls::hide_window(hwnd);
            }
            WebView2UserEvent::Focus => {
                let _ = super::host_controls::focus_window(hwnd);
            }
            WebView2UserEvent::Minimize => {
                let _ = super::host_controls::minimize_window(hwnd);
            }
            WebView2UserEvent::Maximize => {
                let _ = super::host_controls::maximize_window(hwnd);
            }
            WebView2UserEvent::Unmaximize => {
                let _ = super::host_controls::unmaximize_window(hwnd);
            }
            WebView2UserEvent::Exit => event_loop.exit(),
        }
    }
}

fn create_webview2(
    hwnd: isize,
    config: &WebView2Config,
    url: &str,
    inner: Arc<WebView2ProcessInner>,
) -> WebView2Result<()> {
    let parent = windows::Win32::Foundation::HWND(hwnd as *mut _);

    let user_data_dir = webview_user_data_dir(&config.title, url);
    std::fs::create_dir_all(&user_data_dir).map_err(WebView2Error::Io)?;
    let user_data_dir_str = user_data_dir
        .to_str()
        .ok_or_else(|| WebView2Error::Backend("user data dir is not UTF-8".to_string()))?;
    let user_data_wide = bridge::wide_pwstr(user_data_dir_str);
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();

    let env: ICoreWebView2Environment = {
        let (tx, rx) = std::sync::mpsc::channel();
        let env_options = options.clone();
        let user_data_ptr = user_data_wide.as_ptr();
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
            move |error_code, environment| {
                let result = (|| {
                    error_code?;
                    environment.ok_or_else(|| {
                        windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
                    })
                })();
                tx.send(result).map_err(|_| {
                    windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
                })
            },
        ));
        unsafe {
            SysWin32::CreateCoreWebView2EnvironmentWithOptions(
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR(user_data_ptr),
                &env_options,
                &handler,
            )?;
        }
        match webview2_com::wait_with_pump(rx) {
            Ok(Ok(env)) => env,
            Ok(Err(error)) => return Err(bridge::webview2_error(error)),
            Err(error) => {
                return Err(WebView2Error::Backend(format!(
                    "env wait_with_pump: {error}"
                )));
            }
        }
    };
    inner.metrics.mark("env.ready");

    let controller: ICoreWebView2Controller = {
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |error_code, controller| {
                let result = (|| {
                    error_code?;
                    controller.ok_or_else(|| {
                        windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
                    })
                })();
                tx.send(result).map_err(|_| {
                    windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
                })
            },
        ));
        unsafe {
            env.CreateCoreWebView2Controller(parent, &handler)?;
        }
        match webview2_com::wait_with_pump(rx) {
            Ok(Ok(controller)) => controller,
            Ok(Err(error)) => return Err(bridge::webview2_error(error)),
            Err(error) => {
                return Err(WebView2Error::Backend(format!(
                    "controller wait_with_pump: {error}"
                )));
            }
        }
    };
    inner.metrics.mark("controller.env.ready");

    let size = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: config.width as i32,
        bottom: config.height as i32,
    };
    unsafe {
        controller.SetBounds(size).map_err(bridge::webview2_error)?;
        controller
            .SetIsVisible(config.visible)
            .map_err(bridge::webview2_error)?;
    }

    let webview = unsafe { controller.CoreWebView2() }.map_err(bridge::webview2_error)?;
    inner.metrics.mark("webview.ready");

    if let Ok(settings) = unsafe { webview.Settings() } {
        let _ = unsafe { settings.SetAreDefaultContextMenusEnabled(false) };
        let _ = unsafe { settings.SetAreDevToolsEnabled(true) };
    }

    bridge::install_bridge_script(&webview, &inner)?;
    bridge::register_navigation_starting(&webview, inner.clone())?;
    bridge::register_web_message_received(&webview, inner.clone())?;

    wait_for_dev_server(url, &inner.event_sender);

    let url_wide = bridge::wide_pwstr(url);
    unsafe {
        webview
            .Navigate(windows::core::PCWSTR(url_wide.as_ptr()))
            .map_err(bridge::webview2_error)?;
    }
    inner.metrics.mark("navigate.ready");

    *inner.controller.lock().unwrap() = Some(controller);
    *inner.webview.lock().unwrap() = Some(webview);

    Ok(())
}

fn emit_activity_event(inner: Arc<WebView2ProcessInner>, update: ActivityHostUpdate) {
    let name = match update {
        ActivityHostUpdate::Begin(_) => "fenestra.activity.begin",
        ActivityHostUpdate::End(_) => "fenestra.activity.end",
    };
    let payload = fenestra_bridge::host_update_json(&update);
    if let Some(webview) = inner.webview.lock().unwrap().clone() {
        bridge::execute_bridge_emit(&webview, name, &payload);
    }
}

fn webview_user_data_dir(title: &str, url: &str) -> PathBuf {
    user_cache_home()
        .join("fenestra")
        .join("webviews")
        .join(format!("{:016x}", stable_hash(&[title, url])))
        .join("instances")
        .join(instance_key())
}

fn user_cache_home() -> PathBuf {
    if let Some(cache) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(cache);
    }
    if let Some(profile) = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|home| home.join("AppData").join("Local"))
    {
        return profile;
    }
    std::env::temp_dir()
}

fn instance_key() -> String {
    let counter = std::sync::atomic::AtomicU64::fetch_add(
        &INSTANCE_COUNTER,
        1,
        std::sync::atomic::Ordering::Relaxed,
    );
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{counter}-{timestamp}", std::process::id())
}

static INSTANCE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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

fn entry_url(config: &WebView2Config) -> WebView2Result<String> {
    if let Some(url) = &config.dev_url {
        return Ok(url.clone());
    }
    if let Some(url) = &config.url {
        return Ok(url.clone());
    }
    let Some(entry) = &config.entry else {
        return Err(WebView2Error::Backend(
            "WebView2 window has no entry, URL, or dev URL".to_string(),
        ));
    };
    let path = std::path::PathBuf::from(entry);
    let canonical = path.canonicalize().unwrap_or(path);
    Ok(format!(
        "file:///{}",
        canonical.display().to_string().replace('\\', "/")
    ))
}

fn metrics_label(config: &WebView2Config) -> String {
    config
        .app_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.title)
        .to_string()
}

fn wait_for_dev_server(url: &str, _event_tx: &Sender<WebView2UserEvent>) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if dev_server_reachable(url) {
            return;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn dev_server_reachable(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let port = authority
        .rsplit(':')
        .next()
        .and_then(|value| value.parse::<u16>().ok());
    if port.is_none() {
        return true;
    }
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .rsplit(':')
        .next()
        .unwrap_or(authority);
    let host = if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        "127.0.0.1"
    } else {
        host
    };
    let _ = scheme;
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::new(
            host.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST.into()),
            port.unwrap_or(0),
        ),
        Duration::from_millis(150),
    )
    .is_ok()
}

/// User events that the winit event loop processes in addition to
/// `winit::Event::WindowEvent`. Bridge handlers and the activity
/// emitter send these via an `mpsc::Sender`; the app's
/// `about_to_wait` callback drains the channel and dispatches the
/// events on the UI thread.
#[derive(Debug, Clone)]
pub enum WebView2UserEvent {
    BridgeEvent {
        name: String,
        payload: serde_json::Value,
    },
    Activity {
        update: ActivityHostUpdate,
    },
    SetVisible(bool),
    Show,
    Hide,
    Focus,
    Minimize,
    Maximize,
    Unmaximize,
    Exit,
}

impl WebView2UserEvent {
    pub(crate) fn dispatch(self, sender: &Sender<Self>) -> bool {
        sender.send(self).is_ok()
    }
}
