# Fenestra Implementation Guide

Fenestra is the shared web runtime manager and embedded CEF webview system. Use it when an app wants
HTML/CSS/JS UI with a Rust host, local runtime resolution, native desktop services, and a strict
bridge. Fenestra does not depend on Stuk; Stuk apps wire into Fenestra via `FenestraWindow` from
`fenestra-cef`.

## Crate Map

| Crate | Owns |
| --- | --- |
| `fenestra-runtime` | CEF runtime discovery, user-local installs, package metadata, pruning, and validation |
| `fenestra-cef` | CEF window launch, OSR native host, bridge dispatch, lifecycle, activity leases, desktop services |
| `fenestra-cli` | `fenestra new`, source installs, runtime commands, and bundle staging |

Runtime files are user-local by default:

```txt
~/.local/share/fenestra/runtimes/cef/
```

Apps can use a system runtime, a bundled runtime, or a user-local runtime. If a client CEF runtime is
missing and user installs are allowed, Fenestra installs into the user-local runtime directory rather
than system-wide.

## Standalone Window

Use `FenestraWindow` for a pure Fenestra app:

```rust
use fenestra_cef::{
    FenestraResult, FenestraWindow, FenestraWindowChrome, WindowBackgroundEffect, WindowRegion,
};

fn main() -> FenestraResult<()> {
    if fenestra_cef::run_fenestra_host_from_args(&std::env::args().collect::<Vec<_>>()) {
        return Ok(());
    }

    let process = FenestraWindow::new()
        .app_id("com.example.notes")
        .title("Notes")
        .entry("ui/dist/index.html")
        .dev_server("http://localhost:5173")
        .fenestra_chrome()
        .glass()
        .blur_region(WindowRegion::rounded_rect(280, 720, 14))
        .launch_or_install()?;

    process.wait()?;
    Ok(())
}
```

Window modes:

| Builder | Result |
| --- | --- |
| `system_chrome()` | Normal OS/window-manager decorations |
| `fenestra_chrome()` | Fenestra-owned OSR native window with built-in titlebar |
| `frameless()` | Undecorated OSR window; app supplies controls and drag regions |
| `frameless().glass()` | Transparent OSR window with compositor blur/materials |
| `shell_surface(...)` | Layer-shell surface for palette/panel-style Linux surfaces |

Declare drag and control regions when the web UI owns the titlebar:

```rust
FenestraWindow::new()
    .frameless()
    .titlebar_drag_region(42)
    .control_region(FenestraWindowControlAction::Close, WindowRegionRect::new(-44, 8, 28, 28));
```

## Dev Workflow

For Vite-style apps:

```rust
FenestraWindow::new()
    .entry("ui/dist/index.html")
    .vite_dev_server(5173);
```

`dev_server(...)` waits for loopback variants including `localhost`, `127.0.0.1`, and `::1`, so
normal Vite/Bun workflows do not need host workarounds.

`dev_url(...)` is a development override. If both `url(...)` and `dev_url(...)` are set, Fenestra
loads the dev URL while developing and keeps the production URL for packaged/runtime config:

```rust
FenestraWindow::new()
    .url("https://raday.lantharos.com")
    .dev_url("http://localhost:5173")
    .dev_command("bun run dev -- --host localhost --port 5173 --strictPort");
```

Use source installs for local desktop entries during development:

```sh
fenestra install
fenestra update
fenestra update --all
```

Use bundling for distributable app trees and native package staging:

```sh
fenestra bundle . --target portable
fenestra bundle . --target deb --release
fenestra bundle . --target appimage
fenestra bundle . --target dmg --binary target/aarch64-apple-darwin/release/my-app
fenestra bundle . --target msi --binary target/x86_64-pc-windows-msvc/release/my-app.exe
```

`Fenestra.toml` can pin web build and package metadata:

```toml
[app]
id = "com.example.notes"
name = "Notes"
version = "0.1.0"
icon = "assets/icon.png"
cargo_manifest = "desktop/Cargo.toml"
mime_types = ["inode/directory"]

[install]
command = "cargo run --manifest-path desktop/Cargo.toml --"

[web]
root = "ui"
dist = "ui/dist"
entry = "ui/dist/index.html"
build = "bun run build"
```

