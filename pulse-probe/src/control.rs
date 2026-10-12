//! 进程管理操作：窗口（应用识别、无响应检测、切换到、正常关闭）、挂起/恢复、优先级、
//! 处理器相关性、效率模式、转储、属性、运行新任务。

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{GENERIC_WRITE, HANDLE, HWND, LPARAM, NTSTATUS, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWithFullMemory, MiniDumpWithHandleData, MiniDumpWithThreadInfo, MiniDumpWriteDump,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetProcessAffinityMask,
    GetProcessInformation, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_SUSPEND_RESUME, PROCESS_VM_READ, ProcessPowerThrottling,
    REALTIME_PRIORITY_CLASS, SetPriorityClass, SetProcessAffinityMask, SetProcessInformation,
};
use windows::Win32::UI::Shell::{SHOP_FILEPATH, SHObjectProperties, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible,
    PostMessageW, SW_RESTORE, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WM_CLOSE,
    WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, w};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> NTSTATUS;
    fn NtResumeProcess(process: HANDLE) -> NTSTATUS;
}

pub type Result<T> = windows::core::Result<T>;

pub(crate) fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle> {
    let h = unsafe { OpenProcess(access, false, pid)? };
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

// ---------------------------------------------------------------- 窗口

/// 进程的主窗口：可见、无所有者、非工具窗口、有标题、未被 DWM 隐藏（UWP 后台窗口会被 cloak）。
#[derive(Clone, Debug)]
pub struct AppWindow {
    pub hwnd: isize,
    pub title: String,
    /// 窗口 5 秒没有处理消息（与任务管理器"未响应"同一判据）。
    pub hung: bool,
}

/// pid → 主窗口（同一进程有多个时取第一个）。
pub fn app_windows() -> HashMap<u32, AppWindow> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        out.push(hwnd);
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    let mut out = HashMap::new();
    for hwnd in all {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool()
                || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid())
            {
                continue;
            }
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                continue;
            }
            let mut cloaked = 0u32;
            let _ =
                DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
            if cloaked != 0 {
                continue;
            }
            let len = GetWindowTextLengthW(hwnd);
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            out.entry(pid).or_insert(AppWindow {
                hwnd: hwnd.0 as isize,
                title: String::from_utf16_lossy(&buf[..n as usize]),
                hung: IsHungAppWindow(hwnd).as_bool(),
            });
        }
    }
    out
}

/// 把窗口切到前台（最小化的先还原）。
pub fn switch_to(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

/// 向进程的所有顶层可见窗口发 WM_CLOSE，让程序自己决定是否保存、退出。返回发出的窗口数。
pub fn close_windows(pid: u32) -> usize {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let (pid, out) = unsafe { &mut *(lparam.0 as *mut (u32, Vec<HWND>)) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == *pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            out.push(hwnd);
        }
        BOOL(1)
    }
    let mut state = (pid, Vec::<HWND>::new());
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut state as *mut (u32, Vec<HWND>) as isize),
        );
    }
    state
        .1
        .iter()
        .filter(|&&h| unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok())
        .count()
}

// ---------------------------------------------------------------- 挂起 / 恢复

pub fn suspend(pid: u32) -> Result<()> {
    let h = open(pid, PROCESS_SUSPEND_RESUME)?;
    unsafe { NtSuspendProcess(raw(&h)) }.ok()
}

pub fn resume(pid: u32) -> Result<()> {
    let h = open(pid, PROCESS_SUSPEND_RESUME)?;
    unsafe { NtResumeProcess(raw(&h)) }.ok()
}

// ---------------------------------------------------------------- 优先级

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    Idle,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
    Realtime,
}

impl Priority {
    pub const ALL: [Priority; 6] = [
        Priority::Realtime,
        Priority::High,
        Priority::AboveNormal,
        Priority::Normal,
        Priority::BelowNormal,
        Priority::Idle,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Priority::Idle => crate::t!("低", "Low", "低"),
            Priority::BelowNormal => crate::t!("低于正常", "Below normal", "通常以下"),
            Priority::Normal => crate::t!("正常", "Normal", "通常"),
            Priority::AboveNormal => crate::t!("高于正常", "Above normal", "通常以上"),
            Priority::High => crate::t!("高", "High", "高"),
            Priority::Realtime => crate::t!("实时", "Realtime", "リアルタイム"),
        }
    }

    /// 由进程的基本优先级（SYSTEM_PROCESS_INFORMATION.BasePriority）反推优先级类。
    pub fn from_base(base: i32) -> Self {
        match base {
            ..=5 => Priority::Idle,
            6..=7 => Priority::BelowNormal,
            8..=9 => Priority::Normal,
            10..=12 => Priority::AboveNormal,
            13..=23 => Priority::High,
            _ => Priority::Realtime,
        }
    }

    fn class(self) -> PROCESS_CREATION_FLAGS {
        match self {
            Priority::Idle => IDLE_PRIORITY_CLASS,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Priority::High => HIGH_PRIORITY_CLASS,
            Priority::Realtime => REALTIME_PRIORITY_CLASS,
        }
    }
}

pub fn set_priority(pid: u32, p: Priority) -> Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    unsafe { SetPriorityClass(raw(&h), p.class()) }
}

// ---------------------------------------------------------------- 相关性

/// (进程相关性掩码, 系统可用掩码)
pub fn affinity(pid: u32) -> Result<(u64, u64)> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let (mut process, mut system) = (0usize, 0usize);
    unsafe { GetProcessAffinityMask(raw(&h), &mut process, &mut system)? };
    Ok((process as u64, system as u64))
}

pub fn set_affinity(pid: u32, mask: u64) -> Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    unsafe { SetProcessAffinityMask(raw(&h), mask as usize) }
}

