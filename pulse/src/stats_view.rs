//! 统计页：各程序的使用时长（日、周、月、全部），以及最近一年每天的使用量热力图。
//!
//! 图表沿用全应用的点阵语言：日视图每个点是 5 分钟，按那段时间在前台最久的程序着色；
//! 周、月、全部视图每列是一天或一个月，按程序分段堆叠。

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Ui,
    pos2, vec2,
};
use pulse_probe::{t, tf};

use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::perf::{Kpi, hero, section};
use crate::stats::{Column, Data, Date, Range, Store};
use crate::theme::{Palette, metric};
use crate::widgets;

/// 排名前几的程序各有自己的颜色，其余归为"其他"。
const COLORS: [Color32; 8] = [
    metric::CPU,
    metric::MEMORY,
    metric::GPU,
    metric::DISK,
    metric::NETWORK,
    metric::NETWORK_UP,
    Color32::from_rgb(0x84, 0xcc, 0x16),
    Color32::from_rgb(0xef, 0x44, 0x44),
];

pub struct StatsState {
    range: Range,
    anchor: Date,
    store: Option<Store>,
    data: Option<Data>,
    loaded: Option<(Range, Date, Instant)>,
    /// 上次按正在运行的进程补全程序路径的时刻。
    filled: Option<Instant>,
    error: Option<String>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            range: Range::Day,
            anchor: Date::today(),
            store: None,
            data: None,
            loaded: None,
            filled: None,
            error: None,
        }
    }
}

impl StatsState {
    /// 区间或日期变了就重新查询；看的是包含今天的区间时每 30 秒刷新一次。
    fn refresh(&mut self, procs: &[pulse_probe::Process]) {
        // 进程列表刚启动时还是空的，所以隔几秒再补一次路径。
        if let Some(store) = &mut self.store
            && !procs.is_empty()
            && self
                .filled
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            store.fill_paths(procs);
            self.filled = Some(Instant::now());
        }
        let stale = match self.loaded {
            Some((r, a, at)) => {
                r != self.range || a != self.anchor || at.elapsed() > Duration::from_secs(30)
            }
            None => true,
        };
        if !stale {
            return;
        }
        // 数据库要等记录线程写入第一分钟后才存在，没打开时每次刷新都再试。
        if !self.store.as_ref().is_some_and(Store::available) {
            self.store = Some(Store::open());
        }
        let store = self.store.as_mut().unwrap();
        match store.query(self.range, self.anchor) {
            Ok(d) => {
                self.data = d;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.loaded = Some((self.range, self.anchor, Instant::now()));
    }

    /// 让下一帧重新查询。
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }
}

fn range_label(r: Range) -> &'static str {
    match r {
        Range::Day => t!("日", "Day", "日"),
        Range::Week => t!("周", "Week", "週"),
        Range::Month => t!("月", "Month", "月"),
        Range::All => t!("全部", "All", "すべて"),
    }
}

/// 时长：`3 小时 12 分`，不到一分钟显示秒。
pub fn fmt_duration(secs: i64) -> String {
    let (h, m) = (secs / 3600, secs / 60 % 60);
    if h >= 100 {
        return tf!("{h} 小时", "{h}h", "{h} 時間");
    }
    match (h, m) {
        (0, 0) => tf!("{secs} 秒", "{secs}s", "{secs} 秒"),
        (0, m) => tf!("{m} 分钟", "{m}m", "{m} 分"),
        (h, 0) => tf!("{h} 小时", "{h}h", "{h} 時間"),
        (h, m) => tf!("{h} 小时 {m} 分", "{h}h {m}m", "{h} 時間 {m} 分"),
    }
}

fn weekday_label(i: u32) -> &'static str {
    match i {
        0 => t!("周一", "Mon", "月"),
        1 => t!("周二", "Tue", "火"),
        2 => t!("周三", "Wed", "水"),
        3 => t!("周四", "Thu", "木"),
        4 => t!("周五", "Fri", "金"),
        5 => t!("周六", "Sat", "土"),
        _ => t!("周日", "Sun", "日"),
    }
}

fn fmt_date(d: Date) -> String {
    tf!(
        "{}年{}月{}日",
        "{}-{:02}-{:02}",
        "{}年{}月{}日",
        d.y,
        d.m,
        d.d
    )
}

