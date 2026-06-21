# Agent Instructions

- Use `bun` for JavaScript package management.
- Fenestra runtime detection, downloads, validation, manifests, locks, pruning, and runtime paths live in `crates/fenestra-runtime`.
- CEF host source, CEF process launch, browser profiles, bridge transport, and webview backend code live in `crates/fenestra-cef`.
- The WebView2 / Evergreen backend for Windows lives in `crates/fenestra-webview2`. On `target_os = "windows"` it owns the `FenestraWindow` alias in `fenestra-cef`; on every other platform `fenestra-cef`'s `CefWindow` is the alias target.
- `fenestra-platform` owns lightweight window/platform/shell types, compositor regions, and Wayland background-effect support shared by the CEF and WebView2 backends.
- Fenestra crates must not depend on Stuk crates. If a primitive is required by Fenestra, keep it in Fenestra-owned crates or modules.
- Stuk must not depend on Fenestra.
- The `stuk-fenestra` adapter crate has been removed. Apps that previously used it should use `FenestraWindow` from `fenestra-cef` directly.
- Run `cargo fmt`, `cargo build --workspace`, and `cargo test --workspace` after code changes. For Windows-only changes, also run `cargo check --target x86_64-pc-windows-gnu --workspace` since the host development environment is typically Linux.
- When publishing, use `scripts/publish.sh`. Crate metadata lives in each crate's `Cargo.toml`; the workspace owns version, license, repository, homepage, authors, keywords, categories.
