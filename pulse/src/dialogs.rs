//! 结束进程前的确认对话框，以及操作结果的通知（右下角，几秒后自动消失）。

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Order, RichText,
    Sense, Shadow, Stroke, Ui, vec2,
};
use pulse_probe::Snapshot;

use crate::icons_gen::Icon;
use crate::theme::{Palette, semibold};
use crate::widgets;

/// 待确认的结束请求。
pub struct KillRequest {
    pub pids: Vec<u32>,
    /// 对话框标题里的对象，例如 "node.exe (1234)"、"msedge.exe 的 29 个进程"。
    pub label: String,
}

/// 结束这些进程会让系统崩溃或注销当前会话。
const CRITICAL: &[&str] = &[
    "system",
    "registry",
    "secure system",
    "memory compression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
];

/// 白字在其上 ≥ 7:1（AAA）。
const DANGER_FILL: Color32 = Color32::from_rgb(0x99, 0x1b, 0x1b);
const DANGER_FILL_HOVER: Color32 = Color32::from_rgb(0x7f, 0x1d, 0x1d);

pub enum Decision {
    Confirm,
    Cancel,
}

/// 绘制确认对话框；返回用户的决定（尚未决定时为 None）。
pub fn confirm_kill(ctx: &egui::Context, req: &KillRequest, snap: &Snapshot) -> Option<Decision> {
    let p = Palette::of(ctx);
    let targets: Vec<_> = req
        .pids
        .iter()
        .filter_map(|pid| snap.processes.iter().find(|pr| pr.pid == *pid))
        .collect();
    let critical: Vec<&str> = targets
        .iter()
        .filter(|pr| CRITICAL.contains(&pr.name.to_lowercase().as_str()))
        .map(|pr| pr.name.as_str())
        .collect();
    let includes_self = req.pids.contains(&std::process::id());

    let frame = Frame::NONE
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(20))
        .shadow(Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: Color32::from_black_alpha(90),
        });
    let mut decision = None;
    let modal = egui::Modal::new(Id::new("kill-confirm"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(110))
        .show(ctx, |ui| {
            ui.set_width(400.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                widgets::icon(ui, Icon::AlertTriangle, r.center(), 20.0, p.danger);
                ui.label(
                    RichText::new(format!("结束 {}？", req.label))
                        .size(16.0)
                        .family(semibold())
                        .color(p.text),
                );
            });
            ui.add_space(6.0);

            let n = targets.len();
            ui.label(
                RichText::new(if n == 1 {
                    "将立即结束以下进程：".to_string()
                } else {
                    format!("将立即结束以下 {n} 个进程：")
                })
                .color(p.muted),
            );
            const SHOWN: usize = 6;
            Frame::NONE
                .fill(p.bg)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for pr in targets.iter().take(SHOWN) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&pr.name).color(p.text));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!("PID {}", pr.pid))
                                        .size(12.0)
                                        .color(p.faint),
                                );
                            });
                        });
                    }
                    if n > SHOWN {
                        ui.label(
                            RichText::new(format!("等 {} 个", n - SHOWN))
                                .size(12.0)
                                .color(p.faint),
                        );
                    }
                    if n == 0 {
                        ui.label(RichText::new("进程已经退出").size(12.0).color(p.faint));
                    }
                });

            ui.add_space(2.0);
            ui.label(
                RichText::new("进程不会收到正常退出的机会，未保存的数据将会丢失。")
                    .size(12.5)
                    .color(p.muted),
            );
            if !critical.is_empty() {
                warning(
                    ui,
                    &format!(
                        "{} 是系统关键进程，结束它可能导致蓝屏或注销。",
                        critical.join("、")
                    ),
                );
            }
            if includes_self {
                warning(ui, "其中包含 Pulse 自己，结束后本窗口会关闭。");
            }

            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if button(ui, "结束进程", true).clicked() || ui.input(|i| i.key_pressed(Key::Enter))
                {
                    decision = Some(Decision::Confirm);
                }
                if button(ui, "取消", false).clicked() {
                    decision = Some(Decision::Cancel);
                }
                ui.label(
                    RichText::new("Enter 确认 · Esc 取消")
                        .size(11.5)
                        .color(p.faint),
                );
            });
        });
    // 点遮罩或按 Esc 视为取消。
    if decision.is_none() && modal.should_close() {
        decision = Some(Decision::Cancel);
    }
    decision
}

fn warning(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        widgets::icon(ui, Icon::AlertCircle, r.center(), 15.0, p.danger);
        ui.label(RichText::new(text).size(12.5).color(p.danger));
    });
}

fn button(ui: &mut Ui, label: &str, danger: bool) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::new(13.0, semibold()),
        Color32::WHITE,
    );
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 32.0, 34.0), Sense::click());
    let (fill, fg) = match (danger, resp.hovered()) {
        (true, false) => (DANGER_FILL, Color32::WHITE),
        (true, true) => (DANGER_FILL_HOVER, Color32::WHITE),
        (false, false) => (p.track, p.text),
        (false, true) => (p.hover, p.text),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

// ---------------------------------------------------------------- 通知

#[derive(Clone, Copy, PartialEq)]
pub enum ToastKind {
    Success,
    Error,
}

pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    pub detail: Option<String>,
    pub at: Instant,
}

const TOAST_TTL: Duration = Duration::from_secs(4);
const TOAST_ERROR_TTL: Duration = Duration::from_secs(7);

impl Toast {
    fn ttl(&self) -> Duration {
        if self.kind == ToastKind::Error {
            TOAST_ERROR_TTL
        } else {
            TOAST_TTL
        }
    }
}

/// 在窗口右下角堆叠显示通知，过期的自动移除。
pub fn toasts(ctx: &egui::Context, toasts: &mut Vec<Toast>) {
    toasts.retain(|t| t.at.elapsed() < t.ttl());
    if toasts.is_empty() {
        return;
    }
    let p = Palette::of(ctx);
    let mut dismissed = None;
    let mut y = -44.0;
    for (i, t) in toasts.iter().enumerate().rev().take(3) {
        let (glyph, accent) = match t.kind {
            ToastKind::Success => (Icon::CheckCircle, crate::theme::metric::GPU),
            ToastKind::Error => (Icon::XCircle, p.danger),
        };
        let resp = egui::Area::new(Id::new(("toast", i)))
            .order(Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, y))
            .interactable(true)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: Color32::from_black_alpha(80),
                    })
                    .show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                            widgets::icon(ui, glyph, r.center(), 17.0, accent);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(&t.title).color(p.text));
                                if let Some(d) = &t.detail {
                                    ui.label(RichText::new(d).size(12.0).color(p.muted));
                                }
                            });
                        });
                    });
            })
            .response;
        if resp
            .interact(Sense::click())
            .on_hover_text("点击关闭")
            .clicked()
        {
            dismissed = Some(i);
        }
        y -= resp.rect.height() + 8.0;
    }
    if let Some(i) = dismissed {
        toasts.remove(i);
    }
    // 到点后需要再画一帧把通知收走。
    if let Some(next) = toasts
        .iter()
        .map(|t| t.ttl().saturating_sub(t.at.elapsed()))
        .min()
    {
        ctx.request_repaint_after(next);
    }
}
