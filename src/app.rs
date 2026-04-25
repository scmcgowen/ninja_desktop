use egui::Ui;
use indexmap::IndexMap;
use std::sync::Arc;
use tokio::runtime::Runtime;

use crate::session::Session;
use crate::settings::Settings;
use crate::storage::{self, AppData};
use crate::token::Token;
use crate::ui;

pub struct AppState {
    pub sessions: IndexMap<Token, Session>,
    pub active: Option<Token>,
    pub settings: Settings,
    pub runtime: Arc<Runtime>,
    pub egui_ctx: egui::Context,

    // Transient UI state --------------------------------------------------
    pub show_settings: bool,
    pub token_prompt: Option<String>,
    pub dirty: bool,
    /// Set to `View::Terminal` to swap the main pane to the terminal stub.
    /// Files is the default.
    pub main_view: MainView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainView { Files, Terminal }

impl AppState {
    pub fn new(runtime: Arc<Runtime>, egui_ctx: egui::Context) -> Self {
        let data = storage::load();
        let mut app = Self {
            sessions: IndexMap::new(),
            active: None,
            settings: data.settings,
            runtime,
            egui_ctx: egui_ctx.clone(),
            show_settings: false,
            token_prompt: None,
            dirty: false,
            main_view: MainView::Files,
        };

        for token in data.tokens {
            app.ensure_session(token);
        }
        app.active = data.active.filter(|t| app.sessions.contains_key(t))
            .or_else(|| app.sessions.keys().next().cloned());

        app
    }

    pub fn add_tab(&mut self, token: Token) {
        self.ensure_session(token.clone());
        self.active = Some(token);
        self.dirty = true;
    }

    pub fn select_tab(&mut self, token: &Token) {
        if self.sessions.contains_key(token) {
            self.active = Some(token.clone());
            self.dirty = true;
        }
    }

    pub fn close_tab(&mut self, token: &Token) {
        let idx = self.sessions.get_index_of(token);
        self.sessions.shift_remove(token);
        if self.active.as_ref() == Some(token) {
            // Pick neighbour — previous if possible, else first remaining.
            let keys: Vec<Token> = self.sessions.keys().cloned().collect();
            self.active = idx.and_then(|i| keys.get(i.saturating_sub(1)).cloned())
                .or_else(|| keys.first().cloned());
        }
        self.dirty = true;
    }

    fn ensure_session(&mut self, token: Token) {
        if self.sessions.contains_key(&token) { return; }
        let session = Session::spawn(
            token.clone(),
            &self.settings,
            self.runtime.handle(),
            self.egui_ctx.clone(),
        );
        self.sessions.insert(token, session);
    }

    fn snapshot(&self) -> AppData {
        AppData {
            tokens: self.sessions.keys().cloned().collect(),
            active: self.active.clone(),
            settings: self.settings.clone(),
        }
    }

    pub fn persist_if_dirty(&mut self) {
        if !self.dirty { return; }
        if let Err(e) = storage::save(&self.snapshot()) {
            log::warn!("failed to persist config: {e}");
        }
        self.dirty = false;
    }
}

impl eframe::App for AppState {

    fn ui(&mut self, ctx: &mut Ui, _frame: &mut eframe::Frame) {

        // 1. Apply theme.
        ctx.set_visuals(if self.settings.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        // 2. Drain per-session inbound channels.
        let mut state_changed = false;
        for session in self.sessions.values_mut() {
            if session.pump() { state_changed = true; }
        }

        // 3. Top tab bar.
        let tab_out = egui::TopBottomPanel::top("tab_bar")
            .show(ctx, |ui| ui::tabs::show(self, ui))
            .inner;
        if tab_out.open_settings { self.show_settings = true; }
        if tab_out.open_token_prompt { self.token_prompt = Some(String::new()); }
        if tab_out.import { self.run_import(); }
        if tab_out.export { self.run_export(); }

        // 4. Optional modal: token prompt inline at top.
        if let Some(buf) = self.token_prompt.as_mut() {
            let mut to_apply: Option<crate::ui::tabs::TokenPromptOutcome> = None;
            egui::TopBottomPanel::top("token_prompt").show(ctx, |ui| {
                to_apply = Some(ui::tabs::token_prompt(ui, buf));
            });
            match to_apply.unwrap_or(crate::ui::tabs::TokenPromptOutcome::Pending) {
                crate::ui::tabs::TokenPromptOutcome::Submit(t) => {
                    self.add_tab(t);
                    self.token_prompt = None;
                }
                crate::ui::tabs::TokenPromptOutcome::Cancel => {
                    self.token_prompt = None;
                }
                crate::ui::tabs::TokenPromptOutcome::Invalid => {
                    // Keep the prompt open so the user sees the input they typed;
                    // a toast would be nicer but we don't have one yet.
                    log::warn!("invalid token entered");
                }
                crate::ui::tabs::TokenPromptOutcome::Pending => {}
            }
        }

        // 5. Settings window.
        if self.show_settings {
            let mut open = true;
            egui::Window::new("Settings")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    if ui::settings::show(&mut self.settings, ui) {
                        self.show_settings = false;
                        self.dirty = true;
                    }
                });
            if !open { self.show_settings = false; }
            self.dirty = true;
        }

        // 6. View switcher (Files | Terminal).
        egui::TopBottomPanel::top("view_switch").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.main_view, MainView::Files, "Files");
                ui.selectable_value(&mut self.main_view, MainView::Terminal, "Terminal");
            });
        });

        // 7. Main content.
        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(active_token) = self.active.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("No sessions. Click + to create a tab.");
                });
                return;
            };
            let Some(session) = self.sessions.get_mut(&active_token) else { return };

            match self.main_view {
                MainView::Terminal => {
                    let settings = self.settings.clone();
                    ui::terminal::show(session, &settings, ui);
                    drop(settings);
                },
                MainView::Files => {
                    let settings = self.settings.clone();
                    let action = ui::files::show(session, &settings, ui);
                    drop(settings);
                    apply_files_action(session, action, self.settings.trim_whitespace_on_save);
                }
            }
        });

        if state_changed { self.dirty = true; }
        self.persist_if_dirty();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.dirty = true;
        self.persist_if_dirty();
    }
}

fn apply_files_action(session: &mut Session, action: ui::files::FilesAction, trim_ws: bool) {
    use ui::files::FilesAction::*;
    match action {
        Idle => {}
        Save => {
            if let Err(e) = session.save_active(trim_ws) {
                log::error!("save failed: {e}");
                session.push_notification(
                    crate::session::NotificationKind::Error,
                    "save",
                    format!("Save failed: {e}"),
                );
            }
        }
        SelectFile(name) => { session.active_file = name; }
        CloseFile(name) => {
            session.files.retain(|f| f.name != name);
            if session.active_file.as_deref() == Some(name.as_str()) {
                session.active_file = None;
            }
        }
        DismissNotification(id) => {
            session.notifications.retain(|n| n.id != id);
        }
    }
}

impl AppState {
    fn run_import(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Samari tokens", &["json"])
            .pick_file() else { return };
        match storage::import_tokens(&path) {
            Ok(tokens) => {
                for t in tokens { self.ensure_session(t); }
                self.dirty = true;
            }
            Err(e) => log::warn!("import failed: {e}"),
        }
    }

    fn run_export(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Samari tokens", &["json"])
            .set_file_name("samari-tokens.json")
            .save_file() else { return };
        let tokens: Vec<Token> = self.sessions.keys().cloned().collect();
        if let Err(e) = storage::export_tokens(&path, &tokens) {
            log::warn!("export failed: {e}");
        }
    }
}
