use std::{
    ffi::{c_char, c_int, c_uint, c_void},
    ptr,
};

use layershellev::WindowState;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

pub(super) struct LayerAlphaModifier {
    display: *mut WlDisplay,
    manager: *mut WlProxy,
    surface_modifier: *mut WlProxy,
    surface: *mut WlProxy,
}

impl LayerAlphaModifier {
    pub(super) fn bind(state: &WindowState<()>) -> Option<Self> {
        let display = display_ptr(state)?;
        let surface = surface_ptr(state)?;
        unsafe { bind_alpha_modifier(display, surface) }
    }

    pub(super) fn set_alpha(&self, alpha: f32) -> bool {
        if self.display.is_null() || self.surface_modifier.is_null() || self.surface.is_null() {
            return false;
        }
        let factor = (alpha.clamp(0.0, 1.0) * c_uint::MAX as f32).round() as c_uint;
        unsafe {
            wl_proxy_marshal_flags(
                self.surface_modifier,
                ALPHA_SURFACE_SET_MULTIPLIER,
                ptr::null(),
                1,
                0,
                factor,
            );
            wl_proxy_marshal_flags(self.surface, SURFACE_COMMIT, ptr::null(), 1, 0);
            wl_display_flush(self.display);
        }
        true
    }
}

impl Drop for LayerAlphaModifier {
    fn drop(&mut self) {
        unsafe {
            if !self.surface_modifier.is_null() {
                wl_proxy_marshal_flags(
                    self.surface_modifier,
                    ALPHA_SURFACE_DESTROY,
                    ptr::null(),
                    1,
                    DESTROY_FLAG,
                );
                self.surface_modifier = ptr::null_mut();
            }
            if !self.manager.is_null() {
                wl_proxy_marshal_flags(
                    self.manager,
                    ALPHA_MANAGER_DESTROY,
                    ptr::null(),
                    1,
                    DESTROY_FLAG,
                );
                self.manager = ptr::null_mut();
            }
        }
    }
}

fn display_ptr(state: &WindowState<()>) -> Option<*mut WlDisplay> {
    match state.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(display) => Some(display.display.as_ptr().cast()),
        _ => None,
    }
}

fn surface_ptr(state: &WindowState<()>) -> Option<*mut WlProxy> {
    match state.window_handle().ok()?.as_raw() {
        RawWindowHandle::Wayland(surface) => Some(surface.surface.as_ptr().cast()),
        _ => None,
    }
}

unsafe fn bind_alpha_modifier(
    display: *mut WlDisplay,
    surface: *mut WlProxy,
) -> Option<LayerAlphaModifier> {
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
        unsafe { wl_proxy_destroy(registry.cast()) };
        return None;
    }

    unsafe {
        wl_display_roundtrip(display);
        wl_display_roundtrip(display);
    }

    let Some(manager_name) = state.manager_name else {
        unsafe { wl_proxy_destroy(registry.cast()) };
        return None;
    };

    let manager = unsafe {
        wl_proxy_marshal_flags(
            registry.cast(),
            REGISTRY_BIND,
            &WP_ALPHA_MODIFIER_V1_INTERFACE,
            1,
            0,
            manager_name,
            WP_ALPHA_MODIFIER_V1_INTERFACE.name,
            1_u32,
            ptr::null::<c_void>(),
        )
    };
    unsafe { wl_proxy_destroy(registry.cast()) };
    if manager.is_null() {
        return None;
    }

    let surface_modifier = unsafe {
        wl_proxy_marshal_flags(
            manager.cast(),
            ALPHA_MANAGER_GET_SURFACE,
            &WP_ALPHA_MODIFIER_SURFACE_V1_INTERFACE,
            1,
            0,
            ptr::null::<c_void>(),
            surface,
        )
    };
    if surface_modifier.is_null() {
        unsafe { wl_proxy_destroy(manager.cast()) };
        return None;
    }

    Some(LayerAlphaModifier {
        display,
        manager: manager.cast(),
        surface_modifier,
        surface,
    })
}

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
    if interface.to_bytes() == ALPHA_MANAGER_INTERFACE_NAME {
        state.manager_name = Some(name);
    }
}

unsafe extern "C" fn registry_global_remove(
    _data: *mut c_void,
    _registry: *mut WlRegistry,
    _name: u32,
) {
}

#[derive(Default)]
struct RegistryState {
    manager_name: Option<u32>,
}

