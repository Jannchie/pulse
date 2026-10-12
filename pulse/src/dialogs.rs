//! 结束进程前的确认对话框，以及操作结果的通知（右下角，几秒后自动消失）。

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Order, RichText,
    Sense, Shadow, Stroke, Ui, vec2,
};
use pulse_probe::{Snapshot, t, tf};

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

    modal(ctx, "kill-confirm", 400.0, |ui| {
        let mut decision = None;
        ui.spacing_mut().item_spacing.y = 6.0;
        danger_title(
            ui,
            &tf!("结束 {}？", "End {}?", "{} を終了しますか？", req.label),
        );
        ui.add_space(6.0);

        let n = targets.len();
        ui.label(
            RichText::new(if n == 1 {
                t!(
                    "将立即结束以下进程：",
                    "This process will be ended immediately:",
                    "次のプロセスを直ちに終了します："
                )
                .to_string()
            } else {
                tf!(
                    "将立即结束以下 {n} 个进程：",
                    "These {n} processes will be ended immediately:",
                    "次の {n} 個のプロセスを直ちに終了します："
                )
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
                        RichText::new(tf!("等 {} 个", "and {} more", "ほか {} 個", n - SHOWN))
                            .size(12.0)
                            .color(p.faint),
                    );
                }
                if n == 0 {
                    ui.label(
                        RichText::new(t!(
                            "进程已经退出",
                            "The process has already exited",
                            "プロセスはすでに終了しています"
                        ))
                        .size(12.0)
                        .color(p.faint),
                    );
                }
            });

        ui.add_space(2.0);
        ui.label(
            RichText::new(t!(
                "进程不会收到正常退出的机会，未保存的数据将会丢失。",
                "The process won't get a chance to exit normally. Unsaved data will be lost.",
                "プロセスは正常に終了できません。保存されていないデータは失われます。"
            ))
            .size(12.5)
            .color(p.muted),
        );
        if !critical.is_empty() {
            warning(
                ui,
                &tf!(
                    "{} 是系统关键进程，结束它可能导致蓝屏或注销。",
                    "{} is a critical system process. Ending it may crash Windows or sign you out.",
                    "{} はシステムの重要なプロセスです。終了すると Windows が停止したり、サインアウトされたりする可能性があります。",
                    critical.join(t!("、", ", ", "、"))
                ),
            );
        }
        if includes_self {
            warning(
                ui,
                t!(
                    "其中包含 Pulse 自己，结束后本窗口会关闭。",
                    "This includes Pulse itself. This window will close.",
                    "Pulse 自体も含まれています。終了するとこのウィンドウは閉じます。"
                ),
            );
        }

        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(
                ui,
                t!("结束进程", "End process", "プロセスを終了する"),
                Kind::Danger,
                true,
            )
            .clicked()
                || ui.input(|i| i.key_pressed(Key::Enter))
            {
                decision = Some(Decision::Confirm);
            }
            if button(
                ui,
                t!("取消", "Cancel", "キャンセル"),
                Kind::Secondary,
                true,
            )
            .clicked()
            {
                decision = Some(Decision::Cancel);
            }
            ui.label(
                RichText::new(t!(
                    "Enter 确认 · Esc 取消",
                    "Enter to confirm · Esc to cancel",
                    "Enter で確定 · Esc でキャンセル"
                ))
                .size(11.5)
                .color(p.faint),
            );
        });
        decision
    })
}

fn warning(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        widgets::icon(ui, Icon::AlertCircle, r.center(), 15.0, p.danger);
        ui.label(RichText::new(text).size(12.5).color(p.danger));
    });
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Primary,
    Danger,
    Secondary,
}

/// 白字在其上 ≥ 7:1（AAA）。
const PRIMARY_FILL: Color32 = Color32::from_rgb(0x1e, 0x40, 0xaf);
const PRIMARY_FILL_HOVER: Color32 = Color32::from_rgb(0x1e, 0x3a, 0x8a);

