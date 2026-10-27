//! Colour themes in Ghostty's theme format, as cmux uses. themes/themes.txt
//! holds the iTerm2-Color-Schemes themes whose licence is known and fits
//! gr's; themes/licenses.yml lists each one's licence and source.

use std::sync::Arc;

use eframe::egui::{self, Color32, Id, Stroke, Visuals};
use gr::Rgb;

pub const DEFAULT_LIGHT: &str = "GitHub Light Default";
pub const DEFAULT_DARK: &str = "GitHub Dark Default";

/// "theme = <name>" lines, each followed by that theme's lines.
const THEMES: &str = include_str!("../themes/themes.txt");

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub bg: Color32,
    pub fg: Color32,
    /// The 16 ANSI colours.
    pub palette: [Color32; 16],
    pub selection: Color32,
}

impl Theme {
    /// Parses Ghostty's "key = value" theme lines; None without a background and foreground.
    pub fn parse(name: &str, text: &str) -> Option<Theme> {
        let hex = |s: &str| {
            let h = u32::from_str_radix(s.trim().strip_prefix('#').unwrap_or(s.trim()), 16).ok()?;
            Some(Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8))
        };
        let (mut bg, mut fg, mut selection) = (None, None, None);
        let mut palette = [None; 16];
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "background" => bg = hex(v),
                "foreground" => fg = hex(v),
                "selection-background" => selection = hex(v),
                "palette" => {
                    if let Some((i, c)) = v.split_once('=') {
                        if let Some(slot) = i.trim().parse::<usize>().ok().and_then(|i| palette.get_mut(i)) {
                            *slot = hex(c);
                        }
                    }
                }
                _ => {}
            }
        }
        let (bg, fg) = (bg?, fg?);
        Some(Theme {
            name: name.to_string(),
            bg,
            fg,
            palette: palette.map(|c| c.unwrap_or(fg)),
            // terminals draw selected text in another colour, gr keeps the
            // foreground, so skip a selection colour that is nearer to it
            selection: selection
                .filter(|&c| (luma(c) - luma(bg)).abs() < (luma(c) - luma(fg)).abs())
                .unwrap_or(bg.lerp_to_gamma(fg, 0.2)),
        })
    }

    pub fn is_dark(&self) -> bool {
        luma(self.bg) < 0.5
    }

    fn mix(&self, t: f32) -> Color32 {
        self.bg.lerp_to_gamma(self.fg, t)
    }

    /// Secondary text: ANSI bright black, unless it is too close to the background.
    fn weak(&self) -> Color32 {
        let p8 = self.palette[8];
        if (luma(p8) - luma(self.bg)).abs() > 0.2 { p8 } else { self.mix(0.5) }
    }

    /// Stand-ins for the core's gitk colours: graph palette, diff colours
    /// and node fills. ANSI has no orange, so it is red and yellow mixed.
    pub fn map(&self, c: Rgb) -> Option<Color32> {
        let p = &self.palette;
        Some(match c {
            [0, 255, 0] | [0, 0x98, 0] | [0, 0x90, 0] => p[2],
            [255, 0, 0] | [0xc3, 0, 0] => p[1],
            [0, 0, 255] => p[4],
            [255, 0, 255] | [160, 32, 240] => p[5],
            [0xff, 0xa5, 0] => p[1].lerp_to_gamma(p[3], 0.5),
            [255, 255, 0] => p[3],
            [0xa9, 0xa9, 0xa9] => p[8],
            [0xa5, 0x2a, 0x2a] => p[1].lerp_to_gamma(p[8], 0.4),
            [0, 0, 0] => self.fg,
            // diff backgrounds: deleted, added, file header
            [0xff, 0xf3, 0xf3] => self.bg.lerp_to_gamma(p[1], 0.15),
            [0xf0, 0xff, 0xf0] => self.bg.lerp_to_gamma(p[2], 0.15),
            [0xaa, 0xaa, 0xaa] => self.mix(0.15),
            _ => return None,
        })
    }

    fn visuals(&self) -> Visuals {
        let dark = self.is_dark();
        let mut v = if dark { Visuals::dark() } else { Visuals::light() };
        v.override_text_color = Some(self.fg);
        v.weak_text_color = Some(self.weak());
        v.panel_fill = self.bg;
        v.window_fill = self.bg;
        v.faint_bg_color = self.mix(0.04);
        v.extreme_bg_color = self.bg.lerp_to_gamma(if dark { Color32::BLACK } else { Color32::WHITE }, 0.3);
        v.code_bg_color = self.mix(0.08);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.mix(0.15));
        for (w, t) in [(&mut v.widgets.inactive, 0.12), (&mut v.widgets.hovered, 0.2), (&mut v.widgets.active, 0.28), (&mut v.widgets.open, 0.2)] {
            w.weak_bg_fill = self.mix(t);
            w.bg_fill = self.mix(t);
        }
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.mix(0.4));
        v.selection.bg_fill = self.selection;
        v.selection.stroke = Stroke::new(1.0, self.fg);
        v.hyperlink_color = self.palette[4];
        v.error_fg_color = self.palette[1];
        v.warn_fg_color = self.palette[3];
        v
    }

    /// Makes this the theme for its mode (light or dark).
    pub fn apply(&self, ctx: &egui::Context) {
        let dark = self.is_dark();
        ctx.set_visuals_of(if dark { egui::Theme::Dark } else { egui::Theme::Light }, self.visuals());
        ctx.data_mut(|d| d.insert_temp(key(dark), Arc::new(self.clone())));
    }
}

fn luma(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

fn key(dark: bool) -> Id {
    Id::unique(("gr-theme", dark))
}

/// The applied theme for light or dark mode.
pub fn current(ctx: &egui::Context, dark: bool) -> Option<Arc<Theme>> {
    ctx.data(|d| d.get_temp(key(dark)))
}

/// The theme named `name`.
pub fn find(name: &str) -> Option<Theme> {
    all().into_iter().find(|t| t.name == name)
}

/// Every theme, sorted by name.
pub fn all() -> Vec<Theme> {
    THEMES
        .split("theme = ")
        .filter_map(|t| {
            let (name, body) = t.split_once('\n')?;
            Theme::parse(name.trim(), body)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_parse() {
        let dark = find(DEFAULT_DARK).unwrap();
        let light = find(DEFAULT_LIGHT).unwrap();
        assert!(dark.is_dark() && !light.is_dark());
        assert_eq!(dark.map([0, 0, 255]), Some(Color32::from_rgb(0x58, 0xa6, 0xff)));
        assert_eq!(light.bg, Color32::WHITE);
        assert_eq!(all().len(), THEMES.matches("theme = ").count());
    }
}
