//! 自绘小部件：KPI 卡片、图标、搜索框、筛选标签、分段开关、热力底色、字母头像。

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, Ui, emath::GuiRounding, pos2, vec2,
};

use crate::icons_gen::{Icon, PX_MAX, PX_MIN};
use crate::theme::{Palette, metric, semibold};
use pulse_probe::Snapshot;

/// 画一个 jannchie-icons 图标。尺寸换算成设备像素后取对应的像素对齐版本，
/// 并把左上角对齐到像素网格，否则对齐就白做了。
pub fn icon(ui: &Ui, icon: Icon, center: Pos2, size: f32, color: Color32) {
    let ppp = ui.ctx().pixels_per_point();
    let px = ((size * ppp).round() as u32).clamp(PX_MIN, PX_MAX);
    let side = px as f32 / ppp;
    let min = (center - vec2(side, side) / 2.0).round_to_pixels(ppp);
    let rect = Rect::from_min_size(min, vec2(side, side));
    let uri = format!("bytes://icon/{}@{px}.svg", icon.name());
    egui::Image::from_bytes(uri, icon.svg(px).as_bytes())
        .tint(color)
        .paint_at(ui, rect);
}

// ---------------------------------------------------------------- KPI 卡片

const CARD_H: f32 = 118.0;
const PAD: f32 = 14.0;

/// 卡片底板与标题行，返回响应与标题行下方的内容区。
fn card_frame(
    ui: &mut Ui,
    width: f32,
    title: &str,
    glyph: Icon,
    sub: &str,
    color: Color32,
    active: bool,
) -> (Response, Rect) {
    let p = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, CARD_H), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        CornerRadius::same(12),
        if resp.hovered() { p.hover } else { p.surface },
    );
    if active {
        // 当前排序依据的指标：左侧一道指标色的竖条，比整圈描边更克制。
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 16.0), vec2(3.0, CARD_H - 32.0));
        painter.rect_filled(bar, CornerRadius::same(2), color);
    }
    let inner = rect.shrink(PAD);
    icon(
        ui,
        glyph,
        pos2(inner.left() + 8.0, inner.top() + 8.0),
        16.0,
        color,
    );
    painter.text(
        pos2(inner.left() + 22.0, inner.top() + 8.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(12.5),
        p.muted,
    );
    painter.text(
        pos2(inner.right(), inner.top() + 8.0),
        Align2::RIGHT_CENTER,
        sub,
        FontId::proportional(11.5),
        p.faint,
    );
    let content = Rect::from_min_max(pos2(inner.left(), inner.top() + 22.0), inner.right_bottom());
    (resp, content)
}

fn big_value(painter: &Painter, pos: Pos2, text: &str, color: Color32) -> Rect {
    painter.text(
        pos,
        Align2::LEFT_TOP,
        text,
        FontId::new(26.0, semibold()),
        color,
    )
}

/// CPU：大数字 + 每个逻辑处理器一根实时柱。一眼看出是均匀负载还是单线程压满一个核。
pub fn cpu_card(
    ui: &mut Ui,
    width: f32,
    active: bool,
    total: f32,
    cores: &[f32],
    color: Color32,
) -> Response {
    let p = Palette::of(ui.ctx());
    let hottest = cores.iter().copied().fold(0.0, f32::max);
    let sub = format!("{} 线程 · 最忙 {hottest:.0}%", cores.len());
    let (resp, c) = card_frame(ui, width, "CPU", Icon::Cpu, &sub, color, active);
    let painter = ui.painter_at(c.expand(1.0));
    big_value(&painter, c.left_top(), &format!("{total:.0}%"), p.text);

    let bars = Rect::from_min_max(pos2(c.left(), c.bottom() - 34.0), c.right_bottom());
    if cores.is_empty() {
        return resp;
    }
    let gap = if cores.len() > 32 { 1.0 } else { 2.0 };
    let w = ((bars.width() - gap * (cores.len() - 1) as f32) / cores.len() as f32).max(1.0);
    for (i, &u) in cores.iter().enumerate() {
        let x = bars.left() + i as f32 * (w + gap);
        let track = Rect::from_min_size(pos2(x, bars.top()), vec2(w, bars.height()));
        painter.rect_filled(track, CornerRadius::same(2), p.track);
        let h = (u / 100.0).clamp(0.0, 1.0) * bars.height();
        if h > 0.5 {
            let fill = Rect::from_min_max(
                pos2(x, bars.bottom() - h.max(2.0)),
                pos2(x + w, bars.bottom()),
            );
            // 越忙越实，空闲的核只留一点淡色。
            painter.rect_filled(
                fill,
                CornerRadius::same(2),
                color.gamma_multiply(0.45 + 0.55 * u / 100.0),
            );
        }
    }
    resp
}

