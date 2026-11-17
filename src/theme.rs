//! vhs-gui redesign: theme tokens for egui 0.34 (from the Claude Design handoff, `tokens.json`).
//!
//! `install_fonts(ctx, bundled_fonts())` then `apply(ctx)` run once from `App::new`. Every colour in
//! the redesign is a constant in `color`; nothing in the panels uses a literal `Color32` or
//! `Color32::RED` / `YELLOW` (a test in this file keeps it that way).
//!
//! Not every helper is used yet: the redesign lands in phases, and each one starts using more of this
//! module. `main.rs` allows dead code on it until the last phase, which removes that.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText, Shadow,
    Stroke, TextStyle, Vec2, Visuals,
};
use std::sync::Arc;

pub mod color {
    use egui::Color32;
    /// Central area, text field fill. `Visuals::extreme_bg_color`.
    pub const BG_EXTREME: Color32 = Color32::from_rgb(0x0A, 0x0B, 0x12);
    /// Side panels. `Visuals::panel_fill`.
    pub const BG_PANEL: Color32 = Color32::from_rgb(0x12, 0x13, 0x1E);
    /// Rail, toolbar, status bar, floating windows. `Visuals::window_fill`.
    pub const BG_RAISED: Color32 = Color32::from_rgb(0x18, 0x1A, 0x28);
    /// Behind the picture.
    pub const VIDEO_BLACK: Color32 = Color32::from_rgb(0x05, 0x05, 0x09);

    pub const WIDGET_IDLE: Color32 = Color32::from_rgb(0x23, 0x26, 0x39);
    pub const WIDGET_HOVERED: Color32 = Color32::from_rgb(0x2D, 0x31, 0x4A);
    pub const WIDGET_ACTIVE: Color32 = Color32::from_rgb(0x3A, 0x3F, 0x5E);
    /// Selected list row. `Visuals::selection.bg_fill`.
    pub const SELECTION: Color32 = Color32::from_rgb(0x3B, 0x1E, 0x3A);

    pub const STROKE_SUBTLE: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x42);
    pub const STROKE_WIDGET: Color32 = Color32::from_rgb(0x44, 0x49, 0x68);

    pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xF3, 0xF2, 0xF8);
    pub const TEXT_NORMAL: Color32 = Color32::from_rgb(0xC9, 0xCA, 0xDB);
    pub const TEXT_WEAK: Color32 = Color32::from_rgb(0x94, 0x97, 0xB0);
    /// Disabled controls: this text, a dashed 1 px `STROKE_SUBTLE` outline, no fill.
    pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x83, 0x87, 0xA0);

    pub const ACCENT: Color32 = Color32::from_rgb(0xF2, 0x7A, 0xCB);
    pub const ACCENT_HOVERED: Color32 = Color32::from_rgb(0xF7, 0x96, 0xD8);
    pub const ACCENT_PRESSED: Color32 = Color32::from_rgb(0xDD, 0x62, 0xB6);
    /// Text and icons on an accent fill.
    pub const ACCENT_INK: Color32 = Color32::from_rgb(0x2A, 0x0A, 0x22);

    /// Destructive outline and text, failure text. `Visuals::error_fg_color`.
    pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x64, 0x75);
    /// Recording toolbar, REC chip, armed confirm button. White text on top.
    pub const DANGER_FILL: Color32 = Color32::from_rgb(0xB2, 0x1F, 0x3A);
    /// Failure banner fill.
    pub const DANGER_DIM: Color32 = Color32::from_rgb(0x3A, 0x14, 0x20);

    /// Missing tool, orphan capture, paused. `Visuals::warn_fg_color`.
    pub const WARNING: Color32 = Color32::from_rgb(0xF2, 0xBC, 0x57);
    pub const WARNING_DIM: Color32 = Color32::from_rgb(0x3A, 0x2C, 0x10);

    pub const SUCCESS: Color32 = Color32::from_rgb(0x62, 0xD6, 0xA4);
    pub const SUCCESS_DIM: Color32 = Color32::from_rgb(0x12, 0x33, 0x2A);

    /// No fill (outline-only buttons, unselected rail items).
    pub const TRANSPARENT: Color32 = Color32::TRANSPARENT;
    /// Text and icons on `DANGER_FILL` (the recording toolbar, REC chip, armed confirm).
    pub const ON_DANGER: Color32 = Color32::WHITE;
}

