//! 网络适配器：名称、类型、链路速度、IP 地址与各自的收发速率。

use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, GetIfTable2, IP_ADAPTER_ADDRESSES_LH, MIB_IF_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

/// 适配器列表（名称、地址）变化很少，定期刷新即可。
const ADAPTER_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct NicStat {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub kind: &'static str,
    pub connected: bool,
    /// 链路速度（bit/s）。
    pub link_speed: u64,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub mac: String,
    pub rx_rate: u64,
    pub tx_rate: u64,
    /// 开机以来的累计收发字节。
    pub rx_total: u64,
    pub tx_total: u64,
    /// Wi-Fi 已连接网络的名称与信号质量（0–100）。
    pub ssid: Option<String>,
    pub signal: Option<u32>,
}

struct Adapter {
    index: u32,
    /// "{GUID}"，用来对上 WLAN 接口。
    guid: String,
    name: String,
    description: String,
    kind: &'static str,
    connected: bool,
    link_speed: u64,
    ipv4: Vec<String>,
    ipv6: Vec<String>,
    mac: String,
}

pub struct NicMonitor {
    adapters: Vec<Adapter>,
    /// 已连接的 Wi-Fi（接口 GUID, SSID, 信号），随适配器列表一起刷新：WLAN 查询要走 RPC，没必要每秒做。
    wifi: Vec<(String, String, u32)>,
    adapters_at: Option<Instant>,
    /// index → (时刻, 收, 发)
    prev: Vec<(u32, Instant, u64, u64)>,
}

impl Default for NicMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl NicMonitor {
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
            wifi: Vec::new(),
            adapters_at: None,
            prev: Vec::new(),
        }
    }

    pub fn sample(&mut self) -> Vec<NicStat> {
        if self
            .adapters_at
            .is_none_or(|t| t.elapsed() >= ADAPTER_INTERVAL)
        {
            self.adapters = adapters();
            self.wifi = if self.adapters.iter().any(|a| a.kind == "Wi-Fi") {
                wifi_connections()
            } else {
                Vec::new()
            };
            self.adapters_at = Some(Instant::now());
        }
        let counters = counters();
        let wifi = &self.wifi;
        let now = Instant::now();
        let mut next = Vec::with_capacity(self.adapters.len());
        let out = self
            .adapters
            .iter()
            .filter_map(|a| {
                let &(_, rx, tx) = counters.iter().find(|c| c.0 == a.index)?;
                let (rx_rate, tx_rate) = match self.prev.iter().find(|p| p.0 == a.index) {
                    Some(&(_, at, prx, ptx)) => {
                        let secs = now.duration_since(at).as_secs_f64().max(1e-3);
                        (
                            (rx.saturating_sub(prx) as f64 / secs) as u64,
                            (tx.saturating_sub(ptx) as f64 / secs) as u64,
                        )
                    }
                    None => (0, 0),
                };
                next.push((a.index, now, rx, tx));
                let wlan = wifi
                    .iter()
                    .find(|w| w.0.eq_ignore_ascii_case(a.guid.trim_matches(['{', '}'])));
                Some(NicStat {
                    index: a.index,
                    name: a.name.clone(),
                    description: a.description.clone(),
                    kind: a.kind,
                    connected: a.connected,
                    link_speed: a.link_speed,
                    ipv4: a.ipv4.clone(),
                    ipv6: a.ipv6.clone(),
                    mac: a.mac.clone(),
                    rx_rate,
                    tx_rate,
                    rx_total: rx,
                    tx_total: tx,
                    ssid: wlan.map(|w| w.1.clone()),
                    signal: wlan.map(|w| w.2),
                })
            })
            .collect();
        self.prev = next;
        out
    }
}

