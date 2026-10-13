//! 统计键盘敲击次数（只计数，不记录按了哪个键），用来区分"打字"和"阅读"；
//! 并记下手柄最后一次被操作的时刻——系统的空闲时间只看键盘和鼠标，用手柄玩游戏时会被当成离开。
//!
//! 用 Raw Input 的 RIDEV_INPUTSINK：窗口不在前台、甚至隐藏到托盘时也能收到，不需要全局钩子。
//! 接收窗口是单独线程里的消息窗口：手柄每秒发几百份报告，放在主窗口上会不停唤醒界面线程。
//!
//! 手柄分两路：DualSense 等通用 HID 手柄走 Raw Input；Xbox 手柄（以及被 Steam、DS4Windows
//! 模拟成 Xbox 手柄的）由记录线程每秒查一次 XInput。手柄空闲时也在不停发报告（陀螺仪、
//! 摇杆的细小抖动），所以只比较按键和越过死区的摇杆、扳机，状态真的变了才算操作。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS, HidP_GetCaps, HidP_GetScaledUsageValue,
    HidP_GetUsageValue, HidP_GetUsages, HidP_GetValueCaps, HidP_Input, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::XboxController::{XINPUT_STATE, XInputGetState};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIDI_PREPARSEDDATA, RIM_TYPEHID, RIM_TYPEKEYBOARD,
    RegisterRawInputDevices,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    RI_KEY_BREAK, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WNDCLASSW,
};
use windows::core::w;

static KEYS: AtomicU32 = AtomicU32::new(0);
/// 手柄最后一次被操作时的 GetTickCount；0 表示还没有过。
static PAD_TICK: AtomicU32 = AtomicU32::new(0);

/// 开一个线程，用消息窗口在后台接收键盘与手柄的 Raw Input。
pub fn start() {
    let _ = std::thread::Builder::new()
        .name("input".into())
        .spawn(|| unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: w!("PulseInput"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("PulseInput"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            ) else {
                return;
            };
            register(hwnd);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    observe(msg, lparam);
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn register(hwnd: HWND) {
    let dev = |usage: u16| RAWINPUTDEVICE {
        usUsagePage: 0x01, // Generic Desktop
        usUsage: usage,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    // 0x06 键盘，0x04 摇杆，0x05 手柄。
    let devices = [dev(0x06), dev(0x04), dev(0x05)];
    unsafe {
        let _ = RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32);
    }
}

/// 取出并清零自上次以来的按键次数。
pub fn take_keys() -> u32 {
    KEYS.swap(0, Ordering::Relaxed)
}

/// 距离上次操作手柄过了多少毫秒；从没操作过时是 `u32::MAX`。
pub fn gamepad_idle_ms() -> u32 {
    match PAD_TICK.load(Ordering::Relaxed) {
        0 => u32::MAX,
        t => unsafe { GetTickCount() }.wrapping_sub(t),
    }
}

fn touch_gamepad() {
    PAD_TICK.store(unsafe { GetTickCount() }.max(1), Ordering::Relaxed);
}

fn observe(msg: u32, lparam: LPARAM) {
    if msg != WM_INPUT {
        return;
    }
    let handle = HRAWINPUT(lparam.0 as *mut _);
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let mut size = 0u32;
    unsafe { GetRawInputData(handle, RID_INPUT, None, &mut size, header) };
    if size == 0 {
        return;
    }
    // 手柄的报告接在结构体后面，比 RAWINPUT 长；按 8 字节对齐分配。
    let mut buf = vec![
        0u64;
        (size as usize)
            .div_ceil(8)
            .max(size_of::<RAWINPUT>().div_ceil(8))
    ];
    let n = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            header,
        )
    };
    if n == u32::MAX {
        return;
    }
    let raw = unsafe { &*(buf.as_ptr() as *const RAWINPUT) };
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        // 只数按下，不数抬起。
        if u32::from(unsafe { raw.data.keyboard.Flags }) & RI_KEY_BREAK == 0 {
            KEYS.fetch_add(1, Ordering::Relaxed);
        }
    } else if raw.header.dwType == RIM_TYPEHID.0 {
        let hid = unsafe { &raw.data.hid };
        let (len, count) = (hid.dwSizeHid as usize, hid.dwCount as usize);
        let start = hid.bRawData.as_ptr();
        let end = unsafe { (buf.as_ptr() as *const u8).add(n as usize) };
        if len == 0 || unsafe { start.add(len * count) } > end {
            return;
        }
        let reports = unsafe { std::slice::from_raw_parts_mut(start as *mut u8, len * count) };
        let mut pads = PADS.lock().unwrap();
        let pad = pads
            .entry(raw.header.hDevice.0 as isize)
            .or_insert_with(|| HidPad::new(raw.header.hDevice));
        for report in reports.chunks_exact_mut(len) {
            if pad.update(report) {
                touch_gamepad();
            }
        }
    }
}

static PADS: std::sync::LazyLock<Mutex<HashMap<isize, HidPad>>> =
    std::sync::LazyLock::new(Mutex::default);

