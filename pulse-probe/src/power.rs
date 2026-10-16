//! 电源计划与处理器电源设置：切换计划、Win11 电源模式、睿频、最大频率。
//! 计划的默认安全描述符允许普通用户修改，不需要管理员。

use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, GetSystemPowerStatus, PowerEnumerate, PowerGetActiveScheme,
    PowerReadACValueIndex, PowerReadDCValueIndex, PowerReadFriendlyName, PowerSetActiveScheme,
    PowerWriteACValueIndex, PowerWriteDCValueIndex, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
pub use windows::core::GUID;
use windows::core::{s, w};

/// 「平衡」计划。电源模式只在它生效时可用。
pub const BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
const POWER_SAVER: GUID = GUID::from_u128(0xa1841308_3541_4fab_bc81_f71556f20b4a);
const HIGH_PERFORMANCE: GUID = GUID::from_u128(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);
const ULTIMATE: GUID = GUID::from_u128(0xe9a42b02_d5df_448d_aa00_03f14749eb61);

/// Win11 的三档电源模式（覆盖方案）。
pub const MODE_EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
pub const MODE_BALANCED: GUID = GUID::zeroed();
pub const MODE_PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);

const SUB_PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
const PERFBOOSTMODE: GUID = GUID::from_u128(0xbe337238_0d82_4146_a960_4f3749d470c7);
/// 最大频率（MHz，0 为不限）。带 1 的是第二个效率等级（混合架构上的性能核）。
const PROCFREQMAX: GUID = GUID::from_u128(0x75b0ae3f_bce0_45a7_8c89_c9611c25e100);
const PROCFREQMAX1: GUID = GUID::from_u128(0x75b0ae3f_bce0_45a7_8c89_c9611c25e101);

/// 睿频关闭以外，打开时默认写入的模式（积极）。
pub const BOOST_DEFAULT: u32 = 2;

#[derive(Clone, Debug)]
pub struct Scheme {
    pub guid: GUID,
    /// 系统自带的计划为 None，界面按语言翻译；自定义计划用系统里的名字。
    pub name: Option<String>,
    pub kind: Option<BuiltIn>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltIn {
    PowerSaver,
    Balanced,
    HighPerformance,
    Ultimate,
}

#[derive(Clone, Debug, Default)]
pub struct PowerState {
    pub schemes: Vec<Scheme>,
    pub active: GUID,
    /// 用户选的电源模式；系统不支持时为 None。
    pub mode: Option<GUID>,
    /// 实际生效的电源模式。节能模式、厂商软件（如 MSI Center）可能让它和 `mode` 不同。
    pub effective_mode: Option<GUID>,
    /// 快速设置里的「节能」开着。它会强制最佳能效，用户选的电源模式不生效。
    pub energy_saver: bool,
    /// 机器有电池（笔记本）。此时设置只作用于当前电源。
    pub has_battery: bool,
    pub on_battery: bool,
    /// 当前计划、当前电源下的睿频模式（0 为关闭）。
    pub boost: Option<u32>,
    /// 当前计划、当前电源下的最大频率（MHz，0 为不限）。
    pub max_mhz: Option<u32>,
}

impl PowerState {
    pub fn query() -> Self {
        let mut st = SYSTEM_POWER_STATUS::default();
        let _ = unsafe { GetSystemPowerStatus(&mut st) };
        // BatteryFlag 128：没有电池；255：未知。
        let has_battery = st.BatteryFlag != 128 && st.BatteryFlag != 255;
        let on_battery = st.ACLineStatus == 0;
        let active = active_scheme().unwrap_or(BALANCED);
        let dc = has_battery && on_battery;
        Self {
            schemes: schemes(),
            active,
            mode: user_mode(dc).or_else(effective_mode),
            effective_mode: effective_mode(),
            energy_saver: energy_saver_on(),
            has_battery,
            on_battery,
            boost: read(&active, &PERFBOOSTMODE, dc),
            max_mhz: read(&active, &PROCFREQMAX, dc),
        }
    }

