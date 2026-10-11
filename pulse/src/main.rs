#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod dialogs;
mod gpu_panel;
mod icons;
mod icons_gen;
mod model;
mod perf;
mod theme;
mod widgets;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, Frame, Key, Layout, Margin, Rect, RichText, Sense, Stroke, Ui, UiBuilder,
    pos2, vec2,
};
use egui_extras::{Column, TableBuilder};
use pulse_probe::ports::Proto;
use pulse_probe::{Collector, NetStatus, Snapshot};

use dialogs::{Decision, KillRequest, Toast, ToastKind};
use gpu_panel::GpuHistory;
use icons::Icons;
use icons_gen::Icon;
use model::{Metrics, Query, Row, SortKey, Table};
use perf::{Device, Series};
use theme::{Palette, metric};

const INTERVAL: Duration = Duration::from_secs(1);
const HISTORY: usize = 60;
const ROW_HEIGHT: f32 = 30.0;
const ICON: f32 = 16.0;
/// 分组模式下为展开箭头预留的宽度，也是成员行的缩进。
const INDENT: f32 = 16.0;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pulse")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([960.0, 480.0]),
        // NVIDIA 等驱动的 OpenGL vsync 在 SwapBuffers 里忙等，鼠标划过表格时能吃满一个核。
        // 窗口由 DWM 合成本就不会撕裂，所以直接关掉。重绘只由输入事件触发（见 theme 里关掉的动画），
        // 不会空转；在 UI 线程里睡眠限帧反而会让驱动线程忙等，实测更耗 CPU。
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Pulse",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
    gpu: VecDeque<f32>,
    /// 原始字节/秒，绘制时按窗口内最大值缩放。
    net_rx: VecDeque<f32>,
    net_tx: VecDeque<f32>,
    /// 每块 GPU 的利用率、各引擎与显存历史。
    gpu_detail: Vec<GpuHistory>,
    /// 性能页的各设备曲线，键如 "cpu.3"、"disk.2.read"、"net.11.rx"。
    series: HashMap<String, Vec<f32>>,
}

/// 采集线程写、UI 线程读。
#[derive(Default)]
struct Shared {
    snapshot: Arc<Snapshot>,
    seq: u64,
    history: History,
}

fn spawn_collector(ctx: egui::Context) -> Arc<Mutex<Shared>> {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let out = shared.clone();
    std::thread::Builder::new()
        .name("collector".into())
        .spawn(move || {
            let mut collector = Collector::new();
            // 第一次采样只建立基线，立刻再采一次让速率列尽快有值。
            collector.sample();
            std::thread::sleep(Duration::from_millis(250));
            loop {
                let started = Instant::now();
                let snap = collector.sample();
                {
                    let mut s = out.lock().unwrap();
                    let h = &mut s.history;
                    push_capped(&mut h.cpu, snap.cpu_total / 100.0);
                    push_capped(
                        &mut h.memory,
                        snap.mem_used as f32 / snap.mem_total.max(1) as f32,
                    );
                    push_capped(&mut h.gpu, snap.gpu_total / 100.0);
                    push_capped(&mut h.net_rx, snap.net_rx as f32);
                    push_capped(&mut h.net_tx, snap.net_tx as f32);
                    let mut put = |k: String, v: f32| push_vec(h.series.entry(k).or_default(), v);
                    put("cpu".into(), snap.cpu_total / 100.0);
                    for (i, u) in snap.cpu_cores.iter().enumerate() {
                        put(format!("cpu.{i}"), u / 100.0);
                    }
                    put(
                        "mem".into(),
                        snap.memory.in_use() as f32 / snap.memory.total.max(1) as f32,
                    );
                    for d in &snap.disks {
                        put(format!("disk.{}.active", d.index), d.active / 100.0);
                        put(format!("disk.{}.read", d.index), d.read_rate as f32);
                        put(format!("disk.{}.write", d.index), d.write_rate as f32);
                    }
                    for n in &snap.nics {
                        put(format!("net.{}.rx", n.index), n.rx_rate as f32);
                        put(format!("net.{}.tx", n.index), n.tx_rate as f32);
                    }
                    for (i, g) in snap.gpus.iter().enumerate() {
                        put(format!("gpu.{i}"), g.usage / 100.0);
                    }
                    h.gpu_detail.resize_with(snap.gpus.len(), Default::default);
                    for (hist, g) in h.gpu_detail.iter_mut().zip(&snap.gpus) {
                        push_vec(&mut hist.usage, g.usage / 100.0);
                        hist.engines.resize_with(g.engines.len(), Vec::new);
                        for (e, (_, u)) in hist.engines.iter_mut().zip(&g.engines) {
                            push_vec(e, u / 100.0);
                        }
                        push_vec(
                            &mut hist.dedicated,
                            g.dedicated_used as f32 / g.dedicated_total.max(1) as f32,
                        );
                        push_vec(
                            &mut hist.shared,
                            g.shared_used as f32 / g.shared_total.max(1) as f32,
                        );
                    }
                    s.snapshot = Arc::new(snap);
                    s.seq += 1;
                }
                ctx.request_repaint();
                std::thread::sleep(INTERVAL.saturating_sub(started.elapsed()));
            }
        })
        .expect("spawn collector");
    shared
}

