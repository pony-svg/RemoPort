//! Sign-in form rendered before connecting.

use super::DANGER;
use crate::i18n::{t, tf};
use crate::session::ActiveSession;
use crate::theme::Theme;
use egui::{RichText, Ui};

pub(super) fn sign_in_form(session: &mut ActiveSession, ui: &mut Ui, theme: &Theme) {
    session.measure_canvas(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(70.0);
        ui.label(
            RichText::new(tf("Sign in to {}", &[&session.bookmark.display_title()]))
                .font(crate::fonts::medium(22.0))
                .color(theme.text_primary),
        );
        ui.label(RichText::new(&session.bookmark.pc_name).color(theme.text_secondary));
        ui.add_space(20.0);

        let mut submit = false;
        let mut forget = false;
        // A grid does not center itself: pad it to the middle.
        const FORM_WIDTH: f32 = 380.0;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - FORM_WIDTH) / 2.0).max(0.0));
            egui::Grid::new("sign_in")
                .num_columns(2)
                .spacing([12.0, 10.0])
                .show(ui, |ui| {
                    ui.label(t("User name"));
                    ui.add(
                        egui::TextEdit::singleline(&mut session.signin.username)
                            .hint_text(t("user, DOMAIN\\user or user@domain"))
                            .desired_width(280.0),
                    );
                    ui.end_row();
                    ui.label(t("Password"));
                    if session.signin.saved_password.is_some() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t("🔑 Saved in your keyring"))
                                    .color(theme.text_secondary),
                            );
                            forget = ui.small_button(t("Forget")).clicked();
                        });
                    } else {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut *session.signin.password)
                                .password(true)
                                .desired_width(280.0),
                        );
                        submit |=
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    ui.end_row();
                    if session.saved && session.signin.saved_password.is_none() {
                        ui.label("");
                        ui.checkbox(
                            &mut session.signin.remember,
                            t("Remember in the system keyring"),
                        );
                        ui.end_row();
                    }
                    if session.gateway_account().is_some() {
                        ui.label(t("Gateway password"));
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut *session.signin.gateway_password)
                                .password(true)
                                .hint_text(session.bookmark.gateway_username.trim().to_string())
                                .desired_width(280.0),
                        );
                        submit |=
                            field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.end_row();
                    }
                });
        });
        if forget {
            session.signin.forget_saved(session.bookmark.id);
        }

        if let Some(error) = &session.signin.sign_in_error {
            ui.add_space(8.0);
            ui.colored_label(DANGER, error);
        }
        ui.add_space(16.0);
        submit |= ui
            .add_sized([200.0, 34.0], theme.primary_button(t("Connect")))
            .clicked();
        if submit {
            session.connect();
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(t(
                "Remoport confirms the PC's identity before your password is used.",
            ))
            .size(12.0)
            .color(theme.text_secondary),
        );
    });
}
