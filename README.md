# Fenestra

Fenestra owns shared web runtime management and embedded webview backends.

Runtime files live under:

```txt
~/.local/share/fenestra/runtimes/cef/
```

For a practical map of the crates, window modes, bridge, activity leases, lifecycle, desktop
services, dev workflow, and bundling, see
[`docs/implementation-guide.md`](docs/implementation-guide.md).

## Prerequisites

Fenestra is CEF on every supported platform; the public API is `FenestraWindow`
because the runtime is meant to be an implementation detail. You need a Rust
toolchain plus the C/C++ build chain the embedded CEF host compiles with, and
the optional packaging tools you actually want to invoke.

**All platforms**

- Rust 1.89 or newer (`rustup default stable`)
- `cmake` ≥ 3.21 and a C++20 compiler
  - Linux: `gcc` / `clang`, plus `ninja` (preferred)
  - Windows: MSVC (`x86_64-pc-windows-msvc`)
  - macOS: Xcode command-line tools
- A CEF binary distribution for the host OS/arch — pulled automatically by
  `fenestra runtime install cef` from `https://cef-builds.spotifycdn.com/index.json`
- `curl` (or `wget`), `tar`, and `sha1sum` (Linux) / `shasum` (macOS) — used
  to fetch, extract, and verify the CEF archive

**JavaScript**

