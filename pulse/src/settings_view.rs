//! 设置页：分组卡片，每行左侧是名称与说明，右侧是控件。

use eframe::egui::{
    self, Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, Ui, pos2, vec2,
};

use crate::icons_gen::Icon;
use crate::settings::{Settings, Speed, ThemeChoice};
use crate::table::Col;
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
                        RichText::new("设置")
                            .size(22.0)
                            .family(semibold())
                            .color(Palette::of(ui.ctx()).text),
                    );
                    ui.add_space(4.0);

                    group(ui, "常规", |ui| {
                        row(
                            ui,
                            "更新速度",
                            "数据刷新的间隔。暂停后画面停在当前数值。",
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
                            "窗口置顶",
                            "始终显示在其他窗口之上。",
                            |ui| {
                                widgets::switch(ui, &mut settings.always_on_top, true);
                            },
                        );
                    });

                    group(ui, "外观", |ui| {
                        row(
                            ui,
                            "主题",
                            "跟随系统会随 Windows 的深浅色设置自动切换。",
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
                    });

                    group(ui, "进程列表", |ui| {
                        row(
                            ui,
                            "按程序分组",
                            "同名进程合并为一行，可展开查看。",
                            |ui| {
                                widgets::switch(ui, &mut settings.grouped, true);
                            },
                        );
                        divider(ui);
                        row(
                            ui,
                            "显示的列",
                            "窗口变窄时，不重要的列会先自动隐藏。",
                            |_| {},
                        );
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for c in Col::ALL {
                                let shown = !settings.hidden_columns.contains(&c);
                                if chip(ui, c.label(), shown).clicked() {
                                    if shown {
                                        settings.hidden_columns.push(c);
                                    } else {
                                        settings.hidden_columns.retain(|&x| x != c);
                                    }
                                }
                            }
                        });
                        ui.add_space(14.0);
                    });

                    group(ui, "系统", |ui| {
                        let desc = if elevated {
                            "按 Ctrl+Shift+Esc 或从任务栏打开任务管理器时，改为启动 Pulse。"
                        } else {
                            "按 Ctrl+Shift+Esc 时启动 Pulse。需要以管理员身份运行 Pulse 才能修改。"
                        };
                        row(ui, "替换任务管理器", desc, |ui| {
                            let mut on = taskmgr_replaced;
                            if widgets::switch(ui, &mut on, elevated).changed() {
                                action = Some(SettingsAction::SetTaskmgrReplaced(on));
                            }
                        });
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
fn row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    const H: f32 = 64.0;
    let p = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::hover());
    let title = ui
        .painter()
        .layout_no_wrap(title.to_string(), FontId::proportional(13.5), p.text);
    let desc = ui
        .painter()
        .layout_no_wrap(desc.to_string(), FontId::proportional(12.0), p.muted);
    let gap = 3.0;
    let top = rect.center().y - (title.size().y + gap + desc.size().y) / 2.0;
    let desc_top = top + title.size().y + gap;
    ui.painter().galley(pos2(rect.left(), top), title, p.text);
    ui.painter()
        .galley(pos2(rect.left(), desc_top), desc, p.muted);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    control(&mut child);
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
        let resp = ui.interact(r, ui.id().with(("choice", i)), Sense::click());
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

/// 可勾选的标签。
fn chip(ui: &mut Ui, label: &str, on: bool) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(12.5), p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 40.0, 30.0), Sense::click());
    let fill = if on {
        p.accent.gamma_multiply(0.18)
    } else if resp.hovered() {
        p.hover
    } else {
        p.track
    };
    ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
    let fg = if on { p.text } else { p.muted };
    let glyph = if on { Icon::Check } else { Icon::Plus };
    widgets::icon(
        ui,
        glyph,
        pos2(rect.left() + 16.0, rect.center().y),
        13.0,
        if on { p.accent } else { p.faint },
    );
    ui.painter().galley(
        pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
