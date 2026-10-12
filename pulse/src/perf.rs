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
    /// 复制设备摘要到剪贴板。
    Copy(String),
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
            .show(ui, |ui| {
                sidebar(ui, snap, series, device);
                ui.add_space(10.0);
                if widgets::tool_button(ui, Icon::Clipboard, "复制摘要", true, false)
                    .on_hover_text("把当前设备的信息复制为文本")
                    .clicked()
                {
                    action = Some(PerfAction::Copy(summary(*device, snap)));
                }
            });
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
                            let h = gpu_history.get(i).unwrap_or(&empty);
                            if let Some(PanelAction::Search(name)) =
                                gpu_panel::page(ui, g, h, snap, icons)
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
            sub: format!("{} · {:.0}%", widgets::short_gpu_name(&g.name), g.usage),
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
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let (tile, _) = ui.allocate_exact_size(vec2(42.0, 42.0), Sense::hover());
        ui.painter()
            .rect_filled(tile, CornerRadius::same(10), color.gamma_multiply(0.16));
        widgets::icon(ui, glyph, tile.center(), 22.0, color);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                RichText::new(title)
                    .size(20.0)
                    .family(semibold())
                    .color(p.text),
            );
            ui.add(egui::Label::new(RichText::new(subtitle).size(12.5).color(p.muted)).truncate());
        });
    });
    ui.add_space(14.0);
}

/// 关键指标卡片：标签、大号数值（附单位）、说明，以及可选的占比条。
pub struct Kpi {
    label: &'static str,
    value: String,
    unit: String,
    color: Color32,
    caption: Option<String>,
    bar: Option<f32>,
}

impl Kpi {
    pub fn new(
        label: &'static str,
        value: impl Into<String>,
        unit: impl Into<String>,
        color: Color32,
    ) -> Self {
        Self {
            label,
            value: value.into(),
            unit: unit.into(),
            color,
            caption: None,
            bar: None,
        }
    }

    pub fn caption(mut self, text: impl Into<String>) -> Self {
        self.caption = Some(text.into());
        self
    }

    pub fn bar(mut self, frac: f32) -> Self {
        self.bar = Some(frac.clamp(0.0, 1.0));
        self
    }
}

/// 页面顶部的一排关键指标；窄窗口时两列换行。
pub fn hero(ui: &mut Ui, items: &[Kpi]) {
    let p = Palette::of(ui.ctx());
    let gap = 10.0;
    let w = ui.available_width();
    let cols = if w > 640.0 { items.len() } else { 2 }.max(1);
    let cell_w = (w - gap * (cols - 1) as f32) / cols as f32;
    let height = if items.iter().any(|k| k.bar.is_some()) {
        104.0
    } else {
        92.0
    };
    for chunk in items.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for k in chunk {
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, height), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(12), p.surface);
                painter.circle_filled(pos2(rect.left() + 17.0, rect.top() + 19.0), 3.5, k.color);
                painter.text(
                    pos2(rect.left() + 27.0, rect.top() + 19.0),
                    Align2::LEFT_CENTER,
                    k.label,
                    FontId::proportional(12.0),
                    p.muted,
                );
                let value = painter.text(
                    pos2(rect.left() + 14.0, rect.top() + 30.0),
                    Align2::LEFT_TOP,
                    &k.value,
                    FontId::new(26.0, semibold()),
                    p.text,
                );
                if !k.unit.is_empty() {
                    painter.text(
                        pos2(value.right() + 5.0, value.bottom() - 6.0),
                        Align2::LEFT_BOTTOM,
                        &k.unit,
                        FontId::proportional(12.5),
                        p.muted,
                    );
                }
                let mut bottom = rect.bottom() - 12.0;
                if let Some(frac) = k.bar {
                    let bar = Rect::from_min_max(
                        pos2(rect.left() + 14.0, rect.bottom() - 14.0),
                        pos2(rect.right() - 14.0, rect.bottom() - 10.0),
                    );
                    widgets::meter(&painter, bar, frac, k.color, p.track);
                    bottom = bar.top() - 7.0;
                }
                if let Some(caption) = &k.caption {
                    painter.with_clip_rect(rect.shrink(8.0)).text(
                        pos2(rect.left() + 14.0, bottom),
                        Align2::LEFT_BOTTOM,
                        caption,
                        FontId::proportional(11.5),
                        p.faint,
                    );
                }
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// 宽度足够时左右两栏，否则上下排列。
pub fn two_columns(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    if ui.available_width() < 720.0 {
        left(ui);
        ui.add_space(8.0);
        right(ui);
        return;
    }
    ui.spacing_mut().item_spacing.x = 12.0;
    ui.columns(2, |cols| {
        left(&mut cols[0]);
        right(&mut cols[1]);
    });
}

