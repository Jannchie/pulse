//! 统计页：各程序的使用时长（日、周、月、年、最近 N 天、自选区间、全部），以及最近一年每天的使用量热力图。
//!
//! 图表沿用全应用的点阵语言：日视图每个点是 5 分钟，按那段时间在前台最久的程序着色；
//! 周、月、全部视图每列是一天或一个月，按程序分段堆叠。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Ui,
    pos2, vec2,
};
use pulse_probe::{t, tf};

use crate::category::{Category, Classifier, Reason};
use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::perf::section;
use crate::stats::{Column, Data, Date, Grain, Range, Span, Store};
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
    /// 按分类筛选后的数据（没有筛选时与 `data` 相同）。
    view: Option<Arc<Data>>,
    classifier: Option<Classifier>,
    /// 各程序的分类与依据。
    cats: HashMap<i64, (Category, Reason)>,
    /// 只看某一类。
    filter: Option<Category>,
    /// 右键菜单：程序与位置。
    menu: Option<(i64, Pos2)>,
    loaded: Option<(Range, Date, Instant)>,
    /// 上次按正在运行的进程补全程序路径的时刻。
    filled: Option<Instant>,
    error: Option<String>,
    /// 日历：正在看的月份（1 日）与已点下的起始日期。
    cal_month: Date,
    picking: Option<Date>,
}

impl Default for StatsState {
    fn default() -> Self {
        Self {
            range: Range::Day,
            anchor: Date::today(),
            store: None,
            data: None,
            view: None,
            classifier: None,
            cats: HashMap::new(),
            filter: None,
            menu: None,
            loaded: None,
            filled: None,
            error: None,
            cal_month: Date::today().month_start(),
            picking: None,
        }
    }
}

impl StatsState {
    /// 区间或日期变了就重新查询；看的是包含今天的区间时每 30 秒刷新一次。
    fn refresh(&mut self, procs: &[pulse_probe::Process]) {
        let mut changed = match &mut self.classifier {
            Some(c) => c.rescan_if_stale(),
            None => {
                self.classifier = Some(Classifier::load());
                true
            }
        };
        // 进程列表刚启动时还是空的，所以隔几秒再补一次路径。
        if let Some(store) = &mut self.store
            && !procs.is_empty()
            && self
                .filled
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            store.fill_paths(procs);
            self.filled = Some(Instant::now());
            changed = true;
        }
        if changed {
            self.recategorize();
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
        self.recategorize();
    }

    /// 重新给各程序分类，并按当前筛选生成要显示的数据。
    fn recategorize(&mut self) {
        if let (Some(store), Some(c)) = (&self.store, &self.classifier) {
            self.cats = store
                .apps
                .iter()
                .map(|(&id, a)| (id, c.classify(&a.key, a.path.as_deref())))
                .collect();
        }
        self.view = self.data.as_ref().map(|d| {
            Arc::new(match self.filter {
                Some(f) => d.filtered(|app| self.category(app) == f),
                None => d.clone(),
            })
        });
    }

