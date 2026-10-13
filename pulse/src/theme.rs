//! 配色、字体与全局样式。跟随系统深浅色。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme,
    Visuals,
};
use eframe::epaint::text::VariationCoords;
use pulse_probe::i18n::Lang;

use crate::fonts::{self, Face};

/// 文字色均满足 WCAG AAA（≥ 7:1），覆盖它们会出现的全部背景：窗口、卡片、悬停行、
/// 选中行（`selection` 叠在 `bg` 上）以及端口标签底色。改色后请重新核对对比度。
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub hover: Color32,
    /// 进度条、仪表盘的轨道色。
    pub track: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub danger: Color32,
    /// 选中行底色的 accent 透明度。
    pub selection_alpha: f32,
}

/// 配色对齐 Windows 11 资源管理器、任务管理器的中性灰（不带色偏）；次要文字对比度不低于 7:1。
const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x20, 0x20, 0x20),
    surface: Color32::from_rgb(0x2b, 0x2b, 0x2b),
    hover: Color32::from_rgb(0x33, 0x33, 0x33),
    track: Color32::from_rgb(0x3a, 0x3a, 0x3a),
    border: Color32::from_rgb(0x38, 0x38, 0x38),
    text: Color32::from_rgb(0xff, 0xff, 0xff),  // ≥ 12.6:1
    muted: Color32::from_rgb(0xd6, 0xd6, 0xd6), // ≥ 8.6:1
    faint: Color32::from_rgb(0xc2, 0xc2, 0xc2), // ≥ 7.0:1
    accent: Color32::from_rgb(0x4f, 0x8c, 0xff),
    danger: Color32::from_rgb(0xff, 0xb4, 0xb4), // ≥ 7.4:1（只出现在按钮上）
    selection_alpha: 0.10,
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf3, 0xf3, 0xf3),
    surface: Color32::from_rgb(0xfb, 0xfb, 0xfb),
    hover: Color32::from_rgb(0xe9, 0xe9, 0xe9),
    track: Color32::from_rgb(0xe3, 0xe3, 0xe3),
    border: Color32::from_rgb(0xe5, 0xe5, 0xe5),
    text: Color32::from_rgb(0x1b, 0x1b, 0x1b),  // ≥ 14.1:1
    muted: Color32::from_rgb(0x3d, 0x3d, 0x3d), // ≥ 8.9:1
    faint: Color32::from_rgb(0x4a, 0x4a, 0x4a), // ≥ 7.3:1
    accent: Color32::from_rgb(0x25, 0x63, 0xeb),
    danger: Color32::from_rgb(0x8a, 0x18, 0x18), // ≥ 7.7:1（只出现在按钮上）
    selection_alpha: 0.07,
};

/// 各指标的主题色，卡片曲线与表格热力底色共用。
/// 进程表的热力底色。
pub const HEAT: Color32 = Color32::from_rgb(0xf2, 0xa9, 0x3b);

pub mod metric {
    use eframe::egui::Color32;
    pub const CPU: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xff);
    /// 混合架构的能效核（小核）。
    pub const CPU_EFFICIENT: Color32 = Color32::from_rgb(0x38, 0xbd, 0xf8);
    pub const MEMORY: Color32 = Color32::from_rgb(0xa4, 0x6b, 0xff);
    pub const GPU: Color32 = Color32::from_rgb(0x10, 0xb9, 0x81);
    pub const DISK: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
    pub const NETWORK: Color32 = Color32::from_rgb(0x06, 0xb6, 0xd4);
    /// 网络卡片里的上传方向。
    pub const NETWORK_UP: Color32 = Color32::from_rgb(0xf4, 0x72, 0xb6);
}

impl Palette {
    pub fn of(ctx: &egui::Context) -> Self {
        match ctx.theme() {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// 进程、服务表格的行高。滚轮一格滚 3 行，与 Windows 默认一致。
pub const ROW_HEIGHT: f32 = 30.0;

pub fn install(ctx: &egui::Context, lang: Lang) {
    install_fonts(ctx, lang);
    ctx.options_mut(|o| o.input_options.line_scroll_speed = ROW_HEIGHT * 3.0);
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(20.0, semibold())),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            ),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.menu_margin = Margin::same(6);
        s.interaction.selectable_labels = false;
        s.interaction.tooltip_delay = 0.4;
        // 默认要等指针"静止"才弹 tooltip，期间每帧 request_repaint，鼠标在表格上一动就按刷新率连续重绘。
        // 改为纯延时触发，重绘只跟随真实输入事件。
        s.interaction.show_tooltips_only_when_still = false;
        // 关掉动画（平滑滚动、悬停淡入等）。动画期间 egui 每帧都请求重绘，而我们关了 vsync，
        // 没有动画就保证了重绘只由真实输入触发。
        s.animation_time = 0.0;
        s.scroll_animation = egui::style::ScrollAnimation::none();
        // 滚动条：平时 4px 细条，悬停变 8px。滑块用正文色按透明度绘制（深浅主题下与背景都 ≥ 3:1），
        // 默认的滑块取控件底色，和窗口底色几乎一样，看不见。
        // 预留 floating_allocated_width，滚动条不压住最右一列；animation_time 为 0，悬停变宽不会连续重绘。
        s.spacing.scroll = egui::style::ScrollStyle {
            floating: true,
            bar_width: 8.0,
            floating_width: 4.0,
            floating_allocated_width: 10.0,
            handle_min_length: 28.0,
            bar_inner_margin: 2.0,
            bar_outer_margin: 2.0,
            foreground_color: true,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.6,
            dormant_handle_opacity: 0.5,
            active_handle_opacity: 0.65,
            interact_handle_opacity: 0.8,
            ..egui::style::ScrollStyle::floating()
        };
    });
}

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(10);
    let shadow_alpha = if dark { 120 } else { 28 };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.window_shadow = v.popup_shadow;
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    // 选中行用中性的浅灰（与任务管理器、资源管理器一致），不染强调色。
    v.selection.bg_fill = p.text.gamma_multiply(p.selection_alpha);
    // 表格选中行用这个颜色画文字，保持正文色即可。
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.striped = false;

    let w = &mut v.widgets;
    for (wv, fill) in [
        (&mut w.noninteractive, p.bg),
        (&mut w.inactive, p.surface),
        (&mut w.hovered, p.hover),
        (&mut w.active, p.hover),
        (&mut w.open, p.hover),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = Stroke::NONE;
        wv.corner_radius = CornerRadius::same(8);
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.expansion = 0.0;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v
}