/// 属性列表：一张卡片里逐行"名称 … 值"，宽度足够时分两栏（按列从上往下排）。
fn kv_list(ui: &mut Ui, items: &[(&str, String)]) {
    const ROW: f32 = 34.0;
    if items.is_empty() {
        return;
    }
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let cols = if w > 640.0 && items.len() > 3 { 2 } else { 1 };
    let rows = items.len().div_ceil(cols);
    let (card, _) = ui.allocate_exact_size(vec2(w, rows as f32 * ROW), Sense::hover());
    let painter = ui.painter_at(card);
    painter.rect_filled(card, CornerRadius::same(12), p.surface);
    let col_w = w / cols as f32;
    for (i, (label, value)) in items.iter().enumerate() {
        let (c, r) = (i / rows, i % rows);
        let cell = Rect::from_min_size(
            pos2(card.left() + c as f32 * col_w, card.top() + r as f32 * ROW),
            vec2(col_w, ROW),
        );
        if r > 0 {
            painter.hline(
                (cell.left() + 14.0)..=(cell.right() - 14.0),
                cell.top(),
                Stroke::new(1.0, p.track),
            );
        }
        let label = painter.text(
            pos2(cell.left() + 14.0, cell.center().y),
            Align2::LEFT_CENTER,
            *label,
            FontId::proportional(12.5),
            p.muted,
        );
        let clip = Rect::from_min_max(pos2(label.right() + 12.0, cell.top()), cell.max);
        painter.with_clip_rect(clip).text(
            pos2(cell.right() - 14.0, cell.center().y),
            Align2::RIGHT_CENTER,
            value,
            FontId::proportional(13.0),
            p.text,
        );
    }
    if cols == 2 {
        painter.vline(
            card.left() + col_w,
            (card.top() + 8.0)..=(card.bottom() - 8.0),
            Stroke::new(1.0, p.track),
        );
    }
}

