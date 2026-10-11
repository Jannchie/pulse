//! 配色、字体与全局样式。跟随系统深浅色。

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme,
    Visuals,
};

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
    /// 端口标签文字。
    pub chip: Color32,
    pub danger: Color32,
    /// 选中行底色的 accent 透明度。
    pub selection_alpha: f32,
    /// 端口标签底色的 accent 透明度。
    pub chip_alpha: f32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0f, 0x0f, 0x11),
    surface: Color32::from_rgb(0x18, 0x18, 0x1b),
    hover: Color32::from_rgb(0x1f, 0x1f, 0x23),
    track: Color32::from_rgb(0x26, 0x26, 0x2b),
    border: Color32::from_rgb(0x27, 0x27, 0x2c),
    text: Color32::from_rgb(0xf2, 0xf2, 0xf4),  // ≥ 14.5:1
    muted: Color32::from_rgb(0xc4, 0xc4, 0xcc), // ≥ 9.3:1
    faint: Color32::from_rgb(0xab, 0xab, 0xb5), // ≥ 7.1:1
    accent: Color32::from_rgb(0x4f, 0x8c, 0xff),
    chip: Color32::from_rgb(0xb3, 0xcd, 0xff),   // ≥ 8.5:1
    danger: Color32::from_rgb(0xff, 0xa3, 0xa3), // ≥ 8.6:1（只出现在按钮上）
    selection_alpha: 0.14,
    chip_alpha: 0.12,
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xf7, 0xf7, 0xf8),
    surface: Color32::from_rgb(0xff, 0xff, 0xff),
    hover: Color32::from_rgb(0xef, 0xef, 0xf2),
    track: Color32::from_rgb(0xe9, 0xe9, 0xee),
    border: Color32::from_rgb(0xe5, 0xe5, 0xea),
    text: Color32::from_rgb(0x11, 0x11, 0x13),  // ≥ 15.6:1
    muted: Color32::from_rgb(0x3f, 0x3f, 0x47), // ≥ 8.6:1
    faint: Color32::from_rgb(0x4b, 0x4b, 0x53), // ≥ 7.1:1
    accent: Color32::from_rgb(0x25, 0x63, 0xeb),
    chip: Color32::from_rgb(0x1a, 0x37, 0x99),   // ≥ 7.5:1
    danger: Color32::from_rgb(0x9b, 0x1c, 0x1c), // ≥ 7.1:1（只出现在按钮上）
    selection_alpha: 0.09,
    chip_alpha: 0.09,
};

/// 各指标的主题色，卡片曲线与表格热力底色共用。
pub mod metric {
    use eframe::egui::Color32;
    pub const CPU: Color32 = Color32::from_rgb(0x4f, 0x8c, 0xff);
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

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
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
        // 常驻滚动条：浮动滚动条悬停时有淡入动画，鼠标一动就连续重绘；而且会盖住最右一列。
        s.spacing.scroll = egui::style::ScrollStyle::solid();
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
    v.selection.bg_fill = p.accent.gamma_multiply(p.selection_alpha);
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

/// Segoe UI 负责西文与数字（等宽数字，列对齐好看），微软雅黑兜底中文。
/// 全部内存映射加载：字体文件页可与其他进程共享，不计入私有内存。
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut add = |key: &str, file: &str| {
        let Some(bytes) = map_font(file) else {
            return false;
        };
        fonts
            .font_data
            .insert(key.into(), Arc::new(egui::FontData::from_static(bytes)));
        true
    };
    let ui = add("segoe", "segoeui.ttf");
    let ui_sb = add("segoe-sb", "seguisb.ttf");
    let cjk = add("msyh", "msyh.ttc");
    let cjk_b = add("msyh-b", "msyhbd.ttc");

    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    if ui {
        prop.insert(0, "segoe".into());
    }
    if cjk {
        prop.push("msyh".into());
    }
    let defaults = prop.clone();
    let sb = fonts.families.entry(semibold()).or_default();
    if ui_sb {
        sb.push("segoe-sb".into());
    }
    if cjk_b {
        sb.push("msyh-b".into());
    }
    sb.extend(defaults);
    if cjk {
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("msyh".into());
    }
    ctx.set_fonts(fonts);
}

fn map_font(file: &str) -> Option<&'static [u8]> {
    let path = std::path::Path::new(r"C:\Windows\Fonts").join(file);
    let file = std::fs::File::open(path).ok()?;
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    Some(Box::leak(Box::new(map)))
}
