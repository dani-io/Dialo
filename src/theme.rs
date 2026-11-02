//! Colours, fonts and the egui style built from them.

use crate::config::ThemeName;
use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Visuals};

#[derive(Clone, Copy)]
pub struct Palette {
    pub panel: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub good: Color32,
    pub bad: Color32,
    pub warn: Color32,
    pub dark: bool,
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl ThemeName {
    pub const ALL: [ThemeName; 4] = [ThemeName::Midnight, ThemeName::Mocha, ThemeName::TokyoNight, ThemeName::Paper];

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Midnight => "Midnight",
            ThemeName::Mocha => "Mocha",
            ThemeName::TokyoNight => "Tokyo Night",
            ThemeName::Paper => "Paper",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            ThemeName::Midnight => Palette {
                panel: c(0x18191d),
                raised: c(0x23252a),
                hover: c(0x2d3036),
                text: c(0xe8e8eb),
                dim: c(0x8a8d97),
                accent: c(0x7aa2ff),
                good: c(0x3ecf8e),
                bad: c(0xff5c6a),
                warn: c(0xffc23d),
                dark: true,
            },
            ThemeName::Mocha => Palette {
                panel: c(0x1e1e2e),
                raised: c(0x313244),
                hover: c(0x45475a),
                text: c(0xcdd6f4),
                dim: c(0x9399b2),
                accent: c(0xcba6f7),
                good: c(0xa6e3a1),
                bad: c(0xf38ba8),
                warn: c(0xf9e2af),
                dark: true,
            },
            ThemeName::TokyoNight => Palette {
                panel: c(0x1a1b26),
                raised: c(0x292e42),
                hover: c(0x343a55),
                text: c(0xc0caf5),
                dim: c(0x737aa2),
                accent: c(0x7aa2f7),
                good: c(0x9ece6a),
                bad: c(0xf7768e),
                warn: c(0xe0af68),
                dark: true,
            },
            ThemeName::Paper => Palette {
                panel: c(0xf9f9f7),
                raised: c(0xe4e4e0),
                hover: c(0xd8d8d3),
                text: c(0x1d1d1f),
                dim: c(0x6e6e73),
                accent: c(0x3366ff),
                good: c(0x1f9d55),
                bad: c(0xe5484d),
                warn: c(0xa86f00),
                dark: false,
            },
        }
    }
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.raised;
    v.faint_bg_color = p.raised;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.window_corner_radius = CornerRadius::same(14);
    v.window_stroke = Stroke::new(1.0, p.hover);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    for (w, fill) in [
        (&mut v.widgets.noninteractive, p.panel),
        (&mut v.widgets.inactive, p.raised),
        (&mut v.widgets.hovered, p.hover),
        (&mut v.widgets.active, p.hover),
        (&mut v.widgets.open, p.raised),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = CornerRadius::same(8);
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, p.text);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.dim);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.5));
    let t = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(t);
    ctx.set_visuals_of(t, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
    });
}

/// egui's own fonts, then the first system font found for scripts they lack (Persian,
/// Arabic), so names in those scripts show as letters rather than boxes.
pub fn fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let candidates = [
        "/usr/share/fonts/TTF/Vazirmatn-Regular.ttf",
        "/usr/share/fonts/vazirmatn/Vazirmatn-Regular.ttf",
        "/usr/share/fonts/truetype/vazirmatn/Vazirmatn-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansArabic-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf",
        "/usr/share/fonts/google-noto/NotoSansArabic-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        defs.font_data.insert("fallback".into(), FontData::from_owned(bytes).into());
        for fam in [FontFamily::Proportional, FontFamily::Monospace] {
            defs.families.entry(fam).or_default().push("fallback".into());
        }
    }
    ctx.set_fonts(defs);
}

/// Two colours for an avatar, picked from the name so a person always looks the same.
pub fn avatar_colors(seed: &str) -> (Color32, Color32) {
    let h = seed.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    let a = egui::ecolor::Hsva::new(hue, 0.62, 0.72, 1.0);
    let b = egui::ecolor::Hsva::new((hue + 0.12) % 1.0, 0.70, 0.48, 1.0);
    (a.into(), b.into())
}

pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| c.is_whitespace() || c == '.' || c == '_').filter(|w| !w.is_empty()).collect();
    let mut s: String = words.iter().take(2).filter_map(|w| w.chars().next()).flat_map(|c| c.to_uppercase()).collect();
    if s.is_empty() {
        s.push('#');
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn initials() {
        assert_eq!(super::initials("ada lovelace"), "AL");
        assert_eq!(super::initials("201"), "2");
        assert_eq!(super::initials(""), "#");
    }
}
