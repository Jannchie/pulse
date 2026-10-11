//! 物理磁盘：活动时间、读写速率、响应时间、分区，以及健康状态（温度、NVMe SMART）。
//!
//! - 性能：`IOCTL_DISK_PERFORMANCE`，口径同任务管理器：活动时间 = 1 − 空闲时间 / 经过时间。
//! - 设备信息：`IOCTL_STORAGE_QUERY_PROPERTY`（型号、总线、是否有寻道开销 → SSD/HDD、温度）。
//! - NVMe 健康：协议相关查询读取 SMART/Health Information 日志页（0x02）。
//! - 分区：每个盘符用 `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` 找到所在的物理盘。

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    BusTypeAta, BusTypeNvme, BusTypeRAID, BusTypeSas, BusTypeSata, BusTypeScsi, BusTypeSd,
    BusTypeUsb, CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DISK_PERFORMANCE, GET_LENGTH_INFORMATION,
    IOCTL_DISK_GET_LENGTH_INFO, IOCTL_DISK_PERFORMANCE, IOCTL_STORAGE_QUERY_PROPERTY,
    NVMeDataTypeLogPage, PropertyStandardQuery, ProtocolTypeNvme, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_PROPERTY_ID, STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_SPECIFIC_DATA,
    STORAGE_TEMPERATURE_DATA_DESCRIPTOR, StorageDeviceProperty,
    StorageDeviceProtocolSpecificProperty, StorageDeviceSeekPenaltyProperty,
    StorageDeviceTemperatureProperty,
};
use windows::core::PCWSTR;

