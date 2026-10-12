//! 进程表格：表头排序、分段（应用 / 后台进程）、分组展开、状态标签、右键菜单、悬停详情。

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, FontId, Layout, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2,
};
use egui_extras::{Column, TableBuilder};
use pulse_probe::control::Priority;
use pulse_probe::ports::Proto;
use pulse_probe::{NetStatus, Process, Snapshot, t, tf};

use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::model::{self, Metrics, Row, SortKey, Table};
use crate::theme::{Palette, metric, semibold};
use crate::{
    Action, Selection, cmd_summary, fmt_bytes, fmt_cpu_time, fmt_rate, fmt_uptime, same_name,
    widgets,
};

const ICON: f32 = 16.0;
/// 分组模式下为展开箭头预留的宽度，也是成员行的缩进。
const INDENT: f32 = 16.0;

pub struct View<'a> {
    pub snapshot: &'a Snapshot,
    pub table: &'a Table,
    pub selected: Option<&'a Selection>,
    pub sort: SortKey,
    pub descending: bool,
    pub grouped: bool,
    pub icons: &'a mut Icons,
    /// 键盘导航后需要滚动到的行。
    pub scroll_to: Option<usize>,
    /// pid → 该进程托管的服务名（svchost 等）。
    pub services: &'a std::collections::HashMap<u32, Vec<String>>,
}

pub fn show(ui: &mut Ui, v: View) -> Vec<Action> {
    let p = Palette::of(ui.ctx());
    let View {
        snapshot,
        table,
        selected,
        sort,
        descending,
        grouped,
        icons,
        scroll_to,
        services,
    } = v;
    let mut actions = Vec::new();
    let procs = &snapshot.processes;
    let cores = snapshot.cpu_count.max(1) as f32;
    let net_ok = snapshot.net_status == NetStatus::Running;
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
    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, None);
    }
    builder
        .header(34.0, |mut h| {
            let header = std::iter::once((t!("名称", "Name"), SortKey::Name, false))
                .chain(cols.iter().map(|c| (c.label(), c.key(), true)));
            for (label, key, right_aligned) in header {
                h.col(|ui| {
                    header_cell(
                        ui,
                        label,
                        key,
                        right_aligned,
                        sort,
                        descending,
                        net_ok,
                        &mut actions,
                    )
                });
            }
        })
        .body(|body| {
            body.rows(crate::theme::ROW_HEIGHT, table.rows.len(), |mut row| {
                let index = row.index();
                let r = table.rows[index];
                if let Row::Section(section, n) = r {
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        ui.painter().text(
                            pos2(rect.left() + 4.0, rect.center().y + 3.0),
                            egui::Align2::LEFT_CENTER,
                            format!("{} ({n})", section.label()),
                            FontId::new(12.5, semibold()),
                            p.muted,
                        );
                    });
                    for _ in &cols {
                        row.col(|_| {});
                    }
                    return;
                }
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
                    Row::Section(..) => unreachable!(),
                };
                row.set_selected(selected == Some(&sel));
                let members: &[usize] = match r {
                    Row::Group(gi) => &table.groups[gi].members,
                    Row::Member(i) | Row::Single(i) => std::slice::from_ref(&procs_index[i]),
                    Row::Section(..) => &[],
                };
                let member_procs: Vec<&Process> = members.iter().map(|&i| &procs[i]).collect();
                // 分组里的成员共用同一个描述，改为显示命令行摘要，才看得出各自在跑什么（vite.js、main.js…）。
                let secondary: Option<String> = match r {
                    Row::Member(i) => procs[i].command_line.as_deref().and_then(cmd_summary),
                    _ => None,
                }
                .or_else(|| {
                    member_procs
                        .iter()
                        .find_map(|pr| pr.description.clone())
                        .filter(|d| !same_name(d, name))
                        .map(|d| d.to_string())
                });
                let tcp = tcp_ports(&member_procs);
                let status = Status::of(&member_procs);

                row.col(|ui| {
                    name_cell(
                        ui,
                        NameCell {
                            name,
                            path: path.as_ref(),
                            indent,
                            count,
                            expanded: is_expanded(table, index),
                            secondary: secondary.as_deref(),
                            tcp: &tcp,
                            status,
                            icons,
                        },
                    )
                });
                for c in &cols {
                    row.col(|ui| value_cell(ui, *c, &sel, &metrics, cores, net_ok));
                }

                let resp = row.response();
                if resp.clicked() || resp.secondary_clicked() {
                    actions.push(Action::Select(sel.clone()));
                    if let (true, Selection::Group(key)) = (resp.clicked(), &sel) {
                        actions.push(Action::Toggle(key.clone()));
                    }
                }
                // 双击应用切到它的窗口。
                if resp.double_clicked()
                    && let Some(w) = member_procs.iter().find_map(|pr| pr.window.as_ref())
                {
                    actions.push(Action::SwitchTo(w.hwnd));
                }
                // 右键菜单由 App 按选中项单独绘制，固定在点击位置，不跟着行移动。
                if resp.secondary_clicked()
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    actions.push(Action::OpenMenu(sel.clone(), pos));
                }
                if let Row::Member(i) | Row::Single(i) = r {
                    let pr = &procs[i];
                    resp.on_hover_ui(|ui| tooltip(ui, pr, net_ok, services.get(&pr.pid)));
                }
            });
        });
    actions
}

