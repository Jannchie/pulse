//! CPU 与内存的详细信息：型号、拓扑、缓存、频率；内存构成、内核池；内存条（SMBIOS）。

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows::core::w;
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GetSystemFirmwareTable, RSMB, RelationAll,
};

#[derive(Clone, Debug, Default)]
pub struct CpuInfo {
    pub name: String,
    /// 基准频率（MHz）。
    pub base_mhz: u32,
    pub sockets: u32,
    pub cores: u32,
    pub logical: u32,
    /// 各级缓存总量（字节）。
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    /// 处理器组 0 中每个逻辑处理器所在核心的效率等级。混合架构上数值越大越偏性能。
    pub efficiency: Vec<u8>,
    /// 性能核（大核）与能效核（小核）的物理核心数；非混合架构时全部算作性能核。
    pub p_cores: u32,
    pub e_cores: u32,
}

impl CpuInfo {
    /// 是否为大小核混合架构。
    pub fn hybrid(&self) -> bool {
        self.e_cores > 0
    }

    /// 逻辑处理器 `i` 是否在能效核上。
    pub fn is_efficient(&self, i: usize) -> bool {
        let top = self.efficiency.iter().copied().max().unwrap_or(0);
        self.efficiency.get(i).is_some_and(|&c| c < top)
    }

    pub fn query() -> Self {
        let mut info = CpuInfo {
            name: reg_string(
                r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
                "ProcessorNameString",
            )
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default(),
            base_mhz: reg_dword(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "~MHz")
                .unwrap_or(0),
            ..Default::default()
        };
        let mut len = 0u32;
        unsafe {
            let _ = GetLogicalProcessorInformationEx(RelationAll, None, &mut len);
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if unsafe {
            GetLogicalProcessorInformationEx(RelationAll, Some(buf.as_mut_ptr().cast()), &mut len)
        }
        .is_err()
        {
            return info;
        }
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len as usize) };
        let mut core_classes = Vec::new();
        let mut off = 0;
        // SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX：Relationship(u32)、Size(u32)，之后是联合体。
        while off + 8 <= bytes.len() {
            let rel = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            let size = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap()) as usize;
            if size == 0 {
                break;
            }
            let body = &bytes[off + 8..off + size];
            match rel {
                // RelationProcessorCore：Flags(u8) 的 bit0 表示 SMT。逻辑处理器数取组掩码里的位数。
                0 => {
                    info.cores += 1;
                    // PROCESSOR_RELATIONSHIP：Flags, EfficiencyClass, Reserved[20], GroupCount(u16), GroupMask[]
                    let class = body[1];
                    core_classes.push(class);
                    let groups = u16::from_le_bytes(body[22..24].try_into().unwrap()) as usize;
                    for g in 0..groups {
                        // GROUP_AFFINITY：Mask(usize)、Group(u16)、Reserved[3]，共 16 字节，从 24 开始。
                        let at = 24 + g * 16;
                        let m = u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
                        let group = u16::from_le_bytes(body[at + 8..at + 10].try_into().unwrap());
                        info.logical += m.count_ones();
                        if group == 0 {
                            for bit in (0..64).filter(|b| m & (1u64 << b) != 0) {
                                if info.efficiency.len() <= bit {
                                    info.efficiency.resize(bit + 1, class);
                                }
                                info.efficiency[bit] = class;
                            }
                        }
                    }
                }
                // RelationCache：Level(u8)、Associativity(u8)、LineSize(u16)、CacheSize(u32)
                2 => {
                    let level = body[0];
                    let size = u32::from_le_bytes(body[4..8].try_into().unwrap()) as u64;
                    match level {
                        1 => info.l1 += size,
                        2 => info.l2 += size,
                        3 => info.l3 += size,
                        _ => {}
                    }
                }
                // RelationProcessorPackage
                3 => info.sockets += 1,
                _ => {}
            }
            off += size;
        }
        let top = core_classes.iter().copied().max().unwrap_or(0);
        info.p_cores = core_classes.iter().filter(|&&c| c == top).count() as u32;
        info.e_cores = core_classes.len() as u32 - info.p_cores;
        info
    }
}

