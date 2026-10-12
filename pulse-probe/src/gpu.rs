//! GPU：D3DKMTQueryStatistics 读取引擎（node）累计运行时间与显存段（segment）用量。
//! 口径与任务管理器一致：
//! - 进程/适配器的利用率 = 其最忙那个引擎的占用率；同类引擎（如多个 Copy）合并取最忙者。
//! - 专用显存 = 非 aperture 段的驻留字节；共享 GPU 内存 = aperture 段（系统内存）的驻留字节。

use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTERADDRESS, D3DKMT_CLOSEADAPTER, D3DKMT_NODEMETADATA, D3DKMT_OPENADAPTERFROMLUID,
    D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER,
    D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_PROCESS_NODE,
    D3DKMT_QUERYSTATISTICS_PROCESS_SEGMENT, D3DKMT_QUERYSTATISTICS_SEGMENT, D3DKMTCloseAdapter,
    D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics, DXGK_ENGINE_TYPE,
    DXGK_ENGINE_TYPE_3D, DXGK_ENGINE_TYPE_COPY, DXGK_ENGINE_TYPE_CRYPTO, DXGK_ENGINE_TYPE_OVERLAY,
    DXGK_ENGINE_TYPE_SCENE_ASSEMBLY, DXGK_ENGINE_TYPE_VIDEO_DECODE, DXGK_ENGINE_TYPE_VIDEO_ENCODE,
    DXGK_ENGINE_TYPE_VIDEO_PROCESSING, KMTQAITYPE_ADAPTERADDRESS, KMTQAITYPE_NODEMETADATA,
};
use windows::Win32::Foundation::{HANDLE, LUID};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

/// 一类引擎：名称与它包含的节点（在本适配器内的序号）。
pub struct Engine {
    name: EngineName,
    nodes: Vec<usize>,
}

/// 引擎名称的来源；已知类型的名称随界面语言变化，所以不在枚举时就定成字符串。
#[derive(PartialEq)]
enum EngineName {
    Kind(DXGK_ENGINE_TYPE),
    Friendly(String),
    Node(usize),
}

impl Engine {
    pub fn name(&self) -> String {
        use crate::{t, tf};
        match &self.name {
            EngineName::Kind(kind) => match *kind {
                DXGK_ENGINE_TYPE_3D => "3D",
                DXGK_ENGINE_TYPE_VIDEO_DECODE => t!("视频解码", "Video Decode"),
                DXGK_ENGINE_TYPE_VIDEO_ENCODE => t!("视频编码", "Video Encode"),
                DXGK_ENGINE_TYPE_VIDEO_PROCESSING => t!("视频处理", "Video Processing"),
                DXGK_ENGINE_TYPE_SCENE_ASSEMBLY => t!("场景组装", "Scene Assembly"),
                DXGK_ENGINE_TYPE_COPY => t!("复制", "Copy"),
                DXGK_ENGINE_TYPE_OVERLAY => t!("叠加", "Overlay"),
                _ => t!("加密", "Crypto"),
            }
            .to_string(),
            EngineName::Friendly(s) => s.clone(),
            EngineName::Node(n) => tf!("引擎 {n}", "Engine {n}"),
        }
    }
}

pub struct Adapter {
    pub name: String,
    pub engines: Vec<Engine>,
    pub dedicated_total: u64,
    pub shared_total: u64,
    luid: LUID,
    pci: bool,
    /// 在扁平化节点数组中的起始下标。
    node_base: usize,
    node_count: usize,
    /// (段 ID, 是否 aperture/系统内存)
    segments: Vec<(u32, bool)>,
}

/// 一次全局采样中某个适配器的状态。
pub struct AdapterSample {
    pub usage: f32,
    /// 与 `Adapter::engines` 一一对应的利用率（0–100）。
    pub engines: Vec<f32>,
    pub dedicated_used: u64,
    pub shared_used: u64,
}

pub struct GpuMonitor {
    pub adapters: Vec<Adapter>,
    total_nodes: usize,
    prev_global: Vec<i64>,
    /// 本轮全局运行时间有变化的节点。没动过的节点不可能有进程在用，可以跳过逐进程查询。
    active: Vec<bool>,
}

