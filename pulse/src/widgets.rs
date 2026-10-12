//! 自绘小部件：KPI 卡片、图标、搜索框、筛选标签、分段开关、热力底色、字母头像。

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Response, RichText,
    Sense, Shape, Stroke, Ui, emath::GuiRounding, pos2, vec2,
};

use crate::icons_gen::{Icon, PX_MAX, PX_MIN};
use crate::theme::{Palette, metric, semibold};
use pulse_probe::{Snapshot, t, tf};

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
    efficient: &[bool],
    color: Color32,
) -> Response {
    let p = Palette::of(ui.ctx());
    let hottest = cores.iter().copied().fold(0.0, f32::max);
    let sub = tf!(
        "{} 线程 · 最忙 {hottest:.0}%",
        "{} threads · peak {hottest:.0}%",
        "{} スレッド · 最大 {hottest:.0}%",
        cores.len()
    );
    let (resp, c) = card_frame(ui, width, "CPU", Icon::Cpu, &sub, color, active);
    let painter = ui.painter_at(c.expand(1.0));
    big_value(&painter, c.left_top(), &format!("{total:.0}%"), p.text);

    let bars = Rect::from_min_max(pos2(c.left(), c.bottom() - 34.0), c.right_bottom());
    if cores.is_empty() {
        return resp;
    }
    // 点阵：每个核心一列小点、核心之间空一列，大小核交界处再多空一列；按占用率从下往上点亮。
    const PER_CORE: usize = 1;
    let is_e = |i: usize| efficient.get(i).copied().unwrap_or(false);
    let boundaries = (1..cores.len()).filter(|&i| is_e(i) != is_e(i - 1)).count();
    let dot_cols = cores.len() * (PER_CORE + 1) - 1 + boundaries;
    let feather = 1.0 / painter.pixels_per_point();
    let (pitch, dot) = fill_pitch(bars.width(), dot_cols, 0.67);
    let rows = ((bars.height() - dot) / pitch).floor().max(0.0) as usize + 1;
    let x0 = bars.left();
    let bottom = bars.bottom();
    let mut mesh = Mesh::default();
    let mut col = 0;
    for (i, &u) in cores.iter().enumerate() {
        if i > 0 && is_e(i) != is_e(i - 1) {
            col += 1;
        }
        let c = if is_e(i) {
            metric::CPU_EFFICIENT
        } else {
            color
        };
        let dim = c.gamma_multiply(0.07);
        let n = ((u / 100.0).clamp(0.0, 1.0) * rows as f32).round() as usize;
        let lit = if u > 0.5 { n.max(1) } else { n };
        for _ in 0..PER_CORE {
            let x = x0 + col as f32 * pitch;
            for r in 0..rows {
                let y = bottom - r as f32 * pitch - dot;
                let fill = if r < lit {
                    c.gamma_multiply(0.45 + 0.55 * (r + 1) as f32 / lit as f32)
                } else {
                    dim
                };
                aa_dot(
                    &mut mesh,
                    Rect::from_min_size(pos2(x, y), vec2(dot, dot)),
                    fill,
                    feather,
                );
            }
            col += 1;
        }
        col += 1;
    }
    painter.add(Shape::mesh(mesh));
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
        t!("内存", "Memory", "メモリ"),
        Icon::Memory,
        &tf!("共 {:.0} GB", "{:.0} GB total", "合計 {:.0} GB", gb(total)),
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
        tf!(
            "已提交 {:.1} / {:.0} GB",
            "Commit {:.1} / {:.0} GB",
            "コミット {:.1} / {:.0} GB",
            gb(commit_used),
            gb(commit_limit)
        ),
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
        .map_or(
            t!("未检测到", "Not detected", "検出されません"),
            |g| short_gpu_name(&g.name),
        )
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
        tf!(
            "显存 {:.1} / {:.0} GB",
            "VRAM {:.1} / {:.0} GB",
            "VRAM {:.1} / {:.0} GB",
            gb(used),
            gb(total)
        ),
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
    let sub = match snap.disks.len() {
        1 => t!("1 块", "1 disk", "1 台").to_string(),
        n => tf!("{n} 块", "{n} disks", "{n} 台"),
    };
    let (resp, c) = card_frame(
        ui,
        width,
        t!("磁盘", "Disk", "ディスク"),
        Icon::HardDrive,
        &sub,
        color,
        false,
    );
    let painter = ui.painter_at(c.expand(1.0));
    let (read, write) = snap
        .disks
        .iter()
        .fold((0, 0), |a, d| (a.0 + d.read_rate, a.1 + d.write_rate));
    let fmt = |b: u64| crate::fmt_bytes(b);
    let busiest = snap.disks.iter().map(|d| d.active).fold(0.0, f32::max);
    big_value(&painter, c.left_top(), &format!("{busiest:.0}%"), p.text);
    let rates = [
        tf!("读 {}/s", "R {}/s", "読込 {}/s", fmt(read)),
        tf!("写 {}/s", "W {}/s", "書込 {}/s", fmt(write)),
    ];
    let mut rates_w: f32 = 0.0;
    for (text, y) in rates.into_iter().zip([c.bottom() - 16.0, c.bottom()]) {
        let r = painter.text(
            pos2(c.left(), y),
            Align2::LEFT_BOTTOM,
            text,
            FontId::proportional(11.5),
            p.faint,
        );
        rates_w = rates_w.max(r.width());
    }

    // 右侧：每块盘的活动时间，最多 4 行。速率文字较长（日文、大数值）时整列右移让开。
    let col_left = (c.width() * 0.46).max(rates_w + 10.0).min(c.width() * 0.6);
    let col = Rect::from_min_max(pos2(c.left() + col_left, c.top() + 2.0), c.right_bottom());
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

/// Pulse 图标，按物理像素渲染成贴图（每个尺寸缓存一份），点阵不糊。返回所占区域。
pub fn brand_mark(ui: &mut Ui, size: f32) -> Rect {
    let ctx = ui.ctx().clone();
    let px = (size * ctx.pixels_per_point()).round().max(16.0) as u32;
    let id = egui::Id::new(("brand-mark", px));
    let tex = match ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        Some(t) => t,
        None => {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [px as usize, px as usize],
                &crate::brand_art::brand_rgba(px),
            );
            let t = ctx.load_texture(format!("brand-{px}"), image, egui::TextureOptions::NEAREST);
            ctx.data_mut(|d| d.insert_temp(id, t.clone()));
            t
        }
    };
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    // 左上角对齐到物理像素，贴图与屏幕像素一一对应。
    let ppp = ctx.pixels_per_point();
    let min = pos2(
        (rect.left() * ppp).round() / ppp,
        (rect.top() * ppp).round() / ppp,
    );
    let draw = Rect::from_min_size(min, vec2(px as f32 / ppp, px as f32 / ppp));
    ui.painter().image(
        tex.id(),
        draw,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    rect
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
    // 与图表同一套点阵语言：一排方点，边长等于条的高度，点距微调到正好铺满；
    // 按比例从左往右点亮，最后一个点按余数调透明度，数值变化是连续的。
    let dot = rect.height();
    let n = (((rect.width() + dot * 0.5) / (dot * 1.5)).floor() as usize).max(2);
    let pitch = (rect.width() - dot) / (n - 1) as f32;
    let lit = frac.clamp(0.0, 1.0) * n as f32;
    let feather = 1.0 / painter.pixels_per_point();
    let mut mesh = Mesh::default();
    for i in 0..n {
        let r = Rect::from_min_size(
            pos2(rect.left() + i as f32 * pitch, rect.top()),
            vec2(dot, dot),
        );
        let on = (lit - i as f32).clamp(0.0, 1.0);
        aa_dot(&mut mesh, r, track, feather);
        if on > 0.0 {
            aa_dot(&mut mesh, r, color.gamma_multiply(on), feather);
        }
    }
    painter.add(Shape::mesh(mesh));
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
    let (resp, c) = card_frame(
        ui,
        width,
        t!("网络", "Network", "ネットワーク"),
        Icon::Network,
        t!("近 60 秒", "Last 60 s", "直近 60 秒"),
        down,
        active,
    );
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
    // 上下两半之间的间隔与点距一致。
    let gap = dot_gap(chart.width(), chart.height() / 2.0);
    let mid = chart.center().y;
    let upper = Rect::from_min_max(chart.left_top(), pos2(chart.right(), mid - gap / 2.0));
    let lower = Rect::from_min_max(pos2(chart.left(), mid + gap / 2.0), chart.right_bottom());
    area(&painter, upper, rx_hist, max, down, false);
    area(&painter, lower, tx_hist, max, up, true);
    resp
}

