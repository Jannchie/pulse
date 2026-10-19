//! 采集层：一次 `NtQuerySystemInformation(SystemProcessInformation)` 拿到全部进程，
//! 与上一次快照做差得到 CPU / I/O / GPU / 网络速率。不依赖任何 UI。

pub mod autostart;
pub mod control;
pub mod disk;
pub mod fonts;
pub mod foreground;
pub mod games;
pub mod gpu;
pub mod i18n;
pub mod icon;
pub mod locks;
pub mod net;
pub mod nic;
pub mod os;
pub mod ports;
pub mod power;
pub mod runtime;
pub mod services;
pub mod sessions;
pub mod startup;
pub mod system;

use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation, SystemProcessorPerformanceInformation,
};
use windows::Win32::Foundation::{FILETIME, HANDLE, STATUS_INFO_LENGTH_MISMATCH};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetSystemTimes,
};

pub use windows::core::Error as WinError;

/// Rust 字符串 → 以 0 结尾的 UTF-16，供各种 W 版 Win32 API 使用。
/// Win32 错误码对应的系统错误信息。
pub(crate) fn win32_message(code: u32) -> String {
    windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message()
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub use os::{is_elevated, kill, relaunch_elevated, reveal_in_explorer};

/// phnt 中的完整 SYSTEM_PROCESS_INFORMATION（windows crate 里那份把时间字段藏成了 Reserved）。
#[repr(C)]
struct RawProcessInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_len: u16,
    image_name_max_len: u16,
    image_name_buf: *const u16,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

/// SYSTEM_THREAD_INFORMATION，紧跟在每个 SYSTEM_PROCESS_INFORMATION 之后，共 NumberOfThreads 个。
#[repr(C)]
struct RawThreadInfo {
    kernel_time: i64,
    user_time: i64,
    create_time: i64,
    wait_time: u32,
    start_address: usize,
    client_process: usize,
    client_thread: usize,
    priority: i32,
    base_priority: i32,
    context_switches: u32,
    state: u32,
    wait_reason: u32,
}

/// KTHREAD_STATE::Waiting 与 KWAIT_REASON::Suspended
const THREAD_WAITING: u32 = 5;
const WAIT_SUSPENDED: u32 = 5;

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    /// 可执行文件完整路径；受保护进程或无权限时为 None。
    pub path: Option<Arc<str>>,
    /// exe 的文件描述，例如 Code.exe → "Visual Studio Code"。
    pub description: Option<Arc<str>>,
    pub command_line: Option<Arc<str>>,
    /// 工作目录；只对 node、python 等运行时读取（用来认出项目）。
    pub cwd: Option<Arc<str>>,
    /// 比 exe 名更具体的身份：node.exe → "vite · pulse"，Chromium 子进程 → "GPU 进程"。
    pub identity: Option<runtime::Identity>,
    /// 监听中的 TCP 端口与已绑定的 UDP 端口。
    pub ports: Vec<ports::Port>,
    /// 占全部逻辑核心的百分比（0–100），与任务管理器口径一致。
    pub cpu: f32,
    /// 私有工作集（任务管理器"内存"列）。
    pub private_ws: u64,
    pub working_set: u64,
    /// 提交大小（任务管理器"提交大小"列）：进程独占、计入系统提交量的内存。
    pub commit: u64,
    /// 所有 I/O（读+写+其他）的字节/秒。
    pub io_rate: u64,
    /// 最忙 GPU 引擎的占用率（0–100）。
    pub gpu: f32,
    /// 进程提交的专用显存 / 共享 GPU 内存。
    pub gpu_dedicated: u64,
    pub gpu_shared: u64,
    /// 累计 CPU 时间（用户态、内核态），100ns 为单位。
    pub user_time: u64,
    pub kernel_time: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    /// 自上次采样以来收发的字节数（统计每天的流量用）。
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub create_time: i64,
    pub priority: control::Priority,
    /// 所有线程都被挂起（与任务管理器"已挂起"状态一致）。
    pub suspended: bool,
    /// 效率模式（EcoQoS）。
    pub efficiency: bool,
    /// 有可见主窗口的进程即"应用"。
    pub window: Option<control::AppWindow>,
}

