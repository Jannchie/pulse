//! 网络：
//! - 系统总量：GetIfTable2 汇总物理网卡的收发字节，无需管理员。
//! - 逐进程：订阅 ETW Microsoft-Windows-Kernel-Network 的收发事件，需要管理员。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::thread::JoinHandle;

use crate::wide;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER,
    EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES,
    EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESS_TRACE_MODE_REAL_TIME, ProcessTrace, StartTraceW, WNODE_FLAG_TRACED_GUID,
};
use windows::core::{GUID, PCWSTR, PWSTR};

/// Microsoft-Windows-Kernel-Network
const PROVIDER: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
/// KERNEL_NETWORK_KEYWORD_IPV4 | KERNEL_NETWORK_KEYWORD_IPV6
const KEYWORDS: u64 = 0x10 | 0x20;
const SESSION: &str = "Pulse-KernelNetwork";
/// 让 FlushTimer 以毫秒计（windows crate 未导出该常量）。
const EVENT_TRACE_USE_MS_FLUSH_TIMER: u32 = 0x10;

/// pid → [接收, 发送] 累计字节。
static TOTALS: LazyLock<Mutex<HashMap<u32, [u64; 2]>>> = LazyLock::new(Default::default);

pub struct NetMonitor {
    control: CONTROLTRACE_HANDLE,
    thread: Option<JoinHandle<()>>,
}

impl NetMonitor {
    pub fn start() -> Result<Self, WIN32_ERROR> {
        let mut control = CONTROLTRACE_HANDLE::default();
        let mut props = Properties::new();
        let name = wide(SESSION);
        let mut err =
            unsafe { StartTraceW(&mut control, PCWSTR(name.as_ptr()), props.as_mut_ptr()) };
        if err == ERROR_ALREADY_EXISTS {
            // 上次异常退出残留的会话：ETW 实时会话不随进程结束。
            stop_session(CONTROLTRACE_HANDLE::default());
            props = Properties::new();
            err = unsafe { StartTraceW(&mut control, PCWSTR(name.as_ptr()), props.as_mut_ptr()) };
        }
        if err != ERROR_SUCCESS {
            return Err(err);
        }
        let err = unsafe {
            EnableTraceEx2(
                control,
                &PROVIDER,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
                4,
                KEYWORDS,
                0,
                0,
                None,
            )
        };
        if err != ERROR_SUCCESS {
            stop_session(control);
            return Err(err);
        }
        let thread = std::thread::Builder::new()
            .name("etw-network".into())
            .spawn(|| {
                let mut name = wide(SESSION);
                let mut logfile = EVENT_TRACE_LOGFILEW {
                    LoggerName: PWSTR(name.as_mut_ptr()),
                    ..Default::default()
                };
                logfile.Anonymous1.ProcessTraceMode =
                    PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                logfile.Anonymous2.EventRecordCallback = Some(on_event);
                let handle = unsafe { OpenTraceW(&mut logfile) };
                if handle.Value == u64::MAX {
                    return;
                }
                // 阻塞直到会话被停止。
                unsafe {
                    let _ = ProcessTrace(&[handle], None, None);
                    let _ = CloseTrace(handle);
                }
            })
            .ok();
        Ok(Self { control, thread })
    }

    /// 读取各进程累计收发字节，并丢掉已不存在的进程。
    pub fn totals(&self, alive: impl Fn(u32) -> bool) -> HashMap<u32, [u64; 2]> {
        let mut t = TOTALS.lock().unwrap();
        t.retain(|pid, _| alive(*pid));
        t.clone()
    }
}

impl Drop for NetMonitor {
    fn drop(&mut self) {
        stop_session(self.control);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn stop_session(control: CONTROLTRACE_HANDLE) {
    let name = wide(SESSION);
    let mut props = Properties::new();
    unsafe {
        let _ = ControlTraceW(
            control,
            PCWSTR(name.as_ptr()),
            props.as_mut_ptr(),
            EVENT_TRACE_CONTROL_STOP,
        );
    }
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    let record = unsafe { &*record };
    // TCP/UDP × IPv4/IPv6 的发送与接收。负载布局：PID(u32), size(u32), daddr, saddr, ...
    let (dir, v6) = match record.EventHeader.EventDescriptor.Id {
        11 | 43 => (0, false),
        10 | 42 => (1, false),
        27 | 59 => (0, true),
        26 | 58 => (1, true),
        _ => return,
    };
    let addr_len = if v6 { 16 } else { 4 };
    if (record.UserDataLength as usize) < 8 + addr_len * 2 || record.UserData.is_null() {
        return;
    }
    let data = unsafe {
        std::slice::from_raw_parts(record.UserData as *const u8, record.UserDataLength as usize)
    };
    // 本机回环（localhost 开发服务器、进程间通信）不经过网卡，任务管理器也不计入。
    let daddr = &data[8..8 + addr_len];
    let saddr = &data[8 + addr_len..8 + addr_len * 2];
    if is_loopback(daddr) || is_loopback(saddr) {
        return;
    }
    let pid = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let size = u32::from_le_bytes(data[4..8].try_into().unwrap());
    TOTALS.lock().unwrap().entry(pid).or_default()[dir] += size as u64;
}

fn is_loopback(addr: &[u8]) -> bool {
    match addr.len() {
        4 => addr[0] == 127,
        16 => addr[..15].iter().all(|&b| b == 0) && addr[15] == 1,
        _ => false,
    }
}

/// EVENT_TRACE_PROPERTIES 后面必须紧跟会话名的空间。
struct Properties(Vec<u64>);

impl Properties {
    fn new() -> Self {
        let header = size_of::<EVENT_TRACE_PROPERTIES>();
        let total = header + (SESSION.len() + 1) * 2;
        let mut buf = vec![0u64; total.div_ceil(8)];
        let p = unsafe { &mut *(buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES) };
        p.Wnode.BufferSize = total as u32;
        p.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        p.Wnode.ClientContext = 1;
        p.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_USE_MS_FLUSH_TIMER;
        // 默认 1 秒才刷一次缓冲，会让网络列比其他列慢一拍。
        p.FlushTimer = 200;
        p.BufferSize = 64;
        p.LoggerNameOffset = header as u32;
        Self(buf)
    }

    fn as_mut_ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.0.as_mut_ptr().cast()
    }
}

/// 所有物理网卡的累计 [接收, 发送] 字节。
pub fn interface_totals() -> [u64; 2] {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != ERROR_SUCCESS {
        return [0, 0];
    }
    let mut out = [0u64; 2];
    unsafe {
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        for row in rows {
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            // HardwareInterface 且不是 FilterInterface，避免同一块网卡被过滤驱动重复计数。
            if flags & 0b01 != 0 && flags & 0b10 == 0 {
                out[0] += row.InOctets;
                out[1] += row.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
    }
    out
}