/// 已采集的样本总数（所有曲线每次采集都推入一个样本）。点阵按这个绝对序号分列，
/// 新样本到来时历史列保持不变、只整体左移，不会因为分组边界移动而闪烁。
static SAMPLE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn set_sample_seq(seq: u64) {
    SAMPLE_SEQ.store(seq, std::sync::atomic::Ordering::Relaxed);
}

/// 点阵布局：列数取 60 的约数或倍数（每列合并整数个样本，或每个样本占整数列），
/// 挑点距最接近目标（小图 3pt、大图 7pt）的那个，再按实际宽度微调点距，点阵正好铺满。
struct Grid {
    /// 每个样本占几列。
    per_sample: u64,
    /// 每列合并几个样本。
    per_col: u64,
    cols: usize,
    pitch: f32,
    dot: f32,
}

fn grid(width: f32, height: f32) -> Grid {
    const SLOTS: u64 = 60;
    let (target, ratio) = if height < 40.0 {
        (3.0, 0.67)
    } else {
        (7.0, 0.64)
    };
    let (per_sample, per_col, cols) = [1u64, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30, 60]
        .into_iter()
        .map(|d| (1, d, SLOTS / d))
        .chain((2..=8).map(|k| (k, 1, SLOTS * k)))
        .min_by(|a, b| {
            let err = |c: u64| (width / c as f32 - target).abs();
            err(a.2).total_cmp(&err(b.2))
        })
        .unwrap_or((1, 1, SLOTS));
    // 最后一列只有点、没有点后的空隙，所以宽度 = (cols - 1) 个点距 + 一个点。
    let (pitch, dot) = fill_pitch(width, cols as usize, ratio);
    Grid {
        per_sample,
        per_col,
        cols: cols as usize,
        pitch,
        dot,
    }
}

