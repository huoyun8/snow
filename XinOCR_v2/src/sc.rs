//! ScreenCapture 静态库的 Rust 绑定。
//!
//! 对应 C 头 `ScreenCapture_v2/Src/sc_api.h`。约定：
//! - 所有函数都必须在主（UI）线程调用 —— Ling/Direct2D 都绑在那个线程上；
//! - 回调也在该线程上同步触发；
//! - 回调里拿到的 `pixels` / `png` 只在回调期间有效，需要留存要自己拷。

#![allow(non_camel_case_types)]

use std::os::raw::{c_int, c_void};

pub const SC_OK: c_int = 0;
pub const SC_ERR_NOT_INIT: c_int = -1;
pub const SC_ERR_BUSY: c_int = -2;
pub const SC_ERR_NO_SELECTION: c_int = -3;
pub const SC_ERR_ENCODE: c_int = -4;
pub const SC_ERR_NO_PIXELS: c_int = -5;
pub const SC_ERR_INTERNAL: c_int = -6;

pub const SC_MODE_OCR: c_int = 0;
pub const SC_MODE_QR: c_int = 1;
pub const SC_MODE_ANNOT: c_int = 2;

pub type ScOnDone = extern "C" fn(
    result: c_int,
    mode: c_int,
    w: c_int,
    h: c_int,
    pixels: *const u8,
    png: *const u8,
    png_len: c_int,
    user: *mut c_void,
);
pub type ScOnCancel = extern "C" fn(mode: c_int, user: *mut c_void);

extern "C" {
    pub fn sc_init() -> c_int;
    pub fn sc_set_callbacks(on_done: Option<ScOnDone>, on_cancel: Option<ScOnCancel>, user: *mut c_void);
    pub fn sc_start_capture(mode: c_int) -> c_int;
    pub fn sc_cancel() -> c_int;
    pub fn sc_is_capturing() -> c_int;
    pub fn sc_free_buffer(p: *mut c_void);
    pub fn sc_dispose();
}

/// 把回调里那个 `(w, h, pixels)` 拷成 Rust 自己的 Vec（BGRA、top-down、行紧凑）
pub fn copy_bgra(w: c_int, h: c_int, pixels: *const u8) -> Option<(u32, u32, Vec<u8>)> {
    if w <= 0 || h <= 0 || pixels.is_null() {
        return None;
    }
    let len = (w as usize) * (h as usize) * 4;
    let mut v = vec![0u8; len];
    unsafe { std::ptr::copy_nonoverlapping(pixels, v.as_mut_ptr(), len) };
    Some((w as u32, h as u32, v))
}

/// 把回调里的 PNG 字节拷出来
pub fn copy_png(png: *const u8, png_len: c_int) -> Option<Vec<u8>> {
    if png.is_null() || png_len <= 0 {
        return None;
    }
    let mut v = vec![0u8; png_len as usize];
    unsafe { std::ptr::copy_nonoverlapping(png, v.as_mut_ptr(), png_len as usize) };
    Some(v)
}
