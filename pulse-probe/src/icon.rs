//! 从 exe 里取指定尺寸的图标，转成 RGBA。

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, HBITMAP, HDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW,
};

pub struct Icon {
    pub size: usize,
    pub rgba: Vec<u8>,
}

pub fn extract(path: &str, size: i32) -> Option<Icon> {
    let mut name = [0u16; 260];
    let units: Vec<u16> = path.encode_utf16().collect();
    if units.len() >= name.len() {
        return None;
    }
    name[..units.len()].copy_from_slice(&units);

    let mut icons = [HICON::default()];
    let n = unsafe { PrivateExtractIconsW(&name, 0, size, size, Some(&mut icons), None, 0) };
    if n == 0 || n == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    let icon = to_rgba(icons[0], size);
    unsafe {
        let _ = DestroyIcon(icons[0]);
    }
    icon
}

fn to_rgba(icon: HICON, size: i32) -> Option<Icon> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let dc = unsafe { CreateCompatibleDC(None) };
    let color = read_bits(dc, info.hbmColor, size);
    let result = color.map(|mut px| {
        // 老式图标没有 alpha 通道，用 AND 掩码补：掩码为 0 的像素不透明。
        if px.chunks_exact(4).all(|p| p[3] == 0) {
            let mask = read_bits(dc, info.hbmMask, size);
            for (i, p) in px.chunks_exact_mut(4).enumerate() {
                p[3] = match &mask {
                    Some(m) if m[i * 4] != 0 => 0,
                    _ => 255,
                };
            }
        }
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2); // BGRA → RGBA
        }
        Icon {
            size: size as usize,
            rgba: px,
        }
    });
    unsafe {
        let _ = DeleteDC(dc);
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
    }
    result
}

fn read_bits(dc: HDC, bitmap: HBITMAP, size: i32) -> Option<Vec<u8>> {
    if bitmap.is_invalid() {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // 自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut px = vec![0u8; (size * size * 4) as usize];
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            size as u32,
            Some(px.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    (lines == size).then_some(px)
}
