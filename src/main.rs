use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, ComboBox, FontFamily, FontId, Galley, Key, Layout, Painter, Panel,
    Id, Pos2, Rect, Sense, Stroke, TextEdit, Vec2,
};
use egui_extras::{Column, TableBuilder};
use gr::{Details, DiffOpts, Field, Hit, History, Mode, Query, RefKind, Repo, Rgb, Seg, Visit, REMOTE_PREFIX_BG};

const ROW_H: f32 = 18.0;
const ID_EDIT: &str = "commit_id";
const FIND_EDIT: &str = "find";

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// A core colour adapted to the theme: in dark mode HSL lightness is
/// inverted, so pale backgrounds turn dark and black text turns white.
fn bg(ui: &egui::Ui, c: Rgb) -> Color32 {
    if !ui.visuals().dark_mode {
        return rgb(c);
    }
    let (lo, hi) = (*c.iter().min().unwrap() as i16, *c.iter().max().unwrap() as i16);
    let shift = 255 - hi - lo;
    rgb(c.map(|v| (v as i16 + shift) as u8))
}

/// Like `bg`, but lifted towards white in dark mode so saturated colours
/// such as pure blue stay readable on a dark background.
fn fg(ui: &egui::Ui, c: Rgb) -> Color32 {
    let col = bg(ui, c);
    if ui.visuals().dark_mode { col.lerp_to_gamma(Color32::WHITE, 0.35) } else { col }
}

fn galley(p: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    p.layout_no_wrap(text.to_string(), font, color)
}

fn font(size: f32, bold: bool) -> FontId {
    FontId::new(size, if bold { FontFamily::Name("bold".into()) } else { FontFamily::Proportional })
}

/// Paints a galley vertically centred on `y`.
fn paint_text(p: &Painter, x: f32, y: f32, g: Arc<Galley>, color: Color32) -> f32 {
    let w = g.size().x;
    p.galley(Pos2::new(x, y - g.size().y / 2.0), g, color);
    w
}

struct App {
    repo: Repo,
    sel: usize,
    details: Details,
    opts: DiffOpts,
    history: History,
    id_text: String,
    status: String,
    find_text: String,
    mode: Mode,
    field: Field,
    query: Option<Query>,
    scroll_list: bool,
    diff_scroll: Option<Vec2>,
    // d/u scroll animation: from, to, start time
    diff_anim: Option<(f32, f32, f64)>,
    diff_off: Vec2,
    diff_h: f32,
    list_h: f32,
    list_off: f32,
    list_scroll: Option<f32>,
    max_cols: usize,
}

impl App {
    fn new(repo: Repo) -> App {
        let sel = repo.head_row.unwrap_or(0);
        let mut app = App {
            repo,
            sel,
            // filled by select below
            details: Default::default(),
            opts: DiffOpts::default(),
            history: History::default(),
            id_text: String::new(),
            status: String::new(),
            find_text: String::new(),
            mode: Mode::Exact,
            field: Field::All,
            query: None,
            scroll_list: true,
            diff_scroll: None,
            diff_anim: None,
            diff_off: Vec2::ZERO,
            diff_h: 0.0,
            list_h: 0.0,
            list_off: 0.0,
            list_scroll: None,
            max_cols: 0,
        };
        app.select(sel, true);
        app
    }

    fn select(&mut self, row: usize, push: bool) {
        if self.repo.commits.is_empty() {
            return;
        }
        let row = row.min(self.repo.commits.len() - 1);
        self.sel = row;
        self.details = self.repo.details(row, &self.opts);
        self.max_cols = self.details.lines.iter().map(|l| l.text.chars().count()).max().unwrap_or(0);
        self.id_text = self.repo.commits[row].id().to_string();
        self.status.clear();
        self.scroll_list = true;
        self.diff_scroll = Some(Vec2::ZERO);
        if push {
            self.history.push(row);
        }
    }

    fn find(&mut self, forward: bool, wrap: bool) {
        if let Some(q) = &self.query {
            if let Some(r) = self.repo.find(q, Some(self.sel), forward, wrap) {
                self.select(r, true);
            }
        }
    }

    fn visit(&mut self, v: Option<Visit>) {
        if let Some(v) = v {
            self.select(v.row, false);
            self.scroll_diff(v.line * ROW_H);
        }
    }

    fn back(&mut self) {
        let v = self.history.back();
        self.visit(v);
    }

