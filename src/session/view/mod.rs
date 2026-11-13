//! UI rendering and view state for an active remote desktop session.

mod bars;
mod identity;
pub mod screen;
mod signin;

use super::ActiveSession;
use crate::i18n::{t, tf};
use crate::logging;
use crate::session::controller::{Phase, MAX_RECONNECTS, RECONNECT_TIMEOUT};
use crate::theme::Theme;
use egui::{Color32, Rect, RichText, Ui};
use std::time::{Duration, Instant};

pub use screen::screen_scale;
#[cfg(test)]
pub(super) use screen::{remote_button, scale_percent};

/// How long full screen says how to leave it, since the bar is hidden there.
pub(super) const FULLSCREEN_HINT: Duration = Duration::from_secs(5);

const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

pub struct ViewState {
    pub notification_msg: Option<String>,
    /// Give the remote screen the keyboard back after a menu took it.
    pub(super) refocus: bool,
    /// Until when full screen says how to leave it.
    pub(super) fullscreen_hint_until: Option<Instant>,
    /// Where the bar lies while it shows over a full-screen session.
    pub(super) bar_rect: Option<Rect>,
}

impl ViewState {
    pub(super) fn new() -> Self {
        Self {
            notification_msg: None,
            refocus: false,
            fullscreen_hint_until: None,
            bar_rect: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.bar_rect = None;
    }
}

pub(super) fn render(
    session: &mut ActiveSession,
    ui: &mut Ui,
    theme: &Theme,
    on_disconnect: &mut bool,
) {
    session.poll_keyring();
    session.poll_engine();

    let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
    bars::render_bar(session, ui, theme, fullscreen, on_disconnect);

    if let Some(app) = session.open_app.clone() {
        message(
            ui,
            &tf("\"{}\" would open here", &[&app]),
            t("Apps published by workspaces are not available yet."),
        );
        return;
    }

    if !session.controller.is_connected() {
        session.input.key_grab = None;
        session.input.wayland_keys_rx = None;
        session.input.key_grab_tried = false;
    } else if !session.input.key_grab_tried {
        session.input.key_grab_tried = true;
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ui.ctx().clone();
        let wakeup = Box::new(move || ctx.request_repaint());
        session.input.key_grab = session.ports.keys.start(tx, wakeup);
        if session.input.key_grab.is_some() {
            session.input.wayland_keys_rx = Some(rx);
        }
    }
    if let Some(grab) = &session.input.key_grab {
        grab.set_fullscreen(fullscreen);
    }

    if let Phase::Reconnecting {
        attempt,
        at: Some(at),
    } = session.controller.phase
    {
        if Instant::now() >= at {
            session.controller.phase = Phase::Reconnecting { attempt, at: None };
            session.start(RECONNECT_TIMEOUT);
        }
    }

    match session.controller.phase {
        Phase::SignIn => signin::sign_in_form(session, ui, theme),
        Phase::Connecting => {
            session.measure_canvas(ui);
            message(
                ui,
                &tf("Connecting to {}…", &[&session.bookmark.pc_name]),
                t("Checking the PC's identity before signing in."),
            );
        }
        Phase::VerifyIdentity { .. } => identity::identity_question(session, ui, theme),
        Phase::Connected => screen::remote_screens(session, ui),
        Phase::Reconnecting { attempt, .. } => reconnecting(session, ui, attempt),
        Phase::Ended(ref reason) => {
            let text = crate::i18n::end_reason_text(reason);
            ended(session, ui, theme, &text);
        }
    }

    if matches!(
        session.controller.phase,
        Phase::Connecting | Phase::Connected | Phase::Reconnecting { .. }
    ) || session.signin.keyring.is_some()
    {
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}

fn reconnecting(session: &mut ActiveSession, ui: &mut Ui, attempt: u32) {
    session.measure_canvas(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(110.0);
        ui.label(
            RichText::new(tf("Reconnecting to {}…", &[&session.bookmark.pc_name]))
                .font(crate::fonts::medium(18.0)),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(tf(
                "The connection dropped. Attempt {} of {}: your session on the PC normally \
                 stays open and comes back as you left it.",
                &[&attempt, &MAX_RECONNECTS],
            ))
            .weak(),
        );
        ui.add_space(18.0);
        if ui
            .add_sized([180.0, 34.0], egui::Button::new(t("Stop reconnecting")))
            .clicked()
        {
            logging::event("Stopped reconnecting");
            session.reset_session();
            session.forget_session();
            session.controller.phase = Phase::Ended(crate::session::EndReason::StoppedReconnecting);
        }
    });
}

fn message(ui: &mut Ui, title: &str, detail: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(110.0);
        ui.label(RichText::new(title).font(crate::fonts::medium(18.0)));
        ui.add_space(6.0);
        ui.label(RichText::new(detail).weak());
    });
}

fn ended(session: &mut ActiveSession, ui: &mut Ui, theme: &Theme, reason: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(90.0);
        ui.set_max_width(620.0);
        ui.label(
            RichText::new(t("The session ended"))
                .font(crate::fonts::medium(20.0))
                .color(theme.text_primary),
        );
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(reason).color(theme.text_secondary)).wrap());
        ui.add_space(18.0);
        if ui
            .add_sized([180.0, 34.0], theme.primary_button(t("Try again")))
            .clicked()
        {
            logging::event("Try again");
            session.signin.sign_in_error = None;
            session.controller.phase = Phase::SignIn;
        }
    });
}
