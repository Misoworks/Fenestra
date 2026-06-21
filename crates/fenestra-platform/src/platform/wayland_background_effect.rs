use std::sync::Arc;

use winit::window::Window;

use crate::WindowOptions;
#[cfg(target_os = "linux")]
use crate::{WindowBackgroundEffect, WindowRegion};

#[cfg(target_os = "linux")]
use std::ffi::{c_char, c_uint, c_void};
#[cfg(target_os = "linux")]
use std::ptr;

#[cfg(target_os = "linux")]
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

#[cfg(target_os = "linux")]
#[path = "wayland_background_effect_protocol.rs"]
mod wayland_background_effect_protocol;
#[cfg(target_os = "linux")]
use wayland_background_effect_protocol::*;

#[cfg(target_os = "linux")]
pub fn request(window: &Arc<dyn Window>, options: &WindowOptions) -> Option<WaylandEffect> {
    let wants_blur =
        options.transparent && options.background_effect != WindowBackgroundEffect::None;
    if !wants_blur && options.regions.is_empty() {
        debug("skipped: background effect and surface regions were not requested");
        return None;
    }
    let Some(display) = wayland_display(window) else {
        debug("skipped: native window is not backed by a Wayland display");
        return None;
    };
    let Some(surface) = wayland_surface(window) else {
        debug("skipped: native window is not backed by a Wayland surface");
        return None;
    };
    debug("requesting ext_background_effect_v1");
    unsafe {
        ExtBackgroundEffect::bind(
            display,
            surface,
            options.background_effect,
            options.width as i32,
            options.height as i32,
            wants_blur,
            options,
        )
    }
}

#[cfg(not(target_os = "linux"))]
pub fn request(_window: &Arc<dyn Window>, _options: &WindowOptions) -> Option<WaylandEffect> {
    None
}

