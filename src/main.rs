use std::sync::Arc;

use eframe::egui::{
    self, Align, CollapsingHeader, Color32, RichText, ComboBox, FontFamily, FontId, Galley, Key, Layout, Painter, Panel,
    Id, Pos2, Rect, Sense, Stroke, TextEdit, Vec2,
};
mod menu;
mod settings;
mod theme;

use egui_extras::{Column, TableBuilder};
use gr::{Annotations, Details, Location, Locations, Field, Hit, History, Mode, Query, RefKind, Repo, Rgb, Seg, Visit, REMOTE_PREFIX};

const ROW_H: f32 = 18.0;
/// Commit list rows hold two lines: subject, then committer and date.
const LIST_H: f32 = 2.0 * ROW_H;
/// Horizontal distance between graph lanes.
const LANE_W: f32 = 9.0;
const ID_EDIT: &str = "commit_id";
const FIND_EDIT: &str = "find";

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// A core colour in the current theme. Colours the theme has no stand-in
/// for are used as is in light mode and get their HSL lightness inverted in
/// dark mode.
fn bg(ui: &egui::Ui, c: Rgb) -> Color32 {
    let dark = ui.visuals().dark_mode;
    if let Some(m) = theme::current(ui.ctx(), dark).and_then(|t| t.map(c)) {
        return m;
    }
    if !dark {
        return rgb(c);
    }
    let (lo, hi) = (*c.iter().min().unwrap() as i16, *c.iter().max().unwrap() as i16);
    let shift = 255 - hi - lo;
    rgb(c.map(|v| (v as i16 + shift) as u8))
}

/// Like `bg`, but inverted colours are lifted towards white so saturated
/// ones such as pure blue stay readable on a dark background.
fn fg(ui: &egui::Ui, c: Rgb) -> Color32 {
    let dark = ui.visuals().dark_mode;
    let themed = theme::current(ui.ctx(), dark).is_some_and(|t| t.map(c).is_some());
    let col = bg(ui, c);
    if dark && !themed { col.lerp_to_gamma(Color32::WHITE, 0.35) } else { col }
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

/// What `App::keys` does, for the Help menu. Cmd is Ctrl outside macOS.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    ("Commits", &[
        ("↑ ↓  p n", "previous, next commit"),
        ("PageUp PageDown", "a page of commits up, down"),
        ("Home End", "first, last commit"),
        ("← z h", "back in the visit history"),
        ("→ x l", "forward in the visit history"),
        ("Cmd+1 … Cmd+9", "go to the first … ninth parent"),
        ("g", "type a commit id, branch or tag to go to"),
        ("Cmd+↑ Cmd+↓", "scroll the list by a commit"),
        ("Cmd+PageUp Cmd+PageDown", "scroll the list by a page"),
        ("Cmd+Home Cmd+End", "scroll the list to the top, bottom"),
    ]),
    ("Review", &[
        ("k", "mark the commit ok, or unmark it"),
        ("j", "next commit to review on this branch"),
    ]),
    ("Find", &[
        ("/", "type what to find"),
        ("Enter  Cmd+F", "next match"),
        ("?", "previous match"),
        ("Cmd+G", "next match, without wrapping around"),
        ("Shift+↓ Shift+↑", "next, previous match without wrapping around"),
    ]),
    ("Diff", &[
        ("Space Backspace", "a page down, up"),
        ("d u", "18 lines down, up"),
        ("f b", "next, previous file"),
    ]),
    ("Window", &[
        ("r  F5", "reload"),
        ("Cmd+,", "settings"),
        ("Cmd+plus Cmd+minus", "text size"),
        ("Cmd+W  Cmd+Q", "close"),
    ]),
];

struct App {
    repo: Repo,
    locs: Locations,
    notes: Annotations,
    sel: usize,
    details: Details,
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
    /// Left edge of the diff column last frame; the bottom bar lines up with it.
    diff_left: f32,
    list_h: f32,
    list_off: f32,
    list_scroll: Option<f32>,
    max_cols: usize,
    half_cols: usize,
    /// A git command from the Git menu, running in the background.
    job: Option<std::thread::JoinHandle<Result<(), String>>>,
    /// The OS menu bar; None where the menu is drawn with egui.
    native: Option<menu::Native>,
    settings: settings::Settings,
    watch: gr::Watch,
    /// The keyboard shortcuts window is open.
    shortcuts: bool,
    /// A reload running in the background.
    loading: Option<std::thread::JoinHandle<std::io::Result<Repo>>>,
    ctx: egui::Context,
}