## Hosted Web Apps

Use `url(...)` for an existing hosted app. This is the normal path for turning a deployed web
product into a desktop app without copying the web build into the desktop package:

```rust
let process = FenestraWindow::new()
    .app_id("com.lantharos.raday")
    .title("Raday")
    .url("https://raday.lantharos.com")
    .dev_url("http://localhost:5173")
    .allowed_origin("https://preview.raday.lantharos.com")
    .fenestra_chrome()
    .launch_or_install()?;
```

`url(...)`, `remote_url(...)`, and `bundled_url(...)` are aliases for the production web entry.
Fenestra automatically allows that URL's origin for bridge traffic. `allowed_origin(...)` adds any
extra origins that are allowed to invoke the bridge.

The web code should be able to run in a normal browser with no desktop APIs:

```ts
const fenestra = window.fenestra;

export async function invokeDesktop<T>(command: string, payload?: unknown): Promise<T | null> {
  if (!fenestra?.bridge) {
    return null;
  }
  return fenestra.bridge.invoke(command, payload) as Promise<T>;
}
```

For hosted apps, treat the bridge as a privileged desktop-only enhancement. The public website should
continue to work when `window.fenestra` is absent, and native-only actions should stay behind Rust
bridge commands with explicit command names and allowed origins.

Remote-only bundle config:

```toml
[web]
url = "https://raday.lantharos.com"
dev_url = "http://localhost:5173"
allowed_origins = [
  "https://raday.lantharos.com",
  "https://preview.raday.lantharos.com",
  "http://localhost:5173",
]
```

When `root`, `dist`, and `entry` are omitted, `fenestra bundle` does not build or copy local web
assets. Add those fields only when the package should include a local web build.

## Bridge

Register native commands explicitly:

```rust
let process = FenestraWindow::new()
    .entry("ui/dist/index.html")
    .bridge_descriptor_handler(
        BridgeCommandDescriptor::new("notes.list")
            .target("desktop")
            .permission("notes"),
        |_| Ok(BridgeResponse::json(serde_json::json!([
            { "id": "1", "title": "Product notes" }
        ]))),
    )
    .launch_or_install()?;
```

Invoke from web:

```js
const notes = await window.fenestra.bridge.invoke("notes.list");
```

Use `BridgeCommandDescriptor` for permissions, target gating, and per-command allowed origins:

```rust
FenestraWindow::new()
    .url("https://raday.lantharos.com")
    .security(WebViewSecurity::default().allow_origin("https://raday.lantharos.com"))
    .bridge_descriptor_handler(
        BridgeCommandDescriptor::new("files.pick")
            .target("desktop")
            .permission("files")
            .allowed_origin("https://raday.lantharos.com"),
        pick_file,
    );
```

Keep privileged work in Rust and expose small command surfaces to the web page. If a command needs
access to the filesystem, credentials, notifications, native messaging, or global shortcuts, it
should be a Rust command with validation rather than browser-side logic.

## Activity Leases

Hidden UI can be throttled or hibernated, but Fenestra must not hibernate while active work is in
progress. Long-running jobs should usually live on the Rust side; leases tell Fenestra that work is
active and hibernation must wait.

Rust-side lease:

```rust
let lease = process.begin_activity("backup.sync");
run_backup_job()?;
lease.end();
```

Use a non-blocking lease for diagnostics or status-only activity:

```rust
let lease = process.begin_activity_with(
    ActivityOptions::new("metrics.flush").prevents_hibernation(false),
);
```

Web-side lease:

```js
const lease = await window.fenestra.activity.begin({
  name: "ai.indexing",
  preventsHibernation: true,
});

try {
  await runIndexing();
} finally {
  await lease.end();
}
```

Activity leases are not a replacement for durable workers. If a task must survive window closure,
network loss, or app restart, move it to a Rust worker and persist progress.

## Lifecycle

Lifecycle policy controls rendering and hibernation:

```rust
FenestraWindow::new()
    .hidden()
    .hide_on_blur(true)
    .background_frame_rate(1)
    .hibernate_after(Duration::from_secs(300));
```

