use std::ffi::{c_char, c_int, c_void};
use std::ptr;

pub(super) type WlDisplay = c_void;
pub(super) type WlRegistry = c_void;
pub(super) type WlProxy = c_void;

#[repr(C)]
pub(super) struct WlInterface {
    pub(super) name: *const c_char,
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

unsafe impl Sync for WlInterface {}
unsafe impl Sync for WlMessage {}
unsafe impl Sync for InterfaceTypes {}

pub(super) const MANAGER_INTERFACE_NAME: &[u8] = b"ext_background_effect_manager_v1";
pub(super) const COMPOSITOR_INTERFACE_NAME: &[u8] = b"wl_compositor";
pub(super) const DISPLAY_GET_REGISTRY: u32 = 1;
pub(super) const REGISTRY_BIND: u32 = 0;
pub(super) const COMPOSITOR_CREATE_REGION: u32 = 1;
pub(super) const SURFACE_SET_OPAQUE_REGION: u32 = 4;
pub(super) const SURFACE_SET_INPUT_REGION: u32 = 5;
pub(super) const SURFACE_COMMIT: u32 = 6;
pub(super) const REGION_DESTROY: u32 = 0;
pub(super) const REGION_ADD: u32 = 1;
pub(super) const MANAGER_DESTROY: u32 = 0;
pub(super) const MANAGER_GET_BACKGROUND_EFFECT: u32 = 1;
pub(super) const MANAGER_CAPABILITY_BLUR: u32 = 1;
pub(super) const EFFECT_DESTROY: u32 = 0;
pub(super) const EFFECT_SET_BLUR_REGION: u32 = 1;
pub(super) const DESTROY_FLAG: u32 = 1;

#[repr(C)]
struct InterfaceTypes {
    manager_get_background_effect: [*const WlInterface; 2],
    effect_set_blur_region: [*const WlInterface; 1],
}

static INTERFACE_TYPES: InterfaceTypes = InterfaceTypes {
    manager_get_background_effect: [&EXT_BACKGROUND_EFFECT_SURFACE_V1_INTERFACE, unsafe {
        &WL_SURFACE_INTERFACE
    }],
    effect_set_blur_region: [unsafe { &WL_REGION_INTERFACE }],
};

static MANAGER_METHODS: [WlMessage; 2] = [
    WlMessage {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: ptr::null(),
    },
    WlMessage {
        name: c"get_background_effect".as_ptr(),
        signature: c"no".as_ptr(),
        types: INTERFACE_TYPES.manager_get_background_effect.as_ptr(),
    },
];

static MANAGER_EVENTS: [WlMessage; 1] = [WlMessage {
    name: c"capabilities".as_ptr(),
    signature: c"u".as_ptr(),
    types: ptr::null(),
}];

static EFFECT_METHODS: [WlMessage; 2] = [
    WlMessage {
        name: c"destroy".as_ptr(),
        signature: c"".as_ptr(),
        types: ptr::null(),
    },
    WlMessage {
        name: c"set_blur_region".as_ptr(),
        signature: c"?o".as_ptr(),
        types: INTERFACE_TYPES.effect_set_blur_region.as_ptr(),
    },
];

pub(super) static EXT_BACKGROUND_EFFECT_MANAGER_V1_INTERFACE: WlInterface = WlInterface {
    name: c"ext_background_effect_manager_v1".as_ptr(),
    version: 1,
    method_count: MANAGER_METHODS.len() as c_int,
    methods: MANAGER_METHODS.as_ptr(),
    event_count: MANAGER_EVENTS.len() as c_int,
    events: MANAGER_EVENTS.as_ptr(),
};

pub(super) static EXT_BACKGROUND_EFFECT_SURFACE_V1_INTERFACE: WlInterface = WlInterface {
    name: c"ext_background_effect_surface_v1".as_ptr(),
    version: 1,
    method_count: EFFECT_METHODS.len() as c_int,
    methods: EFFECT_METHODS.as_ptr(),
    event_count: 0,
    events: ptr::null(),
};

#[link(name = "wayland-client")]
unsafe extern "C" {
    #[link_name = "wl_surface_interface"]
    pub(super) static WL_SURFACE_INTERFACE: WlInterface;
    #[link_name = "wl_compositor_interface"]
    pub(super) static WL_COMPOSITOR_INTERFACE: WlInterface;
    #[link_name = "wl_registry_interface"]
    pub(super) static WL_REGISTRY_INTERFACE: WlInterface;
    #[link_name = "wl_region_interface"]
    pub(super) static WL_REGION_INTERFACE: WlInterface;

    pub(super) fn wl_display_roundtrip(display: *mut WlDisplay) -> c_int;
    pub(super) fn wl_display_flush(display: *mut WlDisplay) -> c_int;
    pub(super) fn wl_proxy_add_listener(
        proxy: *mut WlProxy,
        implementation: *mut c_void,
        data: *mut c_void,
    ) -> c_int;
    pub(super) fn wl_proxy_get_version(proxy: *mut WlProxy) -> u32;
    pub(super) fn wl_proxy_destroy(proxy: *mut WlProxy);
    pub(super) fn wl_proxy_marshal_flags(
        proxy: *mut WlProxy,
        opcode: u32,
        interface: *const WlInterface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut WlProxy;
}