    fn forward(&mut self) {
        let v = self.history.forward();
        self.visit(v);
    }

    fn reload(&mut self) {
        match self.repo.reload(self.sel) {
            Ok((repo, row)) => {
                self.repo = repo;
                self.history = History::default();
                self.select(row, true);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn scroll_diff(&mut self, y: f32) {
        self.diff_scroll = Some(Vec2::new(self.diff_off.x, y));
    }

    /// Starts (or extends) an eased scroll of the diff by `dy`, stopping at either end.
    fn animate_diff(&mut self, ctx: &egui::Context, dy: f32) {
        let max = (self.details.lines.len() as f32 * ROW_H - self.diff_h).max(0.0);
        let to = self.diff_anim.map_or(self.diff_off.y, |a| a.1) + dy;
        self.diff_anim = Some((self.diff_off.y, to.clamp(0.0, max), ctx.input(|i| i.time)));
    }

    fn top_line(&self) -> usize {
        (self.diff_off.y / ROW_H).floor() as usize
    }

    fn update_query(&mut self) {
        self.query = Query::new(&self.find_text, self.mode, self.field).ok().flatten();
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let cmd = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k));
        if cmd(Key::Q) || cmd(Key::W) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if cmd(Key::F) {
            self.find(true, true);
        }
        if cmd(Key::G) {
            self.find(true, false);
        }
        let list_page = (self.list_h - ROW_H).max(ROW_H);
        if cmd(Key::Home) {
            self.list_scroll = Some(0.0);
        } else if cmd(Key::End) {
            self.list_scroll = Some(self.repo.commits.len() as f32 * ROW_H);
        } else if cmd(Key::ArrowUp) {
            self.list_scroll = Some((self.list_off - ROW_H).max(0.0));
        } else if cmd(Key::ArrowDown) {
            self.list_scroll = Some(self.list_off + ROW_H);
        } else if cmd(Key::PageUp) {
            self.list_scroll = Some((self.list_off - list_page).max(0.0));
        } else if cmd(Key::PageDown) {
            self.list_scroll = Some(self.list_off + list_page);
        }
        let nums = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (n, k) in nums.into_iter().enumerate() {
            if cmd(k) {
                let parent = self.repo.commits.get(self.sel).and_then(|c| c.parents().nth(n));
                if let Some(r) = parent.and_then(|p| self.repo.index_of(p)) {
                    self.select(r, true);
                }
            }
        }
        if ctx.input(|i| i.key_pressed(Key::F5) && !(i.modifiers.ctrl || i.modifiers.alt || i.modifiers.command)) {
            self.reload();
        }
        let shift = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.shift_only());
        if shift(Key::ArrowUp) {
            self.find(false, false);
        } else if shift(Key::ArrowDown) {
            self.find(true, false);
        }
        let page = ((self.list_h / ROW_H) as usize).saturating_sub(1).max(1);
        let last = self.repo.commits.len().saturating_sub(1);
        let pressed = |k| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        if pressed(Key::ArrowUp) {
            self.select(self.sel.saturating_sub(1), true);
        } else if pressed(Key::ArrowDown) {
            self.select((self.sel + 1).min(last), true);
        } else if pressed(Key::PageUp) {
            self.select(self.sel.saturating_sub(page), true);
        } else if pressed(Key::PageDown) {
            self.select((self.sel + page).min(last), true);
        }
        if ctx.text_edit_focused() {
            return;
        }
        // a focused button turns Enter/Space into its own click
        let button = ctx.memory(|m| m.focused()).is_some();
        let diff_page = (self.diff_h - ROW_H).max(ROW_H);
        let typed = |s: &str| ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == s)));
        // keep the focusing keystroke out of the entry
        let drop_text = || ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        if pressed(Key::P) || pressed(Key::K) {
            self.select(self.sel.saturating_sub(1), true);
        } else if pressed(Key::N) || pressed(Key::J) {
            self.select((self.sel + 1).min(last), true);
        } else if pressed(Key::Home) {
            self.select(0, true);
        } else if pressed(Key::End) {
            self.select(last, true);
        } else if pressed(Key::ArrowLeft) || pressed(Key::Z) || pressed(Key::H) {
            self.back();
        } else if pressed(Key::ArrowRight) || pressed(Key::X) || pressed(Key::L) {
            self.forward();
        } else if pressed(Key::Enter) && !button {
            self.find(true, true);
        } else if typed("?") {
            self.find(false, true);
        } else if typed("/") {
            drop_text();
            ctx.memory_mut(|m| m.request_focus(Id::unique(FIND_EDIT)));
        } else if pressed(Key::G) {
            drop_text();
            self.id_text.clear();
            ctx.memory_mut(|m| m.request_focus(Id::unique(ID_EDIT)));
        } else if pressed(Key::Space) && !button {
            self.scroll_diff(self.diff_off.y + diff_page);
        } else if pressed(Key::Backspace) || pressed(Key::Delete) {
            self.scroll_diff((self.diff_off.y - diff_page).max(0.0));
        } else if pressed(Key::D) {
            self.animate_diff(ctx, 18.0 * ROW_H);
        } else if pressed(Key::U) {
            self.animate_diff(ctx, -18.0 * ROW_H);
        } else if pressed(Key::F) {
            let next = self.details.file_at(self.top_line()) + 1;
            if let Some(&(_, line)) = self.details.files.get(next) {
                self.scroll_diff(line as f32 * ROW_H);
            }
        } else if pressed(Key::B) {
            let top = self.top_line();
            let prev = self.details.files.iter().rev().map(|f| f.1).find(|&l| l < top);
            self.scroll_diff(prev.unwrap_or(0) as f32 * ROW_H);
        }
    }

    fn id_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Commit ID:");
            let r = ui.add(TextEdit::singleline(&mut self.id_text).id(Id::unique(ID_EDIT)).font(egui::TextStyle::Monospace).desired_width(330.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                match self.repo.resolve(&self.id_text) {
                    Ok(row) => self.select(row, true),
                    Err(e) => self.status = e,
                }
            }
            if ui.add_enabled(self.history.can_back(), egui::Button::new("←")).clicked() {
                self.back();
            }
            if ui.add_enabled(self.history.can_forward(), egui::Button::new("→")).clicked() {
                self.forward();
            }
            ui.label(format!("Row {} / {}", self.sel + 1, self.repo.commits.len()));
            ui.colored_label(Color32::RED, &self.status);
        });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let find = ui.button("Find").clicked();
            if ui.button("↓").clicked() || find {
                self.find(true, true);
            }
            if ui.button("↑").clicked() {
                self.find(false, true);
            }
            ui.label("commit containing:");
            let r = ui.add(TextEdit::singleline(&mut self.find_text).id(Id::unique(FIND_EDIT)).desired_width(300.0));
            if r.changed() {
                self.update_query();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.find(true, true);
                r.request_focus();
            }
            let (mode, field) = (self.mode, self.field);
            ComboBox::from_id_salt("mode").selected_text(self.mode.label()).show_ui(ui, |ui| {
                for m in Mode::ALL {
                    ui.selectable_value(&mut self.mode, m, m.label());
                }
            });
            ComboBox::from_id_salt("field").selected_text(self.field.label()).width(110.0).show_ui(ui, |ui| {
                for f in Field::ALL {
                    ui.selectable_value(&mut self.field, f, f.label());
                }
            });
            if (mode, field) != (self.mode, self.field) {
                self.update_query();
            }
        });
    }

    fn commit_list(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        let scroll_to = std::mem::take(&mut self.scroll_list).then_some(self.sel);
        let width = ui.available_width();
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        let mut table = TableBuilder::new(ui)
            .id_salt("commits")
            .resizable(true)
            .sense(Sense::click())
            .animate_scrolling(false)
            .auto_shrink(false)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(width * 0.55).at_least(100.0).clip(true))
            .column(Column::initial(width * 0.25).at_least(50.0).clip(true))
            .column(Column::remainder().at_least(50.0).clip(true));
        if let Some(r) = scroll_to {
            // recentre when far off, otherwise move just enough
            let rows = (self.list_h / ROW_H) as usize;
            let list_top = (self.list_off / ROW_H) as usize;
            let (top, bottom) = (list_top.saturating_sub(rows), list_top + 2 * rows);
            let align = if r < top || r > bottom { Some(Align::Center) } else { None };
            table = table.scroll_to_row(r, align);
        } else if let Some(off) = self.list_scroll.take() {
            table = table.vertical_scroll_offset(off);
        }
        let (repo, sel, query) = (&self.repo, self.sel, &self.query);
        let out = table.body(|body| {
            body.rows(ROW_H, repo.commits.len(), |mut row| {
                let i = row.index();
                let c = &repo.commits[i];
                let hit = query.as_ref().map_or(Hit::No, |q| q.hit(c));
                row.set_selected(i == sel);
                row.col(|ui| draw_graph_cell(ui, c, &repo.segs(i), hit != Hit::No));
                for (text, bold) in [(c.author_name(), hit == Hit::Author), (&*c.author_date(), false)] {
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        let p = ui.painter();
                        let color = ui.visuals().text_color();
                        let g = galley(p, text, font(13.0, bold), color);
                        paint_text(p, rect.left() + 2.0, rect.center().y, g, color);
                    });
                }
                if row.response().clicked() {
                    clicked = Some(i);
                }
            });
        });
        self.list_h = out.inner_rect.height();
        self.list_off = out.state.clamped_offset().y;
        if let Some(i) = clicked {
            self.select(i, true);
        }
    }

    fn diff_view(&mut self, ui: &mut egui::Ui) {
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font(12.0, false), 'x'));
        let mut sa = egui::ScrollArea::both().id_salt("diff").auto_shrink(false);
        if ui.input(|i| i.smooth_scroll_delta.y != 0.0) || self.diff_scroll.is_some() {
            self.diff_anim = None;
        }
        if let Some((from, to, start)) = self.diff_anim {
            let t = ((ui.input(|i| i.time) - start) / 0.2).min(1.0) as f32;
            // ease-out cubic
            self.diff_scroll = Some(Vec2::new(self.diff_off.x, from + (to - from) * (1.0 - (1.0 - t).powi(3))));
            if t < 1.0 {
                ui.ctx().request_repaint();
            } else {
                self.diff_anim = None;
            }
        }
        if let Some(off) = self.diff_scroll.take() {
            sa = sa.scroll_offset(off);
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut link = None;
        let lines = &self.details.lines;
        let content_w = self.max_cols as f32 * char_w + 8.0;
        let out = sa.show_rows(ui, ROW_H, lines.len(), |ui, range| {
            let w = content_w.max(ui.available_width());
            for line in &lines[range] {
                let sense = if line.link.is_some() { Sense::click() } else { Sense::hover() };
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), sense);
                let p = ui.painter();
                if let Some(bg_c) = line.style.bg {
                    p.rect_filled(rect, 0.0, bg(ui, bg_c));
                }
                let fg = fg(ui, line.style.fg);
                let g = galley(p, &line.text, font(12.0, line.style.bold), fg);
                let tw = paint_text(p, rect.left() + 4.0, rect.center().y, g, fg);
                if let Some(row) = line.link {
                    let y = rect.center().y + 7.0;
                    p.hline(rect.left() + 4.0..=rect.left() + 4.0 + tw, y, Stroke::new(1.0, fg));
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        link = Some(row);
                    }
                }
            }
        });
        self.diff_off = out.state.clamped_offset();
        self.history.set_line(self.diff_off.y / ROW_H);
        self.diff_h = out.inner_rect.height();
        if let Some(row) = link {
            self.select(row, true);
        }
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        let cur = self.details.file_at(self.top_line());
        let mut jump = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        let files = &self.details.files;
        egui::ScrollArea::both().id_salt("files").auto_shrink(false).show_rows(ui, ROW_H, files.len(), |ui, range| {
            for (i, (name, line)) in files.iter().enumerate().take(range.end).skip(range.start) {
                let color = ui.visuals().text_color();
                let g = galley(ui.painter(), name, font(13.0, false), color);
                let w = ui.available_width().max(g.size().x + 8.0);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
                if i == cur {
                    ui.painter().rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
                }
                paint_text(ui.painter(), rect.left() + 4.0, rect.center().y, g, color);
                if resp.clicked() {
                    jump = Some(*line);
                }
            }
        });
        if let Some(line) = jump {
            self.scroll_diff(line as f32 * ROW_H);
        }
    }

    fn diff_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let opts = self.opts.clone();
            ui.label("Lines of context:");
            ui.add(egui::DragValue::new(&mut self.opts.context).range(0..=1000));
            ui.checkbox(&mut self.opts.ignore_space, "Ignore space change");
            if opts.context != self.opts.context || opts.ignore_space != self.opts.ignore_space {
                self.select(self.sel, false);
            }
        });
    }
}