fn push_vec(v: &mut Vec<f32>, x: f32) {
    if v.len() == HISTORY {
        v.remove(0);
    }
    v.push(x);
}

fn push_capped(q: &mut VecDeque<f32>, v: f32) {
    if q.len() == HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Processes,
    Performance,
}

#[derive(Clone, PartialEq)]
enum Selection {
    Process(u32),
    Group(String),
}

enum Action {
    Select(Selection),
    Toggle(String),
    Kill { pids: Vec<u32>, label: String },
    Reveal(Arc<str>),
    Copy(String),
    Sort(SortKey),
}

struct App {
    shared: Arc<Mutex<Shared>>,
    snapshot: Arc<Snapshot>,
    history: [Vec<f32>; 5],
    gpu_history: Vec<GpuHistory>,
    series: Series,
    view: View,
    device: Device,
    seen_seq: u64,
    table: Table,
    dirty: bool,
    sort: SortKey,
    descending: bool,
    filter: String,
    grouped: bool,
    listening_only: bool,
    expanded: HashSet<String>,
    selected: Option<Selection>,
    icons: Icons,
    /// 等待用户确认的结束请求。
    pending_kill: Option<KillRequest>,
    toasts: Vec<Toast>,
    /// 最近一秒内的重绘时刻，用来确认空闲时没有空转。
    passes: VecDeque<Instant>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        egui_extras::install_image_loaders(ctx);
        Self {
            shared: spawn_collector(ctx.clone()),
            snapshot: Arc::default(),
            history: Default::default(),
            gpu_history: Vec::new(),
            series: HashMap::new(),
            view: View::Processes,
            device: Device::Cpu,
            seen_seq: 0,
            table: Table::default(),
            dirty: true,
            sort: SortKey::Cpu,
            descending: true,
            // 命令行参数作为初始搜索词，例如 `pulse :5173` 直接定位占用端口的进程。
            filter: std::env::args().skip(1).collect::<Vec<_>>().join(" "),
            grouped: true,
            listening_only: false,
            expanded: HashSet::new(),
            selected: None,
            icons: Icons::new(ctx),
            pending_kill: None,
            toasts: Vec::new(),
            passes: VecDeque::new(),
        }
    }

    fn pull(&mut self) {
        let s = self.shared.lock().unwrap();
        if s.seq == self.seen_seq {
            return;
        }
        self.seen_seq = s.seq;
        self.snapshot = s.snapshot.clone();
        let h = &s.history;
        for (dst, src) in self
            .history
            .iter_mut()
            .zip([&h.cpu, &h.memory, &h.gpu, &h.net_rx, &h.net_tx])
        {
            dst.clear();
            dst.extend(src);
        }
        self.gpu_history.clone_from(&h.gpu_detail);
        self.series.clone_from(&h.series);
        self.dirty = true;
    }

    fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.descending = !self.descending;
        } else {
            self.sort = key;
            self.descending = key.default_descending();
        }
        self.dirty = true;
    }

    fn selected_pids(&self) -> Option<(Vec<u32>, String)> {
        let procs = &self.snapshot.processes;
        match self.selected.as_ref()? {
            Selection::Process(pid) => {
                let p = procs.iter().find(|p| p.pid == *pid)?;
                Some((vec![p.pid], format!("{} ({})", p.name, p.pid)))
            }
            Selection::Group(key) => {
                let pids: Vec<u32> = procs
                    .iter()
                    .filter(|p| p.name.to_lowercase() == *key)
                    .map(|p| p.pid)
                    .collect();
                let name = procs
                    .iter()
                    .find(|p| p.name.to_lowercase() == *key)?
                    .name
                    .clone();
                Some((pids.clone(), format!("{} 的 {} 个进程", name, pids.len())))
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Select(s) => self.selected = Some(s),
            Action::Toggle(key) => {
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
                self.dirty = true;
            }
            // 结束进程一律先弹确认框，真正执行在 `kill_confirmed`。
            Action::Kill { pids, label } => self.pending_kill = Some(KillRequest { pids, label }),
            Action::Reveal(path) => pulse_probe::reveal_in_explorer(&path),
            Action::Copy(text) => ctx.copy_text(text),
            Action::Sort(key) => self.set_sort(key),
        }
    }

    fn kill_confirmed(&mut self, req: KillRequest) {
        // E_ACCESSDENIED
        const ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;
        let mut failures = Vec::new();
        for &pid in &req.pids {
            if let Err(e) = pulse_probe::kill(pid) {
                failures.push(e);
            }
        }
        let denied = failures.iter().any(|e| e.code().0 == ACCESS_DENIED);
        let reason = failures.first().map(|e| {
            let msg = e.message().trim().to_string();
            let msg = if msg.is_empty() {
                format!("错误 0x{:08X}", e.code().0)
            } else {
                msg
            };
            if denied && !self.snapshot.elevated {
                format!("{msg}。以管理员身份重启 Pulse 后再试。")
            } else {
                msg
            }
        });
        let toast = match failures.len() {
            0 => Toast {
                kind: ToastKind::Success,
                title: format!("已结束 {}", req.label),
                detail: None,
                at: Instant::now(),
            },
            n if n == req.pids.len() => Toast {
                kind: ToastKind::Error,
                title: format!("无法结束 {}", req.label),
                detail: reason,
                at: Instant::now(),
            },
            n => Toast {
                kind: ToastKind::Error,
                title: format!("{} 中有 {n} 个未能结束", req.label),
                detail: reason,
                at: Instant::now(),
            },
        };
        self.toasts.push(toast);
    }

    fn cards(&mut self, ui: &mut Ui) {
        let snap = self.snapshot.clone();
        let gap = 12.0;
        let width = ((ui.available_width() - gap * 4.0) / 5.0).floor();
        let h = &self.history;
        let first_disk = snap
            .disks
            .first()
            .map_or(Device::Cpu, |d| Device::Disk(d.index));
        // 网络卡片跳到正在用的网卡：优先已连接的。
        let first_nic = snap
            .nics
            .iter()
            .find(|n| n.connected)
            .or(snap.nics.first())
            .map_or(Device::Cpu, |n| Device::Network(n.index));
        let mut clicked = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let cards = [
                (
                    Device::Cpu,
                    widgets::cpu_card(
                        ui,
                        width,
                        false,
                        snap.cpu_total,
                        &snap.cpu_cores,
                        metric::CPU,
                    ),
                ),
                (
                    Device::Memory,
                    widgets::memory_card(ui, width, false, &snap),
                ),
                (first_disk, widgets::disk_card(ui, width, &snap)),
                (
                    first_nic,
                    widgets::network_card(ui, width, false, &snap, &h[3], &h[4]),
                ),
                (Device::Gpu(0), widgets::gpu_card(ui, width, &snap, &h[2])),
            ];
            for (device, resp) in cards {
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("查看详情")
                    .clicked()
                {
                    clicked = Some(device);
                }
            }
        });
        if let Some(device) = clicked {
            self.device = device;
            self.view = View::Performance;
        }
    }

    /// 顶部标签：进程 / 性能。
    fn tabs(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (view, glyph, label) in [
                (View::Processes, Icon::List, "进程"),
                (View::Performance, Icon::Activity, "性能"),
            ] {
                let on = self.view == view;
                let galley = ui.painter().layout_no_wrap(
                    label.into(),
                    egui::FontId::new(14.0, theme::semibold()),
                    p.text,
                );
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(galley.size().x + 46.0, 34.0), Sense::click());
                if on || resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(8),
                        if on { p.surface } else { p.hover },
                    );
                }
                let fg = if on { p.text } else { p.muted };
                widgets::icon(
                    ui,
                    glyph,
                    pos2(rect.left() + 19.0, rect.center().y),
                    16.0,
                    if on { p.accent } else { fg },
                );
                ui.painter().galley(
                    pos2(rect.left() + 32.0, rect.center().y - galley.size().y / 2.0),
                    galley,
                    fg,
                );
                if resp.clicked() {
                    self.view = view;
                }
            }
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let elevated = self.snapshot.elevated;
        let target = self.selected_pids();
        ui.horizontal(|ui| {
            let resp = widgets::search_box(ui, &mut self.filter, 320.0);
            if resp.changed() {
                self.dirty = true;
            }
            if ui.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
                resp.request_focus();
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.filter.clear();
                self.dirty = true;
            }
            if widgets::toggle_chip(ui, &mut self.listening_only, Icon::Plug, "监听端口")
                .on_hover_text("只看正在监听 TCP 端口的进程。也可以直接搜索 :端口号")
                .clicked()
            {
                self.dirty = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::segmented(
                    ui,
                    &mut self.grouped,
                    [(Icon::Layers, "分组"), (Icon::List, "列表")],
                ) {
                    self.dirty = true;
                }
                ui.add_space(4.0);
                let enabled = target.is_some();
                let resp = widgets::tool_button(ui, Icon::OctagonX, "结束任务", enabled, true);
                let resp = match &target {
                    Some((_, label)) => resp.on_hover_text(format!("结束 {label}（Delete）")),
                    None => resp.on_hover_text("先在列表中选中一个进程或分组"),
                };
                if enabled
                    && resp.clicked()
                    && let Some((pids, label)) = target.clone()
                {
                    let ctx = ui.ctx().clone();
                    self.apply(&ctx, Action::Kill { pids, label });
                }
                if !elevated
                    && widgets::tool_button(ui, Icon::Shield, "以管理员身份重启", true, false)
                        .on_hover_text("启用逐进程网络统计，并允许结束更多系统进程")
                        .clicked()
                    && pulse_probe::relaunch_elevated()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn footer(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        let faint = |t: String| RichText::new(t).size(11.5).color(p.faint);
        ui.horizontal(|ui| {
            ui.label(faint(format!(
                "{} 个进程 · 采集 {:.1} ms",
                self.snapshot.processes.len(),
                self.snapshot.collect_time.as_secs_f64() * 1e3,
            )));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match &self.snapshot.net_status {
                    NetStatus::Running => {}
                    NetStatus::NeedsAdmin => {
                        ui.label(faint("逐进程网络统计需要管理员权限".into()));
                    }
                    NetStatus::Failed(e) => {
                        ui.label(faint(e.clone()));
                    }
                }
            });
        });
    }

    fn table(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        let mut actions = Vec::new();
        let Self {
            icons,
            table,
            snapshot,
            selected,
            sort,
            descending,
            grouped,
            ..
        } = self;
        let procs = &snapshot.processes;
        let cores = snapshot.cpu_count.max(1) as f32;
        let net_ok = snapshot.net_status == NetStatus::Running;
        let grouped = *grouped;
        // Row 里存的是下标，这里给"单个进程"借出一个可切片的下标数组。
        let procs_index: Vec<usize> = (0..procs.len()).collect();

        ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
        let cols = Col::visible(ui.available_width(), ui.spacing().item_spacing.x);
        let mut builder = TableBuilder::new(ui)
            .striped(false)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(Col::MIN_NAME).clip(true));
        for c in &cols {
            builder = builder.column(Column::exact(c.width()));
        }
        builder
            .header(34.0, |mut h| {
                let header = std::iter::once(("名称", SortKey::Name, false))
                    .chain(cols.iter().map(|c| (c.label(), c.key(), true)));
                for (label, key, right) in header {
                    h.col(|ui| {
                        let r = ui.max_rect();
                        ui.painter().hline(
                            r.x_range().expand(6.0),
                            r.bottom() - 0.5,
                            Stroke::new(1.0, p.border),
                        );
                        let active = *sort == key;
                        let text = RichText::new(label).size(12.0).color(if active {
                            p.text
                        } else {
                            p.muted
                        });
                        let layout = if right {
                            Layout::right_to_left(Align::Center)
                        } else {
                            Layout::left_to_right(Align::Center)
                        };
                        ui.with_layout(layout, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if !right {
                                ui.add_space(4.0);
                            }
                            let arrow = |ui: &mut Ui| {
                                if active {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                                    let glyph = if *descending {
                                        Icon::ArrowDown
                                    } else {
                                        Icon::ArrowUp
                                    };
                                    widgets::icon(ui, glyph, r.center(), 13.0, p.text);
                                }
                            };
                            // 右对齐列里箭头在最右侧，先放。
                            if right {
                                arrow(ui);
                            }
                            let mut resp = ui.add(egui::Label::new(text).sense(Sense::click()));
                            if !right {
                                arrow(ui);
                            }
                            if key == SortKey::Network && !net_ok {
                                resp = resp.on_hover_text("逐进程网络统计需要管理员权限");
                            }
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                actions.push(Action::Sort(key));
                            }
                        });
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, table.rows.len(), |mut row| {
                    let index = row.index();
                    let r = table.rows[index];
                    let (name, metrics, path, indent, sel, count) = match r {
                        Row::Group(gi) => {
                            let g = &table.groups[gi];
                            let path = g.members.iter().find_map(|&i| procs[i].path.clone());
                            (
                                g.name.as_str(),
                                g.metrics,
                                path,
                                0.0,
                                Selection::Group(g.key.clone()),
                                Some(g.members.len()),
                            )
                        }
                        Row::Member(i) | Row::Single(i) => {
                            let pr = &procs[i];
                            let indent = match (r, grouped) {
                                (Row::Member(_), _) => INDENT * 2.0,
                                (_, true) => INDENT,
                                _ => 0.0,
                            };
                            (
                                pr.name.as_str(),
                                Metrics::of(pr),
                                pr.path.clone(),
                                indent,
                                Selection::Process(pr.pid),
                                None,
                            )
                        }
                    };
                    row.set_selected(selected.as_ref() == Some(&sel));
                    let members: &[usize] = match r {
                        Row::Group(gi) => &table.groups[gi].members,
                        Row::Member(i) | Row::Single(i) => std::slice::from_ref(&procs_index[i]),
                    };
                    // 分组里的成员共用同一个描述，改为显示命令行摘要，才看得出各自在跑什么（vite.js、main.js…）。
                    let secondary: Option<String> = match r {
                        Row::Member(i) => procs[i].command_line.as_deref().and_then(cmd_summary),
                        _ => None,
                    }
                    .or_else(|| {
                        members
                            .iter()
                            .find_map(|&i| procs[i].description.clone())
                            .filter(|d| !same_name(d, name))
                            .map(|d| d.to_string())
                    });
                    let mut tcp: Vec<u16> = members
                        .iter()
                        .flat_map(|&i| {
                            procs[i]
                                .ports
                                .iter()
                                .filter(|x| x.proto == Proto::Tcp)
                                .map(|x| x.port)
                        })
                        .collect();
                    tcp.sort_unstable();
                    tcp.dedup();

                    row.col(|ui| {
                        let rect = ui.max_rect();
                        let cy = rect.center().y;
                        let mut x = rect.left() + 4.0 + indent;
                        if count.is_some() {
                            widgets::chevron(
                                ui,
                                pos2(x + 6.0, cy),
                                is_expanded(table, index),
                                p.muted,
                            );
                            x += INDENT;
                        }
                        let icon_rect =
                            Rect::from_min_size(pos2(x, cy - ICON / 2.0), vec2(ICON, ICON));
                        match path.as_ref().and_then(|path| icons.get(path)) {
                            Some(tex) => {
                                ui.painter().image(
                                    tex.id(),
                                    icon_rect,
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            }
                            None => widgets::avatar(ui.painter(), icon_rect, name),
                        }
                        x += ICON + 10.0;
                        let text_rect =
                            Rect::from_min_max(pos2(x, rect.top()), rect.right_bottom());
                        // 右侧先放端口标签，剩下的空间给名称和描述，描述过长时截断。
                        ui.scope_builder(
                            UiBuilder::new()
                                .max_rect(text_rect)
                                .layout(Layout::right_to_left(Align::Center)),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                // 至少给名称留 140px，剩下的空间每 ~54px 放一个端口标签，最多 3 个。
                                let max_chips =
                                    ((text_rect.width() - 140.0) / 54.0).floor().clamp(0.0, 3.0)
                                        as usize;
                                if tcp.len() > max_chips {
                                    ui.label(
                                        RichText::new(format!("+{}", tcp.len() - max_chips))
                                            .size(11.0)
                                            .color(p.faint),
                                    );
                                }
                                for port in tcp.iter().take(max_chips).rev() {
                                    widgets::port_chip(ui, *port);
                                }
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    ui.add(
                                        egui::Label::new(RichText::new(name).color(p.text))
                                            .truncate(),
                                    );
                                    if let Some(n) = count {
                                        ui.label(
                                            RichText::new(format!("{n}")).size(11.5).color(p.faint),
                                        );
                                    }
                                    if let Some(d) = &secondary {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(d).size(12.0).color(p.faint),
                                            )
                                            .truncate(),
                                        );
                                    }
                                });
                            },
                        );
                    });
                    for c in &cols {
                        row.col(|ui| match c {
                            Col::Pid => {
                                let text = match &sel {
                                    Selection::Process(pid) => pid.to_string(),
                                    Selection::Group(_) => String::new(),
                                };
                                right(ui, RichText::new(text).color(p.faint));
                            }
                            Col::Cpu => {
                                // 两个核心满载即视为最热，否则多核机器上永远看不出颜色。
                                widgets::heat(ui, metrics.cpu * cores / 200.0, metric::CPU);
                                right(
                                    ui,
                                    value(&p, format!("{:.1}%", metrics.cpu), metrics.cpu < 0.05),
                                );
                            }
                            Col::CpuTime => {
                                // 不足 1 秒的视为几乎没用过 CPU，淡显。
                                let t = metrics.cpu_time;
                                right(ui, value(&p, fmt_cpu_time(t), t < 10_000_000));
                            }
                            Col::Memory => {
                                widgets::heat(
                                    ui,
                                    metrics.memory as f32 / (4u64 << 30) as f32,
                                    metric::MEMORY,
                                );
                                right(ui, value(&p, fmt_bytes(metrics.memory), false));
                            }
                            Col::Gpu => {
                                widgets::heat(ui, metrics.gpu / 60.0, metric::GPU);
                                right(
                                    ui,
                                    value(&p, format!("{:.1}%", metrics.gpu), metrics.gpu < 0.05),
                                );
                            }
                            Col::Vram => {
                                widgets::heat(
                                    ui,
                                    metrics.vram as f32 / (4u64 << 30) as f32,
                                    metric::GPU,
                                );
                                let text = if metrics.vram == 0 {
                                    "—".to_string()
                                } else {
                                    fmt_bytes(metrics.vram)
                                };
                                right(ui, value(&p, text, metrics.vram == 0));
                            }
                            Col::Disk => {
                                widgets::heat(ui, metrics.disk as f32 / 50e6, metric::DISK);
                                right(ui, value(&p, fmt_rate(metrics.disk), metrics.disk == 0));
                            }
                            Col::Network if net_ok => {
                                widgets::heat(ui, metrics.net() as f32 / 10e6, metric::NETWORK);
                                right(ui, value(&p, fmt_rate(metrics.net()), metrics.net() == 0));
                            }
                            Col::Network => right(ui, RichText::new("—").color(p.faint)),
                        });
                    }

                    let resp = row.response();
                    if resp.clicked() || resp.secondary_clicked() {
                        actions.push(Action::Select(sel.clone()));
                        if let (true, Selection::Group(key)) = (resp.clicked(), &sel) {
                            actions.push(Action::Toggle(key.clone()));
                        }
                    }
                    resp.context_menu(|ui| {
                        ui.set_min_width(220.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let heading = |ui: &mut Ui, t: String| {
                            ui.add_space(2.0);
                            ui.label(RichText::new(t).size(12.0).color(p.muted));
                            ui.add_space(2.0);
                        };
                        match (&sel, count) {
                            (Selection::Group(key), Some(n)) => {
                                heading(ui, format!("{name} · {n} 个进程"));
                                if widgets::menu_item(
                                    ui,
                                    Icon::OctagonX,
                                    &format!("结束全部 {n} 个进程"),
                                    true,
                                )
                                .clicked()
                                {
                                    let pids = procs
                                        .iter()
                                        .filter(|p| p.name.to_lowercase() == *key)
                                        .map(|p| p.pid)
                                        .collect();
                                    actions.push(Action::Kill {
                                        pids,
                                        label: format!("{name} 的 {n} 个进程"),
                                    });
                                }
                            }
                            (Selection::Process(pid), _) => {
                                heading(ui, format!("{name} · {pid}"));
                                if widgets::menu_item(ui, Icon::OctagonX, "结束进程", true)
                                    .clicked()
                                {
                                    actions.push(Action::Kill {
                                        pids: vec![*pid],
                                        label: format!("{name} ({pid})"),
                                    });
                                }
                                let tree = model::descendants(snapshot, *pid);
                                if tree.len() > 1
                                    && widgets::menu_item(
                                        ui,
                                        Icon::GitBranch,
                                        &format!("结束进程树（{} 个）", tree.len()),
                                        true,
                                    )
                                    .clicked()
                                {
                                    // 先结束子进程，避免父进程退出后孩子被重新托管。
                                    let pids = tree.into_iter().rev().collect();
                                    actions.push(Action::Kill {
                                        pids,
                                        label: format!("{name} 及其子进程"),
                                    });
                                }
                                ui.separator();
                                if widgets::menu_item(ui, Icon::Hash, "复制 PID", false).clicked()
                                {
                                    actions.push(Action::Copy(pid.to_string()));
                                }
                            }
                            _ => {}
                        }
                        let cmd = members.first().and_then(|&i| procs[i].command_line.clone());
                        if let (Selection::Process(_), Some(cmd)) = (&sel, cmd)
                            && widgets::menu_item(ui, Icon::Terminal, "复制命令行", false).clicked()
                        {
                            actions.push(Action::Copy(cmd.to_string()));
                        }
                        if !tcp.is_empty()
                            && widgets::menu_item(ui, Icon::Plug, "复制监听端口", false).clicked()
                        {
                            actions.push(Action::Copy(
                                tcp.iter()
                                    .map(u16::to_string)
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ));
                        }
                        if let Some(path) = &path {
                            if matches!(sel, Selection::Group(_)) {
                                ui.separator();
                            }
                            if widgets::menu_item(ui, Icon::Copy, "复制路径", false).clicked() {
                                actions.push(Action::Copy(path.to_string()));
                            }
                            if widgets::menu_item(ui, Icon::FolderOpen, "打开文件位置", false)
                                .clicked()
                            {
                                actions.push(Action::Reveal(path.clone()));
                            }
                        }
                    });
                    if let Selection::Process(pid) = sel
                        && let Some(pr) = procs.iter().find(|p| p.pid == pid)
                    {
                        resp.on_hover_ui(|ui| {
                            ui.set_max_width(520.0);
                            let small = |t: String| RichText::new(t).size(11.5).color(p.muted);
                            ui.label(RichText::new(&pr.name).strong());
                            if let Some(d) = &pr.description {
                                ui.label(small(d.to_string()));
                            }
                            ui.label(small(format!(
                                "PID {} · 线程 {} · 句柄 {}",
                                pr.pid, pr.threads, pr.handles
                            )));
                            ui.label(small(format!(
                                "已运行 {} · CPU 时间 {}（用户 {} · 内核 {}）",
                                fmt_uptime(pr.create_time),
                                fmt_cpu_time(pr.user_time + pr.kernel_time),
                                fmt_cpu_time(pr.user_time),
                                fmt_cpu_time(pr.kernel_time),
                            )));
                            if pr.gpu_dedicated + pr.gpu_shared > 0 {
                                ui.label(small(format!(
                                    "显存 {} · 共享 GPU 内存 {}",
                                    fmt_bytes(pr.gpu_dedicated),
                                    fmt_bytes(pr.gpu_shared)
                                )));
                            }
                            let fmt_ports = |proto: Proto| {
                                pr.ports
                                    .iter()
                                    .filter(|x| x.proto == proto)
                                    .map(|x| x.port.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            let (t, u) = (fmt_ports(Proto::Tcp), fmt_ports(Proto::Udp));
                            if !t.is_empty() {
                                ui.label(small(format!("监听 TCP {t}")));
                            }
                            if !u.is_empty() {
                                ui.label(small(format!("UDP {u}")));
                            }
                            if net_ok && pr.net_rx + pr.net_tx > 0 {
                                ui.label(small(format!(
                                    "↓ {}/s  ↑ {}/s",
                                    fmt_bytes(pr.net_rx),
                                    fmt_bytes(pr.net_tx)
                                )));
                            }
                            if let Some(cmd) = &pr.command_line {
                                ui.add_space(4.0);
                                let cmd: String = cmd.chars().take(600).collect();
                                ui.label(RichText::new(cmd).monospace().size(11.0).color(p.text));
                            } else if let Some(path) = &pr.path {
                                ui.label(small(path.to_string()));
                            }
                        });
                    }
                });
            });

        let ctx = ui.ctx().clone();
        for a in actions {
            self.apply(&ctx, a);
        }
    }
}

