//! 启动应用：注册表 Run 键与启动文件夹，启用状态读写 `Explorer\StartupApproved`（与任务管理器共用）。

use std::path::Path;

use crate::wide;
use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows::Win32::Storage::FileSystem::{SearchPathW, WIN32_FIND_DATAW};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IPersistFile, STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY,
    REG_EXPAND_SZ, REG_OPEN_CREATE_OPTIONS, REG_SZ, RegCloseKey, RegCreateKeyExW, RegEnumValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::UI::Shell::{
    FOLDERID_CommonStartup, FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{Interface, PCWSTR, PWSTR};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// HKCU\…\Run
    UserRegistry,
    /// HKLM\…\Run
    MachineRegistry,
    /// HKLM\WOW6432Node\…\Run（32 位程序）
    MachineRegistry32,
    /// 当前用户的启动文件夹
    UserFolder,
    /// 所有用户的启动文件夹
    CommonFolder,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::UserRegistry => crate::t!(
                "注册表（当前用户）",
                "Registry (current user)",
                "レジストリ (現在のユーザー)"
            ),
            Source::MachineRegistry => crate::t!(
                "注册表（所有用户）",
                "Registry (all users)",
                "レジストリ (全ユーザー)"
            ),
            Source::MachineRegistry32 => {
                crate::t!(
                    "注册表（所有用户，32 位）",
                    "Registry (all users, 32-bit)",
                    "レジストリ (全ユーザー、32 ビット)"
                )
            }
            Source::UserFolder => {
                crate::t!(
                    "启动文件夹（当前用户）",
                    "Startup folder (current user)",
                    "スタートアップ フォルダー (現在のユーザー)"
                )
            }
            Source::CommonFolder => {
                crate::t!(
                    "启动文件夹（所有用户）",
                    "Startup folder (all users)",
                    "スタートアップ フォルダー (全ユーザー)"
                )
            }
        }
    }

    /// 修改启用状态是否需要管理员。
    pub fn needs_admin(self) -> bool {
        matches!(
            self,
            Source::MachineRegistry | Source::MachineRegistry32 | Source::CommonFolder
        )
    }

    fn approved(self) -> (HKEY, &'static str) {
        match self {
            Source::UserRegistry => (HKEY_CURRENT_USER, "Run"),
            Source::MachineRegistry => (HKEY_LOCAL_MACHINE, "Run"),
            Source::MachineRegistry32 => (HKEY_LOCAL_MACHINE, "Run32"),
            Source::UserFolder => (HKEY_CURRENT_USER, "StartupFolder"),
            Source::CommonFolder => (HKEY_LOCAL_MACHINE, "StartupFolder"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StartupItem {
    /// 注册表值名或启动文件夹里的文件名（StartupApproved 里也用它做键）。
    pub key: String,
    /// 显示名：快捷方式去掉 .lnk。
    pub name: String,
    pub command: String,
    /// 解析出的可执行文件路径。
    pub path: Option<std::sync::Arc<str>>,
    pub source: Source,
    pub enabled: bool,
    /// 禁用的时刻（FILETIME）。
    pub disabled_at: Option<i64>,
    pub publisher: Option<String>,
    pub description: Option<String>,
}

pub fn list() -> Vec<StartupItem> {
    // 解析快捷方式要用 COM；重复初始化返回 S_FALSE，无害。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut out = Vec::new();
    for (root, key, source) in [
        (HKEY_CURRENT_USER, RUN, Source::UserRegistry),
        (HKEY_LOCAL_MACHINE, RUN, Source::MachineRegistry),
        (HKEY_LOCAL_MACHINE, RUN32, Source::MachineRegistry32),
    ] {
        for (name, command) in reg_values(root, key) {
            out.push(item(name.clone(), name, command, source));
        }
    }
    for (folder, source) in [
        (FOLDERID_Startup, Source::UserFolder),
        (FOLDERID_CommonStartup, Source::CommonFolder),
    ] {
        let Some(dir) = known_folder(&folder) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let full = e.path().to_string_lossy().to_string();
            let (name, command) = if file.to_lowercase().ends_with(".lnk") {
                let target = resolve_lnk(&full).unwrap_or_else(|| full.clone());
                (file[..file.len() - 4].to_string(), target)
            } else {
                (file.clone(), full)
            };
            out.push(item(file, name, command, source));
        }
    }
    out.sort_by_cached_key(|i| i.name.to_lowercase());
    out
}

fn item(key: String, name: String, command: String, source: Source) -> StartupItem {
    let (enabled, disabled_at) = approved_state(source, &key);
    let path = exe_path(&command).map(std::sync::Arc::<str>::from);
    let (publisher, description) = match path.as_deref() {
        Some(p) => {
            let mut v =
                crate::os::version_strings(p, &["CompanyName", "FileDescription"]).into_iter();
            (v.next().flatten(), v.next().flatten())
        }
        None => (None, None),
    };
    StartupItem {
        key,
        name,
        command,
        path,
        source,
        enabled,
        disabled_at,
        publisher,
        description,
    }
}

/// StartupApproved 里没有记录即视为启用；有记录时首字节为偶数（02、06）表示启用，奇数（03、07）表示禁用。
fn approved_state(source: Source, key: &str) -> (bool, Option<i64>) {
    let (root, sub) = source.approved();
    let Some(data) = reg_binary(root, &format!(r"{APPROVED}\{sub}"), key) else {
        return (true, None);
    };
    let enabled = data.first().is_none_or(|b| b & 1 == 0);
    let at = (!enabled && data.len() >= 12)
        .then(|| i64::from_le_bytes(data[4..12].try_into().unwrap()))
        .filter(|&t| t > 0);
    (enabled, at)
}

/// 启用或禁用。写法与任务管理器一致：12 字节，首字节 02/03，禁用时附带当前时间。
pub fn set_enabled(item: &StartupItem, enabled: bool) -> Result<(), String> {
    let (root, sub) = item.source.approved();
    let mut data = [0u8; 12];
    if enabled {
        data[0] = 0x02;
    } else {
        data[0] = 0x03;
        let ft: FILETIME = unsafe { GetSystemTimeAsFileTime() };
        let t = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        data[4..12].copy_from_slice(&t.to_le_bytes());
    }
    let path = wide(&format!(r"{APPROVED}\{sub}"));
    let mut key = HKEY::default();
    let r = unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPEN_CREATE_OPTIONS(0),
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(win32_message(r.0));
    }
    let name = wide(&item.key);
    let r = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_BINARY, Some(&data)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(win32_message(r.0))
    }
}