    fn dc(&self) -> bool {
        self.has_battery && self.on_battery
    }

    pub fn set_scheme(&self, guid: GUID) -> Result<(), String> {
        check(unsafe { PowerSetActiveScheme(None, Some(&guid)) })
    }

    /// 新系统用「用户配置的电源模式」接口（设置应用用的就是它），旧系统退回覆盖方案接口。
    pub fn set_mode(&self, mode: GUID) -> Result<(), String> {
        let f = mode_fns();
        let set_user = if self.dc() {
            f.set_user_dc
        } else {
            f.set_user_ac
        };
        let err = match (set_user, f.set_overlay) {
            (Some(set), _) => unsafe { set(&mode) },
            (None, Some(set)) => unsafe { set(mode) },
            (None, None) => 1, // ERROR_INVALID_FUNCTION
        };
        check(WIN32_ERROR(err))
    }

    pub fn set_boost(&self, mode: u32) -> Result<(), String> {
        self.write(&[PERFBOOSTMODE], mode)
    }

    /// 0 为不限。两个效率等级一起写，混合架构上大小核用同一个上限。
    pub fn set_max_mhz(&self, mhz: u32) -> Result<(), String> {
        self.write(&[PROCFREQMAX, PROCFREQMAX1], mhz)
    }

    /// 写入当前计划、当前电源下的值，再重新激活计划让它立即生效。
    fn write(&self, settings: &[GUID], value: u32) -> Result<(), String> {
        let scheme = self.active;
        for setting in settings {
            let err = unsafe {
                if self.dc() {
                    WIN32_ERROR(PowerWriteDCValueIndex(
                        None,
                        &scheme,
                        Some(&SUB_PROCESSOR),
                        Some(setting),
                        value,
                    ))
                } else {
                    PowerWriteACValueIndex(
                        None,
                        &scheme,
                        Some(&SUB_PROCESSOR),
                        Some(setting),
                        value,
                    )
                }
            };
            check(err)?;
        }
        check(unsafe { PowerSetActiveScheme(None, Some(&scheme)) })
    }
}

/// 节能模式的状态没有公开接口，台式机上 GetSystemPowerStatus 也不反映它。
/// 注册表里 EnergySaverState 为 1 时开启、2 时关闭（实测）。
fn energy_saver_on() -> bool {
    let mut v = 0u32;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SYSTEM\CurrentControlSet\Control\Power"),
            w!("EnergySaverState"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    ok == ERROR_SUCCESS && v == 1
}

fn check(err: WIN32_ERROR) -> Result<(), String> {
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(crate::win32_message(err.0))
    }
}

fn active_scheme() -> Option<GUID> {
    let mut p: *mut GUID = std::ptr::null_mut();
    if unsafe { PowerGetActiveScheme(None, &mut p) } != ERROR_SUCCESS || p.is_null() {
        return None;
    }
    let guid = unsafe { *p };
    unsafe { LocalFree(Some(HLOCAL(p.cast()))) };
    Some(guid)
}

fn schemes() -> Vec<Scheme> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut guid = GUID::zeroed();
        let mut size = size_of::<GUID>() as u32;
        let err = unsafe {
            PowerEnumerate(
                None,
                None,
                None,
                ACCESS_SCHEME,
                index,
                Some((&mut guid as *mut GUID).cast()),
                &mut size,
            )
        };
        if err != ERROR_SUCCESS {
            break;
        }
        let kind = match guid {
            POWER_SAVER => Some(BuiltIn::PowerSaver),
            BALANCED => Some(BuiltIn::Balanced),
            HIGH_PERFORMANCE => Some(BuiltIn::HighPerformance),
            ULTIMATE => Some(BuiltIn::Ultimate),
            _ => None,
        };
        out.push(Scheme {
            guid,
            name: kind.is_none().then(|| friendly_name(&guid)).flatten(),
            kind,
        });
    }
    // 自带的按 节能 → 平衡 → 高性能 → 卓越性能，自定义的排在后面。
    out.sort_by_key(|s| s.kind.map_or(4, |k| k as u8));
    out
}