/// 健康信息变化很慢，没必要每秒查。
const HEALTH_INTERVAL: Duration = Duration::from_secs(30);
/// 分区（盘符、剩余空间）的刷新间隔。
const VOLUME_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Volume {
    pub letter: char,
    pub label: String,
    pub file_system: String,
    pub total: u64,
    pub free: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NvmeHealth {
    /// 非 0 表示控制器报告了严重警告（备用空间不足、温度、可靠性下降、只读等）。
    pub critical_warning: u8,
    pub available_spare: u8,
    pub spare_threshold: u8,
    /// 厂商估计的寿命已用百分比，可超过 100。
    pub percentage_used: u8,
    pub data_read: u64,
    pub data_written: u64,
    pub power_cycles: u64,
    pub power_on_hours: u64,
    pub unsafe_shutdowns: u64,
    pub media_errors: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DiskHealth {
    /// 摄氏度。
    pub temperature: Option<i16>,
    pub warning_temperature: Option<i16>,
    pub critical_temperature: Option<i16>,
    pub nvme: Option<NvmeHealth>,
}

#[derive(Clone, Debug)]
pub struct DiskStat {
    pub index: u32,
    pub model: String,
    pub bus: &'static str,
    /// None 表示驱动没报告是否有寻道开销。
    pub ssd: Option<bool>,
    pub removable: bool,
    pub size: u64,
    /// 活动时间（0–100）。
    pub active: f32,
    pub read_rate: u64,
    pub write_rate: u64,
    /// 平均响应时间（毫秒）。
    pub response_ms: f32,
    pub volumes: Vec<Volume>,
    pub health: DiskHealth,
}

struct Disk {
    index: u32,
    handle: OwnedHandle,
    model: String,
    bus: &'static str,
    ssd: Option<bool>,
    removable: bool,
    size: u64,
    prev: Option<DISK_PERFORMANCE>,
    health: DiskHealth,
}

pub struct DiskMonitor {
    disks: Vec<Disk>,
    volumes: Vec<(u32, Volume)>,
    volumes_at: Option<Instant>,
    health_at: Option<Instant>,
}

impl DiskMonitor {
    pub fn new() -> Self {
        let disks = (0..32).filter_map(open_disk).collect();
        Self {
            disks,
            volumes: Vec::new(),
            volumes_at: None,
            health_at: None,
        }
    }

    pub fn sample(&mut self) -> Vec<DiskStat> {
        if self
            .volumes_at
            .is_none_or(|t| t.elapsed() >= VOLUME_INTERVAL)
        {
            self.volumes = volumes();
            self.volumes_at = Some(Instant::now());
        }
        let refresh_health = self
            .health_at
            .is_none_or(|t| t.elapsed() >= HEALTH_INTERVAL);
        if refresh_health {
            self.health_at = Some(Instant::now());
        }
        self.disks
            .iter_mut()
            .map(|d| {
                if refresh_health {
                    d.health = health(raw(&d.handle), d.bus);
                }
                let (mut active, mut read_rate, mut write_rate, mut response_ms) = (0.0, 0, 0, 0.0);
                if let Some(now) =
                    ioctl_out::<DISK_PERFORMANCE>(raw(&d.handle), IOCTL_DISK_PERFORMANCE)
                {
                    if let Some(prev) = d.prev {
                        let elapsed = (now.QueryTime - prev.QueryTime).max(1) as f64; // 100ns
                        let idle = (now.IdleTime - prev.IdleTime).max(0) as f64;
                        active = ((1.0 - idle / elapsed) * 100.0).clamp(0.0, 100.0) as f32;
                        let secs = elapsed / 1e7;
                        read_rate = ((now.BytesRead - prev.BytesRead).max(0) as f64 / secs) as u64;
                        write_rate =
                            ((now.BytesWritten - prev.BytesWritten).max(0) as f64 / secs) as u64;
                        let ios = now.ReadCount.wrapping_sub(prev.ReadCount) as u64
                            + now.WriteCount.wrapping_sub(prev.WriteCount) as u64;
                        let busy = (now.ReadTime - prev.ReadTime + now.WriteTime - prev.WriteTime)
                            .max(0) as f64;
                        if ios > 0 {
                            response_ms = (busy / ios as f64 / 1e4) as f32;
                        }
                    }
                    d.prev = Some(now);
                }
                DiskStat {
                    index: d.index,
                    model: d.model.clone(),
                    bus: d.bus,
                    ssd: d.ssd,
                    removable: d.removable,
                    size: d.size,
                    active,
                    read_rate,
                    write_rate,
                    response_ms,
                    volumes: self
                        .volumes
                        .iter()
                        .filter(|(i, _)| *i == d.index)
                        .map(|(_, v)| v.clone())
                        .collect(),
                    health: d.health.clone(),
                }
            })
            .collect()
    }
}

impl Default for DiskMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

fn open(path: &str, access: u32) -> Option<OwnedHandle> {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;
    Some(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

fn open_disk(index: u32) -> Option<Disk> {
    let path = format!(r"\\.\PhysicalDrive{index}");
    // NVMe 协议查询需要读写权限（管理员）；拿不到就退回零权限，性能计数和基本信息仍可用。
    let handle = open(&path, GENERIC_READ.0 | GENERIC_WRITE.0).or_else(|| open(&path, 0))?;
    let h = raw(&handle);
    let (model, bus, removable) =
        device_descriptor(h).unwrap_or_else(|| (format!("磁盘 {index}"), "未知", false));
    let ssd = query_property::<DEVICE_SEEK_PENALTY_DESCRIPTOR>(h, StorageDeviceSeekPenaltyProperty)
        .map(|d| !d.IncursSeekPenalty);
    let size = ioctl_out::<GET_LENGTH_INFORMATION>(h, IOCTL_DISK_GET_LENGTH_INFO)
        .map_or(0, |l| l.Length as u64);
    Some(Disk {
        index,
        handle,
        model,
        bus,
        ssd,
        removable,
        size,
        prev: None,
        health: DiskHealth::default(),
    })
}

/// 无输入、输出为固定结构体的 IOCTL。
fn ioctl_out<T: Default>(h: HANDLE, code: u32) -> Option<T> {
    let mut out = T::default();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            code,
            None,
            0,
            Some((&mut out as *mut T).cast()),
            size_of::<T>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .ok()?;
    Some(out)
}

/// 发一个标准的 IOCTL_STORAGE_QUERY_PROPERTY，结果放进 `out`（足够大的字节缓冲）。
fn storage_query(h: HANDLE, input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(input.as_ptr().cast()),
            input.len() as u32,
            Some(out.as_mut_ptr().cast()),
            out.len() as u32,
            Some(&mut returned),
            None,
        )
    }
    .ok()?;
    Some(returned as usize)
}

fn property_query(id: STORAGE_PROPERTY_ID) -> Vec<u8> {
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: id,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&q as *const STORAGE_PROPERTY_QUERY).cast::<u8>(),
            size_of::<STORAGE_PROPERTY_QUERY>(),
        )
    };
    bytes.to_vec()
}

