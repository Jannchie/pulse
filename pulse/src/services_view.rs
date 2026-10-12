//! 服务页：搜索、状态筛选、启动 / 停止 / 重启、修改启动类型、在服务与进程之间跳转。

use eframe::egui::{self, Align, Align2, FontId, Layout, RichText, Sense, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use pulse_probe::i18n::{Lang, lang};
use pulse_probe::services::{Service, StartType, State};
use pulse_probe::{t, tf};

use crate::icons_gen::Icon;
use crate::theme::{Palette, metric, semibold};
use crate::widgets;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Running,
    Stopped,
}

#[derive(Default)]
pub struct ServicesState {
    pub filter: String,
    pub state: Filter,
    pub selected: Option<String>,
    /// 打开着的右键菜单：服务名与位置。
    menu: Option<(String, egui::Pos2)>,
    /// 过滤结果缓存：(列表指针, 搜索词, 状态筛选) → 行下标。鼠标移动时每秒上百帧，不必每帧重算。
    rows: Option<(usize, String, Filter, Vec<usize>)>,
}

pub enum ServiceAction {
    Start(String),
    Stop(String),
    Restart(String),
    SetStartType(String, StartType),
    GotoProcess(u32),
    Copy(String),
    Search(String),
    OpenManager,
}

/// `needle` 已经是小写。
fn matches(s: &Service, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if let Ok(pid) = needle.parse::<u32>() {
        return s.pid == pid;
    }
    let n = needle;
    [
        Some(&s.name),
        Some(&s.display_name),
        s.description.as_ref(),
        s.group.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|f| f.to_lowercase().contains(n))
}

