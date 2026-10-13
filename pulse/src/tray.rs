//! 通知区域（托盘）图标与单实例。
//!
//! 关闭窗口时只是隐藏它，Pulse 留在后台继续采样；单击托盘图标重新打开，右键菜单可以退出。
//! 再次启动 Pulse（双击、Ctrl+Shift+Esc、开机自启）时，新进程把参数交给已在运行的窗口后退出。
//! 托盘与窗口消息都在 titlebar 的窗口子类过程里转到 [`handle`]。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use pulse_probe::t;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM,
    LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_NONE, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, ChangeWindowMessageFilterEx, CreatePopupMenu,
    DestroyMenu, EnumWindows, GetCursorPos, GetPropW, GetWindowThreadProcessId, HICON, IMAGE_ICON,
    IsIconic, LR_DEFAULTCOLOR, LoadImageW, MF_SEPARATOR, MF_STRING, MSGFLT_ALLOW, PostMessageW,
    RegisterWindowMessageW, SM_CXSMICON, SMTO_ABORTIFHUNG, SW_HIDE, SW_RESTORE, SW_SHOW,
    SendMessageTimeoutW, SetForegroundWindow, SetPropW, ShowWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU, WM_COPYDATA, WM_ENDSESSION,
    WM_LBUTTONUP, WM_NULL,
};
use windows::core::{BOOL, PCWSTR, w};

/// 托盘图标发回窗口的回调消息。
const WM_TRAY: u32 = WM_APP + 1;
/// WM_COPYDATA 的标记：参数来自另一个 Pulse 进程。`SHOW` 表示同时把窗口调出来。
const COPY_SHOW: usize = 0x5055_4c01;
const COPY_SILENT: usize = 0x5055_4c00;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static ADDED: AtomicBool = AtomicBool::new(false);
/// 用户选了"退出"或正在为更新重启：关闭窗口时真正退出，而不是缩到托盘。
static QUITTING: AtomicBool = AtomicBool::new(false);
/// 另一个进程交过来、还没被界面取走的搜索参数。
static PENDING: Mutex<Option<String>> = Mutex::new(None);
static MUTEX: AtomicIsize = AtomicIsize::new(0);
/// 资源管理器重启后广播的消息，收到时要重新添加图标。
static TASKBAR_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// 退出前要做的清理（把还没写入的记录存盘），由 main 注册。
static ON_QUIT: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

pub fn set_on_quit(f: impl Fn() + Send + 'static) {
    *ON_QUIT.lock().unwrap() = Some(Box::new(f));
}

/// 单实例：已有 Pulse 在运行时把 `filter`（搜索参数）交给它，返回 true，调用方应直接退出。
/// `show` 为 false（开机自启的 `--tray`）时只交参数，不调出窗口。
pub fn hand_over(filter: &str, show: bool) -> bool {
    let h = unsafe { CreateMutexW(None, false, w!("Local\\Pulse-SingleInstance-7d3f")) };
    let err = unsafe { GetLastError() };
    if let Ok(h) = h {
        MUTEX.store(h.0 as isize, Ordering::Relaxed);
    }
    // 已有实例以管理员身份运行时，普通权限打开它的互斥体会被拒绝，同样说明已有实例。
    if err != ERROR_ALREADY_EXISTS && err != ERROR_ACCESS_DENIED {
        return false;
    }
    let Some(hwnd) = find_instance() else {
        return false;
    };
    let text: Vec<u16> = filter.encode_utf16().collect();
    let data = COPYDATASTRUCT {
        dwData: if show { COPY_SHOW } else { COPY_SILENT },
        cbData: (text.len() * 2) as u32,
        lpData: text.as_ptr() as *mut _,
    };
    unsafe {
        // 让对方能把自己的窗口切到前台（否则只会在任务栏闪烁）。
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let _ = AllowSetForegroundWindow(pid);
        let mut result = 0usize;
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&data as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG,
            3000,
            Some(&mut result),
        )
        .0 != 0
    }
}