/// 大图表：卡片底板 + 网格线 + 面积曲线。`max` 为 None 时按数据自动缩放并在右上角标出刻度。
pub fn chart(
    ui: &mut Ui,
    title: &str,
    height: f32,
    lines: &[(&[f32], Color32, bool)],
    max: Option<f32>,
    unit: &dyn Fn(f32) -> String,
    legend: &[(&str, Color32)],
) {
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, height), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, CornerRadius::same(12), p.surface);
    let inner = Rect::from_min_max(rect.min + vec2(14.0, 30.0), rect.max - vec2(14.0, 22.0));
    let title = painter.text(
        rect.left_top() + vec2(14.0, 10.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(12.5),
        p.muted,
    );
    // 图例：标题右侧的色点 + 名称（上下镜像图靠它说明哪一半是什么）。
    let mut x = title.right() + 14.0;
    for (name, color) in legend {
        painter.circle_filled(pos2(x + 3.5, title.center().y), 3.5, *color);
        let r = painter.text(
            pos2(x + 11.0, title.center().y),
            Align2::LEFT_CENTER,
            *name,
            FontId::proportional(12.0),
            p.muted,
        );
        x = r.right() + 12.0;
    }
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
    // 上下镜像时下半区的底边同样是上限，两端刻度一起变化。
    let mirrored = lines.iter().any(|l| l.2);
    painter.text(
        pos2(inner.right(), rect.bottom() - 8.0),
        Align2::RIGHT_BOTTOM,
        if mirrored { unit(max) } else { "0".into() },
        FontId::proportional(11.0),
        p.faint,
    );
    if mirrored {
        // 上下镜像：上半区正向，下半区反向，共用刻度；中间留出间隔，两组数据不粘在一起。
        // 两半之间的间隔与点阵内部的点距一致，看起来仍是一张网格。
        let gap = widgets::dot_gap(
            painter.pixels_per_point(),
            inner.width(),
            (inner.height() - 3.0) / 2.0,
        );
        let mid = inner.center().y;
        let upper = Rect::from_min_max(inner.min, pos2(inner.right(), mid - gap / 2.0));
        let lower = Rect::from_min_max(pos2(inner.left(), mid + gap / 2.0), inner.max);
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

pub fn section(ui: &mut Ui, title: &str) {
    let p = Palette::of(ui.ctx());
    ui.add_space(16.0);
    ui.label(
        RichText::new(title)
            .size(13.0)
            .family(semibold())
            .color(p.text),
    );
    ui.add_space(6.0);
}

pub fn pct(v: f32) -> String {
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
    let info = &snap.cpu_info;
    page_header(ui, Icon::Cpu, metric::CPU, "CPU", &info.name);
    let threads: u32 = snap.processes.iter().map(|p| p.threads).sum();
    let handles: u32 = snap.processes.iter().map(|p| p.handles).sum();
    let busiest = snap.cpu_cores.iter().copied().fold(0.0, f32::max);
    let up = snap.uptime.as_secs();
    hero(
        ui,
        &[
            Kpi::new("利用率", format!("{:.0}", snap.cpu_total), "%", metric::CPU)
                .caption(format!("最忙的逻辑处理器 {busiest:.0}%"))
                .bar(snap.cpu_total / 100.0),
            Kpi::new(
                "速度",
                snap.cpu_mhz
                    .map_or("—".into(), |m| format!("{:.2}", m as f64 / 1000.0)),
                "GHz",
                metric::CPU,
            )
            .caption(if info.base_mhz > 0 {
                format!("基准 {:.2} GHz", info.base_mhz as f64 / 1000.0)
            } else {
                "驱动报告的当前频率".into()
            }),
            Kpi::new("进程", snap.processes.len().to_string(), "个", metric::CPU)
                .caption(format!("{threads} 线程 · {handles} 句柄")),
            Kpi::new(
                "正常运行时间",
                format!("{}:{:02}", up / 3600, up / 60 % 60),
                "小时",
                metric::CPU,
            )
            .caption(fmt_duration(snap.uptime)),
        ],
    );
    ui.add_space(10.0);
    let empty = Vec::new();
    let total = series.get("cpu").unwrap_or(&empty);
    chart(
        ui,
        "总利用率",
        170.0,
        &[(total, metric::CPU, false)],
        Some(1.0),
        &pct,
        &[],
    );

    section(ui, "逻辑处理器");
    let n = snap.cpu_cores.len();
    if info.hybrid() {
        // 大小核分两组；各组标出物理核心数与线程数。
        let (e, p_idx): (Vec<usize>, Vec<usize>) = (0..n).partition(|&i| info.is_efficient(i));
        core_label(
            ui,
            metric::CPU,
            &format!("性能核 · {} 核 {} 线程", info.p_cores, p_idx.len()),
        );
        core_grid(ui, snap, series, &p_idx, metric::CPU);
        ui.add_space(8.0);
        core_label(
            ui,
            metric::CPU_EFFICIENT,
            &format!("能效核 · {} 核 {} 线程", info.e_cores, e.len()),
        );
        core_grid(ui, snap, series, &e, metric::CPU_EFFICIENT);
    } else {
        core_grid(ui, snap, series, &(0..n).collect::<Vec<_>>(), metric::CPU);
    }

    section(ui, "规格");
    let ghz = |mhz: u32| format!("{:.2} GHz", mhz as f64 / 1000.0);
    kv_list(
        ui,
        &[
            (
                "基准速度",
                if info.base_mhz > 0 {
                    ghz(info.base_mhz)
                } else {
                    "—".into()
                },
            ),
            ("插槽", info.sockets.to_string()),
            (
                "内核",
                if info.hybrid() {
                    format!(
                        "{}（{} 性能核 + {} 能效核）",
                        info.cores, info.p_cores, info.e_cores
                    )
                } else {
                    info.cores.to_string()
                },
            ),
            ("逻辑处理器", info.logical.to_string()),
            ("L1 缓存", fmt_bytes(info.l1)),
            ("L2 缓存", fmt_bytes(info.l2)),
            ("L3 缓存", fmt_bytes(info.l3)),
            ("当前频率（驱动报告）", snap.cpu_mhz.map_or("—".into(), ghz)),
        ],
    );
}

/// 大小核分组的小标题：色点 + 文字。
fn core_label(ui: &mut Ui, color: Color32, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 3.5, color);
        ui.label(RichText::new(text).size(12.0).color(p.muted));
    });
    ui.add_space(4.0);
}

