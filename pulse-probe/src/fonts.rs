//! 已安装字体清单：HKLM 与 HKCU 下的 `Windows NT\CurrentVersion\Fonts`。
//!
//! 系统字体的值只有文件名（在 `%WINDIR%\Fonts`），按用户安装的字体是完整路径
//! （通常在 `%LOCALAPPDATA%\Microsoft\Windows\Fonts`）。

use std::path::PathBuf;

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

const FONTS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts";

/// 注册表里登记的字体：(显示名, 文件路径)。显示名形如
/// `Yu Gothic Medium & Yu Gothic UI Regular (TrueType)`，一个 .ttc 的多个字面用 `&` 连起来。
pub fn installed() -> Vec<(String, PathBuf)> {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let system = PathBuf::from(windir).join("Fonts");
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .flat_map(|root| crate::startup::reg_values(root, FONTS))
        .map(|(name, file)| {
            let file = PathBuf::from(file.trim());
            let path = if file.is_absolute() {
                file
            } else {
                system.join(file)
            };
            (name, path)
        })
        .collect()
}
