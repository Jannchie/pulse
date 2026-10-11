//! 性能页：左侧设备列表（带迷你曲线），右侧选中设备的详情。

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, RichText, ScrollArea, Sense, Stroke, Ui,
    pos2, vec2,
};
use pulse_probe::Snapshot;
use pulse_probe::disk::DiskStat;
use pulse_probe::nic::NicStat;

use crate::gpu_panel::{self, GpuHistory, PanelAction};
use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::theme::{Palette, metric, semibold};
use crate::{fmt_bytes, widgets};

/// 近 60 秒的各类曲线，键见 `main.rs` 的采集线程。
pub type Series = HashMap<String, Vec<f32>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    Cpu,
    Memory,
    /// 物理磁盘号。
    Disk(u32),
    /// 网卡接口索引。
    Network(u32),
    /// 第几块 GPU。
    Gpu(usize),
}

pub enum PerfAction {
    /// 回到进程页并搜索这个名字。
    Search(String),
}

const SIDEBAR_W: f32 = 236.0;

pub fn show(
    ui: &mut Ui,
    snap: &Snapshot,
    series: &Series,
    gpu_history: &[GpuHistory],
    device: &mut Device,
    icons: &mut Icons,
) -> Option<PerfAction> {
    let mut action = None;
    let full = ui.available_rect_before_wrap();
    let side = Rect::from_min_size(full.min, vec2(SIDEBAR_W, full.height()));
    let main = Rect::from_min_max(pos2(side.right() + 16.0, full.top()), full.max);

    ui.scope_builder(egui::UiBuilder::new().max_rect(side), |ui| {
        ScrollArea::vertical()
            .id_salt("perf-side")
            .auto_shrink([false, false])
            .show(ui, |ui| sidebar(ui, snap, series, device));
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| {
        ScrollArea::vertical()
            .id_salt(("perf-main", format!("{device:?}")))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 12.0);
                match *device {
                    Device::Cpu => cpu_page(ui, snap, series),
                    Device::Memory => memory_page(ui, snap, series),
                    Device::Disk(i) => match snap.disks.iter().find(|d| d.index == i) {
                        Some(d) => disk_page(ui, d, series),
                        None => missing(ui, "这块磁盘已经不在了"),
                    },
                    Device::Network(i) => match snap.nics.iter().find(|n| n.index == i) {
                        Some(n) => network_page(ui, n, series),
                        None => missing(ui, "这个网络适配器已经不在了"),
                    },
                    Device::Gpu(i) => match snap.gpus.get(i) {
                        Some(g) => {
                            let empty = GpuHistory::default();
                            page_header(ui, Icon::Gpu, metric::GPU, &format!("GPU {i}"), &g.name);
                            let usage =
                                series.get(&format!("gpu.{i}")).cloned().unwrap_or_default();
                            chart(
                                ui,
                                "利用率（最忙引擎）",
                                200.0,
                                &[(&usage, metric::GPU, false)],
                                Some(1.0),
                                &|m| pct(m),
                            );
                            gpu_panel::adapter(ui, g, gpu_history.get(i).unwrap_or(&empty));
                            ui.add_space(8.0);
                            if let Some(PanelAction::Search(name)) =
                                gpu_panel::top_processes(ui, snap, icons)
                            {
                                action = Some(PerfAction::Search(name));
                            }
                        }
                        None => missing(ui, "这块 GPU 已经不在了"),
                    },
                }
                ui.add_space(16.0);
            });
    });
    ui.allocate_rect(full, Sense::hover());
    action
}

fn missing(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.add_space(40.0);
    ui.label(RichText::new(text).color(p.muted));
}

// ---------------------------------------------------------------- 侧栏

struct Entry {
    device: Device,
    icon: Icon,
    color: Color32,
    title: String,
    sub: String,
    /// 迷你曲线：数据与纵轴上限（None 表示按数据自动缩放）。
    spark: Vec<f32>,
    max: Option<f32>,
}