    fn category(&self, app: i64) -> Category {
        self.cats.get(&app).map_or(Category::Other, |c| c.0)
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
        Range::Year => t!("年", "Year", "年"),
        Range::All => t!("全部", "All", "すべて"),
        Range::Last24h => t!("24 小时", "24 hours", "24 時間"),
        Range::Last7 => t!("7 天", "7 days", "7 日間"),
        Range::Last30 => t!("30 天", "30 days", "30 日間"),
        Range::Last365 => t!("365 天", "365 days", "365 日間"),
        Range::Custom(..) => t!("自选", "Custom", "期間指定"),
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

fn fmt_month_day(d: Date) -> String {
    tf!("{}月{}日", "{:02}-{:02}", "{}月{}日", d.m, d.d)
}

/// 日期区间（两端都含）：同一年时省去后一个日期的年份，不是今年时前一个带上年份。
fn fmt_days(first: Date, last: Date) -> String {
    let this_year = Date::today().y;
    let a = if first.y == this_year && last.y == this_year {
        fmt_month_day(first)
    } else {
        fmt_date(first)
    };
    let b = if first.y == last.y {
        fmt_month_day(last)
    } else {
        fmt_date(last)
    };
    if first == last {
        a
    } else {
        format!("{a} – {b}")
    }
}

fn month_title(d: Date) -> String {
    tf!("{}年{}月", "{}-{:02}", "{}年{}月", d.y, d.m)
}

/// 当前区间的标题：`2026年10月13日 周二`、`10月12日 – 10月18日`、`2026年10月`、`过去 7 天 · …`。
fn range_title(range: Range, span: &Span) -> String {
    let last = span.end.add_days(-1);
    match range {
        Range::Day => format!(
            "{} {}",
            fmt_date(span.start),
            weekday_label(span.start.weekday())
        ),
        Range::Week | Range::Custom(..) => fmt_days(span.start, last),
        Range::Month => month_title(span.start),
        Range::Year => tf!("{}年", "{}", "{}年", span.start.y),
        Range::Last24h => t!("过去 24 小时", "Past 24 hours", "過去 24 時間").to_string(),
        Range::Last7 | Range::Last30 | Range::Last365 => {
            let n = span.len_days();
            format!(
                "{} · {}",
                tf!("过去 {n} 天", "Past {n} days", "過去 {n} 日間"),
                fmt_days(span.start, last)
            )
        }
        Range::All => tf!(
            "{}年{}月起",
            "Since {}-{:02}",
            "{}年{}月から",
            span.start.y,
            span.start.m
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
            let (Some(all), Some(data)) = (state.data.clone(), state.view.clone()) else {
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
            // 概览：第一格是全部（区间内的累计活跃时长），其后是各分类；点一格只看那一类。
            if let Some(f) = categories(ui, &all, state) {
                state.filter = f;
                state.recategorize();
            }
            if data.grain == Grain::FiveMin {
                section(ui, t!("时间线", "Timeline", "タイムライン"));
                day_chart(ui, &data, state, &color_of);
            } else {
                section(ui, t!("使用时长", "Usage", "使用時間"));
                if let Some(i) = columns_chart(ui, &data, &color_of) {
                    // 点某一列看那一天、那一周或那个月。
                    let b = data.buckets[i];
                    let (range, anchor) = match data.grain {
                        Grain::Month => (Range::Month, b),
                        Grain::Week => (Range::Custom(b, b.add_days(7).min(data.span.end)), b),
                        _ => (Range::Day, b),
                    };
                    state.range = range;
                    state.anchor = anchor;
                }
            }
            section(
                ui,
                state
                    .filter
                    .map_or(t!("应用", "Apps", "アプリ"), Category::label),
            );
            ranking(ui, &data, state, icons, &color_of);
            section(ui, t!("网络流量", "Network traffic", "ネットワーク通信量"));
            traffic(ui, &data, state, icons);
            app_menu(ui, state);
            section(ui, t!("最近一年", "Past year", "過去 1 年"));
            if let Some(day) = heatmap(ui, &data) {
                state.range = Range::Day;
                state.anchor = day;
            }
            ui.add_space(16.0);
        });
}

/// 页头第一行：自然区间（日、周、月、年、全部）、截止到现在的区间（24 小时 … 365 天）、日历；
/// 第二行：前后翻页、当前区间的标题、回到今天。
fn header(ui: &mut Ui, state: &mut StatsState) {
    let p = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        let pick = |ui: &mut Ui, list: &[Range], current: Range| {
            let labels: Vec<&str> = list.iter().map(|&r| range_label(r)).collect();
            let cur = list
                .iter()
                .position(|&r| r == current)
                .unwrap_or(usize::MAX);
            crate::settings_view::choice(ui, &labels, cur).map(|i| list[i])
        };
        if let Some(r) = pick(ui, &Range::PERIODS, state.range) {
            state.range = r;
        }
        ui.add_space(8.0);
        if let Some(r) = pick(ui, &Range::ROLLING, state.range) {
            state.range = r;
        }
        ui.add_space(8.0);
        calendar(ui, state);
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let first = state.data.as_ref().and_then(|d| d.first);
        let span = state.range.span(state.anchor, first);
        let today = Date::today();
        if state.range.steppable() {
            if nav_button(ui, Icon::ChevronLeft, true).clicked() {
                (state.range, state.anchor) = state.range.step(state.anchor, -1);
            }
            let can_next = span.end <= today;
            if nav_button(ui, Icon::ChevronRight, can_next).clicked() && can_next {
                (state.range, state.anchor) = state.range.step(state.anchor, 1);
            }
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new(range_title(state.range, &span))
                .size(14.0)
                .color(p.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let showing_today = span.start <= today && today < span.end;
            if state.range.steppable()
                && !showing_today
                && widgets::tool_button(ui, Icon::History, t!("今天", "Today", "今日"), true, false)
                    .clicked()
            {
                if matches!(state.range, Range::Custom(..)) {
                    state.range = Range::Day;
                }
                state.anchor = today;
            }
        });
    });
}

/// 日历按钮与弹出的月历：先点开始日期，再点结束日期（两端都含）；同一天点两次就是那一天。
fn calendar(ui: &mut Ui, state: &mut StatsState) {
    let p = Palette::of(ui.ctx());
    let button = widgets::tool_button(
        ui,
        Icon::Calendar,
        t!("选择日期", "Pick dates", "日付を選択"),
        true,
        false,
    );
    if button.clicked() {
        let first = state.data.as_ref().and_then(|d| d.first);
        let span = state.range.span(state.anchor, first);
        state.cal_month = span.end.add_days(-1).month_start();
        state.picking = None;
    }
    let popup = egui::Popup::from_toggle_button_response(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(6.0);
    let popup_id = popup.get_id();
    let today = Date::today();
    let mut done = None;
    popup.show(|ui| {
        const CELL: f32 = 34.0;
        ui.set_width(CELL * 7.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
        // 月份与翻页。
        ui.horizontal(|ui| {
            if nav_button(ui, Icon::ChevronLeft, true).clicked() {
                state.cal_month = state.cal_month.add_months(-1);
            }
            let (r, _) = ui.allocate_exact_size(vec2(CELL * 7.0 - 56.0, 28.0), Sense::hover());
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                month_title(state.cal_month),
                FontId::proportional(13.5),
                p.text,
            );
            let can_next = state.cal_month.add_months(1) <= today;
            if nav_button(ui, Icon::ChevronRight, can_next).clicked() && can_next {
                state.cal_month = state.cal_month.add_months(1);
            }
        });
        let (head, _) = ui.allocate_exact_size(vec2(CELL * 7.0, 20.0), Sense::hover());
        for i in 0..7 {
            ui.painter().text(
                pos2(head.left() + (i as f32 + 0.5) * CELL, head.center().y),
                Align2::CENTER_CENTER,
                weekday_short(i),
                FontId::proportional(11.0),
                p.faint,
            );
        }
        let first_cell = state
            .cal_month
            .add_days(-(state.cal_month.weekday() as i64));
        let (grid, resp) = ui.allocate_exact_size(vec2(CELL * 7.0, CELL * 6.0), Sense::click());
        let hover = resp.hover_pos().map(|h| {
            let c = ((h.x - grid.left()) / CELL).clamp(0.0, 6.0) as i64;
            let r = ((h.y - grid.top()) / CELL).clamp(0.0, 5.0) as i64;
            first_cell.add_days(r * 7 + c)
        });
        // 高亮：正在选时是起点到悬停处，否则是当前的自选区间。
        let selected = match (state.picking, hover, state.range) {
            (Some(a), Some(h), _) => Some((a.min(h), a.max(h))),
            (Some(a), None, _) => Some((a, a)),
            (None, _, Range::Custom(a, b)) => Some((a, b.add_days(-1))),
            _ => None,
        };
        for k in 0..42 {
            let day = first_cell.add_days(k);
            let cell = Rect::from_min_size(
                pos2(
                    grid.left() + (k % 7) as f32 * CELL,
                    grid.top() + (k / 7) as f32 * CELL,
                ),
                vec2(CELL, CELL),
            );
            let inside = day.m == state.cal_month.m;
            let future = day > today;
            let in_sel = selected.is_some_and(|(a, b)| a <= day && day <= b);
            let edge = selected.is_some_and(|(a, b)| day == a || day == b);
            let r = cell.shrink(2.0);
            if edge {
                ui.painter().rect_filled(r, CornerRadius::same(8), p.accent);
            } else if in_sel {
                ui.painter()
                    .rect_filled(r, CornerRadius::same(8), p.accent.gamma_multiply(0.22));
            } else if hover == Some(day) && !future {
                ui.painter().rect_filled(r, CornerRadius::same(8), p.hover);
            }
            if day == today && !edge {
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(8),
                    egui::Stroke::new(1.0, p.faint),
                    egui::StrokeKind::Inside,
                );
            }
            let color = if edge {
                Color32::WHITE
            } else if future {
                p.faint.gamma_multiply(0.5)
            } else if inside {
                p.text
            } else {
                p.faint
            };
            ui.painter().text(
                cell.center(),
                Align2::CENTER_CENTER,
                day.d.to_string(),
                FontId::proportional(12.5),
                color,
            );
        }
        if resp.clicked()
            && let Some(day) = hover
            && day <= today
        {
            match state.picking.take() {
                None => state.picking = Some(day),
                Some(a) => done = Some((a.min(day), a.max(day))),
            }
        }
        let _ = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.label(
            RichText::new(if state.picking.is_some() {
                t!("再点结束日期", "Now pick the end date", "終了日を選択")
            } else {
                t!("点开始日期", "Pick the start date", "開始日を選択")
            })
            .size(11.5)
            .color(p.muted),
        );
    });
    if let Some((a, b)) = done {
        if a == b {
            state.range = Range::Day;
            state.anchor = a;
        } else {
            state.range = Range::Custom(a, b.add_days(1));
            state.anchor = a;
        }
        egui::Popup::close_id(ui.ctx(), popup_id);
    }
}

fn weekday_short(i: u32) -> &'static str {
    match i {
        0 => t!("一", "Mo", "月"),
        1 => t!("二", "Tu", "火"),
        2 => t!("三", "We", "水"),
        3 => t!("四", "Th", "木"),
        4 => t!("五", "Fr", "金"),
        5 => t!("六", "Sa", "土"),
        _ => t!("日", "Su", "日"),
    }
}