`fenestra` auto-detects the web build from `ui/`, `frontend/`, `web/`, or
`package.json`. [Bun](https://bun.sh) is preferred when a `bun.lock*`,
`packageManager = "bun@..."`, or any other lockfile is present; otherwise it
falls back to `pnpm`, `yarn`, or `npm`.

**Linux desktop integration (optional, recommended)**

For tray icons, XDG portal global shortcuts, deep-link MIME defaults, autostart
`.desktop` files, and the status-notifier icon to actually appear:

```sh
# Debian / Ubuntu
sudo apt install libgtk-3-bin desktop-file-utils

# Fedora
sudo dnf install gtk-update-icon-cache desktop-file-utils

# Arch
sudo pacman -S gtk3 desktop-file-utils
```

`fenestra install` calls `update-desktop-database` and `gtk-update-icon-cache`
if they are on `PATH`; the install still completes without them.

**Linux packaging tools (optional)**

```sh
# .deb — Debian / Ubuntu
sudo apt install dpkg          # provides dpkg-deb

# .rpm — Fedora / RHEL
sudo dnf install rpm-build

# AppImage — download appimagetool from
#   https://github.com/AppImageCommunity/appimagetool
#   and put it on $PATH
```

`fenestra bundle --target rpm` does not shell out to `rpmbuild` itself; it
writes a spec and a `build-rpm.sh` script that you run on a host with the RPM
toolchain installed.

**macOS packaging (optional)**

`hdiutil` ships with macOS and is used for `.dmg`. Nothing extra to install.

**Windows packaging (optional)**

- `.msi` — [WiX v4](https://wixtoolset.org/) (`wix` on `PATH`)
- `.exe` — [NSIS](https://nsis.sourceforge.io/) (`makensis` on `PATH`)

Code signing, notarization, and installer signing are not part of the CLI;
they still need the relevant platform credentials.

## Install

### Library

Add Fenestra to your app's `Cargo.toml`:

```toml
[dependencies]
fenestra-cef = "0.1"
```

Or pin to the git repo (useful during development or when you want the very latest):

```toml
[dependencies]
fenestra-cef = { git = "https://github.com/Misoworks/Fenestra", branch = "main" }
```

Most apps only need `fenestra-cef`. Standalone uses:

- `fenestra-runtime` — runtime discovery, install, and pruning for both `cef` and `webview2` (the latter is system-managed)
- `fenestra-bridge` — engine-neutral bridge, activity, JS, and launch-metrics types
- `fenestra-webview2` — the WebView2 (Evergreen) backend on Windows; re-exported as `FenestraWindow` on that host
- `fenestra-cli` — the `fenestra` binary

### CLI

Install the `fenestra` binary globally with `cargo`:

```sh
cargo install fenestra-cli
```

Or from the git repo:

```sh
cargo install --git https://github.com/Misoworks/Fenestra --package fenestra-cli
```

After install, `fenestra --help` lists every subcommand. The current surface:

| Command | What it does |
| --- | --- |
| `fenestra new <NAME> [--template notes]` | Scaffold a new app from a built-in template (only `notes` today) |
| `fenestra install [SOURCE] [--autostart]` | Register the current source checkout as a local desktop app; writes `~/.local/share/fenestra/apps/<id>/` plus a `.desktop` file under `~/.local/share/applications/`. `--autostart` also writes `~/.config/autostart/<id>.desktop` |
| `fenestra update [TARGET] [--all]` | Rebuild web assets, re-stage the app, and refresh launcher metadata for a previously installed source checkout |
| `fenestra bundle [SOURCE] --target <T>` | Stage the app and (when the right tool is on `PATH`) produce a package. Targets: `portable`, `linux`, `deb`, `rpm`, `appimage`, `windows`, `exe`, `msi`, `macos`, `dmg` |
| `fenestra runtime list` | Show installed runtimes under `~/.local/share/fenestra/runtimes/<engine>/` |
| `fenestra runtime install <engine> [--package minimal\|client\|standard]` | Download and unpack a runtime for the host platform. `engine` is `cef` or `webview2` |
| `fenestra runtime remove <engine> [VERSION] [--package ...]` | Remove a specific runtime version. `webview2` is a no-op (system-managed) |
| `fenestra runtime prune <engine> --keep N` | Keep only the N most recent runtime versions. `webview2` is a no-op |
| `fenestra runtime doctor` | Check that the runtime, CMake toolchain, and shell-out deps are usable. The `engine` is the platform default: `webview2` on Windows, `cef` elsewhere |

The CLI shell-outs to `cargo`, `bun` / `pnpm` / `yarn` / `npm`, `tar`,
`dpkg-deb`, `appimagetool`, `hdiutil`, `wix`, and `makensis`. When the tool for
a target is missing, `fenestra bundle` still completes the staged tree and
writes a `build-<target>.sh` script so the package can be finished later on a
host that has the tool.

## Platforms

Fenestra picks the right engine on each host. The public `FenestraWindow`
type is a type alias:

- **Windows** — `fenestra_webview2::WebView2Window`. WebView2 (Evergreen)
  is bundled with Windows, auto-updated by Windows Update, and shares the
  process. No C++ host, no CMake build, no ninja. The default.
- **Linux / macOS** — `fenestra_cef::CefWindow`. CEF is downloaded and built
  on first launch. The same builder API works for both.

Both backends share the same wire format, JS surface, and activity
registry — they live in `fenestra-bridge` (`crates/fenestra-bridge/`).
The canonical `web_bridge.js` is a single file included as a `&str` from
`fenestra_bridge::INSTALL_SCRIPT` and embedded into the C++ CEF host as
a generated `fenestra_bridge_js.h` at build time. There is exactly one
JS body, no C++ copy to keep in sync.

### What runs on each OS

- **Windows** — WebView2 backend in `fenestra-webview2`. The launch flow
  uses `winit 0.31` for the event loop and HWND, then
  `CreateCoreWebView2EnvironmentWithOptions` +
  `CreateCoreWebView2ControllerWithOptions` to mount the webview in-process.
  Bridge dispatch goes through `add_NavigationStarting` (cancels the
  `fenestra://bridge/...` navigation, parses with
  `fenestra_bridge::parse_bridge_url`, dispatches through
  `fenestra_bridge::BridgeRuntime`, then posts the response via
  `webview.ExecuteScript("window.__fenestraBridgeResolve(id, ok, payload)")`).
  Bridge install uses `add_DocumentStateChanged` to call
  `fenestra_bridge::install_script(&commands)` on every main-frame
  document. Frameless / drag regions / DWM glass are stubbed pending a
  Windows-host CI run; the load-bearing shapes
  (`apply_dwm_backdrop`, `NonClientRegionChanged`, the controller event
  surface) are in place and the `Windows` build of `fenestra-webview2`
  compiles cleanly.
- **Linux** — two internal CEF host modes:
  - **OSR (off-screen rendering) host** — used for frameless, transparent,
    and glass windows, and for everything that needs to draw into a
    Wayland layer-shell surface. The Rust side picks this automatically
    via `FenestraWindow::should_use_osr_host`
    (`crates/fenestra-cef/src/lib.rs:1211-1238`).
  - **Windowed CEF host** — used for `system_chrome()` and other
    system-decorated compatibility windows. Force it explicitly with
    `FENESTRA_CEF_BACKEND=windowed` if you want a top-level CEF window
    on Linux.
  - Wayland is preferred. `fenestra-cef` passes
    `--ozone-platform=wayland`, sets `GDK_BACKEND=wayland` and
    `XDG_SESSION_TYPE=wayland`, and exports `LD_LIBRARY_PATH` for the CEF
    release dir.
  - Linux-only desktop services: `ksni` StatusNotifier tray icons, `ashpd`
    XDG desktop portal global shortcuts, `layershellev` layer-shell
    surfaces, `.desktop` autostart, `x-scheme-handler` deep-link MIME
    registration, Chromium / Firefox native-messaging manifests, and
    single-instance routing through a `UnixListener` in
    `$XDG_RUNTIME_DIR`.
  - Tray, global-shortcut, and second-instance events are forwarded into
    the web bridge as
    `window.fenestra.bridge.listen("tray.activate" | "globalShortcut.activate" | "singleInstance.activate", ...)`.
- **macOS** — windowed CEF host, Xcode command-line tools.
  `DYLD_FALLBACK_LIBRARY_PATH` points at the CEF `Release` dir. As on
  Linux, desktop services are gated to Linux; on macOS they come from
  upstream `stuk-platform`.
- **Mobile** — `FenestraWindow::launch_or_install` returns
  `FenestraError::MobileSystemWebViewRequired` on Android and iOS; mobile
  targets are expected to use the OS webview directly.

### Forcing a specific CEF host on Linux

```sh
FENESTRA_CEF_BACKEND=windowed fenestra-notes --system
```

Accepted values: `osr` (default for frameless / glass), `windowed` /
`cef-windowed` / `system-window` (top-level CEF window).

## Examples

The `examples/` directory holds reference apps. They are not published to crates.io, so install them
from git:

```sh
# Fenestra CEF reference app
cargo install --git https://github.com/Misoworks/Fenestra --package fenestra-notes

# Stuk reference app (uses the underlying stuk crates, not Fenestra)
cargo install --git https://github.com/Misoworks/Stuk --package notes
```

## Standalone Windows

Fenestra has three standalone window modes:

```rust
FenestraWindow::new().system_chrome()
FenestraWindow::new().fenestra_chrome()
FenestraWindow::new().frameless()
FenestraWindow::new().frameless().glass()
FenestraWindow::new().frameless().glass_material(WindowBackgroundEffect::Niko)
```

- `system_chrome()` uses normal OS/window-manager decorations.
- `fenestra_chrome()` uses a Fenestra-owned native OSR host with a built-in titlebar.
- `frameless()` creates a true undecorated OSR window; apps provide any titlebar UI and declare drag/control regions.
- `frameless().glass()` uses the same app-drawn OSR path with transparent composition and Luca material.
- `glass_material(...)` requests a specific semantic material such as Luca, Niko, or Maris. Apps can call `glass_low_power_material(WindowBackgroundEffect::Maris)` to opt into a Maris fallback when the session is in low-power mode.

Linux is Wayland-first. Frameless, transparent, and glass windows use the OSR native-host path by
default; the direct CEF top-level path remains available for opaque system-decorated compatibility
windows. Windows and macOS use the windowed CEF host with the same `FenestraWindow` API; see
[`docs/implementation-guide.md`](docs/implementation-guide.md#platform-notes) for what each backend
currently supports.

Run the modes:

```sh
fenestra-notes --system
fenestra-notes --fenestra-chrome
fenestra-notes --frameless
fenestra-notes --glass
notes
```

Create a standalone app:

```sh
fenestra new my-notes
```

Register a source checkout as a local desktop app:

```sh
fenestra install
fenestra install . --autostart
fenestra update
fenestra update --all
```

Source installs are user-local. Fenestra writes launchers and registry records under
`~/.local/share/fenestra/apps/<app-id>/` and, on Linux desktops, writes `.desktop` files under
`~/.local/share/applications/`. Passing `--autostart` also writes an autostart entry under
`~/.config/autostart/`. Updating rebuilds configured web assets, restages the app, and refreshes
the launcher metadata from the current source checkout.

## Bundles

Fenestra can build the web assets, build the Rust desktop host, stage the app, and produce unsigned
desktop packages:

```sh
fenestra bundle . --target portable
fenestra bundle . --target deb --release
fenestra bundle . --target appimage
fenestra bundle . --target dmg --binary target/aarch64-apple-darwin/release/my-app
fenestra bundle . --target msi --binary target/x86_64-pc-windows-msvc/release/my-app.exe
```

Targets are `portable`, `linux`, `deb`, `rpm`, `appimage`, `windows`, `exe`, `msi`, `macos`, and
`dmg`. Fenestra can stage every target on one host and will run local packaging tools when they are
available: `tar`, `dpkg-deb`, `appimagetool`, `hdiutil`, WiX, or NSIS. When a native packaging tool
is not available, Fenestra leaves the staged app tree plus the manifest or build script needed to
finish that package on the right machine.

Cross-host packaging is supported at the staging layer. Fully native binaries, code signing,
notarization, and installer signing still need the relevant platform toolchain and credentials. Use
`--binary` to package an executable that was built elsewhere or by a custom cross-compile step.

Web builds run before the Rust build unless `--no-web-build` is passed. Fenestra auto-detects Vite
and other package builds from `ui/package.json`, `frontend/package.json`, `web/package.json`, or
`package.json`. Bun is preferred when a Bun lockfile or `packageManager = "bun@..."` is present.

Projects can make bundling explicit with `Fenestra.toml`:

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
url = "https://app.example.com"
dev_url = "http://localhost:5173"
allowed_origins = ["https://app.example.com", "http://localhost:5173"]
```

For a hosted app, omit `root`, `dist`, `entry`, and `build`; Fenestra will load `url` directly and
will not stage local web assets. `dev_url` overrides `url` while developing and waits for normal
loopback variants such as `localhost`, `127.0.0.1`, and `::1`.

## Desktop Services

Standalone Fenestra windows can declare desktop service requirements alongside the webview:

```rust
FenestraWindow::new()
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

CEF hosts are launched with app-runtime defaults: Wayland/Ozone first on Linux, Vulkan disabled, and
non-app browser services such as sync, translate, media routing, extensions, crash uploading,
component updates, and background networking disabled.

## Performance Diagnostics

Set `FENESTRA_TRACE=1` to print launch and OSR lifecycle timings:

```sh
FENESTRA_TRACE=1 fenestra-notes --glass
```

Launched processes also expose a metrics snapshot:

```rust
let process = FenestraWindow::new().vite_dev_server(5173).launch_or_install()?;
let metrics = process.metrics();
```

The snapshot records stages such as dev command spawn, dev server readiness, host build/readiness,
host process spawn, and launch readiness. OSR hosts also trace first paint and lifecycle frame-rate
changes when tracing is enabled.

## Webview Lifecycle

Fenestra can throttle or hibernate OSR webviews without app code managing CEF processes:

```rust
FenestraWindow::new()
    .lifecycle_policy(FenestraLifecyclePolicy::browser_tab())
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

Activity leases prevent hibernation while declared work is active:

```rust
let lease = process.begin_activity("backup.sync");
run_backup_job()?;
lease.end();
```

```js
const lease = await window.fenestra.activity.begin({ name: "ai.indexing" });
try {
  await runIndexing();
} finally {
  await lease.end();
}
```

Use Rust services for durable long-running jobs. Activity leases only tell Fenestra that a running
window or webview process should not be hibernated while that work is active.

## Palette Windows

Hidden palette apps can keep a process alive while the native window is hidden:

```rust
let process = FenestraWindow::new()
    .hidden()
    .frameless()
    .glass()
    .launch_or_install()?;

process.show();
process.focus_window();
process.hide();
```

Hidden OSR windows enter the suspended lifecycle immediately and stay warm by default so palettes can
show instantly. They resume when shown or focused. To trade instant resume for lower hidden memory,
opt into hibernation explicitly:

```rust
FenestraWindow::new()
    .hidden()
    .lifecycle_policy(FenestraLifecyclePolicy::memory_saver_hidden_window());
```

The same controls are available in web content:

```js
window.fenestra.window.show();
window.fenestra.window.focus();
window.fenestra.window.hide();
```

These calls are host controls, not bridge commands, so they work even when the app exposes no native
command surface.

## Publishing

The three public Fenestra crates ship from this repo: `fenestra-runtime`, `fenestra-cli`, and
`fenestra-cef`. They depend on four upstream stuk crates (`stuk-platform`, `stuk-platform-shell`,
`stuk-render`, `stuk-style`) that ship from the sibling [stuk](https://github.com/Misoworks/Stuk)
repo.

Releases run automatically through GitHub Actions and crates.io
[trusted publishing](https://crates.io/docs/trusted-publishing). Push a tag and
[`.github/workflows/publish.yml`](.github/workflows/publish.yml) exchanges a short-lived OIDC token
for a crates.io API token, then publishes every crate in dependency order. Publish stuk first, then
fenestra:

```sh
# 1. From the stuk repo: bump workspace.package.version, then
cd ../stuk
git tag v0.1.1 && git push --tags

# 2. After stuk crates land on crates.io, from this repo:
cd ../fenestra
git tag v0.1.1 && git push --tags
```

For the very first publish of a crate (or for local testing) use the script directly:

```sh
cargo login <CRATES_IO_TOKEN>

# from stuk first
cd ../stuk && scripts/publish.sh

# then fenestra (use --package-only if upstream stuk crates are not yet on crates.io)
cd ../fenestra && scripts/publish.sh
```

After the first manual publish, configure a trusted publisher on crates.io for each crate
(`Misoworks` / `Fenestra` / `publish.yml` / environment `release`) so the workflow can take over.

Tag the repo (`git tag v0.1.0 && git push --tags`) after a successful publish so users can pin git
deps to a known release.

## License

Fenestra is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

CEF/Chromium runtimes carry their own licenses and notices. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for release packaging notes.
