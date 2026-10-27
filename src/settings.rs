//! The settings window: a searchable list of settings, laid out like cmux's.

use eframe::egui::{self, Panel, Pos2, Rect, RichText, Sense, Stroke, TextEdit, ThemePreference, Vec2};
use gr::DiffOpts;

use crate::theme::{self, Theme};
use crate::{font, galley};

/// Sidebar entries, in page order.
const SECTIONS: [&str; 2] = ["Appearance", "Diff"];

pub struct Settings {
    pub open: bool,
    query: String,
    /// Section the sidebar asked to scroll to.
    jump: Option<&'static str>,
    /// Every theme, read when the window first opens.
    themes: Option<Vec<Theme>>,
    /// Names of the themes used in light and dark mode.
    pub light: String,
    pub dark: String,
    pub diff: DiffOpts,
    pub split: Split,
}

/// When to show diffs side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// When the diff has room for two columns of `AUTO_COLS` / 2.
    Auto,
    On,
    Off,
}

impl Split {
    pub const ALL: [Split; 3] = [Split::Auto, Split::On, Split::Off];
    /// Characters the diff needs across for Auto to go side by side.
    pub const AUTO_COLS: f32 = 160.0;

    pub fn name(self) -> &'static str {
        match self {
            Split::Auto => "Auto",
            Split::On => "On",
            Split::Off => "Off",
        }
    }

    /// Whether to split a diff `cols` characters wide.
    pub fn on(self, cols: f32) -> bool {
        match self {
            Split::Auto => cols >= Self::AUTO_COLS,
            Split::On => true,
            Split::Off => false,
        }
    }
}

impl Settings {
    /// Restores saved settings through `saved`, applying the themes.
    pub fn new(ctx: &egui::Context, saved: impl Fn(&str) -> Option<String>) -> Settings {
        let pick = |name: Option<String>, default: &str, dark: bool| {
            let t = name.and_then(|n| theme::find(&n)).filter(|t| t.is_dark() == dark);
            let t = t.or_else(|| theme::find(default)).expect("default theme is built in");
            t.apply(ctx);
            t.name
        };
        let mut diff = DiffOpts::default();
        if let Some(n) = saved("diff_context").and_then(|v| v.parse().ok()) {
            diff.context = n;
        }
        let flag = |k: &str, default: bool| saved(k).map_or(default, |v| v == "true");
        diff.ignore_space = flag("ignore_space", diff.ignore_space);
        diff.words = flag("word_diff", diff.words);
        diff.syntax = flag("syntax", diff.syntax);

        Settings {
            open: false,
            query: String::new(),
            jump: None,
            themes: None,
            light: pick(saved("light_theme"), theme::DEFAULT_LIGHT, false),
            dark: pick(saved("dark_theme"), theme::DEFAULT_DARK, true),
            diff,
            split: saved("side_by_side").and_then(|v| Split::ALL.into_iter().find(|s| s.name() == v)).unwrap_or(Split::Auto),
        }
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string("light_theme", self.light.clone());
        storage.set_string("dark_theme", self.dark.clone());
        storage.set_string("diff_context", self.diff.context.to_string());
        storage.set_string("ignore_space", self.diff.ignore_space.to_string());
        storage.set_string("word_diff", self.diff.words.to_string());
        storage.set_string("syntax", self.diff.syntax.to_string());
        storage.set_string("side_by_side", self.split.name().to_string());
    }