#[derive(Debug)]
#[cfg(target_os = "linux")]
pub struct WaylandEffect {
    display: *mut WlDisplay,
    surface: *mut WlProxy,
    effect: *mut WlProxy,
    manager: *mut WlProxy,
    compositor: *mut WlProxy,
    _manager_state: Box<ManagerState>,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub struct WaylandEffect;

#[cfg(not(target_os = "linux"))]
impl WaylandEffect {
    pub fn update(&self, _options: &WindowOptions, _width: i32, _height: i32) -> bool {
        false
    }
}

#[cfg(target_os = "linux")]
impl Drop for WaylandEffect {
    fn drop(&mut self) {
        unsafe {
            if !self.effect.is_null() {
                wl_proxy_marshal_flags(self.effect, EFFECT_DESTROY, ptr::null(), 1, DESTROY_FLAG);
                self.effect = ptr::null_mut();
            }
            if !self.manager.is_null() {
                wl_proxy_marshal_flags(self.manager, MANAGER_DESTROY, ptr::null(), 1, DESTROY_FLAG);
                self.manager = ptr::null_mut();
            }
            if !self.compositor.is_null() {
                wl_proxy_destroy(self.compositor.cast());
                self.compositor = ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl WaylandEffect {
    pub fn update(&self, options: &WindowOptions, width: i32, height: i32) -> bool {
        if self.display.is_null() || self.surface.is_null() || self.compositor.is_null() {
            return false;
        }
        unsafe {
            apply_surface_regions(
                self.display,
                self.surface,
                self.compositor,
                self.effect,
                options,
                width,
                height,
            )
        }
    }
}

#[cfg(target_os = "linux")]
fn wayland_display(window: &Arc<dyn Window>) -> Option<*mut WlDisplay> {
    match window.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(display) => Some(display.display.as_ptr().cast()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn wayland_surface(window: &Arc<dyn Window>) -> Option<*mut WlProxy> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Wayland(surface) => Some(surface.surface.as_ptr().cast()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
struct ExtBackgroundEffect;

#[cfg(target_os = "linux")]
impl ExtBackgroundEffect {
    unsafe fn bind(
        display: *mut WlDisplay,
        surface: *mut WlProxy,
        _effect: WindowBackgroundEffect,
        width: i32,
        height: i32,
        wants_blur: bool,
        options: &WindowOptions,
    ) -> Option<WaylandEffect> {
        if display.is_null() || surface.is_null() {
            return None;
        }

        let registry = unsafe {
            wl_proxy_marshal_flags(
                display.cast(),
                DISPLAY_GET_REGISTRY,
                &WL_REGISTRY_INTERFACE,
                wl_proxy_get_version(display.cast()),
                0,
                ptr::null::<c_void>(),
            )
        };
        if registry.is_null() {
            debug("failed to create wl_registry");
            return None;
        }

        let mut state = RegistryState::default();
        let add_listener = unsafe {
            wl_proxy_add_listener(
                registry.cast(),
                &REGISTRY_LISTENER as *const RegistryListener as *mut _,
                &mut state as *mut RegistryState as *mut c_void,
            )
        };
        if add_listener != 0 {
            debug("failed to add wl_registry listener");
            unsafe { wl_proxy_destroy(registry.cast()) };
            return None;
        }

        unsafe {
            wl_display_roundtrip(display);
            wl_display_roundtrip(display);
        }

        let Some(compositor_name) = state.compositor_name else {
            debug("wl_compositor was not advertised");
            unsafe { wl_proxy_destroy(registry.cast()) };
            return None;
        };

        let compositor = unsafe {
            wl_proxy_marshal_flags(
                registry.cast(),
                REGISTRY_BIND,
                &WL_COMPOSITOR_INTERFACE,
                1,
                0,
                compositor_name,
                WL_COMPOSITOR_INTERFACE.name,
                1_u32,
                ptr::null::<c_void>(),
            )
        };
        if compositor.is_null() {
            debug("failed to bind wl_compositor");
            unsafe { wl_proxy_destroy(registry.cast()) };
            return None;
        }

        let mut manager_state = Box::<ManagerState>::default();
        let mut manager = ptr::null_mut();
        let mut effect_proxy = ptr::null_mut();
        if wants_blur {
            let Some(manager_name) = state.manager_name else {
                debug("ext_background_effect_manager_v1 was not advertised");
                unsafe { wl_proxy_destroy(registry.cast()) };
                unsafe { wl_proxy_destroy(compositor.cast()) };
                return None;
            };
            debug("binding ext_background_effect_manager_v1");
            manager = unsafe {
                wl_proxy_marshal_flags(
                    registry.cast(),
                    REGISTRY_BIND,
                    &EXT_BACKGROUND_EFFECT_MANAGER_V1_INTERFACE,
                    1,
                    0,
                    manager_name,
                    EXT_BACKGROUND_EFFECT_MANAGER_V1_INTERFACE.name,
                    1_u32,
                    ptr::null::<c_void>(),
                )
            };
            if manager.is_null() {
                debug("failed to bind ext_background_effect_manager_v1");
                unsafe { wl_proxy_destroy(registry.cast()) };
                unsafe { wl_proxy_destroy(compositor.cast()) };
                return None;
            }
            debug("bound ext_background_effect_manager_v1");

            let add_manager_listener = unsafe {
                wl_proxy_add_listener(
                    manager.cast(),
                    &MANAGER_LISTENER as *const ManagerListener as *mut _,
                    manager_state.as_mut() as *mut ManagerState as *mut c_void,
                )
            };
            if add_manager_listener != 0 {
                debug("failed to add ext_background_effect_manager_v1 listener");
                unsafe { wl_proxy_destroy(registry.cast()) };
                unsafe { wl_proxy_destroy(compositor.cast()) };
                unsafe { wl_proxy_destroy(manager.cast()) };
                return None;
            }
            unsafe {
                wl_display_roundtrip(display);
            }
            if !manager_state.supports_blur() {
                debug("ext_background_effect_manager_v1 does not advertise blur capability");
                unsafe { wl_proxy_destroy(registry.cast()) };
                unsafe { wl_proxy_destroy(compositor.cast()) };
                unsafe { wl_proxy_destroy(manager.cast()) };
                return None;
            }

            effect_proxy = unsafe {
                wl_proxy_marshal_flags(
                    manager.cast(),
                    MANAGER_GET_BACKGROUND_EFFECT,
                    &EXT_BACKGROUND_EFFECT_SURFACE_V1_INTERFACE,
                    1,
                    0,
                    ptr::null::<c_void>(),
                    surface,
                )
            };
            if effect_proxy.is_null() {
                debug("failed to create ext_background_effect_surface_v1");
                unsafe { wl_proxy_destroy(registry.cast()) };
                unsafe { wl_proxy_destroy(compositor.cast()) };
                unsafe { wl_proxy_destroy(manager.cast()) };
                return None;
            }
            debug("created ext_background_effect_surface_v1");
        }
        unsafe { wl_proxy_destroy(registry.cast()) };

        unsafe {
            apply_surface_regions(
                display,
                surface,
                compositor,
                effect_proxy,
                options,
                width,
                height,
            )
        };
        debug("committed ext_background_effect_surface_v1");

        Some(WaylandEffect {
            display,
            surface,
            effect: effect_proxy,
            manager: manager.cast(),
            compositor: compositor.cast(),
            _manager_state: manager_state,
        })
    }
}

#[cfg(target_os = "linux")]
unsafe fn apply_surface_regions(
    display: *mut WlDisplay,
    surface: *mut WlProxy,
    compositor: *mut WlProxy,
    effect_proxy: *mut WlProxy,
    options: &WindowOptions,
    width: i32,
    height: i32,
) -> bool {
    if !effect_proxy.is_null() {
        let blur = options
            .regions
            .blur
            .clone()
            .unwrap_or_else(WindowRegion::adaptive_full);
        let Some(region) = (unsafe { create_region(compositor, &blur, width, height) }) else {
            debug("failed to create blur wl_region");
            return false;
        };
        unsafe {
            wl_proxy_marshal_flags(
                effect_proxy,
                EFFECT_SET_BLUR_REGION,
                ptr::null(),
                1,
                0,
                region,
            );
            wl_proxy_marshal_flags(region, REGION_DESTROY, ptr::null(), 1, DESTROY_FLAG);
        }
    }

    if let Some(opaque) = &options.regions.opaque
        && let Some(region) = (unsafe { create_region(compositor, opaque, width, height) })
    {
        unsafe {
            wl_proxy_marshal_flags(
                surface,
                SURFACE_SET_OPAQUE_REGION,
                ptr::null(),
                wl_proxy_get_version(surface),
                0,
                region,
            );
            wl_proxy_marshal_flags(region, REGION_DESTROY, ptr::null(), 1, DESTROY_FLAG);
        }
    }

    if let Some(input) = &options.regions.input
        && let Some(region) = (unsafe { create_region(compositor, input, width, height) })
    {
        unsafe {
            wl_proxy_marshal_flags(
                surface,
                SURFACE_SET_INPUT_REGION,
                ptr::null(),
                wl_proxy_get_version(surface),
                0,
                region,
            );
            wl_proxy_marshal_flags(region, REGION_DESTROY, ptr::null(), 1, DESTROY_FLAG);
        }
    }

    unsafe {
        wl_proxy_marshal_flags(
            surface,
            SURFACE_COMMIT,
            ptr::null(),
            wl_proxy_get_version(surface),
            0,
        );
        wl_display_flush(display);
    }
    true
}

#[cfg(target_os = "linux")]
unsafe fn create_region(
    compositor: *mut WlProxy,
    region: &WindowRegion,
    width: i32,
    height: i32,
) -> Option<*mut WlProxy> {
    let proxy = unsafe {
        wl_proxy_marshal_flags(
            compositor,
            COMPOSITOR_CREATE_REGION,
            &WL_REGION_INTERFACE,
            1,
            0,
            ptr::null::<c_void>(),
        )
    };
    if proxy.is_null() {
        return None;
    }

    for rect in region.resolved_rects(width, height) {
        unsafe {
            wl_proxy_marshal_flags(
                proxy,
                REGION_ADD,
                ptr::null(),
                1,
                0,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
            );
        }
    }
    Some(proxy)
}

#[cfg(target_os = "linux")]
fn debug(message: &str) {
    if std::env::var_os("STUK_WAYLAND_EFFECT_DEBUG").is_some() {
        eprintln!("stuk wayland background effect: {message}");
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct RegistryState {
    manager_name: Option<u32>,
    compositor_name: Option<u32>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct ManagerState {
    capabilities: u32,
}

#[cfg(target_os = "linux")]
impl ManagerState {
    fn supports_blur(&self) -> bool {
        self.capabilities & MANAGER_CAPABILITY_BLUR != 0
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn registry_global(
    data: *mut c_void,
    _registry: *mut WlRegistry,
    name: u32,
    interface: *const c_char,
    _version: u32,
) {
    if data.is_null() || interface.is_null() {
        return;
    }
    let state = unsafe { &mut *(data.cast::<RegistryState>()) };
    let interface = unsafe { std::ffi::CStr::from_ptr(interface) };
    if interface.to_bytes() == MANAGER_INTERFACE_NAME {
        state.manager_name = Some(name);
    } else if interface.to_bytes() == COMPOSITOR_INTERFACE_NAME {
        state.compositor_name = Some(name);
    }
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn registry_global_remove(
    _data: *mut c_void,
    _registry: *mut WlRegistry,
    _name: u32,
) {
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct RegistryListener {
    global: unsafe extern "C" fn(*mut c_void, *mut WlRegistry, u32, *const c_char, u32),
    global_remove: unsafe extern "C" fn(*mut c_void, *mut WlRegistry, u32),
}

#[cfg(target_os = "linux")]
static REGISTRY_LISTENER: RegistryListener = RegistryListener {
    global: registry_global,
    global_remove: registry_global_remove,
};

#[cfg(target_os = "linux")]
unsafe impl Sync for RegistryListener {}

#[cfg(target_os = "linux")]
unsafe extern "C" fn manager_capabilities(
    data: *mut c_void,
    _manager: *mut WlProxy,
    flags: c_uint,
) {
    if data.is_null() {
        return;
    }
    let state = unsafe { &mut *(data.cast::<ManagerState>()) };
    state.capabilities = flags;
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct ManagerListener {
    capabilities: unsafe extern "C" fn(*mut c_void, *mut WlProxy, c_uint),
}

#[cfg(target_os = "linux")]
static MANAGER_LISTENER: ManagerListener = ManagerListener {
    capabilities: manager_capabilities,
};

#[cfg(target_os = "linux")]
unsafe impl Sync for ManagerListener {}