fn sidebar(ui: &mut Ui, snap: &Snapshot, series: &Series, device: &mut Device) {
    let get = |k: &str| series.get(k).cloned().unwrap_or_default();
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let mut entries = vec![
        Entry {
            device: Device::Cpu,
            icon: Icon::Cpu,
            color: metric::CPU,
            title: "CPU".into(),
            sub: match snap.cpu_mhz {
                Some(mhz) => format!("{:.0}%  {:.2} GHz", snap.cpu_total, mhz as f64 / 1000.0),
                None => format!("{:.0}%", snap.cpu_total),
            },
            spark: get("cpu"),
            max: Some(1.0),
        },
        Entry {
            device: Device::Memory,
            icon: Icon::Memory,
            color: metric::MEMORY,
            title: "内存".into(),
            sub: format!(
                "{:.1}/{:.1} GB ({:.0}%)",
                gb(snap.mem_used),
                gb(snap.mem_total),
                snap.mem_used as f64 / snap.mem_total.max(1) as f64 * 100.0
            ),
            spark: get("mem"),
            max: Some(1.0),
        },
    ];
    for d in &snap.disks {
        entries.push(Entry {
            device: Device::Disk(d.index),
            icon: Icon::HardDrive,
            color: metric::DISK,
            title: format!("磁盘 {} ({})", d.index, letters(d)),
            sub: format!("{} · {:.0}%", disk_kind(d), d.active),
            spark: get(&format!("disk.{}.active", d.index)),
            max: Some(1.0),
        });
    }
    for n in &snap.nics {
        entries.push(Entry {
            device: Device::Network(n.index),
            icon: if n.kind == "Wi-Fi" {
                Icon::Wifi
            } else {
                Icon::Network
            },
            color: metric::NETWORK,
            title: n.kind.to_string(),
            sub: if n.connected {
                format!("↓ {}  ↑ {}", fmt_bits(n.rx_rate), fmt_bits(n.tx_rate))
            } else {
                "未连接".into()
            },
            spark: {
                let (rx, tx) = (
                    get(&format!("net.{}.rx", n.index)),
                    get(&format!("net.{}.tx", n.index)),
                );
                rx.iter()
                    .zip(tx.iter().chain(std::iter::repeat(&0.0)))
                    .map(|(a, b)| a + b)
                    .collect()
            },
            max: None,
        });
    }
    for (i, g) in snap.gpus.iter().enumerate() {
        entries.push(Entry {
            device: Device::Gpu(i),
            icon: Icon::Gpu,
            color: metric::GPU,
            title: format!("GPU {i}"),
            sub: format!("{} · {:.0}%", short_gpu(&g.name), g.usage),
            spark: get(&format!("gpu.{i}")),
            max: Some(1.0),
        });
    }

    let p = Palette::of(ui.ctx());
    ui.spacing_mut().item_spacing.y = 4.0;
    for e in entries {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
        let selected = *device == e.device;
        let painter = ui.painter_at(rect.expand(1.0));
        let fill = if selected {
            p.surface
        } else if resp.hovered() {
            p.hover
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(10), fill);
        if selected {
            let bar = Rect::from_min_size(
                rect.left_top() + vec2(0.0, 14.0),
                vec2(3.0, rect.height() - 28.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), e.color);
        }
        // 右侧迷你曲线
        let thumb = Rect::from_min_max(
            pos2(rect.right() - 74.0, rect.top() + 12.0),
            pos2(rect.right() - 10.0, rect.bottom() - 12.0),
        );
        painter.rect_stroke(
            thumb,
            CornerRadius::same(4),
            Stroke::new(1.0, p.track),
            egui::StrokeKind::Inside,
        );
        let max = e
            .max
            .unwrap_or_else(|| e.spark.iter().copied().fold(0.0, f32::max))
            .max(1e-6);
        widgets::area(
            &painter.with_clip_rect(thumb.shrink(1.0)),
            thumb.shrink(1.0),
            &e.spark,
            max,
            e.color,
            false,
        );

        widgets::icon(
            ui,
            e.icon,
            pos2(rect.left() + 20.0, rect.top() + 20.0),
            16.0,
            e.color,
        );
        painter.text(
            pos2(rect.left() + 34.0, rect.top() + 20.0),
            Align2::LEFT_CENTER,
            &e.title,
            FontId::new(13.0, semibold()),
            p.text,
        );
        let sub_rect = Rect::from_min_max(
            pos2(rect.left() + 34.0, rect.top() + 30.0),
            pos2(thumb.left() - 8.0, rect.bottom() - 6.0),
        );
        painter.with_clip_rect(sub_rect).text(
            pos2(sub_rect.left(), rect.top() + 40.0),
            Align2::LEFT_CENTER,
            &e.sub,
            FontId::proportional(11.5),
            p.muted,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *device = e.device;
        }
    }
}