/// 点阵正好铺满 `width`：点距与点的边长都用精确的小数，靠 [`aa_dot`] 的抗锯齿保证每个点看起来一样大。
/// 返回 (点距, 点的边长)。
fn fill_pitch(width: f32, cols: usize, ratio: f32) -> (f32, f32) {
    let ideal = width / (cols as f32 - 1.0 + ratio);
    let dot = ideal * ratio;
    let pitch = if cols > 1 {
        (width - dot) / (cols - 1) as f32
    } else {
        ideal
    };
    (pitch, dot)
}

/// 抗锯齿方点：内圈实色，外面一圈 1 物理像素宽、渐变到透明（与 egui 画形状时的 feathering 相同）。
/// 位置和尺寸是小数也不会被硬切成不同的像素宽度。
fn aa_dot(mesh: &mut Mesh, rect: Rect, color: Color32, feather: f32) {
    let inner = rect.shrink(feather / 2.0);
    let outer = rect.expand(feather / 2.0);
    let base = mesh.vertices.len() as u32;
    for p in [
        inner.left_top(),
        inner.right_top(),
        inner.right_bottom(),
        inner.left_bottom(),
    ] {
        mesh.colored_vertex(p, color);
    }
    for p in [
        outer.left_top(),
        outer.right_top(),
        outer.right_bottom(),
        outer.left_bottom(),
    ] {
        mesh.colored_vertex(p, Color32::TRANSPARENT);
    }
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
    for k in 0..4 {
        let n = (k + 1) % 4;
        mesh.add_triangle(base + k, base + 4 + k, base + 4 + n);
        mesh.add_triangle(base + k, base + 4 + n, base + n);
    }
}

/// 点阵里相邻两点之间的空隙，镜像图上下两半之间也留这么宽。
pub fn dot_gap(width: f32, height: f32) -> f32 {
    let g = grid(width, height);
    g.pitch - g.dot
}