impl GpuMonitor {
    pub fn new() -> Option<Self> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
        let mut adapters = Vec::new();
        let mut seen: Vec<[u32; 3]> = Vec::new();
        for i in 0.. {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
                break;
            };
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let luid = desc.AdapterLuid;
            if adapters.iter().any(|a: &Adapter| a.luid == luid) {
                continue;
            }
            let q = D3DKMT_QUERYSTATISTICS {
                Type: D3DKMT_QUERYSTATISTICS_ADAPTER,
                AdapterLuid: luid,
                ..Default::default()
            };
            if unsafe { D3DKMTQueryStatistics(&q) }.is_err() {
                continue;
            }
            let info = unsafe { q.QueryResult.AdapterInformation };
            let node_count = info.NodeCount as usize;
            let pci = pci_address(luid);
            if pci.is_some_and(|a| seen.contains(&a)) {
                continue;
            }
            seen.extend(pci);
            let segments = (0..info.NbSegments)
                .filter_map(|id| query_segment(luid, id).map(|s| (id, s.Aperture != 0)))
                .collect();
            let len = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            adapters.push(Adapter {
                name: String::from_utf16_lossy(&desc.Description[..len])
                    .trim()
                    .to_string(),
                engines: engines(luid, node_count),
                dedicated_total: desc.DedicatedVideoMemory as u64,
                shared_total: desc.SharedSystemMemory as u64,
                luid,
                pci: pci.is_some(),
                node_base: 0,
                node_count,
                segments,
            });
        }
        // 同一块物理 GPU 可能以多个 LUID 出现（例如装了虚拟显示驱动），虚拟的那个没有 PCI 地址，
        // 统计却镜像真实显卡。有 PCI 显卡时丢掉无 PCI 地址的适配器，避免重复显示和加倍查询；
        // 只有 ACPI 枚举 GPU 的设备（如 ARM 笔记本）则全部保留。
        if adapters.iter().any(|a| a.pci) {
            adapters.retain(|a| a.pci);
        }
        let mut total_nodes = 0;
        for a in &mut adapters {
            a.node_base = total_nodes;
            total_nodes += a.node_count;
        }
        if adapters.is_empty() {
            return None;
        }
        Some(Self {
            adapters,
            total_nodes,
            prev_global: Vec::new(),
            active: vec![true; total_nodes],
        })
    }

    pub fn node_count(&self) -> usize {
        self.total_nodes
    }

    /// 刷新全局节点时间与显存段用量。
    pub fn sample_global(&mut self, elapsed_100ns: f64) -> Vec<AdapterSample> {
        let mut now = vec![0i64; self.total_nodes];
        for a in &self.adapters {
            for n in 0..a.node_count {
                now[a.node_base + n] = query_node(a.luid, None, n as u32).unwrap_or(0);
            }
        }
        let first = self.prev_global.is_empty();
        let node_usage = |i: usize| {
            if first || elapsed_100ns <= 0.0 {
                0.0
            } else {
                ((now[i] - self.prev_global[i]).max(0) as f64 / elapsed_100ns * 100.0).min(100.0)
                    as f32
            }
        };
        let samples = self
            .adapters
            .iter()
            .map(|a| {
                let usage = (0..a.node_count)
                    .map(|n| node_usage(a.node_base + n))
                    .fold(0.0, f32::max);
                let engines = a
                    .engines
                    .iter()
                    .map(|e| {
                        e.nodes
                            .iter()
                            .map(|&n| node_usage(a.node_base + n))
                            .fold(0.0, f32::max)
                    })
                    .collect();
                let (mut dedicated_used, mut shared_used) = (0, 0);
                for &(id, aperture) in &a.segments {
                    if let Some(s) = query_segment(a.luid, id) {
                        *if aperture {
                            &mut shared_used
                        } else {
                            &mut dedicated_used
                        } += s.BytesResident;
                    }
                }
                AdapterSample {
                    usage,
                    engines,
                    dedicated_used,
                    shared_used,
                }
            })
            .collect();
        for (i, active) in self.active.iter_mut().enumerate() {
            *active = first || now[i] != self.prev_global[i];
        }
        self.prev_global = now;
        samples
    }

    /// 读取某进程在各节点上的累计运行时间。`all` 为 false 时只查本轮活动过的节点，其余保留旧值。
    pub fn query_process(&self, process: HANDLE, times: &mut Vec<i64>, all: bool) {
        times.resize(self.total_nodes, 0);
        for a in &self.adapters {
            for n in 0..a.node_count {
                let i = a.node_base + n;
                if (all || self.active[i])
                    && let Some(t) = query_node(a.luid, Some(process), n as u32)
                {
                    times[i] = t;
                }
            }
        }
    }

    /// 进程在所有适配器上提交的 (专用显存, 共享 GPU 内存) 字节数。
    pub fn process_memory(&self, process: HANDLE) -> (u64, u64) {
        let (mut dedicated, mut shared) = (0, 0);
        for a in &self.adapters {
            for &(id, aperture) in &a.segments {
                let mut q = D3DKMT_QUERYSTATISTICS {
                    Type: D3DKMT_QUERYSTATISTICS_PROCESS_SEGMENT,
                    AdapterLuid: a.luid,
                    hProcess: process,
                    ..Default::default()
                };
                q.Anonymous.QueryProcessSegment.SegmentId = id;
                if unsafe { D3DKMTQueryStatistics(&q) }.is_ok() {
                    let bytes = unsafe { q.QueryResult.ProcessSegmentInformation.BytesCommitted };
                    *if aperture {
                        &mut shared
                    } else {
                        &mut dedicated
                    } += bytes;
                }
            }
        }
        (dedicated, shared)
    }
}