// ---------------------------------------------------------------- 效率模式

/// 效率模式 = EcoQoS（执行速度节流）+ 低优先级，与任务管理器的做法一致。
pub fn efficiency_mode(process: HANDLE) -> bool {
    let mut s = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ..Default::default()
    };
    let ok = unsafe {
        GetProcessInformation(
            process,
            ProcessPowerThrottling,
            (&mut s as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            size_of_val(&s) as u32,
        )
    }
    .is_ok();
    ok && s.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
}

pub fn set_efficiency_mode(pid: u32, on: bool) -> Result<()> {
    let h = open(pid, PROCESS_SET_INFORMATION)?;
    let s = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    unsafe {
        SetProcessInformation(
            raw(&h),
            ProcessPowerThrottling,
            (&s as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            size_of_val(&s) as u32,
        )?;
        SetPriorityClass(
            raw(&h),
            if on {
                IDLE_PRIORITY_CLASS
            } else {
                NORMAL_PRIORITY_CLASS
            },
        )
    }
}

// ---------------------------------------------------------------- 转储 / 属性 / 搜索 / 运行

/// 写完整内存转储到 %TEMP%\<名称>.DMP（与任务管理器相同位置），返回文件路径。可能耗时数秒，应在后台线程调用。
pub fn create_dump(pid: u32, name: &str) -> Result<PathBuf> {
    let h = open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)?;
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("process");
    let path = std::env::temp_dir().join(format!("{stem}.DMP"));
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let file = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )?
    };
    let file = unsafe { OwnedHandle::from_raw_handle(file.0) };
    let kind = MiniDumpWithFullMemory | MiniDumpWithHandleData | MiniDumpWithThreadInfo;
    unsafe { MiniDumpWriteDump(raw(&h), pid, raw(&file), kind, None, None, None)? };
    Ok(path)
}

/// 打开资源管理器的文件属性对话框。
pub fn show_properties(path: &str) {
    let wide = crate::wide(path);
    unsafe {
        let _ = SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(wide.as_ptr()), PCWSTR::null());
    }
}

/// 用默认浏览器搜索。`url_prefix` 如 `https://www.google.com/search?q=`，关键词编码后接在后面。
pub fn search_online(url_prefix: &str, query: &str) {
    let encoded: String = query
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    open_url(&format!("{url_prefix}{encoded}"));
}

pub fn open_url(url: &str) {
    let wide = crate::wide(url);
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// 运行新任务：命令拆成程序与参数交给 ShellExecute，`admin` 时以管理员身份运行（触发 UAC）。
/// 也支持文档、文件夹、URL 等可被 Shell 打开的目标。
pub fn run(command: &str, admin: bool) -> std::result::Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err(crate::t!(
            "命令为空",
            "The command is empty",
            "コマンドが入力されていません"
        )
        .into());
    }
    let (file, args) = split_command(command);
    let file_w = crate::wide(&file);
    let args_w = crate::wide(&args);
    let verb = if admin { w!("runas") } else { w!("open") };
    let r = unsafe {
        ShellExecuteW(
            None,
            verb,
            PCWSTR(file_w.as_ptr()),
            if args.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR(args_w.as_ptr())
            },
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute 返回值 ≤ 32 表示失败。
    match r.0 as usize {
        n if n > 32 => Ok(()),
        2 | 3 => Err(crate::tf!(
            "找不到 \"{file}\"",
            "Couldn't find \"{file}\"",
            "「{file}」が見つかりません"
        )),
        5 => Err(crate::t!("拒绝访问", "Access is denied", "アクセスが拒否されました").into()),
        n => Err(crate::tf!(
            "无法运行（错误 {n}）",
            "Couldn't run it (error {n})",
            "実行できませんでした (エラー {n})"
        )),
    }
}

/// `"C:\a b\c.exe" --x` → (`C:\a b\c.exe`, `--x`)；`notepad foo.txt` → (`notepad`, `foo.txt`)。
pub fn split_command(command: &str) -> (String, String) {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
    }
    match command.split_once(char::is_whitespace) {
        Some((file, args)) => (file.to_string(), args.trim().to_string()),
        None => (command.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_commands() {
        assert_eq!(
            split_command(r#""C:\a b\c.exe" --x 1"#),
            (r"C:\a b\c.exe".into(), "--x 1".into())
        );
        assert_eq!(
            split_command("notepad foo.txt"),
            ("notepad".into(), "foo.txt".into())
        );
        assert_eq!(split_command("  cmd  "), ("cmd".into(), String::new()));
    }

    #[test]
    fn priority_from_base() {
        assert_eq!(Priority::from_base(8), Priority::Normal);
        assert_eq!(Priority::from_base(4), Priority::Idle);
        assert_eq!(Priority::from_base(13), Priority::High);
        assert_eq!(Priority::from_base(24), Priority::Realtime);
    }

    #[test]
    fn suspend_and_resume_child() {
        // 拿一个自己启动的子进程测挂起/恢复与优先级，不碰别的进程。
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        suspend(pid).unwrap();
        resume(pid).unwrap();
        set_priority(pid, Priority::BelowNormal).unwrap();
        set_efficiency_mode(pid, true).unwrap();
        let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION).unwrap();
        assert!(efficiency_mode(raw(&h)));
        set_efficiency_mode(pid, false).unwrap();
        assert!(!efficiency_mode(raw(&h)));
        let (mask, system) = affinity(pid).unwrap();
        assert!(mask != 0 && system != 0);
        set_affinity(pid, 1).unwrap();
        assert_eq!(affinity(pid).unwrap().0, 1);
        let _ = child.kill();
        let _ = child.wait();
    }
}