pub mod size {
    pub const RAIL_WIDTH: f32 = 56.0;
    pub const TOOLBAR_HEIGHT: f32 = 40.0;
    pub const STATUS_BAR_HEIGHT: f32 = 36.0;
    pub const SIDE_PANEL_WIDTH: f32 = 232.0;
    pub const SIDE_PANEL_MIN: f32 = 220.0;
    pub const SIDE_PANEL_MAX: f32 = 320.0;
    pub const CONTROL_HEIGHT: f32 = 26.0;
    pub const PRIMARY_HEIGHT: f32 = 32.0;
    pub const NEXT_STEP_HEIGHT: f32 = 36.0;
    pub const LIST_ROW_HEIGHT: f32 = 24.0;
    pub const RADIUS_WIDGET: u8 = 4;
    pub const RADIUS_WINDOW: u8 = 6;
    pub const RADIUS_BADGE: u8 = 3;
    pub const RADIUS_BAR: u8 = 2;
    /// Spacing scale. Use these, not ad hoc values.
    pub const SPACE: [f32; 7] = [2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0];
    pub const MIN_WINDOW: [f32; 2] = [944.0, 516.0];
}

/// Named font families for the weights egui cannot synthesize.
pub const FAMILY_MEDIUM: &str = "inter-medium";
pub const FAMILY_SEMIBOLD: &str = "inter-semibold";
pub const FAMILY_BOLD: &str = "inter-bold";
pub const FAMILY_MONO_BOLD: &str = "mono-bold";
pub const FAMILY_ICONS: &str = "phosphor";

/// Custom text styles beyond egui's five.
pub const STYLE_CLOCK: &str = "clock";
pub const STYLE_SECTION: &str = "section";

/// Static font files (SIL OFL), passed in so this module does not fix their location.
pub struct Fonts {
    pub inter_regular: &'static [u8],
    pub inter_medium: &'static [u8],
    pub inter_semibold: &'static [u8],
    pub inter_bold: &'static [u8],
    pub mono_regular: &'static [u8],
    pub mono_bold: &'static [u8],
    /// Phosphor Regular TTF, for example `egui_phosphor::Variant::Regular.font_bytes()`.
    pub icons: &'static [u8],
}

/// The fonts embedded in the binary (`assets/fonts/`, SIL OFL) and the Phosphor icon font.
pub fn bundled_fonts() -> Fonts {
    Fonts {
        inter_regular: include_bytes!("../assets/fonts/Inter-Regular.ttf"),
        inter_medium: include_bytes!("../assets/fonts/Inter-Medium.ttf"),
        inter_semibold: include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
        inter_bold: include_bytes!("../assets/fonts/Inter-Bold.ttf"),
        mono_regular: include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        mono_bold: include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
        icons: egui_phosphor::Variant::Regular.font_bytes(),
    }
}

pub fn install_fonts(ctx: &egui::Context, f: Fonts) {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        defs.font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("inter-regular", f.inter_regular);
    add("inter-medium", f.inter_medium);
    add("inter-semibold", f.inter_semibold);
    add("inter-bold", f.inter_bold);
    add("mono-regular", f.mono_regular);
    add("mono-bold", f.mono_bold);
    add("phosphor", f.icons);

    // Icons are a fallback in every family, so a Phosphor glyph can sit inside any label.
    let fam = |first: &str| vec![first.to_owned(), "phosphor".to_owned()];
    defs.families
        .entry(FontFamily::Proportional)
        .or_default()
        .splice(0..0, fam("inter-regular"));
    defs.families
        .entry(FontFamily::Monospace)
        .or_default()
        .splice(0..0, fam("mono-regular"));
    defs.families
        .insert(FontFamily::Name(FAMILY_MEDIUM.into()), fam("inter-medium"));
    defs.families.insert(
        FontFamily::Name(FAMILY_SEMIBOLD.into()),
        fam("inter-semibold"),
    );
    defs.families
        .insert(FontFamily::Name(FAMILY_BOLD.into()), fam("inter-bold"));
    defs.families
        .insert(FontFamily::Name(FAMILY_MONO_BOLD.into()), fam("mono-bold"));
    defs.families.insert(
        FontFamily::Name(FAMILY_ICONS.into()),
        vec!["phosphor".to_owned()],
    );
    ctx.set_fonts(defs);
}

