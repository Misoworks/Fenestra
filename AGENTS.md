# Agent Instructions

- Use `bun` for JavaScript package management.
- Fenestra runtime detection, downloads, validation, manifests, locks, pruning, and runtime paths live in `crates/fenestra-runtime`.
- CEF host source, CEF process launch, browser profiles, bridge transport, and webview backend code live in `crates/fenestra-cef`.
- Stuk integration lives in `crates/stuk-fenestra`.
- Stuk must not depend on Fenestra. Fenestra core crates must not depend on Stuk.
- `stuk-fenestra` may depend on both Fenestra and Stuk.
- Run `cargo fmt`, `cargo build --workspace`, and `cargo test --workspace` after code changes.