pub fn button(ui: &mut Ui, label: &str, kind: Kind, enabled: bool) -> egui::Response {
    let p = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::new(13.0, semibold()),
        Color32::WHITE,
    );
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) =
        ui.allocate_exact_size(vec2((galley.size().x + 32.0).max(72.0), 34.0), sense);
    let hovered = enabled && resp.hovered();
    let (fill, fg) = match (kind, enabled, hovered) {
        (_, false, _) => (p.track, p.faint),
        (Kind::Primary, _, false) => (PRIMARY_FILL, Color32::WHITE),
        (Kind::Primary, _, true) => (PRIMARY_FILL_HOVER, Color32::WHITE),
        (Kind::Danger, _, false) => (DANGER_FILL, Color32::WHITE),
        (Kind::Danger, _, true) => (DANGER_FILL_HOVER, Color32::WHITE),
        (Kind::Secondary, _, false) => (p.track, p.text),
        (Kind::Secondary, _, true) => (p.hover, p.text),
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
    pub fn ok(title: String, detail: Option<String>) -> Self {
        Self {
            kind: ToastKind::Success,
            title,
            detail,
            at: Instant::now(),
        }
    }

    pub fn err(title: String, detail: Option<String>) -> Self {
        Self {
            kind: ToastKind::Error,
            title,
            detail,
            at: Instant::now(),
        }
    }

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
            .on_hover_text(t!("点击关闭", "Click to dismiss", "クリックして閉じる"))
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

// ---------------------------------------------------------------- 相关性

pub struct AffinityDialog {
    pub pid: u32,
    pub label: String,
    pub mask: u64,
    /// 系统允许的处理器掩码。
    pub system: u64,
}

/// 统一的模态框：外框、遮罩，点遮罩或按 Esc 视为取消。`body` 返回用户的决定。
fn modal(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    body: impl FnOnce(&mut Ui) -> Option<Decision>,
) -> Option<Decision> {
    let p = Palette::of(ctx);
    let resp = egui::Modal::new(Id::new(id))
        .frame(dialog_frame(&p))
        .backdrop_color(Color32::from_black_alpha(110))
        .show(ctx, |ui| {
            ui.set_width(width);
            // 对话框里 egui 自带的复选框、输入框：底色换成轨道色并加描边，否则和对话框底色融为一体。
            let w = &mut ui.visuals_mut().widgets;
            w.inactive.bg_fill = p.track;
            w.inactive.weak_bg_fill = p.track;
            w.inactive.bg_stroke = Stroke::new(1.0, p.border);
            w.hovered.bg_fill = p.hover;
            w.hovered.bg_stroke = Stroke::new(1.0, p.faint);
            body(ui)
        });
    let closed = resp.should_close();
    resp.inner.or(closed.then_some(Decision::Cancel))
}

/// 警告图标 + 标题。
fn danger_title(ui: &mut Ui, title: &str) {
    let p = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        widgets::icon(ui, Icon::AlertTriangle, r.center(), 20.0, p.danger);
        ui.label(
            RichText::new(title)
                .size(16.0)
                .family(semibold())
                .color(p.text),
        );
    });
}

fn dialog_frame(p: &Palette) -> Frame {
    Frame::NONE
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(20))
        .shadow(Shadow {
            offset: [0, 12],
            blur: 32,
            spread: 0,
            color: Color32::from_black_alpha(90),
        })
}

