//! GPU 详情页的组成部分：引擎利用率、专用显存与共享 GPU 内存、显存占用最多的进程。

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Ui, pos2, vec2,
};
use pulse_probe::{GpuStat, Snapshot};

use crate::icons::Icons;
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

/// 引擎与显存两部分；利用率大图由性能页统一绘制。
pub fn adapter(ui: &mut Ui, gpu: &GpuStat, h: &GpuHistory) {
    section(ui, "引擎");
    for (j, (name, usage)) in gpu.engines.iter().enumerate() {
        let hist = h.engines.get(j).map_or(&[][..], Vec::as_slice);
        engine_row(ui, name, *usage, hist);
    }

    section(ui, "专用显存");
    memory_block(
        ui,
        gpu.dedicated_used,
        gpu.dedicated_total,
        &h.dedicated,
        metric::GPU,
    );
    section(ui, "共享 GPU 内存");
    memory_block(
        ui,
        gpu.shared_used,
        gpu.shared_total,
        &h.shared,
        metric::MEMORY,
    );
}

fn engine_row(ui: &mut Ui, name: &str, usage: f32, hist: &[f32]) {
    let p = Palette::of(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    let idle = usage < 0.05 && hist.iter().all(|&v| v < 0.0005);
    let fg = if idle { p.faint } else { p.text };
    painter.text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(12.5),
        fg,
    );
    painter.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        format!("{usage:.1}%"),
        FontId::proportional(12.5),
        fg,
    );
    // 中间：迷你曲线（近 60 秒，纵轴 0–100%）。
    let chart = Rect::from_min_max(
        pos2(rect.left() + 92.0, rect.top() + 4.0),
        pos2(rect.right() - 52.0, rect.bottom() - 4.0),
    );
    painter.rect_filled(
        chart.expand2(vec2(0.0, 1.0)),
        CornerRadius::same(3),
        p.surface,
    );
    widgets::area(&painter, chart, hist, 1.0, metric::GPU, false);
}

fn memory_block(ui: &mut Ui, used: u64, total: u64, hist: &[f32], color: Color32) {
    let p = Palette::of(ui.ctx());
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let frac = used as f32 / total.max(1) as f32;
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 92.0), Sense::hover());
    let painter = ui.painter_at(rect.expand(1.0));
    painter.rect_filled(rect, CornerRadius::same(10), p.surface);
    let inner = rect.shrink(12.0);
    let value = painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        format!("{:.2} GB", gb(used)),
        FontId::new(18.0, semibold()),
        p.text,
    );
    painter.text(
        pos2(value.right() + 6.0, value.bottom() - 2.0),
        Align2::LEFT_BOTTOM,
        format!("/ {:.1} GB", gb(total)),
        FontId::proportional(12.0),
        p.muted,
    );
    painter.text(
        inner.right_top(),
        Align2::RIGHT_TOP,
        format!("{:.0}%", frac * 100.0),
        FontId::new(13.0, semibold()),
        p.muted,
    );
    let chart = Rect::from_min_max(
        pos2(inner.left(), inner.top() + 28.0),
        pos2(inner.right(), inner.bottom() - 18.0),
    );
    widgets::area(&painter, chart, hist, 1.0, color, false);
    let bar = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - 10.0),
        pos2(inner.right(), inner.bottom() - 5.0),
    );
    widgets::meter(&painter, bar, frac, color, p.track);
}

/// 显存占用最多的程序（按名称合并），点击即在主表格中搜索。
pub fn top_processes(ui: &mut Ui, snap: &Snapshot, icons: &mut Icons) -> Option<PanelAction> {
    let p = Palette::of(ui.ctx());
    let mut by_name: HashMap<String, (u64, usize, Option<std::sync::Arc<str>>)> = HashMap::new();
    for pr in snap.processes.iter().filter(|pr| pr.gpu_dedicated > 0) {
        let e = by_name.entry(pr.name.clone()).or_insert((0, 0, None));
        e.0 += pr.gpu_dedicated;
        e.1 += 1;
        if e.2.is_none() {
            e.2 = pr.path.clone();
        }
    }
    let mut top: Vec<_> = by_name.into_iter().collect();
    top.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    top.truncate(10);
    section(ui, "显存占用最多");
    if top.is_empty() {
        ui.label(
            RichText::new("没有进程占用专用显存")
                .size(12.0)
                .color(p.faint),
        );
        return None;
    }
    let max = top[0].1.0.max(1);
    let mut action = None;
    for (name, (bytes, count, path)) in top {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
        let painter = ui.painter_at(rect.expand(1.0));
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(6), p.hover);
        }
        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + 14.0, rect.top() + 13.0),
            vec2(16.0, 16.0),
        );
        match path.as_ref().and_then(|path| icons.get(path)) {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => widgets::avatar(&painter, icon_rect, &name),
        }
        let label = if count > 1 {
            format!("{name}  ×{count}")
        } else {
            name.clone()
        };
        painter.text(
            pos2(rect.left() + 30.0, rect.top() + 13.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            p.text,
        );
        painter.text(
            pos2(rect.right() - 6.0, rect.top() + 13.0),
            Align2::RIGHT_CENTER,
            crate::fmt_bytes(bytes),
            FontId::new(12.5, semibold()),
            p.text,
        );
        // 相对第一名的占比条，便于一眼比较。
        let bar = Rect::from_min_max(
            pos2(rect.left() + 30.0, rect.bottom() - 7.0),
            pos2(rect.right() - 6.0, rect.bottom() - 4.0),
        );
        widgets::meter(
            &painter,
            bar,
            bytes as f32 / max as f32,
            metric::GPU.gamma_multiply(0.8),
            p.track,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("在列表中搜索")
            .clicked()
        {
            action = Some(PanelAction::Search(name));
        }
    }
    action
}
