//! Windows app-icon extraction for launcher results.
//!
//! M1 resolves each scanned .lnk to its target .exe, so the shell's large
//! icon for that file is a good proxy for the application icon. The icon is
//! drawn into a 32-bit DIB (transparency from the AND mask / alpha channel),
//! converted to RGBA, PNG-encoded and wrapped in a `gpui::Image` so the
//! results list can render it directly.

#![cfg(target_os = "windows")]

use std::{os::windows::ffi::OsStrExt, path::Path, sync::Arc};

use image::ImageEncoder;
use windows_sys::Win32::{
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    },
    UI::{
        Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON},
        WindowsAndMessaging::{DestroyIcon, DrawIconEx, GetIconInfo, DI_NORMAL, HICON, ICONINFO},
    },
};

/// Extract the shell icon for `path` as a renderable `gpui::Image`, or `None`
/// when the file has no icon.
pub fn app_icon_image(path: &Path) -> Option<Arc<gpui::Image>> {
    let png = app_icon_png(path)?;
    Some(Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Png,
        png,
    )))
}

/// Extract the shell's large icon for `path` and encode it as PNG bytes.
fn app_icon_png(path: &Path) -> Option<Vec<u8>> {
    // Shortcut targets may keep environment variables (e.g. %windir%) even
    // after resolution; expand them so the shell can find the file.
    let path = expand_env_vars(path);
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        let result = SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if result == 0 || shfi.hIcon.is_null() {
            return None;
        }

        let (width, height, rgba) = icon_to_rgba(shfi.hIcon)?;
        DestroyIcon(shfi.hIcon);

        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
            .ok()?;
        Some(out)
    }
}

/// Expand `%VAR%` tokens in a path using the process environment, leaving
/// unknown tokens intact.
fn expand_env_vars(path: &Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    if !text.contains('%') {
        return path.to_path_buf();
    }

    let mut out = String::new();
    let mut rest = text.as_ref();
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            if let Ok(value) = std::env::var(name) {
                out.push_str(&value);
                rest = &after[end + 1..];
                continue;
            }
        }
        // No closing token or unknown variable: keep the `%` literally.
        out.push('%');
        rest = after;
    }
    out.push_str(rest);
    std::path::PathBuf::from(out)
}

/// Convert an `HICON` into top-down RGBA pixels at the icon's native size.
unsafe fn icon_to_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }

    // The color plane holds the pixels; old monochrome icons only have the
    // mask, which is twice as tall (color + mask rows).
    let hbm = if info.hbmColor.is_null() {
        info.hbmMask
    } else {
        info.hbmColor
    };
    let mut bmp: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        hbm as _,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bmp as *mut _ as *mut _,
    );
    if got == 0 {
        free_icon_info(info);
        return None;
    }

    let width = bmp.bmWidth as u32;
    let height = if info.hbmColor.is_null() {
        (bmp.bmHeight as u32) / 2
    } else {
        bmp.bmHeight as u32
    };
    if width == 0 || height == 0 {
        free_icon_info(info);
        return None;
    }

    // Draw the icon into a zero-initialized 32-bit DIB: transparent areas
    // (mask holes / alpha 0) keep their background, so the result has real
    // transparency for both modern alpha icons and legacy masked icons.
    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        free_icon_info(info);
        return None;
    }
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32); // top-down rows
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() {
        DeleteDC(dc);
        free_icon_info(info);
        return None;
    }
    std::ptr::write_bytes(bits, 0, (width * height * 4) as usize);

    let old = SelectObject(dc, dib as _);
    DrawIconEx(
        dc,
        0,
        0,
        icon,
        width as i32,
        height as i32,
        0,
        std::ptr::null_mut(),
        DI_NORMAL,
    );
    SelectObject(dc, old);

    let bgra = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
    let mut rgba = vec![0u8; bgra.len()];
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }

    DeleteObject(dib);
    DeleteDC(dc);
    free_icon_info(info);

    Some((width, height, rgba))
}

unsafe fn free_icon_info(info: ICONINFO) {
    if !info.hbmColor.is_null() {
        DeleteObject(info.hbmColor as _);
    }
    if !info.hbmMask.is_null() {
        DeleteObject(info.hbmMask as _);
    }
}