pub fn affinity(ctx: &egui::Context, d: &mut AffinityDialog) -> Option<Decision> {
    let p = Palette::of(ctx);
    modal(ctx, "affinity", 440.0, |ui| {
        let mut decision = None;
        ui.label(
            RichText::new(t!("处理器相关性", "Processor affinity", "プロセッサの関係"))
                .size(16.0)
                .family(semibold())
                .color(p.text),
        );
        ui.label(
            RichText::new(tf!(
                "允许 {} 在哪些处理器上运行",
                "Choose which processors {} can run on",
                "{} の実行を許可するプロセッサを選択してください",
                d.label
            ))
            .size(12.5)
            .color(p.muted),
        );
        ui.add_space(10.0);
        let cpus: Vec<u32> = (0..64).filter(|i| d.system & (1u64 << i) != 0).collect();
        let cols = 6;
        egui::Grid::new("affinity-grid")
            .spacing(vec2(8.0, 6.0))
            .show(ui, |ui| {
                for (n, i) in cpus.iter().enumerate() {
                    let mut on = d.mask & (1u64 << i) != 0;
                    if ui.checkbox(&mut on, format!("CPU {i}")).changed() {
                        d.mask ^= 1u64 << i;
                    }
                    if (n + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.link(t!("全选", "Select all", "すべて選択")).clicked() {
                d.mask = d.system;
            }
            ui.label(
                RichText::new(tf!(
                    "已选 {} / {}",
                    "{} of {} selected",
                    "選択済み {} / {}",
                    (d.mask & d.system).count_ones(),
                    cpus.len()
                ))
                .size(12.0)
                .color(p.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let valid = d.mask & d.system != 0;
                if button(ui, t!("确定", "OK", "OK"), Kind::Primary, valid).clicked()
                    || (valid && ui.input(|i| i.key_pressed(Key::Enter)))
                {
                    decision = Some(Decision::Confirm);
                }
                if button(
                    ui,
                    t!("取消", "Cancel", "キャンセル"),
                    Kind::Secondary,
                    true,
                )
                .clicked()
                {
                    decision = Some(Decision::Cancel);
                }
            });
        });
        decision
    })
}

// ---------------------------------------------------------------- 运行新任务

#[derive(Default)]
pub struct RunDialog {
    pub command: String,
    pub admin: bool,
    pub error: Option<String>,
    /// 打开后第一帧把焦点给输入框。
    pub focused: bool,
}

pub fn run_task(ctx: &egui::Context, d: &mut RunDialog, elevated: bool) -> Option<Decision> {
    let p = Palette::of(ctx);
    modal(ctx, "run-task", 440.0, |ui| {
        let mut decision = None;
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            widgets::icon(ui, Icon::Plus, r.center(), 20.0, p.accent);
            ui.label(
                RichText::new(t!("运行新任务", "Run new task", "新しいタスクを実行する"))
                    .size(16.0)
                    .family(semibold())
                    .color(p.text),
            );
        });
        ui.label(
            RichText::new(t!(
                "输入程序、文件夹、文档或网址，Windows 会用对应的程序打开。",
                "Type the name of a program, folder, document or website, and Windows will open it.", "実行するプログラム、または開くフォルダー、ドキュメント、Web サイトの名前を入力してください。Windows が対応するプログラムで開きます。"
            ))
            .size(12.5)
            .color(p.muted),
        );
        ui.add_space(8.0);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut d.command)
                .hint_text(t!(
                    r"例如 notepad、cmd、C:\Windows、https://…",
                    r"e.g. notepad, cmd, C:\Windows, https://…",
                    r"例：notepad、cmd、C:\Windows、https://…"
                ))
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(10, 8)),
        );
        if !d.focused {
            edit.request_focus();
            d.focused = true;
        }
        ui.add_space(6.0);
        if elevated {
            ui.label(
                RichText::new(t!(
                    "Pulse 正以管理员身份运行，新任务也会继承管理员权限。",
                    "Pulse is running as administrator, so the new task will be too.", "Pulse は管理者として実行されているため、新しいタスクも管理者権限で実行されます。"
                ))
                .size(12.0)
                .color(p.faint),
            );
        } else {
            ui.checkbox(
                &mut d.admin,
                t!(
                    "以管理员身份运行",
                    "Create this task with administrative privileges",
                    "このタスクに管理者特権を付与して作成する"
                ),
            );
        }
        if let Some(e) = &d.error {
            ui.label(RichText::new(e).size(12.5).color(p.danger));
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let ready = !d.command.trim().is_empty();
            if button(ui, t!("运行", "Run", "実行"), Kind::Primary, ready).clicked()
                || (ready && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                decision = Some(Decision::Confirm);
            }
            if button(
                ui,
                t!("取消", "Cancel", "キャンセル"),
                Kind::Secondary,
                true,
            )
            .clicked()
            {
                decision = Some(Decision::Cancel);
            }
        });
        decision
    })
}

// ---------------------------------------------------------------- 通用确认

/// 通用的危险操作确认框（注销用户等）。
pub fn confirm(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    body: &str,
    button_label: &str,
) -> Option<Decision> {
    let p = Palette::of(ctx);
    modal(ctx, ("confirm", id), 400.0, |ui| {
        let mut decision = None;
        danger_title(ui, title);
        ui.add_space(6.0);
        ui.label(RichText::new(body).size(13.0).color(p.muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(ui, button_label, Kind::Danger, true).clicked() {
                decision = Some(Decision::Confirm);
            }
            if button(
                ui,
                t!("取消", "Cancel", "キャンセル"),
                Kind::Secondary,
                true,
            )
            .clicked()
            {
                decision = Some(Decision::Cancel);
            }
        });
        decision
    })
}