/// 一个 HID 手柄：解析报告用的描述，以及上一次的状态。
struct HidPad {
    /// HidP_* 要用的预解析数据（按 8 字节对齐）。
    preparsed: Vec<u64>,
    /// 摇杆、扳机、方向键：(用途, 逻辑最小值, 逻辑最大值)。
    axes: Vec<(u16, i32, i32)>,
    last: Option<Vec<i32>>,
}

impl HidPad {
    fn new(device: HANDLE) -> Self {
        let mut pad = Self {
            preparsed: Vec::new(),
            axes: Vec::new(),
            last: None,
        };
        let mut size = 0u32;
        unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_PREPARSEDDATA, None, &mut size) };
        if size == 0 {
            return pad;
        }
        pad.preparsed = vec![0u64; (size as usize).div_ceil(8)];
        let got = unsafe {
            GetRawInputDeviceInfoW(
                Some(device),
                RIDI_PREPARSEDDATA,
                Some(pad.preparsed.as_mut_ptr().cast()),
                &mut size,
            )
        };
        if got == u32::MAX {
            pad.preparsed.clear();
            return pad;
        }
        let data = pad.data();
        let mut caps = HIDP_CAPS::default();
        if unsafe { HidP_GetCaps(data, &mut caps) } != HIDP_STATUS_SUCCESS {
            pad.preparsed.clear();
            return pad;
        }
        let mut n = caps.NumberInputValueCaps;
        let mut values = vec![HIDP_VALUE_CAPS::default(); n as usize];
        if n > 0
            && unsafe { HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut n, data) }
                == HIDP_STATUS_SUCCESS
        {
            for v in values.iter().take(n as usize) {
                // Generic Desktop 页的 X、Y、Z、Rx、Ry、Rz 与方向键（0x39）；陀螺仪等在厂商自定义页，不看。
                let usage = if v.IsRange {
                    unsafe { v.Anonymous.Range.UsageMin }
                } else {
                    unsafe { v.Anonymous.NotRange.Usage }
                };
                if v.UsagePage == 0x01 && ((0x30..=0x35).contains(&usage) || usage == 0x39) {
                    pad.axes.push((usage, v.LogicalMin, v.LogicalMax));
                }
            }
        }
        pad
    }

    fn data(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
    }

    /// 读入一份报告，状态变了返回 true（第一份报告只建立基线）。
    fn update(&mut self, report: &mut [u8]) -> bool {
        if self.preparsed.is_empty() {
            return false;
        }
        let data = self.data();
        let mut state = Vec::with_capacity(self.axes.len() + 8);
        for &(usage, min, max) in &self.axes {
            // 逻辑范围有负数时要按有符号读，否则按原始值读。
            let value = if min < 0 {
                let mut v = 0i32;
                (unsafe {
                    HidP_GetScaledUsageValue(HidP_Input, 0x01, None, usage, &mut v, data, report)
                } == HIDP_STATUS_SUCCESS)
                    .then_some(v)
            } else {
                let mut v = 0u32;
                (unsafe { HidP_GetUsageValue(HidP_Input, 0x01, None, usage, &mut v, data, report) }
                    == HIDP_STATUS_SUCCESS)
                    .then_some(v as i32)
            };
            if let Some(v) = value {
                state.push(if usage == 0x39 {
                    v
                } else {
                    quantize(v, min, max)
                });
            }
        }
        // 按下的按键（Button 页）。
        let mut buttons = [0u16; 64];
        let mut count = buttons.len() as u32;
        if unsafe {
            HidP_GetUsages(
                HidP_Input,
                0x09,
                None,
                buttons.as_mut_ptr(),
                &mut count,
                data,
                report,
            )
        } == HIDP_STATUS_SUCCESS
        {
            let mut pressed = buttons[..count as usize].to_vec();
            pressed.sort_unstable();
            state.push(-1);
            state.extend(pressed.into_iter().map(i32::from));
        }
        let changed = self.last.as_ref().is_some_and(|l| *l != state);
        self.last = Some(state);
        changed
    }
}

/// 摇杆、扳机的位置分档：中间 30% 是死区，其余分成每侧 4 档；细小抖动不会跨档。
fn quantize(v: i32, min: i32, max: i32) -> i32 {
    if max <= min {
        return 0;
    }
    let c = (v as f64 - min as f64) / (max as f64 - min as f64) * 2.0 - 1.0;
    if c.abs() < 0.3 {
        0
    } else {
        (c * 4.0).round() as i32
    }
}

/// Xbox 手柄（XInput）上一次的状态，按槽位。
static XINPUT: Mutex<[Option<[i32; 7]>; 4]> = Mutex::new([None; 4]);

