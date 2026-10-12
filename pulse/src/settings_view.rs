//! 设置页：分组卡片，每行左侧是名称与说明，右侧是控件。

use eframe::egui::{
    self, Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, Ui, pos2, vec2,
};

use pulse_probe::{t, tf};

use crate::settings::{Language, SearchEngine, Settings, Speed, ThemeChoice};
use crate::theme::{Palette, semibold};
use crate::widgets;

pub enum SettingsAction {
    /// 切换"替换任务管理器"（写 IFEO 注册表，需要管理员权限）。
    SetTaskmgrReplaced(bool),
}

pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    taskmgr_replaced: bool,
    elevated: bool,
) -> Option<SettingsAction> {
    let mut action = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // 内容限制在 720pt 宽并居中，宽屏上不至于左右拉得太散。
            let w = ui.available_width().min(720.0);
            let pad = ((ui.available_width() - w) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(t!("设置", "Settings"))
                            .size(22.0)
                            .family(semibold())
                            .color(Palette::of(ui.ctx()).text),
                    );
                    ui.add_space(4.0);

                    group(ui, t!("常规", "General"), |ui| {
                        row(
                            ui,
                            t!("更新速度", "Update speed"),
                            t!(
                                "数据刷新的间隔。暂停后画面停在当前数值。",
                                "How often data refreshes. Paused freezes the current values."
                            ),
                            |ui| {
                                let labels = Speed::ALL.map(|s| s.label());
                                let cur = Speed::ALL
                                    .iter()
                                    .position(|&s| s == settings.speed)
                                    .unwrap_or(0);
                                if let Some(i) = choice(ui, &labels, cur) {
                                    settings.speed = Speed::ALL[i];
                                }
                            },
                        );
                        divider(ui);
                        row(
                            ui,
                            t!("搜索引擎", "Search engine"),
                            t!(
                                "右键菜单里的“在线搜索”用它，在默认浏览器中打开。",
                                "Used by “Search online”. Opens in your default browser."
                            ),
                            |ui| {
                                let labels = SearchEngine::ALL.map(|e| e.label());
                                let cur = SearchEngine::ALL
                                    .iter()
                                    .position(|&e| e == settings.search)
                                    .unwrap_or(0);
                                if let Some(i) = choice(ui, &labels, cur) {
                                    settings.search = SearchEngine::ALL[i];
                                }
                            },
                        );
                        divider(ui);
                        row(
                            ui,
                            t!("窗口置顶", "Always on top"),
                            t!("始终显示在其他窗口之上。", "Keep Pulse above other windows."),
                            |ui| {
                                widgets::switch(ui, &mut settings.always_on_top, true);
                            },
                        );
                    });

                    group(ui, t!("外观", "Appearance"), |ui| {
                        row(
                            ui,
                            t!("主题", "Theme"),
                            t!(
                                "跟随系统会随 Windows 的深浅色设置自动切换。",
                                "System follows your Windows light or dark mode."
                            ),
                            |ui| {
                                let labels = ThemeChoice::ALL.map(|t| t.label());
                                let cur = ThemeChoice::ALL
                                    .iter()
                                    .position(|&t| t == settings.theme)
                                    .unwrap_or(0);
                                if let Some(i) = choice(ui, &labels, cur) {
                                    settings.theme = ThemeChoice::ALL[i];
                                }
                            },
                        );
                        divider(ui);
                        row(
                            ui,
                            t!("语言", "Language"),
                            t!(
                                "跟随系统时，Windows 显示语言为中文则用中文，否则用英文。",
                                "System uses Chinese if Windows is in Chinese, otherwise English."
                            ),
                            |ui| {
                                let labels = Language::ALL.map(|l| l.label());
                                let cur = Language::ALL
                                    .iter()
                                    .position(|&l| l == settings.language)
                                    .unwrap_or(0);
                                if let Some(i) = choice(ui, &labels, cur) {
                                    settings.language = Language::ALL[i];
                                }
                            },
                        );
                    });

                    group(ui, t!("进程列表", "Process list"), |ui| {
                        row(
                            ui,
                            t!("按程序分组", "Group by program"),
                            t!(
                                "同名进程合并为一行，可展开查看。",
                                "Combine processes with the same name into one expandable row."
                            ),
                            |ui| {
                                widgets::switch(ui, &mut settings.grouped, true);
                            },
                        );
                    });

                    group(ui, t!("系统", "System"), |ui| {
                        let desc = if elevated {
                            t!(
                                "按 Ctrl+Shift+Esc 或从任务栏打开任务管理器时，改为启动 Pulse。",
                                "Open Pulse instead when you press Ctrl+Shift+Esc or open Task Manager from the taskbar."
                            )
                        } else {
                            t!(
                                "按 Ctrl+Shift+Esc 时启动 Pulse。需要以管理员身份运行 Pulse 才能修改。",
                                "Open Pulse with Ctrl+Shift+Esc. Run Pulse as administrator to change this."
                            )
                        };
                        row(ui, t!("替换任务管理器", "Replace Task Manager"), desc, |ui| {
                            let mut on = taskmgr_replaced;
                            if widgets::switch(ui, &mut on, elevated).changed() {
                                action = Some(SettingsAction::SetTaskmgrReplaced(on));
                            }
                        });
                    });

                    group(ui, t!("关于", "About"), |ui| {
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            widgets::brand_mark(ui, 56.0);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                let p = Palette::of(ui.ctx());
                                ui.spacing_mut().item_spacing.y = 4.0;
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new("Pulse")
                                        .size(15.0)
                                        .family(semibold())
                                        .color(p.text),
                                );
                                ui.label(
                                    RichText::new(tf!(
                                        "版本 {}",
                                        "Version {}",
                                        env!("CARGO_PKG_VERSION")
                                    ))
                                        .size(12.0)
                                        .color(p.muted),
                                );
                            });
                        });
                        ui.add_space(16.0);
                    });
                    ui.add_space(16.0);
                });
            });
        });
    action
}