#[repr(C)]
struct RegistryListener {
    global: unsafe extern "C" fn(*mut c_void, *mut WlRegistry, u32, *const c_char, u32),
    global_remove: unsafe extern "C" fn(*mut c_void, *mut WlRegistry, u32),
}

static REGISTRY_LISTENER: RegistryListener = RegistryListener {
    global: registry_global,
    global_remove: registry_global_remove,
};

unsafe impl Sync for RegistryListener {}

type WlDisplay = c_void;
type WlRegistry = c_void;
type WlProxy = c_void;

#[repr(C)]
struct WlInterface {
    name: *const c_char,
    version: c_int,
    method_count: c_int,
    methods: *const WlMessage,
    event_count: c_int,
    events: *const WlMessage,
}

#[repr(C)]
struct WlMessage {
    name: *const c_char,
    signature: *const c_char,
    types: *const *const WlInterface,
}

#[repr(C)]
struct InterfaceTypes {
    manager_get_surface: [*const WlInterface; 2],
}

unsafe impl Sync for WlInterface {}
unsafe impl Sync for WlMessage {}
unsafe impl Sync for InterfaceTypes {}

const ALPHA_MANAGER_INTERFACE_NAME: &[u8] = b"wp_alpha_modifier_v1";
const DISPLAY_GET_REGISTRY: u32 = 1;
const REGISTRY_BIND: u32 = 0;
const SURFACE_COMMIT: u32 = 6;
const ALPHA_MANAGER_DESTROY: u32 = 0;
const ALPHA_MANAGER_GET_SURFACE: u32 = 1;
const ALPHA_SURFACE_DESTROY: u32 = 0;
const ALPHA_SURFACE_SET_MULTIPLIER: u32 = 1;
const DESTROY_FLAG: u32 = 1;

static INTERFACE_TYPES: InterfaceTypes = InterfaceTypes {
    manager_get_surface: [&WP_ALPHA_MODIFIER_SURFACE_V1_INTERFACE, unsafe {
        &WL_SURFACE_INTERFACE
    }],
};

static ALPHA_MANAGER_METHODS: [WlMessage; 2] = [
    WlMessage {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: ptr::null(),
    },
    WlMessage {
        name: c"get_surface".as_ptr(),
        signature: c"no".as_ptr(),
        types: INTERFACE_TYPES.manager_get_surface.as_ptr(),
    },
];

static ALPHA_SURFACE_METHODS: [WlMessage; 2] = [
    WlMessage {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: ptr::null(),
    },
    WlMessage {
        name: c"set_multiplier".as_ptr(),
        signature: c"u".as_ptr(),
        types: ptr::null(),
    },
];

static WP_ALPHA_MODIFIER_V1_INTERFACE: WlInterface = WlInterface {
    name: c"wp_alpha_modifier_v1".as_ptr(),
    version: 1,
    method_count: ALPHA_MANAGER_METHODS.len() as c_int,
    methods: ALPHA_MANAGER_METHODS.as_ptr(),
    event_count: 0,
    events: ptr::null(),
};

static WP_ALPHA_MODIFIER_SURFACE_V1_INTERFACE: WlInterface = WlInterface {
    name: c"wp_alpha_modifier_surface_v1".as_ptr(),
    version: 1,
    method_count: ALPHA_SURFACE_METHODS.len() as c_int,
    methods: ALPHA_SURFACE_METHODS.as_ptr(),
    event_count: 0,
    events: ptr::null(),
};

#[link(name = "wayland-client")]
unsafe extern "C" {
    #[link_name = "wl_surface_interface"]
    static WL_SURFACE_INTERFACE: WlInterface;
    #[link_name = "wl_registry_interface"]
    static WL_REGISTRY_INTERFACE: WlInterface;

    fn wl_display_roundtrip(display: *mut WlDisplay) -> c_int;
    fn wl_display_flush(display: *mut WlDisplay) -> c_int;
    fn wl_proxy_add_listener(
        proxy: *mut WlProxy,
        implementation: *mut c_void,
        data: *mut c_void,
    ) -> c_int;
    fn wl_proxy_get_version(proxy: *mut WlProxy) -> u32;
    fn wl_proxy_destroy(proxy: *mut WlProxy);
    fn wl_proxy_marshal_flags(
        proxy: *mut WlProxy,
        opcode: u32,
        interface: *const WlInterface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut WlProxy;
}
