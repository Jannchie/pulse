//! 前台窗口与用户空闲时间，供使用时长统计。

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId, InternalGetWindowText,
};
use windows::core::{BOOL, PWSTR};

/// 当前前台窗口所属的程序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreground {
    pub pid: u32,
    /// exe 完整路径；受保护进程拿不到时为 None。
    pub path: Option<String>,
    pub title: String,
}

impl Foreground {
    /// exe 文件名（不含扩展名），例如 `Code`。
    pub fn stem(&self) -> Option<&str> {
        let path = self.path.as_deref()?;
        let name = path.rsplit(['\\', '/']).next()?;
        Some(
            name.len()
                .checked_sub(4)
                .filter(|&n| name[n..].eq_ignore_ascii_case(".exe"))
                .map_or(name, |n| &name[..n]),
        )
    }
}

/// 前台窗口所属的程序。UWP 应用的窗口属于 ApplicationFrameHost.exe，取其中真正应用的进程。
pub fn foreground() -> Option<Foreground> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    let mut path = image_path(pid);
    if path.as_deref().is_some_and(|p| {
        p.to_ascii_lowercase()
            .ends_with("\\applicationframehost.exe")
    }) && let Some(child) = uwp_child(hwnd, pid)
    {
        pid = child;
        path = image_path(pid);
    }
    Some(Foreground {
        pid,
        path,
        title: window_text(hwnd),
    })
}

/// 距离最后一次键盘或鼠标输入的毫秒数。
pub fn idle_ms() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return 0;
    }
    unsafe { GetTickCount() }.wrapping_sub(info.dwTime)
}

fn image_path(pid: u32) -> Option<String> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let h = unsafe { OwnedHandle::from_raw_handle(h.0) };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            HANDLE(h.as_raw_handle()),
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// 窗口标题。InternalGetWindowText 不向目标窗口发消息，对方无响应时也不会卡住。
fn window_text(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { InternalGetWindowText(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// ApplicationFrameHost 窗口里属于其他进程的子窗口（UWP 应用的 CoreWindow）。
fn uwp_child(hwnd: HWND, host: u32) -> Option<u32> {
    struct Search {
        host: u32,
        found: u32,
    }
    unsafe extern "system" fn each(child: HWND, l: LPARAM) -> BOOL {
        let s = unsafe { &mut *(l.0 as *mut Search) };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(child, Some(&mut pid)) };
        if pid != 0 && pid != s.host {
            s.found = pid;
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut s = Search { host, found: 0 };
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd),
            Some(each),
            LPARAM(&mut s as *mut Search as isize),
        );
    }
    (s.found != 0).then_some(s.found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_strips_exe() {
        let f = |p: &str| Foreground {
            pid: 1,
            path: Some(p.into()),
            title: String::new(),
        };
        assert_eq!(f(r"C:\a\Code.exe").stem(), Some("Code"));
        assert_eq!(
            f(r"C:\a\PotPlayerMini64.EXE").stem(),
            Some("PotPlayerMini64")
        );
        assert_eq!(f(r"C:\a\noext").stem(), Some("noext"));
    }

    #[test]
    fn foreground_and_idle_do_not_panic() {
        let _ = foreground();
        let _ = idle_ms();
    }
}