/// 实际运行频率：`% Processor Performance` 是实际性能相对额定性能的百分比（APERF/MPERF），
/// 乘以基准频率即得含睿频的有效频率。任务管理器的"速度"与 [`current_mhz`] 只是各核额定频率，
/// 混合架构上还会被能效核拉低（12900K：16×3.19 + 8×2.39 → 2.92 GHz）。
pub struct CpuFrequency {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl CpuFrequency {
    /// 计数器不可用（性能计数器被禁用等）时返回 None。
    pub fn new() -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(None, 0, &mut query) } != 0 {
            return None;
        }
        let mut counter = PDH_HCOUNTER::default();
        // 英文路径，不受系统语言影响。
        let path = w!(r"\Processor Information(_Total)\% Processor Performance");
        if unsafe { PdhAddEnglishCounterW(query, path, 0, &mut counter) } != 0 {
            unsafe { PdhCloseQuery(query) };
            return None;
        }
        // 百分比计数器需要两次采样，先建立基线。
        unsafe { PdhCollectQueryData(query) };
        Some(Self { query, counter })
    }

    /// 自上次调用以来的平均有效频率（MHz）。
    pub fn sample(&self, base_mhz: u32) -> Option<u32> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let ok = unsafe {
            PdhCollectQueryData(self.query) == 0
                && PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut v) == 0
                && v.CStatus == 0
        };
        let pct = unsafe { v.Anonymous.doubleValue };
        (ok && base_mhz > 0 && pct > 0.0).then(|| (base_mhz as f64 * pct / 100.0).round() as u32)
    }
}

impl Drop for CpuFrequency {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// 各逻辑处理器驱动报告的当前频率的平均值（MHz）。
/// 多数平台上这个值固定为额定频率，不反映睿频；只在 [`CpuFrequency`] 不可用时兜底。
pub fn current_mhz(logical: u32) -> Option<u32> {
    /// PROCESSOR_POWER_INFORMATION
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Ppi {
        number: u32,
        max_mhz: u32,
        current_mhz: u32,
        mhz_limit: u32,
        max_idle_state: u32,
        current_idle_state: u32,
    }
    let n = logical.max(1) as usize;
    let mut buf = vec![Ppi::default(); n];
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            None,
            0,
            Some(buf.as_mut_ptr().cast()),
            (n * size_of::<Ppi>()) as u32,
        )
    };
    if status.is_err() {
        return None;
    }
    let sum: u64 = buf.iter().map(|p| p.current_mhz as u64).sum();
    Some((sum / n as u64) as u32)
}

#[derive(Clone, Debug, Default)]
pub struct MemoryDetail {
    pub total: u64,
    pub available: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    pub commit_peak: u64,
    /// 系统文件缓存。
    pub cache: u64,
    pub paged_pool: u64,
    pub nonpaged_pool: u64,
    pub handles: u32,
    pub processes: u32,
    pub threads: u32,
    /// 页面列表，需要管理员；拿不到时为 None。
    pub lists: Option<MemoryLists>,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryLists {
    /// 零页 + 空闲页。
    pub free: u64,
    /// 已修改（待写回）。
    pub modified: u64,
    /// 备用（缓存的、可随时回收的页）。
    pub standby: u64,
}

impl MemoryDetail {
    pub fn query() -> Self {
        let mut pi = PERFORMANCE_INFORMATION {
            cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
            ..Default::default()
        };
        if unsafe { GetPerformanceInfo(&mut pi, pi.cb) }.is_err() {
            return Self::default();
        }
        let page = pi.PageSize as u64;
        Self {
            total: pi.PhysicalTotal as u64 * page,
            available: pi.PhysicalAvailable as u64 * page,
            commit_used: pi.CommitTotal as u64 * page,
            commit_limit: pi.CommitLimit as u64 * page,
            commit_peak: pi.CommitPeak as u64 * page,
            cache: pi.SystemCache as u64 * page,
            paged_pool: pi.KernelPaged as u64 * page,
            nonpaged_pool: pi.KernelNonpaged as u64 * page,
            handles: pi.HandleCount,
            processes: pi.ProcessCount,
            threads: pi.ThreadCount,
            lists: memory_lists(page),
        }
    }

    /// 使用中 = 总量 − 空闲 − 备用 − 已修改（与任务管理器的"内存组合"一致）。
    pub fn in_use(&self) -> u64 {
        match &self.lists {
            Some(l) => self.total.saturating_sub(l.free + l.standby + l.modified),
            None => self.total.saturating_sub(self.available),
        }
    }
}

fn memory_lists(page: u64) -> Option<MemoryLists> {
    /// SYSTEM_MEMORY_LIST_INFORMATION（SystemMemoryListInformation = 80）
    #[repr(C)]
    #[derive(Default)]
    struct Lists {
        zero: usize,
        free: usize,
        modified: usize,
        modified_no_write: usize,
        bad: usize,
        standby_by_priority: [usize; 8],
        repurposed_by_priority: [usize; 8],
        modified_pagefile: usize,
    }
    let mut l = Lists::default();
    let mut len = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_INFORMATION_CLASS(80),
            (&mut l as *mut Lists).cast(),
            size_of::<Lists>() as u32,
            &mut len,
        )
    };
    status.is_ok().then(|| MemoryLists {
        free: (l.zero + l.free) as u64 * page,
        modified: (l.modified + l.modified_no_write) as u64 * page,
        standby: l.standby_by_priority.iter().sum::<usize>() as u64 * page,
    })
}