/// 当前区间的标题：`2026年10月13日 周二`、`10月12日 – 10月18日`、`2026年10月`。
fn range_title(range: Range, start: Date, end: Date) -> String {
    match range {
        Range::Day => format!("{} {}", fmt_date(start), weekday_label(start.weekday())),
        Range::Week => {
            let last = end.add_days(-1);
            tf!(
                "{}月{}日 – {}月{}日",
                "{:02}-{:02} – {:02}-{:02}",
                "{}月{}日 – {}月{}日",
                start.m,
                start.d,
                last.m,
                last.d
            )
        }
        Range::Month => tf!("{}年{}月", "{}-{:02}", "{}年{}月", start.y, start.m),
        Range::All => tf!(
            "{}年{}月起",
            "Since {}-{:02}",
            "{}年{}月から",
            start.y,
            start.m
        ),
    }
}

pub fn show(
    ui: &mut Ui,
    state: &mut StatsState,
    icons: &mut Icons,
    procs: &[pulse_probe::Process],
) {
    state.refresh(procs);
    let p = Palette::of(ui.ctx());
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);
            header(ui, state);
            if let Some(e) = &state.error {
                ui.add_space(12.0);
                ui.label(RichText::new(e).color(p.muted));
                return;
            }
            let Some(data) = state.data.clone() else {
                empty(ui);
                return;
            };
            if data.first.is_none() {
                empty(ui);
                return;
            }
            // 排名前几的程序与颜色的对应。
            let colors: Vec<i64> = data
                .totals
                .iter()
                .take(COLORS.len())
                .map(|t| t.app)
                .collect();
            let color_of = |app: i64| {
                colors
                    .iter()
                    .position(|&a| a == app)
                    .map_or(p.faint.gamma_multiply(0.55), |i| COLORS[i])
            };
            ui.add_space(12.0);
            kpis(ui, &data, state);
            match data.range {
                Range::Day => {
                    section(ui, t!("时间线", "Timeline", "タイムライン"));
                    day_chart(ui, &data, state, &color_of);
                }
                _ => {
                    section(ui, t!("使用时长", "Usage", "使用時間"));
                    if let Some(pick) = columns_chart(ui, &data, &color_of) {
                        // 点某一天看当天；全部视图里点某个月看那个月。
                        let (range, anchor) = match data.range {
                            Range::All => (Range::Month, pick),
                            _ => (Range::Day, pick),
                        };
                        state.range = range;
                        state.anchor = anchor;
                    }
                }
            }
            section(ui, t!("应用", "Apps", "アプリ"));
            ranking(ui, &data, state, icons, &color_of);
            section(ui, t!("最近一年", "Past year", "過去 1 年"));
            if let Some(day) = heatmap(ui, &data) {
                state.range = Range::Day;
                state.anchor = day;
            }
            ui.add_space(16.0);
        });
}

fn header(ui: &mut Ui, state: &mut StatsState) {
    let p = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        let labels = Range::ALL.map(range_label);
        let cur = Range::ALL
            .iter()
            .position(|&r| r == state.range)
            .unwrap_or(0);
        if let Some(i) = crate::settings_view::choice(ui, &labels, cur) {
            state.range = Range::ALL[i];
        }
        ui.add_space(12.0);
        let first = state.data.as_ref().and_then(|d| d.first);
        let (start, end) = state.range.bounds(state.anchor, first);
        if state.range != Range::All {
            if nav_button(ui, Icon::ChevronLeft).clicked() {
                state.anchor = state.range.step(state.anchor, -1);
            }
            let today = Date::today();
            let can_next = end <= today;
            let resp = nav_button(ui, Icon::ChevronRight);
            if resp.clicked() && can_next {
                state.anchor = state.range.step(state.anchor, 1);
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(range_title(state.range, start, end))
                .size(14.0)
                .color(p.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let today = Date::today();
            let showing_today = start <= today && today < end;
            if !showing_today
                && widgets::tool_button(ui, Icon::History, t!("今天", "Today", "今日"), true, false)
                    .clicked()
            {
                state.anchor = today;
            }
        });
    });
}

