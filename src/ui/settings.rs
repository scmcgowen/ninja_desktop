use crate::settings::Settings;
use egui::Ui;

pub fn show(settings: &mut Settings, ui: &mut Ui) -> bool {
    let mut close = false;
    ui.vertical(|ui| {
        ui.heading("Settings");
        ui.separator();

        ui.checkbox(&mut settings.dark_mode, "Dark mode");
        ui.horizontal(|ui| {
            ui.label("Font size");
            ui.add(egui::Slider::new(&mut settings.font_size, 10.0..=24.0).step_by(1.0));
        });
        ui.checkbox(&mut settings.trim_whitespace_on_save, "Trim trailing whitespace on save");

        ui.horizontal(|ui| {
            ui.label("Server host");
            ui.text_edit_singleline(&mut settings.server_host)
                .on_hover_text("Defaults to cc.minecartchris.cc; SAMARI_DEV=1 overrides to localhost:8080");
        });

        ui.separator();
        if ui.button("Close").clicked() { close = true; }
    });
    close
}