/// 内存：左边数值，右边双环仪表——外环物理内存占用，内环提交量占提交上限。
pub fn memory_card(ui: &mut Ui, width: f32, active: bool, snap: &Snapshot) -> Response {
    let p = Palette::of(ui.ctx());
    let color = metric::MEMORY;
    let (used, total) = (snap.mem_used, snap.mem_total);
    let (commit_used, commit_limit) = (snap.commit_used, snap.commit_limit);
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let frac = used as f32 / total.max(1) as f32;
    let commit = commit_used as f32 / commit_limit.max(1) as f32;
    let (resp, c) = card_frame(
        ui,
        width,
        "内存",
        Icon::Memory,
        &format!("共 {:.0} GB", gb(total)),
        color,
        active,
    );
    let painter = ui.painter_at(c.expand(4.0));
    big_value(
        &painter,
        c.left_top(),
        &format!("{:.1} GB", gb(used)),
        p.text,
    );
    painter.text(
        pos2(c.left(), c.bottom()),
        Align2::LEFT_BOTTOM,
        format!("已提交 {:.1} / {:.0} GB", gb(commit_used), gb(commit_limit)),
        FontId::proportional(11.5),
        p.faint,
    );

    let r = (c.height() / 2.0).min(34.0);
    let center = pos2(c.right() - r, c.center().y + 2.0);
    ring(&painter, center, r - 4.0, 7.0, frac, color, p.track);
    ring(
        &painter,
        center,
        r - 13.0,
        3.0,
        commit,
        color.gamma_multiply(0.55),
        p.track,
    );
    painter.text(
        center,
        Align2::CENTER_CENTER,
        format!("{:.0}%", frac * 100.0),
        FontId::new(12.5, semibold()),
        p.text,
    );
    resp
}

/// GPU：利用率 + 右上小曲线，底部是专用显存占用条。
pub fn gpu_card(ui: &mut Ui, width: f32, snap: &Snapshot, history: &[f32]) -> Response {
    let p = Palette::of(ui.ctx());
    let color = metric::GPU;
    let name = snap
        .gpus
        .first()
        .map_or("未检测到", |g| short_gpu_name(&g.name))
        .to_string();
    let (resp, c) = card_frame(ui, width, "GPU", Icon::Gpu, &name, color, false);
    let painter = ui.painter_at(c.expand(1.0));
    let value = if snap.gpus.is_empty() {
        "—".to_string()
    } else {
        format!("{:.0}%", snap.gpu_total)
    };
    big_value(&painter, c.left_top(), &value, p.text);

    // 右上：近 60 秒利用率小曲线。
    let chart = Rect::from_min_max(
        pos2(c.left() + c.width() * 0.42, c.top() + 2.0),
        pos2(c.right(), c.top() + 32.0),
    );
    area(&painter, chart, history, 1.0, color, false);

    // 底部：专用显存（多块 GPU 时合计）。
    let (used, total) = snap.gpus.iter().fold((0, 0), |a, g| {
        (a.0 + g.dedicated_used, a.1 + g.dedicated_total)
    });
    let frac = used as f32 / total.max(1) as f32;
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let bar = Rect::from_min_max(pos2(c.left(), c.bottom() - 5.0), c.right_bottom());
    painter.text(
        pos2(c.left(), bar.top() - 5.0),
        Align2::LEFT_BOTTOM,
        format!("显存 {:.1} / {:.0} GB", gb(used), gb(total)),
        FontId::proportional(11.5),
        p.faint,
    );
    painter.text(
        pos2(c.right(), bar.top() - 5.0),
        Align2::RIGHT_BOTTOM,
        format!("{:.0}%", frac * 100.0),
        FontId::proportional(11.5),
        p.faint,
    );
    meter(&painter, bar, frac, color, p.track);
    resp
}

