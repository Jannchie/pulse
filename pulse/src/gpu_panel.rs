//! GPU 详情页：关键指标、利用率与显存曲线、各引擎、使用 GPU 的进程。

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Ui, pos2, vec2};
use pulse_probe::{GpuStat, Snapshot};

use crate::icons::Icons;
use crate::perf::{Kpi, chart, hero, pct, section, two_columns};
use crate::theme::{Palette, metric, semibold};
use crate::widgets;

/// 一块 GPU 近 60 秒的历史（利用率与显存都是 0–1 的比例）。
#[derive(Clone, Default)]
pub struct GpuHistory {
    pub usage: Vec<f32>,
    pub engines: Vec<Vec<f32>>,
    pub dedicated: Vec<f32>,
    pub shared: Vec<f32>,
}

pub enum PanelAction {
    /// 在主表格里搜索这个进程名。
    Search(String),
}

pub fn page(
    ui: &mut Ui,
    gpu: &GpuStat,
    h: &GpuHistory,
    snap: &Snapshot,
    icons: &mut Icons,
) -> Option<PanelAction> {
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let frac = |used: u64, total: u64| used as f32 / total.max(1) as f32;
    let users = by_program(snap);
    let busiest = gpu
        .engines
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|e| e.1 >= 0.5);
    hero(
        ui,
        &[
            Kpi::new("利用率", format!("{:.0}", gpu.usage), "%", metric::GPU)
                .caption(busiest.map_or("空闲".into(), |e| format!("最忙：{}", e.0)))
                .bar(gpu.usage / 100.0),
            Kpi::new(
                "专用显存",
                format!("{:.1}", gb(gpu.dedicated_used)),
                format!("/ {:.0} GB", gb(gpu.dedicated_total)),
                metric::GPU,
            )
            .caption(format!(
                "{:.0}%",
                frac(gpu.dedicated_used, gpu.dedicated_total) * 100.0
            ))
            .bar(frac(gpu.dedicated_used, gpu.dedicated_total)),
            Kpi::new(
                "共享 GPU 内存",
                format!("{:.1}", gb(gpu.shared_used)),
                format!("/ {:.0} GB", gb(gpu.shared_total)),
                metric::MEMORY,
            )
            .caption("显存不够时借用的系统内存")
            .bar(frac(gpu.shared_used, gpu.shared_total)),
            Kpi::new("使用中的程序", users.len().to_string(), "个", metric::GPU).caption(
                users
                    .first()
                    .map_or("—".into(), |u| format!("显存最多：{}", u.name)),
            ),
        ],
    );

    ui.add_space(10.0);
    two_columns(
        ui,
        |ui| {
            chart(
                ui,
                "利用率",
                150.0,
                &[(&h.usage, metric::GPU, false)],
                Some(1.0),
                &pct,
                &[],
            )
        },
        |ui| {
            chart(
                ui,
                "专用显存",
                150.0,
                &[(&h.dedicated, metric::GPU, false)],
                Some(1.0),
                &pct,
                &[],
            )
        },
    );

    let mut action = None;
    two_columns(
        ui,
        |ui| {
            section(ui, "引擎");
            engines(ui, gpu, h);
        },
        |ui| {
            section(ui, "使用 GPU 的程序");
            action = programs(ui, &users, icons);
        },
    );
    action
}

/// 引擎网格：名称、当前利用率、近 60 秒曲线。长期空闲的引擎用弱化色。
fn engines(ui: &mut Ui, gpu: &GpuStat, h: &GpuHistory) {
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let cols = if w > 380.0 { 2 } else { 1 };
    let gap = 8.0;
    let cell_w = (w - gap * (cols - 1) as f32) / cols as f32;
    for (row, chunk) in gpu.engines.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (k, (name, usage)) in chunk.iter().enumerate() {
                let hist = h.engines.get(row * cols + k).map_or(&[][..], Vec::as_slice);
                let idle = *usage < 0.05 && hist.iter().all(|&v| v < 0.0005);
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, 56.0), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(10), p.surface);
                painter.text(
                    rect.left_top() + vec2(12.0, 9.0),
                    Align2::LEFT_TOP,
                    name,
                    FontId::proportional(12.5),
                    if idle { p.faint } else { p.text },
                );
                painter.text(
                    rect.right_top() + vec2(-12.0, 9.0),
                    Align2::RIGHT_TOP,
                    format!("{usage:.1}%"),
                    FontId::new(12.5, semibold()),
                    if idle { p.faint } else { p.text },
                );
                let chart =
                    Rect::from_min_max(rect.min + vec2(12.0, 30.0), rect.max - vec2(12.0, 8.0));
                widgets::area(&painter, chart, hist, 1.0, metric::GPU, false);
            }
        });
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// 按程序名合并的 GPU 使用情况。
struct Program {
    name: String,
    count: usize,
    path: Option<Arc<str>>,
    usage: f32,
    dedicated: u64,
    shared: u64,
}