fn nav_button(ui: &mut Ui, glyph: Icon, enabled: bool) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), sense);
    if enabled && resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    let color = if enabled { p.text } else { p.faint };
    widgets::icon(ui, glyph, rect.center(), 16.0, color);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
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

/// 日视图与最近 24 小时：48 列（每列半小时）× 6 行（每行 5 分钟），每个点按那 5 分钟里
/// 在前台最久的程序着色，越接近整 5 分钟都在用越不透明。
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
    let first_minute = data.span.minute_bounds().0;
    // 某分钟的本地时、分。
    let clock = |minute: i64| {
        pulse_probe::os::local_time(minute * 60).map_or((0, 0), |(_, _, _, h, m)| (h, m))
    };
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
        // 每 3 小时（本地时间的 0、3、6 … 点）一个刻度，最近 24 小时的列不一定从整点开始。
        let col_start = first_minute + c as i64 * 30;
        let (h, m) = clock(col_start);
        let to_hour = (60 - m as i64) % 60;
        let hour = (h + u32::from(m > 0)) % 24;
        if to_hour < 30 && hour % 3 == 0 {
            painter.text(
                pos2(
                    left + (c as f32 + to_hour as f32 / 30.0) * pitch + pitch * 0.14,
                    top + ROWS as f32 * pitch + 6.0,
                ),
                Align2::LEFT_TOP,
                format!("{hour}:00"),
                FontId::proportional(11.0),
                p.faint,
            );
        }
    }
    if let Some(slot) = hovered
        && let Some(col) = data.columns.get(slot)
        && col.total() > 0
    {
        let start = first_minute + slot as i64 * 5;
        let ((h0, m0), (h1, m1)) = (clock(start), clock(start + 5));
        let label = format!("{h0:02}:{m0:02}–{h1:02}:{m1:02}");
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

/// 按天以上的粒度：每列一天、一周或一个月，从底部往上按程序堆叠，每个点代表固定的时长。
/// 点击某一列返回它的序号。
fn columns_chart(ui: &mut Ui, data: &Data, color_of: &dyn Fn(i64) -> Color32) -> Option<usize> {
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
        let date = data.buckets[i];
        let label = axis_label(data.grain, n, i, date);
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
        let date = data.buckets[i];
        let title = match data.grain {
            Grain::Month => month_title(date),
            Grain::Week => fmt_days(date, date.add_days(6).min(data.span.end.add_days(-1))),
            _ => format!("{} {}", fmt_date(date), weekday_label(date.weekday())),
        };
        resp.clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(fmt_duration(col.total()));
            });
    }
    if resp.clicked() {
        picked = hover_col;
    }
    picked
}