/// 点阵柱状图：横纵等距的方点网格，最新的在最右。每列从基线点亮到数值对应的高度，越靠上越亮；
/// 未点亮的点留很淡的底色。列与样本的对应见 [`grid`]，合并多个样本的列按绝对序号对齐、取最大值。`downward` 时从顶部向下长。
pub fn area(painter: &Painter, rect: Rect, data: &[f32], max: f32, color: Color32, downward: bool) {
    let Grid {
        per_sample,
        per_col,
        cols,
        pitch,
        dot,
    } = grid(rect.width(), rect.height());
    let feather = 1.0 / painter.pixels_per_point();
    let rows = ((rect.height() - dot) / pitch).floor().max(0.0) as usize + 1;
    // 取整后点阵会比 rect 略窄，居中放；起点和基线都对齐到像素。
    let x0 = rect.left();
    let (top, bottom) = (rect.top(), rect.bottom());
    let dim = color.gamma_multiply(0.07);
    let max = max.max(1e-6);

    // 绝对序号 → 数值。data 的最后一个是序号 seq-1 的样本。
    let seq = SAMPLE_SEQ
        .load(std::sync::atomic::Ordering::Relaxed)
        .max(data.len() as u64);
    let first = seq - data.len() as u64;
    let sample = |abs: u64| (abs >= first && abs < seq).then(|| data[(abs - first) as usize]);
    // 最右一列对应的样本组：按 per_col 对齐到绝对序号。
    let last_group = seq.saturating_sub(1) / per_col;

    let mut mesh = Mesh::default();
    for c in 0..cols {
        let from_right = (cols - 1 - c) as u64;
        let value = if per_col == 1 {
            // 每个样本占 per_sample 列。
            seq.checked_sub(1 + from_right / per_sample)
                .and_then(sample)
        } else {
            let g = last_group.checked_sub(from_right);
            g.and_then(|g| {
                (g * per_col..(g + 1) * per_col)
                    .filter_map(sample)
                    .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
            })
        };
        let lit = value.map_or(0, |v| {
            let n = ((v / max).clamp(0.0, 1.0) * rows as f32).round() as usize;
            // 有数据但很小时也点亮一格，和"没有数据"区分开。
            if v > max * 0.004 { n.max(1) } else { n }
        });
        let x = x0 + c as f32 * pitch;
        for r in 0..rows {
            let y = if downward {
                top + r as f32 * pitch
            } else {
                bottom - r as f32 * pitch - dot
            };
            let fill = if r < lit {
                color.gamma_multiply(0.45 + 0.55 * (r + 1) as f32 / lit as f32)
            } else {
                dim
            };
            aa_dot(
                &mut mesh,
                Rect::from_min_size(pos2(x, y), vec2(dot, dot)),
                fill,
                feather,
            );
        }
    }
    painter.add(Shape::mesh(mesh));
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
/// [`tool_button`] / [`toggle_chip`] 的宽度，排版前预留空间用。
pub fn pill_width(ui: &Ui, label: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        FontId::proportional(12.5),
        Color32::WHITE,
    );
    galley.size().x + 44.0
}

fn pill(ui: &mut Ui, label: &str, enabled: bool) -> (Response, Rect) {
    let size = vec2(pill_width(ui, label), 32.0);
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
    // 至少 70pt；文字较长（英文）时按最宽的一项加图标与边距。
    let seg_w = items
        .iter()
        .map(|(_, label)| {
            ui.painter()
                .layout_no_wrap(label.to_string(), FontId::proportional(12.5), p.text)
                .size()
                .x
                + 42.0
        })
        .fold(70.0, f32::max)
        .ceil();
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
    menu_entry(ui, Some(glyph), label, danger)
}

fn menu_entry(ui: &mut Ui, glyph: Option<Icon>, label: &str, danger: bool) -> Response {
    let (resp, bg) = menu_row(ui, glyph, label, danger);
    if resp.hovered() {
        let p = Palette::of(ui.ctx());
        ui.painter().set(
            bg,
            Shape::rect_filled(resp.rect, CornerRadius::same(6), p.hover),
        );
    }
    if resp.clicked() {
        ui.close();
    }
    resp
}