// ---------------------------------------------------------------- 通用部件

fn page_header(ui: &mut Ui, glyph: Icon, color: Color32, title: &str, subtitle: &str) {
    let p = Palette::of(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
        widgets::icon(ui, glyph, r.center(), 22.0, color);
        ui.label(
            RichText::new(title)
                .size(20.0)
                .family(semibold())
                .color(p.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(subtitle).size(13.0).color(p.muted)).truncate());
        });
    });
    ui.add_space(10.0);
}

/// 大图表：卡片底板 + 网格线 + 面积曲线。`max` 为 None 时按数据自动缩放并在右上角标出刻度。
fn chart(
    ui: &mut Ui,
    title: &str,
    height: f32,
    lines: &[(&[f32], Color32, bool)],
    max: Option<f32>,
    unit: &dyn Fn(f32) -> String,
) {
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, CornerRadius::same(12), p.surface);
    let inner = Rect::from_min_max(rect.min + vec2(14.0, 30.0), rect.max - vec2(14.0, 22.0));
    painter.text(
        rect.left_top() + vec2(14.0, 10.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(12.5),
        p.muted,
    );
    let peak = lines
        .iter()
        .flat_map(|(d, _, _)| d.iter().copied())
        .fold(0.0, f32::max);
    let max = max.unwrap_or_else(|| nice_ceiling(peak)).max(1e-6);
    painter.text(
        pos2(inner.right(), rect.top() + 10.0),
        Align2::RIGHT_TOP,
        unit(max),
        FontId::proportional(11.5),
        p.faint,
    );
    painter.text(
        pos2(inner.left(), rect.bottom() - 8.0),
        Align2::LEFT_BOTTOM,
        "60 秒",
        FontId::proportional(11.0),
        p.faint,
    );
    painter.text(
        pos2(inner.right(), rect.bottom() - 8.0),
        Align2::RIGHT_BOTTOM,
        "0",
        FontId::proportional(11.0),
        p.faint,
    );
    for k in 1..4 {
        let y = inner.top() + inner.height() * k as f32 / 4.0;
        painter.hline(
            inner.x_range(),
            y,
            Stroke::new(1.0, p.track.gamma_multiply(0.6)),
        );
    }
    painter.hline(inner.x_range(), inner.bottom(), Stroke::new(1.0, p.track));
    let mirrored = lines.iter().any(|l| l.2);
    if mirrored {
        // 上下镜像：上半区正向，下半区反向，共用刻度。
        let mid = inner.center().y;
        let upper = Rect::from_min_max(inner.min, pos2(inner.right(), mid));
        let lower = Rect::from_min_max(pos2(inner.left(), mid), inner.max);
        painter.hline(inner.x_range(), mid, Stroke::new(1.0, p.track));
        for (d, c, down) in lines {
            widgets::area(
                &painter,
                if *down { lower } else { upper },
                d,
                max,
                *c,
                *down,
            );
        }
    } else {
        for (d, c, _) in lines {
            widgets::area(&painter, inner, d, max, *c, false);
        }
    }
}