pub fn visuals() -> Visuals {
    use color::*;
    let mut v = Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_RAISED;
    v.extreme_bg_color = BG_EXTREME;
    v.faint_bg_color = WIDGET_IDLE;
    v.code_bg_color = BG_EXTREME;
    v.window_stroke = line(STROKE_WIDGET);
    v.window_corner_radius = CornerRadius::same(size::RADIUS_WINDOW);
    v.menu_corner_radius = CornerRadius::same(size::RADIUS_WIDGET);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(128),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(128),
    };
    v.selection.bg_fill = SELECTION;
    v.selection.stroke = line(ACCENT);
    v.hyperlink_color = ACCENT;
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.weak_text_color = Some(TEXT_WEAK);

    let r = CornerRadius::same(size::RADIUS_WIDGET);
    let w = &mut v.widgets;

    w.noninteractive.bg_fill = BG_PANEL;
    w.noninteractive.weak_bg_fill = BG_PANEL;
    w.noninteractive.bg_stroke = line(STROKE_SUBTLE);
    w.noninteractive.fg_stroke = line(TEXT_NORMAL);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = WIDGET_IDLE;
    w.inactive.weak_bg_fill = WIDGET_IDLE;
    w.inactive.bg_stroke = line(STROKE_WIDGET);
    w.inactive.fg_stroke = line(TEXT_STRONG);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = WIDGET_HOVERED;
    w.hovered.weak_bg_fill = WIDGET_HOVERED;
    w.hovered.bg_stroke = line(STROKE_WIDGET);
    w.hovered.fg_stroke = line(TEXT_STRONG);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = WIDGET_ACTIVE;
    w.active.weak_bg_fill = WIDGET_ACTIVE;
    w.active.bg_stroke = line(ACCENT);
    w.active.fg_stroke = line(TEXT_STRONG);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = WIDGET_ACTIVE;
    w.open.weak_bg_fill = WIDGET_ACTIVE;
    w.open.bg_stroke = line(STROKE_WIDGET);
    w.open.fg_stroke = line(TEXT_STRONG);
    w.open.corner_radius = r;
    v
}

/// Apply visuals, spacing and the type scale. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals(visuals());
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.window_margin = Margin::same(12);
        style.spacing.interact_size.y = size::CONTROL_HEIGHT;
        style.spacing.indent = 16.0;
        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(16.0, FontFamily::Name(FAMILY_SEMIBOLD.into())),
            ),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(12.0, FontFamily::Name(FAMILY_MEDIUM.into())),
            ),
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(12.0, FontFamily::Monospace),
            ),
            (
                TextStyle::Name(STYLE_CLOCK.into()),
                FontId::new(34.0, FontFamily::Name(FAMILY_MONO_BOLD.into())),
            ),
            (
                TextStyle::Name(STYLE_SECTION.into()),
                FontId::new(10.5, FontFamily::Name(FAMILY_BOLD.into())),
            ),
        ]
        .into();
    });
}

// ---- button variants -----------------------------------------------------

/// Filled accent: the single recommended next step.
pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(color::ACCENT_INK))
        .fill(color::ACCENT)
        .stroke(line(color::ACCENT))
}

/// Accent outline: one of two equal next steps (Edit Master: Viewer Encode / VDecimate).
pub fn recommended_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(color::ACCENT))
        .fill(Color32::TRANSPARENT)
        .stroke(line(color::ACCENT))
}