#[derive(Clone, Debug)]
pub struct GpuStat {
    pub name: String,
    pub usage: f32,
    /// 各类引擎（3D、复制、视频解码……）的利用率。
    pub engines: Vec<(String, f32)>,
    pub dedicated_used: u64,
    pub dedicated_total: u64,
    pub shared_used: u64,
    pub shared_total: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum NetStatus {
    #[default]
    Running,
    NeedsAdmin,
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub processes: Vec<Process>,
    pub cpu_total: f32,
    pub cpu_count: u32,
    /// 每个逻辑处理器的占用率（0–100），仅当前处理器组（≤64 核）。
    pub cpu_cores: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// 已提交内存与提交上限（物理内存 + 页面文件）。
    pub commit_used: u64,
    pub commit_limit: u64,
    pub gpus: Vec<GpuStat>,
    /// 所有适配器中最高的利用率。
    pub gpu_total: f32,
    pub net_rx: u64,
    pub net_tx: u64,
    /// 逐进程网络统计是否可用。
    pub net_status: NetStatus,
    pub elevated: bool,
    /// 本次采集耗时。
    pub collect_time: Duration,
    pub cpu_info: Arc<system::CpuInfo>,
    /// 驱动报告的当前频率（MHz），不少平台上固定为基准频率。
    pub cpu_mhz: Option<u32>,
    pub uptime: Duration,
    pub memory: system::MemoryDetail,
    pub memory_modules: Arc<Vec<system::MemoryModule>>,
    /// 内存插槽总数（含空插槽），读不到时为 0。
    pub memory_slots: u32,
    /// 安装的物理内存；减去可用总量即"为硬件保留"。
    pub memory_installed: Option<u64>,
    pub disks: Vec<disk::DiskStat>,
    pub nics: Vec<nic::NicStat>,
}

/// 跨采样保留的每进程状态，以 (pid, 创建时间) 为身份，避免 PID 复用串数据。
struct Entry {
    cpu_time: i64,
    io_bytes: i64,
    net: [u64; 2],
    gpu_times: Vec<i64>,
    /// 一直开着，GPU 统计每秒都要用，省去反复 OpenProcess。
    handle: Option<OwnedHandle>,
    path: Option<Arc<str>>,
    description: Option<Arc<str>>,
    command_line: Option<Arc<str>>,
    cwd: Option<Arc<str>>,
    identity: Option<runtime::Identity>,
}

pub struct Collector {
    buf: Vec<u64>,
    entries: HashMap<(u32, i64), Entry>,
    /// 文件描述按路径缓存：几十个 svchost 只读一次版本资源。
    descriptions: HashMap<Arc<str>, Option<Arc<str>>>,
    prev_at: Option<Instant>,
    prev_sys: Option<(u64, u64, u64)>,
    /// 每核上一次的 (空闲, 总) 时间。
    prev_cores: Vec<(i64, i64)>,
    prev_if: Option<[u64; 2]>,
    cpu_count: u32,
    gpu: Option<gpu::GpuMonitor>,
    net: Option<net::NetMonitor>,
    net_status: NetStatus,
    elevated: bool,
    cpu_info: Arc<system::CpuInfo>,
    memory_modules: Arc<Vec<system::MemoryModule>>,
    memory_slots: u32,
    memory_installed: Option<u64>,
    disks: disk::DiskMonitor,
    nics: nic::NicMonitor,
    wsl: runtime::WslWatcher,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        let elevated = os::is_elevated();
        let (modules, slots) = system::memory_modules();
        let (net, net_status) = if !elevated {
            (None, NetStatus::NeedsAdmin)
        } else {
            match net::NetMonitor::start() {
                Ok(m) => (Some(m), NetStatus::Running),
                Err(e) => (None, NetStatus::Failed(e.0.to_string())),
            }
        };
        Self {
            // 用 u64 保证 8 字节对齐。
            buf: vec![0; 256 * 1024 / 8],
            entries: HashMap::new(),
            descriptions: HashMap::new(),
            prev_at: None,
            prev_sys: None,
            prev_cores: Vec::new(),
            prev_if: None,
            cpu_count: unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1),
            gpu: gpu::GpuMonitor::new(),
            net,
            net_status,
            elevated,
            cpu_info: Arc::new(system::CpuInfo::query()),
            memory_modules: Arc::new(modules),
            memory_slots: slots,
            memory_installed: system::installed_memory(),
            disks: disk::DiskMonitor::new(),
            nics: nic::NicMonitor::new(),
            wsl: runtime::WslWatcher::default(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let started = Instant::now();
        self.query();

        let elapsed_100ns = self
            .prev_at
            .map(|t| started.duration_since(t).as_nanos() as f64 / 100.0)
            .unwrap_or(0.0);
        let elapsed_secs = elapsed_100ns / 1e7;
        let cpu_capacity = elapsed_100ns * self.cpu_count as f64;
        let rate = |now: u64, prev: u64| {
            if elapsed_secs > 0.0 {
                (now.saturating_sub(prev) as f64 / elapsed_secs) as u64
            } else {
                0
            }
        };

        let gpus: Vec<GpuStat> = match &mut self.gpu {
            Some(g) => {
                let samples = g.sample_global(elapsed_100ns);
                g.adapters
                    .iter()
                    .zip(samples)
                    .map(|(a, s)| GpuStat {
                        name: a.name.clone(),
                        usage: s.usage,
                        engines: a
                            .engines
                            .iter()
                            .map(gpu::Engine::name)
                            .zip(s.engines)
                            .collect(),
                        dedicated_used: s.dedicated_used,
                        dedicated_total: a.dedicated_total,
                        shared_used: s.shared_used,
                        shared_total: a.shared_total,
                    })
                    .collect()
            }
            None => Vec::new(),
        };

        let mut ports = ports::listening();
        let windows = control::app_windows();
        let mut processes = Vec::with_capacity(self.entries.len().max(256));
        let mut next = HashMap::with_capacity(self.entries.len());
        let base = self.buf.as_ptr() as *const u8;
        let mut offset = 0usize;
        loop {
            let raw = unsafe { &*(base.add(offset) as *const RawProcessInfo) };
            let pid = raw.unique_process_id as u32;
            let key = (pid, raw.create_time);
            let cpu_time = raw.user_time + raw.kernel_time;
            let io_bytes =
                raw.read_transfer_count + raw.write_transfer_count + raw.other_transfer_count;

            let (mut entry, is_new) = match self.entries.remove(&key) {
                Some(e) => (e, false),
                None => {
                    let handle = if pid > 4 { os::open_query(pid) } else { None };
                    let path: Option<Arc<str>> =
                        handle.as_ref().and_then(os::image_path).map(Arc::from);
                    let description = path.as_ref().and_then(|p| {
                        self.descriptions
                            .entry(p.clone())
                            .or_insert_with(|| os::file_description(p).map(Arc::from))
                            .clone()
                    });
                    let command_line = handle.as_ref().and_then(os::command_line).map(Arc::from);
                    let name = image_name(raw);
                    let cwd = runtime::wants_cwd(&name)
                        .then(|| locks::current_directory(pid))
                        .flatten()
                        .map(Arc::from);
                    let identity = command_line.as_deref().and_then(|cmd| {
                        runtime::identify(&name, path.as_deref(), cmd, cwd.as_deref())
                    });
                    let entry = Entry {
                        cpu_time,
                        io_bytes,
                        net: [0, 0],
                        gpu_times: Vec::new(),
                        handle,
                        path,
                        description,
                        command_line,
                        cwd,
                        identity,
                    };
                    (entry, true)
                }
            };

            let cpu = if !is_new && cpu_capacity > 0.0 {
                ((cpu_time - entry.cpu_time).max(0) as f64 / cpu_capacity * 100.0) as f32
            } else {
                0.0
            };
            let io_rate = if is_new {
                0
            } else {
                rate(io_bytes as u64, entry.io_bytes as u64)
            };
            entry.cpu_time = cpu_time;
            entry.io_bytes = io_bytes;

            let mut gpu = 0.0;
            let mut gpu_mem = (0, 0);
            if let (Some(g), Some(h)) = (&self.gpu, &entry.handle) {
                let prev = entry.gpu_times.clone();
                // 新进程查全部节点建立基线；老进程只查本轮活跃的节点。
                g.query_process(
                    HANDLE(h.as_raw_handle()),
                    &mut entry.gpu_times,
                    is_new || prev.is_empty(),
                );
                if !is_new {
                    gpu = gpu::process_usage(&prev, &entry.gpu_times, elapsed_100ns);
                }
                // 只有真正用过 GPU 的进程才查显存，其余几百个进程查询必然失败，白费系统调用。
                if entry.gpu_times.iter().any(|&t| t > 0) {
                    gpu_mem = g.process_memory(HANDLE(h.as_raw_handle()));
                }
            }

            let threads = unsafe {
                std::slice::from_raw_parts(
                    base.add(offset + size_of::<RawProcessInfo>()) as *const RawThreadInfo,
                    raw.number_of_threads as usize,
                )
            };
            let suspended = pid > 4
                && !threads.is_empty()
                && threads
                    .iter()
                    .all(|t| t.state == THREAD_WAITING && t.wait_reason == WAIT_SUSPENDED);
            let efficiency = entry
                .handle
                .as_ref()
                .is_some_and(|h| control::efficiency_mode(HANDLE(h.as_raw_handle())));

            if pid != 0 {
                // pid 0 是 Idle，不展示（它的 CPU 就是空闲率）。
                processes.push(Process {
                    pid,
                    parent_pid: raw.inherited_from_unique_process_id as u32,
                    name: image_name(raw),
                    path: entry.path.clone(),
                    description: entry.description.clone(),
                    command_line: entry.command_line.clone(),
                    cwd: entry.cwd.clone(),
                    identity: entry.identity.clone(),
                    ports: ports.remove(&pid).unwrap_or_default(),
                    cpu,
                    private_ws: raw.working_set_private_size as u64,
                    working_set: raw.working_set_size as u64,
                    commit: raw.private_page_count as u64,
                    io_rate,
                    gpu,
                    gpu_dedicated: gpu_mem.0,
                    gpu_shared: gpu_mem.1,
                    user_time: raw.user_time as u64,
                    kernel_time: raw.kernel_time as u64,
                    net_rx: 0,
                    net_tx: 0,
                    net_rx_bytes: 0,
                    net_tx_bytes: 0,
                    threads: raw.number_of_threads,
                    handles: raw.handle_count,
                    session_id: raw.session_id,
                    create_time: raw.create_time,
                    priority: control::Priority::from_base(raw.base_priority),
                    suspended,
                    efficiency,
                    window: windows.get(&pid).cloned(),
                });
            }
            next.insert(key, entry);

            if raw.next_entry_offset == 0 {
                break;
            }
            offset += raw.next_entry_offset as usize;
        }
        // 剩下的是已退出的进程，随 self.entries 一起丢弃并关闭句柄。
        self.entries = next;

        if let Some(net) = &self.net {
            let alive: std::collections::HashSet<u32> = processes.iter().map(|p| p.pid).collect();
            let totals = net.totals(|pid| alive.contains(&pid));
            for p in &mut processes {
                let now = totals.get(&p.pid).copied().unwrap_or([0, 0]);
                let entry = self.entries.get_mut(&(p.pid, p.create_time)).unwrap();
                if self.prev_at.is_some() {
                    p.net_rx = rate(now[0], entry.net[0]);
                    p.net_tx = rate(now[1], entry.net[1]);
                    p.net_rx_bytes = now[0].saturating_sub(entry.net[0]);
                    p.net_tx_bytes = now[1].saturating_sub(entry.net[1]);
                }
                entry.net = now;
            }
        }

        // WSL 2 的虚拟机进程：标上运行中的发行版。
        if let Some(vm) = processes
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case("vmmemWSL"))
        {
            vm.identity = Some(runtime::Identity::Wsl(self.wsl.distros()));
        }