/// 自动刻度：取不小于峰值的 1/2/5 × 10^n。
fn nice_ceiling(v: f32) -> f32 {
    if v <= 0.0 {
        return 1.0;
    }
    let mag = 10f32.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|&c| c >= v)
        .unwrap_or(10.0 * mag)
}

/// 两列（宽度允许时三列）的指标网格。
fn stats(ui: &mut Ui, items: &[(&str, String)]) {
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let cols = if w > 760.0 {
        4
    } else if w > 520.0 {
        3
    } else {
        2
    };
    let cell_w = (w - (cols - 1) as f32 * 10.0) / cols as f32;
    for row in items.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            for (label, value) in row {
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, 58.0), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(10), p.surface);
                painter.text(
                    rect.left_top() + vec2(12.0, 10.0),
                    Align2::LEFT_TOP,
                    *label,
                    FontId::proportional(11.5),
                    p.muted,
                );
                let clip = painter.with_clip_rect(rect.shrink(4.0));
                clip.text(
                    rect.left_bottom() + vec2(12.0, -10.0),
                    Align2::LEFT_BOTTOM,
                    value,
                    FontId::new(15.0, semibold()),
                    p.text,
                );
            }
        });
        ui.add_space(4.0);
    }
}

fn section(ui: &mut Ui, title: &str) {
    let p = Palette::of(ui.ctx());
    ui.add_space(12.0);
    ui.label(
        RichText::new(title)
            .size(13.0)
            .family(semibold())
            .color(p.text),
    );
    ui.add_space(6.0);
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn fmt_bits(bytes_per_sec: u64) -> String {
    let bits = bytes_per_sec as f64 * 8.0;
    if bits < 1e3 {
        format!("{bits:.0} bps")
    } else if bits < 1e6 {
        format!("{:.0} Kbps", bits / 1e3)
    } else if bits < 1e9 {
        format!("{:.1} Mbps", bits / 1e6)
    } else {
        format!("{:.2} Gbps", bits / 1e9)
    }
}

fn fmt_duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    format!(
        "{}:{:02}:{:02}:{:02}",
        s / 86_400,
        s / 3600 % 24,
        s / 60 % 60,
        s % 60
    )
}