/// 横轴标签：一周以内每天写星期；按天时每 5（或 10）天写日期，每月 1 日写月份；
/// 按周时在跨进新月份的那一列写月份；按月时一年以内每月都写，更长时只在一月写年份。
fn axis_label(grain: Grain, n: usize, i: usize, date: Date) -> Option<String> {
    match grain {
        Grain::Day if n <= 7 => Some(format!("{} {}", weekday_label(date.weekday()), date.d)),
        Grain::Day => {
            let every = if n <= 31 { 5 } else { 10 };
            if i == 0 || date.d == 1 {
                Some(format!("{}/{}", date.m, date.d))
            } else {
                (date.d.is_multiple_of(every) && date.d < 30).then(|| date.d.to_string())
            }
        }
        Grain::Week => {
            let next = date.add_days(6);
            (i == 0 || date.d == 1 || next.m != date.m).then(|| {
                let m = if date.d == 1 { date } else { next };
                if m.m == 1 {
                    m.y.to_string()
                } else {
                    tf!("{}月", "{}", "{}月", month_name(m.m))
                }
            })
        }
        Grain::Month if n <= 13 => Some(if date.m == 1 && i > 0 {
            date.y.to_string()
        } else {
            tf!("{}月", "{}", "{}月", month_name(date.m))
        }),
        _ => (i == 0 || date.m == 1).then(|| date.y.to_string()),
    }
}

