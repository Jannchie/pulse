#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod brand_art;
mod dialogs;
mod gpu_panel;
mod icons;
mod icons_gen;
mod model;
mod perf;
mod services_view;
mod settings;
mod settings_view;
mod startup_view;
mod table;
mod theme;
mod titlebar;
mod users_view;
mod widgets;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Frame, Key, Layout, Margin, RichText, Sense, Ui, pos2, vec2};
use pulse_probe::control::Priority;
use pulse_probe::{Collector, NetStatus, Snapshot, t, tf};

use dialogs::{AffinityDialog, Decision, KillRequest, RunDialog, Toast};
use gpu_panel::GpuHistory;
use icons::Icons;
use icons_gen::Icon;
use model::{Query, Row, SortKey, Table};
use perf::{Device, Series};
use pulse_probe::services::{Service, ServiceMonitor};
use pulse_probe::startup::StartupItem;
use services_view::{ServiceAction, ServicesState};
use settings::{Settings, Speed};
use startup_view::{StartupAction, StartupState};
use std::sync::atomic::{AtomicU64, Ordering};
use theme::{Palette, metric};
use users_view::UserAction;

const HISTORY: usize = 60;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Pulse")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([1100.0, 640.0])
            // 先给一个图标，免得出现 eframe 默认的 egui 标志；窗口建好后 titlebar 会换成 exe 资源里的多尺寸图标。
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: brand_art::brand_rgba(64),
                width: 64,
                height: 64,
            })),
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
        Box::new(|cc| {
            titlebar::install(cc);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
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

/// `interval` 为采集间隔（毫秒），0 表示暂停。
fn spawn_collector(ctx: egui::Context, interval: Arc<AtomicU64>) -> Arc<Mutex<Shared>> {
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
                let ms = interval.load(Ordering::Relaxed);
                if ms == 0 {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
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
                std::thread::sleep(Duration::from_millis(ms).saturating_sub(started.elapsed()));
            }
        })
        .expect("spawn collector");
    shared
}

enum Modal {
    Kill(KillRequest),
    Affinity(AffinityDialog),
    Run(RunDialog),
    User(UserAction),
}

/// 服务列表由单独的线程采集：首次读取几百个服务的配置要半秒多，放进进程采集线程会拖慢启动。
#[derive(Default)]
struct ServicesShared {
    list: Arc<Vec<Service>>,
    /// 操作完成后请求立即刷新；内层是需要重新读取配置的服务名。
    refresh: Option<Option<String>>,
    /// 服务页正在显示。不显示时只为悬停提示维护 pid → 服务映射，刷新慢得多。
    active: bool,
}