/// Red outline: Cancel…, Stop it, Move to Trash… before it is armed.
pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(color::DANGER))
        .fill(Color32::TRANSPARENT)
        .stroke(line(color::DANGER))
}

/// Solid red, white text: the armed confirm, Start Capture, "Yes, move it".
pub fn danger_solid_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE))
        .fill(color::DANGER_FILL)
        .stroke(line(color::DANGER_FILL))
}

/// Section label: 10.5 px bold capitals in the weak text colour.
pub fn section_label(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .text_style(TextStyle::Name(STYLE_SECTION.into()))
        .color(color::TEXT_WEAK)
}

#[derive(Clone, Copy)]
pub enum Tone {
    Neutral,
    Accent,
    Danger,
    DangerSolid,
    Warning,
    Success,
}

impl Tone {
    /// (fill, text)
    pub fn colors(self) -> (Color32, Color32) {
        use color::*;
        match self {
            Tone::Neutral => (WIDGET_IDLE, TEXT_NORMAL),
            Tone::Accent => (SELECTION, ACCENT),
            Tone::Danger => (DANGER_DIM, DANGER),
            Tone::DangerSolid => (DANGER_FILL, Color32::WHITE),
            Tone::Warning => (WARNING_DIM, WARNING),
            Tone::Success => (SUCCESS_DIM, SUCCESS),
        }
    }
}

/// Status badge: 18 px tall, 3 px radius, bold capitals, optional leading dot.
pub fn badge(ui: &mut egui::Ui, text: &str, tone: Tone, dot: bool) -> egui::Response {
    let (fill, fg) = tone.colors();
    let font = FontId::new(10.5, FontFamily::Name(FAMILY_BOLD.into()));
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let dot_w = if dot { 12.0 } else { 0.0 };
    let desired = Vec2::new(galley.size().x + 12.0 + dot_w, 18.0);
    let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(size::RADIUS_BADGE), fill);
        let mut x = rect.left() + 6.0;
        if dot {
            p.circle_filled(egui::pos2(x + 3.5, rect.center().y), 3.5, fg);
            x += dot_w;
        }
        p.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    resp
}

/// Progress bar in the redesign's shapes: 8 px job, 12 px upscale total, 6 px segment.
pub fn progress(ui: &mut egui::Ui, fraction: f32, height: f32, fill: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        egui::Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        let r = CornerRadius::same(size::RADIUS_BAR);
        let p = ui.painter();
        p.rect_filled(rect, r, color::WIDGET_IDLE);
        let mut done = rect;
        done.set_width(rect.width() * fraction.clamp(0.0, 1.0));
        p.rect_filled(done, r, fill);
    }
    resp
}

/// A 1 px stroke, the default outline weight everywhere.
fn line(c: Color32) -> Stroke {
    Stroke::new(1.0_f32, c)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    /// The panels draw with the theme's colours and Phosphor icons: no literal `Color32` and none of
    /// the emoji or symbol glyphs the redesign replaced. (`mpv_view.rs` draws its on-video display
    /// itself and is not scanned.)
    #[test]
    fn the_panels_use_theme_colours_and_icons_not_literals() {
        const BANNED_GLYPHS: &str = "⏺⬆⚙📁🔧ℹ⚠✓✗🔍🗑▶⏸◀↺⏱⟳";
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![
            root.join("app.rs"),
            root.join("v4l2.rs"),
            root.join("library.rs"),
        ];
        for e in fs::read_dir(root.join("panels")).unwrap() {
            files.push(e.unwrap().path());
        }
        let mut bad = Vec::new();
        for f in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        {
            let text = fs::read_to_string(f).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                if line.contains("Color32::") || line.chars().any(|c| BANNED_GLYPHS.contains(c)) {
                    bad.push(format!("{}:{}: {}", f.display(), n + 1, code));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "literal colours or glyphs:\n{}",
            bad.join("\n")
        );
    }
}