/// 磁盘：左侧总传输速率，右侧每块盘一行活动时间条。
pub fn disk_card(ui: &mut Ui, width: f32, snap: &Snapshot) -> Response {
    let p = Palette::of(ui.ctx());
    let color = metric::DISK;
    let sub = format!("{} 块", snap.disks.len());
    let (resp, c) = card_frame(ui, width, "磁盘", Icon::HardDrive, &sub, color, false);
    let painter = ui.painter_at(c.expand(1.0));
    let (read, write) = snap
        .disks
        .iter()
        .fold((0, 0), |a, d| (a.0 + d.read_rate, a.1 + d.write_rate));
    let fmt = |b: u64| crate::fmt_bytes(b);
    let busiest = snap.disks.iter().map(|d| d.active).fold(0.0, f32::max);
    big_value(&painter, c.left_top(), &format!("{busiest:.0}%"), p.text);
    painter.text(
        pos2(c.left(), c.bottom() - 16.0),
        Align2::LEFT_BOTTOM,
        format!("读 {}/s", fmt(read)),
        FontId::proportional(11.5),
        p.faint,
    );
    painter.text(
        pos2(c.left(), c.bottom()),
        Align2::LEFT_BOTTOM,
        format!("写 {}/s", fmt(write)),
        FontId::proportional(11.5),
        p.faint,
    );

    // 右侧：每块盘的活动时间，最多 4 行。
    let col = Rect::from_min_max(
        pos2(c.left() + c.width() * 0.46, c.top() + 2.0),
        c.right_bottom(),
    );
    let rows = snap.disks.len().min(4);
    if rows > 0 {
        let row_h = col.height() / 4.0;
        for (i, d) in snap.disks.iter().take(rows).enumerate() {
            let y = col.top() + row_h * i as f32 + row_h / 2.0;
            let label = d
                .volumes
                .first()
                .map_or(format!("#{}", d.index), |v| format!("{}:", v.letter));
            painter.text(
                pos2(col.left(), y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(11.5),
                p.muted,
            );
            painter.text(
                pos2(col.right(), y),
                Align2::RIGHT_CENTER,
                format!("{:.0}%", d.active),
                FontId::proportional(11.5),
                p.muted,
            );
            let bar = Rect::from_min_max(
                pos2(col.left() + 24.0, y - 3.0),
                pos2(col.right() - 34.0, y + 3.0),
            );
            meter(&painter, bar, d.active / 100.0, color, p.track);
        }
    }
    resp
}

/// 去掉显卡名里的厂商前缀："NVIDIA GeForce RTX 3090" → "RTX 3090"。
pub fn short_gpu_name(name: &str) -> &str {
    ["NVIDIA GeForce ", "NVIDIA ", "AMD Radeon ", "Intel(R) "]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}

/// 水平进度条：轨道 + 填充，两端圆角。
pub fn meter(painter: &Painter, rect: Rect, frac: f32, color: Color32, track: Color32) {
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    painter.rect_filled(rect, r, track);
    let w = rect.width() * frac.clamp(0.0, 1.0);
    if w > 0.5 {
        let fill = Rect::from_min_size(rect.min, vec2(w.max(rect.height()), rect.height()));
        painter.rect_filled(fill, r, color);
    }
}

/// 网络：下载画在中轴上方、上传镜像在下方，共用纵轴刻度。
pub fn network_card(
    ui: &mut Ui,
    width: f32,
    active: bool,
    snap: &Snapshot,
    rx_hist: &[f32],
    tx_hist: &[f32],
) -> Response {
    let p = Palette::of(ui.ctx());
    let (down, up) = (metric::NETWORK, metric::NETWORK_UP);
    let (rx, tx) = (snap.net_rx, snap.net_tx);
    let (resp, c) = card_frame(ui, width, "网络", Icon::Network, "近 60 秒", down, active);
    let painter = ui.painter_at(c.expand(1.0));

    // 左侧两行：下载（大）、上传（小）。
    icon(
        ui,
        Icon::ArrowDown,
        pos2(c.left() + 7.0, c.top() + 15.0),
        14.0,
        down,
    );
    painter.text(
        pos2(c.left() + 18.0, c.top()),
        Align2::LEFT_TOP,
        crate::fmt_rate_full(rx),
        FontId::new(22.0, semibold()),
        p.text,
    );
    icon(
        ui,
        Icon::ArrowUp,
        pos2(c.left() + 7.0, c.bottom() - 8.0),
        13.0,
        up,
    );
    painter.text(
        pos2(c.left() + 18.0, c.bottom()),
        Align2::LEFT_BOTTOM,
        crate::fmt_rate_full(tx),
        FontId::new(13.0, semibold()),
        p.muted,
    );

    let chart = Rect::from_min_max(
        pos2(c.left() + c.width() * 0.48, c.top() + 2.0),
        c.right_bottom(),
    );
    let max = rx_hist
        .iter()
        .chain(tx_hist)
        .copied()
        .fold(0.0, f32::max)
        .max(1.0);
    let mid = chart.center().y;
    painter.hline(chart.x_range(), mid, Stroke::new(1.0, p.track));
    let upper = Rect::from_min_max(chart.left_top(), pos2(chart.right(), mid));
    let lower = Rect::from_min_max(pos2(chart.left(), mid), chart.right_bottom());
    area(&painter, upper, rx_hist, max, down, false);
    area(&painter, lower, tx_hist, max, up, true);
    resp
}

/// 面积曲线：实线 + 向基线渐隐的填充，最新的点贴着右边。`downward` 时从顶部基线向下画。
/// 返回各数据点的位置（数据不足两个时为 None）。
pub fn area(
    painter: &Painter,
    rect: Rect,
    data: &[f32],
    max: f32,
    color: Color32,
    downward: bool,
) -> Option<Vec<Pos2>> {
    const SLOTS: usize = 60;
    if data.len() < 2 {
        return None;
    }
    let base = if downward { rect.top() } else { rect.bottom() };
    let dir = if downward { 1.0 } else { -1.0 };
    let step = rect.width() / (SLOTS - 1) as f32;
    let x0 = rect.right() - step * (data.len() - 1) as f32;
    let points: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, v)| {
            pos2(
                x0 + step * i as f32,
                base + dir * (v / max).clamp(0.0, 1.0) * (rect.height() - 1.5),
            )
        })
        .collect();

    let mut mesh = Mesh::default();
    let (strong, clear) = (color.gamma_multiply(0.32), color.gamma_multiply(0.0));
    for pt in &points {
        mesh.colored_vertex(*pt, strong);
        mesh.colored_vertex(pos2(pt.x, base), clear);
    }
    for i in 0..points.len() as u32 - 1 {
        let (a, b, c, d) = (i * 2, i * 2 + 1, i * 2 + 2, i * 2 + 3);
        mesh.add_triangle(a, b, c);
        mesh.add_triangle(c, b, d);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points.clone(), Stroke::new(1.5, color)));
    Some(points)
}