        let iface = net::interface_totals();
        let (net_rx, net_tx) = match self.prev_if {
            Some(prev) => (rate(iface[0], prev[0]), rate(iface[1], prev[1])),
            None => (0, 0),
        };
        self.prev_if = Some(iface);

        let cpu_total = self.system_cpu();
        let cpu_cores = self.core_usage();
        let mem = memory();
        self.prev_at = Some(started);

        Snapshot {
            processes,
            cpu_total,
            cpu_count: self.cpu_count,
            cpu_cores,
            mem_used: mem.used,
            mem_total: mem.total,
            commit_used: mem.commit_used,
            commit_limit: mem.commit_limit,
            gpu_total: gpus.iter().map(|g| g.usage).fold(0.0, f32::max),
            gpus,
            net_rx,
            net_tx,
            net_status: self.net_status.clone(),
            elevated: self.elevated,
            cpu_mhz: system::current_mhz(self.cpu_info.logical),
            cpu_info: self.cpu_info.clone(),
            uptime: system::uptime(),
            memory: system::MemoryDetail::query(),
            memory_modules: self.memory_modules.clone(),
            memory_slots: self.memory_slots,
            memory_installed: self.memory_installed,
            disks: self.disks.sample(),
            nics: self.nics.sample(),
            collect_time: started.elapsed(),
        }
    }