pub fn show(ui: &mut Ui, services: &[Service], st: &mut ServicesState) -> Vec<ServiceAction> {
    let p = Palette::of(ui.ctx());
    let mut actions = Vec::new();
    let mut menu_request = None;
    let needle = st.filter.trim().to_lowercase();
    let key = (services.as_ptr() as usize, needle.clone(), st.state);
    if st
        .rows
        .as_ref()
        .is_none_or(|(ptr, f, state, _)| (*ptr, f.clone(), *state) != key)
    {
        let rows = (0..services.len())
            .filter(|&i| {
                let s = &services[i];
                let state_ok = match st.state {
                    Filter::All => true,
                    Filter::Running => s.state == State::Running,
                    Filter::Stopped => s.state == State::Stopped,
                };
                state_ok && matches(s, &needle)
            })
            .collect();
        st.rows = Some((key.0, key.1, key.2, rows));
    }
    let rows: Vec<&Service> = st
        .rows
        .as_ref()
        .map_or(Vec::new(), |r| r.3.iter().map(|&i| &services[i]).collect());
    let selected = st
        .selected
        .as_ref()
        .and_then(|n| services.iter().find(|s| s.name.as_ref() == n));

    // 工具栏
    ui.horizontal(|ui| {
        let resp = widgets::search_box(
            ui,
            &mut st.filter,
            300.0,
            t!(
                "搜索服务名、显示名、描述、组或 PID",
                "Search name, description, group or PID"
            ),
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            resp.request_focus();
        }
        let running = services
            .iter()
            .filter(|s| s.state == State::Running)
            .count();
        for (f, label) in [
            (Filter::All, tf!("全部 {}", "All {}", services.len())),
            (
                Filter::Running,
                tf!("运行中 {running}", "Running {running}"),
            ),
            (
                Filter::Stopped,
                tf!("已停止 {}", "Stopped {}", services.len() - running),
            ),
        ] {
            let mut on = st.state == f;
            let glyph = match f {
                Filter::All => Icon::List,
                Filter::Running => Icon::Play,
                Filter::Stopped => Icon::Stop,
            };
            if widgets::toggle_chip(ui, &mut on, glyph, &label).clicked() {
                st.state = f;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::tool_button(
                ui,
                Icon::ExternalLink,
                t!("服务管理器", "Services app"),
                true,
                false,
            )
            .on_hover_text(t!("打开 services.msc", "Open services.msc"))
            .clicked()
            {
                actions.push(ServiceAction::OpenManager);
            }
            let sel = selected;
            let can_start = sel
                .is_some_and(|s| s.state == State::Stopped && s.start_type != StartType::Disabled);
            let can_stop = sel.is_some_and(|s| s.state == State::Running && s.can_stop);
            if widgets::tool_button(
                ui,
                Icon::Refresh,
                t!("重新启动", "Restart"),
                can_stop,
                false,
            )
            .clicked()
                && let Some(s) = sel
            {
                actions.push(ServiceAction::Restart(s.name.to_string()));
            }
            if widgets::tool_button(ui, Icon::Stop, t!("停止", "Stop"), can_stop, true).clicked()
                && let Some(s) = sel
            {
                actions.push(ServiceAction::Stop(s.name.to_string()));
            }
            if widgets::tool_button(ui, Icon::Play, t!("启动", "Start"), can_start, false).clicked()
                && let Some(s) = sel
            {
                actions.push(ServiceAction::Start(s.name.to_string()));
            }
        });
    });
    ui.add_space(12.0);

    ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
    TableBuilder::new(ui)
        .striped(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(260.0).clip(true))
        .column(Column::exact(60.0))
        .column(Column::exact(96.0))
        // 英文的"Automatic (delayed start)"比中文长。
        .column(Column::exact(match lang() {
            Lang::Zh => 130.0,
            Lang::En => 165.0,
        }))
        .column(Column::exact(170.0).clip(true))
        .header(34.0, |mut h| {
            for (label, right) in [
                (t!("名称", "Name"), false),
                ("PID", true),
                (t!("状态", "Status"), false),
                (t!("启动类型", "Startup type"), false),
                (t!("组", "Group"), false),
            ] {
                h.col(|ui| widgets::header_label(ui, label, right));
            }
        })
        .body(|body| {
            body.rows(30.0, rows.len(), |mut row| {
                let s = rows[row.index()];
                row.set_selected(st.selected.as_deref() == Some(s.name.as_ref()));
                row.col(|ui| {
                    ui.add_space(4.0);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(
                        RichText::new(s.name.as_ref())
                            .family(semibold())
                            .color(p.text),
                    );
                    if s.display_name != s.name {
                        ui.add(
                            egui::Label::new(
                                RichText::new(s.display_name.as_ref())
                                    .size(12.0)
                                    .color(p.faint),
                            )
                            .truncate(),
                        );
                    }
                });
                row.col(|ui| {
                    let text = if s.pid > 0 {
                        s.pid.to_string()
                    } else {
                        String::new()
                    };
                    widgets::right_label(ui, RichText::new(text).color(p.faint));
                });
                row.col(|ui| state_cell(ui, s.state));
                row.col(|ui| {
                    let color = if s.start_type == StartType::Disabled {
                        p.faint
                    } else {
                        p.muted
                    };
                    ui.label(RichText::new(s.start_type.label()).size(12.5).color(color));
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(s.group.as_deref().unwrap_or(""))
                            .size(12.0)
                            .color(p.faint),
                    );
                });
                let resp = row.response();
                if resp.clicked() || resp.secondary_clicked() {
                    st.selected = Some(s.name.to_string());
                }
                if resp.secondary_clicked()
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    menu_request = Some((s.name.to_string(), pos));
                }
                resp.on_hover_ui(|ui| tooltip(ui, s));
            });
        });
    if let Some(m) = menu_request {
        st.menu = Some(m);
    }
    // 右键菜单固定在点击位置，按服务名取内容，列表刷新、重新排序都不影响。
    if let Some((name, pos)) = st.menu.clone() {
        let open = match services.iter().find(|s| *s.name == *name) {
            Some(s) => widgets::floating_menu(ui.ctx(), "service-menu", pos, 230.0, |ui| {
                let before = actions.len();
                menu(ui, s, &mut actions);
                actions.len() > before
            }),
            None => false,
        };
        if !open {
            st.menu = None;
        }
    }
    actions
}