/// 圆环进度：从 12 点钟方向顺时针，两端圆头。
fn ring(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    width: f32,
    frac: f32,
    color: Color32,
    track: Color32,
) {
    painter.circle_stroke(center, radius, Stroke::new(width, track));
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.001 {
        return;
    }
    let n = ((64.0 * frac).ceil() as usize).max(2);
    let at = |t: f32| {
        let a = -FRAC_PI_2 + TAU * frac * t;
        center + vec2(a.cos(), a.sin()) * radius
    };
    let points: Vec<Pos2> = (0..=n).map(|i| at(i as f32 / n as f32)).collect();
    painter.circle_filled(points[0], width / 2.0, color);
    painter.circle_filled(*points.last().unwrap(), width / 2.0, color);
    painter.add(Shape::line(points, Stroke::new(width, color)));
}

// ---------------------------------------------------------------- 工具栏控件

pub fn search_box(ui: &mut Ui, text: &mut String, width: f32, hint: &str) -> Response {
    let p = Palette::of(ui.ctx());
    let height = 32.0;
    // 整个框都可点击聚焦，不只是中间的输入区。
    let (rect, bg) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), p.surface);
    icon(
        ui,
        Icon::Search,
        pos2(rect.left() + 17.0, rect.center().y),
        16.0,
        p.muted,
    );

    let clear_w = if text.is_empty() { 8.0 } else { 30.0 };
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 32.0, rect.top()),
        pos2(rect.right() - clear_w, rect.bottom()),
    );
    // place 不移动布局光标；put 会把光标拉回 edit_rect 末端，导致后面的控件压到搜索框上。
    let mut resp = ui.place(
        edit_rect,
        egui::TextEdit::singleline(text)
            .hint_text(RichText::new(hint).color(p.faint))
            .frame(egui::Frame::NONE)
            .vertical_align(egui::Align::Center)
            .desired_width(edit_rect.width()),
    );
    if bg.clicked() {
        resp.request_focus();
    }
    if !text.is_empty() {
        let r =
            Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
        let x = ui.interact(r, resp.id.with("clear"), Sense::click());
        if x.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(6), p.hover);
        }
        icon(
            ui,
            Icon::X,
            r.center(),
            14.0,
            if x.hovered() { p.text } else { p.muted },
        );
        if x.clicked() {
            text.clear();
            resp.mark_changed();
        }
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, p.accent.gamma_multiply(0.7)),
            egui::StrokeKind::Inside,
        );
    }
    resp
}