fn query_property<T: Copy>(h: HANDLE, id: STORAGE_PROPERTY_ID) -> Option<T> {
    let mut out = vec![0u64; 1024 / 8];
    let bytes = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr().cast::<u8>(), 1024) };
    let n = storage_query(h, &property_query(id), bytes)?;
    (n >= size_of::<T>()).then(|| unsafe { std::ptr::read_unaligned(out.as_ptr().cast::<T>()) })
}

#[allow(non_upper_case_globals)]
fn device_descriptor(h: HANDLE) -> Option<(String, &'static str, bool)> {
    let mut out = vec![0u8; 1024];
    let n = storage_query(h, &property_query(StorageDeviceProperty), &mut out)?;
    if n < size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return None;
    }
    let d = unsafe { std::ptr::read_unaligned(out.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>()) };
    let text = |offset: u32| {
        let start = offset as usize;
        if start == 0 || start >= n {
            return String::new();
        }
        let end = out[start..n]
            .iter()
            .position(|&b| b == 0)
            .map_or(n, |e| start + e);
        String::from_utf8_lossy(&out[start..end]).trim().to_string()
    };
    let (vendor, product) = (text(d.VendorIdOffset), text(d.ProductIdOffset));
    // 很多 NVMe 盘的 vendor 为空或就是 "NVMe"，型号全在 product 里。
    let model =
        if vendor.is_empty() || product.starts_with(&vendor) || vendor.eq_ignore_ascii_case("nvme")
        {
            product
        } else {
            format!("{vendor} {product}")
        };
    let bus = match d.BusType {
        BusTypeNvme => "NVMe",
        BusTypeSata => "SATA",
        BusTypeAta => "ATA",
        BusTypeUsb => "USB",
        BusTypeSas => "SAS",
        BusTypeScsi => "SCSI",
        BusTypeRAID => "RAID",
        BusTypeSd => "SD",
        _ => "其他",
    };
    Some((model, bus, d.RemovableMedia))
}

fn health(h: HANDLE, bus: &str) -> DiskHealth {
    let mut out = DiskHealth::default();
    if let Some(t) = temperature(h) {
        out.temperature = t.0;
        out.warning_temperature = t.1;
        out.critical_temperature = t.2;
    }
    if bus == "NVMe" {
        out.nvme = nvme_health(h);
        // 驱动不报温度属性时用 SMART 日志里的复合温度。
        if out.temperature.is_none() {
            out.temperature = out.nvme.as_ref().and(nvme_temperature(h));
        }
    }
    out
}

/// (当前, 警告阈值, 临界阈值)，单位摄氏度。
#[allow(clippy::type_complexity)]
fn temperature(h: HANDLE) -> Option<(Option<i16>, Option<i16>, Option<i16>)> {
    let d =
        query_property::<STORAGE_TEMPERATURE_DATA_DESCRIPTOR>(h, StorageDeviceTemperatureProperty)?;
    let valid = |t: i16| (t > -100 && t < 200).then_some(t);
    let current = if d.InfoCount > 0 {
        valid(d.TemperatureInfo[0].Temperature)
    } else {
        None
    };
    Some((
        current,
        valid(d.WarningTemperature),
        valid(d.CriticalTemperature),
    ))
}