Defaults are palette-friendly: hidden windows suspend and throttle but stay warm. To trade instant
resume for lower memory, opt in:

```rust
FenestraWindow::new()
    .hidden()
    .lifecycle_policy(FenestraLifecyclePolicy::memory_saver_hidden_window());
```

Web lifecycle events:

```js
window.fenestra.lifecycle.listen(({ state, reason }) => {});
window.addEventListener("fenestra:suspend", event => {});
window.addEventListener("fenestra:resume", event => {});
window.addEventListener("fenestra:hibernate", event => {});
```

## Desktop Services

Desktop integrations are declared on the window:

```rust
FenestraWindow::new()
    .tray_icon(TrayIcon::new("main", "Notes"))
    .autostart(AutostartEntry::new("notes", "Notes", "notes --background"))
    .global_shortcut(GlobalShortcutRegistration::new("show", "Ctrl+Space"))
    .single_instance(SingleInstancePolicy::FocusExisting);
```

Events are forwarded to web as `fenestra:*` events and can also be polled from Rust with
`take_desktop_events()`.

## Runtime And Bundling

Fenestra checks runtime sources in this order:

| Source | Use |
| --- | --- |
| System runtime | Already-installed compatible CEF runtime |
| User-local runtime | Shared runtime under `~/.local/share/fenestra/runtimes/cef/` |
| Bundled runtime | App-provided runtime for offline/self-contained packages |
| Installer | User-local runtime download when no compatible runtime is present |

The runtime is shared, but app cache profiles are isolated per app/window so multiple desktop apps
can run at the same time without Chromium process-singleton collisions.

Bundling can stage native package trees from one host:

```sh
fenestra bundle . --target portable
fenestra bundle . --target deb --release
fenestra bundle . --target rpm --release
fenestra bundle . --target appimage --release
fenestra bundle . --target dmg --binary target/aarch64-apple-darwin/release/raday
fenestra bundle . --target msi --binary target/x86_64-pc-windows-msvc/release/raday.exe
```

Native binaries, signing, notarization, and installer signing still need the relevant platform
toolchain and credentials. `--binary` packages a binary built by CI or a cross-compile step.

## Platform Notes

Fenestra exposes one cross-platform builder, `FenestraWindow`, on every supported target. The
backend is CEF on Linux, Windows, and macOS, so the public API does not change between platforms.

| Platform | Backend | Status |
| --- | --- | --- |
| Linux | CEF with OSR native host (Wayland-first) | Full transparency, blur, glass, shell surfaces |
| Windows | CEF windowed | System chrome, frameless, dev workflow, runtime install |
| macOS  | CEF windowed | System chrome, frameless, dev workflow, runtime install |

OSR features (frameless transparent windows, blur regions, shell surfaces, layer-shell palettes)
currently use the Linux Wayland host. On Windows and macOS the same `FenestraWindow` builder falls
back to the windowed CEF host with native decorations; transparency-style modes still work, the
compositor materials are skipped where the OS does not provide them.

`fenestra-cef` keeps Linux-only crates (`layershellev`, `ksni`, `ashpd`, `wayland-client`, `x11rb`)
behind `cfg(target_os = "linux")` so a downstream app can build for Windows and macOS without
pulling those dependencies. Use the same `fenestra_cef::FenestraWindow` API on every host; the crate
selects the right backend internally.

CEF host build:

- `~/.local/share/fenestra/runtimes/cef/<version>-<package>/` on Linux
- `%LOCALAPPDATA%\fenestra\runtimes\cef\...` on Windows (via `HOME` fallback during cross-platform testing)
- `~/Library/Application Support/fenestra/runtimes/cef/...` on macOS

Fenestra builds a small CEF host binary from `host/shared/` on every platform. The host build needs
CMake plus a platform compiler toolchain (Ninja or Visual Studio on Windows, Xcode Command Line
Tools on macOS, GCC/Clang on Linux). Linux-only macros such as `SET_LINUX_SUID_PERMISSIONS` are
conditioned on `OS_LINUX` inside the CMake file.

Do not switch to system webviews for cross-platform consistency. If a platform ever needs another
backend, it should still preserve the Fenestra bridge, lifecycle, activity, runtime, and window
APIs.
