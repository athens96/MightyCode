//! The part of libghostty's C API (`ghostty.h` of the vendored GhosttyKit.xcframework, upstream c4e16970a803, the library the Swift
//! app links) the terminal view calls: the app and its runtime callbacks, a surface on an NSView with the host-managed I/O backend
//! (the host owns the PTY: Ghostty hands back what the user types through `receive_buffer` and the grid through `receive_resize`, and
//! is fed the PTY's output through `ghostty_surface_write_buffer`), drawing, input, the clipboard requests and reading the grid back.
//! Layouts follow the header field for field (the Step 0 spike's `sys.rs`, proven on CI, plus what the view needs beyond it).

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_int, c_void};

pub type ghostty_app_t = *mut c_void;
pub type ghostty_config_t = *mut c_void;
pub type ghostty_surface_t = *mut c_void;

pub const GHOSTTY_PLATFORM_MACOS: c_int = 1;
pub const GHOSTTY_SURFACE_IO_BACKEND_HOST_MANAGED: c_int = 1;
pub const GHOSTTY_SURFACE_CONTEXT_WINDOW: c_int = 0;

pub const GHOSTTY_CLIPBOARD_STANDARD: c_int = 0;
pub const GHOSTTY_CLIPBOARD_REQUEST_PASTE: c_int = 0;
pub const GHOSTTY_CLIPBOARD_READ_STARTED: c_int = 0;
pub const GHOSTTY_CLIPBOARD_READ_UNAVAILABLE: c_int = 1;
pub const GHOSTTY_CLIPBOARD_READ_UNSUPPORTED: c_int = 2;

pub const GHOSTTY_MOUSE_RELEASE: c_int = 0;
pub const GHOSTTY_MOUSE_PRESS: c_int = 1;
pub const GHOSTTY_MOUSE_LEFT: c_int = 1;
pub const GHOSTTY_MOUSE_RIGHT: c_int = 2;
pub const GHOSTTY_MOUSE_MIDDLE: c_int = 3;

pub const GHOSTTY_COLOR_SCHEME_LIGHT: c_int = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: c_int = 1;

pub const GHOSTTY_ACTION_RELEASE: c_int = 0;
pub const GHOSTTY_ACTION_PRESS: c_int = 1;
pub const GHOSTTY_ACTION_REPEAT: c_int = 2;

pub const GHOSTTY_POINT_VIEWPORT: c_int = 1;
pub const GHOSTTY_POINT_COORD_EXACT: c_int = 0;

pub const GHOSTTY_TARGET_SURFACE: c_int = 1;

/// `ghostty_action_mouse_shape_e` values the view shows a cursor for.
pub const GHOSTTY_MOUSE_SHAPE_DEFAULT: c_int = 0;
pub const GHOSTTY_MOUSE_SHAPE_POINTER: c_int = 3;
pub const GHOSTTY_MOUSE_SHAPE_CELL: c_int = 6;
pub const GHOSTTY_MOUSE_SHAPE_CROSSHAIR: c_int = 7;
pub const GHOSTTY_MOUSE_SHAPE_TEXT: c_int = 8;
pub const GHOSTTY_MOUSE_SHAPE_VERTICAL_TEXT: c_int = 9;
pub const GHOSTTY_MOUSE_SHAPE_NO_DROP: c_int = 13;
pub const GHOSTTY_MOUSE_SHAPE_NOT_ALLOWED: c_int = 14;
pub const GHOSTTY_MOUSE_SHAPE_COL_RESIZE: c_int = 18;
pub const GHOSTTY_MOUSE_SHAPE_ROW_RESIZE: c_int = 19;
pub const GHOSTTY_MOUSE_SHAPE_EW_RESIZE: c_int = 28;
pub const GHOSTTY_MOUSE_SHAPE_NS_RESIZE: c_int = 29;

