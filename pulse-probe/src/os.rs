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
    let h = crate::control::open(pid, PROCESS_TERMINATE)?;
    unsafe { TerminateProcess(HANDLE(h.as_raw_handle()), 1) }
}

/// 在资源管理器中定位文件。
///
/// 不用 `explorer /select,<路径>`：路径含空格时 `Command` 会给整个参数加引号，
/// 变成 `"/select,C:\Program Files\..."`，资源管理器解析失败后打开默认文件夹（逗号同理）。
/// 改用 Shell API；定位不到这一项时退而打开所在文件夹。放到单独线程里做，
/// 资源管理器响应慢时不卡界面。
pub fn reveal_in_explorer(path: &str) {
    let path = path.to_owned();
    let _ = std::thread::Builder::new()
        .name("reveal".into())
        .spawn(move || reveal_blocking(&path));
}

fn reveal_blocking(path: &str) {
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};

    let wide = crate::wide(path);
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    let selected = unsafe {
        let pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
        if pidl.is_null() {
            false
        } else {
            let ok = SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
            ILFree(Some(pidl));
            ok
        }
    };
    if !selected
        && let Some(dir) = std::path::Path::new(path).parent()
        && dir.is_dir()
    {
        let dir = crate::wide(&dir.to_string_lossy());
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(dir.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
}

/// exe 的文件描述（资源管理器"属性 → 详细信息"里的那个），例如 Code.exe → "Visual Studio Code"。
pub(crate) fn file_description(path: &str) -> Option<String> {
    version_strings(path, &["FileDescription"]).pop().flatten()
}

/// 读取 exe 版本资源里的字符串，如 "FileDescription"、"CompanyName"。文件只读一次，按 `keys` 顺序返回。
pub fn version_strings(path: &str, keys: &[&str]) -> Vec<Option<String>> {
    version_strings_inner(path, keys).unwrap_or_else(|| vec![None; keys.len()])
}

fn version_strings_inner(path: &str, keys: &[&str]) -> Option<Vec<Option<String>>> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wide = crate::wide(path);
    let path = PCWSTR(wide.as_ptr());
    let size = unsafe { GetFileVersionInfoSizeW(path, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(path, None, size, data.as_mut_ptr().cast()) }.ok()?;

    let query = |sub: &str| -> Option<(*const u8, usize)> {
        let sub = crate::wide(sub);
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
    let lookup = |key: &str| {
        langs.iter().find_map(|lang| {
            let (ptr, len) = query(&format!(r"\StringFileInfo\{lang}\{key}"))?;
            let units = unsafe { std::slice::from_raw_parts(ptr as *const u16, len) };
            let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let s = String::from_utf16_lossy(&units[..end]).trim().to_string();
            (!s.is_empty()).then_some(s)
        })
    };
    Some(keys.iter().map(|k| lookup(k)).collect())
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

/// 系统的"打开文件"对话框，`owner` 是父窗口句柄。阻塞到用户选完，放在后台线程调用。
pub fn pick_file(owner: isize, title: &str) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoTaskMemFree, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_NODEREFERENCELINKS, FileOpenDialog, IFileOpenDialog,
        SIGDN_FILESYSPATH,
    };
    use windows::core::HSTRING;

    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    let result = unsafe {
        (|| {
            let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
            // 选中快捷方式时查的是快捷方式文件本身，不跳到它指向的目标。
            dlg.SetOptions(FOS_FORCEFILESYSTEM | FOS_NODEREFERENCELINKS)
                .ok()?;
            let _ = dlg.SetTitle(&HSTRING::from(title));
            let parent = (owner != 0).then_some(HWND(owner as *mut _));
            dlg.Show(parent).ok()?;
            let item = dlg.GetResult().ok()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let s = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            s
        })()
    };
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

/// FILETIME（100ns，自 1601 年起，UTC）→ 本地时间 `YYYY-MM-DD HH:MM`。
pub fn format_filetime(t: i64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ft = FILETIME {
        dwLowDateTime: t as u32,
        dwHighDateTime: (t >> 32) as u32,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return "—".into();
        }
    }
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
    )
}

/// 通过"替换任务管理器"启动时命令行里的标记。
pub const TASKMGR_FLAG: &str = "--taskmgr";

const IFEO_TASKMGR: &str =
    r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";

/// 是否已用 IFEO 的 Debugger 值把任务管理器（含 Ctrl+Shift+Esc）重定向到 Pulse。
pub fn taskmgr_replaced() -> bool {
    crate::system::reg_string(IFEO_TASKMGR, "Debugger")
        .is_some_and(|v| v.to_lowercase().contains("pulse"))
}

/// 打开或关闭"替换任务管理器"。写 HKLM，需要管理员。
pub fn set_taskmgr_replaced(on: bool) -> Result<(), String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPEN_CREATE_OPTIONS, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
    };
    let key_path = crate::wide(IFEO_TASKMGR);
    let mut key = HKEY::default();
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key_path.as_ptr()),
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
        return Err(crate::win32_message(r.0));
    }
    let r = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // 带上 --taskmgr：系统会把 taskmgr.exe 路径和开关附加在后面，Pulse 见到标记就忽略其后的参数。
        let value = format!("\"{}\" {TASKMGR_FLAG}", exe.display());
        let data: Vec<u8> = value
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        unsafe { RegSetValueExW(key, w!("Debugger"), None, REG_SZ, Some(&data)) }
    } else {
        unsafe { RegDeleteValueW(key, w!("Debugger")) }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if r == ERROR_SUCCESS || (!on && r.0 == 2) {
        Ok(())
    } else {
        Err(crate::win32_message(r.0))
    }
}

/// 控制台程序的输出（OEM 代码页，日文系统上是 Shift-JIS）转成字符串。
pub(crate) fn decode_console(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar,
    };
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let n = unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, None) };
    if n <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut buf = vec![0u16; n as usize];
    unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut buf)) };
    String::from_utf16_lossy(&buf)
}