/// 物理网卡（硬件接口且非过滤驱动）的 (接口索引, 收, 发) 累计字节。
fn counters() -> Vec<(u32, u64, u64)> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut out = Vec::new();
    unsafe {
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        for row in rows {
            let flags = row.InterfaceAndOperStatusFlags._bitfield;
            if flags & 0b01 != 0 && flags & 0b10 == 0 {
                out.push((row.InterfaceIndex, row.InOctets, row.OutOctets));
            }
        }
        FreeMibTable(table as *const _);
    }
    out
}

fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut len = 16 * 1024u32;
    let mut buf: Vec<u64>;
    loop {
        buf = vec![0u64; (len as usize).div_ceil(8)];
        let r = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC.0 as u32,
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                &mut len,
            )
        };
        if r == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if r != ERROR_SUCCESS.0 {
            return Vec::new();
        }
        break;
    }
    let mut out = Vec::new();
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !cur.is_null() {
        let a = unsafe { &*cur };
        cur = a.Next;
        let kind = match a.IfType {
            6 => "以太网",
            71 => "Wi-Fi",
            243 | 244 => "蜂窝",
            _ => continue, // 回环、隧道等不展示
        };
        let index = unsafe { a.Anonymous1.Anonymous.IfIndex };
        let (mut ipv4, mut ipv6) = (Vec::new(), Vec::new());
        let mut u = a.FirstUnicastAddress;
        while !u.is_null() {
            let ua = unsafe { &*u };
            u = ua.Next;
            let sa = ua.Address.lpSockaddr;
            if sa.is_null() {
                continue;
            }
            unsafe {
                match (*sa).sa_family {
                    AF_INET => {
                        let s = &*(sa as *const SOCKADDR_IN);
                        ipv4.push(Ipv4Addr::from(s.sin_addr.S_un.S_addr.to_ne_bytes()).to_string());
                    }
                    AF_INET6 => {
                        let s = &*(sa as *const SOCKADDR_IN6);
                        ipv6.push(Ipv6Addr::from(s.sin6_addr.u.Byte).to_string());
                    }
                    _ => {}
                }
            }
        }
        let mac = a.PhysicalAddress[..(a.PhysicalAddressLength as usize).min(8)]
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        out.push(Adapter {
            index,
            guid: unsafe { a.AdapterName.to_string() }.unwrap_or_default(),
            name: unsafe { a.FriendlyName.to_string() }.unwrap_or_default(),
            description: unsafe { a.Description.to_string() }.unwrap_or_default(),
            kind,
            connected: a.OperStatus == IfOperStatusUp,
            link_speed: a.ReceiveLinkSpeed.max(a.TransmitLinkSpeed),
            ipv4,
            ipv6,
            mac,
        });
    }
    out
}

/// 已连接的 Wi-Fi：(接口 GUID 不带花括号, SSID, 信号质量)。
fn wifi_connections() -> Vec<(String, String, u32)> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
        WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, wlan_intf_opcode_current_connection,
    };
    let mut version = 0u32;
    let mut handle = HANDLE::default();
    if unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    if unsafe { WlanEnumInterfaces(handle, None, &mut list) } == 0 && !list.is_null() {
        let infos = unsafe {
            std::slice::from_raw_parts(
                (*list).InterfaceInfo.as_ptr(),
                (*list).dwNumberOfItems as usize,
            )
        };
        for info in infos {
            let mut size = 0u32;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            let r = unsafe {
                WlanQueryInterface(
                    handle,
                    &info.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                )
            };
            if r != 0 || data.is_null() {
                continue;
            }
            let attrs = unsafe { &*(data as *const WLAN_CONNECTION_ATTRIBUTES) };
            let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
            let name = String::from_utf8_lossy(&ssid.ucSSID[..(ssid.uSSIDLength as usize).min(32)])
                .to_string();
            let guid = format!("{:?}", info.InterfaceGuid);
            out.push((
                guid,
                name,
                attrs.wlanAssociationAttributes.wlanSignalQuality,
            ));
            unsafe { WlanFreeMemory(data) };
        }
        unsafe { WlanFreeMemory(list.cast()) };
    }
    unsafe { WlanCloseHandle(handle, None) };
    out
}
