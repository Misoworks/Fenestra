//! Non-Linux stub implementations for desktop services.
//!
//! Linux exposes a rich set of platform integrations (tray icons, global
//! shortcuts, deep links, native messaging hosts, single-instance routing) via
//! the `desktop_services` module. Other platforms ship their own equivalents
//! through `stuk_platform` and do not need this Linux-specific wiring.
//!
//! These stubs let the rest of `fenestra-cef` compile and link on every
//! platform without dragging in Linux-only dependencies.

use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::{self, JoinHandle},
};

use stuk_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon,
};

#[derive(Debug, Default)]
pub struct LinuxDesktopServiceState;

impl LinuxDesktopServiceState {
    pub fn take_events(&self) -> Vec<PlatformEvent> {
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_linux_desktop_services(
    _tray: Option<&TrayIcon>,
    _autostart: &[AutostartEntry],
    _shortcuts: &[GlobalShortcutRegistration],
    _deep_links: &[DeepLinkRegistration],
    _native_messaging: &[NativeMessagingHost],
    _single_instance_id: Option<&str>,
    _single_instance_policy: Option<SingleInstancePolicy>,
) -> Result<LinuxDesktopServiceState, String> {
    Ok(LinuxDesktopServiceState)
}

pub fn start_desktop_event_forwarder<F>(
    _state: &LinuxDesktopServiceState,
    _running: Arc<AtomicBool>,
    _forwarder: F,
) -> JoinHandle<()>
where
    F: FnMut(PlatformEvent) + Send + 'static,
{
    thread::spawn(|| {})
}
