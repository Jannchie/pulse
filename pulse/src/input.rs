//! 统计键盘敲击次数（只计数，不记录按了哪个键），用来区分"打字"和"阅读"。
//!
//! 用 Raw Input 的 RIDEV_INPUTSINK：窗口不在前台、甚至隐藏到托盘时也能收到，不需要全局钩子。
//! WM_INPUT 计数后照常交给后面的窗口过程。

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{RI_KEY_BREAK, WM_INPUT};

static KEYS: AtomicU32 = AtomicU32::new(0);

/// 让窗口在后台也接收键盘的 Raw Input。
pub fn register(hwnd: HWND) {
    let dev = RAWINPUTDEVICE {
        usUsagePage: 0x01, // Generic Desktop
        usUsage: 0x06,     // Keyboard
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        let _ = RegisterRawInputDevices(&[dev], size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// 取出并清零自上次以来的按键次数。
pub fn take_keys() -> u32 {
    KEYS.swap(0, Ordering::Relaxed)
}

/// 在窗口子类过程里调用；只计数，不拦截消息。
pub fn observe(msg: u32, lparam: LPARAM) {
    if msg != WM_INPUT {
        return;
    }
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some(&mut raw as *mut RAWINPUT as *mut _),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == u32::MAX || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return;
    }
    // 只数按下，不数抬起。
    if u32::from(unsafe { raw.data.keyboard.Flags }) & RI_KEY_BREAK == 0 {
        KEYS.fetch_add(1, Ordering::Relaxed);
    }
}