    fn query(&mut self) {
        loop {
            let mut needed = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buf.as_mut_ptr().cast(),
                    (self.buf.len() * 8) as u32,
                    &mut needed,
                )
            };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                // 两次调用之间可能有新进程，多留些余量。
                let bytes = needed as usize + 64 * 1024;
                self.buf.resize(bytes.div_ceil(8), 0);
                continue;
            }
            assert!(
                status.is_ok(),
                "NtQuerySystemInformation failed: {status:?}"
            );
            return;
        }
    }

    fn core_usage(&mut self) -> Vec<f32> {
        /// SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct Core {
            idle: i64,
            kernel: i64, // 含 idle
            user: i64,
            dpc: i64,
            interrupt: i64,
            interrupt_count: u32,
        }
        let mut cores = vec![Core::default(); 64];
        let mut len = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessorPerformanceInformation,
                cores.as_mut_ptr().cast(),
                (cores.len() * size_of::<Core>()) as u32,
                &mut len,
            )
        };
        if status.is_err() {
            return Vec::new();
        }
        cores.truncate(len as usize / size_of::<Core>());
        let now: Vec<(i64, i64)> = cores.iter().map(|c| (c.idle, c.kernel + c.user)).collect();
        let usage = if self.prev_cores.len() == now.len() {
            now.iter()
                .zip(&self.prev_cores)
                .map(|(n, p)| {
                    let total = (n.1 - p.1).max(1) as f32;
                    ((1.0 - (n.0 - p.0) as f32 / total) * 100.0).clamp(0.0, 100.0)
                })
                .collect()
        } else {
            vec![0.0; now.len()]
        };
        self.prev_cores = now;
        usage
    }

    fn system_cpu(&mut self) -> f32 {
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            return 0.0;
        }
        let now = (ft(idle), ft(kernel), ft(user));
        let result = match self.prev_sys {
            Some((pi, pk, pu)) => {
                // kernel 时间包含 idle。
                let total = (now.1 - pk) + (now.2 - pu);
                let busy = total.saturating_sub(now.0 - pi);
                if total == 0 {
                    0.0
                } else {
                    busy as f32 / total as f32 * 100.0
                }
            }
            None => 0.0,
        };
        self.prev_sys = Some(now);
        result
    }
}