fn nav_button(ui: &mut Ui, glyph: Icon) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    widgets::icon(ui, glyph, rect.center(), 16.0, p.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn empty(ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
        widgets::icon(ui, Icon::History, r.center(), 28.0, p.muted);
        ui.add_space(8.0);
        ui.label(
            RichText::new(t!(
                "还没有使用记录",
                "No activity recorded yet",
                "まだ記録がありません"
            ))
            .size(15.0)
            .color(p.text),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(t!(
                "Pulse 在后台运行时会记录各程序在前台的时长。",
                "Pulse records how long each program is in the foreground while it runs.",
                "Pulse の実行中は、各プログラムが前面にあった時間を記録します。"
            ))
            .size(12.5)
            .color(p.muted),
        );
    });
}

fn kpis(ui: &mut Ui, data: &Data, state: &StatsState) {
    let active = data.active();
    let typing = data.typing();
    let mut items = vec![
        Kpi::new(
            t!("活跃时长", "Active time", "アクティブ時間"),
            fmt_duration(active),
            "",
            metric::CPU,
        ),
        Kpi::new(
            t!("打字", "Typing", "入力"),
            format!("{:.0}", typing as f64 / active.max(1) as f64 * 100.0),
            "%",
            metric::MEMORY,
        )
        .caption(fmt_duration(typing))
        .bar(typing as f32 / active.max(1) as f32),
    ];
    if let Some(top) = data.totals.first() {
        let name = state
            .store
            .as_ref()
            .and_then(|s| s.apps.get(&top.app))
            .map_or_else(String::new, |a| a.name.clone());
        items.push(
            Kpi::new(
                t!("最常用", "Most used", "最もよく使用"),
                fmt_duration(top.active),
                "",
                COLORS[0],
            )
            .caption(name),
        );
    }
    if data.range != Range::Day {
        items.push(
            Kpi::new(
                t!("日均", "Daily average", "1 日平均"),
                fmt_duration(active / data.days_with_data.max(1)),
                "",
                metric::GPU,
            )
            .caption(tf!(
                "{} 天有记录",
                "{} days recorded",
                "{} 日分の記録",
                data.days_with_data
            )),
        );
    }
    hero(ui, &items);
}

/// 点阵里的一个圆角方点。
fn dot(painter: &egui::Painter, center: Pos2, size: f32, color: Color32) {
    painter.rect_filled(
        Rect::from_center_size(center, vec2(size, size)),
        CornerRadius::same((size * 0.22).round() as u8),
        color,
    );
}

fn card(ui: &mut Ui, height: f32) -> (Rect, egui::Response) {
    let p = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(12), p.surface);
    (rect, resp)
}

/// 日视图：48 列（每列半小时）× 6 行（每行 5 分钟），每个点按那 5 分钟里在前台最久的程序着色，
/// 越接近整 5 分钟都在用越不透明。
fn day_chart(ui: &mut Ui, data: &Data, state: &StatsState, color_of: &dyn Fn(i64) -> Color32) {
    let p = Palette::of(ui.ctx());
    const COLS: usize = 48;
    const ROWS: usize = 6;
    let pad = 16.0;
    let width = ui.available_width() - pad * 2.0;
    let pitch = (width / COLS as f32).min(26.0);
    let size = pitch * 0.72;
    let height = pad + ROWS as f32 * pitch + 22.0 + pad / 2.0;
    let (rect, resp) = card(ui, height);
    let painter = ui.painter_at(rect);
    let left = rect.left() + pad + (width - pitch * COLS as f32) / 2.0;
    let top = rect.top() + pad;
    let mut hovered = None;
    for c in 0..COLS {
        for r in 0..ROWS {
            let slot = c * ROWS + r;
            let center = pos2(
                left + (c as f32 + 0.5) * pitch,
                top + (r as f32 + 0.5) * pitch,
            );
            let col = data.columns.get(slot);
            let total = col.map_or(0, Column::total);
            let color = match col.and_then(Column::top) {
                Some(app) if total > 0 => {
                    color_of(app).gamma_multiply(0.35 + 0.65 * (total as f32 / 300.0).min(1.0))
                }
                _ => p.track,
            };
            dot(&painter, center, size, color);
            if resp
                .hover_pos()
                .is_some_and(|h| (h - center).abs().max_elem() <= pitch / 2.0)
            {
                hovered = Some(slot);
            }
        }
        // 每 3 小时一个刻度。
        if c % 6 == 0 {
            painter.text(
                pos2(
                    left + c as f32 * pitch + pitch * 0.14,
                    top + ROWS as f32 * pitch + 6.0,
                ),
                Align2::LEFT_TOP,
                format!("{}:00", c / 2),
                FontId::proportional(11.0),
                p.faint,
            );
        }
    }
    if let Some(slot) = hovered
        && let Some(col) = data.columns.get(slot)
        && col.total() > 0
    {
        let start = slot as i64 * 5;
        let label = format!(
            "{:02}:{:02}–{:02}:{:02}",
            start / 60,
            start % 60,
            (start + 5) / 60,
            (start + 5) % 60
        );
        let apps = state.store.as_ref().map(|s| &s.apps);
        resp.on_hover_ui_at_pointer(|ui| {
            ui.label(RichText::new(label).strong());
            for (app, secs) in col.parts.iter().take(5) {
                let name = apps
                    .and_then(|a| a.get(app))
                    .map_or("?", |a| a.name.as_str());
                ui.label(format!("{name}  {}", fmt_duration(*secs)));
            }
        });
    }
}