/// 图标 + 文字的按钮底板，供筛选标签、工具按钮共用。
fn pill(ui: &mut Ui, label: &str, enabled: bool) -> (Response, Rect) {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(12.5),
        Color32::WHITE,
    );
    let size = vec2(galley.size().x + 44.0, 32.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    (resp, rect)
}

fn pill_content(ui: &Ui, rect: Rect, glyph: Icon, label: &str, color: Color32) {
    icon(
        ui,
        glyph,
        pos2(rect.left() + 19.0, rect.center().y),
        15.0,
        color,
    );
    ui.painter().text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        color,
    );
}

/// 可开关的筛选标签。
pub fn toggle_chip(ui: &mut Ui, on: &mut bool, glyph: Icon, label: &str) -> Response {
    let p = Palette::of(ui.ctx());
    let (resp, rect) = pill(ui, label, true);
    if resp.clicked() {
        *on = !*on;
    }
    let (fill, color) = match (*on, resp.hovered()) {
        (true, _) => (p.accent.gamma_multiply(p.selection_alpha + 0.04), p.text),
        (false, true) => (p.hover, p.text),
        (false, false) => (p.surface, p.muted),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    if *on {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(8),
            Stroke::new(1.0, p.accent.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    }
    pill_content(ui, rect, glyph, label, color);
    resp
}

/// 工具栏按钮；`danger` 用于结束进程这类破坏性操作。
pub fn tool_button(ui: &mut Ui, glyph: Icon, label: &str, enabled: bool, danger: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let (resp, rect) = pill(ui, label, enabled);
    let fill = if enabled && resp.hovered() {
        p.hover
    } else {
        p.surface
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    let color = match (enabled, danger) {
        (false, _) => p.faint,
        (true, true) => p.danger,
        (true, false) => p.muted,
    };
    pill_content(ui, rect, glyph, label, color);
    resp
}

/// 两段式开关（带图标）。返回是否被切换。
pub fn segmented(ui: &mut Ui, value: &mut bool, items: [(Icon, &str); 2]) -> bool {
    let p = Palette::of(ui.ctx());
    let seg_w = 70.0;
    let height = 32.0;
    let (rect, _) = ui.allocate_exact_size(vec2(seg_w * 2.0 + 6.0, height), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), p.surface);
    let mut changed = false;
    for (i, (glyph, label)) in items.iter().enumerate() {
        let on = (i == 0) == *value;
        let r = Rect::from_min_size(
            pos2(rect.left() + 3.0 + seg_w * i as f32, rect.top() + 3.0),
            vec2(seg_w, height - 6.0),
        );
        let resp = ui.interact(r, ui.id().with(("seg", i)), Sense::click());
        if on {
            ui.painter().rect_filled(r, CornerRadius::same(6), p.track);
        }
        let color = if on || resp.hovered() {
            p.text
        } else {
            p.faint
        };
        icon(ui, *glyph, pos2(r.left() + 18.0, r.center().y), 14.0, color);
        ui.painter().text(
            pos2(r.left() + 30.0, r.center().y),
            Align2::LEFT_CENTER,
            *label,
            FontId::proportional(12.5),
            color,
        );
        if resp.clicked() && !on {
            *value = i == 0;
            changed = true;
        }
    }
    changed
}

/// 右键菜单项：图标 + 文字，整行可点。
pub fn menu_item(ui: &mut Ui, glyph: Icon, label: &str, danger: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let width = ui.available_width().max(200.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    let color = if danger { p.danger } else { p.text };
    icon(
        ui,
        glyph,
        pos2(rect.left() + 14.0, rect.center().y),
        15.0,
        if danger { p.danger } else { p.muted },
    );
    ui.painter().text(
        pos2(rect.left() + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        color,
    );
    if resp.clicked() {
        ui.close();
    }
    resp
}

/// 开关。返回的响应在切换时 `changed()`。
pub fn switch(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), sense);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let track = match (*on, enabled) {
        (true, true) => p.accent,
        (true, false) => p.accent.gamma_multiply(0.4),
        (false, _) => p.track,
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), track);
    if !*on {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
    }
    let x = if *on {
        rect.right() - 10.0
    } else {
        rect.left() + 10.0
    };
    let knob = if *on { Color32::WHITE } else { p.muted };
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 6.5, knob);
    if enabled {
        resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    resp
}

/// 带勾选标记的菜单项（勾在右侧）。
pub fn menu_check(ui: &mut Ui, glyph: Icon, label: &str, checked: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let resp = menu_item(ui, glyph, label, false);
    if checked {
        icon(
            ui,
            Icon::Check,
            pos2(resp.rect.right() - 14.0, resp.rect.center().y),
            14.0,
            p.accent,
        );
    }
    resp
}

/// 供 egui 原生控件（如子菜单按钮）使用的图标图片。
pub fn icon_image(ui: &Ui, glyph: Icon, size: f32, color: Color32) -> egui::Image<'static> {
    let ppp = ui.ctx().pixels_per_point();
    let px = ((size * ppp).round() as u32).clamp(PX_MIN, PX_MAX);
    let side = px as f32 / ppp;
    egui::Image::from_bytes(
        format!("bytes://icon/{}@{px}.svg", glyph.name()),
        glyph.svg(px).as_bytes(),
    )
    .tint(color)
    .fit_to_exact_size(vec2(side, side))
}

