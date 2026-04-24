//! Placeholder for the terminal view. The real renderer will replace this once
//! the file editor is considered done (see PLAN.md).

use egui::Ui;
use crate::session::Session;
use crate::settings::Settings;

pub fn show(session: &Session, settings: &Settings, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.heading("Terminal view — not yet implemented");
        ui.label("Open the web UI to interact with this computer's terminal.");
        ui.add_space(8.0);
        let url = format!("https://{}/?id={}", settings.server_host, session.token.as_str());
        ui.hyperlink_to(&url, &url);
    });
}