fn win32_message(code: u32) -> String {
    windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message()
}

fn open_key(root: HKEY, path: &str) -> Option<HKEY> {
    let w = wide(path);
    let mut key = HKEY::default();
    (unsafe { RegOpenKeyExW(root, PCWSTR(w.as_ptr()), None, KEY_READ, &mut key) } == ERROR_SUCCESS)
        .then_some(key)
}

/// 键下所有字符串值：(名称, 数据)。
fn reg_values(root: HKEY, path: &str) -> Vec<(String, String)> {
    let Some(key) = open_key(root, path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u16; 512];
        let mut name_len = name.len() as u32;
        let mut data = vec![0u8; 8192];
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        let r = unsafe {
            RegEnumValueW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            )
        };
        if r != ERROR_SUCCESS {
            break;
        }
        if kind != REG_SZ.0 && kind != REG_EXPAND_SZ.0 {
            continue;
        }
        let units: Vec<u16> = data[..data_len as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
        let value = String::from_utf16_lossy(&units[..end]);
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        if !value.trim().is_empty() {
            out.push((name, value));
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    out
}

fn reg_binary(root: HKEY, path: &str, value: &str) -> Option<Vec<u8>> {
    let key = open_key(root, path)?;
    let name = wide(value);
    let mut data = vec![0u8; 64];
    let mut len = data.len() as u32;
    let r = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut len),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    (r == ERROR_SUCCESS).then(|| data[..len as usize].to_vec())
}

fn known_folder(id: &windows::core::GUID) -> Option<String> {
    let p = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    let s = unsafe { p.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn resolve_lnk(path: &str) -> Option<String> {
    let link: IShellLinkW =
        unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let file: IPersistFile = link.cast().ok()?;
    let w = wide(path);
    unsafe { file.Load(PCWSTR(w.as_ptr()), STGM_READ) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut fd = WIN32_FIND_DATAW::default();
    unsafe { link.GetPath(&mut buf, &mut fd, 0) }.ok()?;
    let target = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
    let mut args = [0u16; 1024];
    let _ = unsafe { link.GetArguments(&mut args) };
    let args = String::from_utf16_lossy(&args[..args.iter().position(|&c| c == 0).unwrap_or(0)]);
    if target.is_empty() {
        return None;
    }
    Some(if args.is_empty() {
        format!("\"{target}\"")
    } else {
        format!("\"{target}\" {args}")
    })
}

fn expand(s: &str) -> String {
    let w = wide(s);
    let mut buf = vec![0u16; 2048];
    let n = unsafe { ExpandEnvironmentStringsW(PCWSTR(w.as_ptr()), Some(&mut buf)) } as usize;
    if n == 0 || n > buf.len() {
        return s.to_string();
    }
    String::from_utf16_lossy(&buf[..n.saturating_sub(1)])
}

/// 从命令行里找出可执行文件：展开环境变量，处理引号，裸文件名用 PATH 搜索。
pub fn exe_path(command: &str) -> Option<String> {
    let expanded = expand(command);
    let (file, _) = crate::control::split_command(&expanded);
    if file.is_empty() {
        return None;
    }
    if Path::new(&file).is_absolute() {
        if Path::new(&file).exists() {
            return Some(file);
        }
        // 没加引号的带空格路径：截到第一个 .exe 为止再试。
        let lower = expanded.to_lowercase();
        let end = lower.find(".exe")? + 4;
        let candidate = expanded[..end].trim_matches('"').to_string();
        return Path::new(&candidate).exists().then_some(candidate);
    }
    let w = wide(&file);
    let ext = wide(".exe");
    let mut buf = vec![0u16; 1024];
    let n = unsafe {
        SearchPathW(
            PCWSTR::null(),
            PCWSTR(w.as_ptr()),
            PCWSTR(ext.as_ptr()),
            Some(&mut buf),
            None,
        )
    } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_exe_paths() {
        assert!(exe_path("notepad").is_some_and(|p| p.to_lowercase().ends_with("notepad.exe")));
        assert!(
            exe_path(r#""%windir%\explorer.exe" /n"#)
                .is_some_and(|p| p.to_lowercase().ends_with("explorer.exe"))
        );
        assert_eq!(exe_path(r"C:\definitely\missing.exe"), None);
        let unquoted = r"C:\Program Files\Windows NT\Accessories\wordpad.exe --flag";
        if Path::new(r"C:\Program Files\Windows NT\Accessories\wordpad.exe").exists() {
            assert!(exe_path(unquoted).is_some());
        }
    }

    #[test]
    fn lists_without_panicking() {
        let items = list();
        for i in &items {
            assert!(!i.key.is_empty());
        }
    }
}