/// 周、月、全部视图：每列一天（或一个月），从底部往上按程序堆叠，每个点代表固定的时长。
/// 点击某一列返回它的日期（全部视图里是那个月的第一天）。
fn columns_chart(ui: &mut Ui, data: &Data, color_of: &dyn Fn(i64) -> Color32) -> Option<Date> {
    let p = Palette::of(ui.ctx());
    let n = data.columns.len().max(1);
    let pad = 16.0;
    let width = ui.available_width() - pad * 2.0;
    // 列宽里放几列点：列少（一周）时每列宽一些。
    let pitch = 10.0_f32.max((width / n as f32 / 4.0).min(14.0));
    let per_col = ((width / n as f32 / pitch).floor() as usize)
        .saturating_sub(1)
        .max(1);
    let rows = 14usize;
    let size = pitch * 0.72;
    let max = data
        .columns
        .iter()
        .map(Column::total)
        .max()
        .unwrap_or(0)
        .max(1);
    // 每个点的时长取整到好读的值。
    let raw = max as f64 / (rows * per_col) as f64;
    let unit = [
        60,
        300,
        600,
        900,
        1800,
        3600,
        7200,
        3 * 3600,
        6 * 3600,
        12 * 3600,
        24 * 3600,
        48 * 3600,
        100 * 3600,
    ]
    .into_iter()
    .find(|&u| u as f64 >= raw)
    .unwrap_or(200 * 3600) as i64;
    let height = pad + rows as f32 * pitch + 22.0 + pad / 2.0 + 18.0;
    let (rect, resp) = card(ui, height);
    let resp = resp.interact(Sense::click());
    let painter = ui.painter_at(rect);
    let slot_w = width / n as f32;
    let top = rect.top() + pad;
    let base = top + rows as f32 * pitch;
    let mut picked = None;
    let mut hover_col = None;
    for (i, col) in data.columns.iter().enumerate() {
        let x0 = rect.left() + pad + i as f32 * slot_w + (slot_w - per_col as f32 * pitch) / 2.0;
        // 按程序顺序把秒数换成点：先排满最底下一行，再往上。
        let mut colors = Vec::new();
        let mut carry = 0i64;
        for (app, secs) in &col.parts {
            carry += secs;
            let want = (carry + unit / 2) / unit;
            while (colors.len() as i64) < want {
                colors.push(color_of(*app));
            }
        }
        for k in 0..rows * per_col {
            let (r, c) = (k / per_col, k % per_col);
            let center = pos2(
                x0 + (c as f32 + 0.5) * pitch,
                base - (r as f32 + 0.5) * pitch,
            );
            dot(
                &painter,
                center,
                size,
                colors.get(k).copied().unwrap_or(p.track),
            );
        }
        let col_rect = Rect::from_min_max(
            pos2(rect.left() + pad + i as f32 * slot_w, top),
            pos2(rect.left() + pad + (i + 1) as f32 * slot_w, base + 20.0),
        );
        if resp.hover_pos().is_some_and(|h| col_rect.contains(h)) {
            hover_col = Some(i);
        }
        let date = match data.range {
            Range::All => data.start.add_months(i as i32),
            _ => data.start.add_days(i as i64),
        };
        // 横轴标签：周视图每天、月视图每 5 天、全部视图每年一月（或首列）。
        let label = match data.range {
            Range::Week => Some(format!("{} {}", weekday_label(i as u32), date.d)),
            Range::Month => {
                (i == 0 || (date.d % 5 == 0 && date.d < 30)).then(|| date.d.to_string())
            }
            _ => (i == 0 || date.m == 1).then(|| date.y.to_string()),
        };
        if let Some(label) = label {
            painter.text(
                pos2(col_rect.center().x, base + 6.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(11.0),
                p.faint,
            );
        }
    }
    painter.text(
        pos2(rect.right() - pad, rect.bottom() - 10.0),
        Align2::RIGHT_BOTTOM,
        tf!(
            "每个点 {}",
            "Each dot is {}",
            "1 点 = {}",
            fmt_duration(unit)
        ),
        FontId::proportional(11.0),
        p.faint,
    );
    if let Some(i) = hover_col
        && let Some(col) = data.columns.get(i)
    {
        let date = match data.range {
            Range::All => data.start.add_months(i as i32),
            _ => data.start.add_days(i as i64),
        };
        let title = match data.range {
            Range::All => tf!("{}年{}月", "{}-{:02}", "{}年{}月", date.y, date.m),
            _ => format!("{} {}", fmt_date(date), weekday_label(date.weekday())),
        };
        resp.clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(fmt_duration(col.total()));
            });
    }
    if resp.clicked()
        && let Some(i) = hover_col
    {
        picked = Some(match data.range {
            Range::All => data.start.add_months(i as i32),
            _ => data.start.add_days(i as i64),
        });
    }
    picked
}

