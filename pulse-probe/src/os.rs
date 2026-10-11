//! 权限与进程操作。

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, PWSTR, w};

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let token = OwnedHandle::from_raw_handle(token.0);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0;
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0
    }
}

/// 以管理员身份重新启动自己。成功返回 true，调用方随后应退出。
pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe: Vec<u16> = exe
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // 返回值 > 32 表示成功。
    r.0 as usize > 32
}

/// 打开进程用于查询。D3DKMT 的逐进程 GPU 统计要求完整的 PROCESS_QUERY_INFORMATION；
/// 拿不到时退回"有限查询"，至少还能取路径。
pub(crate) fn open_query(pid: u32) -> Option<OwnedHandle> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) }
        .or_else(|_| unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        .ok()?;
    Some(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

pub(crate) fn image_path(handle: &OwnedHandle) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            HANDLE(handle.as_raw_handle()),
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

pub fn kill(pid: u32) -> windows::core::Result<()> {
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, false, pid)?;
        let h = OwnedHandle::from_raw_handle(h.0);
        TerminateProcess(HANDLE(h.as_raw_handle()), 1)
    }
}

/// 在资源管理器中定位文件。
pub fn reveal_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn();
}

/// exe 的文件描述（资源管理器"属性 → 详细信息"里的那个），例如 Code.exe → "Visual Studio Code"。
pub(crate) fn file_description(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let path = PCWSTR(wide.as_ptr());
    let size = unsafe { GetFileVersionInfoSizeW(path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(path, None, size, data.as_mut_ptr().cast()) }.ok()?;

    let query = |sub: &str| -> Option<(*const u8, usize)> {
        let sub: Vec<u16> = sub.encode_utf16().chain([0]).collect();
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let ok = unsafe {
            VerQueryValueW(
                data.as_ptr().cast(),
                PCWSTR(sub.as_ptr()),
                &mut ptr,
                &mut len,
            )
        }
        .as_bool();
        (ok && !ptr.is_null() && len > 0).then_some((ptr as *const u8, len as usize))
    };
    // 先按文件自带的语言/代码页查，退回常见的英文与中文。
    let mut langs: Vec<String> = Vec::new();
    if let Some((ptr, len)) = query(r"\VarFileInfo\Translation") {
        for i in 0..len / 4 {
            let pair = unsafe { std::slice::from_raw_parts(ptr.add(i * 4) as *const u16, 2) };
            langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
        }
    }
    langs.extend(["040904b0", "040904e4", "080404b0", "000004b0"].map(String::from));
    for lang in langs {
        if let Some((ptr, len)) = query(&format!(r"\StringFileInfo\{lang}\FileDescription")) {
            let units = unsafe { std::slice::from_raw_parts(ptr as *const u16, len) };
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let s = String::from_utf16_lossy(&units[..end]).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// 进程命令行（Windows 8.1+ 的 ProcessCommandLineInformation，有限查询权限即可）。
pub(crate) fn command_line(handle: &OwnedHandle) -> Option<String> {
    use windows::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessCommandLineInformation,
    };

    let h = HANDLE(handle.as_raw_handle());
    let mut len = 0u32;
    unsafe {
        let _ = NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            std::ptr::null_mut(),
            0,
            &mut len,
        );
    }
    if len == 0 {
        return None;
    }
    // 结果是一个 UNICODE_STRING，其 Buffer 指向同一块缓冲区的后半部分。用 u64 保证对齐。
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    let status = unsafe {
        NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        )
    };
    if status.is_err() {
        return None;
    }
    let base = buf.as_ptr() as *const u8;
    let bytes = unsafe { (base as *const u16).read() } as usize;
    let ptr = unsafe { (base.add(8) as *const *const u16).read() };
    if ptr.is_null() || bytes == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, bytes / 2) });
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}
