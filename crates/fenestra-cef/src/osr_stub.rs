//! Non-Linux stub for the OSR (off-screen rendering) native host.
//!
//! Fenestra uses an OSR host on Linux to support transparency, blur,
//! shell-surface palettes, frameless windows, and other Wayland-specific
//! composition features. The implementation depends on Linux-only crates
//! (`layershellev`, `wayland-client`, Unix domain sockets), so on other
//! platforms this module compiles to a tiny stub.

use std::path::Path;

use crate::{
    BridgeHandlers, FenestraError, FenestraProcess, FenestraResult, FenestraWindowConfig,
    metrics::LaunchMetrics,
};

pub(crate) fn run_from_args(_args: &[String]) -> bool {
    false
}

pub(crate) fn launch_process(
    _runtime_dir: &Path,
    _config: &FenestraWindowConfig,
    _bridge_handlers: &BridgeHandlers,
    _url: &str,
    _metrics: LaunchMetrics,
) -> FenestraResult<FenestraProcess> {
    Err(FenestraError::CreationFailed {
        message: "Fenestra OSR host is currently only available on Linux".to_string(),
    })
}