/// 标记 Pulse 主窗口的窗口属性，用来在所有顶层窗口里认出它（标题同为 "Pulse" 的旧版本没有这个属性）。
const PROP: PCWSTR = w!("Pulse.Instance");

fn find_instance() -> Option<HWND> {
    unsafe extern "system" fn each(h: HWND, out: LPARAM) -> BOOL {
        if unsafe { GetPropW(h, PROP) }.is_invalid() {
            return BOOL(1);
        }
        unsafe { *(out.0 as *mut HWND) = h };
        BOOL(0)
    }
    let mut found = HWND::default();
    unsafe {
        // 回调提前停止时 EnumWindows 会返回错误，不影响结果。
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut HWND as isize));
    }
    (!found.is_invalid()).then_some(found)
}

/// 放开单实例锁：自动更新会在本进程退出前启动新版本，新版本不能把自己交给即将退出的旧进程。
pub fn release_instance() {
    let h = MUTEX.swap(0, Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = CloseHandle(HANDLE(h as *mut _));
        }
    }
}

/// 窗口建好后调用：记下句柄，允许低权限进程发来的 WM_COPYDATA（Pulse 常以管理员身份运行）。
pub fn attach(hwnd: HWND) {
    HWND_RAW.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        let _ = SetPropW(
            hwnd,
            PROP,
            Some(HANDLE(std::ptr::without_provenance_mut(1))),
        );
    }
    let created =
        *TASKBAR_CREATED.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });
    unsafe {
        let _ = ChangeWindowMessageFilterEx(hwnd, WM_COPYDATA, MSGFLT_ALLOW, None);
        let _ = ChangeWindowMessageFilterEx(hwnd, created, MSGFLT_ALLOW, None);
    }
    allow_dark_menus();
}

fn hwnd() -> Option<HWND> {
    let h = HWND_RAW.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

/// 取走另一个进程交过来的搜索参数。
pub fn take_pending() -> Option<String> {
    PENDING.lock().unwrap().take()
}

pub fn quitting() -> bool {
    QUITTING.load(Ordering::Relaxed)
}

/// 接下来的关闭是真正的退出（更新重启时调用）。
pub fn set_quitting() {
    QUITTING.store(true, Ordering::Relaxed);
}

/// 显示或移除托盘图标。
pub fn set_visible(on: bool) {
    if on { add() } else { remove() }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn add() {
    let Some(hwnd) = hwnd() else { return };
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = small_icon(hwnd);
    copy_into(&mut nid.szTip, "Pulse");
    unsafe {
        if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            ADDED.store(true, Ordering::Relaxed);
        }
    }
}

fn remove() {
    let Some(hwnd) = hwnd() else { return };
    if ADDED.swap(false, Ordering::Relaxed) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
        }
    }
}