fn state_cell(ui: &mut Ui, state: State) {
    let p = Palette::of(ui.ctx());
    let (dot, fg) = match state {
        State::Running => (metric::GPU, p.text),
        State::Starting | State::Stopping => (metric::DISK, p.text),
        State::Paused => (metric::DISK, p.muted),
        State::Stopped => (p.track, p.faint),
    };
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, dot);
    ui.label(RichText::new(state.label()).size(12.5).color(fg));
}

fn menu(ui: &mut Ui, s: &Service, actions: &mut Vec<ServiceAction>) {
    let _p = Palette::of(ui.ctx());
    widgets::menu_heading(ui, &s.display_name, 0.0);
    let name = s.name.to_string();
    let running = s.state == State::Running;
    if s.state == State::Stopped
        && s.start_type != StartType::Disabled
        && widgets::menu_item(ui, Icon::Play, t!("启动", "Start"), false).clicked()
    {
        actions.push(ServiceAction::Start(name.clone()));
    }
    if running && s.can_stop {
        if widgets::menu_item(ui, Icon::Stop, t!("停止", "Stop"), true).clicked() {
            actions.push(ServiceAction::Stop(name.clone()));
        }
        if widgets::menu_item(ui, Icon::Refresh, t!("重新启动", "Restart"), false).clicked() {
            actions.push(ServiceAction::Restart(name.clone()));
        }
    }
    if s.start_type != StartType::Boot {
        widgets::submenu(
            ui,
            Icon::Settings,
            t!("启动类型", "Startup type"),
            200.0,
            |ui| {
                for t in StartType::EDITABLE {
                    if widgets::menu_check(ui, None, t.label(), s.start_type == t).clicked()
                        && s.start_type != t
                    {
                        actions.push(ServiceAction::SetStartType(name.clone(), t));
                    }
                }
            },
        );
    }
    ui.separator();
    if s.pid > 0
        && widgets::menu_item(ui, Icon::Window, t!("转到进程", "Go to process"), false).clicked()
    {
        actions.push(ServiceAction::GotoProcess(s.pid));
    }
    if widgets::menu_item(ui, Icon::Globe, t!("在线搜索", "Search online"), false).clicked() {
        actions.push(ServiceAction::Search(format!(
            "{} {}",
            s.name, s.display_name
        )));
    }
    if widgets::menu_item(
        ui,
        Icon::ExternalLink,
        t!("打开服务管理器", "Open Services app"),
        false,
    )
    .clicked()
    {
        actions.push(ServiceAction::OpenManager);
    }
    widgets::submenu(ui, Icon::Copy, t!("复制", "Copy"), 160.0, |ui| {
        for (label, value) in [
            (t!("服务名", "Service name"), s.name.to_string()),
            (t!("显示名称", "Display name"), s.display_name.to_string()),
            (t!("可执行路径", "Executable path"), s.binary.to_string()),
        ] {
            if !value.is_empty() && widgets::menu_item(ui, Icon::Copy, label, false).clicked() {
                actions.push(ServiceAction::Copy(value));
            }
        }
    });
}

fn tooltip(ui: &mut Ui, s: &Service) {
    let p = Palette::of(ui.ctx());
    ui.set_max_width(480.0);
    let small = |t: &str| RichText::new(t).size(11.5).color(p.muted);
    ui.label(RichText::new(s.display_name.as_ref()).strong());
    if let Some(d) = &s.description {
        ui.label(RichText::new(d.as_ref()).size(12.0).color(p.text));
    }
    ui.add_space(4.0);
    ui.label(small(&format!(
        "{} · {} · {}",
        s.state.label(),
        s.start_type.label(),
        s.account
    )));
    if !s.binary.is_empty() {
        ui.label(
            RichText::new(s.binary.as_ref())
                .monospace()
                .size(11.0)
                .color(p.muted),
        );
    }
}

/// 没有服务数据时（例如采集线程还在第一次枚举）显示的占位。
pub fn loading(ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        t!("正在读取服务列表…", "Loading services…"),
        FontId::proportional(13.0),
        p.muted,
    );
}