fn by_program(snap: &Snapshot) -> Vec<Program> {
    let mut map: HashMap<&str, Program> = HashMap::new();
    for pr in snap
        .processes
        .iter()
        .filter(|pr| pr.gpu_dedicated > 0 || pr.gpu >= 0.05)
    {
        let e = map.entry(pr.name.as_str()).or_insert_with(|| Program {
            name: pr.name.clone(),
            count: 0,
            path: None,
            usage: 0.0,
            dedicated: 0,
            shared: 0,
        });
        e.count += 1;
        e.usage += pr.gpu;
        e.dedicated += pr.gpu_dedicated;
        e.shared += pr.gpu_shared;
        if e.path.is_none() {
            e.path = pr.path.clone();
        }
    }
    let mut out: Vec<Program> = map.into_values().collect();
    out.sort_by(|a, b| {
        b.dedicated
            .cmp(&a.dedicated)
            .then(b.usage.total_cmp(&a.usage))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// 程序列表：名称、GPU 利用率、专用显存（底色条表示相对第一名的占比）。点击在进程页中搜索。
fn programs(ui: &mut Ui, list: &[Program], icons: &mut Icons) -> Option<PanelAction> {
    const ROWS: usize = 8;
    const ROW_H: f32 = 32.0;
    let p = Palette::of(ui.ctx());
    if list.is_empty() {
        ui.label(
            egui::RichText::new("没有程序在使用 GPU")
                .size(12.0)
                .color(p.faint),
        );
        return None;
    }
    let shown = &list[..list.len().min(ROWS)];
    let w = ui.available_width();
    let (card, _) = ui.allocate_exact_size(
        vec2(w, 30.0 + shown.len() as f32 * ROW_H + 6.0),
        Sense::hover(),
    );
    let painter = ui.painter_at(card);
    painter.rect_filled(card, CornerRadius::same(10), p.surface);
    let col_vram = card.right() - 14.0;
    let col_gpu = col_vram - 92.0;
    let head = |x: f32, text: &str, align: Align2| {
        painter.text(
            pos2(x, card.top() + 15.0),
            align,
            text,
            FontId::proportional(11.5),
            p.muted,
        );
    };
    head(card.left() + 14.0, "程序", Align2::LEFT_CENTER);
    head(col_gpu, "GPU", Align2::RIGHT_CENTER);
    head(col_vram, "专用显存", Align2::RIGHT_CENTER);

    let max = shown[0].dedicated.max(1);
    let mut action = None;
    for (k, prog) in shown.iter().enumerate() {
        let row = Rect::from_min_size(
            pos2(card.left() + 4.0, card.top() + 30.0 + k as f32 * ROW_H),
            vec2(card.width() - 8.0, ROW_H),
        );
        let resp = ui.interact(row, ui.id().with(("gpu-prog", k)), Sense::click());
        if resp.hovered() {
            painter.rect_filled(row, CornerRadius::same(6), p.hover);
        }
        // 显存占比条，画在显存列下方，便于横向比较。
        let bar_w = 80.0 * prog.dedicated as f32 / max as f32;
        let bar = Rect::from_min_max(
            pos2(col_vram - bar_w, row.bottom() - 6.0),
            pos2(col_vram, row.bottom() - 4.0),
        );
        painter.rect_filled(bar, CornerRadius::same(1), metric::GPU.gamma_multiply(0.7));

        let icon_rect =
            Rect::from_center_size(pos2(row.left() + 18.0, row.center().y), vec2(16.0, 16.0));
        match prog.path.as_ref().and_then(|path| icons.get(path)) {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => widgets::avatar(&painter, icon_rect, &prog.name),
        }
        let name_clip = Rect::from_min_max(
            pos2(row.left() + 32.0, row.top()),
            pos2(col_gpu - 50.0, row.bottom()),
        );
        let name = painter.with_clip_rect(name_clip).text(
            pos2(name_clip.left(), row.center().y),
            Align2::LEFT_CENTER,
            &prog.name,
            FontId::proportional(12.5),
            p.text,
        );
        if prog.count > 1 {
            painter.with_clip_rect(name_clip).text(
                pos2(name.right() + 6.0, row.center().y),
                Align2::LEFT_CENTER,
                format!("×{}", prog.count),
                FontId::proportional(11.5),
                p.faint,
            );
        }
        let idle = prog.usage < 0.05;
        painter.text(
            pos2(col_gpu, row.center().y),
            Align2::RIGHT_CENTER,
            if idle {
                "—".into()
            } else {
                format!("{:.1}%", prog.usage)
            },
            FontId::proportional(12.5),
            if idle { p.faint } else { p.text },
        );
        painter.text(
            pos2(col_vram, row.center().y - 1.0),
            Align2::RIGHT_CENTER,
            if prog.dedicated == 0 {
                "—".into()
            } else {
                crate::fmt_bytes(prog.dedicated)
            },
            FontId::new(12.5, semibold()),
            p.text,
        );
        let tip = format!(
            "{}{}\n专用显存 {} · 共享 {}\n点击在进程列表中搜索",
            prog.name,
            if prog.count > 1 {
                format!("（{} 个进程）", prog.count)
            } else {
                String::new()
            },
            crate::fmt_bytes(prog.dedicated),
            crate::fmt_bytes(prog.shared),
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tip)
            .clicked()
        {
            action = Some(PanelAction::Search(prog.name.clone()));
        }
    }
    if list.len() > ROWS {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!("另有 {} 个程序", list.len() - ROWS))
                .size(11.5)
                .color(p.faint),
        );
    }
    action
}