/// 查一次 XInput 手柄的状态（记录线程每秒调用），变了就记为操作过。
pub fn poll_xinput() {
    let mut last = XINPUT.lock().unwrap();
    for (slot, prev) in last.iter_mut().enumerate() {
        let mut s = XINPUT_STATE::default();
        if unsafe { XInputGetState(slot as u32, &mut s) } != 0 {
            *prev = None;
            continue;
        }
        let g = s.Gamepad;
        let stick = |v: i16| quantize(v as i32, -32768, 32767);
        let trigger = |v: u8| quantize(v as i32, 0, 255);
        let now = [
            g.wButtons.0 as i32,
            trigger(g.bLeftTrigger),
            trigger(g.bRightTrigger),
            stick(g.sThumbLX),
            stick(g.sThumbLY),
            stick(g.sThumbRX),
            stick(g.sThumbRY),
        ];
        if prev.is_some_and(|p| p != now) {
            touch_gamepad();
        }
        *prev = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::quantize;

    #[test]
    fn quantize_has_a_dead_zone() {
        // 8 位摇杆：中间附近都是 0，推到底是 ±4。
        assert_eq!(quantize(128, 0, 255), 0);
        assert_eq!(quantize(100, 0, 255), 0);
        assert_eq!(quantize(255, 0, 255), 4);
        assert_eq!(quantize(0, 0, 255), -4);
        // 扳机松开在最小值，稳定为 -4。
        assert_eq!(quantize(0, 0, 255), quantize(3, 0, 255));
        // XInput 的有符号摇杆。
        assert_eq!(quantize(-300, -32768, 32767), 0);
        assert_eq!(quantize(32767, -32768, 32767), 4);
    }
}

#[cfg(test)]
mod real_pad {
    use super::*;
    use std::io::Read;
    use windows::Win32::UI::Input::{
        GetRawInputDeviceList, RAWINPUTDEVICELIST, RID_DEVICE_INFO, RIDI_DEVICEINFO,
        RIDI_DEVICENAME,
    };

    /// 用本机连着的 DualSense（USB）的真实报告检查判断：原样、计数器和陀螺仪变化都不算操作，
    /// 按下 ✕、推摇杆算。`cargo test -p pulse -- --ignored --nocapture real_dualsense`
    #[test]
    #[ignore]
    fn real_dualsense() {
        let mut n = 0u32;
        let item = size_of::<RAWINPUTDEVICELIST>() as u32;
        unsafe { GetRawInputDeviceList(None, &mut n, item) };
        let mut list = vec![RAWINPUTDEVICELIST::default(); n as usize];
        unsafe { GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut n, item) };
        let pad = list.iter().find_map(|d| {
            let mut info = RID_DEVICE_INFO {
                cbSize: size_of::<RID_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            let mut size = info.cbSize;
            unsafe {
                GetRawInputDeviceInfoW(
                    Some(d.hDevice),
                    RIDI_DEVICEINFO,
                    Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                    &mut size,
                )
            };
            let hid = unsafe { info.Anonymous.hid };
            (info.dwType == RIM_TYPEHID && hid.dwVendorId == 0x054c && hid.usUsage == 5)
                .then_some(d.hDevice)
        });
        let Some(device) = pad else {
            eprintln!("no DualSense");
            return;
        };
        let mut len = 0u32;
        unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut len) };
        let mut name = vec![0u16; len as usize];
        unsafe {
            GetRawInputDeviceInfoW(
                Some(device),
                RIDI_DEVICENAME,
                Some(name.as_mut_ptr().cast()),
                &mut len,
            )
        };
        let path = String::from_utf16_lossy(
            &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())],
        );
        let mut file = std::fs::File::open(&path).expect("open DualSense");
        let mut buf = [0u8; 128];
        let got = file.read(&mut buf).unwrap();
        let base = buf[..got].to_vec();
        eprintln!("report {got} bytes: {:02x?}", &base[..12]);
        let mut hp = HidPad::new(device);
        eprintln!("axes {:?}", hp.axes);
        let check = |hp: &mut HidPad, edit: &dyn Fn(&mut Vec<u8>)| {
            let mut r = base.clone();
            hp.update(&mut base.clone());
            edit(&mut r);
            hp.update(&mut r)
        };
        assert!(!check(&mut hp, &|_| {}));
        // 计数器（第 7 字节）与陀螺仪、加速度（16–27）在空闲时也会变。
        assert!(!check(&mut hp, &|r| {
            r[7] = r[7].wrapping_add(9);
            for b in &mut r[16..28] {
                *b = b.wrapping_add(37);
            }
        }));
        // 摇杆的细小抖动。
        assert!(!check(&mut hp, &|r| r[1] = r[1].wrapping_add(3)));
        // 按下 ✕（第 8 字节高 4 位是 □✕○△）。
        assert!(check(&mut hp, &|r| r[8] |= 0x20));
        // 左摇杆推到最右。
        assert!(check(&mut hp, &|r| r[1] = 255));
        // 方向键上（低 4 位 0 = 上，8 = 松开）。
        assert!(check(&mut hp, &|r| r[8] &= 0xf0));
        // 右扳机按到底。
        assert!(check(&mut hp, &|r| r[6] = 255));
    }
}