/// 西文字体，所有语言共用。
const LATIN: &[&str] = &["HarmonyOS Sans", "Noto Sans", "Segoe UI"];
/// 中文字形的汉字字体。英文界面也用它：进程名、文件名里照样有汉字。
const CJK_SC: &[&str] = &[
    "HarmonyOS Sans SC",
    "Noto Sans SC",
    "Noto Sans CJK SC",
    "Microsoft YaHei",
    "DengXian",
    "SimSun",
];
/// 日文字形的汉字字体。HarmonyOS Sans SC 只有中文字形，不能排在日文前面。
const CJK_JP: &[&str] = &["Noto Sans JP", "Noto Sans CJK JP", "Yu Gothic UI", "Meiryo"];

const REGULAR: f32 = 400.0;
/// 加粗标题的目标字重：HarmonyOS 取 Medium，Segoe UI / Yu Gothic UI 取 Semibold，
/// 只有常规与粗体的字体（微软雅黑、Meiryo）取粗体。
const SEMIBOLD: f32 = 550.0;

/// 一种语言的字体链：西文在前，界面语言的汉字字体其次，另一种字形的汉字字体兜底
/// （日文字体缺简体字，中文字体也可能缺个别日文汉字）。
struct Chain {
    latin: Vec<Face>,
    cjk: Vec<Face>,
}

fn chain(lang: Lang, weight: f32) -> Chain {
    let (first, second) = match lang {
        Lang::Ja => (CJK_JP, CJK_SC),
        Lang::Zh | Lang::En => (CJK_SC, CJK_JP),
    };
    let mut latin: Vec<Face> = fonts::pick(LATIN, weight).into_iter().collect();
    // 首选西文字体缺的符号由 Segoe UI 补上。
    if latin.first().is_none_or(|f| f.family != "Segoe UI") {
        latin.extend(fonts::pick(&["Segoe UI"], weight));
    }
    let cjk = [first, second]
        .into_iter()
        .filter_map(|list| fonts::pick(list, weight))
        .collect();
    Chain { latin, cjk }
}

/// 按语言装字体。切换语言时再调用一次即可：字体文件只映射一次，之后复用。
pub fn install_fonts(ctx: &egui::Context, lang: Lang) {
    static CHAINS: OnceLock<Mutex<HashMap<Lang, (Chain, Chain)>>> = OnceLock::new();
    let mut chains = CHAINS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (regular, bold) = chains
        .entry(lang)
        .or_insert_with(|| (chain(lang, REGULAR), chain(lang, SEMIBOLD)));

    let mut fonts = egui::FontDefinitions::default();
    let mut add = |face: &Face| -> Option<String> {
        let bytes = face.bytes()?;
        let key = face.key();
        let mut tweak = egui::FontTweak::default();
        if let Some(w) = face.wght {
            tweak.coords = VariationCoords::new([(b"wght", w)]);
        }
        fonts.font_data.insert(
            key.clone(),
            Arc::new(egui::FontData {
                font: std::borrow::Cow::Borrowed(bytes),
                index: face.index,
                tweak,
            }),
        );
        Some(key)
    };
    let latin: Vec<String> = regular.latin.iter().filter_map(&mut add).collect();
    let cjk: Vec<String> = regular.cjk.iter().filter_map(&mut add).collect();
    let latin_b: Vec<String> = bold.latin.iter().filter_map(&mut add).collect();
    let cjk_b: Vec<String> = bold.cjk.iter().filter_map(&mut add).collect();

    // egui 自带的字体（含 emoji）留在最后兜底。
    let defaults = fonts.families[&FontFamily::Proportional].clone();
    let prop: Vec<String> = latin.iter().chain(&cjk).cloned().chain(defaults).collect();
    let sb: Vec<String> = latin_b
        .into_iter()
        .chain(cjk_b)
        .chain(prop.clone())
        .collect();
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(semibold(), sb);
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .extend(cjk);
    ctx.set_fonts(fonts);
}