// ---------------------------------------------------------------- 表格

/// 进程名旁的状态标签（"已挂起"、"无响应"）。
pub fn status_chip(ui: &mut Ui, text: &str, color: Color32) {
    let p = Palette::of(ui.ctx());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 12.0, 18.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), p.track);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// 进程名旁的端口小标签。
pub fn port_chip(ui: &mut Ui, port: u16) {
    let p = Palette::of(ui.ctx());
    let galley = ui
        .painter()
        .layout_no_wrap(format!(":{port}"), FontId::monospace(11.0), p.chip);
    let size = vec2(galley.size().x + 10.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(5),
        p.accent.gamma_multiply(p.chip_alpha),
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, p.chip);
}

/// 按负载给单元格铺一层指标色底，类似任务管理器的热力列。
pub fn heat(ui: &Ui, level: f32, color: Color32) {
    let level = level.clamp(0.0, 1.0);
    // 低负载不着色，避免整列都染上颜色。
    if level < 0.05 {
        return;
    }
    let rect = ui.max_rect().expand2(vec2(6.0, 0.0));
    ui.painter()
        .rect_filled(rect, 0.0, color.gamma_multiply(level * 0.28));
}

/// 没有图标的进程用首字母头像。
pub fn avatar(painter: &Painter, rect: Rect, name: &str) {
    let p = Palette::of(painter.ctx());
    let hue = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
        % 360;
    let color = egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.35, 0.55, 1.0);
    painter.rect_filled(
        rect,
        CornerRadius::same(4),
        Color32::from(color).gamma_multiply(0.45),
    );
    let letter = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::new(10.0, semibold()),
        p.text,
    );
}

