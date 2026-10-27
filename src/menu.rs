//! The menu bar: native on macOS and Windows, drawn with egui elsewhere.

/// Git menu entries: label and git arguments.
pub const GIT: [(&str, &[&str]); 3] = [
    ("Fetch All", &["fetch", "--all", "--prune"]),
    ("Pull", &["pull"]),
    ("Rebase onto Upstream", &["rebase", "@{upstream}"]),
];

/// A clicked menu entry.
pub enum Action {
    /// Index into GIT.
    Git(usize),
    Settings,
    Shortcuts,
}

/// Draws the menu bar in the window, for platforms without a native one.
pub fn egui_bar(ui: &mut eframe::egui::Ui, enabled: bool) -> Option<Action> {
    let mut clicked = None;
    eframe::egui::MenuBar::new().ui(ui, |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Git", |ui| {
                for (i, (label, _)) in GIT.iter().enumerate() {
                    if ui.button(*label).clicked() {
                        clicked = Some(Action::Git(i));
                    }
                }
            });
        });
        if ui.button("Settings").clicked() {
            clicked = Some(Action::Settings);
        }
        ui.menu_button("Help", |ui| {
            if ui.button("Keyboard Shortcuts").clicked() {
                clicked = Some(Action::Shortcuts);
            }
        });
    });
    clicked
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use native::Native;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::sync::mpsc::{channel, Receiver};

    use muda::{Menu, MenuEvent, MenuItem, Submenu};

    use super::Action;

    pub struct Native {
        // the OS menu goes away when this drops
        _menu: Menu,
        git: Vec<MenuItem>,
        events: Receiver<MenuEvent>,
    }

    impl Native {
        pub fn new(cc: &eframe::CreationContext) -> Option<Native> {
            let git: Vec<MenuItem> = super::GIT
                .iter()
                .enumerate()
                .map(|(i, (label, _))| MenuItem::with_id(i.to_string(), *label, true, None))
                .collect();
            let git_menu = Submenu::new("Git", true);
            for item in &git {
                git_menu.append(item).ok()?;
            }
            let help = Submenu::with_items(
                "Help",
                true,
                &[&MenuItem::with_id("shortcuts", "Keyboard Shortcuts", true, None)],
            )
            .ok()?;
            let menu = Menu::new();
            #[cfg(target_os = "macos")]
            {
                use muda::accelerator::{Accelerator, Code, Modifiers};
                use muda::PredefinedMenuItem as P;
                // replaces winit's default menu, so carry its app menu over
                let app = Submenu::with_items(
                    "gr",
                    true,
                    &[
                        &P::about(None, None),
                        &P::separator(),
                        &MenuItem::with_id(
                            "settings",
                            "Settings…",
                            true,
                            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Comma)),
                        ),
                        &P::separator(),
                        &P::services(None),
                        &P::separator(),
                        &P::hide(None),
                        &P::hide_others(None),
                        &P::show_all(None),
                        &P::separator(),
                        &P::quit(None),
                    ],
                )
                .ok()?;
                let window = Submenu::with_items(
                    "Window",
                    true,
                    &[&P::minimize(None), &P::fullscreen(None), &P::bring_all_to_front(None)],
                )
                .ok()?;
                menu.append_items(&[&app, &git_menu, &window, &help]).ok()?;
                window.set_as_windows_menu_for_nsapp();
                help.set_as_help_menu_for_nsapp();
                menu.init_for_nsapp();
            }
            #[cfg(target_os = "windows")]
            {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                menu.append_items(&[&git_menu, &help]).ok()?;
                let RawWindowHandle::Win32(h) = cc.window_handle().ok()?.as_raw() else {
                    return None;
                };
                // SAFETY: the handle is eframe's live main window.
                unsafe { menu.init_for_hwnd(h.hwnd.get()) }.ok()?;
            }
            // menu clicks don't wake egui, so repaint on each one
            let (tx, events) = channel();
            let ctx = cc.egui_ctx.clone();
            MenuEvent::set_event_handler(Some(move |e| {
                let _ = tx.send(e);
                ctx.request_repaint();
            }));
            Some(Native { _menu: menu, git, events })
        }

        /// The entry clicked since the last call.
        pub fn clicked(&self) -> Option<Action> {
            self.events.try_iter().find_map(|e| match e.id.0.as_str() {
                "settings" => Some(Action::Settings),
                "shortcuts" => Some(Action::Shortcuts),
                id => id.parse().ok().map(Action::Git),
            })
        }

        pub fn set_enabled(&self, enabled: bool) {
            for item in &self.git {
                item.set_enabled(enabled);
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub struct Native;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Native {
    pub fn new(_: &eframe::CreationContext) -> Option<Native> {
        None
    }

    pub fn clicked(&self) -> Option<super::Action> {
        None
    }

    pub fn set_enabled(&self, _: bool) {}
}
