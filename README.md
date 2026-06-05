# Fenestra

Fenestra owns shared web runtime management and embedded webview backends.

Stuk remains the native app framework. `stuk-fenestra` is the adapter crate that lets Stuk apps use
Fenestra windows and hybrid webview surfaces without making Stuk depend on CEF or runtime downloads.

Runtime files live under:

```txt
~/.local/share/fenestra/runtimes/cef/
```

## Standalone Windows

Fenestra has three standalone window modes:

```rust
CefWindow::new().system_chrome()
CefWindow::new().fenestra_chrome()
CefWindow::new().frameless()
CefWindow::new().frameless().glass()
```

- `system_chrome()` uses normal OS/window-manager decorations.
- `fenestra_chrome()` uses a Fenestra-owned native OSR host with a built-in titlebar.
- `frameless()` creates a true undecorated OSR window; apps provide any titlebar UI and declare drag/control regions.
- `frameless().glass()` uses the same app-drawn OSR path with transparent composition and blur regions.

Linux is Wayland-first. Frameless, transparent, and glass windows use the OSR native-host path by
default; the direct CEF top-level path remains available for opaque system-decorated compatibility
windows.

Run the modes:

```sh
cargo run -p fenestra-notes -- --system
cargo run -p fenestra-notes -- --fenestra-chrome
cargo run -p fenestra-notes -- --frameless
cargo run -p fenestra-notes -- --glass
cargo run -p stuk-notes
```

Create a standalone app:

```sh
cargo run -p fenestra -- new my-notes
```

Register a source checkout as a local desktop app:

```sh
cargo run -p fenestra -- install .
cargo run -p fenestra -- install . --autostart
cargo run -p fenestra -- update --all
```

Source installs are user-local. Fenestra writes launchers and registry records under
`~/.local/share/fenestra/apps/<app-id>/` and, on Linux desktops, writes `.desktop` files under
`~/.local/share/applications/`. Passing `--autostart` also writes an autostart entry under
`~/.config/autostart/`. Updating a registered git checkout runs `git pull --ff-only` and refreshes
the launcher metadata.

## Desktop Services

Standalone Fenestra windows can declare desktop service requirements alongside the webview:

```rust
CefWindow::new()
    .tray_icon(TrayIcon::new("main", "Notes"))
    .autostart(AutostartEntry::new("notes", "Notes", "notes --background"))
    .single_instance(SingleInstancePolicy::FocusExisting);
```

Backends decide which services are available for the current platform. Desktop Linux, macOS, and
Windows can provide tray icons, autostart registration, global shortcuts, deep links, single-instance
behavior, and native messaging; mobile and browser targets keep only the services that make sense
there.

On Linux today, Fenestra applies autostart entries, deep-link MIME defaults, browser native
messaging manifests, StatusNotifier tray icons, XDG desktop portal global-shortcuts sessions, and
user-runtime single-instance routing before launching the webview. Tray item clicks, global shortcut
activations, and second-instance activations are forwarded into the web bridge as:

```js
window.fenestra.bridge.listen("tray.activate", event => {});
window.fenestra.bridge.listen("globalShortcut.activate", event => {});
window.fenestra.bridge.listen("singleInstance.activate", event => {});
```

Each app/window gets its own CEF cache profile while sharing the installed runtime, so multiple apps
can use `~/.local/share/fenestra/runtimes/cef/` at the same time.

## Runtime Reliability

Runtime installs and Fenestra CEF host builds are locked per user-local runtime. If two apps start
while CEF is missing, one installs or builds while the other waits and reuses the completed runtime.
Stale locks are removed automatically.

Old user-local runtime versions can be pruned from Rust code:

```rust
fenestra_runtime::prune_user_runtimes(&RuntimeConfig::default(), 2)?;
```

or from the CLI:

```sh
fenestra runtime prune cef --keep 2
```

Specific user-local versions can be removed with:

```sh
fenestra runtime remove cef 126.2.7 --package standard
```

Fenestra keeps browser profiles and CEF root cache paths outside the shared runtime installation.
That keeps concurrent apps isolated while sharing binaries and OS page cache.

## Performance Diagnostics

Set `FENESTRA_TRACE=1` to print launch and OSR lifecycle timings:

```sh
FENESTRA_TRACE=1 cargo run -p fenestra-notes -- --glass
```

Launched processes also expose a metrics snapshot:

```rust
let process = CefWindow::new().vite_dev_server(5173).launch_or_install()?;
let metrics = process.metrics();
```

The snapshot records stages such as dev command spawn, dev server readiness, host build/readiness,
host process spawn, and launch readiness. OSR hosts also trace first paint and lifecycle frame-rate
changes when tracing is enabled.

## Webview Lifecycle

Fenestra can throttle or hibernate OSR webviews without app code managing CEF processes:

```rust
CefWindow::new()
    .lifecycle_policy(CefLifecyclePolicy::browser_tab())
    .background_frame_rate(5)
    .hibernate_after(Duration::from_secs(300));
```

By default, webviews suspend when minimized or occluded. `browser_tab()` also suspends on blur and
hard-hibernates after five minutes. Hard hibernation keeps the last native texture visible, sends the
page a hibernate event, then stops the embedded CEF child process. Resuming relaunches the webview.

The page API is always injected:

```js
window.fenestra.lifecycle.listen(({ state, reason }) => {});
window.addEventListener("fenestra:suspend", event => {});
window.addEventListener("fenestra:resume", event => {});
window.addEventListener("fenestra:hibernate", event => {});
```

## Palette Windows

Hidden palette apps can keep a process alive while the native window is hidden:

```rust
let process = CefWindow::new()
    .hidden()
    .frameless()
    .glass()
    .launch_or_install()?;

process.show();
process.focus_window();
process.hide();
```

Hidden OSR windows enter the suspended lifecycle immediately. They resume when shown or focused, and
can still hard-hibernate later if the lifecycle policy enables it.

The same controls are available in web content:

```js
window.fenestra.window.show();
window.fenestra.window.focus();
window.fenestra.window.hide();
```

These calls are host controls, not bridge commands, so they work even when the app exposes no native
command surface.