#[derive(Clone, Debug, Default)]
pub struct MemoryModule {
    pub locator: String,
    pub manufacturer: String,
    pub part_number: String,
    pub size: u64,
    /// MT/s；配置速度优先，没有时取额定速度。
    pub speed: u32,
    pub kind: &'static str,
    /// SMBIOS 的 Form Factor 字段，用 [`MemoryModule::form_factor`] 取名称。
    form_factor_code: u8,
}

impl MemoryModule {
    pub fn form_factor(&self) -> &'static str {
        match self.form_factor_code {
            0x09 => "DIMM",
            0x0D => "SODIMM",
            0x0F => "FB-DIMM",
            0x0B => crate::t!("板载", "Onboard", "オンボード"),
            _ => "",
        }
    }
}

/// 从 SMBIOS 类型 17（Memory Device）读取已插入的内存条，以及插槽总数（含空插槽）。
pub fn memory_modules() -> (Vec<MemoryModule>, u32) {
    let len = unsafe { GetSystemFirmwareTable(RSMB, 0, None) } as usize;
    if len == 0 {
        return (Vec::new(), 0);
    }
    let mut buf = vec![0u8; len];
    if unsafe { GetSystemFirmwareTable(RSMB, 0, Some(&mut buf)) } as usize != len || len < 8 {
        return (Vec::new(), 0);
    }
    // RawSMBIOSData：8 字节头，之后是结构表。
    let table_len = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    let table = &buf[8..(8 + table_len).min(len)];
    let mut out = Vec::new();
    let mut slots = 0u32;
    let mut off = 0;
    while off + 4 <= table.len() {
        let kind = table[off];
        let flen = table[off + 1] as usize;
        if flen < 4 || off + flen > table.len() {
            break;
        }
        let formatted = &table[off..off + flen];
        // 字符串区紧跟格式化区，以两个 0 结束。
        let mut end = off + flen;
        while end + 1 < table.len() && !(table[end] == 0 && table[end + 1] == 0) {
            end += 1;
        }
        let strings: Vec<String> = table[off + flen..end]
            .split(|&b| b == 0)
            .map(|s| String::from_utf8_lossy(s).trim().to_string())
            .collect();
        let s = |idx: u8| {
            if idx == 0 {
                String::new()
            } else {
                strings.get(idx as usize - 1).cloned().unwrap_or_default()
            }
        };
        let u16_at = |o: usize| {
            formatted
                .get(o..o + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
        };
        if kind == 17 {
            slots += 1;
        }
        if kind == 17 && flen >= 0x1B {
            let raw = u16_at(0x0C).unwrap_or(0);
            let size = match raw {
                0 | 0xFFFF => 0,
                0x7FFF => {
                    formatted
                        .get(0x1C..0x20)
                        .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()) as u64)
                        << 20
                }
                r if r & 0x8000 != 0 => ((r & 0x7FFF) as u64) << 10,
                r => (r as u64) << 20,
            };
            if size > 0 {
                let configured = if flen >= 0x22 {
                    u16_at(0x20).unwrap_or(0)
                } else {
                    0
                };
                let rated = u16_at(0x15).unwrap_or(0);
                out.push(MemoryModule {
                    locator: s(formatted[0x10]),
                    manufacturer: s(formatted[0x17]),
                    part_number: s(formatted[0x1A]),
                    size,
                    speed: if configured > 0 { configured } else { rated } as u32,
                    kind: match formatted[0x12] {
                        0x12 => "DDR",
                        0x13 => "DDR2",
                        0x18 => "DDR3",
                        0x1A => "DDR4",
                        0x1B => "LPDDR",
                        0x1C => "LPDDR2",
                        0x1D => "LPDDR3",
                        0x1E => "LPDDR4",
                        0x22 => "DDR5",
                        0x23 => "LPDDR5",
                        _ => "",
                    },
                    form_factor_code: formatted[0x0E],
                });
            }
        }
        if kind == 127 {
            break;
        }
        off = end + 2;
    }
    (out, slots)
}

/// 主板上安装的物理内存总量（含被硬件保留、操作系统不可用的部分）。
pub fn installed_memory() -> Option<u64> {
    let mut kb = 0u64;
    unsafe {
        windows::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory(&mut kb)
    }
    .ok()?;
    Some(kb * 1024)
}

pub(crate) fn reg_string(key: &str, value: &str) -> Option<String> {
    let key = crate::wide(key);
    let value = crate::wide(value);
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if r.is_err() {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]).trim().to_string())
}

fn reg_dword(key: &str, value: &str) -> Option<u32> {
    let key = crate::wide(key);
    let value = crate::wide(value);
    let mut v = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut len),
        )
    };
    r.is_ok().then_some(v)
}

/// 开机以来的时长。
pub fn uptime() -> std::time::Duration {
    std::time::Duration::from_millis(unsafe {
        windows::Win32::System::SystemInformation::GetTickCount64()
    })
}