fn ranking(
    ui: &mut Ui,
    data: &Data,
    state: &mut StatsState,
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
    let mut menu = None;
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
        // 名称列右端是分类按钮：点它（或右键整行）改分类。
        let (cat, reason) = state
            .cats
            .get(&t.app)
            .copied()
            .unwrap_or((Category::Other, Reason::Unknown));
        let tag_font = FontId::proportional(11.5);
        let tag_text =
            ui.painter()
                .layout_no_wrap(cat.label().to_string(), tag_font.clone(), p.muted);
        let tag = Rect::from_min_max(
            pos2(
                row.left() + name_w - 16.0 - tag_text.size().x.min(90.0) - 24.0,
                cy - 11.0,
            ),
            pos2(row.left() + name_w - 8.0, cy + 11.0),
        );
        let tag_w = tag.width() + 8.0;
        widgets::text_truncated(
            &painter,
            pos2(row.left() + 44.0, cy),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(13.0),
            p.text,
            name_w - 60.0 - tag_w,
        );
        // 悬停看分类依据，右键改分类。
        let resp = ui.interact(row, ui.id().with(("stats-app", t.app)), Sense::click());
        if resp.secondary_clicked() {
            menu = resp.interact_pointer_pos().map(|pos| (t.app, pos));
        }
        // 后注册的分类按钮在整行之上，点击归它。
        let tag_resp = ui
            .interact(tag, ui.id().with(("stats-tag", t.app)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let open = state.menu.is_some_and(|m| m.0 == t.app);
        if tag_resp.hovered() || open {
            painter.rect_filled(tag, CornerRadius::same(6), p.hover);
        }
        painter.rect_stroke(
            tag,
            CornerRadius::same(6),
            egui::Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        dot(&painter, pos2(tag.left() + 10.0, cy), 6.0, cat.color());
        widgets::text_truncated(
            &painter,
            pos2(tag.left() + 18.0, cy),
            Align2::LEFT_CENTER,
            cat.label(),
            tag_font,
            p.muted,
            90.0,
        );
        if tag_resp.clicked() {
            menu = Some((t.app, tag.left_bottom() + vec2(0.0, 4.0)));
        }
        resp.on_hover_ui(|ui| {
            ui.label(RichText::new(name).strong());
            ui.label(format!("{} · {}", cat.label(), reason.label()));
            ui.label(
                RichText::new(t!(
                    "点分类或右键可更改",
                    "Click the category or right-click to change it",
                    "カテゴリをクリックまたは右クリックで変更"
                ))
                .color(p.muted),
            );
        });
        // 右侧：时长、输入占比；中间是相对排名第一的长度条。
        let typing = tf!(
            "输入 {:.0}%",
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
    if menu.is_some() {
        state.menu = menu;
    }
}

/// 各程序在区间里收发的数据量（下载、上传），按合计排序。
fn traffic(ui: &mut Ui, data: &Data, state: &mut StatsState, icons: &mut Icons) {
    let p = Palette::of(ui.ctx());
    const SHOWN: usize = 12;
    let row_h = 34.0;
    let n = data.traffic.len().min(SHOWN);
    if n == 0 {
        let (rect, _) = card(ui, 56.0);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            t!(
                "这段时间没有记录到流量。逐进程的网络统计需要以管理员身份运行 Pulse。",
                "No traffic recorded in this period. Per-process network stats require running Pulse as administrator.",
                "この期間の通信量は記録されていません。プロセスごとのネットワーク統計には、Pulse を管理者として実行する必要があります。"
            ),
            FontId::proportional(12.5),
            p.muted,
        );
        return;
    }
    let max = data.traffic.first().map_or(1, |t| t.total()).max(1);
    let (rect, _) = card(ui, n as f32 * row_h + 12.0);
    let apps = state.store.as_ref().map(|s| &s.apps);
    let mut menu = None;
    for (i, t) in data.traffic.iter().take(SHOWN).enumerate() {
        let row = Rect::from_min_size(
            pos2(rect.left() + 16.0, rect.top() + 6.0 + i as f32 * row_h),
            vec2(rect.width() - 32.0, row_h),
        );
        let cy = row.center().y;
        let painter = ui.painter_at(rect);
        let app = apps.and_then(|a| a.get(&t.app));
        let name = app.map_or("?", |a| a.name.as_str());
        let icon_rect = Rect::from_center_size(pos2(row.left() + 13.0, cy), vec2(18.0, 18.0));
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
            pos2(row.left() + 32.0, cy),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(13.0),
            p.text,
            name_w - 44.0,
        );
        // 右侧两列：下载、上传；中间是相对第一名的长度条。
        let col = 92.0;
        for (j, (glyph, bytes, color)) in [
            (Icon::ArrowUp, t.tx, metric::NETWORK_UP),
            (Icon::ArrowDown, t.rx, metric::NETWORK),
        ]
        .into_iter()
        .enumerate()
        {
            let right = row.right() - j as f32 * col;
            let r = painter.text(
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                crate::fmt_bytes(bytes.max(0) as u64),
                FontId::proportional(12.5),
                if j == 1 { p.text } else { p.muted },
            );
            widgets::icon(ui, glyph, pos2(r.left() - 9.0, cy), 12.0, color);
        }
        let bar = Rect::from_min_max(
            pos2(row.left() + name_w, cy - 3.0),
            pos2(row.right() - 2.0 * col - 24.0, cy + 3.0),
        );
        if bar.width() > 40.0 {
            widgets::meter(
                &painter,
                bar,
                t.total() as f32 / max as f32,
                metric::NETWORK,
                p.track,
            );
        }
        let resp = ui.interact(row, ui.id().with(("stats-net", t.app)), Sense::click());
        if resp.secondary_clicked() {
            menu = resp.interact_pointer_pos().map(|pos| (t.app, pos));
        }
        let cat = state.category(t.app);
        resp.on_hover_ui(|ui| {
            ui.label(RichText::new(name).strong());
            ui.label(tf!(
                "下载 {} · 上传 {}",
                "Downloaded {} · Uploaded {}",
                "ダウンロード {} · アップロード {}",
                crate::fmt_bytes(t.rx.max(0) as u64),
                crate::fmt_bytes(t.tx.max(0) as u64)
            ));
            ui.label(RichText::new(cat.label()).color(p.muted));
        });
    }
    if menu.is_some() {
        state.menu = menu;
    }
}

/// 程序的右键菜单：手动指定分类，或恢复自动识别。
fn app_menu(ui: &mut Ui, state: &mut StatsState) {
    let Some((app, pos)) = state.menu else {
        return;
    };
    let Some(info) = state.store.as_ref().and_then(|s| s.apps.get(&app)).cloned() else {
        state.menu = None;
        return;
    };
    let current = state.cats.get(&app).copied();
    let manual = current.is_some_and(|c| c.1 == Reason::User);
    let mut pick: Option<Option<Category>> = None;
    let open = widgets::floating_menu(ui.ctx(), "stats-app-menu", pos, 220.0, |ui| {
        widgets::menu_heading(ui, &info.name, 0.0);
        for c in Category::ALL {
            let checked = manual && current.map(|x| x.0) == Some(c);
            if widgets::menu_check(ui, None, c.label(), checked).clicked() {
                pick = Some(Some(c));
            }
        }
        ui.separator();
        let auto = match current {
            Some((c, r)) if r != Reason::User => {
                tf!("自动识别（{}）", "Automatic ({})", "自動 ({})", c.label())
            }
            _ => t!("自动识别", "Automatic", "自動").to_string(),
        };
        if widgets::menu_check(ui, None, &auto, !manual).clicked() {
            pick = Some(None);
        }
        pick.is_some()
    });
    if let Some(c) = pick
        && let Some(cl) = &mut state.classifier
    {
        cl.set(&info.key, c);
        state.recategorize();
    }
    if !open {
        state.menu = None;
    }
}

/// 概览：上面一排点按各分类的时长比例着色；下面第一格是全部（累计活跃时长，附日均或输入占比），
/// 其后每类一格。点某一格返回要筛选的分类（全部是 `None`）。
fn categories(ui: &mut Ui, data: &Data, state: &StatsState) -> Option<Option<Category>> {
    let p = Palette::of(ui.ctx());
    let mut sums: HashMap<Category, i64> = HashMap::new();
    for t in &data.totals {
        *sums.entry(state.category(t.app)).or_default() += t.active;
    }
    let total: i64 = sums.values().sum::<i64>().max(1);
    let mut list: Vec<(Category, i64)> = Category::ALL
        .into_iter()
        .filter_map(|c| sums.get(&c).filter(|&&s| s > 0).map(|&s| (c, s)))
        .collect();
    // 未分类总在最后。
    list.sort_by_key(|&(c, s)| (c == Category::Other, std::cmp::Reverse(s)));
    if list.is_empty() {
        return None;
    }
    let active = data.active();
    // 全部这一格的附注：按天以上的区间写日均，一天以内写输入占比。
    let note = if data.grain == Grain::FiveMin {
        tf!(
            "输入 {}",
            "Typing {}",
            "入力 {}",
            pct_label(data.typing() as f64 / active.max(1) as f64 * 100.0)
        )
    } else {
        tf!(
            "日均 {}",
            "{} / day",
            "1 日平均 {}",
            fmt_duration(active / data.days_with_data.max(1))
        )
    };
    let tiles: Vec<(Option<Category>, i64)> = std::iter::once((None, active))
        .chain(list.iter().map(|&(c, s)| (Some(c), s)))
        .collect();
    let pad = 16.0;
    let gap = 8.0;
    let width = ui.available_width() - pad * 2.0;
    let cols = ((width + gap) / (170.0 + gap))
        .floor()
        .clamp(1.0, tiles.len() as f32) as usize;
    let cell_w = (width - gap * (cols - 1) as f32) / cols as f32;
    let cell_h = 48.0;
    let rows = tiles.len().div_ceil(cols);
    let pitch = 9.0;
    let height = pad + pitch + 14.0 + rows as f32 * (cell_h + gap) - gap + pad;
    let (rect, _) = card(ui, height);
    let painter = ui.painter_at(rect);
    // 比例条：一排方点按各类时长分配（最大余数法，点数之和不变）。
    let n = (width / pitch).floor().max(1.0) as usize;
    let mut counts: Vec<(usize, f64)> = list
        .iter()
        .map(|&(_, s)| {
            let exact = s as f64 / total as f64 * n as f64;
            (exact.floor() as usize, exact.fract())
        })
        .collect();
    let mut left = n.saturating_sub(counts.iter().map(|c| c.0).sum::<usize>());
    let mut order: Vec<usize> = (0..counts.len()).collect();
    order.sort_by(|&a, &b| counts[b].1.total_cmp(&counts[a].1));
    for i in order {
        if left == 0 {
            break;
        }
        counts[i].0 += 1;
        left -= 1;
    }
    let x0 = rect.left() + pad + (width - n as f32 * pitch) / 2.0;
    let mut k = 0;
    for (i, &(c, _)) in list.iter().enumerate() {
        let dim = state.filter.is_some_and(|f| f != c);
        let color = if dim {
            c.color().gamma_multiply(0.3)
        } else {
            c.color()
        };
        for _ in 0..counts[i].0 {
            dot(
                &painter,
                pos2(
                    x0 + (k as f32 + 0.5) * pitch,
                    rect.top() + pad + pitch / 2.0,
                ),
                pitch * 0.72,
                color,
            );
            k += 1;
        }
    }
    let mut picked = None;
    let top = rect.top() + pad + pitch + 14.0;
    for (i, &(c, secs)) in tiles.iter().enumerate() {
        let (r, col) = (i / cols, i % cols);
        let cell = Rect::from_min_size(
            pos2(
                rect.left() + pad + col as f32 * (cell_w + gap),
                top + r as f32 * (cell_h + gap),
            ),
            vec2(cell_w, cell_h),
        );
        let resp = ui.interact(cell, ui.id().with(("stats-cat", c)), Sense::click());
        let selected = state.filter == c;
        let color = c.map_or(p.muted, Category::color);
        let bg = if selected {
            c.map_or(p.hover, |c| c.color().gamma_multiply(0.16))
        } else if resp.hovered() {
            p.hover
        } else {
            p.bg
        };
        painter.rect_filled(cell, CornerRadius::same(8), bg);
        if selected {
            painter.rect_stroke(
                cell,
                CornerRadius::same(8),
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
        let cy = cell.center().y;
        let label = match c {
            Some(c) => {
                // 分类：右侧写占比。
                let share = painter.text(
                    pos2(cell.right() - 12.0, cy),
                    Align2::RIGHT_CENTER,
                    pct_label(secs as f64 / total as f64 * 100.0),
                    FontId::proportional(11.5),
                    p.faint,
                );
                dot(&painter, pos2(cell.left() + 16.0, cy), 9.0, color);
                (c.label().to_string(), share.left() - cell.left() - 38.0)
            }
            None => {
                // 全部：四个小点拼成的方块，附注写在标题后面。
                for (dx, dy) in [(-2.5, -2.5), (2.5, -2.5), (-2.5, 2.5), (2.5, 2.5)] {
                    dot(&painter, pos2(cell.left() + 16.0 + dx, cy + dy), 4.0, color);
                }
                (
                    format!("{} · {note}", t!("全部", "All", "すべて")),
                    cell.width() - 42.0,
                )
            }
        };
        widgets::text_truncated(
            &painter,
            pos2(cell.left() + 30.0, cy - 8.0),
            Align2::LEFT_CENTER,
            &label.0,
            FontId::proportional(12.0),
            p.muted,
            label.1,
        );
        widgets::text_truncated(
            &painter,
            pos2(cell.left() + 30.0, cy + 9.0),
            Align2::LEFT_CENTER,
            &fmt_duration(secs),
            FontId::proportional(13.5),
            p.text,
            label.1,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            picked = Some(c);
        }
    }
    picked
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
    let heat = data.heat_by_day();
    let mut values: Vec<i64> = heat.values().copied().filter(|&v| v > 0).collect();
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
            let secs = heat.get(&day.key()).copied().unwrap_or(0);
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

/// 占比：不到 1% 时写 "<1%"，不写成 0%。
fn pct_label(pct: f64) -> String {
    if pct > 0.0 && pct < 1.0 {
        "<1%".into()
    } else {
        format!("{pct:.0}%")
    }
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