/// `ghostty_action_tag_e` values the view answers.
pub const ACTION_CELL_SIZE: c_int = 25;
pub const ACTION_RENDER: c_int = 27;
pub const ACTION_DESKTOP_NOTIFICATION: c_int = 32;
pub const ACTION_SET_TITLE: c_int = 33;
pub const ACTION_PWD: c_int = 37;
pub const ACTION_MOUSE_SHAPE: c_int = 38;
pub const ACTION_RING_BELL: c_int = 52;
pub const ACTION_OPEN_URL: c_int = 57;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_diagnostic_s {
    pub message: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_env_var_s {
    pub key: *const c_char,
    pub value: *const c_char,
}

pub type ReceiveBuffer = unsafe extern "C" fn(*mut c_void, *const u8, usize);
pub type ReceiveResize = unsafe extern "C" fn(*mut c_void, u16, u16, u32, u32);

/// `ghostty_surface_config_s`; `platform` is the union of `{void* nsview}` and `{void* uiview}`.
#[repr(C)]
pub struct ghostty_surface_config_s {
    pub platform_tag: c_int,
    pub platform: *mut c_void,
    pub userdata: *mut c_void,
    pub backend: c_int,
    pub receive_userdata: *mut c_void,
    pub receive_buffer: Option<ReceiveBuffer>,
    pub receive_resize: Option<ReceiveResize>,
    pub scale_factor: f64,
    pub font_size: f32,
    pub working_directory: *const c_char,
    pub command: *const c_char,
    pub env_vars: *mut ghostty_env_var_s,
    pub env_var_count: usize,
    pub initial_input: *const c_char,
    pub wait_after_command: bool,
    pub context: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ghostty_surface_size_s {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

/// `ghostty_target_s`: a tag and the union `{ghostty_surface_t surface}`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_target_s {
    pub tag: c_int,
    pub surface: ghostty_surface_t,
}

/// `ghostty_action_s`: a tag and the union `ghostty_action_u`, whose largest members are 24 bytes aligned to 8.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_action_s {
    pub tag: c_int,
    pub payload: [u64; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_clipboard_content_s {
    pub mime: *const c_char,
    pub data: *const c_char,
    pub len: usize,
}

#[repr(C)]
pub struct ghostty_clipboard_complete_s {
    pub contents: *const ghostty_clipboard_content_s,
    pub contents_len: usize,
    pub available: *const *const c_char,
    pub available_len: usize,
    pub confirmed: bool,
    pub remember: bool,
}

#[repr(C)]
pub struct ghostty_clipboard_confirm_s {
    pub contents: *const ghostty_clipboard_content_s,
    pub contents_len: usize,
    pub available: *const *const c_char,
    pub available_len: usize,
    pub name: *const c_char,
    pub can_remember: bool,
}

pub type WakeupCb = unsafe extern "C" fn(*mut c_void);
pub type ActionCb = unsafe extern "C" fn(ghostty_app_t, ghostty_target_s, ghostty_action_s) -> bool;
pub type ReadClipboardCb = unsafe extern "C" fn(*mut c_void, c_int, *mut c_void, *const *const c_char, usize, bool) -> c_int;
pub type ConfirmReadClipboardCb = unsafe extern "C" fn(*mut c_void, *const ghostty_clipboard_confirm_s, *mut c_void, c_int);
pub type WriteClipboardCb = unsafe extern "C" fn(*mut c_void, c_int, *const ghostty_clipboard_content_s, usize, bool);
pub type CloseSurfaceCb = unsafe extern "C" fn(*mut c_void, bool);

#[repr(C)]
pub struct ghostty_runtime_config_s {
    pub userdata: *mut c_void,
    pub supports_selection_clipboard: bool,
    pub wakeup_cb: Option<WakeupCb>,
    pub action_cb: Option<ActionCb>,
    pub read_clipboard_cb: Option<ReadClipboardCb>,
    pub confirm_read_clipboard_cb: Option<ConfirmReadClipboardCb>,
    pub write_clipboard_cb: Option<WriteClipboardCb>,
    pub close_surface_cb: Option<CloseSurfaceCb>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_point_s {
    pub tag: c_int,
    pub coord: c_int,
    pub x: u32,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_selection_s {
    pub top_left: ghostty_point_s,
    pub bottom_right: ghostty_point_s,
    pub rectangle: bool,
}

#[repr(C)]
pub struct ghostty_text_s {
    pub tl_px_x: f64,
    pub tl_px_y: f64,
    pub offset_start: u32,
    pub offset_len: u32,
    pub text: *const c_char,
    pub text_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ghostty_input_key_s {
    pub action: c_int,
    pub mods: c_int,
    pub consumed_mods: c_int,
    pub keycode: u32,
    pub text: *const c_char,
    pub unshifted_codepoint: u32,
    pub composing: bool,
}

unsafe extern "C" {
    pub fn ghostty_init(argc: usize, argv: *mut *mut c_char) -> c_int;
    pub fn ghostty_config_new() -> ghostty_config_t;
    pub fn ghostty_config_free(config: ghostty_config_t);
    pub fn ghostty_config_load_file(config: ghostty_config_t, path: *const c_char);
    pub fn ghostty_config_finalize(config: ghostty_config_t);
    pub fn ghostty_config_diagnostics_count(config: ghostty_config_t) -> u32;
    pub fn ghostty_config_get_diagnostic(config: ghostty_config_t, index: u32) -> ghostty_diagnostic_s;
    pub fn ghostty_app_new(runtime: *const ghostty_runtime_config_s, config: ghostty_config_t) -> ghostty_app_t;
    pub fn ghostty_app_free(app: ghostty_app_t);
    pub fn ghostty_app_tick(app: ghostty_app_t);
    pub fn ghostty_app_set_focus(app: ghostty_app_t, focused: bool);
    pub fn ghostty_app_update_config(app: ghostty_app_t, config: ghostty_config_t);
    pub fn ghostty_app_set_color_scheme(app: ghostty_app_t, scheme: c_int);
    pub fn ghostty_surface_config_new() -> ghostty_surface_config_s;
    pub fn ghostty_surface_new(app: ghostty_app_t, config: *const ghostty_surface_config_s) -> ghostty_surface_t;
    pub fn ghostty_surface_free(surface: ghostty_surface_t);
    pub fn ghostty_surface_userdata(surface: ghostty_surface_t) -> *mut c_void;
    pub fn ghostty_surface_update_config(surface: ghostty_surface_t, config: ghostty_config_t);
    pub fn ghostty_surface_write_buffer(surface: ghostty_surface_t, bytes: *const u8, len: usize);
    pub fn ghostty_surface_refresh(surface: ghostty_surface_t);
    pub fn ghostty_surface_draw(surface: ghostty_surface_t);
    pub fn ghostty_surface_set_content_scale(surface: ghostty_surface_t, x: f64, y: f64);
    pub fn ghostty_surface_set_focus(surface: ghostty_surface_t, focused: bool);
    pub fn ghostty_surface_set_occlusion(surface: ghostty_surface_t, visible: bool);
    pub fn ghostty_surface_set_size(surface: ghostty_surface_t, width: u32, height: u32);
    pub fn ghostty_surface_size(surface: ghostty_surface_t) -> ghostty_surface_size_s;
    pub fn ghostty_surface_set_color_scheme(surface: ghostty_surface_t, scheme: c_int);
    pub fn ghostty_surface_key_translation_mods(surface: ghostty_surface_t, mods: c_int) -> c_int;
    pub fn ghostty_surface_key(surface: ghostty_surface_t, key: ghostty_input_key_s) -> bool;
    pub fn ghostty_surface_key_is_binding(surface: ghostty_surface_t, key: ghostty_input_key_s, flags: *mut c_int) -> bool;
    pub fn ghostty_surface_text(surface: ghostty_surface_t, text: *const c_char, len: usize);
    pub fn ghostty_surface_preedit(surface: ghostty_surface_t, text: *const c_char, len: usize);
    pub fn ghostty_surface_mouse_button(surface: ghostty_surface_t, state: c_int, button: c_int, mods: c_int) -> bool;
    pub fn ghostty_surface_mouse_pos(surface: ghostty_surface_t, x: f64, y: f64, mods: c_int);
    pub fn ghostty_surface_mouse_scroll(surface: ghostty_surface_t, x: f64, y: f64, mods: c_int);
    pub fn ghostty_surface_ime_point(surface: ghostty_surface_t, x: *mut f64, y: *mut f64, width: *mut f64, height: *mut f64);
    pub fn ghostty_surface_binding_action(surface: ghostty_surface_t, action: *const c_char, len: usize) -> bool;
    pub fn ghostty_surface_complete_clipboard_request(
        surface: ghostty_surface_t,
        complete: *const ghostty_clipboard_complete_s,
        state: *mut c_void,
    );
    pub fn ghostty_surface_deny_clipboard_request(surface: ghostty_surface_t, state: *mut c_void);
    pub fn ghostty_surface_has_selection(surface: ghostty_surface_t) -> bool;
    pub fn ghostty_surface_read_selection(surface: ghostty_surface_t, out: *mut ghostty_text_s) -> bool;
    pub fn ghostty_surface_read_text(surface: ghostty_surface_t, selection: ghostty_selection_s, out: *mut ghostty_text_s) -> bool;
    pub fn ghostty_surface_free_text(surface: ghostty_surface_t, text: *mut ghostty_text_s);
}