fn friendly_name(guid: &GUID) -> Option<String> {
    let mut size = 0u32;
    let _ = unsafe { PowerReadFriendlyName(None, Some(guid), None, None, None, &mut size) };
    if size < 2 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    if unsafe {
        PowerReadFriendlyName(
            None,
            Some(guid),
            None,
            None,
            Some(buf.as_mut_ptr().cast()),
            &mut size,
        )
    } != ERROR_SUCCESS
    {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    (!name.trim().is_empty()).then_some(name)
}

fn read(scheme: &GUID, setting: &GUID, dc: bool) -> Option<u32> {
    let mut v = 0u32;
    let err = unsafe {
        if dc {
            WIN32_ERROR(PowerReadDCValueIndex(
                None,
                Some(scheme),
                Some(&SUB_PROCESSOR),
                Some(setting),
                &mut v,
            ))
        } else {
            PowerReadACValueIndex(
                None,
                Some(scheme),
                Some(&SUB_PROCESSOR),
                Some(setting),
                &mut v,
            )
        }
    };
    (err == ERROR_SUCCESS).then_some(v)
}

type GetMode = unsafe extern "system" fn(*mut GUID) -> u32;
type SetMode = unsafe extern "system" fn(*const GUID) -> u32;
type SetOverlay = unsafe extern "system" fn(GUID) -> u32;

/// 电源模式相关函数：覆盖方案接口 Win10 1709 起才有且没有文档，
/// 「用户配置的电源模式」接口是 Win11 新加的，都按名字动态取。
#[derive(Clone, Copy, Default)]
struct ModeFns {
    effective: Option<GetMode>,
    user_ac: Option<GetMode>,
    user_dc: Option<GetMode>,
    set_user_ac: Option<SetMode>,
    set_user_dc: Option<SetMode>,
    set_overlay: Option<SetOverlay>,
}

fn mode_fns() -> ModeFns {
    static FNS: std::sync::OnceLock<ModeFns> = std::sync::OnceLock::new();
    *FNS.get_or_init(|| unsafe {
        let Ok(lib) = LoadLibraryW(w!("powrprof.dll")) else {
            return ModeFns::default();
        };
        let get = |name| GetProcAddress(lib, name).map(|f| std::mem::transmute::<_, GetMode>(f));
        let set = |name| GetProcAddress(lib, name).map(|f| std::mem::transmute::<_, SetMode>(f));
        ModeFns {
            effective: get(s!("PowerGetEffectiveOverlayScheme")),
            user_ac: get(s!("PowerGetUserConfiguredACPowerMode")),
            user_dc: get(s!("PowerGetUserConfiguredDCPowerMode")),
            set_user_ac: set(s!("PowerSetUserConfiguredACPowerMode")),
            set_user_dc: set(s!("PowerSetUserConfiguredDCPowerMode")),
            set_overlay: GetProcAddress(lib, s!("PowerSetActiveOverlayScheme"))
                .map(|f| std::mem::transmute::<_, SetOverlay>(f)),
        }
    })
}

fn call_get(f: Option<GetMode>) -> Option<GUID> {
    let mut guid = GUID::zeroed();
    (unsafe { f?(&mut guid) } == 0).then_some(guid)
}

fn effective_mode() -> Option<GUID> {
    call_get(mode_fns().effective)
}

fn user_mode(dc: bool) -> Option<GUID> {
    let f = mode_fns();
    call_get(if dc { f.user_dc } else { f.user_ac })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只读：当前计划应该出现在枚举结果里（CI 虚拟机上可能没有可枚举的计划）。
    #[test]
    fn reads_active_scheme() {
        let st = PowerState::query();
        println!("{st:#?}");
        assert!(st.schemes.is_empty() || st.schemes.iter().any(|s| s.guid == st.active));
    }
}