/// 表格里的数值列。宽度不够时按优先级从低到高依次隐藏，保证名称列至少 `MIN_NAME` 宽。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Col {
    Pid,
    Cpu,
    CpuTime,
    Memory,
    Gpu,
    Vram,
    Disk,
    Network,
}

impl Col {
    const ALL: [Col; 8] = [
        Col::Pid,
        Col::Cpu,
        Col::CpuTime,
        Col::Memory,
        Col::Gpu,
        Col::Vram,
        Col::Disk,
        Col::Network,
    ];
    const MIN_NAME: f32 = 260.0;

    fn width(self) -> f32 {
        match self {
            Col::Pid | Col::Gpu => 52.0,
            Col::Cpu => 56.0,
            Col::CpuTime | Col::Vram => 72.0,
            Col::Memory => 76.0,
            Col::Disk | Col::Network => 80.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Col::Pid => "PID",
            Col::Cpu => "CPU",
            Col::CpuTime => "CPU 时间",
            Col::Memory => "内存",
            Col::Gpu => "GPU",
            Col::Vram => "显存",
            Col::Disk => "磁盘",
            Col::Network => "网络",
        }
    }

    fn key(self) -> SortKey {
        match self {
            Col::Pid => SortKey::Pid,
            Col::Cpu => SortKey::Cpu,
            Col::CpuTime => SortKey::CpuTime,
            Col::Memory => SortKey::Memory,
            Col::Gpu => SortKey::Gpu,
            Col::Vram => SortKey::GpuMemory,
            Col::Disk => SortKey::Disk,
            Col::Network => SortKey::Network,
        }
    }

