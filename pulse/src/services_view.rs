//! 服务页：搜索、状态筛选、启动 / 停止 / 重启、修改启动类型、在服务与进程之间跳转。

use eframe::egui::{self, Align, Align2, FontId, Layout, RichText, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use pulse_probe::services::{Service, StartType, State};

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

fn matches(s: &Service, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if let Ok(pid) = needle.parse::<u32>() {
        return s.pid == pid;
    }
    let n = needle.to_lowercase();
    [
        Some(&s.name),
        Some(&s.display_name),
        s.description.as_ref(),
        s.group.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|f| f.to_lowercase().contains(&n))
}

pub fn show(ui: &mut Ui, services: &[Service], st: &mut ServicesState) -> Vec<ServiceAction> {
    let p = Palette::of(ui.ctx());
    let mut actions = Vec::new();
    let needle = st.filter.trim().to_string();
    let rows: Vec<&Service> = services
        .iter()
        .filter(|s| match st.state {
            Filter::All => true,
            Filter::Running => s.state == State::Running,
            Filter::Stopped => s.state == State::Stopped,
        })
        .filter(|s| matches(s, &needle))
        .collect();
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
            "搜索服务名、显示名、描述、组或 PID",
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            resp.request_focus();
        }
        let running = services
            .iter()
            .filter(|s| s.state == State::Running)
            .count();
        for (f, label) in [
            (Filter::All, format!("全部 {}", services.len())),
            (Filter::Running, format!("运行中 {running}")),
            (
                Filter::Stopped,
                format!("已停止 {}", services.len() - running),
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
            if widgets::tool_button(ui, Icon::ExternalLink, "服务管理器", true, false)
                .on_hover_text("打开 services.msc")
                .clicked()
            {
                actions.push(ServiceAction::OpenManager);
            }
            let sel = selected;
            let can_start = sel
                .is_some_and(|s| s.state == State::Stopped && s.start_type != StartType::Disabled);
            let can_stop = sel.is_some_and(|s| s.state == State::Running && s.can_stop);
            if widgets::tool_button(ui, Icon::Refresh, "重新启动", can_stop, false).clicked()
                && let Some(s) = sel
            {
                actions.push(ServiceAction::Restart(s.name.to_string()));
            }
            if widgets::tool_button(ui, Icon::Stop, "停止", can_stop, true).clicked()
                && let Some(s) = sel
            {
                actions.push(ServiceAction::Stop(s.name.to_string()));
            }
            if widgets::tool_button(ui, Icon::Play, "启动", can_start, false).clicked()
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
        .column(Column::exact(130.0))
        .column(Column::exact(170.0).clip(true))
        .header(34.0, |mut h| {
            for (label, right) in [
                ("名称", false),
                ("PID", true),
                ("状态", false),
                ("启动类型", false),
                ("组", false),
            ] {
                h.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().hline(
                        r.x_range().expand(6.0),
                        r.bottom() - 0.5,
                        Stroke::new(1.0, p.border),
                    );
                    let layout = if right {
                        Layout::right_to_left(Align::Center)
                    } else {
                        Layout::left_to_right(Align::Center)
                    };
                    ui.with_layout(layout, |ui| {
                        if !right {
                            ui.add_space(4.0);
                        }
                        ui.label(RichText::new(label).size(12.0).color(p.muted));
                    });
                });
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
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let text = if s.pid > 0 {
                            s.pid.to_string()
                        } else {
                            String::new()
                        };
                        ui.label(RichText::new(text).color(p.faint));
                    });
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
                resp.context_menu(|ui| menu(ui, s, &mut actions));
                resp.on_hover_ui(|ui| tooltip(ui, s));
            });
        });
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
    let p = Palette::of(ui.ctx());
    ui.set_min_width(220.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(2.0);
    ui.label(
        RichText::new(s.display_name.as_ref())
            .size(12.0)
            .color(p.muted),
    );
    ui.add_space(2.0);
    let name = s.name.to_string();
    let running = s.state == State::Running;
    if s.state == State::Stopped
        && s.start_type != StartType::Disabled
        && widgets::menu_item(ui, Icon::Play, "启动", false).clicked()
    {
        actions.push(ServiceAction::Start(name.clone()));
    }
    if running && s.can_stop {
        if widgets::menu_item(ui, Icon::Stop, "停止", true).clicked() {
            actions.push(ServiceAction::Stop(name.clone()));
        }
        if widgets::menu_item(ui, Icon::Refresh, "重新启动", false).clicked() {
            actions.push(ServiceAction::Restart(name.clone()));
        }
    }
    if s.start_type != StartType::Boot {
        ui.menu_image_text_button(
            widgets::icon_image(ui, Icon::Settings, 15.0, p.muted),
            "启动类型",
            |ui| {
                ui.set_min_width(170.0);
                for t in StartType::EDITABLE {
                    if widgets::menu_check(ui, Icon::Check, t.label(), s.start_type == t).clicked()
                        && s.start_type != t
                    {
                        actions.push(ServiceAction::SetStartType(name.clone(), t));
                    }
                }
            },
        );
    }
    ui.separator();
    if s.pid > 0 && widgets::menu_item(ui, Icon::Window, "转到进程", false).clicked() {
        actions.push(ServiceAction::GotoProcess(s.pid));
    }
    if widgets::menu_item(ui, Icon::Globe, "在线搜索", false).clicked() {
        actions.push(ServiceAction::Search(format!(
            "{} {}",
            s.name, s.display_name
        )));
    }
    ui.menu_image_text_button(
        widgets::icon_image(ui, Icon::Copy, 15.0, p.muted),
        "复制",
        |ui| {
            ui.set_min_width(150.0);
            for (label, value) in [
                ("服务名", s.name.to_string()),
                ("显示名称", s.display_name.to_string()),
                ("可执行路径", s.binary.to_string()),
            ] {
                if !value.is_empty() && widgets::menu_item(ui, Icon::Copy, label, false).clicked() {
                    actions.push(ServiceAction::Copy(value));
                }
            }
        },
    );
    if widgets::menu_item(ui, Icon::ExternalLink, "打开服务管理器", false).clicked() {
        actions.push(ServiceAction::OpenManager);
    }
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
        "正在读取服务列表…",
        FontId::proportional(13.0),
        p.muted,
    );
}