impl App {
    fn new(repo: Repo, native: Option<menu::Native>, settings: settings::Settings, ctx: &egui::Context) -> App {
        let sel = repo.head_row.unwrap_or(0);
        let watch = {
            let ctx = ctx.clone();
            repo.watch(move || ctx.request_repaint())
        };
        let (notes, status) = match repo.annotations() {
            Ok(n) => (n, String::new()),
            Err(e) => (Annotations::default(), format!("gr.info: {e}")),
        };
        let mut app = App {
            locs: repo.locations(),
            notes,
            repo,
            sel,
            // filled by select below
            details: Default::default(),
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
            diff_left: 0.0,
            list_h: 0.0,
            list_off: 0.0,
            list_scroll: None,
            max_cols: 0,
            half_cols: 0,
            job: None,
            native,
            settings,
            watch,
            shortcuts: false,
            loading: None,
            ctx: ctx.clone(),
        };
        app.select(sel, true);
        // select clears the status, so report a bad gr.info after it
        app.status = status;
        app.carry_marks();
        app
    }

    fn select(&mut self, row: usize, push: bool) {
        if self.repo.commits.is_empty() {
            return;
        }
        let row = row.min(self.repo.commits.len() - 1);
        self.sel = row;
        self.details = self.repo.details(row, &self.settings.diff);
        // full-width lines, and the wider half of side-by-side ones
        let lines = &self.details.lines;
        let len = |l: &gr::Line| l.text.chars().count();
        self.max_cols = lines.iter().filter(|l| l.right.is_none()).map(len).max().unwrap_or(0);
        self.half_cols = lines.iter().filter_map(|l| Some(len(l).max(len(l.right.as_ref()?)))).max().unwrap_or(0);
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

    /// Starts reopening the repository; `poll_reload` takes the result.
    fn reload(&mut self) {
        if self.loading.is_some() {
            return;
        }
        let job = self.repo.reopen();
        let ctx = self.ctx.clone();
        self.loading = Some(std::thread::spawn(move || {
            let r = job();
            ctx.request_repaint();
            r
        }));
    }

    fn poll_reload(&mut self) {
        if !self.loading.as_ref().is_some_and(|j| j.is_finished()) {
            return;
        }
        let res = self.loading.take().unwrap().join().unwrap_or_else(|_| Err(std::io::Error::other("load thread panicked")));
        match res.map(|repo| { let row = self.repo.place(&repo, self.sel); (repo, row) }) {
            Ok((repo, row)) => {
                self.repo = repo;
                self.locs = self.repo.locations();
                self.history = History::default();
                self.select(row, true);
                match self.repo.annotations() {
                    Ok(n) => {
                        self.notes = n;
                        self.carry_marks();
                    }
                    Err(e) => self.status = format!("gr.info: {e}"),
                }
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Starts `git args` in the background; `poll_job` reloads when it ends.
    fn run_git(&mut self, ctx: &egui::Context, args: &'static [&'static str]) {
        if let Some(n) = &self.native {
            n.set_enabled(false);
        }
        let (dir, ctx) = (self.repo.dir.clone(), ctx.clone());
        self.job = Some(std::thread::spawn(move || {
            let out = std::process::Command::new("git")
                .current_dir(dir)
                .args(args)
                // fail instead of waiting for a password nobody can type
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(std::process::Stdio::null())
                .output();
            ctx.request_repaint();
            match out {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                Err(e) => Err(format!("git: {e}")),
            }
        }));
    }

    fn poll_job(&mut self) {
        if !self.job.as_ref().is_some_and(|j| j.is_finished()) {
            return;
        }
        let res = self.job.take().unwrap().join().unwrap_or_else(|_| Err("git thread panicked".into()));
        if let Some(n) = &self.native {
            n.set_enabled(true);
        }
        self.reload();
        if let Err(e) = res {
            self.status = e;
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
        if cmd(Key::Comma) {
            self.settings.open = true;
        }
        if cmd(Key::F) {
            self.find(true, true);
        }
        if cmd(Key::G) {
            self.find(true, false);
        }
        let list_page = (self.list_h - LIST_H).max(LIST_H);
        if cmd(Key::Home) {
            self.list_scroll = Some(0.0);
        } else if cmd(Key::End) {
            self.list_scroll = Some(self.repo.commits.len() as f32 * LIST_H);
        } else if cmd(Key::ArrowUp) {
            self.list_scroll = Some((self.list_off - LIST_H).max(0.0));
        } else if cmd(Key::ArrowDown) {
            self.list_scroll = Some(self.list_off + LIST_H);
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
        let page = ((self.list_h / LIST_H) as usize).saturating_sub(1).max(1);
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
        if pressed(Key::P) {
            self.select(self.sel.saturating_sub(1), true);
        } else if pressed(Key::N) {
            self.select((self.sel + 1).min(last), true);
        } else if pressed(Key::K) {
            self.toggle_note("ok");
        } else if pressed(Key::J) {
            self.next_unreviewed();
        } else if pressed(Key::R) {
            self.reload();
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

    /// Selects the oldest commit not yet ok on the selected commit's branch,
    /// or on the checked out branch when the selection is on none.
    fn next_unreviewed(&mut self) {
        let id = self.repo.commits[self.sel].id();
        let mut branches: Vec<&Location> = self.locs.branches.iter().collect();
        branches.sort_by_key(|b| !b.current);
        let Some(b) = branches.iter().find(|b| b.own.iter().any(|o| o == id)).or_else(|| branches.iter().find(|b| b.current)) else {
            return;
        };
        let next = b.own.iter().filter(|o| !self.notes.has(o, "ok")).filter_map(|o| self.repo.index_of(o)).max();
        match next {
            Some(row) => self.select(row, true),
            None => self.status = format!("{} has no commits left to review", b.name),
        }
    }

    /// Moves ok marks from rewritten commits onto their new versions.
    fn carry_marks(&mut self) {
        let own: std::collections::BTreeSet<&str> = self.locs.branches.iter().flat_map(|b| &b.own).map(String::as_str).collect();
        let own: Vec<&str> = own.into_iter().collect();
        if let Err(e) = self.repo.carry_tag(&mut self.notes, "ok", &own) {
            self.status = format!("gr.info: {e}");
        }
    }

    /// Toggles `tag` on the selected commit.
    fn toggle_note(&mut self, tag: &str) {
        let c = &self.repo.commits[self.sel];
        if c.is_fake() {
            return;
        }
        let id = c.id().to_string();
        let res = self.notes.toggle(&id, tag).and_then(|set| {
            // the patch id lets the mark follow the commit through a rebase
            let pid = if set && tag == "ok" { self.repo.patch_ids(&[&id]).remove(&id) } else { None };
            self.notes.set_value(&id, "patch-id", pid.as_deref())
        });
        if let Err(e) = res {
            self.status = format!("gr.info: {e}");
        }
    }

    fn id_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = ui.label("Commit ID:");
            let r = ui.add(TextEdit::singleline(&mut self.id_text).id(Id::unique(ID_EDIT)).font(egui::TextStyle::Monospace).desired_width(330.0)).labelled_by(label.id);
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
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.job.is_some() || self.loading.is_some() {
                ui.spinner();
            }
            self.id_bar(ui);
            ui.separator();
            self.find_bar(ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    self.settings.open = true;
                }
                ui.separator();
                let err = ui.visuals().error_fg_color;
                ui.add(egui::Label::new(RichText::new(&self.status).color(err)).truncate());
            });
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
            let label = ui.label("commit containing:");
            let r = ui.add(TextEdit::singleline(&mut self.find_text).id(Id::unique(FIND_EDIT)).desired_width(300.0)).labelled_by(label.id);
            if r.changed() {
                self.update_query();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.find(true, true);
                r.request_focus();
            }
            let (mode, field) = (self.mode, self.field);
            // named for screen readers, which have no label to go by
            let named = |r: egui::Response, name: &'static str| r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name));
            let r = ComboBox::from_id_salt("mode").selected_text(self.mode.label()).show_ui(ui, |ui| {
                for m in Mode::ALL {
                    ui.selectable_value(&mut self.mode, m, m.label());
                }
            });
            named(r.response, "Match");
            let r = ComboBox::from_id_salt("field").selected_text(self.field.label()).width(110.0).show_ui(ui, |ui| {
                for f in Field::ALL {
                    ui.selectable_value(&mut self.field, f, f.label());
                }
            });
            named(r.response, "Search in");
            if (mode, field) != (self.mode, self.field) {
                self.update_query();
            }
        });
    }

    fn commit_list(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        let scroll_to = std::mem::take(&mut self.scroll_list).then_some(self.sel);
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        let mut table = TableBuilder::new(ui)
            .id_salt("commits")
            .resizable(true)
            .sense(Sense::click())
            .animate_scrolling(false)
            .auto_shrink(false)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().clip(true));
        if let Some(r) = scroll_to {
            // recentre when far off, otherwise move just enough
            let rows = (self.list_h / LIST_H) as usize;
            let list_top = (self.list_off / LIST_H) as usize;
            let (top, bottom) = (list_top.saturating_sub(rows), list_top + 2 * rows);
            let align = if r < top || r > bottom { Some(Align::Center) } else { None };
            table = table.scroll_to_row(r, align);
        } else if let Some(off) = self.list_scroll.take() {
            table = table.vertical_scroll_offset(off);
        }
        let (repo, sel, query, notes) = (&self.repo, self.sel, &self.query, &self.notes);
        let done: std::collections::HashSet<&str> = self.locs.branches.iter()
            .filter(|b| { let (d, n) = reviewed(b, notes); n > 0 && d == n })
            .map(|b| b.name.as_str())
            .collect();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let out = table.body(|body| {
            body.rows(LIST_H, repo.commits.len(), |mut row| {
                let i = row.index();
                let c = &repo.commits[i];
                let hit = query.as_ref().map_or(Hit::No, |q| q.hit(c));
                row.set_selected(i == sel);
                row.col(|ui| draw_commit_cell(ui, c, &repo.segs(i), hit != Hit::No, notes.has(c.id(), "ok"), &done, now));
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
        let content_w = (self.max_cols as f32 * char_w).max(2.0 * self.half_cols as f32 * char_w + 8.0) + 8.0;
        let out = sa.show_rows(ui, ROW_H, lines.len(), |ui, range| {
            let w = content_w.max(ui.available_width());
            for line in &lines[range] {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::hover());
                let resp = match &line.right {
                    None => draw_line(ui, rect, line, char_w),
                    Some(right) => {
                        // each half its own column, the left cut at the middle
                        let mid = rect.left() + w / 2.0;
                        let (l, r) = (Rect::from_x_y_ranges(rect.left()..=mid, rect.y_range()), Rect::from_x_y_ranges(mid..=rect.right(), rect.y_range()));
                        let resp = draw_line(&mut ui.new_child(egui::UiBuilder::new().max_rect(l)), l, line, char_w);
                        draw_line(&mut ui.new_child(egui::UiBuilder::new().max_rect(r)), r, right, char_w);
                        ui.painter().vline(mid, rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
                        resp
                    }
                };
                if let Some(row) = line.link {
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

    /// The file list, floating over the top-right corner of the diff in `diff`.
    fn file_list(&mut self, ctx: &egui::Context, diff: Rect) {
        let cur = self.details.file_at(self.top_line());
        let mut jump = None;
        let files = &self.details.files;
        if files.is_empty() {
            return;
        }
        let widest = files.iter().map(|(name, _)| ctx.fonts_mut(|f| f.layout_no_wrap(name.clone(), font(13.0, false), Color32::WHITE).size().x)).fold(0.0, f32::max);
        let w = (widest + 8.0).min(diff.width() * 0.4);
        // clear of the diff's vertical scroll bar
        let pos = diff.right_top() + Vec2::new(-18.0, 6.0);
        let id = Id::unique("files");
        // see-through once the diff scrolls under it, solid again under the pointer
        let hovered = ctx.memory(|m| m.area_rect(id)).zip(ctx.input(|i| i.pointer.hover_pos())).is_some_and(|(r, p)| r.contains(p));
        let target = if self.diff_off.y > 0.0 && !hovered { 0.3 } else { 1.0 };
        let opacity = ctx.animate_value_with_time(id, target, 0.15);
        egui::Area::new(id).pivot(egui::Align2::RIGHT_TOP).fixed_pos(pos).show(ctx, |ui| {
            ui.set_opacity(opacity);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                // the area sizes itself from the last frame, which can cut the last
                // row when the file count changes, so give the height outright
                let h = (files.len() as f32 * ROW_H).min(diff.height() * 0.4);
                egui::ScrollArea::vertical().id_salt("files").min_scrolled_height(h).max_height(h).show_rows(ui, ROW_H, files.len(), |ui, range| {
                    for (i, (name, line)) in files.iter().enumerate().take(range.end).skip(range.start) {
                        let color = ui.visuals().text_color();
                        let g = galley(ui.painter(), name, font(13.0, false), color);
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
                        if i == cur {
                            ui.painter().rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
                        }
                        // long names are cut at the list's edge
                        ui.painter().with_clip_rect(rect).galley(Pos2::new(rect.left() + 4.0, rect.center().y - g.size().y / 2.0), g, color);
                        if resp.clicked() {
                            jump = Some(*line);
                        }
                    }
                });
            });
        });
        if let Some(line) = jump {
            self.scroll_diff(line as f32 * ROW_H);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let mut go = None;
        let cur = self.repo.commits[self.sel].id();
        let (l, notes) = (&self.locs, &self.notes);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // (title, entries, nest on '/', open by default)
            for (title, list, nest, open) in [
                ("BRANCHES", &l.branches, true, true),
                ("REMOTES", &l.remotes, true, false),
                ("TAGS", &l.tags, true, false),
                ("STASHES", &l.stashes, false, false),
                ("SUBMODULES", &l.submodules, false, false),
                ("WORKTREES", &l.worktrees, false, false),
            ] {
                let n = if title == "REMOTES" {
                    list.iter().map(|r| r.name.split('/').next()).collect::<std::collections::HashSet<_>>().len()
                } else {
                    list.len()
                };
                CollapsingHeader::new(RichText::new(format!("{title} ({n})")).strong()).default_open(open).show(ui, |ui| {
                    if nest {
                        location_tree(ui, title, list, 0, title == "BRANCHES", cur, notes, &mut go);
                    } else {
                        for loc in list.iter() {
                            location_row(ui, &loc.name, loc, cur, notes, &mut go);
                        }
                    }
                });
            }
        });
        if let Some(loc) = go {
            match self.repo.index_of(&loc.id) {
                Some(row) => self.select(row, true),
                None => self.status = format!("{} is not in this view", loc.name),
            }
        }
    }

    fn diff_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let opts = self.settings.diff.clone();
            let label = ui.label("Lines of context:");
            ui.add(egui::DragValue::new(&mut self.settings.diff.context).range(0..=1000)).labelled_by(label.id);
            ui.checkbox(&mut self.settings.diff.ignore_space, "Ignore space change");
            ui.checkbox(&mut self.settings.diff.split, "Side by side");
            if opts != self.settings.diff {
                self.select(self.sel, false);
            }
        });
    }
}

/// Rows of `list` (sorted by name) nested on the '/'-separated segment at `depth`.
fn location_tree(ui: &mut egui::Ui, salt: &str, list: &[Location], depth: usize, open: bool, cur: &str, notes: &Annotations, go: &mut Option<Location>) {
    let seg = |l: &Location| l.name.split('/').nth(depth).unwrap_or("").to_string();
    let leaf = |l: &Location| l.name.split('/').count() == depth + 1;
    let mut i = 0;
    while i < list.len() {
        let s = seg(&list[i]);
        if leaf(&list[i]) {
            location_row(ui, &s, &list[i], cur, notes, go);
            i += 1;
            continue;
        }
        // sorted names keep a shared prefix contiguous
        let n = list[i..].iter().take_while(|l| !leaf(l) && seg(l) == s).count();
        let path = list[i].name.split('/').take(depth + 1).collect::<Vec<_>>().join("/");
        CollapsingHeader::new(&s).id_salt((salt, &path)).default_open(open).show(ui, |ui| {
            location_tree(ui, salt, &list[i..i + n], depth + 1, open, cur, notes, go);
        });
        i += n;
    }
}

/// Paints `line` into `rect`: background, changed words, then its text as
/// a selectable label, so text can be selected across lines and copied.
fn draw_line(ui: &mut egui::Ui, rect: Rect, line: &gr::Line, char_w: f32) -> egui::Response {
    let fg = fg(ui, line.style.fg);
    let p = ui.painter().with_clip_rect(rect);
    if let Some(bg_c) = line.style.bg {
        let bg = bg(ui, bg_c);
        p.rect_filled(rect, 0.0, bg);
        // the changed words in a deeper shade of the line's colour
        for &(s, e) in &line.changed {
            let x = |c: usize| rect.left() + 4.0 + c as f32 * char_w;
            p.rect_filled(Rect::from_x_y_ranges(x(s)..=x(e), rect.y_range()), 2.0, bg.lerp_to_gamma(fg, 0.25));
        }
    }
    let g = line_galley(ui, line, fg);
    let tw = g.size().x;
    if line.link.is_some() {
        p.hline(rect.left() + 4.0..=rect.left() + 4.0 + tw, rect.center().y + 7.0, Stroke::new(1.0, fg));
    }
    let text_rect = Rect::from_center_size(Pos2::new(rect.left() + 4.0 + tw / 2.0, rect.center().y), g.size());
    // a child ui, unlike ui.put, leaves the row cursor alone; put
    // moved it up to the label's bottom and the rows overlapped
    let mut text = ui.new_child(egui::UiBuilder::new().max_rect(text_rect));
    text.set_clip_rect(rect.intersect(ui.clip_rect()));
    text.add(egui::Label::new(g).selectable(true))
}

/// A diff line's text in `fg`, with its syntax colours.
fn line_galley(ui: &egui::Ui, line: &gr::Line, fg: Color32) -> Arc<Galley> {
    let f = font(12.0, line.style.bold);
    if line.syntax.is_empty() {
        return galley(ui.painter(), &line.text, f, fg);
    }
    let byte = |c: usize| line.text.char_indices().nth(c).map_or(line.text.len(), |(b, _)| b);
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for &(s, e, color) in &line.syntax {
        let (s, e) = (byte(s), byte(e));
        job.append(&line.text[at..s], 0.0, egui::TextFormat::simple(f.clone(), fg));
        job.append(&line.text[s..e], 0.0, egui::TextFormat::simple(f.clone(), self::fg(ui, color)));
        at = e;
    }
    job.append(&line.text[at..], 0.0, egui::TextFormat::simple(f, fg));
    ui.painter().layout_job(job)
}

/// (ok, all) of a branch's own commits.
fn reviewed(loc: &Location, notes: &Annotations) -> (usize, usize) {
    (loc.own.iter().filter(|id| notes.has(id, "ok")).count(), loc.own.len())
}

fn location_row(ui: &mut egui::Ui, name: &str, loc: &Location, cur: &str, notes: &Annotations, go: &mut Option<Location>) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    let p = ui.painter();
    if !loc.id.is_empty() && loc.id == cur {
        p.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
    } else if resp.hovered() {
        p.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    // the counts keep their place on the right; the name is cut short before them
    let mut right = rect.right() - 4.0;
    if let Some((behind, ahead)) = loc.track.filter(|&t| t != (0, 0)) {
        let weak = ui.visuals().weak_text_color();
        let g = galley(p, &format!("{behind}↓ {ahead}↑"), font(12.0, false), weak);
        right -= g.size().x;
        paint_text(p, right, rect.center().y, g, weak);
        right -= 8.0;
    }
    let (done, n) = reviewed(loc, notes);
    if done > 0 {
        // ✓ once every commit is ok, the progress until then
        let (text, color) = if done == n { ("✓".to_string(), fg(ui, [0, 0x98, 0])) } else { (format!("{done}/{n}"), ui.visuals().weak_text_color()) };
        let g = galley(p, &text, font(12.0, done == n), color);
        right -= g.size().x;
        paint_text(p, right, rect.center().y, g, color);
        right -= 8.0;
    }
    let text = ui.visuals().text_color();
    let g = galley(p, name, font(13.0, loc.current), text);
    p.with_clip_rect(Rect::from_x_y_ranges(rect.left()..=right, rect.y_range()))
        .galley(Pos2::new(rect.left() + 2.0, rect.center().y - g.size().y / 2.0), g, text);
    if resp.on_hover_text(&loc.name).clicked() && !loc.id.is_empty() {
        *go = Some(loc.clone());
    }
}

/// A commit list row: graph, refs and subject on the first line;
/// committer and short date on the second.
/// `done` names the branches whose own commits are all ok; their labels get a ✓.
fn draw_commit_cell(ui: &mut egui::Ui, c: &gr::Commit, segs: &[Seg], bold: bool, ok: bool, done: &std::collections::HashSet<&str>, now: i64) {
    let rect = ui.max_rect();
    let p = ui.painter();
    let text = ui.visuals().text_color();
    let x0 = rect.left() + 3.0 + LANE_W / 2.0;
    // the node sits on the first line, so stretch the lower half of the graph
    let pt = |x: f32, y: f32| {
        let py = if y <= 0.5 { y * ROW_H } else { ROW_H / 2.0 + (y - 0.5) * 2.0 * (rect.height() - ROW_H / 2.0) };
        Pos2::new(x0 + x * LANE_W, rect.top() + py)
    };
    let thick = 1.5;
    for s in segs {
        p.line_segment([pt(s.x0, s.y0), pt(s.x1, s.y1)], Stroke::new(thick, fg(ui, s.color)));
    }
    let node = pt(c.lane as f32, 0.5);
    let text_x = x0 + c.width.max(1) as f32 * LANE_W - LANE_W / 2.0 + 6.0;
    let mut x = text_x;
    let y = node.y;
    if !c.refs.is_empty() {
        p.hline(node.x..=x, y, Stroke::new(1.0, text));
        // gitk's label colours, as a tint of the theme's matching colour
        let panel = ui.visuals().panel_fill;
        let tint = |c: Rgb| panel.lerp_to_gamma(fg(ui, c), 0.3);
        for r in &c.refs {
            let accent = fg(ui, r.accent());
            let check = r.kind == RefKind::Head && done.contains(r.name.as_str());
            let name = if check { format!("{} ✓", r.name) } else { r.name.clone() };
            let g = galley(p, &name, font(11.0, r.current), text);
            let w = g.size().x + 6.0;
            let b = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(w, ROW_H - 4.0));
            p.rect_filled(b, 2.0, tint(r.accent()));
            let n = r.remote_prefix_len();
            if r.kind == RefKind::Remote && n > 0 {
                let pw = galley(p, &r.name[..n], font(11.0, r.current), text).size().x + 3.0;
                p.rect_filled(Rect::from_min_size(b.min, Vec2::new(pw, b.height())), 2.0, tint(REMOTE_PREFIX));
            }
            p.rect_stroke(b, 2.0, Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            paint_text(p, x + 3.0, y, g, text);
            x += w + 4.0;
        }
    }
    let fill = if ok { [0, 0x98, 0] } else { c.node_fill };
    p.circle(node, 3.5, fg(ui, fill), Stroke::new(1.0, Color32::BLACK));
    let g = galley(p, c.subject(), font(13.0, bold), text);
    paint_text(p, x, y, g, text);

    let weak = ui.visuals().weak_text_color();
    let y2 = y + ROW_H;
    let date = galley(p, &c.committer_date_short(now), font(12.0, false), weak);
    let mut date_x = rect.right() - date.size().x - 6.0;
    paint_text(p, date_x, y2, date, weak);
    if ok {
        let g = galley(p, "✓ ok", font(12.0, true), fg(ui, [0, 0x98, 0]));
        date_x -= g.size().x + 8.0;
        paint_text(p, date_x, y2, g, fg(ui, [0, 0x98, 0]));
    }
    let name = galley(p, c.committer_name(), font(12.0, false), weak);
    // keep the name from running under the date
    p.with_clip_rect(Rect::from_x_y_ranges(rect.left()..=date_x - 8.0, rect.y_range()))
        .galley(Pos2::new(text_x, y2 - name.size().y / 2.0), name, weak);
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        self.poll_reload();
        if self.watch.changed() && self.job.is_none() {
            self.reload();
        }
        self.keys(ui.ctx());
        let w = ui.available_width();
        let mut clicked = self.native.as_ref().and_then(|n| n.clicked());
        if self.native.is_none() {
            Panel::top("menubar").show(ui, |ui| clicked = menu::egui_bar(ui, self.job.is_none()));
        }
        match clicked {
            Some(menu::Action::Git(i)) if self.job.is_none() => self.run_git(ui.ctx(), menu::GIT[i].1),
            Some(menu::Action::Settings) => self.settings.open = true,
            Some(menu::Action::Shortcuts) => self.shortcuts = true,
            _ => {}
        }
        egui::Window::new("Keyboard Shortcuts").open(&mut self.shortcuts).resizable(false).collapsible(false).show(ui.ctx(), |ui| {
            for &(group, keys) in SHORTCUTS {
                ui.label(RichText::new(group).strong());
                egui::Grid::new(group).num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, what) in keys {
                        ui.label(RichText::new(*k).monospace());
                        ui.label(*what);
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
            }
        });
        if self.settings.show(ui.ctx()) {
            self.select(self.sel, false);
        }
        let frame = egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 8));
        Panel::top("toolbar").frame(frame).show(ui, |ui| self.toolbar(ui));
        Panel::bottom("statusbar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space((self.diff_left - ui.cursor().left()).max(0.0));
                self.diff_bar(ui);
            });
        });
        Panel::left("locations").resizable(true).default_size(230.0).show(ui, |ui| self.sidebar(ui));
        Panel::left("list").resizable(true).default_size(w * 0.35).min_size(200.0).show(ui, |ui| {
            self.commit_list(ui);
        });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            let left = ui.max_rect().left();
            if left != self.diff_left {
                // the bottom bar used the old edge this frame
                self.diff_left = left;
                ui.ctx().request_repaint();
            }
            self.diff_view(ui);
            self.file_list(ui.ctx(), ui.max_rect());
        });
    }
}