    /// 越小越先被隐藏。
    fn priority(self) -> u8 {
        match self {
            Col::CpuTime => 0,
            Col::Pid => 1,
            Col::Disk => 2,
            Col::Vram => 3,
            Col::Gpu => 4,
            Col::Network => 5,
            Col::Memory => 6,
            Col::Cpu => 7,
        }
    }

    fn visible(available: f32, spacing: f32) -> Vec<Col> {
        let mut cols = Col::ALL.to_vec();
        let name_width =
            |cols: &[Col]| available - cols.iter().map(|c| c.width() + spacing).sum::<f32>();
        while name_width(&cols) < Col::MIN_NAME && cols.len() > 2 {
            let drop = *cols.iter().min_by_key(|c| c.priority()).unwrap();
            cols.retain(|&c| c != drop);
        }
        cols
    }
}

/// 分组行之后紧跟成员行即视为已展开（搜索时会自动展开）。
fn is_expanded(table: &Table, index: usize) -> bool {
    matches!(table.rows.get(index + 1), Some(Row::Member(_)))
}

impl eframe::App for App {
    /// egui 会把鼠标滚轮的一格（按行计）拆到之后几十帧里平滑消化；我们关了 vsync，
    /// 这些帧会不限速地连续渲染，滚一下就吃满一个核。把行滚动改写成若干段 <8pt 的像素滚动，
    /// egui 对这种"触控板式"输入不做平滑，一帧内全部生效。
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let line = ctx.options(|o| o.input_options.line_scroll_speed);
        let page = raw_input.screen_rect.map_or(400.0, |r| r.height());
        let mut events = Vec::with_capacity(raw_input.events.len());
        for e in raw_input.events.drain(..) {
            match e {
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    modifiers,
                } if unit != egui::MouseWheelUnit::Point => {
                    let total = delta
                        * if unit == egui::MouseWheelUnit::Line {
                            line
                        } else {
                            page
                        };
                    let steps = (total.length() / 7.5).ceil().max(1.0);
                    for _ in 0..steps as usize {
                        events.push(egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: total / steps,
                            phase,
                            modifiers,
                        });
                    }
                }
                e => events.push(e),
            }
        }
        raw_input.events = events;
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // 自适应限帧：只有持续高帧率（1000Hz 游戏鼠标等）时才睡眠到一个刷新周期。
        // 平时不睡：在帧内睡眠会让 NVIDIA 驱动线程忙等，每帧额外多耗约 1.7ms。
        const BURST_WINDOW: Duration = Duration::from_millis(100);
        const BURST_FRAMES: usize = 20; // 100ms 内超过 20 帧即 >200fps；普通鼠标每次移动画两帧，约 130fps
        const MIN_FRAME: Duration = Duration::from_micros(6900); // ~144fps
        let now = Instant::now();
        let recent = self
            .passes
            .iter()
            .rev()
            .take_while(|t| now.duration_since(**t) < BURST_WINDOW)
            .count();
        if recent > BURST_FRAMES
            && let Some(&last) = self.passes.back()
        {
            let since = now.duration_since(last);
            if since < MIN_FRAME {
                std::thread::sleep(MIN_FRAME - since);
            }
        }
        let started = Instant::now();
        self.passes.push_back(started);
        while self
            .passes
            .front()
            .is_some_and(|t| started.duration_since(*t) > Duration::from_secs(1))
        {
            self.passes.pop_front();
        }
        self.pull();
        self.icons.poll(ui.ctx());
        if self.dirty {
            self.table = model::build(
                &self.snapshot,
                &Query {
                    filter: &self.filter,
                    grouped: self.grouped,
                    listening_only: self.listening_only,
                    sort: self.sort,
                    descending: self.descending,
                    expanded: &self.expanded,
                },
            );
            self.dirty = false;
        }

        let p = Palette::of(ui.ctx());
        egui::Panel::top("top")
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg).inner_margin(Margin {
                left: 20,
                right: 20,
                top: 18,
                bottom: 12,
            }))
            .show(ui, |ui| {
                self.tabs(ui);
                if self.view == View::Processes {
                    ui.add_space(12.0);
                    self.cards(ui);
                    ui.add_space(16.0);
                    self.toolbar(ui);
                }
            });
        egui::Panel::bottom("footer")
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(20, 8)),
            )
            .show(ui, |ui| self.footer(ui));
        let central = egui::CentralPanel::default().frame(
            Frame::NONE
                .fill(p.bg)
                .inner_margin(Margin::symmetric(14, 0)),
        );
        match self.view {
            View::Processes => {
                central.show(ui, |ui| self.table(ui));
            }
            View::Performance => {
                let action = central
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        perf::show(
                            ui,
                            &self.snapshot,
                            &self.series,
                            &self.gpu_history,
                            &mut self.device,
                            &mut self.icons,
                        )
                    })
                    .inner;
                if let Some(perf::PerfAction::Search(name)) = action {
                    self.filter = name;
                    self.dirty = true;
                    self.view = View::Processes;
                }
            }
        }

        let ctx = ui.ctx().clone();
        if let Some(req) = &self.pending_kill {
            match dialogs::confirm_kill(&ctx, req, &self.snapshot) {
                Some(Decision::Confirm) => {
                    let req = self.pending_kill.take().unwrap();
                    self.kill_confirmed(req);
                }
                Some(Decision::Cancel) => self.pending_kill = None,
                None => {}
            }
        }
        dialogs::toasts(&ctx, &mut self.toasts);

        // Delete 结束选中项；输入框有焦点或确认框打开时不响应，免得删字时误杀进程。
        let nothing_focused = ui.ctx().memory(|m| m.focused().is_none());
        if nothing_focused
            && self.view == View::Processes
            && self.pending_kill.is_none()
            && ui.input(|i| i.key_pressed(Key::Delete))
            && let Some((pids, label)) = self.selected_pids()
        {
            let ctx = ui.ctx().clone();
            self.apply(&ctx, Action::Kill { pids, label });
        }
    }
}

