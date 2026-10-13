//! 设置页：分组卡片，每行左侧是名称与说明，右侧是控件。

use eframe::egui::{
    self, Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, Sense,
    Stroke, Ui, pos2, vec2,
};

use pulse_probe::{t, tf};

use crate::settings::{Language, SearchEngine, Settings, Speed, ThemeChoice};
use crate::theme::{Palette, semibold};
use crate::update::Check;
use crate::{dialogs, update_view, widgets};

pub enum SettingsAction {
    /// 切换"替换任务管理器"（写 IFEO 注册表，需要管理员权限）。
    SetTaskmgrReplaced(bool),
    /// 立即检查更新。
    CheckUpdate,
    /// 打开更新对话框。
    ShowUpdate,
}

pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    taskmgr_replaced: bool,
    elevated: bool,
    update: &Check,
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

                    group(ui, t!("常规", "General", "全般"), |ui| {
                        row(
                            ui,
                            t!("更新速度", "Update speed", "更新速度"),
                            t!(
                                "数据刷新的间隔。暂停后画面停在当前数值。",
                                "How often data refreshes. Paused freezes the current values.", "データの更新間隔。一時停止すると現在の値で止まります。"
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
                            t!("搜索引擎", "Search engine", "検索エンジン"),
                            t!(
                                "右键菜单里的“在线搜索”用它，在默认浏览器中打开。",
                                "Used by “Search online”. Opens in your default browser.", "「オンラインで検索」で使い、既定のブラウザーで開きます。"
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
                            t!("窗口置顶", "Always on top", "常に手前に表示"),
                            t!("始终显示在其他窗口之上。", "Keep Pulse above other windows.", "Pulse を常にほかのウィンドウより手前に表示します。"),
                            |ui| {
                                widgets::switch(ui, &mut settings.always_on_top, true);
                            },
                        );
                        divider(ui);
                        row(
                            ui,
                            t!("自动检查更新", "Check for updates automatically", "更新を自動的に確認する"),
                            t!(
                                "启动时和每天检查一次新版本，确认后才安装。",
                                "Checks for a new version at startup and daily; installs only after you confirm.", "起動時と 1 日 1 回、新しいバージョンを確認します。インストールは確認後のみです。"
                            ),
                            |ui| {
                                widgets::switch(ui, &mut settings.auto_update, true);
                            },
                        );
                    });

                    group(ui, t!("外观", "Appearance", "外観"), |ui| {
                        row(
                            ui,
                            t!("主题", "Theme", "テーマ"),
                            t!(
                                "跟随系统会随 Windows 的深浅色设置自动切换。",
                                "System follows your Windows light or dark mode.", "[システム] では Windows のライト / ダーク設定に合わせます。"
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
                            t!("语言", "Language", "言語"),
                            t!(
                                "跟随系统时使用 Windows 的显示语言。",
                                "System uses the Windows display language.", "[システム] では Windows の表示言語を使います。"
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

                    group(ui, t!("进程列表", "Process list", "プロセス一覧"), |ui| {
                        row(
                            ui,
                            t!("按程序分组", "Group by program", "プログラムごとにグループ化"),
                            t!(
                                "同名进程合并为一行，可展开查看。",
                                "Combine processes with the same name into one expandable row.", "同じ名前のプロセスを、展開できる 1 行にまとめます。"
                            ),
                            |ui| {
                                widgets::switch(ui, &mut settings.grouped, true);
                            },
                        );
                    });

                    group(ui, t!("系统", "System", "システム"), |ui| {
                        let desc = if elevated {
                            t!(
                                "按 Ctrl+Shift+Esc 或从任务栏打开任务管理器时，改为启动 Pulse。",
                                "Open Pulse instead when you press Ctrl+Shift+Esc or open Task Manager from the taskbar.", "Ctrl+Shift+Esc を押したときやタスク バーからタスク マネージャーを開いたときに、代わりに Pulse を起動します。"
                            )
                        } else {
                            t!(
                                "按 Ctrl+Shift+Esc 时启动 Pulse。需要以管理员身份运行 Pulse 才能修改。",
                                "Open Pulse with Ctrl+Shift+Esc. Run Pulse as administrator to change this.", "Ctrl+Shift+Esc で Pulse を起動します。変更するには Pulse を管理者として実行してください。"
                            )
                        };
                        row(ui, t!("替换任务管理器", "Replace Task Manager", "タスク マネージャーを置き換える"), desc, |ui| {
                            let mut on = taskmgr_replaced;
                            if widgets::switch(ui, &mut on, elevated).changed() {
                                action = Some(SettingsAction::SetTaskmgrReplaced(on));
                            }
                        });
                    });

                    group(ui, t!("关于", "About", "バージョン情報"), |ui| {
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
                                        "Version {}", "バージョン {}",
                                        env!("CARGO_PKG_VERSION")
                                    ))
                                        .size(12.0)
                                        .color(p.muted),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 12.0;
                                let available = matches!(update, Check::Available(_));
                                let label = if available {
                                    t!("查看更新", "View update", "更新を表示")
                                } else {
                                    t!("检查更新", "Check for updates", "更新を確認")
                                };
                                let kind = if available {
                                    dialogs::Kind::Primary
                                } else {
                                    dialogs::Kind::Secondary
                                };
                                let enabled = !matches!(update, Check::Checking);
                                if dialogs::button(ui, label, kind, enabled).clicked() {
                                    action = Some(if available {
                                        SettingsAction::ShowUpdate
                                    } else {
                                        SettingsAction::CheckUpdate
                                    });
                                }
                                if let Some((text, color)) = update_view::check_status(ui, update) {
                                    ui.add(
                                        egui::Label::new(RichText::new(text).size(12.0).color(color))
                                            .wrap(),
                                    );
                                }
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

/// 一行设置：左边名称 + 一行说明，右边控件，两者都在行内垂直居中。
fn row(ui: &mut Ui, title: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    const H: f32 = 60.0;
    let p = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::hover());
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
    // 说明只占一行，放不下时以省略号截断，悬停在这一行上显示全文。
    let mut job = egui::text::LayoutJob::simple_singleline(
        desc.to_string(),
        FontId::proportional(12.0),
        p.muted,
    );
    job.wrap =
        egui::text::TextWrapping::truncate_at_width((rect.width() - control_w - 24.0).max(120.0));
    let galley = ui.painter().layout_job(job);
    if galley.elided {
        resp.on_hover_text(desc);
    }
    let gap = 3.0;
    let top = rect.center().y - (title.size().y + gap + galley.size().y) / 2.0;
    let desc_top = top + title.size().y + gap;
    ui.painter().galley(pos2(rect.left(), top), title, p.text);
    ui.painter()
        .galley(pos2(rect.left(), desc_top), galley, p.muted);
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
