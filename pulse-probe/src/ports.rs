//! 监听端口：TCP 处于 LISTEN 的端口与已绑定的 UDP 端口，按 PID 归类。无需管理员。

use std::collections::HashMap;

use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER, UDP_TABLE_OWNER_PID,
};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Port {
    pub proto: Proto,
    pub port: u16,
}

/// pid → 去重排序后的监听端口（TCP 在前）。
pub fn listening() -> HashMap<u32, Vec<Port>> {
    let mut out: HashMap<u32, Vec<Port>> = HashMap::new();
    let mut add = |pid: u32, proto: Proto, raw_port: u32| {
        // 端口以网络字节序存放在低 16 位。
        let port = u16::from_be(raw_port as u16);
        out.entry(pid).or_default().push(Port { proto, port });
    };
    for row in table::<MIB_TCPROW_OWNER_PID>(|b, s| unsafe {
        GetExtendedTcpTable(b, s, false, AF_INET, TCP_TABLE_OWNER_PID_LISTENER, 0)
    }) {
        add(row.dwOwningPid, Proto::Tcp, row.dwLocalPort);
    }
    for row in table::<MIB_TCP6ROW_OWNER_PID>(|b, s| unsafe {
        GetExtendedTcpTable(b, s, false, AF_INET6, TCP_TABLE_OWNER_PID_LISTENER, 0)
    }) {
        add(row.dwOwningPid, Proto::Tcp, row.dwLocalPort);
    }
    for row in table::<MIB_UDPROW_OWNER_PID>(|b, s| unsafe {
        GetExtendedUdpTable(b, s, false, AF_INET, UDP_TABLE_OWNER_PID, 0)
    }) {
        add(row.dwOwningPid, Proto::Udp, row.dwLocalPort);
    }
    for row in table::<MIB_UDP6ROW_OWNER_PID>(|b, s| unsafe {
        GetExtendedUdpTable(b, s, false, AF_INET6, UDP_TABLE_OWNER_PID, 0)
    }) {
        add(row.dwOwningPid, Proto::Udp, row.dwLocalPort);
    }
    for ports in out.values_mut() {
        ports.sort();
        ports.dedup();
    }
    out
}

/// 调用"先问大小再取数据"风格的 IP Helper 表函数。表头是 u32 条目数，紧跟行数组。
fn table<Row: Copy>(call: impl Fn(Option<*mut core::ffi::c_void>, *mut u32) -> u32) -> Vec<Row> {
    let mut size = 0u32;
    for _ in 0..4 {
        // 用 u32 缓冲保证对齐；两次调用之间表可能变大，所以循环重试。
        let mut buf = vec![0u32; (size as usize).div_ceil(4).max(1)];
        let err = call(Some(buf.as_mut_ptr().cast()), &mut size);
        if err == 0 {
            let n = buf[0] as usize;
            let rows = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(1) as *const Row, n) };
            return rows.to_vec();
        }
        // ERROR_INSUFFICIENT_BUFFER：size 已更新为所需大小。
        if err != 122 {
            break;
        }
    }
    Vec::new()
}