fn right(ui: &mut Ui, text: RichText) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(text));
}

fn value(p: &Palette, text: String, zero: bool) -> RichText {
    RichText::new(text).color(if zero { p.faint } else { p.text })
}

/// 描述与 exe 名只差扩展名或大小写时不重复显示（如 "Notepad" 与 notepad.exe）。
fn same_name(description: &str, exe: &str) -> bool {
    let stem = exe
        .strip_suffix(".exe")
        .or_else(|| exe.strip_suffix(".EXE"))
        .unwrap_or(exe);
    description.eq_ignore_ascii_case(stem) || description.eq_ignore_ascii_case(exe)
}

/// 命令行摘要：去掉 exe 本身，路径只保留文件名。
/// `"C:\...\node.exe" "C:\app\js\main.js" --port 3000` → `main.js --port 3000`
fn cmd_summary(cmd: &str) -> Option<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in cmd.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        args.push(cur);
    }
    let summary: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| match a.rsplit(['\\', '/']).next() {
            // 以 - 开头的参数（如 --config=C:\x）保持原样，只缩短纯路径。
            Some(file) if !a.starts_with('-') && !file.is_empty() => file,
            _ => a.as_str(),
        })
        .collect();
    (!summary.is_empty()).then(|| summary.join(" "))
}

fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    let b = b as f64;
    if b < KB {
        format!("{b:.0} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else {
        format!("{:.2} GB", b / KB / KB / KB)
    }
}

/// 累计 CPU 时间（100ns 单位）→ `h:mm:ss`，与 Process Explorer 一致。
fn fmt_cpu_time(t: u64) -> String {
    let secs = t / 10_000_000;
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// 进程创建时间（FILETIME，100ns 自 1601 年起）到现在的时长。
fn fmt_uptime(create_time: i64) -> String {
    // 1601-01-01 到 1970-01-01 的 100ns 数。
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64 / 100 + EPOCH_DIFF);
    if create_time <= 0 || now <= create_time {
        return "—".into();
    }
    let secs = (now - create_time) / 10_000_000;
    match secs {
        s if s < 60 => format!("{s} 秒"),
        s if s < 3600 => format!("{} 分钟", s / 60),
        s if s < 86_400 => format!("{} 小时 {} 分钟", s / 3600, s / 60 % 60),
        s => format!("{} 天 {} 小时", s / 86_400, s / 3600 % 24),
    }
}

/// 速率，0 也显示数值（卡片里用；表格里 0 显示为"—"）。
pub fn fmt_rate_full(b: u64) -> String {
    format!("{}/s", fmt_bytes(b))
}