/// 展开/折叠箭头。
pub fn chevron(ui: &Ui, center: Pos2, expanded: bool, color: Color32) {
    let glyph = if expanded {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    icon(ui, glyph, center, 14.0, color);
}

// ---------------------------------------------------------------- 表格与菜单的共用部件

/// 表头单元格：底部细线 + 左/右对齐的标签。返回标签的响应（可用于点击排序）。
pub fn table_header(
    ui: &mut Ui,
    label: RichText,
    right_aligned: bool,
    trailing: impl FnOnce(&mut Ui),
) -> Response {
    let p = Palette::of(ui.ctx());
    let r = ui.max_rect();
    ui.painter().hline(
        r.x_range().expand(6.0),
        r.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    let layout = if right_aligned {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    let mut trailing = Some(trailing);
    ui.with_layout(layout, |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if !right_aligned {
            ui.add_space(4.0);
        }
        // 附加内容（排序箭头）总在标签外侧：右对齐列里先放，左对齐列里后放。
        if right_aligned && let Some(f) = trailing.take() {
            f(ui);
        }
        let resp = ui.add(egui::Label::new(label).sense(Sense::click()));
        if let Some(f) = trailing.take() {
            f(ui);
        }
        resp
    })
    .inner
}

/// 普通表头：12px 次要色标签。
pub fn header_label(ui: &mut Ui, label: &str, right_aligned: bool) {
    let p = Palette::of(ui.ctx());
    table_header(
        ui,
        RichText::new(label).size(12.0).color(p.muted),
        right_aligned,
        |_| {},
    );
}

/// 右对齐的单元格文字。
pub fn right_label(ui: &mut Ui, text: RichText) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(text)
    });
}

/// 右键菜单顶部的标题行。
pub fn menu_heading(ui: &mut Ui, text: &str, min_width: f32) {
    let p = Palette::of(ui.ctx());
    ui.set_min_width(min_width);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.0).color(p.muted));
    ui.add_space(2.0);
}

/// 带图标的子菜单入口。
pub fn submenu(
    ui: &mut Ui,
    glyph: Icon,
    label: &str,
    min_width: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    let p = Palette::of(ui.ctx());
    ui.menu_image_text_button(icon_image(ui, glyph, 15.0, p.muted), label, |ui| {
        ui.set_min_width(min_width);
        add_contents(ui);
    });
}

#[cfg(test)]
mod tests {
    use crate::icons_gen::Icon;

    #[test]
    /// 生成的 SVG 必须渲染成白色描边（运行时靠 tint 着色）。曾因 `stroke="undefined"` 渲染成黑块。
    fn svg_rasterizes_white_strokes() {
        let svg = Icon::Search.svg(16);
        let img = egui_extras::image::load_svg_bytes_with_size(
            svg.as_bytes(),
            eframe::egui::SizeHint::Size {
                width: 16,
                height: 16,
                maintain_aspect_ratio: false,
            },
            &Default::default(),
        )
        .expect("svg");
        let opaque: Vec<_> = img.pixels.iter().filter(|c| c.a() > 0).collect();
        assert!(!opaque.is_empty());
        assert!(
            opaque.iter().all(|c| c.r() == c.a()),
            "strokes should be white (premultiplied)"
        );
    }
}
