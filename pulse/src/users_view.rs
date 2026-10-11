//! 用户页：每个登录会话的资源占用，断开连接 / 注销。

use eframe::egui::{Align, Layout, RichText, Sense, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use pulse_probe::Snapshot;
use pulse_probe::sessions::Session;

use crate::icons_gen::Icon;
use crate::theme::{Palette, metric, semibold};
use crate::{fmt_bytes, fmt_rate, widgets};

pub enum UserAction {
    Disconnect(u32, String),
    Logoff(u32, String),
}

#[derive(Default)]
struct Usage {
    processes: usize,
    cpu: f32,
    memory: u64,
    disk: u64,
    net: u64,
}

pub fn show(ui: &mut Ui, sessions: &[Session], snap: &Snapshot) -> Vec<UserAction> {
    let p = Palette::of(ui.ctx());
    let mut actions = Vec::new();
    let usage: Vec<Usage> = sessions
        .iter()
        .map(|s| {
            let mut u = Usage::default();
            for pr in snap.processes.iter().filter(|pr| pr.session_id == s.id) {
                u.processes += 1;
                u.cpu += pr.cpu;
                u.memory += pr.private_ws;
                u.disk += pr.io_rate;
                u.net += pr.net_rx + pr.net_tx;
            }
            u
        })
        .collect();

    ui.label(
        RichText::new(format!("{} 个已登录的用户会话", sessions.len()))
            .size(13.0)
            .color(p.muted),
    );
    ui.add_space(12.0);
    ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
    TableBuilder::new(ui)
        .striped(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(240.0).clip(true))
        .column(Column::exact(150.0))
        .column(Column::exact(80.0))
        .column(Column::exact(70.0))
        .column(Column::exact(70.0))
        .column(Column::exact(90.0))
        .column(Column::exact(90.0))
        .column(Column::exact(90.0))
        .header(34.0, |mut h| {
            for (label, right) in [
                ("用户", false),
                ("会话", false),
                ("状态", false),
                ("进程", true),
                ("CPU", true),
                ("内存", true),
                ("磁盘", true),
                ("网络", true),
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
            body.rows(40.0, sessions.len(), |mut row| {
                let i = row.index();
                let (s, u) = (&sessions[i], &usage[i]);
                let name = if s.domain.is_empty() {
                    s.user.clone()
                } else {
                    format!(r"{}\{}", s.domain, s.user)
                };
                row.col(|ui| {
                    ui.add_space(4.0);
                    let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                    widgets::icon(ui, Icon::User, r.center(), 18.0, p.accent);
                    ui.label(RichText::new(&name).family(semibold()).color(p.text));
                    if s.current {
                        widgets::status_chip(ui, "当前", p.muted);
                    }
                });
                row.col(|ui| {
                    ui.label(
                        RichText::new(format!("{} · {}", s.id, s.station))
                            .size(12.5)
                            .color(p.muted),
                    );
                });
                row.col(|ui| {
                    ui.label(RichText::new(s.state).size(12.5).color(p.text));
                });
                let right = |ui: &mut Ui, text: String| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(text).color(p.text))
                    });
                };
                row.col(|ui| right(ui, u.processes.to_string()));
                row.col(|ui| {
                    widgets::heat(ui, u.cpu / 50.0, metric::CPU);
                    right(ui, format!("{:.1}%", u.cpu));
                });
                row.col(|ui| {
                    widgets::heat(ui, u.memory as f32 / (16u64 << 30) as f32, metric::MEMORY);
                    right(ui, fmt_bytes(u.memory));
                });
                row.col(|ui| right(ui, fmt_rate(u.disk)));
                row.col(|ui| right(ui, fmt_rate(u.net)));
                row.response().context_menu(|ui| {
                    ui.set_min_width(200.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(&name).size(12.0).color(p.muted));
                    if s.state == "活动"
                        && widgets::menu_item(ui, Icon::Unlink, "断开连接", false).clicked()
                    {
                        actions.push(UserAction::Disconnect(s.id, name.clone()));
                    }
                    if widgets::menu_item(ui, Icon::Logout, "注销", true).clicked() {
                        actions.push(UserAction::Logoff(s.id, name.clone()));
                    }
                });
            });
        });
    actions
}