fn ranking(
    ui: &mut Ui,
    data: &Data,
    state: &StatsState,
    icons: &mut Icons,
    color_of: &dyn Fn(i64) -> Color32,
) {
    let p = Palette::of(ui.ctx());
    const SHOWN: usize = 30;
    let max = data.totals.first().map_or(1, |t| t.active).max(1);
    let row_h = 34.0;
    let n = data.totals.len().min(SHOWN);
    if n == 0 {
        let (rect, _) = card(ui, 56.0);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            t!(
                "这段时间没有记录",
                "Nothing recorded in this period",
                "この期間の記録はありません"
            ),
            FontId::proportional(12.5),
            p.muted,
        );
        return;
    }
    let (rect, _) = card(ui, n as f32 * row_h + 12.0);
    let apps = state.store.as_ref().map(|s| &s.apps);
    for (i, t) in data.totals.iter().take(SHOWN).enumerate() {
        let row = Rect::from_min_size(
            pos2(rect.left() + 16.0, rect.top() + 6.0 + i as f32 * row_h),
            vec2(rect.width() - 32.0, row_h),
        );
        let cy = row.center().y;
        let painter = ui.painter_at(rect);
        let app = apps.and_then(|a| a.get(&t.app));
        let name = app.map_or("?", |a| a.name.as_str());
        // 颜色点（只有前几名有自己的颜色）、图标、名称。
        dot(&painter, pos2(row.left() + 4.0, cy), 8.0, color_of(t.app));
        let icon_rect = Rect::from_center_size(pos2(row.left() + 26.0, cy), vec2(18.0, 18.0));
        match app
            .and_then(|a| a.path.as_ref())
            .and_then(|path| icons.get(path))
        {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    icon_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => widgets::avatar(&painter, icon_rect, name),
        }
        let name_w = (row.width() * 0.34).max(120.0);
        widgets::text_truncated(
            &painter,
            pos2(row.left() + 44.0, cy),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(13.0),
            p.text,
            name_w - 52.0,
        );
        // 右侧：时长、打字占比；中间是相对排名第一的长度条。
        let typing = tf!(
            "打字 {:.0}%",
            "Typing {:.0}%",
            "入力 {:.0}%",
            t.typing as f64 / t.active.max(1) as f64 * 100.0
        );
        painter.text(
            pos2(row.right(), cy),
            Align2::RIGHT_CENTER,
            typing,
            FontId::proportional(11.5),
            p.faint,
        );
        let dur_right = row.right() - 92.0;
        painter.text(
            pos2(dur_right, cy),
            Align2::RIGHT_CENTER,
            fmt_duration(t.active),
            FontId::proportional(12.5),
            p.text,
        );
        let bar = Rect::from_min_max(
            pos2(row.left() + name_w, cy - 3.0),
            pos2(dur_right - 110.0, cy + 3.0),
        );
        if bar.width() > 40.0 {
            widgets::meter(
                &painter,
                bar,
                t.active as f32 / max as f32,
                color_of(t.app),
                p.track,
            );
        }
    }
}