/// The app, or the history still loading at startup.
enum Gr {
    Loading(Option<(std::thread::JoinHandle<std::io::Result<Repo>>, Option<menu::Native>, settings::Settings)>),
    Ready(Box<App>),
}

impl eframe::App for Gr {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Gr::Ready(app) = self {
            app.save(storage);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Gr::Loading(slot) = self {
            if !slot.as_ref().is_some_and(|(job, _, _)| job.is_finished()) {
                egui::CentralPanel::default_margins().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 2.0 - 30.0);
                        ui.spinner();
                        ui.label("Loading history…");
                    });
                });
                // the loader thread cannot wake egui
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            let (job, native, settings) = slot.take().unwrap();
            match job.join().unwrap_or_else(|_| Err(std::io::Error::other("load thread panicked"))) {
                Ok(repo) => {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(repo.title()));
                    *self = Gr::Ready(Box::new(App::new(repo, native, settings, ui.ctx())));
                }
                Err(e) => {
                    eprintln!("gr: {e}");
                    std::process::exit(1);
                }
            }
        }
        if let Gr::Ready(app) = self {
            app.ui(ui, frame);
        }
    }
}

/// Fonts and saved settings, before the first frame. Without
/// `system_fonts` only the bundled fonts are used, as the tests do.
fn init(cc: &eframe::CreationContext, system_fonts: bool) -> settings::Settings {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("regular".into(), Arc::new(egui::FontData::from_static(include_bytes!("../fonts/LythMono-Regular.ttf"))));
    fonts.font_data.insert("bold".into(), Arc::new(egui::FontData::from_static(include_bytes!("../fonts/LythMono-Bold.ttf"))));
    // Menlo ships with macOS but may not be redistributed, so load it
    // from the system and keep Lyth Mono for other platforms.
    if let Some(menlo) = system_fonts.then(|| std::fs::read("/System/Library/Fonts/Menlo.ttc").ok()).flatten() {
        for (name, index) in [("regular", 0), ("bold", 1)] {
            let mut f = egui::FontData::from_owned(menlo.clone());
            f.index = index;
            fonts.font_data.insert(name.into(), Arc::new(f));
        }
    }
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(fam).or_default().insert(0, "regular".into());
    }
    let bold = std::iter::once("bold".into()).chain(fonts.families[&FontFamily::Proportional].clone()).collect();
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    cc.egui_ctx.set_fonts(fonts);
    // egui follows the system light/dark setting until the toolbar overrides it
    settings::Settings::new(&cc.egui_ctx, |k| cc.storage.and_then(|s| s.get_string(k)))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| ".".into());
    let log_args: Vec<String> = args.collect();
    let loading = std::thread::spawn(move || Repo::open(&dir, &log_args));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("gr").with_inner_size([1600.0, 900.0]),
        ..Default::default()
    };
    let res = eframe::run_native(
        "gr",
        options,
        Box::new(|cc| {
            let settings = init(cc, true);
            let native = menu::Native::new(cc);
            Ok(Box::new(Gr::Loading(Some((loading, native, settings)))))
        }),
    );
    if let Err(e) = res {
        eprintln!("gr: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    /// A small repository with fixed names and dates, so renders repeat.
    fn sample_repo() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gr-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut n = 0;
        let mut sh = |args: &[&str]| {
            n += 1;
            let date = format!("2001-02-03T04:{n:02}:00Z");
            let ok = std::process::Command::new("git")
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .args(["-c", "user.name=Ada Lovelace", "-c", "user.email=ada@example.com", "-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{args:?}");
        };
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        sh(&["init", "-q", "-b", "main"]);
        write("lib.rs", "/// Says hello.\nfn greet(name: &str) -> String {\n    format!(\"hello {}\", name)\n}\n");
        sh(&["add", "lib.rs"]);
        sh(&["commit", "-q", "-m", "lib: greet by name"]);
        sh(&["tag", "v1.0"]);
        sh(&["checkout", "-q", "-b", "topic"]);
        write("lib.rs", "/// Says hello.\nfn greet(name: &str, times: u32) -> String {\n    format!(\"hello {}\", name).repeat(times as usize)\n}\n");
        sh(&["commit", "-q", "-am", "lib: greet more than once"]);
        sh(&["checkout", "-q", "main"]);
        write("readme.txt", "hello\n");
        sh(&["add", "readme.txt"]);
        sh(&["commit", "-q", "-m", "readme: say hello"]);
        dir
    }

    #[test]
    fn snapshots() {
        let dir = sample_repo();
        let repo = Repo::open(&dir, &[]).unwrap();
        // topic's only commit reviewed: its node, the ok label and the branch check
        let mut notes = repo.annotations().unwrap();
        let topic = repo.commits.iter().position(|c| c.subject() == "lib: greet more than once").unwrap();
        let topic_id = repo.commits[topic].id().to_string();
        notes.toggle(&topic_id, "ok").unwrap();
        let mut harness = Harness::builder()
            .with_size([1600.0, 420.0])
            .build_eframe(|cc| App::new(repo, None, init(cc, false), &cc.egui_ctx));
        for (pref, name) in [(egui::ThemePreference::Light, "light"), (egui::ThemePreference::Dark, "dark")] {
            harness.ctx.set_theme(pref);
            harness.run();
            harness.snapshot(name);
        }
        // word diff and syntax colours
        harness.state_mut().select(topic, true);
        for (pref, name) in [(egui::ThemePreference::Light, "diff_light"), (egui::ThemePreference::Dark, "diff_dark")] {
            harness.ctx.set_theme(pref);
            harness.run();
            harness.snapshot(name);
        }
        harness.state_mut().settings.diff.split = true;
        harness.state_mut().select(topic, true);
        harness.run();
        harness.snapshot("diff_split");
        harness.state_mut().settings.diff.split = false;
        harness.state_mut().select(0, true);
        harness.state_mut().settings.open = true;
        harness.ctx.set_theme(egui::ThemePreference::Dark);
        harness.set_size(egui::Vec2::new(1100.0, 760.0));
        harness.run();
        harness.snapshot("settings");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