fn group(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    let p = Palette::of(ui.ctx());
    ui.add_space(18.0);
    ui.label(
        RichText::new(title)
            .size(13.0)
            .family(semibold())
            .color(p.muted),
    );
    ui.add_space(8.0);
    Frame::NONE
        .fill(p.surface)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui);
        });
}

/// 一行设置：左边名称 + 说明，右边控件，两者都在行内垂直居中。
/// 说明在控件左侧换行，不会压到控件上（英文说明往往更长）。
fn row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    const H: f32 = 64.0;
    let p = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    control(&mut child);
    let control_w = child.min_rect().width();
    let title = ui
        .painter()
        .layout_no_wrap(title.to_string(), FontId::proportional(13.5), p.text);
    let desc = ui.painter().layout(
        desc.to_string(),
        FontId::proportional(12.0),
        p.muted,
        (rect.width() - control_w - 24.0).max(120.0),
    );
    let gap = 3.0;
    let top = rect.center().y - (title.size().y + gap + desc.size().y) / 2.0;
    let desc_top = top + title.size().y + gap;
    ui.painter().galley(pos2(rect.left(), top), title, p.text);
    ui.painter()
        .galley(pos2(rect.left(), desc_top), desc, p.muted);
}

fn divider(ui: &mut Ui) {
    let p = Palette::of(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.center().y, Stroke::new(1.0, p.track));
}

/// 文字分段选择器，返回新选中的下标。
fn choice(ui: &mut Ui, labels: &[&str], selected: usize) -> Option<usize> {
    let p = Palette::of(ui.ctx());
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_string(), font.clone(), p.text)
                .size()
                .x
                + 24.0
        })
        .collect();
    let height = 30.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(widths.iter().sum::<f32>() + 6.0, height),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), p.track);
    let mut x = rect.left() + 3.0;
    let mut picked = None;
    for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(*w, height - 6.0));
        x += w;
        // 按选项文字区分 id：同一页上有多个选择器，父 UI 的 id 可能相同。
        let resp = ui.interact(r, egui::Id::new(("choice", labels, i)), Sense::click());
        let on = i == selected;
        if on {
            ui.painter().rect_filled(r, CornerRadius::same(6), p.bg);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(6), p.hover);
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if on || resp.hovered() {
                p.text
            } else {
                p.muted
            },
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
            && !on
        {
            picked = Some(i);
        }
    }
    picked
}
