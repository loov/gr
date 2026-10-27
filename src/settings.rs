//! The settings window: a searchable list of settings, laid out like cmux's.

use eframe::egui::{self, Panel, Pos2, Rect, RichText, Sense, Stroke, TextEdit, ThemePreference, Vec2};

use crate::theme::{self, Theme};
use crate::{font, galley};

pub struct Settings {
    pub open: bool,
    query: String,
    /// Every theme, read when the window first opens.
    themes: Option<Vec<Theme>>,
    /// Names of the themes used in light and dark mode.
    pub light: String,
    pub dark: String,
}

impl Settings {
    /// Applies the saved themes, falling back to the defaults.
    pub fn new(ctx: &egui::Context, light: Option<String>, dark: Option<String>) -> Settings {
        let pick = |name: Option<String>, default: &str, dark: bool| {
            let t = name.and_then(|n| theme::find(&n)).filter(|t| t.is_dark() == dark);
            let t = t.or_else(|| theme::find(default)).unwrap();
            t.apply(ctx);
            t.name
        };
        Settings {
            open: false,
            query: String::new(),
            themes: None,
            light: pick(light, theme::DEFAULT_LIGHT, false),
            dark: pick(dark, theme::DEFAULT_DARK, true),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let builder = egui::ViewportBuilder::default().with_title("Settings").with_inner_size([960.0, 720.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let cmd_w = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::W));
            if cmd_w || ui.input(|i| i.viewport().close_requested()) {
                self.open = false;
            }
            Panel::left("settings-nav").resizable(false).exact_size(220.0).show(ui, |ui| {
                ui.add_space(8.0);
                ui.add(TextEdit::singleline(&mut self.query).hint_text("🔍 Search").desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.label(RichText::new("General").small().weak());
                let _ = ui.selectable_label(true, "Themes");
            });
            egui::CentralPanel::default_margins().show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.themes_page(ui));
            });
        });
    }

    fn themes_page(&mut self, ui: &mut egui::Ui) {
        let q = self.query.trim().to_lowercase();
        let hit = |s: &str| s.to_lowercase().contains(&q);
        ui.heading("Themes");
        ui.add_space(8.0);
        let mut shown = false;

        const APPEARANCE: (&str, &str) = ("Appearance", "Follow the system, or always use the light or dark theme.");
        if hit(APPEARANCE.0) || hit(APPEARANCE.1) || hit("themes") {
            shown = true;
            row(ui, APPEARANCE, |ui| {
                let mut pref = ui.ctx().options(|o| o.theme_preference);
                for (p, label) in [(ThemePreference::Dark, "Dark"), (ThemePreference::Light, "Light"), (ThemePreference::System, "System")] {
                    ui.selectable_value(&mut pref, p, label);
                }
                ui.ctx().set_theme(pref);
            });
        }

        let themes = self.themes.get_or_insert_with(theme::all);
        let mut picked = None;
        for (dark, title, desc, current) in [
            (false, "Light Theme", "Colours used in light mode.", &self.light),
            (true, "Dark Theme", "Colours used in dark mode.", &self.dark),
        ] {
            // a match on the section shows all of it, otherwise themes match by name
            let all = hit(title) || hit(desc) || hit("themes");
            let list: Vec<&Theme> = themes.iter().filter(|t| t.is_dark() == dark && (all || hit(&t.name))).collect();
            if list.is_empty() {
                continue;
            }
            shown = true;
            ui.add_space(16.0);
            ui.label(RichText::new(title).size(15.0).strong());
            ui.label(RichText::new(format!("{desc} Current: {current}")).weak());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(10.0);
                for t in list {
                    if card(ui, t, t.name == *current).clicked() {
                        picked = Some(t.clone());
                    }
                }
            });
        }
        if let Some(t) = picked {
            t.apply(ui.ctx());
            let slot = if t.is_dark() { &mut self.dark } else { &mut self.light };
            *slot = t.name;
        }
        if !shown {
            ui.label(RichText::new(format!("No settings match \"{}\".", self.query.trim())).weak());
        }
    }
}

/// A titled settings row with its control on the right.
fn row(ui: &mut egui::Ui, (title, desc): (&str, &str), control: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(15.0).strong());
                ui.label(RichText::new(desc).weak());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
        });
    });
}

/// A theme preview: its name in its colours above its 16 ANSI colours.
fn card(ui: &mut egui::Ui, t: &Theme, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(186.0, 62.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, t.bg);
    let border = if selected {
        Stroke::new(2.0, ui.visuals().hyperlink_color)
    } else if resp.hovered() {
        Stroke::new(1.0, ui.visuals().text_color())
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    p.rect_stroke(rect, 6.0, border, egui::StrokeKind::Inside);
    let inner = rect.shrink2(Vec2::new(10.0, 8.0));
    let g = galley(p, &t.name, font(13.0, false), t.fg);
    p.with_clip_rect(inner).galley(inner.min, g, t.fg);
    let (w, h, gap) = (17.0, 10.0, 3.0);
    for (i, c) in t.palette.iter().enumerate() {
        let (col, line) = ((i % 8) as f32, (i / 8) as f32);
        let min = Pos2::new(inner.left() + col * (w + gap), inner.top() + 20.0 + line * (h + gap));
        p.rect_filled(Rect::from_min_size(min, Vec2::new(w, h)), 2.0, *c);
    }
    if !selected && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(&t.name)
}