fn draw_graph_cell(ui: &mut egui::Ui, c: &gr::Commit, segs: &[Seg], bold: bool) {
    let rect = ui.max_rect();
    let p = ui.painter();
    let text = ui.visuals().text_color();
    let x0 = rect.left() + 3.0 + ROW_H / 2.0;
    let pt = |x: f32, y: f32| Pos2::new(x0 + x * ROW_H, rect.top() + y * ROW_H);
    let thick = (ROW_H / 9.0).floor() + 1.0;
    for s in segs {
        p.line_segment([pt(s.x0, s.y0), pt(s.x1, s.y1)], Stroke::new(thick, fg(ui, s.color)));
    }
    let node = pt(c.lane as f32, 0.5);
    let mut x = x0 + c.width.max(1) as f32 * ROW_H - ROW_H / 2.0 + 4.0;
    let y = rect.center().y;
    if !c.refs.is_empty() {
        p.hline(node.x..=x, y, Stroke::new(1.0, text));
        for r in &c.refs {
            let (bg, fg) = r.colors();
            let g = galley(p, &r.name, font(11.0, r.current), rgb(fg));
            let w = g.size().x + 6.0;
            let b = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(w, ROW_H - 4.0));
            p.rect_filled(b, 0.0, rgb(bg));
            let n = r.remote_prefix_len();
            if r.kind == RefKind::Remote && n > 0 {
                let pw = galley(p, &r.name[..n], font(11.0, r.current), Color32::BLACK).size().x + 3.0;
                p.rect_filled(Rect::from_min_size(b.min, Vec2::new(pw, b.height())), 0.0, rgb(REMOTE_PREFIX_BG));
            }
            p.rect_stroke(b, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
            paint_text(p, x + 3.0, y, g, rgb(fg));
            x += w + 4.0;
        }
    }
    p.circle(node, ROW_H / 3.0, fg(ui, c.node_fill), Stroke::new(1.0, Color32::BLACK));
    let g = galley(p, c.subject(), font(13.0, bold), text);
    paint_text(p, x, y, g, text);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.keys(ui.ctx());
        let h = ui.available_height();
        Panel::top("list").resizable(true).default_size(h * 0.5).min_size(100.0).show(ui, |ui| {
            Panel::bottom("bars").resizable(false).show(ui, |ui| {
                self.id_bar(ui);
                self.find_bar(ui);
            });
            egui::CentralPanel::no_frame().show(ui, |ui| self.commit_list(ui));
        });
        Panel::right("files").resizable(true).default_size(250.0).show(ui, |ui| self.file_list(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.diff_bar(ui);
            self.diff_view(ui);
        });
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let log_args: Vec<String> = args.collect();
    let repo = match Repo::open(&dir, &log_args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gr: {e}");
            std::process::exit(1);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(repo.title()).with_inner_size([1100.0, 800.0]),
        ..Default::default()
    };
    let res = eframe::run_native(
        "gr",
        options,
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            for (name, data) in [("lyth", &include_bytes!("../fonts/LythMono-Regular.ttf")[..]), ("bold", include_bytes!("../fonts/LythMono-Bold.ttf"))] {
                fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(data)));
            }
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().insert(0, "lyth".into());
            }
            let bold = std::iter::once("bold".into()).chain(fonts.families[&FontFamily::Proportional].clone()).collect();
            fonts.families.insert(FontFamily::Name("bold".into()), bold);
            cc.egui_ctx.set_fonts(fonts);
            // egui follows the system light/dark setting by default
            for (theme, mut v, text, panel, stroke, sel) in [
                (egui::Theme::Light, egui::Visuals::light(), 0x00, 0xff, 160, 0xd9),
                (egui::Theme::Dark, egui::Visuals::dark(), 0xe6, 0x1e, 90, 0x45),
            ] {
                v.override_text_color = Some(Color32::from_gray(text));
                v.panel_fill = Color32::from_gray(panel);
                v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(stroke));
                v.selection.bg_fill = Color32::from_gray(sel);
                v.selection.stroke = Stroke::new(1.0, Color32::from_gray(text));
                cc.egui_ctx.set_visuals_of(theme, v);
            }
            Ok(Box::new(App::new(repo)))
        }),
    );
    if let Err(e) = res {
        eprintln!("gr: {e}");
        std::process::exit(1);
    }
}