#[allow(clippy::too_many_arguments)]
fn header_cell(
    ui: &mut Ui,
    label: &str,
    key: SortKey,
    right_aligned: bool,
    sort: SortKey,
    descending: bool,
    net_ok: bool,
    actions: &mut Vec<Action>,
) {
    let p = Palette::of(ui.ctx());
    let active = sort == key;
    let text = RichText::new(label)
        .size(12.0)
        .color(if active { p.text } else { p.muted });
    let arrow = |ui: &mut Ui| {
        if active {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            let glyph = if descending {
                Icon::ArrowDown
            } else {
                Icon::ArrowUp
            };
            widgets::icon(ui, glyph, r.center(), 13.0, p.text);
        }
    };
    let mut resp = widgets::table_header(ui, text, right_aligned, arrow);
    if key == SortKey::Network && !net_ok {
        resp = resp.on_hover_text(t!(
            "逐进程网络统计需要管理员权限",
            "Per-process network stats require administrator rights"
        ));
    }
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        actions.push(Action::Sort(key));
    }
}

/// 行（或分组内任一成员）的状态。
#[derive(Clone, Copy, Default)]
struct Status {
    suspended: bool,
    hung: bool,
    efficiency: bool,
}

impl Status {
    fn of(procs: &[&Process]) -> Self {
        Self {
            // 分组只有全部成员都挂起才算挂起；无响应与效率模式只要有一个就提示。
            suspended: !procs.is_empty() && procs.iter().all(|p| p.suspended),
            hung: procs
                .iter()
                .any(|p| p.window.as_ref().is_some_and(|w| w.hung)),
            efficiency: procs.iter().any(|p| p.efficiency),
        }
    }
}

struct NameCell<'a> {
    name: &'a str,
    path: Option<&'a Arc<str>>,
    indent: f32,
    count: Option<usize>,
    expanded: bool,
    secondary: Option<&'a str>,
    tcp: &'a [u16],
    status: Status,
    icons: &'a mut Icons,
}

fn name_cell(ui: &mut Ui, c: NameCell) {
    let p = Palette::of(ui.ctx());
    let rect = ui.max_rect();
    let cy = rect.center().y;
    let mut x = rect.left() + 4.0 + c.indent;
    if c.count.is_some() {
        widgets::chevron(ui, pos2(x + 6.0, cy), c.expanded, p.muted);
        x += INDENT;
    }
    let icon_rect = Rect::from_min_size(pos2(x, cy - ICON / 2.0), vec2(ICON, ICON));
    match c.path.and_then(|path| c.icons.get(path)) {
        Some(tex) => {
            ui.painter().image(
                tex.id(),
                icon_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => widgets::avatar(ui.painter(), icon_rect, c.name),
    }
    x += ICON + 10.0;
    let text_rect = Rect::from_min_max(pos2(x, rect.top()), rect.right_bottom());
    // 右侧先放端口标签，剩下的空间给名称、状态与描述，描述过长时截断。
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // 至少给名称留 140px，剩下的空间每 ~54px 放一个端口标签，最多 3 个。
            let max_chips = ((text_rect.width() - 140.0) / 54.0).floor().clamp(0.0, 3.0) as usize;
            if c.tcp.len() > max_chips {
                ui.label(
                    RichText::new(format!("+{}", c.tcp.len() - max_chips))
                        .size(11.0)
                        .color(p.faint),
                );
            }
            for port in c.tcp.iter().take(max_chips).rev() {
                widgets::port_chip(ui, *port);
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add(egui::Label::new(RichText::new(c.name).color(p.text)).truncate());
                if let Some(n) = c.count {
                    ui.label(RichText::new(format!("{n}")).size(11.5).color(p.faint));
                }
                if c.status.efficiency {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    widgets::icon(ui, Icon::Leaf, r.center(), 13.0, metric::GPU);
                }
                if c.status.hung {
                    widgets::status_chip(ui, t!("无响应", "Not responding"), p.danger);
                }
                if c.status.suspended {
                    widgets::status_chip(ui, t!("已挂起", "Suspended"), p.muted);
                }
                if let Some(d) = c.secondary {
                    ui.add(egui::Label::new(RichText::new(d).size(12.0).color(p.faint)).truncate());
                }
            });
        },
    );
}