/// 读取 NVMe SMART / Health Information 日志页（512 字节）。
fn nvme_log(h: HANDLE) -> Option<Vec<u8>> {
    const LOG_LEN: usize = 512;
    let header = size_of::<u32>() * 2; // STORAGE_PROPERTY_QUERY 的 PropertyId + QueryType
    let spec_len = size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>();
    let mut buf = vec![0u8; header + spec_len + LOG_LEN];
    buf[0..4].copy_from_slice(&StorageDeviceProtocolSpecificProperty.0.to_le_bytes());
    buf[4..8].copy_from_slice(&PropertyStandardQuery.0.to_le_bytes());
    let spec = STORAGE_PROTOCOL_SPECIFIC_DATA {
        ProtocolType: ProtocolTypeNvme,
        DataType: NVMeDataTypeLogPage.0 as u32,
        ProtocolDataRequestValue: 0x02, // NVME_LOG_PAGE_HEALTH_INFO
        ProtocolDataRequestSubValue: 0,
        ProtocolDataOffset: spec_len as u32,
        ProtocolDataLength: LOG_LEN as u32,
        ..Default::default()
    };
    let spec_bytes = unsafe {
        std::slice::from_raw_parts(
            (&spec as *const STORAGE_PROTOCOL_SPECIFIC_DATA).cast::<u8>(),
            spec_len,
        )
    };
    buf[header..header + spec_len].copy_from_slice(spec_bytes);
    let input = buf.clone();
    let n = storage_query(h, &input, &mut buf)?;
    // 返回的是 STORAGE_PROTOCOL_DATA_DESCRIPTOR：Version、Size，后跟同样布局的协议数据，日志在其后。
    let start = header + spec_len;
    (n >= start + LOG_LEN).then(|| buf[start..start + LOG_LEN].to_vec())
}

fn nvme_health(h: HANDLE) -> Option<NvmeHealth> {
    let log = nvme_log(h)?;
    let u128_at = |off: usize| u128::from_le_bytes(log[off..off + 16].try_into().unwrap());
    // 数据单位 = 1000 个 512 字节扇区。
    let units = |off: usize| (u128_at(off) * 512_000).min(u64::MAX as u128) as u64;
    let count = |off: usize| u128_at(off).min(u64::MAX as u128) as u64;
    Some(NvmeHealth {
        critical_warning: log[0],
        available_spare: log[3],
        spare_threshold: log[4],
        percentage_used: log[5],
        data_read: units(32),
        data_written: units(48),
        power_cycles: count(112),
        power_on_hours: count(128),
        unsafe_shutdowns: count(144),
        media_errors: count(160),
    })
}

fn nvme_temperature(h: HANDLE) -> Option<i16> {
    let log = nvme_log(h)?;
    let kelvin = u16::from_le_bytes([log[1], log[2]]) as i16;
    (kelvin > 0).then_some(kelvin - 273)
}

/// 所有本地固定/可移动盘符及其所在的物理盘号（跨盘卷会在每块盘下各出现一次）。
fn volumes() -> Vec<(u32, Volume)> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
        // DRIVE_REMOVABLE = 2, DRIVE_FIXED = 3
        let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
        if kind != 2 && kind != 3 {
            continue;
        }
        let Some(handle) = open(&format!(r"\\.\{letter}:"), 0) else {
            continue;
        };
        let mut extents = vec![0u64; 64];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                raw(&handle),
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                None,
                0,
                Some(extents.as_mut_ptr().cast()),
                (extents.len() * 8) as u32,
                Some(&mut returned),
                None,
            )
        }
        .is_ok();
        if !ok {
            continue;
        }
        // VOLUME_DISK_EXTENTS：u32 数量（按 8 字节对齐），后跟 24 字节的 DISK_EXTENT 数组。
        let bytes =
            unsafe { std::slice::from_raw_parts(extents.as_ptr().cast::<u8>(), returned as usize) };
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let disks: Vec<u32> = (0..count)
            .filter_map(|k| bytes.get(8 + k * 24..12 + k * 24))
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();

        let (mut total, mut free) = (0u64, 0u64);
        unsafe {
            let _ = GetDiskFreeSpaceExW(
                PCWSTR(root.as_ptr()),
                None,
                Some(&mut total),
                Some(&mut free),
            );
        }
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        unsafe {
            let _ = GetVolumeInformationW(
                PCWSTR(root.as_ptr()),
                Some(&mut label),
                None,
                None,
                None,
                Some(&mut fs),
            );
        }
        let text = |w: &[u16]| {
            String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
        };
        let volume = Volume {
            letter,
            label: text(&label),
            file_system: text(&fs),
            total,
            free,
        };
        let mut seen = Vec::new();
        for disk in disks {
            if !seen.contains(&disk) {
                seen.push(disk);
                out.push((disk, volume.clone()));
            }
        }
    }
    out
}
