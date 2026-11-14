//! Settings page: appearance, default resolution, data paths, trusted certificates, and about links.

use crate::ui::app::{RemoportApp, FEEDBACK_URL};
use crate::ui::fonts;
use crate::ui::i18n::{resolution_label, t, tf};
use crate::ui::theme::Theme;
use crate::ui::widgets;
use egui::{RichText, Ui};
use remoport_core::domain::{Appearance, ResolutionOption};
use remoport_core::logging;
use remoport_core::store::{state_path, trust_store_path};

const REPO_URL: &str = "https://github.com/pony-svg/RemoPort";

pub(crate) fn show(app: &mut RemoportApp, ui: &mut Ui) {
    settings_heading(app, ui, t("Appearance"));
    let mut appearance = app.theme.appearance;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (option, label) in [
            (Appearance::Light, t("Light")),
            (Appearance::Dark, t("Dark")),
        ] {
            if widgets::chip(&app.theme, ui, label, appearance == option, false).clicked() {
                appearance = option;
            }
        }
    });
    if appearance != app.theme.appearance {
        logging::event(format_args!("Appearance: {appearance:?}"));
        app.state.appearance = appearance;
        app.theme = Theme::for_appearance(appearance);
        app.theme_applied = false;
    }

    ui.add_space(26.0);
    settings_heading(app, ui, t("New PCs"));
    let resolution = app.state.default_resolution;
    egui::ComboBox::from_label(
        RichText::new(t("Resolution for PCs you add")).color(app.theme.text_secondary),
    )
    .selected_text(resolution_label(&app.state.default_resolution))
    .show_ui(ui, |ui| {
        for option in [
            ResolutionOption::Native,
            ResolutionOption::Fhd1080p,
            ResolutionOption::Qhd1440p,
            ResolutionOption::Uhd4k,
        ] {
            ui.selectable_value(
                &mut app.state.default_resolution,
                option,
                resolution_label(&option),
            );
        }
    });
    if app.state.default_resolution != resolution {
        logging::event(format_args!(
            "Resolution for new PCs: {}",
            resolution_label(&app.state.default_resolution)
        ));
    }

    ui.add_space(26.0);
    settings_heading(app, ui, t("Your data"));
    let location = state_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| t("nowhere (no home folder)").to_string());
    ui.label(
        RichText::new(tf("Your PCs and settings are saved in {}", &[&location]))
            .color(app.theme.text_secondary),
    );
    let log = logging::log_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| t("standard error only").to_string());
    ui.label(
        RichText::new(tf("What you do in Remoport is logged to {}", &[&log]))
            .color(app.theme.text_secondary),
    );

    ui.add_space(26.0);
    show_security_settings(app, ui);

    ui.add_space(26.0);
    settings_heading(app, ui, t("About Remoport"));
    for line in [
        format!("Remoport {}", env!("CARGO_PKG_VERSION")),
        t("Your remote computers and apps, from Linux.").to_string(),
        t("Free software under the GNU GPL, version 3 or later.").to_string(),
        t("Its RDP engine is built on IronRDP, with parts from RustConn by Anton Isaiev.")
            .to_string(),
    ] {
        ui.label(RichText::new(line).color(app.theme.text_secondary));
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.hyperlink_to(t("Source code"), REPO_URL).clicked() {
            logging::event("Opened the source code page");
        }
        ui.label(RichText::new("·").color(app.theme.text_secondary));
        if ui
            .hyperlink_to(t("Report a problem"), FEEDBACK_URL)
            .clicked()
        {
            logging::event("Opened the feedback page");
        }
    });
}

fn settings_heading(app: &RemoportApp, ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(fonts::medium(16.0))
            .color(app.theme.text_primary),
    );
    ui.add_space(8.0);
}

/// How Remoport protects a connection, and the certificates the user trusted.
fn show_security_settings(app: &RemoportApp, ui: &mut Ui) {
    settings_heading(app, ui, t("Security"));
    for line in [
        t(
            "Before your password is used, Remoport confirms the PC's identity: a certificate \
         from an authority this computer trusts, or one you checked and trusted yourself.",
        ),
        t(
            "Network Level Authentication is required, unless you allow a PC to sign in without \
         it in its settings.",
        ),
        t("Passwords are remembered only if you ask, and only in the system keyring."),
    ] {
        ui.label(RichText::new(line).color(app.theme.text_secondary));
    }
    ui.add_space(10.0);
    let Some(store) = trust_store_path().map(remoport_rdp::TrustStore::new) else {
        return;
    };
    let servers = store.entries();
    if servers.is_empty() {
        ui.label(
            RichText::new(t("You have not trusted any PC's certificate yourself."))
                .color(app.theme.text_secondary),
        );
        return;
    }
    ui.label(RichText::new(t("Certificates you trusted")).color(app.theme.text_primary));
    ui.add_space(4.0);
    let mut forget = None;
    egui::Grid::new("trusted_servers")
        .striped(true)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for server in &servers {
                ui.label(RichText::new(&server.server).color(app.theme.text_primary));
                ui.label(
                    RichText::new(server.sha256.get(..23).unwrap_or(&server.sha256))
                        .monospace()
                        .color(app.theme.text_secondary),
                )
                .on_hover_text(format!("{}\nSHA-256 {}", server.subject, server.sha256));
                ui.label(
                    RichText::new(server.trusted_at.get(..10).unwrap_or(&server.trusted_at))
                        .color(app.theme.text_secondary),
                );
                if ui
                    .button(t("Forget"))
                    .on_hover_text(t("The next connection asks again"))
                    .clicked()
                {
                    forget = Some(server.server.clone());
                }
                ui.end_row();
            }
        });
    if let Some(server) = forget {
        match store.forget(&server) {
            Ok(()) => logging::event(format_args!("Stopped trusting the certificate of {server}")),
            Err(err) => logging::event(format_args!("Could not forget {server}: {err}")),
        }
    }
}
