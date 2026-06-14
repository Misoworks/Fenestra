# Fenestra

Fenestra owns shared web runtime management and embedded webview backends.

Runtime files live under:

```txt
~/.local/share/fenestra/runtimes/cef/
```

For a practical map of the crates, window modes, bridge, activity leases, lifecycle, desktop
services, dev workflow, and bundling, see
[`docs/implementation-guide.md`](docs/implementation-guide.md).

## Install

From crates.io:

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

- `fenestra-runtime` — runtime discovery, install, and pruning, without the CEF launcher
- `fenestra-cli` — the `fenestra` binary (`cargo install fenestra-cli`)

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
cargo run -p fenestra-notes -- --system
cargo run -p fenestra-notes -- --fenestra-chrome
cargo run -p fenestra-notes -- --frameless
cargo run -p fenestra-notes -- --glass
cargo run -p stuk-notes
```

Create a standalone app:

```sh
cargo run -p fenestra-cli --bin fenestra -- new my-notes
```

Register a source checkout as a local desktop app:

```sh
cargo run -p fenestra-cli --bin fenestra -- install
cargo run -p fenestra-cli --bin fenestra -- install . --autostart
cargo run -p fenestra-cli --bin fenestra -- update
cargo run -p fenestra-cli --bin fenestra -- update --all
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
cargo run -p fenestra-cli --bin fenestra -- bundle . --target portable
cargo run -p fenestra-cli --bin fenestra -- bundle . --target deb --release
cargo run -p fenestra-cli --bin fenestra -- bundle . --target appimage
cargo run -p fenestra-cli --bin fenestra -- bundle . --target dmg --binary target/aarch64-apple-darwin/release/my-app
cargo run -p fenestra-cli --bin fenestra -- bundle . --target msi --binary target/x86_64-pc-windows-msvc/release/my-app.exe
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
FENESTRA_TRACE=1 cargo run -p fenestra-notes -- --glass
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
`stuk-render`, `stuk-style`) that ship from the sibling [stuk](https://github.com/Misoworks/stuk)
repo.

Publish in this order:

```sh
# 1. From the stuk repo, publish the four upstream crates (and their helpers).
cd ../stuk
scripts/publish.sh --dry-run    # sanity check
scripts/publish.sh              # actual publish

# 2. From this repo, publish the three fenestra crates.
cd ../fenestra
scripts/publish.sh --dry-run
scripts/publish.sh
```

`scripts/publish.sh` publishes in dependency order and waits between crates so the crates.io index
can refresh. Tag the repo (`git tag v0.1.0 && git push --tags`) after a successful publish so users
can pin git deps to a known release.

## License

Fenestra is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

CEF/Chromium runtimes carry their own licenses and notices. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for release packaging notes.