fn spawn_services(ctx: egui::Context) -> Arc<Mutex<ServicesShared>> {
    let shared = Arc::new(Mutex::new(ServicesShared::default()));
    let out = shared.clone();
    std::thread::Builder::new()
        .name("services".into())
        .spawn(move || {
            let mut monitor = ServiceMonitor::new();
            loop {
                {
                    let mut s = out.lock().unwrap();
                    if let Some(name) = s.refresh.take() {
                        monitor.invalidate(name.as_deref());
                    }
                    monitor.set_interval(Duration::from_secs(if s.active { 3 } else { 15 }));
                }
                let list = monitor.sample();
                let changed = {
                    let mut s = out.lock().unwrap();
                    let changed = !Arc::ptr_eq(&s.list, &list);
                    s.list = list;
                    changed
                };
                if changed {
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("spawn services");
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
    Services,
    Startup,
    Users,
    Settings,
}

#[derive(Clone, PartialEq)]
enum Selection {
    Process(u32),
    Group(String),
}

enum Action {
    Select(Selection),
    Toggle(String),
    /// 强制结束（先确认）。
    Kill {
        pids: Vec<u32>,
        label: String,
    },
    /// 给窗口发 WM_CLOSE，让程序自己退出。
    Close {
        pids: Vec<u32>,
        label: String,
    },
    SwitchTo(isize),
    Suspend {
        pids: Vec<u32>,
        label: String,
        resume: bool,
    },
    Efficiency {
        pids: Vec<u32>,
        label: String,
        on: bool,
    },
    Priority {
        pid: u32,
        label: String,
        priority: Priority,
    },
    Affinity {
        pid: u32,
        label: String,
    },
    Dump {
        pid: u32,
        name: String,
    },
    Properties(Arc<str>),
    SearchOnline(String),
    GotoServices(u32),
    Reveal(Arc<str>),
    Copy(String),
    Sort(SortKey),
    /// 在指定位置打开进程右键菜单。
    OpenMenu(Selection, egui::Pos2),
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
    listening_only: bool,
    expanded: HashSet<String>,
    selected: Option<Selection>,
    icons: Icons,
    /// 打开着的进程右键菜单：针对的选中项与位置。
    menu: Option<(Selection, egui::Pos2)>,
    /// 当前打开的对话框，同一时间最多一个。
    modal: Option<Modal>,
    toasts: Vec<Toast>,
    /// 后台任务（转储等）完成后发回的通知。
    jobs: (Sender<Toast>, Receiver<Toast>),
    /// pid → 该进程托管的服务名。
    services_by_pid: HashMap<u32, Vec<String>>,
    services_shared: Arc<Mutex<ServicesShared>>,
    services: Arc<Vec<Service>>,
    services_view: ServicesState,
    /// None 表示还没读过。
    startup: Option<Vec<StartupItem>>,
    startup_view: StartupState,
    /// 正在后台读取启动项。
    startup_rx: Option<Receiver<Vec<StartupItem>>>,
    sessions: Vec<pulse_probe::sessions::Session>,
    sessions_at: Option<Instant>,
    settings: Settings,
    interval: Arc<AtomicU64>,
    taskmgr_replaced: bool,
    /// 键盘导航后让表格滚动到这一行。
    scroll_to: Option<usize>,
    /// 最近一秒内的重绘时刻，用来确认空闲时没有空转。
    passes: VecDeque<Instant>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        egui_extras::install_image_loaders(ctx);
        let settings = Settings::load();
        pulse_probe::i18n::set_lang(settings.language.resolve());
        ctx.set_theme(settings.theme.preference());
        if settings.always_on_top {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
        }
        let interval = Arc::new(AtomicU64::new(settings.speed.millis()));
        Self {
            shared: spawn_collector(ctx.clone(), interval.clone()),
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
            filter: initial_filter(),
            listening_only: false,
            expanded: HashSet::new(),
            selected: None,
            icons: Icons::new(ctx),
            menu: None,
            modal: None,
            toasts: Vec::new(),
            jobs: channel(),
            services_by_pid: HashMap::new(),
            services_shared: spawn_services(ctx.clone()),
            services: Arc::default(),
            services_view: ServicesState::default(),
            startup: None,
            startup_view: StartupState::default(),
            startup_rx: None,
            sessions: Vec::new(),
            sessions_at: None,
            interval,
            taskmgr_replaced: pulse_probe::os::taskmgr_replaced(),
            settings,
            scroll_to: None,
            passes: VecDeque::new(),
        }
    }

    fn pull(&mut self) {
        let list = {
            let mut s = self.services_shared.lock().unwrap();
            s.active = self.view == View::Services;
            s.list.clone()
        };
        if !Arc::ptr_eq(&list, &self.services) {
            self.services_by_pid.clear();
            for svc in list.iter().filter(|s| s.pid > 0) {
                self.services_by_pid
                    .entry(svc.pid)
                    .or_default()
                    .push(svc.name.to_string());
            }
            self.services = list;
        }
        let s = self.shared.lock().unwrap();
        if s.seq == self.seen_seq {
            return;
        }
        self.seen_seq = s.seq;
        widgets::set_sample_seq(s.seq);
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
                let members: Vec<_> = procs
                    .iter()
                    .filter(|p| p.name.to_lowercase() == *key)
                    .collect();
                let name = &members.first()?.name;
                let n = members.len();
                let label = tf!(
                    "{name} 的 {n} 个进程",
                    "{name} ({n} processes)",
                    "{name} の {n} 個のプロセス"
                );
                Some((members.iter().map(|p| p.pid).collect(), label))
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        use pulse_probe::control;
        match action {
            Action::Select(s) => self.selected = Some(s),
            Action::Toggle(key) => {
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
                self.dirty = true;
            }
            // 强制结束一律先弹确认框，真正执行在 `kill_confirmed`。
            Action::Kill { pids, label } => {
                self.modal = Some(Modal::Kill(KillRequest { pids, label }))
            }
            Action::Close { pids, label } => {
                let sent: usize = pids.iter().map(|&pid| control::close_windows(pid)).sum();
                if sent == 0 {
                    self.notify_err(
                        tf!("无法关闭 {label}", "Couldn't close {label}", "{label} を閉じられませんでした"),
                        Some(
                            t!(
                                "它没有可以关闭的窗口，可以改用结束进程",
                                "It has no window to close. Try End task instead.", "閉じられるウィンドウがありません。代わりに [タスクを終了する] を使用してください。"
                            )
                            .into(),
                        ),
                    );
                } else {
                    self.notify_ok(
                        tf!(
                            "已请求 {label} 关闭",
                            "Asked {label} to close",
                            "{label} に終了を要求しました"
                        ),
                        Some(
                            t!(
                                "程序可能会先询问是否保存",
                                "It may ask whether to save first.",
                                "終了前に保存するかどうかを確認される場合があります。"
                            )
                            .into(),
                        ),
                    );
                }
            }
            Action::SwitchTo(hwnd) => control::switch_to(hwnd),
            Action::Suspend {
                pids,
                label,
                resume,
            } => {
                let op = if resume {
                    control::resume
                } else {
                    control::suspend
                };
                let (done, failed) = if resume {
                    (
                        tf!(
                            "已恢复 {label}",
                            "Resumed {label}",
                            "{label} を再開しました"
                        ),
                        tf!(
                            "无法恢复 {label}",
                            "Couldn't resume {label}",
                            "{label} を再開できませんでした"
                        ),
                    )
                } else {
                    (
                        tf!(
                            "已挂起 {label}",
                            "Suspended {label}",
                            "{label} を中断しました"
                        ),
                        tf!(
                            "无法挂起 {label}",
                            "Couldn't suspend {label}",
                            "{label} を中断できませんでした"
                        ),
                    )
                };
                self.report(pids.iter().map(|&pid| op(pid)).collect(), done, failed);
            }
            Action::Efficiency { pids, label, on } => {
                let results = pids
                    .iter()
                    .map(|&pid| control::set_efficiency_mode(pid, on))
                    .collect();
                let (done, failed) = if on {
                    (
                        tf!(
                            "已为 {label} 开启效率模式",
                            "Turned on efficiency mode for {label}",
                            "{label} の効率モードをオンにしました"
                        ),
                        tf!(
                            "无法为 {label} 开启效率模式",
                            "Couldn't turn on efficiency mode for {label}",
                            "{label} の効率モードをオンにできませんでした"
                        ),
                    )
                } else {
                    (
                        tf!(
                            "已为 {label} 关闭效率模式",
                            "Turned off efficiency mode for {label}",
                            "{label} の効率モードをオフにしました"
                        ),
                        tf!(
                            "无法为 {label} 关闭效率模式",
                            "Couldn't turn off efficiency mode for {label}",
                            "{label} の効率モードをオフにできませんでした"
                        ),
                    )
                };
                self.report(results, done, failed);
            }
            Action::Priority {
                pid,
                label,
                priority,
            } => {
                let r = control::set_priority(pid, priority);
                let prio = priority.label();
                self.report(
                    vec![r],
                    tf!(
                        "已将 {label} 的优先级设为{prio}",
                        "Set the priority of {label} to {prio}",
                        "{label} の優先度を「{prio}」に設定しました"
                    ),
                    tf!(
                        "无法将 {label} 的优先级设为{prio}",
                        "Couldn't set the priority of {label} to {prio}",
                        "{label} の優先度を「{prio}」に設定できませんでした"
                    ),
                );
            }
            Action::Affinity { pid, label } => match control::affinity(pid) {
                Ok((mask, system)) => {
                    self.modal = Some(Modal::Affinity(AffinityDialog {
                        pid,
                        label,
                        mask,
                        system,
                    }))
                }
                Err(e) => self.notify_err(
                    tf!(
                        "无法读取 {label} 的相关性",
                        "Couldn't read the affinity of {label}",
                        "{label} のプロセッサの関係を読み取れませんでした"
                    ),
                    Some(error_text(&e)),
                ),
            },
            Action::Dump { pid, name } => {
                self.notify_ok(
                    tf!(
                        "正在为 {name} 创建转储文件…",
                        "Creating a dump file for {name}…",
                        "{name} のダンプ ファイルを作成しています…"
                    ),
                    Some(
                        t!(
                            "完整内存转储可能需要几秒到几十秒",
                            "A full memory dump can take from a few seconds to a minute.",
                            "完全メモリ ダンプには数秒から 1 分ほどかかる場合があります。"
                        )
                        .into(),
                    ),
                );
                self.spawn_job(ctx, move || match control::create_dump(pid, &name) {
                    Ok(path) => {
                        let path = path.to_string_lossy().to_string();
                        pulse_probe::reveal_in_explorer(&path);
                        Toast::ok(
                            tf!(
                                "已为 {name} 创建转储文件",
                                "Created a dump file for {name}",
                                "{name} のダンプ ファイルを作成しました"
                            ),
                            Some(path),
                        )
                    }
                    Err(e) => Toast::err(
                        tf!(
                            "无法为 {name} 创建转储文件",
                            "Couldn't create a dump file for {name}",
                            "{name} のダンプ ファイルを作成できませんでした"
                        ),
                        Some(error_text(&e)),
                    ),
                });
            }
            Action::Properties(path) => control::show_properties(&path),
            Action::SearchOnline(name) => {
                control::search_online(self.settings.search.url_prefix(), &name)
            }
            Action::GotoServices(pid) => self.goto_services(pid),
            Action::Reveal(path) => pulse_probe::reveal_in_explorer(&path),
            Action::Copy(text) => ctx.copy_text(text),
            Action::Sort(key) => self.set_sort(key),
            Action::OpenMenu(sel, pos) => self.menu = Some((sel, pos)),
        }
    }

    /// 在后台线程执行 `job`，完成后把它返回的通知送回 UI。
    fn spawn_job(&self, ctx: &egui::Context, job: impl FnOnce() -> Toast + Send + 'static) {
        let (tx, ctx) = (self.jobs.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    /// 回到进程页并用 `filter` 搜索。
    fn show_processes(&mut self, filter: String, select: Option<Selection>) {
        self.view = View::Processes;
        self.filter = filter;
        if select.is_some() {
            self.selected = select;
        }
        self.dirty = true;
    }

    fn goto_services(&mut self, pid: u32) {
        self.view = View::Services;
        self.services_view.filter = pid.to_string();
        self.services_view.state = services_view::Filter::All;
    }

    /// 读取启动项要访问几十个 exe 的版本资源，放到后台线程。
    fn reload_startup(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        self.startup_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(pulse_probe::startup::list());
            ctx.request_repaint();
        });
    }

    fn apply_startup(&mut self, ctx: &egui::Context, action: StartupAction) {
        use pulse_probe::control;
        match action {
            StartupAction::SetEnabled(i, on) => {
                let Some(item) = self.startup.as_mut().and_then(|v| v.get_mut(i)) else {
                    return;
                };
                match pulse_probe::startup::set_enabled(item, on) {
                    Ok(()) => {
                        item.enabled = on;
                        let name = item.name.clone();
                        let msg = if on {
                            tf!(
                                "已启用 {name} 的开机启动",
                                "Enabled {name} at startup",
                                "{name} のスタートアップを有効にしました"
                            )
                        } else {
                            tf!(
                                "已禁用 {name} 的开机启动",
                                "Disabled {name} at startup",
                                "{name} のスタートアップを無効にしました"
                            )
                        };
                        self.notify_ok(msg, None);
                    }
                    Err(e) => {
                        let name = item.name.clone();
                        let msg = if on {
                            tf!(
                                "无法启用 {name}",
                                "Couldn't enable {name}",
                                "{name} を有効にできませんでした"
                            )
                        } else {
                            tf!(
                                "无法禁用 {name}",
                                "Couldn't disable {name}",
                                "{name} を無効にできませんでした"
                            )
                        };
                        self.notify_err(msg, Some(e));
                    }
                }
            }
            StartupAction::Reveal(path) => pulse_probe::reveal_in_explorer(&path),
            StartupAction::Properties(path) => control::show_properties(&path),
            StartupAction::Search(q) => {
                control::search_online(self.settings.search.url_prefix(), &q)
            }
            StartupAction::Copy(text) => ctx.copy_text(text),
            StartupAction::OpenFolder => {
                let _ = control::run("shell:startup", false);
            }
            StartupAction::Refresh => self.reload_startup(ctx),
        }
    }

    fn apply_service(&mut self, ctx: &egui::Context, action: ServiceAction) {
        use pulse_probe::services;
        type Job = Box<dyn FnOnce(&str) -> Result<(), String> + Send>;
        // ([进行中, 成功, 失败] 的通知, 服务名, 执行, 是否要先提示"正在…"——启停服务可能要等好几秒)
        let (msgs, name, run, slow): ([String; 3], String, Job, bool) = match action {
            ServiceAction::Start(n) => (
                [
                    tf!(
                        "正在启动服务 {n}…",
                        "Starting service {n}…",
                        "サービス {n} を開始しています…"
                    ),
                    tf!(
                        "已启动服务 {n}",
                        "Started service {n}",
                        "サービス {n} を開始しました"
                    ),
                    tf!(
                        "无法启动服务 {n}",
                        "Couldn't start service {n}",
                        "サービス {n} を開始できませんでした"
                    ),
                ],
                n,
                Box::new(services::start),
                true,
            ),
            ServiceAction::Stop(n) => (
                [
                    tf!(
                        "正在停止服务 {n}…",
                        "Stopping service {n}…",
                        "サービス {n} を停止しています…"
                    ),
                    tf!(
                        "已停止服务 {n}",
                        "Stopped service {n}",
                        "サービス {n} を停止しました"
                    ),
                    tf!(
                        "无法停止服务 {n}",
                        "Couldn't stop service {n}",
                        "サービス {n} を停止できませんでした"
                    ),
                ],
                n,
                Box::new(services::stop),
                true,
            ),
            ServiceAction::Restart(n) => (
                [
                    tf!(
                        "正在重新启动服务 {n}…",
                        "Restarting service {n}…",
                        "サービス {n} を再起動しています…"
                    ),
                    tf!(
                        "已重新启动服务 {n}",
                        "Restarted service {n}",
                        "サービス {n} を再起動しました"
                    ),
                    tf!(
                        "无法重新启动服务 {n}",
                        "Couldn't restart service {n}",
                        "サービス {n} を再起動できませんでした"
                    ),
                ],
                n,
                Box::new(services::restart),
                true,
            ),
            ServiceAction::SetStartType(n, st) => {
                let label = st.label();
                (
                    [
                        String::new(),
                        tf!(
                            "已将 {n} 的启动类型改为{label}",
                            "Changed the startup type of {n} to {label}",
                            "{n} のスタートアップの種類を「{label}」に変更しました"
                        ),
                        tf!(
                            "无法将 {n} 的启动类型改为{label}",
                            "Couldn't change the startup type of {n} to {label}",
                            "{n} のスタートアップの種類を「{label}」に変更できませんでした"
                        ),
                    ],
                    n,
                    Box::new(move |n: &str| services::set_start_type(n, st)),
                    false,
                )
            }
            ServiceAction::GotoProcess(pid) => {
                return self.show_processes(pid.to_string(), Some(Selection::Process(pid)));
            }
            ServiceAction::Copy(text) => return ctx.copy_text(text),
            ServiceAction::Search(q) => {
                return pulse_probe::control::search_online(self.settings.search.url_prefix(), &q);
            }
            ServiceAction::OpenManager => {
                if let Err(e) = pulse_probe::control::run("services.msc", false) {
                    self.notify_err(
                        t!(
                            "无法打开服务管理器",
                            "Couldn't open the Services app",
                            "サービス管理ツールを開けませんでした"
                        )
                        .into(),
                        Some(e),
                    );
                }
                return;
            }
        };
        let [progress, done, failed] = msgs;
        if slow {
            self.notify_ok(progress, None);
        }
        let shared = self.services_shared.clone();
        self.spawn_job(ctx, move || {
            let result = run(&name);
            shared.lock().unwrap().refresh = Some(Some(name));
            match result {
                Ok(()) => Toast::ok(done, None),
                Err(e) => Toast::err(failed, Some(e)),
            }
        });
    }

    fn notify_ok(&mut self, title: String, detail: Option<String>) {
        self.toasts.push(Toast::ok(title, detail));
    }

    fn notify_err(&mut self, title: String, detail: Option<String>) {
        self.toasts.push(Toast::err(title, detail));
    }

    /// 把一批操作结果汇总成一条通知。`done` / `failed` 是全部成功、失败时的标题。
    fn report(
        &mut self,
        results: Vec<pulse_probe::control::Result<()>>,
        done: String,
        failed: String,
    ) {
        let failures: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        match failures.first() {
            None => self.notify_ok(done, None),
            Some(e) if failures.len() == results.len() => {
                let detail = self.denied_hint(error_text(e), e);
                self.notify_err(failed, Some(detail));
            }
            Some(e) => {
                let detail = self.denied_hint(error_text(e), e);
                let (n, total) = (failures.len(), results.len());
                self.notify_err(
                    tf!(
                        "{failed}（{n} 个失败）",
                        "{failed} ({n} of {total} failed)",
                        "{failed} ({total} 件中 {n} 件が失敗)"
                    ),
                    Some(detail),
                );
            }
        }
    }

    fn denied_hint(&self, msg: String, e: &pulse_probe::WinError) -> String {
        // E_ACCESSDENIED
        if e.code().0 == 0x8007_0005_u32 as i32 && !self.snapshot.elevated {
            tf!(
                "{msg}。以管理员身份重启 Pulse 后再试。",
                "{msg}. Restart Pulse as administrator and try again.",
                "{msg}。Pulse を管理者として再起動してから、もう一度お試しください。"
            )
        } else {
            msg
        }
    }

    fn confirm_modal(&mut self, modal: Modal) {
        match modal {
            Modal::Kill(req) => self.kill_confirmed(req),
            Modal::Affinity(d) => {
                let r = pulse_probe::control::set_affinity(d.pid, d.mask);
                let label = &d.label;
                self.report(
                    vec![r],
                    tf!(
                        "已设置 {label} 的相关性",
                        "Set the affinity of {label}",
                        "{label} のプロセッサの関係を設定しました"
                    ),
                    tf!(
                        "无法设置 {label} 的相关性",
                        "Couldn't set the affinity of {label}",
                        "{label} のプロセッサの関係を設定できませんでした"
                    ),
                );
            }
            Modal::Run(d) => match pulse_probe::control::run(&d.command, d.admin) {
                Ok(()) => self.notify_ok(
                    tf!(
                        "已运行 {}",
                        "Started {}",
                        "{} を実行しました",
                        d.command.trim()
                    ),
                    None,
                ),
                // 出错时保留对话框，让用户改命令。
                Err(e) => {
                    self.modal = Some(Modal::Run(RunDialog {
                        error: Some(e),
                        ..d
                    }))
                }
            },
            Modal::User(a) => {
                let (r, id) = match a {
                    UserAction::Logoff(id, _) => (pulse_probe::sessions::logoff(id), id),
                    UserAction::Disconnect(id, _) => (pulse_probe::sessions::disconnect(id), id),
                };
                let (done, failed) = match a {
                    UserAction::Logoff(..) => (
                        tf!(
                            "已注销会话 {id}",
                            "Signed out session {id}",
                            "セッション {id} をサインアウトしました"
                        ),
                        tf!(
                            "无法注销会话 {id}",
                            "Couldn't sign out session {id}",
                            "セッション {id} をサインアウトできませんでした"
                        ),
                    ),
                    UserAction::Disconnect(..) => (
                        tf!(
                            "已断开连接会话 {id}",
                            "Disconnected session {id}",
                            "セッション {id} を切断しました"
                        ),
                        tf!(
                            "无法断开连接会话 {id}",
                            "Couldn't disconnect session {id}",
                            "セッション {id} を切断できませんでした"
                        ),
                    ),
                };
                self.sessions_at = None;
                self.report(vec![r], done, failed);
            }
        }
    }

    fn kill_confirmed(&mut self, req: KillRequest) {
        let results = req.pids.iter().map(|&pid| pulse_probe::kill(pid)).collect();
        let label = &req.label;
        self.report(
            results,
            tf!("已结束 {label}", "Ended {label}", "{label} を終了しました"),
            tf!(
                "无法结束 {label}",
                "Couldn't end {label}",
                "{label} を終了できませんでした"
            ),
        );
    }

    /// ↑/↓ 移动选中行，Home/End 跳到首尾，Enter 展开分组或切换到应用窗口，Delete 结束选中项。
    fn keyboard(&mut self, ui: &Ui) -> Option<Action> {
        let (up, down, home, end, enter, delete) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Delete),
            )
        });
        if delete {
            let (pids, label) = self.selected_pids()?;
            return Some(Action::Kill { pids, label });
        }
        if !(up || down || home || end || enter) {
            return None;
        }
        let selectable: Vec<(usize, Selection)> = self
            .table
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match *r {
                Row::Group(gi) => Some((i, Selection::Group(self.table.groups[gi].key.clone()))),
                Row::Member(pi) | Row::Single(pi) => {
                    Some((i, Selection::Process(self.snapshot.processes[pi].pid)))
                }
                Row::Section(..) => None,
            })
            .collect();
        if selectable.is_empty() {
            return None;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|s| selectable.iter().position(|(_, x)| x == s));
        if enter {
            return match self.selected.as_ref()? {
                Selection::Group(key) => Some(Action::Toggle(key.clone())),
                Selection::Process(pid) => {
                    let p = self.snapshot.processes.iter().find(|p| p.pid == *pid)?;
                    p.window.as_ref().map(|w| Action::SwitchTo(w.hwnd))
                }
            };
        }
        let last = selectable.len() - 1;
        let next = match (current, up, down, home, end) {
            (_, _, _, true, _) => 0,
            (_, _, _, _, true) => last,
            (None, ..) => 0,
            (Some(c), true, ..) => c.saturating_sub(1),
            (Some(c), _, true, ..) => (c + 1).min(last),
            (Some(c), ..) => c,
        };
        let (row, sel) = selectable[next].clone();
        self.selected = Some(sel);
        self.scroll_to = Some(row);
        None
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
                        &(0..snap.cpu_cores.len())
                            .map(|i| snap.cpu_info.is_efficient(i))
                            .collect::<Vec<_>>(),
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
                    .on_hover_text(t!("查看详情", "View details", "詳細を表示"))
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

    /// 标题行：品牌、页签、设置，接管标题栏时还有最小化 / 最大化 / 关闭。
    /// 空白处可拖动窗口（见 `titlebar`）。
    fn title_bar(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        let custom = titlebar::installed();
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TITLE_H), Sense::hover());
        let mut holes = Vec::new();
        let mut max_button = egui::Rect::NOTHING;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 4.0;

        widgets::brand_mark(ui, 20.0);
        ui.add_space(2.0);
        ui.label(
            RichText::new("Pulse")
                .size(13.5)
                .family(theme::semibold())
                .color(p.text),
        );
        ui.add_space(14.0);

        for (view, glyph, label) in [
            (
                View::Processes,
                Icon::List,
                t!("进程", "Processes", "プロセス"),
            ),
            (
                View::Performance,
                Icon::Activity,
                t!("性能", "Performance", "パフォーマンス"),
            ),
            (
                View::Services,
                Icon::Settings,
                t!("服务", "Services", "サービス"),
            ),
            (
                View::Startup,
                Icon::Rocket,
                t!("启动应用", "Startup apps", "スタートアップ アプリ"),
            ),
            (View::Users, Icon::Users, t!("用户", "Users", "ユーザー")),
        ] {
            let resp = tab(ui, self.view == view, glyph, label);
            holes.push(resp.rect);
            if resp.clicked() {
                self.view = view;
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if custom {
                max_button = caption_buttons(ui, &mut holes);
                ui.add_space(8.0);
            } else {
                ui.add_space(20.0);
            }
            let resp = tab(
                ui,
                self.view == View::Settings,
                Icon::Settings,
                t!("设置", "Settings", "設定"),
            );
            holes.push(resp.rect);
            if resp.clicked() {
                self.view = View::Settings;
            }
        });
        if custom {
            let caption =
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(f32::INFINITY, row.bottom()));
            titlebar::set_layout(ui.ctx(), caption, holes, max_button);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let elevated = self.snapshot.elevated;
        let target = self.selected_pids();
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::segmented(
                    ui,
                    &mut self.settings.grouped,
                    [
                        (Icon::Layers, t!("分组", "Grouped", "グループ")),
                        (Icon::List, t!("列表", "List", "一覧")),
                    ],
                ) {
                    self.dirty = true;
                    self.settings.save();
                }
                ui.add_space(4.0);
                let enabled = target.is_some();
                let resp = widgets::tool_button(
                    ui,
                    Icon::OctagonX,
                    t!("结束任务", "End task", "タスクを終了する"),
                    enabled,
                    true,
                );
                let resp = match &target {
                    Some((_, label)) => {
                        resp.on_hover_text(tf!("结束 {label}（Delete）", "End {label} (Delete)", "{label} を終了 (Delete)"))
                    }
                    None => resp.on_hover_text(t!(
                        "先在列表中选中一个进程或分组",
                        "Select a process or group in the list first", "先に一覧でプロセスまたはグループを選択してください"
                    )),
                };
                if enabled
                    && resp.clicked()
                    && let Some((pids, label)) = target.clone()
                {
                    let ctx = ui.ctx().clone();
                    self.apply(&ctx, Action::Kill { pids, label });
                }
                if widgets::tool_button(
                    ui,
                    Icon::Plus,
                    t!("运行新任务", "Run new task", "新しいタスクを実行する"),
                    true,
                    false,
                )
                .clicked()
                {
                    self.modal = Some(Modal::Run(RunDialog::default()));
                }
                if !elevated
                    && widgets::tool_button(
                        ui,
                        Icon::Shield,
                        t!("以管理员身份重启", "Restart as admin", "管理者として再起動"),
                        true,
                        false,
                    )
                    .on_hover_text(t!(
                        "启用逐进程网络统计，并允许结束更多系统进程",
                        "Enables per-process network stats and lets you end more system processes", "プロセスごとのネットワーク統計が有効になり、より多くのシステム プロセスを終了できるようになります"
                    ))
                    .clicked()
                    && pulse_probe::relaunch_elevated()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    // 右侧按钮先排，搜索框用剩下的宽度（日文等长文案下按钮更宽）。
                    let chip = t!("监听端口", "Listening ports", "リッスン ポート");
                    let width = (ui.available_width()
                        - widgets::pill_width(ui, chip)
                        - ui.spacing().item_spacing.x * 2.0)
                        .clamp(180.0, 320.0);
                    let resp = widgets::search_box(
                        ui,
                        &mut self.filter,
                        width,
                        t!(
                            "模糊搜索名称、描述、命令行，或 :端口",
                            "Search name, description, command, :port", "名前、説明、コマンド、:ポートで検索"
                        ),
                    );
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
                    if widgets::toggle_chip(
                        ui,
                        &mut self.listening_only,
                        Icon::Plug,
                        chip,
                    )
                    .on_hover_text(t!(
                        "只看正在监听 TCP 端口的进程。也可以直接搜索 :端口号",
                        "Show only processes listening on TCP ports. You can also search for :port", "TCP ポートをリッスンしているプロセスだけを表示します。「:ポート番号」で直接検索することもできます。"
                    ))
                    .clicked()
                    {
                        self.dirty = true;
                    }
                });
            });
        });
    }

    fn apply_settings(&mut self, ctx: &egui::Context, before: &Settings) {
        let s = &self.settings;
        if s.grouped != before.grouped {
            self.dirty = true;
        }
        if s.speed != before.speed {
            self.interval.store(s.speed.millis(), Ordering::Relaxed);
        }
        if s.theme != before.theme {
            ctx.set_theme(s.theme.preference());
        }
        if s.language != before.language {
            // 文字都是每帧按当前语言生成的（表格行只存下标），切换后重画一帧即可。
            pulse_probe::i18n::set_lang(s.language.resolve());
            self.dirty = true;
            ctx.request_repaint();
        }
        if s.always_on_top != before.always_on_top {
            let level = if s.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
        self.settings.save();
    }

    fn footer(&mut self, ui: &mut Ui) {
        let p = Palette::of(ui.ctx());
        let faint = |t: String| RichText::new(t).size(11.5).color(p.faint);
        ui.horizontal(|ui| {
            ui.label(faint(tf!(
                "{} 个进程 · 采集 {:.1} ms",
                "{} processes · sampled in {:.1} ms",
                "プロセス {} 個 · 収集 {:.1} ms",
                self.snapshot.processes.len(),
                self.snapshot.collect_time.as_secs_f64() * 1e3,
            )));
            if self.settings.speed == Speed::Paused {
                widgets::status_chip(
                    ui,
                    t!("已暂停更新", "Updates paused", "更新を一時停止中"),
                    p.muted,
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match &self.snapshot.net_status {
                    NetStatus::Running => {}
                    NetStatus::NeedsAdmin => {
                        ui.label(faint(
                            t!(
                                "逐进程网络统计需要管理员权限",
                                "Per-process network stats require administrator rights",
                                "プロセスごとのネットワーク統計には管理者権限が必要です"
                            )
                            .into(),
                        ));
                    }
                    NetStatus::Failed(e) => {
                        ui.label(faint(tf!(
                            "ETW 启动失败：{e}",
                            "ETW failed to start: {e}",
                            "ETW を開始できませんでした：{e}"
                        )));
                    }
                }
            });
        });
    }

    fn table(&mut self, ui: &mut Ui) {
        let actions = table::show(
            ui,
            table::View {
                snapshot: &self.snapshot,
                table: &self.table,
                selected: self.selected.as_ref(),
                sort: self.sort,
                descending: self.descending,
                grouped: self.settings.grouped,
                icons: &mut self.icons,
                scroll_to: self.scroll_to.take(),
                services: &self.services_by_pid,
            },
        );
        let ctx = ui.ctx().clone();
        for a in actions {
            self.apply(&ctx, a);
        }
        if let Some((sel, pos)) = self.menu.clone() {
            let (actions, open) =
                table::menu(&ctx, &sel, pos, &self.snapshot, &self.services_by_pid);
            if !open {
                self.menu = None;
            }
            for a in actions {
                self.apply(&ctx, a);
            }
        }
    }
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
                    grouped: self.settings.grouped,
                    listening_only: self.listening_only,
                    sort: self.sort,
                    descending: self.descending,
                    expanded: &self.expanded,
                },
            );
            self.dirty = false;
        }

        let p = Palette::of(ui.ctx());
        let custom = titlebar::installed();
        egui::Panel::top("top")
            .show_separator_line(false)
            .frame(Frame::NONE.fill(p.bg).inner_margin(Margin {
                left: 20,
                right: if custom { 0 } else { 20 },
                top: if custom { 0 } else { 8 },
                bottom: 12,
            }))
            .show(ui, |ui| {
                self.title_bar(ui);
                if self.view == View::Processes {
                    Frame::NONE
                        .inner_margin(Margin {
                            right: if custom { 20 } else { 0 },
                            ..Default::default()
                        })
                        .show(ui, |ui| {
                            ui.add_space(4.0);
                            self.cards(ui);
                            ui.add_space(16.0);
                            self.toolbar(ui);
                        });
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
            View::Settings => {
                let before = self.settings.clone();
                let elevated = self.snapshot.elevated;
                let replaced = self.taskmgr_replaced;
                let action = central
                    .show(ui, |ui| {
                        settings_view::show(ui, &mut self.settings, replaced, elevated)
                    })
                    .inner;
                if self.settings != before {
                    self.apply_settings(ui.ctx(), &before);
                }
                if let Some(settings_view::SettingsAction::SetTaskmgrReplaced(want)) = action {
                    match pulse_probe::os::set_taskmgr_replaced(want) {
                        Ok(()) => {
                            self.taskmgr_replaced = want;
                            let msg = if want {
                                t!(
                                    "已替换任务管理器：Ctrl+Shift+Esc 将打开 Pulse",
                                    "Replaced Task Manager: Ctrl+Shift+Esc now opens Pulse",
                                    "タスク マネージャーを置き換えました：Ctrl+Shift+Esc で Pulse が開きます"
                                )
                            } else {
                                t!(
                                    "已恢复系统任务管理器",
                                    "Restored the Windows Task Manager",
                                    "Windows のタスク マネージャーに戻しました"
                                )
                            };
                            self.notify_ok(msg.into(), None);
                        }
                        Err(e) => self.notify_err(
                            t!(
                                "无法修改任务管理器设置",
                                "Couldn't change the Task Manager setting",
                                "タスク マネージャーの設定を変更できませんでした"
                            )
                            .into(),
                            Some(e),
                        ),
                    }
                }
            }
            View::Users => {
                if self
                    .sessions_at
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
                {
                    self.sessions = pulse_probe::sessions::list();
                    self.sessions_at = Some(Instant::now());
                }
                let actions = central
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        users_view::show(ui, &self.sessions, &self.snapshot)
                    })
                    .inner;
                if let Some(a) = actions {
                    self.modal = Some(Modal::User(a));
                }
            }
            View::Startup => {
                if self.startup.is_none() && self.startup_rx.is_none() {
                    self.reload_startup(ui.ctx());
                }
                let actions = central
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        let Some(items) = &self.startup else {
                            ui.label(
                                RichText::new(t!(
                                    "正在读取启动项…",
                                    "Loading startup apps…",
                                    "スタートアップ アプリを読み込んでいます…"
                                ))
                                .color(p.muted),
                            );
                            return Vec::new();
                        };
                        startup_view::show(
                            ui,
                            items,
                            &mut self.startup_view,
                            &mut self.icons,
                            self.snapshot.elevated,
                        )
                    })
                    .inner;
                let ctx = ui.ctx().clone();
                for a in actions {
                    self.apply_startup(&ctx, a);
                }
            }
            View::Services => {
                let actions = central
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        if self.services.is_empty() {
                            services_view::loading(ui);
                            Vec::new()
                        } else {
                            services_view::show(ui, &self.services, &mut self.services_view)
                        }
                    })
                    .inner;
                let ctx = ui.ctx().clone();
                for a in actions {
                    self.apply_service(&ctx, a);
                }
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
                match action {
                    Some(perf::PerfAction::Search(name)) => self.show_processes(name, None),
                    Some(perf::PerfAction::Copy(text)) => {
                        ui.ctx().copy_text(text);
                        self.notify_ok(
                            t!(
                                "已复制设备摘要",
                                "Copied the device summary",
                                "デバイスの概要をコピーしました"
                            )
                            .into(),
                            None,
                        );
                    }
                    None => {}
                }
            }
        }

        let ctx = ui.ctx().clone();
        if let Some(modal) = &mut self.modal {
            let decision = match modal {
                Modal::Kill(req) => dialogs::confirm_kill(&ctx, req, &self.snapshot),
                Modal::Affinity(d) => dialogs::affinity(&ctx, d),
                Modal::Run(d) => dialogs::run_task(&ctx, d, self.snapshot.elevated),
                Modal::User(a) => {
                    let (title, body, button) = user_prompt(a);
                    dialogs::confirm(&ctx, "user", &title, body, button)
                }
            };
            match decision {
                Some(Decision::Confirm) => {
                    let modal = self.modal.take().unwrap();
                    self.confirm_modal(modal);
                }
                Some(Decision::Cancel) => self.modal = None,
                None => {}
            }
        }
        if let Some(rx) = &self.startup_rx
            && let Ok(items) = rx.try_recv()
        {
            self.startup = Some(items);
            self.startup_rx = None;
        }
        while let Ok(t) = self.jobs.1.try_recv() {
            self.toasts.push(t);
        }
        dialogs::toasts(&ctx, &mut self.toasts);
        if self.view == View::Processes
            && self.modal.is_none()
            && ui.ctx().memory(|m| m.focused().is_none())
            && let Some(action) = self.keyboard(ui)
        {
            let ctx = ui.ctx().clone();
            self.apply(&ctx, action);
        }
    }
}