fn short_gpu(name: &str) -> &str {
    ["NVIDIA GeForce ", "NVIDIA ", "AMD Radeon ", "Intel(R) "]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

fn letters(d: &DiskStat) -> String {
    if d.volumes.is_empty() {
        "无分区".into()
    } else {
        d.volumes
            .iter()
            .map(|v| format!("{}:", v.letter))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn disk_kind(d: &DiskStat) -> String {
    let media = match d.ssd {
        Some(true) => "SSD",
        Some(false) => "HDD",
        None => "磁盘",
    };
    format!("{media} ({})", d.bus)
}

// ---------------------------------------------------------------- CPU

fn cpu_page(ui: &mut Ui, snap: &Snapshot, series: &Series) {
    let p = Palette::of(ui.ctx());
    let info = &snap.cpu_info;
    page_header(ui, Icon::Cpu, metric::CPU, "CPU", &info.name);
    let empty = Vec::new();
    let total = series.get("cpu").unwrap_or(&empty);
    chart(
        ui,
        "总利用率",
        200.0,
        &[(total, metric::CPU, false)],
        Some(1.0),
        &|m| pct(m),
    );

    section(ui, "逻辑处理器");
    let n = snap.cpu_cores.len().max(1);
    let w = ui.available_width();
    let cols = (w / 110.0).floor().clamp(4.0, 12.0) as usize;
    let cell_w = (w - (cols - 1) as f32 * 6.0) / cols as f32;
    for row in (0..n).collect::<Vec<_>>().chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for &i in row {
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, 52.0), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(8), p.surface);
                let data = series.get(&format!("cpu.{i}")).unwrap_or(&empty);
                let chart =
                    Rect::from_min_max(rect.min + vec2(6.0, 18.0), rect.max - vec2(6.0, 4.0));
                widgets::area(&painter, chart, data, 1.0, metric::CPU, false);
                painter.text(
                    rect.left_top() + vec2(8.0, 5.0),
                    Align2::LEFT_TOP,
                    format!("{i}"),
                    FontId::proportional(11.0),
                    p.faint,
                );
                let u = snap.cpu_cores.get(i).copied().unwrap_or(0.0);
                painter.text(
                    rect.right_top() + vec2(-8.0, 5.0),
                    Align2::RIGHT_TOP,
                    format!("{u:.0}%"),
                    FontId::proportional(11.0),
                    p.muted,
                );
            }
        });
        ui.add_space(2.0);
    }

    section(ui, "规格与状态");
    let threads: u32 = snap.processes.iter().map(|p| p.threads).sum();
    let handles: u32 = snap.processes.iter().map(|p| p.handles).sum();
    let ghz = |mhz: u32| format!("{:.2} GHz", mhz as f64 / 1000.0);
    stats(
        ui,
        &[
            ("利用率", format!("{:.0}%", snap.cpu_total)),
            ("当前频率（驱动报告）", snap.cpu_mhz.map_or("—".into(), ghz)),
            (
                "基准频率",
                if info.base_mhz > 0 {
                    ghz(info.base_mhz)
                } else {
                    "—".into()
                },
            ),
            ("正常运行时间", fmt_duration(snap.uptime)),
            ("插槽", info.sockets.to_string()),
            ("内核", info.cores.to_string()),
            ("逻辑处理器", info.logical.to_string()),
            ("进程", snap.processes.len().to_string()),
            ("L1 缓存", fmt_bytes(info.l1)),
            ("L2 缓存", fmt_bytes(info.l2)),
            ("L3 缓存", fmt_bytes(info.l3)),
            ("线程 / 句柄", format!("{threads} / {handles}")),
        ],
    );
}

// ---------------------------------------------------------------- 内存