fn value_cell(ui: &mut Ui, c: Col, sel: &Selection, m: &Metrics, cores: f32, net_ok: bool) {
    let p = Palette::of(ui.ctx());
    match c {
        Col::Pid => {
            let text = match sel {
                Selection::Process(pid) => pid.to_string(),
                Selection::Group(_) => String::new(),
            };
            widgets::right_label(ui, RichText::new(text).color(p.faint));
        }
        Col::Cpu => {
            // 两个核心满载即视为最热，否则多核机器上永远看不出颜色。
            widgets::heat(ui, m.cpu * cores / 200.0, metric::CPU);
            widgets::right_label(ui, value(&p, format!("{:.1}%", m.cpu), m.cpu < 0.05));
        }
        Col::CpuTime => {
            // 不足 1 秒的视为几乎没用过 CPU，淡显。
            widgets::right_label(
                ui,
                value(&p, fmt_cpu_time(m.cpu_time), m.cpu_time < 10_000_000),
            );
        }
        Col::Memory => {
            widgets::heat(ui, m.memory as f32 / (4u64 << 30) as f32, metric::MEMORY);
            widgets::right_label(ui, value(&p, fmt_bytes(m.memory), false));
        }
        Col::Gpu => {
            widgets::heat(ui, m.gpu / 60.0, metric::GPU);
            widgets::right_label(ui, value(&p, format!("{:.1}%", m.gpu), m.gpu < 0.05));
        }
        Col::Vram => {
            widgets::heat(ui, m.vram as f32 / (4u64 << 30) as f32, metric::GPU);
            let text = if m.vram == 0 {
                "—".to_string()
            } else {
                fmt_bytes(m.vram)
            };
            widgets::right_label(ui, value(&p, text, m.vram == 0));
        }
        Col::Disk => {
            widgets::heat(ui, m.disk as f32 / 50e6, metric::DISK);
            widgets::right_label(ui, value(&p, fmt_rate(m.disk), m.disk == 0));
        }
        Col::Network if net_ok => {
            widgets::heat(ui, m.net() as f32 / 10e6, metric::NETWORK);
            widgets::right_label(ui, value(&p, fmt_rate(m.net()), m.net() == 0));
        }
        Col::Network => widgets::right_label(ui, RichText::new("—").color(p.faint)),
    }
}

/// 一组进程监听的 TCP 端口（去重、升序）。
fn tcp_ports(members: &[&Process]) -> Vec<u16> {
    let mut tcp: Vec<u16> = members
        .iter()
        .flat_map(|pr| {
            pr.ports
                .iter()
                .filter(|x| x.proto == Proto::Tcp)
                .map(|x| x.port)
        })
        .collect();
    tcp.sort_unstable();
    tcp.dedup();
    tcp
}