/// 标题行里的页签：图标 + 文字，选中时有底色。
fn tab(ui: &mut Ui, on: bool, glyph: Icon, label: &str) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let galley =
        ui.painter()
            .layout_no_wrap(label.into(), egui::FontId::proportional(14.0), p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 44.0, 32.0), Sense::click());
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
        pos2(rect.left() + 18.0, rect.center().y),
        16.0,
        if on { p.accent } else { fg },
    );
    ui.painter().galley(
        pos2(rect.left() + 31.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp
}

/// 标题行高度。
const TITLE_H: f32 = 44.0;

/// 对齐到物理像素中心，让 1px 线条清晰。
fn pixel_center(ui: &Ui, p: egui::Pos2) -> egui::Pos2 {
    let painter = ui.painter();
    pos2(
        painter.round_to_pixel_center(p.x),
        painter.round_to_pixel_center(p.y),
    )
}

/// 自绘的最小化 / 最大化 / 关闭按钮（从右往左放），返回最大化按钮的区域。
/// 最大化按钮的鼠标事件由系统送到非客户区，状态从 `titlebar` 读取。
fn caption_buttons(ui: &mut Ui, holes: &mut Vec<egui::Rect>) -> egui::Rect {
    let p = Palette::of(ui.ctx());
    let (focused, maximized) = ui.input(|i| {
        let v = i.viewport();
        (v.focused.unwrap_or(true), v.maximized.unwrap_or(false))
    });
    let fg = if focused { p.text } else { p.faint };
    let size = vec2(46.0, TITLE_H);
    ui.spacing_mut().item_spacing.x = 0.0;
    let stroke = |c| egui::Stroke::new(1.0, c);

    // 关闭
    let (close, resp) = ui.allocate_exact_size(size, Sense::click());
    holes.push(close);
    let hot = resp.hovered() || resp.is_pointer_button_down_on();
    if hot {
        let red = egui::Color32::from_rgb(0xc4, 0x2b, 0x1c);
        let fill = if resp.is_pointer_button_down_on() {
            red.gamma_multiply(0.85)
        } else {
            red
        };
        ui.painter()
            .rect_filled(close, egui::CornerRadius::ZERO, fill);
    }
    let c = pixel_center(ui, close.center());
    let color = if hot { egui::Color32::WHITE } else { fg };
    for (a, b) in [
        (vec2(-5.0, -5.0), vec2(5.0, 5.0)),
        (vec2(-5.0, 5.0), vec2(5.0, -5.0)),
    ] {
        ui.painter().line_segment([c + a, c + b], stroke(color));
    }
    if resp.clicked() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }

    // 最大化 / 还原
    let (max, _) = ui.allocate_exact_size(size, Sense::hover());
    let (hover, pressed) = titlebar::max_button_state();
    if hover || pressed {
        ui.painter().rect_filled(
            max,
            egui::CornerRadius::ZERO,
            if pressed { p.track } else { p.hover },
        );
    }
    let c = pixel_center(ui, max.center());
    if maximized {
        let front = egui::Rect::from_min_size(c + vec2(-5.0, -3.0), vec2(8.0, 8.0));
        ui.painter().rect_stroke(
            front,
            egui::CornerRadius::same(1),
            stroke(fg),
            egui::StrokeKind::Middle,
        );
        ui.painter().add(egui::Shape::line(
            vec![
                c + vec2(-3.0, -3.0),
                c + vec2(-3.0, -5.0),
                c + vec2(5.0, -5.0),
                c + vec2(5.0, 3.0),
                c + vec2(3.0, 3.0),
            ],
            stroke(fg),
        ));
    } else {
        let r = egui::Rect::from_center_size(c, vec2(10.0, 10.0));
        ui.painter().rect_stroke(
            r,
            egui::CornerRadius::same(1),
            stroke(fg),
            egui::StrokeKind::Middle,
        );
    }

    // 最小化
    let (min, resp) = ui.allocate_exact_size(size, Sense::click());
    holes.push(min);
    if resp.hovered() || resp.is_pointer_button_down_on() {
        let fill = if resp.is_pointer_button_down_on() {
            p.track
        } else {
            p.hover
        };
        ui.painter()
            .rect_filled(min, egui::CornerRadius::ZERO, fill);
    }
    let c = pixel_center(ui, min.center());
    ui.painter()
        .line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke(fg));
    if resp.clicked() {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    max
}