fn memory_page(ui: &mut Ui, snap: &Snapshot, series: &Series) {
    let p = Palette::of(ui.ctx());
    let m = &snap.memory;
    let modules = &snap.memory_modules;
    let subtitle = match modules.first() {
        Some(first) if first.speed > 0 => format!(
            "{:.0} GB {} · {} MT/s",
            m.total as f64 / (1u64 << 30) as f64,
            first.kind,
            first.speed
        ),
        _ => format!("{:.0} GB", m.total as f64 / (1u64 << 30) as f64),
    };
    page_header(ui, Icon::Memory, metric::MEMORY, "内存", &subtitle);
    let empty = Vec::new();
    chart(
        ui,
        "内存使用量",
        200.0,
        &[(series.get("mem").unwrap_or(&empty), metric::MEMORY, false)],
        Some(1.0),
        &|m| pct(m),
    );

    section(ui, "内存组合");
    // 使用中 / 已修改 / 备用 / 可用，与任务管理器一致。
    let (modified, standby, free) = m
        .lists
        .as_ref()
        .map_or((0, 0, m.available), |l| (l.modified, l.standby, l.free));
    let parts = [
        ("使用中", m.in_use(), metric::MEMORY),
        ("已修改", modified, Color32::from_rgb(0xf5, 0x9e, 0x0b)),
        ("备用", standby, metric::MEMORY.gamma_multiply(0.45)),
        ("空闲", free, p.track),
    ];
    let w = ui.available_width();
    let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let painter = ui.painter_at(bar);
    painter.rect_filled(bar, CornerRadius::same(8), p.track);
    let mut x = bar.left();
    for (_, bytes, color) in &parts {
        let seg = bar.width() * *bytes as f32 / m.total.max(1) as f32;
        if seg > 0.5 {
            let r = Rect::from_min_max(
                pos2(x, bar.top()),
                pos2((x + seg).min(bar.right()), bar.bottom()),
            );
            painter.rect_filled(r, CornerRadius::ZERO, *color);
        }
        x += seg;
    }
    painter.rect_stroke(
        bar,
        CornerRadius::same(8),
        Stroke::new(2.0, p.bg),
        egui::StrokeKind::Outside,
    );
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (label, bytes, color) in &parts {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(3), *color);
            ui.label(
                RichText::new(format!("{label} {}", fmt_bytes(*bytes)))
                    .size(12.0)
                    .color(p.muted),
            );
            ui.add_space(10.0);
        }
    });
    if m.lists.is_none() {
        ui.label(
            RichText::new("已修改与备用内存需要管理员权限才能读取")
                .size(11.5)
                .color(p.faint),
        );
    }

    section(ui, "详细信息");
    stats(
        ui,
        &[
            ("使用中", fmt_bytes(m.in_use())),
            ("可用", fmt_bytes(m.available)),
            (
                "已提交",
                format!(
                    "{} / {}",
                    fmt_bytes(m.commit_used),
                    fmt_bytes(m.commit_limit)
                ),
            ),
            ("提交峰值", fmt_bytes(m.commit_peak)),
            ("已缓存", fmt_bytes(m.cache)),
            ("分页缓冲池", fmt_bytes(m.paged_pool)),
            ("非分页缓冲池", fmt_bytes(m.nonpaged_pool)),
            (
                "已用插槽",
                if modules.is_empty() {
                    "—".into()
                } else {
                    modules.len().to_string()
                },
            ),
        ],
    );

    if !modules.is_empty() {
        section(ui, "内存条");
        for mm in modules.iter() {
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::same(10), p.surface);
            let cy = rect.center().y;
            painter.text(
                pos2(rect.left() + 14.0, cy - 8.0),
                Align2::LEFT_CENTER,
                fmt_bytes(mm.size),
                FontId::new(14.0, semibold()),
                p.text,
            );
            painter.text(
                pos2(rect.left() + 14.0, cy + 10.0),
                Align2::LEFT_CENTER,
                &mm.locator,
                FontId::proportional(11.5),
                p.faint,
            );
            let spec = [
                mm.kind.to_string(),
                if mm.speed > 0 {
                    format!("{} MT/s", mm.speed)
                } else {
                    String::new()
                },
                mm.form_factor.to_string(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            painter.text(
                pos2(rect.left() + 150.0, cy - 8.0),
                Align2::LEFT_CENTER,
                spec,
                FontId::proportional(12.5),
                p.text,
            );
            painter.text(
                pos2(rect.left() + 150.0, cy + 10.0),
                Align2::LEFT_CENTER,
                format!("{} {}", mm.manufacturer, mm.part_number).trim(),
                FontId::proportional(11.5),
                p.muted,
            );
            ui.add_space(4.0);
        }
    }
}

// ---------------------------------------------------------------- 磁盘