fn image_name(raw: &RawProcessInfo) -> String {
    if raw.image_name_buf.is_null() || raw.image_name_len == 0 {
        return match raw.unique_process_id {
            4 => "System".into(),
            _ => "<unknown>".into(),
        };
    }
    let units =
        unsafe { std::slice::from_raw_parts(raw.image_name_buf, raw.image_name_len as usize / 2) };
    String::from_utf16_lossy(units)
}

fn ft(t: FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

#[derive(Default)]
struct Memory {
    used: u64,
    total: u64,
    commit_used: u64,
    commit_limit: u64,
}

fn memory() -> Memory {
    let mut s = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    match unsafe { GlobalMemoryStatusEx(&mut s) } {
        Ok(()) => Memory {
            used: s.ullTotalPhys - s.ullAvailPhys,
            total: s.ullTotalPhys,
            commit_used: s.ullTotalPageFile - s.ullAvailPageFile,
            commit_limit: s.ullTotalPageFile,
        },
        Err(_) => Memory::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_phnt() {
        assert_eq!(std::mem::offset_of!(RawProcessInfo, image_name_len), 56);
        assert_eq!(std::mem::offset_of!(RawProcessInfo, unique_process_id), 80);
        assert_eq!(
            std::mem::offset_of!(RawProcessInfo, other_transfer_count),
            248
        );
        assert_eq!(size_of::<RawProcessInfo>(), 256);
        assert_eq!(std::mem::offset_of!(RawThreadInfo, state), 68);
        assert_eq!(size_of::<RawThreadInfo>(), 80);
    }

    #[test]
    fn sample_finds_self() {
        let mut c = Collector::new();
        let s = c.sample();
        let me = std::process::id();
        let p = s
            .processes
            .iter()
            .find(|p| p.pid == me)
            .expect("self in snapshot");
        assert!(p.path.as_deref().is_some_and(|path| path.ends_with(".exe")));
        assert!(s.mem_total > 0);
    }

    #[test]
    fn icon_from_self() {
        // 测试二进制没有图标资源，用 explorer 代替。
        let icon = icon::extract(r"C:\Windows\explorer.exe", 32).expect("explorer icon");
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        assert!(icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }
}