/// 菜单行的公共部分：（可选的）图标 + 文字，背景留一个占位，由调用方按悬停 / 展开状态填充。
/// 宽度跟随菜单（菜单本身定宽）。
fn menu_row(
    ui: &mut Ui,
    glyph: Option<Icon>,
    label: &str,
    danger: bool,
) -> (Response, egui::layers::ShapeIdx) {
    let p = Palette::of(ui.ctx());
    let bg = ui.painter().add(Shape::Noop);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if let Some(glyph) = glyph {
        icon(
            ui,
            glyph,
            pos2(rect.left() + 14.0, rect.center().y),
            15.0,
            if danger { p.danger } else { p.muted },
        );
    }
    let text_x = if glyph.is_some() { 30.0 } else { 12.0 };
    ui.painter().text(
        pos2(rect.left() + text_x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        if danger { p.danger } else { p.text },
    );
    (resp, bg)
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

/// 带勾选标记的菜单项（勾在右侧）。单选列表里的选项不需要图标，传 `None`。
pub fn menu_check(ui: &mut Ui, glyph: Option<Icon>, label: &str, checked: bool) -> Response {
    let p = Palette::of(ui.ctx());
    let resp = menu_entry(ui, glyph, label, false);
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

/// 右键菜单顶部的标题行。`width` 大于 0 时同时定下菜单宽度（菜单项按可用宽度铺满，
/// egui 自带的 context_menu 不定宽会被撑开）；浮动菜单自己定宽，传 0。
pub fn menu_heading(ui: &mut Ui, text: &str, width: f32) {
    let p = Palette::of(ui.ctx());
    if width > 0.0 {
        ui.set_width(width);
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(12.0).color(p.muted));
    ui.add_space(2.0);
}

/// 固定在 `pos` 的右键菜单。与 `Response::context_menu` 不同，它不挂在某一行上，
/// 列表重排时不会跟着跳，内容也由调用方按记下的对象生成。
/// 底层是 egui 的菜单弹窗（`PopupKind::Menu`），所以里面可以直接用 [`submenu`] 做多级菜单；
/// 点击菜单项、点击菜单外或按 Esc 时由 egui 关闭。`add` 返回是否执行了某个操作。返回菜单是否继续显示。
pub fn floating_menu(
    ctx: &egui::Context,
    id: &str,
    pos: Pos2,
    width: f32,
    add: impl FnOnce(&mut Ui) -> bool,
) -> bool {
    let mut open = true;
    let mut acted = false;
    // id 带上位置：在别处再次右键时是一个新菜单，不会被"点击了菜单外"立即关掉。
    egui::Popup::new(
        egui::Id::new((id, pos.x.to_bits(), pos.y.to_bits())),
        ctx.clone(),
        egui::PopupAnchor::Position(pos),
        egui::LayerId::background(),
    )
    .kind(egui::PopupKind::Menu)
    .layout(egui::Layout::top_down_justified(egui::Align::Min))
    .style(egui::containers::menu::menu_style)
    .width(width)
    .open_bool(&mut open)
    .show(|ui| {
        // 菜单项按可用宽度铺满，这里必须定死宽度，否则弹窗会被撑到很宽。
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        acted = add(ui);
    });
    open && !acted
}

/// 二级菜单：入口与普通菜单项同样式，右侧有箭头，悬停时展开（egui 原生 `SubMenu`）。
pub fn submenu(
    ui: &mut Ui,
    glyph: Icon,
    label: &str,
    min_width: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    let p = Palette::of(ui.ctx());
    let (resp, bg) = menu_row(ui, Some(glyph), label, false);
    icon(
        ui,
        Icon::ChevronRight,
        pos2(resp.rect.right() - 12.0, resp.rect.center().y),
        14.0,
        p.muted,
    );
    let open = egui::containers::menu::SubMenu::new()
        .show(ui, &resp, |ui| {
            ui.set_width(min_width);
            ui.spacing_mut().item_spacing.y = 2.0;
            add_contents(ui);
        })
        .is_some();
    if open || resp.hovered() {
        ui.painter().set(
            bg,
            Shape::rect_filled(resp.rect, CornerRadius::same(6), p.hover),
        );
    }
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