fn disk_page(ui: &mut Ui, d: &DiskStat, series: &Series) {
    let p = Palette::of(ui.ctx());
    page_header(
        ui,
        Icon::HardDrive,
        metric::DISK,
        &format!("磁盘 {} ({})", d.index, letters(d)),
        &d.model,
    );
    let empty = Vec::new();
    let get = |k: &str| {
        series
            .get(&format!("disk.{}.{k}", d.index))
            .unwrap_or(&empty)
    };
    chart(
        ui,
        "活动时间",
        150.0,
        &[(get("active"), metric::DISK, false)],
        Some(1.0),
        &|m| pct(m),
    );
    ui.add_space(8.0);
    chart(
        ui,
        "传输速率（上：读取 · 下：写入）",
        150.0,
        &[
            (get("read"), metric::DISK, false),
            (get("write"), metric::NETWORK_UP, true),
        ],
        None,
        &|m| format!("{}/s", fmt_bytes(m as u64)),
    );

    section(ui, "性能");
    stats(
        ui,
        &[
            ("活动时间", format!("{:.0}%", d.active)),
            ("平均响应时间", format!("{:.2} ms", d.response_ms)),
            ("读取速度", format!("{}/s", fmt_bytes(d.read_rate))),
            ("写入速度", format!("{}/s", fmt_bytes(d.write_rate))),
            ("容量", fmt_bytes(d.size)),
            ("类型", disk_kind(d)),
            (
                "可移动",
                if d.removable {
                    "是".into()
                } else {
                    "否".into()
                },
            ),
            ("分区", d.volumes.len().to_string()),
        ],
    );

    health_section(ui, d);

    if !d.volumes.is_empty() {
        section(ui, "分区");
        for v in &d.volumes {
            let used = v.total.saturating_sub(v.free);
            let frac = used as f32 / v.total.max(1) as f32;
            // 剩余不足 10% 时用警示色。
            let full = frac >= 0.9;
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::same(10), p.surface);
            let title = if v.label.is_empty() {
                format!("{}:", v.letter)
            } else {
                format!("{}:  {}", v.letter, v.label)
            };
            painter.text(
                rect.left_top() + vec2(14.0, 10.0),
                Align2::LEFT_TOP,
                title,
                FontId::new(13.0, semibold()),
                p.text,
            );
            painter.text(
                rect.right_top() + vec2(-14.0, 10.0),
                Align2::RIGHT_TOP,
                format!(
                    "{} 可用，共 {} · {}",
                    fmt_bytes(v.free),
                    fmt_bytes(v.total),
                    v.file_system
                ),
                FontId::proportional(12.0),
                if full { p.danger } else { p.muted },
            );
            let bar = Rect::from_min_max(
                pos2(rect.left() + 14.0, rect.bottom() - 18.0),
                pos2(rect.right() - 14.0, rect.bottom() - 11.0),
            );
            widgets::meter(
                &painter,
                bar,
                frac,
                if full { p.danger } else { metric::DISK },
                p.track,
            );
            ui.add_space(4.0);
        }
    }
}