/// 最近 53 周的热力图：每列一周（周一在上），颜色深浅表示当天的活跃时长。点击某天返回它。
fn heatmap(ui: &mut Ui, data: &Data) -> Option<Date> {
    let p = Palette::of(ui.ctx());
    const WEEKS: usize = 53;
    let pad = 16.0;
    let label_w = 28.0;
    let width = ui.available_width() - pad * 2.0 - label_w;
    let pitch = (width / WEEKS as f32).min(24.0);
    let size = pitch * 0.74;
    let height = pad + 18.0 + 7.0 * pitch + pad;
    let (rect, resp) = card(ui, height);
    let resp = resp.interact(Sense::click());
    let painter = ui.painter_at(rect);
    let today = Date::today();
    // 最后一列是本周。
    let last_monday = today.add_days(-(today.weekday() as i64));
    let first_monday = last_monday.add_days(-7 * (WEEKS as i64 - 1));
    let left = rect.left() + pad + label_w;
    let top = rect.top() + pad + 18.0;
    // 颜色深浅按这一年里有记录的日子分四档（四分位），每天都差不多长时也看得出起伏。
    let mut values: Vec<i64> = data.heat.values().copied().filter(|&v| v > 0).collect();
    values.sort_unstable();
    let quartile = |q: usize| {
        values
            .get(values.len() * q / 4)
            .copied()
            .unwrap_or(i64::MAX)
    };
    let cuts = [quartile(1), quartile(2), quartile(3)];
    let mut hovered = None;
    for w in 0..WEEKS {
        let monday = first_monday.add_days(7 * w as i64);
        if w == 0 || monday.d <= 7 {
            painter.text(
                pos2(left + w as f32 * pitch, rect.top() + pad),
                Align2::LEFT_TOP,
                tf!("{}月", "{}", "{}月", month_name(monday.m)),
                FontId::proportional(11.0),
                p.faint,
            );
        }
        for d in 0..7 {
            let day = monday.add_days(d);
            if day > today {
                continue;
            }
            let center = pos2(
                left + (w as f32 + 0.5) * pitch,
                top + (d as f32 + 0.5) * pitch,
            );
            let secs = data.heat.get(&day.key()).copied().unwrap_or(0);
            let color = if secs <= 0 {
                p.track
            } else {
                let level = cuts.iter().filter(|&&c| secs >= c).count();
                metric::GPU.gamma_multiply([0.3, 0.5, 0.75, 1.0][level])
            };
            dot(&painter, center, size, color);
            if resp
                .hover_pos()
                .is_some_and(|h| (h - center).abs().max_elem() <= pitch / 2.0)
            {
                hovered = Some((day, secs));
            }
        }
    }
    for (d, label) in [
        (0, weekday_label(0)),
        (2, weekday_label(2)),
        (4, weekday_label(4)),
    ] {
        painter.text(
            pos2(rect.left() + pad, top + (d as f32 + 0.5) * pitch),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.5),
            p.faint,
        );
    }
    let (day, secs) = hovered?;
    let clicked = resp.clicked();
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_ui_at_pointer(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} {}",
                    fmt_date(day),
                    weekday_label(day.weekday())
                ))
                .strong(),
            );
            ui.label(fmt_duration(secs));
        });
    clicked.then_some(day)
}

fn month_name(m: u32) -> String {
    if pulse_probe::i18n::lang() == pulse_probe::i18n::Lang::En {
        [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ][(m as usize + 11) % 12]
            .to_string()
    } else {
        m.to_string()
    }
}
