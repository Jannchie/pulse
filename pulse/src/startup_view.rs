//! 启动应用页：启用 / 禁用开机自启项，状态与任务管理器共用。

use std::sync::Arc;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use pulse_probe::startup::StartupItem;

use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::theme::{Palette, semibold};
use crate::{fmt_filetime, widgets};

pub enum StartupAction {
    SetEnabled(usize, bool),
    Reveal(String),
    Properties(String),
    Search(String),
    Copy(String),
    OpenFolder,
    Refresh,
}

#[derive(Default)]
pub struct StartupState {
    pub selected: Option<String>,
}

pub fn show(
    ui: &mut Ui,
    items: &[StartupItem],
    st: &mut StartupState,
    icons: &mut Icons,
    elevated: bool,
) -> Vec<StartupAction> {
    let p = Palette::of(ui.ctx());
    let mut actions = Vec::new();
    let enabled = items.iter().filter(|i| i.enabled).count();

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} 个启动项 · {enabled} 个已启用", items.len()))
                .size(13.0)
                .color(p.muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::tool_button(ui, Icon::Refresh, "刷新", true, false).clicked() {
                actions.push(StartupAction::Refresh);
            }
            if widgets::tool_button(ui, Icon::FolderOpen, "启动文件夹", true, false)
                .on_hover_text("打开当前用户的启动文件夹，放进去的快捷方式会在登录时运行")
                .clicked()
            {
                actions.push(StartupAction::OpenFolder);
            }
            if !elevated {
                ui.label(
                    RichText::new("修改\"所有用户\"的启动项需要管理员权限")
                        .size(12.0)
                        .color(p.faint),
                );
            }
        });
    });
    ui.add_space(12.0);

    ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
    TableBuilder::new(ui)
        .striped(false)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::remainder().at_least(280.0).clip(true))
        .column(Column::exact(190.0).clip(true))
        .column(Column::exact(120.0))
        .column(Column::exact(200.0).clip(true))
        .header(34.0, |mut h| {
            for label in ["名称", "发布者", "状态", "来源"] {
                h.col(|ui| {
                    let r = ui.max_rect();
                    ui.painter().hline(
                        r.x_range().expand(6.0),
                        r.bottom() - 0.5,
                        Stroke::new(1.0, p.border),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(label).size(12.0).color(p.muted));
                });
            }
        })
        .body(|body| {
            body.rows(40.0, items.len(), |mut row| {
                let i = row.index();
                let it = &items[i];
                row.set_selected(st.selected.as_deref() == Some(&it.key));
                row.col(|ui| {
                    let rect = ui.max_rect();
                    let icon_rect = Rect::from_center_size(
                        pos2(rect.left() + 14.0, rect.center().y),
                        vec2(20.0, 20.0),
                    );
                    let path: Option<Arc<str>> = it.path.as_deref().map(Arc::from);
                    match path.as_ref().and_then(|p| icons.get(p)) {
                        Some(tex) => {
                            ui.painter().image(
                                tex.id(),
                                icon_rect,
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        None => widgets::avatar(ui.painter(), icon_rect, &it.name),
                    }
                    ui.add_space(34.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let fg = if it.enabled { p.text } else { p.muted };
                        ui.add(
                            egui::Label::new(RichText::new(&it.name).family(semibold()).color(fg))
                                .truncate(),
                        );
                        let sub = it.description.clone().unwrap_or_else(|| it.command.clone());
                        ui.add(
                            egui::Label::new(RichText::new(sub).size(11.5).color(p.faint))
                                .truncate(),
                        );
                    });
                });
                row.col(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(it.publisher.as_deref().unwrap_or("—"))
                                .size(12.5)
                                .color(p.muted),
                        )
                        .truncate(),
                    );
                });
                row.col(|ui| {
                    let mut on = it.enabled;
                    let locked = it.source.needs_admin() && !elevated;
                    let resp = widgets::switch(ui, &mut on, !locked);
                    if resp.changed() {
                        actions.push(StartupAction::SetEnabled(i, on));
                    }
                    let label = if it.enabled {
                        "已启用".to_string()
                    } else {
                        "已禁用".to_string()
                    };
                    let resp = ui.label(RichText::new(label).size(12.5).color(if it.enabled {
                        p.text
                    } else {
                        p.faint
                    }));
                    if locked {
                        resp.on_hover_text("需要以管理员身份运行 Pulse");
                    } else if let Some(t) = it.disabled_at {
                        resp.on_hover_text(format!("禁用于 {}", fmt_filetime(t)));
                    }
                });
                row.col(|ui| {
                    ui.label(RichText::new(it.source.label()).size(12.0).color(p.faint));
                });
                let resp = row.response();
                if resp.clicked() || resp.secondary_clicked() {
                    st.selected = Some(it.key.clone());
                }
                resp.context_menu(|ui| menu(ui, i, it, elevated, &mut actions));
                resp.on_hover_ui(|ui| {
                    ui.set_max_width(520.0);
                    ui.label(RichText::new(&it.name).strong());
                    ui.label(
                        RichText::new(&it.command)
                            .monospace()
                            .size(11.0)
                            .color(p.text),
                    );
                    ui.label(RichText::new(it.source.label()).size(11.5).color(p.muted));
                });
            });
        });
    actions
}

fn menu(ui: &mut Ui, i: usize, it: &StartupItem, elevated: bool, actions: &mut Vec<StartupAction>) {
    let p = Palette::of(ui.ctx());
    ui.set_min_width(210.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(2.0);
    ui.label(RichText::new(&it.name).size(12.0).color(p.muted));
    ui.add_space(2.0);
    let locked = it.source.needs_admin() && !elevated;
    let (glyph, label) = if it.enabled {
        (Icon::Pause, "禁用")
    } else {
        (Icon::Play, "启用")
    };
    let resp = ui
        .add_enabled_ui(!locked, |ui| widgets::menu_item(ui, glyph, label, false))
        .inner;
    if resp.clicked() {
        actions.push(StartupAction::SetEnabled(i, !it.enabled));
    }
    ui.separator();
    if let Some(path) = &it.path {
        if widgets::menu_item(ui, Icon::FolderOpen, "打开文件位置", false).clicked() {
            actions.push(StartupAction::Reveal(path.clone()));
        }
        if widgets::menu_item(ui, Icon::Info, "属性", false).clicked() {
            actions.push(StartupAction::Properties(path.clone()));
        }
    }
    if widgets::menu_item(ui, Icon::Globe, "在线搜索", false).clicked() {
        actions.push(StartupAction::Search(it.name.clone()));
    }
    if widgets::menu_item(ui, Icon::Terminal, "复制命令行", false).clicked() {
        actions.push(StartupAction::Copy(it.command.clone()));
    }
}
