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
use windows::core::{Interface, PCWSTR};
use windows::Win32::{
    Foundation::SIZE,
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
    UI::Shell::{
        IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_ICONONLY,
    },
};
use windows_sys::Win32::{
    Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDIBits, GetObjectW,
        SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
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
    let text = path.to_string_lossy();
    // `shell:` parsing names (UWP aliases from the AppsFolder scan, ...) have
    // no file system path, so `SHGetFileInfoW` cannot resolve them; the shell
    // item image factory handles them natively.
    if text.starts_with("shell:") {
        return shell_item_icon_png(&text);
    }

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

/// Requested icon size for shell items. The shell may return a larger bitmap
/// (`SIIGBF_BIGGERSIZEOK`), which GPUI scales down to the row's 24px icon.
const SHELL_ICON_SIZE: i32 = 32;

/// Extract the icon for a shell parsing name (`shell:AppsFolder\...` UWP
/// aliases, ...) via `IShellItemImageFactory`. Returns PNG bytes, or `None`
/// when the item exposes no icon.
fn shell_item_icon_png(parsing_name: &str) -> Option<Vec<u8>> {
    // The image factory is a COM object. GPUI's Windows platform calls
    // `OleInitialize` at startup, but keep this function self-contained (and
    // unit-testable): initialize COM for this thread when it is not already
    // initialized, balancing every successful init (including `S_FALSE`).
    let co_init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let initialized = co_init.is_ok();
    let result = unsafe { shell_item_icon_png_inner(parsing_name) };
    if initialized {
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn shell_item_icon_png_inner(parsing_name: &str) -> Option<Vec<u8>> {
    let wide: Vec<u16> = parsing_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
    let factory: IShellItemImageFactory = item.cast().ok()?;
    let bitmap = factory
        .GetImage(
            SIZE {
                cx: SHELL_ICON_SIZE,
                cy: SHELL_ICON_SIZE,
            },
            SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
        )
        .ok()?;
    let (width, height, rgba) = bitmap_to_rgba(bitmap.0)?;
    DeleteObject(bitmap.0);

    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(out)
}

/// Convert a shell-provided `HBITMAP` (a 32-bit DIB section carrying an alpha
/// channel) into top-down RGBA pixels at its native size.
unsafe fn bitmap_to_rgba(bitmap: *mut core::ffi::c_void) -> Option<(u32, u32, Vec<u8>)> {
    // Resolve dimensions with GetObjectW: it works for both device-dependent
    // bitmaps and DIB sections, whereas the two-pass GetDIBits idiom leaves
    // the header unpopulated for some shell-provided bitmaps.
    let mut bmp: BITMAP = std::mem::zeroed();
    let got = GetObjectW(
        bitmap,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bmp as *mut _ as *mut _,
    );
    if got == 0 {
        return None;
    }
    let width = bmp.bmWidth.unsigned_abs();
    let height = bmp.bmHeight.unsigned_abs();
    if width == 0 || height == 0 {
        return None;
    }

    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        return None;
    }
    let old = SelectObject(dc, bitmap);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut bgra = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        height,
        bgra.as_mut_ptr() as *mut _,
        &mut bmi,
        DIB_RGB_COLORS,
    );
    SelectObject(dc, old);
    DeleteDC(dc);
    if lines == 0 {
        return None;
    }

    let mut rgba = vec![0u8; bgra.len()];
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    Some((width, height, rgba))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a PNG payload and report whether it contains any non-transparent
    /// pixels, i.e. the shell actually drew an icon.
    fn png_has_visible_pixels(png: &[u8]) -> bool {
        let image = image::load_from_memory(png).expect("icon should decode as PNG");
        let rgba = image.to_rgba8();
        rgba.pixels().any(|p| p[3] > 0)
    }

    #[test]
    fn extracts_icon_for_shell_apps_folder_alias() {
        // Every `shell:AppsFolder\...` alias is a parsing name that
        // `SHGetFileInfoW` cannot resolve; each must still yield an icon via
        // the shell item image factory. Enumerate the real aliases instead of
        // hardcoding a package AUMID (which varies per install).
        use windows::Win32::{
            System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED},
            UI::Shell::{BHID_EnumItems, IEnumShellItems, SIGDN},
        };
        const SIGDN_FORPARSING: SIGDN = SIGDN(0x80018000u32 as i32);

        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let apps_folder: Vec<u16> = "shell:AppsFolder"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let folder: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(apps_folder.as_ptr()), None)
                    .expect("bind to AppsFolder");
            let enumerator: IEnumShellItems = folder
                .BindToHandler(None, &BHID_EnumItems)
                .expect("enumerate AppsFolder");

            let mut aliases = 0;
            let mut icons = 0;
            loop {
                let mut one = [None];
                if enumerator.Next(&mut one, None).is_err() {
                    break;
                }
                let Some(item) = one[0].take() else {
                    break;
                };
                let Ok(parsing) = item.GetDisplayName(SIGDN_FORPARSING) else {
                    continue;
                };
                let text = parsing.to_string().unwrap_or_default();
                CoTaskMemFree(Some(parsing.0 as _));
                // AppsFolder parsing names are usually bare AUMIDs; normalize
                // them the same way the scanner does before extraction.
                let is_alias = text.starts_with("shell:AppsFolder\\")
                    || (!text.contains('\\') && !text.contains('{') && !text.contains('}'));
                if !is_alias {
                    continue;
                }
                let parsing = if text.starts_with("shell:") {
                    text
                } else {
                    format!("shell:AppsFolder\\{text}")
                };
                aliases += 1;
                if let Some(png) = shell_item_icon_png(&parsing) {
                    if png_has_visible_pixels(&png) {
                        icons += 1;
                    }
                }
            }
            eprintln!("AppsFolder aliases: {aliases}, icons extracted: {icons}");
            assert!(aliases > 0, "no AppsFolder aliases found");
            assert!(icons > 0, "no icons extracted from AppsFolder aliases");
        }
    }

    #[test]
    fn extracts_icon_for_system_executable() {
        let png = app_icon_png(std::path::Path::new(r"C:\Windows\System32\notepad.exe"))
            .expect("exe icon");
        assert!(png_has_visible_pixels(&png), "icon must not be empty");
    }
}