/// 逻辑处理器小格：编号、当前占用率、近 60 秒点阵。
fn core_grid(ui: &mut Ui, snap: &Snapshot, series: &Series, indices: &[usize], color: Color32) {
    let p = Palette::of(ui.ctx());
    let empty = Vec::new();
    let w = ui.available_width();
    let cols = (w / 110.0).floor().clamp(4.0, 12.0) as usize;
    let cell_w = (w - (cols - 1) as f32 * 6.0) / cols as f32;
    for row in indices.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for &i in row {
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, 52.0), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(8), p.surface);
                let data = series.get(&format!("cpu.{i}")).unwrap_or(&empty);
                let chart =
                    Rect::from_min_max(rect.min + vec2(8.0, 20.0), rect.max - vec2(8.0, 6.0));
                widgets::area(&painter, chart, data, 1.0, color, false);
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
    let gb = |b: u64| format!("{:.1}", b as f64 / (1u64 << 30) as f64);
    let frac = |a: u64, b: u64| a as f32 / b.max(1) as f32;
    hero(
        ui,
        &[
            Kpi::new(
                "使用中",
                gb(m.in_use()),
                format!("/ {} GB", gb(m.total)),
                metric::MEMORY,
            )
            .caption(format!("{:.0}%", frac(m.in_use(), m.total) * 100.0))
            .bar(frac(m.in_use(), m.total)),
            Kpi::new("可用", gb(m.available), "GB", metric::MEMORY).caption("可立即分配给程序"),
            Kpi::new(
                "已提交",
                gb(m.commit_used),
                format!("/ {} GB", gb(m.commit_limit)),
                metric::MEMORY,
            )
            .caption("物理内存 + 页面文件")
            .bar(frac(m.commit_used, m.commit_limit)),
            Kpi::new("已缓存", gb(m.cache), "GB", metric::MEMORY).caption("备用与已修改的页面"),
        ],
    );
    ui.add_space(10.0);
    let empty = Vec::new();
    chart(
        ui,
        "内存使用量",
        170.0,
        &[(series.get("mem").unwrap_or(&empty), metric::MEMORY, false)],
        Some(1.0),
        &pct,
        &[],
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
    // 点阵：4 行，每个点代表相同份额的内存，按类别依次（逐列从上到下）上色。
    const ROWS: usize = 4;
    // 点的边长取整到物理像素（每个点一样大）；点距微调到正好铺满整行，每个点的位置吸附到像素。
    let ppp = ui.ctx().pixels_per_point();
    let dot = (7.0 * ppp).round().max(1.0) / ppp;
    let w = ui.available_width();
    let cols = ((w + 10.0 - dot) / 10.0).floor().max(2.0) as usize;
    let pitch = (w - dot) / (cols - 1) as f32;
    let total_dots = cols * ROWS;
    let (grid, _) =
        ui.allocate_exact_size(vec2(w, ROWS as f32 * pitch - (pitch - dot)), Sense::hover());
    let painter = ui.painter_at(grid);
    let mut colors = Vec::with_capacity(total_dots);
    let mut acc = 0u64;
    for (_, bytes, color) in &parts {
        acc += *bytes;
        let until = ((acc as f64 / m.total.max(1) as f64) * total_dots as f64).round() as usize;
        while colors.len() < until.min(total_dots) {
            colors.push(*color);
        }
    }
    colors.resize(total_dots, p.track);
    for (k, color) in colors.into_iter().enumerate() {
        let (c, r) = (k / ROWS, k % ROWS);
        let dot = Rect::from_min_size(
            pos2(
                ((grid.left() + c as f32 * pitch) * ppp).round() / ppp,
                ((grid.top() + r as f32 * pitch) * ppp).round() / ppp,
            ),
            vec2(dot, dot),
        );
        painter.rect_filled(dot, CornerRadius::same(2), color);
    }
    ui.add_space(10.0);
    // 图例：色块、名称、数值与占比。
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (label, bytes, color) in &parts {
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), *color);
            ui.label(RichText::new(*label).size(12.0).color(p.muted));
            ui.label(
                RichText::new(fmt_bytes(*bytes))
                    .size(12.5)
                    .family(semibold())
                    .color(p.text),
            );
            ui.label(
                RichText::new(format!(
                    "{:.0}%",
                    *bytes as f64 / m.total.max(1) as f64 * 100.0
                ))
                .size(12.0)
                .color(p.faint),
            );
            ui.add_space(14.0);
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
    kv_list(
        ui,
        &[
            ("提交峰值", fmt_bytes(m.commit_peak)),
            ("分页缓冲池", fmt_bytes(m.paged_pool)),
            ("非分页缓冲池", fmt_bytes(m.nonpaged_pool)),
            (
                "已用插槽",
                match (modules.len(), snap.memory_slots) {
                    (0, _) => "—".into(),
                    (used, 0) => used.to_string(),
                    (used, slots) => format!("{used} / {slots}"),
                },
            ),
            (
                "为硬件保留",
                snap.memory_installed
                    .map_or("—".into(), |i| fmt_bytes(i.saturating_sub(m.total))),
            ),
            (
                "速度",
                modules
                    .first()
                    .filter(|f| f.speed > 0)
                    .map_or("—".into(), |f| format!("{} MT/s", f.speed)),
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
    hero(
        ui,
        &[
            Kpi::new("活动时间", format!("{:.0}", d.active), "%", metric::DISK)
                .bar(d.active / 100.0),
            Kpi::new("读取", fmt_bytes(d.read_rate), "/s", metric::DISK),
            Kpi::new("写入", fmt_bytes(d.write_rate), "/s", metric::NETWORK_UP),
            Kpi::new(
                "平均响应时间",
                format!("{:.2}", d.response_ms),
                "ms",
                metric::DISK,
            ),
        ],
    );
    ui.add_space(10.0);
    let empty = Vec::new();
    let get = |k: &str| {
        series
            .get(&format!("disk.{}.{k}", d.index))
            .unwrap_or(&empty)
    };
    two_columns(
        ui,
        |ui| {
            chart(
                ui,
                "活动时间",
                150.0,
                &[(get("active"), metric::DISK, false)],
                Some(1.0),
                &pct,
                &[],
            )
        },
        |ui| {
            chart(
                ui,
                "传输速率",
                150.0,
                &[
                    (get("read"), metric::DISK, false),
                    (get("write"), metric::NETWORK_UP, true),
                ],
                None,
                &|m| format!("{}/s", fmt_bytes(m as u64)),
                &[("读取", metric::DISK), ("写入", metric::NETWORK_UP)],
            )
        },
    );

    health_section(ui, d);

    section(ui, "属性");
    let yes_no = |b: bool| {
        if b {
            "是".to_string()
        } else {
            "否".to_string()
        }
    };
    kv_list(
        ui,
        &[
            ("型号", d.model.clone()),
            ("容量", fmt_bytes(d.size)),
            ("类型", disk_kind(d)),
            ("分区", d.volumes.len().to_string()),
            ("可移动", yes_no(d.removable)),
            ("系统盘", yes_no(d.volumes.iter().any(|v| v.system))),
            ("页面文件", yes_no(d.volumes.iter().any(|v| v.pagefile))),
        ],
    );

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
            let mut title = if v.label.is_empty() {
                format!("{}:", v.letter)
            } else {
                format!("{}:  {}", v.letter, v.label)
            };
            if v.system {
                title.push_str("  · 系统");
            }
            if v.pagefile {
                title.push_str("  · 页面文件");
            }
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
        kv_list(ui, &items);
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
    let status = match (&n.ssid, n.signal) {
        (Some(ssid), Some(signal)) => {
            Kpi::new("信号强度", signal.to_string(), "%", metric::NETWORK)
                .caption(ssid.clone())
                .bar(signal as f32 / 100.0)
        }
        _ => Kpi::new(
            "状态",
            if n.connected {
                "已连接"
            } else {
                "未连接"
            },
            "",
            metric::NETWORK,
        )
        .caption(n.kind.to_string()),
    };
    hero(
        ui,
        &[
            Kpi::new("接收", fmt_bits(n.rx_rate), "", metric::NETWORK)
                .caption(format!("开机以来 {}", fmt_bytes(n.rx_total))),
            Kpi::new("发送", fmt_bits(n.tx_rate), "", metric::NETWORK_UP)
                .caption(format!("开机以来 {}", fmt_bytes(n.tx_total))),
            Kpi::new(
                "链路速度",
                if n.link_speed > 0 {
                    fmt_bits(n.link_speed / 8)
                } else {
                    "—".into()
                },
                "",
                metric::NETWORK,
            ),
            status,
        ],
    );
    ui.add_space(10.0);
    let empty = Vec::new();
    let get = |k: &str| {
        series
            .get(&format!("net.{}.{k}", n.index))
            .unwrap_or(&empty)
    };
    chart(
        ui,
        "吞吐量",
        190.0,
        &[
            (get("rx"), metric::NETWORK, false),
            (get("tx"), metric::NETWORK_UP, true),
        ],
        None,
        &|m| fmt_bits(m as u64),
        &[("接收", metric::NETWORK), ("发送", metric::NETWORK_UP)],
    );

    section(ui, "适配器");
    let mut items = vec![
        ("名称", n.name.clone()),
        ("描述", n.description.clone()),
        ("类型", n.kind.to_string()),
        (
            "MAC 地址",
            if n.mac.is_empty() {
                "—".into()
            } else {
                n.mac.clone()
            },
        ),
    ];
    if let Some(ssid) = &n.ssid {
        items.push(("Wi-Fi 网络", ssid.clone()));
    }
    for a in &n.ipv4 {
        items.push(("IPv4", a.clone()));
    }
    for a in &n.ipv6 {
        items.push(("IPv6", a.clone()));
    }
    kv_list(ui, &items);
}

/// 当前设备的纯文本摘要（"复制摘要"）。
fn summary(device: Device, snap: &Snapshot) -> String {
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let mut out = Vec::new();
    match device {
        Device::Cpu => {
            let i = &snap.cpu_info;
            out.push(format!("CPU\t{}", i.name));
            out.push(format!("利用率\t{:.0}%", snap.cpu_total));
            if let Some(mhz) = snap.cpu_mhz {
                out.push(format!("速度\t{:.2} GHz", mhz as f64 / 1000.0));
            }
            out.push(format!("基准速度\t{:.2} GHz", i.base_mhz as f64 / 1000.0));
            out.push(format!("插槽\t{}", i.sockets));
            out.push(format!("内核\t{}", i.cores));
            out.push(format!("逻辑处理器\t{}", i.logical));
            out.push(format!(
                "L1 / L2 / L3 缓存\t{} / {} / {}",
                fmt_bytes(i.l1),
                fmt_bytes(i.l2),
                fmt_bytes(i.l3)
            ));
            out.push(format!("进程\t{}", snap.processes.len()));
            out.push(format!("正常运行时间\t{}", fmt_duration(snap.uptime)));
        }
        Device::Memory => {
            let m = &snap.memory;
            out.push(format!("内存\t{:.1} GB", gb(m.total)));
            out.push(format!("使用中\t{:.1} GB", gb(m.in_use())));
            out.push(format!("可用\t{:.1} GB", gb(m.available)));
            out.push(format!(
                "已提交\t{:.1} / {:.1} GB",
                gb(m.commit_used),
                gb(m.commit_limit)
            ));
            out.push(format!("已缓存\t{:.1} GB", gb(m.cache)));
            out.push(format!("分页缓冲池\t{}", fmt_bytes(m.paged_pool)));
            out.push(format!("非分页缓冲池\t{}", fmt_bytes(m.nonpaged_pool)));
            for mm in snap.memory_modules.iter() {
                out.push(format!(
                    "{}\t{} {} {} MT/s {} {}",
                    mm.locator,
                    fmt_bytes(mm.size),
                    mm.kind,
                    mm.speed,
                    mm.manufacturer,
                    mm.part_number
                ));
            }
        }
        Device::Disk(i) => {
            if let Some(d) = snap.disks.iter().find(|d| d.index == i) {
                out.push(format!("磁盘 {}\t{}", d.index, d.model));
                out.push(format!("类型\t{}", disk_kind(d)));
                out.push(format!("容量\t{}", fmt_bytes(d.size)));
                out.push(format!("活动时间\t{:.0}%", d.active));
                out.push(format!(
                    "读取 / 写入\t{}/s / {}/s",
                    fmt_bytes(d.read_rate),
                    fmt_bytes(d.write_rate)
                ));
                out.push(format!("平均响应时间\t{:.2} ms", d.response_ms));
                if let Some(t) = d.health.temperature {
                    out.push(format!("温度\t{t}°C"));
                }
                if let Some(n) = &d.health.nvme {
                    out.push(format!(
                        "剩余寿命\t{}%",
                        100u8.saturating_sub(n.percentage_used.min(100))
                    ));
                    out.push(format!("通电时间\t{} 小时", n.power_on_hours));
                    out.push(format!("累计写入\t{}", fmt_tb(n.data_written)));
                }
                for v in &d.volumes {
                    out.push(format!(
                        "{}:\t{} 可用，共 {} {}",
                        v.letter,
                        fmt_bytes(v.free),
                        fmt_bytes(v.total),
                        v.file_system
                    ));
                }
            }
        }
        Device::Network(i) => {
            if let Some(n) = snap.nics.iter().find(|n| n.index == i) {
                out.push(format!("{}\t{}", n.name, n.description));
                out.push(format!("类型\t{}", n.kind));
                if let Some(ssid) = &n.ssid {
                    out.push(format!("Wi-Fi 网络\t{ssid}"));
                }
                out.push(format!("链路速度\t{}", fmt_bits(n.link_speed / 8)));
                out.push(format!(
                    "接收 / 发送\t{} / {}",
                    fmt_bits(n.rx_rate),
                    fmt_bits(n.tx_rate)
                ));
                for a in n.ipv4.iter().chain(&n.ipv6) {
                    out.push(format!("地址\t{a}"));
                }
                out.push(format!("MAC\t{}", n.mac));
            }
        }
        Device::Gpu(i) => {
            if let Some(g) = snap.gpus.get(i) {
                out.push(format!("GPU {i}\t{}", g.name));
                out.push(format!("利用率\t{:.0}%", g.usage));
                for (name, u) in &g.engines {
                    out.push(format!("{name}\t{u:.1}%"));
                }
                out.push(format!(
                    "专用显存\t{:.2} / {:.1} GB",
                    gb(g.dedicated_used),
                    gb(g.dedicated_total)
                ));
                out.push(format!(
                    "共享 GPU 内存\t{:.2} / {:.1} GB",
                    gb(g.shared_used),
                    gb(g.shared_total)
                ));
            }
        }
    }
    out.join("\n")
}