/// 更新鼠标悬停时的提示文字（最多 127 个字符）。可以在任意线程调用。
pub fn set_tip(text: &str) {
    let Some(hwnd) = hwnd() else { return };
    if !ADDED.load(Ordering::Relaxed) {
        return;
    }
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_TIP | NIF_SHOWTIP;
    copy_into(&mut nid.szTip, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// 在托盘图标上弹出一条系统通知。
pub fn notify(title: &str, body: &str) {
    let Some(hwnd) = hwnd() else { return };
    if !ADDED.load(Ordering::Relaxed) {
        return;
    }
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_NONE;
    copy_into(&mut nid.szInfoTitle, title);
    copy_into(&mut nid.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    for (d, c) in dst
        .iter_mut()
        .zip(s.encode_utf16().take(max).chain(Some(0)))
    {
        *d = c;
    }
}

/// exe 资源里的图标，按通知区域的尺寸（小图标）取。
fn small_icon(hwnd: HWND) -> HICON {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return HICON::default();
        };
        let px = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(hwnd));
        LoadImageW(
            Some(module.into()),
            PCWSTR(std::ptr::without_provenance(1)),
            IMAGE_ICON,
            px,
            px,
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

/// 把窗口调到前台（从托盘或另一个进程唤起）。
pub fn show_window() {
    let Some(hwnd) = hwnd() else { return };
    unsafe {
        let cmd = if IsIconic(hwnd).as_bool() {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        let _ = ShowWindow(hwnd, cmd);
        let _ = SetForegroundWindow(hwnd);
    }
    crate::titlebar::request_repaint();
}

/// 窗口是否可见（隐藏到托盘时不必重画）。
pub fn window_visible() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    hwnd().is_none_or(|h| unsafe { IsWindowVisible(h) }.as_bool())
}

/// 隐藏启动（`--tray`）：eframe 画完第一帧总会把窗口设为可见，先用 DWM 把窗口隐身（cloak），
/// 等第一帧之后再真正隐藏、解除隐身，窗口就不会闪现。
pub fn cloak(on: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let Some(hwnd) = hwnd() else { return };
    let value = windows::core::BOOL::from(on);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &value as *const _ as *const _,
            size_of::<windows::core::BOOL>() as u32,
        );
    }
}

pub fn hide_window() {
    if let Some(hwnd) = hwnd() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 真正退出：做完清理、移除托盘图标后结束进程。
pub fn quit() -> ! {
    QUITTING.store(true, Ordering::Relaxed);
    if let Some(f) = ON_QUIT.lock().unwrap().as_ref() {
        f();
    }
    remove();
    std::process::exit(0);
}

fn menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let open_label: Vec<u16> = t!("打开 Pulse", "Open Pulse", "Pulse を開く")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let quit_label: Vec<u16> = t!("退出", "Exit", "終了")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, PCWSTR(open_label.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, PCWSTR(quit_label.as_ptr()));
        let mut pt = Default::default();
        let _ = GetCursorPos(&mut pt);
        // 菜单要求窗口在前台，否则点别处时不会自动关闭。
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            MENU_OPEN => show_window(),
            MENU_QUIT => quit(),
            _ => {}
        }
    }
}

/// 右键菜单跟随系统的深色 / 浅色模式（uxtheme 未公开的 SetPreferredAppMode，序号 135）。
fn allow_dark_menus() {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("uxtheme.dll")) else {
            return;
        };
        if let Some(f) =
            GetProcAddress(lib, windows::core::PCSTR(std::ptr::without_provenance(135)))
        {
            // 1 = AllowDark：跟随系统设置。
            let set: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(1);
        }
    }
}

/// 窗口子类过程里先调用它；返回 Some 表示消息已处理。
pub fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4：事件在 lParam 低位。
            match (lparam.0 & 0xffff) as u32 {
                WM_LBUTTONUP | NIN_SELECT => show_window(),
                WM_CONTEXTMENU => menu(hwnd),
                _ => {}
            }
            Some(LRESULT(0))
        }
        WM_COPYDATA => {
            let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            if data.dwData != COPY_SHOW && data.dwData != COPY_SILENT {
                return None;
            }
            let units = unsafe {
                std::slice::from_raw_parts(data.lpData as *const u16, data.cbData as usize / 2)
            };
            let text = String::from_utf16_lossy(units);
            if !text.is_empty() {
                *PENDING.lock().unwrap() = Some(text);
            }
            if data.dwData == COPY_SHOW {
                show_window();
            } else {
                crate::titlebar::request_repaint();
            }
            Some(LRESULT(1))
        }
        // 注销、关机，以及安装程序通过 Restart Manager 关闭应用时：做完清理直接退出。
        WM_ENDSESSION if wparam.0 != 0 => quit(),
        m if Some(&m) == TASKBAR_CREATED.get() => {
            if ADDED.swap(false, Ordering::Relaxed) {
                add();
            }
            None
        }
        _ => None,
    }
}