/// 进程右键菜单：固定在 `pos`，内容只由 `sel` 决定，与表格当前的行顺序无关。
/// 返回 (选中的操作, 菜单是否继续显示)。进程已退出、按 Esc 或点击菜单外时关闭。
pub fn menu(
    ctx: &egui::Context,
    sel: &Selection,
    pos: egui::Pos2,
    snapshot: &Snapshot,
    services: &std::collections::HashMap<u32, Vec<String>>,
) -> (Vec<Action>, bool) {
    let members: Vec<&Process> = match sel {
        Selection::Process(pid) => snapshot
            .processes
            .iter()
            .filter(|p| p.pid == *pid)
            .collect(),
        Selection::Group(key) => snapshot
            .processes
            .iter()
            .filter(|p| p.name.to_lowercase() == *key)
            .collect(),
    };
    let Some(first) = members.first() else {
        return (Vec::new(), false);
    };
    let tcp = tcp_ports(&members);
    let mut actions = Vec::new();
    let open = widgets::floating_menu(ctx, "process-menu", pos, 250.0, |ui| {
        context_menu(
            ui,
            MenuCtx {
                name: &first.name,
                sel,
                members: &members,
                path: first.path.as_ref(),
                tcp: &tcp,
                snapshot,
                services,
                actions: &mut actions,
            },
        );
        !actions.is_empty()
    });
    (actions, open)
}

struct MenuCtx<'a> {
    name: &'a str,
    sel: &'a Selection,
    members: &'a [&'a Process],
    path: Option<&'a Arc<str>>,
    tcp: &'a [u16],
    snapshot: &'a Snapshot,
    services: &'a std::collections::HashMap<u32, Vec<String>>,
    actions: &'a mut Vec<Action>,
}

