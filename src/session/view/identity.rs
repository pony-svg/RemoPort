//! Interactive server certificate verification view.

use super::DANGER;
use crate::i18n::{t, tf};
use crate::logging;
use crate::session::controller::Phase;
use crate::session::ActiveSession;
use crate::theme::Theme;
use egui::{Color32, RichText, Rounding, Ui};
use remoport_rdp::IdentityProblem;

pub(super) fn identity_question(session: &mut ActiveSession, ui: &mut Ui, theme: &Theme) {
    let Phase::VerifyIdentity {
        info,
        problem,
        replacement_confirmed,
        ..
    } = &mut session.controller.phase
    else {
        return;
    };
    let changed = matches!(problem, IdentityProblem::Changed { .. });
    let mut decision = None;
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.set_max_width(620.0);
        let (title, color) = if changed {
            (t("Warning: this PC's certificate changed"), DANGER)
        } else {
            (
                t("Remoport cannot confirm this PC's identity"),
                theme.text_primary,
            )
        };
        ui.label(
            RichText::new(title)
                .font(crate::fonts::medium(21.0))
                .color(color),
        );
        ui.add_space(10.0);
        let explanation = if changed {
            tf(
                "{} presents a different certificate from the one you trusted. Someone may \
                 be intercepting the connection to capture your password. Continue only if \
                 your administrator confirms the certificate was replaced.",
                &[&info.host],
            )
        } else {
            tf(
                "Remoport could not verify {}: {}. If this is your PC, compare the \
                 fingerprint below with the one your administrator gives you before you \
                 trust it. Nothing has been sent to it yet.",
                &[&info.host, &info.why_unverified],
            )
        };
        ui.label(RichText::new(explanation).color(theme.text_secondary));
        ui.add_space(14.0);

        egui::Frame::none()
            .fill(theme.card_bg)
            .stroke(theme.card_stroke())
            .rounding(Rounding::same(8.0))
            .inner_margin(egui::Margin::same(14.0))
            .show(ui, |ui| {
                egui::Grid::new("certificate")
                    .num_columns(2)
                    .spacing([14.0, 8.0])
                    .show(ui, |ui| {
                        let row = |ui: &mut Ui, label: &str, value: &str, mono: bool| {
                            ui.label(RichText::new(label).color(theme.text_secondary));
                            let text = RichText::new(value).color(theme.text_primary);
                            ui.add(
                                egui::Label::new(if mono { text.monospace() } else { text }).wrap(),
                            );
                            ui.end_row();
                        };
                        row(ui, "PC", &format!("{}:{}", info.host, info.port), false);
                        row(ui, t("Certificate for"), &info.subject, false);
                        row(ui, t("Issued by"), &info.issuer, false);
                        row(
                            ui,
                            t("Valid"),
                            &tf("{} to {}", &[&info.valid_from, &info.valid_until]),
                            false,
                        );
                        if let IdentityProblem::Changed { previous_sha256 } = &problem {
                            row(ui, t("You trusted (SHA-256)"), previous_sha256, true);
                            row(ui, t("Now presented"), &info.sha256, true);
                        } else {
                            row(ui, t("Fingerprint (SHA-256)"), &info.sha256, true);
                        }
                    });
            });

        ui.add_space(16.0);
        if changed {
            ui.checkbox(
                replacement_confirmed,
                t("My administrator confirmed this certificate was replaced"),
            );
            ui.add_space(10.0);
        }
        ui.horizontal(|ui| {
            let width = 2.0 * 220.0 + ui.spacing().item_spacing.x;
            ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
            if ui
                .add_sized([220.0, 34.0], theme.primary_button(t("Don't connect")))
                .clicked()
            {
                decision = Some(false);
            }
            let trust = if changed {
                egui::Button::new(
                    RichText::new(t("Trust the new certificate")).color(Color32::WHITE),
                )
                .fill(DANGER)
            } else {
                egui::Button::new(t("Trust and connect"))
            };
            let enabled = !changed || *replacement_confirmed;
            if ui
                .add_enabled_ui(enabled, |ui| ui.add_sized([220.0, 34.0], trust))
                .inner
                .clicked()
            {
                decision = Some(true);
            }
        });
    });

    if let Some(trust) = decision {
        let (host, sha) = (info.host.clone(), info.sha256.clone());
        if let Phase::VerifyIdentity { reply, .. } = &mut session.controller.phase {
            if let Some(reply) = reply.take() {
                reply.answer(trust);
            }
        }
        logging::event(format_args!(
            "{} the certificate of {host} ({sha})",
            if trust { "You trusted" } else { "You declined" }
        ));
        session.controller.phase = Phase::Connecting;
    }
}