fn health_section(ui: &mut Ui, d: &DiskStat) {
    let p = Palette::of(ui.ctx());
    let h = &d.health;
    section(ui, "健康状态");
    let mut items: Vec<(&str, String)> = Vec::new();
    let (status, ok) = match &h.nvme {
        Some(n) if n.critical_warning != 0 => ("警告：控制器报告了严重问题".to_string(), false),
        Some(n) if n.available_spare <= n.spare_threshold => {
            ("警告：可用备用空间低于阈值".to_string(), false)
        }
        Some(n) if n.percentage_used >= 90 => ("注意：寿命接近耗尽".to_string(), false),
        Some(n) if n.media_errors > 0 => ("注意：存在介质错误".to_string(), false),
        Some(_) => ("良好".to_string(), true),
        None => ("未提供 SMART 数据".to_string(), true),
    };
    let temp_hot = matches!((h.temperature, h.warning_temperature), (Some(t), Some(w)) if t >= w);
    // 状态条
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(10), p.surface);
    let good = ok && !temp_hot;
    let (glyph, color) = if good {
        (Icon::CheckCircle, metric::GPU)
    } else {
        (Icon::AlertTriangle, p.danger)
    };
    widgets::icon(
        ui,
        glyph,
        pos2(rect.left() + 22.0, rect.center().y),
        18.0,
        color,
    );
    let text = if temp_hot && ok {
        "注意：温度超过警告阈值".to_string()
    } else {
        status
    };
    painter.text(
        pos2(rect.left() + 40.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::new(13.5, semibold()),
        if good { p.text } else { p.danger },
    );
    ui.add_space(6.0);

    if let Some(t) = h.temperature {
        let thresholds = match (h.warning_temperature, h.critical_temperature) {
            (Some(w), Some(c)) => format!("（警告 {w}°C · 临界 {c}°C）"),
            (Some(w), None) => format!("（警告 {w}°C）"),
            _ => String::new(),
        };
        items.push(("温度", format!("{t}°C {thresholds}")));
    }
    if let Some(n) = &h.nvme {
        items.push((
            "剩余寿命",
            format!("{}%", 100u8.saturating_sub(n.percentage_used.min(100))),
        ));
        items.push((
            "可用备用空间",
            format!("{}%（阈值 {}%）", n.available_spare, n.spare_threshold),
        ));
        items.push((
            "通电时间",
            format!(
                "{} 小时（{:.1} 年）",
                n.power_on_hours,
                n.power_on_hours as f64 / 8766.0
            ),
        ));
        items.push(("通电次数", n.power_cycles.to_string()));
        items.push(("累计写入", fmt_tb(n.data_written)));
        items.push(("累计读取", fmt_tb(n.data_read)));
        items.push(("异常关机", n.unsafe_shutdowns.to_string()));
        items.push(("介质错误", n.media_errors.to_string()));
    }
    if items.is_empty() {
        ui.label(
            RichText::new("这块磁盘没有通过系统接口报告温度或 SMART 数据（SATA 盘的完整 SMART 需要 ATA 直通，暂不支持）")
                .size(12.0)
                .color(p.faint),
        );
    } else {
        stats(ui, &items);
    }
}

fn fmt_tb(bytes: u64) -> String {
    let tb = bytes as f64 / 1e12;
    if tb >= 1.0 {
        format!("{tb:.1} TB")
    } else {
        format!("{:.0} GB", bytes as f64 / 1e9)
    }
}

// ---------------------------------------------------------------- 网络

fn network_page(ui: &mut Ui, n: &NicStat, series: &Series) {
    page_header(
        ui,
        if n.kind == "Wi-Fi" {
            Icon::Wifi
        } else {
            Icon::Network
        },
        metric::NETWORK,
        &n.name,
        &n.description,
    );
    let empty = Vec::new();
    let get = |k: &str| {
        series
            .get(&format!("net.{}.{k}", n.index))
            .unwrap_or(&empty)
    };
    chart(
        ui,
        "吞吐量（上：接收 · 下：发送）",
        220.0,
        &[
            (get("rx"), metric::NETWORK, false),
            (get("tx"), metric::NETWORK_UP, true),
        ],
        None,
        &|m| fmt_bits(m as u64),
    );

    section(ui, "连接");
    let mut items = vec![
        ("接收", fmt_bits(n.rx_rate)),
        ("发送", fmt_bits(n.tx_rate)),
        (
            "状态",
            if n.connected {
                "已连接".into()
            } else {
                "未连接".into()
            },
        ),
        (
            "链路速度",
            if n.link_speed > 0 {
                fmt_bits(n.link_speed / 8)
            } else {
                "—".into()
            },
        ),
        ("类型", n.kind.to_string()),
        (
            "MAC 地址",
            if n.mac.is_empty() {
                "—".into()
            } else {
                n.mac.clone()
            },
        ),
        ("开机以来接收", fmt_bytes(n.rx_total)),
        ("开机以来发送", fmt_bytes(n.tx_total)),
    ];
    if let Some(v4) = n.ipv4.first() {
        items.insert(4, ("IPv4", v4.clone()));
    }
    stats(ui, &items);
    if !n.ipv6.is_empty() {
        section(ui, "IPv6 地址");
        let p = Palette::of(ui.ctx());
        for a in &n.ipv6 {
            ui.label(RichText::new(a).monospace().size(12.0).color(p.muted));
        }
    }
}