fn fmt_rate(b: u64) -> String {
    if b == 0 {
        "—".into()
    } else {
        format!("{}/s", fmt_bytes(b))
    }
}

#[cfg(test)]
mod tests {
    use super::{Col, cmd_summary};

    #[test]
    fn summarizes_command_lines() {
        assert_eq!(
            cmd_summary(r#""C:\Program Files\nodejs\node.exe" "C:\app\js\main.js" --port 3000"#)
                .as_deref(),
            Some("main.js --port 3000")
        );
        assert_eq!(
            cmd_summary(r"node ./node_modules/vite/bin/vite.js dev").as_deref(),
            Some("vite.js dev")
        );
        assert_eq!(
            cmd_summary(r"python -m http.server --dir=C:\www").as_deref(),
            Some(r"-m http.server --dir=C:\www")
        );
        assert_eq!(cmd_summary(r"C:\Windows\explorer.exe"), None);
    }

    #[test]
    fn columns_hide_by_priority() {
        let all = Col::visible(2000.0, 12.0);
        assert_eq!(all.len(), Col::ALL.len());
        // 变窄时先藏 CPU 时间，再藏 PID、磁盘；CPU 与内存始终保留。
        let narrow = Col::visible(800.0, 12.0);
        assert!(!narrow.contains(&Col::CpuTime));
        assert!(narrow.contains(&Col::Cpu) && narrow.contains(&Col::Memory));
        let tiny = Col::visible(300.0, 12.0);
        assert_eq!(tiny, vec![Col::Cpu, Col::Memory]);
    }
}
