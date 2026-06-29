//! Engine-neutral Fenestra bridge, activity, and web page IPC primitives.
//!
//! `fenestra-bridge` is the load-bearing crate for the bridge protocol that
//! lets a Fenestra webview call into the host process. It is shared by the
//! CEF and WebView2 backends so the wire format, the JS surface, the
//! activity registry, and the security model have one source of truth.
//!
//! Crates that depend on `fenestra-bridge`:
//!
//! - `fenestra-cef` — drives the C++ CEF host over stdio; re-exports the
//!   bridge types from this crate to keep the historical public surface
//!   stable.
//! - `fenestra-webview2` — drives WebView2 in-process; defines the
//!   [`ActivityEventEmitter`] implementation that talks to
//!   `ICoreWebView2::ExecuteScript`.
//! - Apps depend on `fenestra-cef` (which re-exports the bridge surface).

pub mod activity;
pub mod bridge;
pub mod metrics;
pub mod web_bridge;

pub use activity::{
    ActivityEventEmitter, ActivityHostUpdate, ActivityOptions, ActivityRecord, ActivityRegistry,
    FenestraActivityLease, POPUP_CLOSE_COMMAND, POPUP_OPEN_COMMAND, bridge_commands_with_internal,
    host_update_json,
};
pub use bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeHandlers, BridgeRegistry,
    BridgeResponse, BridgeResult, BridgeRuntime, WebViewSecurity, current_bridge_targets,
};
pub use metrics::{
    FENESTRA_TRACE_ENV, FenestraLaunchMetric, FenestraLaunchMetricsSnapshot, LaunchMetrics,
};
pub use web_bridge::{
    BRIDGE_SCHEME, BridgeRequest, INSTALL_SCRIPT, WINDOW_SCHEME, WindowCommand, bridge_url,
    install_script, parse_bridge_url,
};