/// 注销 / 断开连接确认框的 (标题, 说明, 按钮)。
fn user_prompt(a: &UserAction) -> (String, &'static str, &'static str) {
    match a {
        UserAction::Disconnect(_, name) => (
            tf!(
                "断开 {name} 的连接？",
                "Disconnect {name}?",
                "{name} を切断しますか？"
            ),
            t!(
                "会话会保持登录状态，程序继续运行，用户可以稍后重新连接。",
                "The session stays signed in and its programs keep running. The user can reconnect later.",
                "セッションはサインインしたままで、プログラムも実行を続けます。ユーザーは後で再接続できます。"
            ),
            t!("断开连接", "Disconnect", "切断"),
        ),
        UserAction::Logoff(_, name) => (
            tf!(
                "注销 {name}？",
                "Sign out {name}?",
                "{name} をサインアウトしますか？"
            ),
            t!(
                "该用户所有打开的程序都会被关闭，未保存的数据将会丢失。",
                "All of this user's open programs will be closed. Unsaved data will be lost.",
                "このユーザーが開いているプログラムはすべて閉じられます。保存されていないデータは失われます。"
            ),
            t!("注销", "Sign out", "サインアウト"),
        ),
    }
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

/// 命令行参数作为初始搜索词，例如 `pulse :5173`。
/// 通过"替换任务管理器"启动时命令行是 `pulse.exe --taskmgr <taskmgr.exe 路径> /7 …`，标记之后的都忽略。
fn initial_filter() -> String {
    std::env::args()
        .skip(1)
        .take_while(|a| a != pulse_probe::os::TASKMGR_FLAG)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 系统错误的可读描述（为空时退回错误码）。
fn error_text(e: &pulse_probe::WinError) -> String {
    let msg = e.message().trim().trim_end_matches('。').to_string();
    if msg.is_empty() {
        tf!(
            "错误 0x{:08X}",
            "Error 0x{:08X}",
            "エラー 0x{:08X}",
            e.code().0
        )
    } else {
        msg
    }
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
        s if s < 60 => tf!("{s} 秒", "{s} s", "{s} 秒"),
        s if s < 3600 => tf!("{} 分钟", "{} min", "{} 分", s / 60),
        s if s < 86_400 => tf!(
            "{} 小时 {} 分钟",
            "{} h {} min",
            "{} 時間 {} 分",
            s / 3600,
            s / 60 % 60
        ),
        s => tf!(
            "{} 天 {} 小时",
            "{} d {} h",
            "{} 日 {} 時間",
            s / 86_400,
            s / 3600 % 24
        ),
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
        fmt_rate_full(b)
    }
}

#[cfg(test)]
mod tests {
    use super::cmd_summary;

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
}