/// 进程在两次采样间的 GPU 利用率：最忙引擎的占用率。
pub fn process_usage(prev: &[i64], now: &[i64], elapsed_100ns: f64) -> f32 {
    if prev.len() != now.len() || elapsed_100ns <= 0.0 {
        return 0.0;
    }
    prev.iter()
        .zip(now)
        .map(|(p, n)| (n - p).max(0) as f64 / elapsed_100ns * 100.0)
        .fold(0.0, f64::max)
        .min(100.0) as f32
}

/// 按引擎类型把节点归类；"其他"类按驱动给的名称（如 Security、OFA_0）分开。
fn engines(luid: LUID, node_count: usize) -> Vec<Engine> {
    let mut out: Vec<Engine> = Vec::new();
    let Some(handle) = open_adapter(luid) else {
        return out;
    };
    for n in 0..node_count {
        let mut md: D3DKMT_NODEMETADATA = unsafe { std::mem::zeroed() };
        md.NodeOrdinalAndAdapterIndex = n as u32;
        let mut q = D3DKMT_QUERYADAPTERINFO {
            hAdapter: handle,
            Type: KMTQAITYPE_NODEMETADATA,
            pPrivateDriverData: (&mut md as *mut D3DKMT_NODEMETADATA).cast(),
            PrivateDriverDataSize: size_of::<D3DKMT_NODEMETADATA>() as u32,
        };
        if unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_err() {
            continue;
        }
        // 结构体是 packed 的，字段要先复制出来再用。
        let data = md.NodeData;
        let (kind, raw_name) = (data.EngineType, data.FriendlyName);
        let len = raw_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(raw_name.len());
        let friendly = String::from_utf16_lossy(&raw_name[..len]);
        let name = match kind {
            DXGK_ENGINE_TYPE_3D
            | DXGK_ENGINE_TYPE_VIDEO_DECODE
            | DXGK_ENGINE_TYPE_VIDEO_ENCODE
            | DXGK_ENGINE_TYPE_VIDEO_PROCESSING
            | DXGK_ENGINE_TYPE_SCENE_ASSEMBLY
            | DXGK_ENGINE_TYPE_COPY
            | DXGK_ENGINE_TYPE_OVERLAY
            | DXGK_ENGINE_TYPE_CRYPTO => EngineName::Kind(kind),
            _ if !friendly.is_empty() => EngineName::Friendly(friendly),
            _ => EngineName::Node(n),
        };
        match out.iter_mut().find(|e| e.name == name) {
            Some(e) => e.nodes.push(n),
            None => out.push(Engine {
                name,
                nodes: vec![n],
            }),
        }
    }
    close_adapter(handle);
    out
}

fn query_segment(
    luid: LUID,
    id: u32,
) -> Option<windows::Wdk::Graphics::Direct3D::D3DKMT_QUERYSTATISTICS_SEGMENT_INFORMATION> {
    let mut q = D3DKMT_QUERYSTATISTICS {
        Type: D3DKMT_QUERYSTATISTICS_SEGMENT,
        AdapterLuid: luid,
        ..Default::default()
    };
    q.Anonymous.QuerySegment.SegmentId = id;
    unsafe { D3DKMTQueryStatistics(&q) }.ok().ok()?;
    Some(unsafe { q.QueryResult.SegmentInformation })
}

fn query_node(luid: LUID, process: Option<HANDLE>, node: u32) -> Option<i64> {
    let mut q = D3DKMT_QUERYSTATISTICS {
        AdapterLuid: luid,
        ..Default::default()
    };
    match process {
        Some(h) => {
            q.Type = D3DKMT_QUERYSTATISTICS_PROCESS_NODE;
            q.hProcess = h;
            q.Anonymous.QueryProcessNode.NodeId = node;
        }
        None => {
            q.Type = D3DKMT_QUERYSTATISTICS_NODE;
            q.Anonymous.QueryNode.NodeId = node;
        }
    }
    unsafe { D3DKMTQueryStatistics(&q) }.ok().ok()?;
    Some(unsafe {
        match process {
            Some(_) => q.QueryResult.ProcessNodeInformation.RunningTime,
            None => q.QueryResult.NodeInformation.GlobalInformation.RunningTime,
        }
    })
}

fn open_adapter(luid: LUID) -> Option<u32> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.ok().ok()?;
    Some(open.hAdapter)
}

fn close_adapter(handle: u32) {
    unsafe {
        let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: handle });
    }
}

fn pci_address(luid: LUID) -> Option<[u32; 3]> {
    let handle = open_adapter(luid)?;
    let mut addr = D3DKMT_ADAPTERADDRESS::default();
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: KMTQAITYPE_ADAPTERADDRESS,
        pPrivateDriverData: (&mut addr as *mut D3DKMT_ADAPTERADDRESS).cast(),
        PrivateDriverDataSize: size_of::<D3DKMT_ADAPTERADDRESS>() as u32,
    };
    let ok = unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_ok();
    close_adapter(handle);
    (ok && addr.BusNumber != u32::MAX).then_some([
        addr.BusNumber,
        addr.DeviceNumber,
        addr.FunctionNumber,
    ])
}