fn context_menu(ui: &mut Ui, m: MenuCtx) {
    let _p = Palette::of(ui.ctx());
    let MenuCtx {
        name,
        sel,
        members,
        path,
        tcp,
        snapshot,
        services,
        actions,
    } = m;
    let pids: Vec<u32> = members.iter().map(|pr| pr.pid).collect();
    let window = members.iter().find_map(|pr| pr.window.as_ref());
    // (通知里的称呼, 菜单标题, 结束按钮文字)
    let (label, heading, kill_label) = match sel {
        Selection::Process(pid) => (
            format!("{name} ({pid})"),
            format!("{name} · {pid}"),
            t!("结束进程", "End task").to_string(),
        ),
        Selection::Group(_) => {
            let n = pids.len();
            (
                tf!("{name} 的 {n} 个进程", "{name} ({n} processes)"),
                tf!("{name} · {n} 个进程", "{name} · {n} processes"),
                tf!("结束全部 {n} 个进程", "End all {n} processes"),
            )
        }
    };
    widgets::menu_heading(ui, &heading, 0.0);

    // 窗口相关
    if let Some(w) = window {
        if widgets::menu_item(ui, Icon::Window, t!("切换到", "Switch to"), false).clicked() {
            actions.push(Action::SwitchTo(w.hwnd));
        }
        let with_windows: Vec<u32> = members
            .iter()
            .filter(|pr| pr.window.is_some())
            .map(|pr| pr.pid)
            .collect();
        if widgets::menu_item(
            ui,
            Icon::WindowClose,
            t!("关闭窗口（正常退出）", "Close window"),
            false,
        )
        .on_hover_text(t!(
            "像点关闭按钮一样请求程序退出，程序可以先保存或询问",
            "Asks the program to exit, like clicking its close button. It can save or ask first"
        ))
        .clicked()
        {
            actions.push(Action::Close {
                pids: with_windows,
                label: label.clone(),
            });
        }
    }
    // 结束
    if widgets::menu_item(ui, Icon::OctagonX, &kill_label, true).clicked() {
        actions.push(Action::Kill {
            pids: pids.clone(),
            label: label.clone(),
        });
    }
    if let Selection::Process(pid) = sel {
        let tree = model::descendants(snapshot, *pid);
        if tree.len() > 1
            && widgets::menu_item(
                ui,
                Icon::GitBranch,
                &tf!("结束进程树（{} 个）", "End process tree ({})", tree.len()),
                true,
            )
            .clicked()
        {
            // 先结束子进程，避免父进程退出后孩子被重新托管。
            let pids = tree.into_iter().rev().collect();
            actions.push(Action::Kill {
                pids,
                label: tf!("{name} 及其子进程", "{name} and its child processes"),
            });
        }
    }
    ui.separator();

    // 调度
    let all_suspended = members.iter().all(|pr| pr.suspended);
    let (glyph, text) = if all_suspended {
        (Icon::Play, t!("恢复", "Resume"))
    } else {
        (Icon::Pause, t!("挂起", "Suspend"))
    };
    if widgets::menu_item(ui, glyph, text, false)
        .on_hover_text(t!(
            "挂起会冻结进程的所有线程，恢复前它不会响应",
            "Suspending freezes all of the process's threads. It won't respond until resumed"
        ))
        .clicked()
    {
        actions.push(Action::Suspend {
            pids: pids.clone(),
            label: label.clone(),
            resume: all_suspended,
        });
    }
    let eff = members.iter().all(|pr| pr.efficiency);
    if widgets::menu_check(ui, Some(Icon::Leaf), t!("效率模式", "Efficiency mode"), eff)
        .on_hover_text(t!(
            "降低优先级并启用 EcoQoS，让进程省电、少抢前台资源",
            "Lowers priority and enables EcoQoS to save power and leave more resources for foreground apps"
        ))
        .clicked()
    {
        actions.push(Action::Efficiency {
            pids: pids.clone(),
            label: label.clone(),
            on: !eff,
        });
    }
    if let Selection::Process(pid) = sel {
        let current = members.first().map(|pr| pr.priority);
        widgets::submenu(
            ui,
            Icon::Gauge,
            t!("设置优先级", "Set priority"),
            160.0,
            |ui| {
                for prio in Priority::ALL {
                    if widgets::menu_check(ui, None, prio.label(), current == Some(prio)).clicked()
                    {
                        actions.push(Action::Priority {
                            pid: *pid,
                            label: label.clone(),
                            priority: prio,
                        });
                    }
                }
            },
        );
        if widgets::menu_item(ui, Icon::Cpu, t!("设置相关性…", "Set affinity…"), false).clicked()
        {
            actions.push(Action::Affinity {
                pid: *pid,
                label: label.clone(),
            });
        }
    }
    ui.separator();

    // 文件与诊断
    if let Selection::Process(pid) = sel {
        if widgets::menu_item(
            ui,
            Icon::Download,
            t!("创建转储文件", "Create dump file"),
            false,
        )
        .on_hover_text(t!(
            "写完整内存转储到临时目录，供调试器分析",
            "Writes a full memory dump to the temp folder for debugging"
        ))
        .clicked()
        {
            actions.push(Action::Dump {
                pid: *pid,
                name: name.to_string(),
            });
        }
        if services.contains_key(pid)
            && widgets::menu_item(ui, Icon::Settings, t!("转到服务", "Go to services"), false)
                .clicked()
        {
            actions.push(Action::GotoServices(*pid));
        }
    }
    if let Some(path) = path {
        if widgets::menu_item(
            ui,
            Icon::FolderOpen,
            t!("打开文件位置", "Open file location"),
            false,
        )
        .clicked()
        {
            actions.push(Action::Reveal(path.clone()));
        }
        if widgets::menu_item(ui, Icon::Info, t!("属性", "Properties"), false).clicked() {
            actions.push(Action::Properties(path.clone()));
        }
    }
    if widgets::menu_item(ui, Icon::Globe, t!("在线搜索", "Search online"), false).clicked() {
        actions.push(Action::SearchOnline(name.to_string()));
    }

    widgets::submenu(ui, Icon::Copy, t!("复制", "Copy"), 170.0, |ui| {
        let mut copies: Vec<(Icon, &str, String)> =
            vec![(Icon::Copy, t!("名称", "Name"), name.to_string())];
        if let Selection::Process(pid) = sel {
            copies.push((Icon::Hash, "PID", pid.to_string()));
            if let Some(cmd) = members.first().and_then(|pr| pr.command_line.clone()) {
                copies.push((
                    Icon::Terminal,
                    t!("命令行", "Command line"),
                    cmd.to_string(),
                ));
            }
        }
        if let Some(path) = path {
            copies.push((Icon::FolderOpen, t!("路径", "Path"), path.to_string()));
        }
        if !tcp.is_empty() {
            let ports = tcp
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            copies.push((Icon::Plug, t!("监听端口", "Listening ports"), ports));
        }
        for (glyph, label, value) in copies {
            if widgets::menu_item(ui, glyph, label, false).clicked() {
                actions.push(Action::Copy(value));
            }
        }
    });
}

