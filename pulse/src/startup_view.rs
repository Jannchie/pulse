//! 启动应用页：启用 / 禁用开机自启项，状态与任务管理器共用。

use std::sync::Arc;

use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use pulse_probe::startup::StartupItem;

use crate::icons::Icons;
use crate::icons_gen::Icon;
use crate::theme::{Palette, semibold};
use crate::widgets;
use pulse_probe::os::format_filetime as fmt_filetime;

pub enum StartupAction {
    SetEnabled(usize, bool),
    Reveal(Arc<str>),
    Properties(Arc<str>),
    Search(String),
    Copy(String),
    OpenFolder,
    Refresh,
}

#[derive(Default)]
pub struct StartupState {
    pub selected: Option<String>,
    pub filter: String,
    pub state: Filter,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Enabled,
    Disabled,
}

/// 搜索词按空格拆开，每个词都要出现在名称、发布者、描述、命令行或来源之一（不区分大小写）。
fn matches(it: &StartupItem, words: &[String]) -> bool {
    let fields = [
        Some(it.name.as_str()),
        it.publisher.as_deref(),
        it.description.as_deref(),
        Some(it.command.as_str()),
        Some(it.source.label()),
    ];
    words.iter().all(|w| {
        fields
            .iter()
            .flatten()
            .any(|f| f.to_lowercase().contains(w.as_str()))
    })
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
    let words: Vec<String> = st
        .filter
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    // 保留原始下标：启用 / 禁用操作按下标找回条目。
    let rows: Vec<usize> = (0..items.len())
        .filter(|&i| {
            let it = &items[i];
            let state_ok = match st.state {
                Filter::All => true,
                Filter::Enabled => it.enabled,
                Filter::Disabled => !it.enabled,
            };
            state_ok && matches(it, &words)
        })
        .collect();

    ui.horizontal(|ui| {
        let resp = widgets::search_box(ui, &mut st.filter, 300.0, "搜索名称、发布者、命令行");
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            resp.request_focus();
        }
        if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.filter.clear();
        }
        for (f, glyph, label) in [
            (Filter::All, Icon::List, format!("全部 {}", items.len())),
            (Filter::Enabled, Icon::Play, format!("已启用 {enabled}")),
            (
                Filter::Disabled,
                Icon::Pause,
                format!("已禁用 {}", items.len() - enabled),
            ),
        ] {
            let mut on = st.state == f;
            if widgets::toggle_chip(ui, &mut on, glyph, &label).clicked() {
                st.state = f;
            }
        }
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
                h.col(|ui| widgets::header_label(ui, label, false));
            }
        })
        .body(|body| {
            body.rows(40.0, rows.len(), |mut row| {
                let i = rows[row.index()];
                let it = &items[i];
                row.set_selected(st.selected.as_deref() == Some(&it.key));
                row.col(|ui| {
                    let rect = ui.max_rect();
                    let icon_rect = Rect::from_center_size(
                        pos2(rect.left() + 14.0, rect.center().y),
                        vec2(20.0, 20.0),
                    );
                    match it.path.as_ref().and_then(|p| icons.get(p)) {
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
    if rows.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("没有匹配的启动项").color(p.faint));
        });
    }
    actions
}

fn menu(ui: &mut Ui, i: usize, it: &StartupItem, elevated: bool, actions: &mut Vec<StartupAction>) {
    let _p = Palette::of(ui.ctx());
    widgets::menu_heading(ui, &it.name, 210.0);
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
