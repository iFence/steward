//! Windows application-icon extraction for launcher result rows.

#![cfg(target_os = "windows")]

use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
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

const SHELL_ICON_SIZE: i32 = 32;
const SETTINGS_EXECUTABLE: &str = r"%SystemRoot%\ImmersiveControlPanel\SystemSettings.exe";

#[cfg(test)]
pub(crate) static ICON_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn app_icon_data_url(path: &Path) -> Option<String> {
    let png = app_icon_png(path)?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

/// The Windows Settings app shares one executable across all pages. Extract
/// its shell icon once and reuse the encoded result for every Settings row.
pub(crate) fn settings_icon_data_url() -> Option<String> {
    static ICON: OnceLock<Option<String>> = OnceLock::new();
    ICON.get_or_init(|| app_icon_data_url(Path::new(SETTINGS_EXECUTABLE)))
        .clone()
}

fn app_icon_png(path: &Path) -> Option<Vec<u8>> {
    let text = path.to_string_lossy();
    if text.starts_with("shell:") {
        return shell_item_icon_png(&text);
    }
    if text.contains('!') && !text.chars().any(|ch| matches!(ch, '\\' | '/' | ':')) {
        return shell_item_icon_png(&format!("shell:AppsFolder\\{text}"));
    }

    let path = expand_env_vars(path);
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut info: SHFILEINFOW = std::mem::zeroed();
        let result = SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if result == 0 || info.hIcon.is_null() {
            return None;
        }
        let result = icon_to_rgba(info.hIcon).and_then(encode_png);
        DestroyIcon(info.hIcon);
        result
    }
}

fn shell_item_icon_png(parsing_name: &str) -> Option<Vec<u8>> {
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
    let result = bitmap_to_rgba(bitmap.0).and_then(encode_png);
    DeleteObject(bitmap.0);
    result
}

fn encode_png((width, height, rgba): (u32, u32, Vec<u8>)) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(out)
}

unsafe fn bitmap_to_rgba(bitmap: *mut core::ffi::c_void) -> Option<(u32, u32, Vec<u8>)> {
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
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width as i32;
    info.bmiHeader.biHeight = -(height as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;

    let mut bgra = vec![0u8; (width * height * 4) as usize];
    let lines = GetDIBits(
        dc,
        bitmap,
        0,
        height,
        bgra.as_mut_ptr() as *mut _,
        &mut info,
        DIB_RGB_COLORS,
    );
    SelectObject(dc, old);
    DeleteDC(dc);
    if lines == 0 {
        return None;
    }
    Some((width, height, bgra_to_rgba(bgra)))
}

unsafe fn icon_to_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(icon, &mut info) == 0 {
        return None;
    }

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

    let dc = CreateCompatibleDC(std::ptr::null_mut());
    if dc.is_null() {
        free_icon_info(info);
        return None;
    }
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32);
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
    let rgba = bgra_to_rgba(bgra.to_vec());
    DeleteObject(dib);
    DeleteDC(dc);
    free_icon_info(info);
    Some((width, height, rgba))
}

fn bgra_to_rgba(bgra: Vec<u8>) -> Vec<u8> {
    let mut rgba = vec![0u8; bgra.len()];
    for (dst, src) in rgba.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    rgba
}

fn expand_env_vars(path: &Path) -> PathBuf {
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
        out.push('%');
        rest = after;
    }
    out.push_str(rest);
    PathBuf::from(out)
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

    #[test]
    fn extracts_a_system_executable_icon_as_a_data_url() {
        let _guard = ICON_TEST_LOCK.lock().unwrap();
        let url =
            app_icon_data_url(Path::new(r"C:\Windows\System32\notepad.exe")).expect("notepad icon");
        let encoded = url
            .strip_prefix("data:image/png;base64,")
            .expect("PNG data URL");
        let png = STANDARD.decode(encoded).expect("valid base64");
        let image = image::load_from_memory(&png).expect("valid PNG");
        assert!(image.to_rgba8().pixels().any(|pixel| pixel.0[3] > 0));
    }

    #[test]
    fn extracts_the_windows_settings_icon() {
        let _guard = ICON_TEST_LOCK.lock().unwrap();
        let url = settings_icon_data_url().expect("Windows Settings icon");
        let encoded = url
            .strip_prefix("data:image/png;base64,")
            .expect("PNG data URL");
        let png = STANDARD.decode(encoded).expect("valid base64");
        let image = image::load_from_memory(&png).expect("valid PNG");
        assert!(image.to_rgba8().pixels().any(|pixel| pixel.0[3] > 0));
    }
}