fn tooltip(ui: &mut Ui, pr: &Process, net_ok: bool, services: Option<&Vec<String>>) {
    let p = Palette::of(ui.ctx());
    ui.set_max_width(520.0);
    let small = |t: String| RichText::new(t).size(11.5).color(p.muted);
    ui.label(RichText::new(&pr.name).strong());
    if let Some(w) = &pr.window {
        ui.label(small(tf!("窗口：{}", "Window: {}", w.title)));
    }
    if let Some(d) = &pr.description {
        ui.label(small(d.to_string()));
    }
    let mut state = vec![tf!("优先级 {}", "Priority {}", pr.priority.label())];
    if pr.suspended {
        state.push(t!("已挂起", "Suspended").into());
    }
    if pr.efficiency {
        state.push(t!("效率模式", "Efficiency mode").into());
    }
    if pr.window.as_ref().is_some_and(|w| w.hung) {
        state.push(t!("无响应", "Not responding").into());
    }
    ui.label(small(tf!(
        "PID {} · 线程 {} · 句柄 {} · {}",
        "PID {} · {} threads · {} handles · {}",
        pr.pid,
        pr.threads,
        pr.handles,
        state.join(" · ")
    )));
    ui.label(small(tf!(
        "已运行 {} · CPU 时间 {}（用户 {} · 内核 {}）",
        "Running for {} · CPU time {} (user {} · kernel {})",
        fmt_uptime(pr.create_time),
        fmt_cpu_time(pr.user_time + pr.kernel_time),
        fmt_cpu_time(pr.user_time),
        fmt_cpu_time(pr.kernel_time),
    )));
    if pr.gpu_dedicated + pr.gpu_shared > 0 {
        ui.label(small(tf!(
            "显存 {} · 共享 GPU 内存 {}",
            "Dedicated GPU memory {} · shared {}",
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
        ui.label(small(tf!("监听 TCP {t}", "Listening on TCP {t}")));
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
    if let Some(svcs) = services {
        let shown: Vec<&str> = svcs.iter().take(8).map(String::as_str).collect();
        let more = if svcs.len() > 8 {
            tf!(" 等 {} 个", " ({} total)", svcs.len())
        } else {
            String::new()
        };
        ui.label(small(tf!(
            "服务：{}{more}",
            "Services: {}{more}",
            shown.join(t!("、", ", "))
        )));
    }
    if let Some(cmd) = &pr.command_line {
        ui.add_space(4.0);
        let cmd: String = cmd.chars().take(600).collect();
        ui.label(RichText::new(cmd).monospace().size(11.0).color(p.text));
    } else if let Some(path) = &pr.path {
        ui.label(small(path.to_string()));
    }
}

/// 分组行之后紧跟成员行即视为已展开（搜索时会自动展开）。
fn is_expanded(table: &Table, index: usize) -> bool {
    matches!(table.rows.get(index + 1), Some(Row::Member(_)))
}

fn value(p: &Palette, text: String, zero: bool) -> RichText {
    RichText::new(text).color(if zero { p.faint } else { p.text })
}

/// 表格里的数值列。宽度不够时按优先级从低到高依次隐藏，保证名称列至少 `MIN_NAME` 宽。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Col {
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
    pub const ALL: [Col; 8] = [
        Col::Pid,
        Col::Cpu,
        Col::CpuTime,
        Col::Memory,
        Col::Gpu,
        Col::Vram,
        Col::Disk,
        Col::Network,
    ];
    pub const MIN_NAME: f32 = 260.0;

    fn width(self) -> f32 {
        match self {
            Col::Pid | Col::Gpu => 52.0,
            Col::Cpu => 56.0,
            Col::CpuTime | Col::Vram => 72.0,
            Col::Memory => 76.0,
            Col::Disk | Col::Network => 80.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Col::Pid => "PID",
            Col::Cpu => "CPU",
            Col::CpuTime => t!("CPU 时间", "CPU time"),
            Col::Memory => t!("内存", "Memory"),
            Col::Gpu => "GPU",
            Col::Vram => t!("显存", "VRAM"),
            Col::Disk => t!("磁盘", "Disk"),
            Col::Network => t!("网络", "Network"),
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

    pub fn visible(available: f32, spacing: f32) -> Vec<Col> {
        let mut cols: Vec<Col> = Col::ALL.to_vec();
        let name_width =
            |cols: &[Col]| available - cols.iter().map(|c| c.width() + spacing).sum::<f32>();
        while name_width(&cols) < Col::MIN_NAME && cols.len() > 2 {
            let drop = *cols.iter().min_by_key(|c| c.priority()).unwrap();
            cols.retain(|&c| c != drop);
        }
        cols
    }
}

#[cfg(test)]
mod tests {
    use super::Col;

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
