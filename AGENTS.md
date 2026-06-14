# Agent Instructions

- Use `bun` for JavaScript package management.
- Fenestra runtime detection, downloads, validation, manifests, locks, pruning, and runtime paths live in `crates/fenestra-runtime`.
- CEF host source, CEF process launch, browser profiles, bridge transport, and webview backend code live in `crates/fenestra-cef`.
- `fenestra-cef` re-exports lightweight window/platform types from `stuk-platform` and `stuk-platform-shell`. These four upstream stuk crates (`stuk-platform`, `stuk-platform-shell`, `stuk-render`, `stuk-style`) are part of Fenestra's publish set and live in the sibling `stuk` repo.
- Fenestra core crates must not depend on the higher-level Stuk app framework crates (`stuk`, `stuk-widgets`, `stuk-text`, etc.). Only the four shared platform/render/style crates are allowed dependencies.
- Stuk must not depend on Fenestra.
- The `stuk-fenestra` adapter crate has been removed. Apps that previously used it should use `FenestraWindow` from `fenestra-cef` directly.
- Run `cargo fmt`, `cargo build --workspace`, and `cargo test --workspace` after code changes.
- When publishing, use `scripts/publish.sh` (publish stuk repo first, then fenestra repo). Crate metadata lives in each crate's `Cargo.toml`; the workspace owns version, license, repository, homepage, authors, keywords, categories.

