use egui::{Color32, RichText, Stroke, Ui};

use crate::app::AppState;
use crate::session::Session;
use crate::token::{check_token, gen_token, Token};

pub struct TabBarOutput {
    pub open_settings: bool,
    pub open_token_prompt: bool,
    pub import: bool,
    pub export: bool,
}

pub fn show(app: &mut AppState, ui: &mut Ui) -> TabBarOutput {
    let mut out = TabBarOutput {
        open_settings: false,
        open_token_prompt: false,
        import: false,
        export: false,
    };

    // Buffered actions so we don't try to re-borrow `app` while iterating.
    let mut pending: Vec<(Token, TabAction)> = Vec::new();
    let mut add_random = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;

        let tokens: Vec<Token> = app.sessions.keys().cloned().collect();
        let active = app.active.clone();

        for token in &tokens {
            let session = match app.sessions.get(token) { Some(s) => s, None => continue };
            let is_active = active.as_ref() == Some(token);
            let clicked = tab_button(ui, session, is_active);
            if let Some(action) = clicked { pending.push((token.clone(), action)); }
        }

        if ui.button("+").on_hover_text("New random token").clicked() { add_random = true; }
        if ui.button("+token").on_hover_text("Attach an existing token").clicked() {
            out.open_token_prompt = true;
        }

        ui.separator();

        if ui.button("Import").on_hover_text("Load tokens from a JSON file").clicked() {
            out.import = true;
        }
        if ui.button("Export").on_hover_text("Save tokens to a JSON file").clicked() {
            out.export = true;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⚙").on_hover_text("Settings").clicked() {
                out.open_settings = true;
            }
        });
    });

    for (token, action) in pending {
        match action {
            TabAction::Select => app.select_tab(&token),
            TabAction::Close => app.close_tab(&token),
        }
    }
    if add_random { app.add_tab(gen_token()); }

    out
}

pub enum TabAction { Select, Close }

fn tab_button(ui: &mut Ui, session: &Session, is_active: bool) -> Option<TabAction> {
    let display = tab_display_name(session);
    let text = if is_active {
        RichText::new(display).strong()
    } else {
        RichText::new(display)
    };

    let frame_color = if is_active {
        ui.visuals().widgets.active.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };

    let mut clicked: Option<TabAction> = None;
    egui::Frame::none()
        .fill(frame_color)
        .stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                if label.clicked() { clicked = Some(TabAction::Select); }

                let (color, tooltip) = status_indicator(session);
                let (rect, _resp) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.add_space(2.0);

                if ui.small_button("×").on_hover_text(tooltip).clicked() {
                    clicked = Some(TabAction::Close);
                }
            });
        });
    clicked
}

fn tab_display_name(session: &Session) -> String {
    if let Some(label) = &session.info.label { return label.clone(); }
    if let Some(id) = session.info.computer_id { return format!("Computer #{id}"); }
    session.token.short().to_string()
}

fn status_indicator(session: &Session) -> (Color32, &'static str) {
    use crate::session::SessionStatus::*;
    match &session.status {
        Connecting => (Color32::from_rgb(200, 200, 0), "Connecting…"),
        Waiting => (Color32::from_rgb(200, 150, 0), "Connected to relay; waiting for computer"),
        Connected => (Color32::from_rgb(0, 200, 0), "Computer connected"),
        LostConnection => (Color32::from_rgb(200, 0, 0), "Lost connection"),
        Errored(_) => (Color32::from_rgb(200, 0, 0), "Error"),
    }
}

/// Inline pop-up for "+token". Returns Some(token) when the user submits.
pub fn token_prompt(ui: &mut Ui, buf: &mut String) -> TokenPromptOutcome {
    ui.horizontal(|ui| {
        ui.label("Token:");
        let resp = ui.add(egui::TextEdit::singleline(buf)
            .desired_width(260.0)
            .hint_text("32-char alphanumeric token"));

        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let submit = ui.button("Add").clicked() || enter;
        let cancel = ui.button("Cancel").clicked()
            || ui.input(|i| i.key_pressed(egui::Key::Escape));

        if submit {
            let trimmed = buf.trim().to_string();
            if check_token(&trimmed) {
                return TokenPromptOutcome::Submit(Token::new(trimmed).unwrap());
            } else {
                return TokenPromptOutcome::Invalid;
            }
        }
        if cancel { return TokenPromptOutcome::Cancel; }
        TokenPromptOutcome::Pending
    }).inner
}

pub enum TokenPromptOutcome {
    Pending,
    Submit(Token),
    Invalid,
    Cancel,
}
