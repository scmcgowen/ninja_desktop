use egui::{FontId, RichText, ScrollArea, Ui};

use crate::session::{NotificationKind, Session, SessionStatus};
use crate::settings::Settings;

pub enum FilesAction {
    Idle,
    Save,
    CloseFile(String),
    SelectFile(Option<String>),
    DismissNotification(String),
}

/// Draw the file list sidebar + editor or welcome screen. Returns an action
/// the caller should apply (save / close file / etc.) — we return rather than
/// mutate in-place so borrow gymnastics stay sane.
pub fn show(session: &mut Session, settings: &Settings, ui: &mut Ui) -> FilesAction {
    let mut action = FilesAction::Idle;

    // Global save shortcut (Ctrl+S). egui's input is cheap; no need to gate.
    if ui.input(|i| i.modifiers.command_only() && i.key_pressed(egui::Key::S)) {
        action = FilesAction::Save;
    }

    egui::SidePanel::left("file_list")
        .resizable(true)
        .default_width(220.0)
        .min_width(160.0)
        .show_inside(ui, |ui| {
            ui.heading("Files");
            ui.label(
                RichText::new(format!("Token: {}", session.token.as_str()))
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            ui.separator();

            let status_label = match &session.status {
                SessionStatus::Connecting => "Connecting…".to_string(),
                SessionStatus::Waiting => "Waiting for computer…".to_string(),
                SessionStatus::Connected => "Connected".to_string(),
                SessionStatus::LostConnection => "Lost connection".to_string(),
                SessionStatus::Errored(e) => format!("Error: {e}"),
            };
            ui.label(status_label);
            ui.separator();

            if session.files.is_empty() {
                ui.label(
                    RichText::new("No files yet — run `samari edit <file>` on the computer.")
                        .italics()
                        .weak(),
                );
                return;
            }

            ScrollArea::vertical().show(ui, |ui| {
                // Collect names first to avoid double-borrow when we mutate below.
                let rows: Vec<(String, bool, bool, bool)> = session.files.iter()
                    .map(|f| (f.name.clone(), f.modified(), f.read_only, f.is_new))
                    .collect();
                let active = session.active_file.clone();

                for (name, modified, read_only, is_new) in rows {
                    let selected = active.as_deref() == Some(name.as_str());
                    ui.horizontal(|ui| {
                        let label = if modified {
                            format!("● {}",name)
                        } else {
                            name.clone()
                        };
                        let resp = ui.selectable_label(selected, label);
                        if resp.clicked() {
                            action = FilesAction::SelectFile(Some(name.clone()));
                        }
                        if read_only {
                            ui.label(RichText::new("ro").weak().small())
                                .on_hover_text("Read only");
                        }
                        if is_new {
                            ui.label(RichText::new("new").weak().small())
                                .on_hover_text("New file (not yet saved remotely)");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Close file").clicked() {
                                action = FilesAction::CloseFile(name.clone());
                            }
                        });
                    });
                }
            });
        });

    // Main area: editor for the active file, or a welcome screen.
    egui::CentralPanel::default().show_inside(ui, |ui| {
        // Notifications first so they're always visible.
        show_notifications(session, ui, &mut action);

        let Some(name) = session.active_file.clone() else {
            welcome_screen(session, ui);
            return;
        };
        let Some(idx) = session.files.iter().position(|f| f.name == name) else {
            session.active_file = None;
            welcome_screen(session, ui);
            return;
        };

        let file = &mut session.files[idx];

        ui.horizontal(|ui| {
            ui.heading(&file.name);
            if file.read_only { ui.label("(read only)"); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Save (Ctrl+S)").clicked() {
                    action = FilesAction::Save;
                }
            });
        });
        ui.separator();

        let text_edit = egui::TextEdit::multiline(&mut file.buffer)
            .font(FontId::monospace(settings.font_size))
            .code_editor()
            .desired_width(f32::INFINITY)
            .desired_rows(24)
            .interactive(!file.read_only);

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| { ui.add_sized(ui.available_size(), text_edit); });
    });

    action
}

fn welcome_screen(session: &Session, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.heading("Ninja Catcher — desktop");
        ui.label(
            "Use `ninja edit <file>` on the computer to open a file here, \
             or attach to an existing session via `+token`.",
        );
        ui.add_space(12.0);
        ui.group(|ui| {
            ui.label("In-game setup:");
            ui.code(format!(
                "wget https://ninja.reconnected.cc/ninja.lua\nninja.lua {}",
                session.token.as_str()
            ));
        });
    });
}

fn show_notifications(session: &mut Session, ui: &mut Ui, action: &mut FilesAction) {
    if session.notifications.is_empty() { return; }
    let notifs = session.notifications.clone();
    for n in notifs {
        let color = match n.kind {
            NotificationKind::Ok => egui::Color32::from_rgb(80, 160, 80),
            NotificationKind::Warn => egui::Color32::from_rgb(200, 150, 0),
            NotificationKind::Error => egui::Color32::from_rgb(200, 70, 70),
        };
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0, color))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color, match n.kind {
                        NotificationKind::Ok => "OK",
                        NotificationKind::Warn => "WARN",
                        NotificationKind::Error => "ERROR",
                    });
                    ui.label(&n.message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            *action = FilesAction::DismissNotification(n.id.clone());
                        }
                    });
                });
            });
    }
}