    /// Draws the window while open; returns whether the diff options changed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if !self.open {
            return false;
        }
        let before = self.diff.clone();
        let builder = egui::ViewportBuilder::default().with_title("Settings").with_inner_size([960.0, 720.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let cmd_w = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::W));
            if cmd_w || ui.input(|i| i.viewport().close_requested()) {
                self.open = false;
            }
            Panel::left("settings-nav").resizable(false).exact_size(220.0).show(ui, |ui| {
                ui.add_space(8.0);
                ui.add(TextEdit::singleline(&mut self.query).hint_text("Search").desired_width(f32::INFINITY));
                ui.add_space(8.0);
                for s in SECTIONS {
                    if ui.selectable_label(self.jump == Some(s), s).clicked() {
                        self.jump = Some(s);
                    }
                }
            });
            egui::CentralPanel::default_margins().show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.page(ui));
            });
        });
        before != self.diff
    }

    /// Every section, with only the rows matching the search.
    fn page(&mut self, ui: &mut egui::Ui) {
        let q = self.query.trim().to_lowercase();
        let hit = |s: &str| s.to_lowercase().contains(&q);
        let mut shown = false;
        let mut heading = |ui: &mut egui::Ui, name: &'static str, jump: &mut Option<&'static str>| {
            if !shown {
                shown = true;
            } else {
                ui.add_space(24.0);
            }
            let r = ui.heading(name);
            if *jump == Some(name) {
                r.scroll_to_me(Some(egui::Align::TOP));
                *jump = None;
            }
        };

        // Appearance
        const MODE: (&str, &str) = ("Appearance", "Follow the system, or always use the light or dark theme.");
        const SIZE: (&str, &str) = ("Text Size", "Scales all text and controls. Cmd+plus and Cmd+minus change it too.");
        let section = hit("Appearance") || hit("themes");
        let themes = self.themes.get_or_insert_with(theme::all);
        let groups: Vec<(bool, &str, &str, &String, Vec<&Theme>)> = [
            (false, "Light Theme", "Colours used in light mode.", &self.light),
            (true, "Dark Theme", "Colours used in dark mode.", &self.dark),
        ]
        .into_iter()
        .map(|(dark, title, desc, current)| {
            // a match on the group shows all of it, otherwise themes match by name
            let all = section || hit(title) || hit(desc);
            let list = themes.iter().filter(|t| t.is_dark() == dark && (all || hit(&t.name))).collect();
            (dark, title, desc, current, list)
        })
        .collect();
        let rows = [MODE, SIZE].map(|r| section || hit(r.0) || hit(r.1));
        let mut picked = None;
        if rows.contains(&true) || groups.iter().any(|g| !g.4.is_empty()) {
            heading(ui, "Appearance", &mut self.jump);
            ui.add_space(8.0);
            if rows[0] {
                row(ui, MODE, |ui| {
                    let mut pref = ui.ctx().options(|o| o.theme_preference);
                    for (p, label) in [(ThemePreference::Dark, "Dark"), (ThemePreference::Light, "Light"), (ThemePreference::System, "System")] {
                        ui.selectable_value(&mut pref, p, label);
                    }
                    ui.ctx().set_theme(pref);
                    None
                });
            }
            if rows[1] {
                row(ui, SIZE, |ui| {
                    let mut zoom = ui.ctx().zoom_factor();
                    if ui.button("Reset").clicked() {
                        zoom = 1.0;
                    }
                    let r = ui.add(egui::Slider::new(&mut zoom, 0.5..=2.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    if zoom != ui.ctx().zoom_factor() {
                        ui.ctx().set_zoom_factor(zoom);
                    }
                    Some(r)
                });
            }
            for (_, title, desc, current, list) in groups.iter().filter(|g| !g.4.is_empty()) {
                ui.add_space(16.0);
                ui.label(RichText::new(*title).size(15.0).strong());
                ui.label(RichText::new(format!("{desc} Current: {current}")).weak());
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(10.0);
                    for t in list {
                        if card(ui, t, t.name == **current).clicked() {
                            picked = Some((*t).clone());
                        }
                    }
                });
            }
        }
        if let Some(t) = picked {
            t.apply(ui.ctx());
            // show the pick even when the other mode is on
            if t.is_dark() != ui.visuals().dark_mode {
                ui.ctx().set_theme(if t.is_dark() { ThemePreference::Dark } else { ThemePreference::Light });
            }
            let slot = if t.is_dark() { &mut self.dark } else { &mut self.light };
            *slot = t.name;
        }

        // Diff
        const CONTEXT: (&str, &str) = ("Lines of Context", "Unchanged lines shown around each change. Also in the bar under the diff.");
        const SPACE: (&str, &str) = ("Ignore Space Change", "Hide changes that only add or remove whitespace.");
        const WORDS: (&str, &str) = ("Word Diff", "Mark the words that changed between a removed line and the added line after it.");
        const SYNTAX: (&str, &str) = ("Syntax Highlighting", "Colour keywords, strings, comments and numbers, by the file's extension.");
        let section = hit("Diff");
        const SPLIT: (&str, &str) = (
            "Side by Side",
            "Removed lines on the left, added ones on the right. Auto does so when the diff has room for 160 characters. Also in the bar under the diff.",
        );
        let rows = [CONTEXT, SPACE, WORDS, SYNTAX, SPLIT].map(|r| section || hit(r.0) || hit(r.1));
        if rows.contains(&true) {
            heading(ui, "Diff", &mut self.jump);
            ui.add_space(8.0);
            if rows[0] {
                row(ui, CONTEXT, |ui| Some(ui.add(egui::DragValue::new(&mut self.diff.context).range(0..=1000))));
            }
            if rows[1] {
                row(ui, SPACE, |ui| Some(ui.checkbox(&mut self.diff.ignore_space, "")));
            }
            if rows[2] {
                row(ui, WORDS, |ui| Some(ui.checkbox(&mut self.diff.words, "")));
            }
            if rows[3] {
                row(ui, SYNTAX, |ui| Some(ui.checkbox(&mut self.diff.syntax, "")));
            }
            if rows[4] {
                row(ui, SPLIT, |ui| {
                    for s in Split::ALL.into_iter().rev() {
                        ui.selectable_value(&mut self.split, s, s.name());
                    }
                    None
                });
            }
        }

        if !shown {
            ui.label(RichText::new(format!("No settings match \"{}\".", self.query.trim())).weak());
        }
    }
}

/// A titled settings row with its control on the right; the text wraps in
/// the width the control leaves. A returned control is named by the title.
fn row(ui: &mut egui::Ui, (title, desc): (&str, &str), control: impl FnOnce(&mut egui::Ui) -> Option<egui::Response>) {
    egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // top-aligned: a centred layout grows to the scroll area's height
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let r = control(ui);
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                let label = ui.label(RichText::new(title).size(15.0).strong());
                ui.label(RichText::new(desc).weak());
                if let Some(r) = r {
                    r.labelled_by(label.id);
                }
            });
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
