//! Top bar and floating full-screen bar for the active session.

use super::{DANGER, FULLSCREEN_HINT};
use crate::ui::i18n::{t, tf};
use crate::ui::keymap;
use crate::ui::session::ActiveSession;
use crate::ui::theme::Theme;
use egui::{pos2, Context, RichText, Ui};
use remoport_core::logging;
use remoport_core::session::controller::{Phase, MAX_RECONNECTS};
use remoport_rdp::Command;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// How near the top edge the pointer brings up the full-screen bar, in points.
const BAR_EDGE: f32 = 4.0;

pub(super) fn render_bar(
    session: &mut ActiveSession,
    ui: &mut Ui,
    theme: &Theme,
    fullscreen: bool,
    on_disconnect: &mut bool,
) {
    // In full screen the PC gets the whole screen, and the bar shows over
    // it when the pointer reaches the top edge.
    if fullscreen && session.controller.is_connected() {
        floating_bar(session, &ui.ctx().clone(), on_disconnect);
    } else {
        session.view.bar_rect = None;
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(12.0, 6.0))
            .show(ui, |ui| top_bar(session, ui, on_disconnect));
        ui.separator();
        if let Some(ref msg) = session.view.notification_msg {
            ui.colored_label(theme.accent, format!("ℹ {msg}"));
        }
    }
}

/// The bar over a full-screen session: it shows when the pointer reaches
/// the top edge, and stays while the pointer or one of its menus is there.
/// It floats so that showing it does not resize the remote desktop.
fn floating_bar(session: &mut ActiveSession, ctx: &Context, on_disconnect: &mut bool) {
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let at_edge = pointer.is_some_and(|p| p.y <= BAR_EDGE);
    let on_bar = session
        .view
        .bar_rect
        .zip(pointer)
        .is_some_and(|(bar, p)| bar.expand(BAR_EDGE).contains(p));
    if !(at_edge || on_bar || ctx.memory(egui::Memory::any_popup_open)) {
        session.view.bar_rect = None;
        return;
    }
    let width = ctx.screen_rect().width();
    let shown = egui::Area::new(egui::Id::new("session-bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(ctx.style().visuals.panel_fill)
                .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    top_bar(session, ui, on_disconnect);
                });
        });
    session.view.bar_rect = Some(shown.response.rect);
}

fn top_bar(session: &mut ActiveSession, ui: &mut Ui, on_disconnect: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(format!("🖥 {}", session.bookmark.display_title()));
        ui.separator();
        ui.label(tf("Host: {}", &[&session.bookmark.pc_name]));
        ui.separator();
        ui.label(status(session));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let connected = session.controller.is_connected();
            if ui
                .button(if connected {
                    t("❌ Disconnect")
                } else {
                    t("❌ Close")
                })
                .clicked()
            {
                logging::event(format_args!(
                    "{} {}",
                    if connected {
                        "Disconnected from"
                    } else {
                        "Closed the session of"
                    },
                    session.bookmark.pc_name
                ));
                *on_disconnect = true;
            }
            if connected && session.media.microphone_live {
                let muted = session.media.microphone_muted.load(Ordering::Relaxed);
                let label = if muted {
                    RichText::new(t("Microphone muted"))
                } else {
                    RichText::new(t("Microphone on")).color(DANGER)
                };
                if ui
                    .button(label)
                    .on_hover_text(if muted {
                        t("The PC gets silence. Click to unmute.")
                    } else {
                        t("The PC is using your microphone. Click to mute.")
                    })
                    .clicked()
                {
                    session
                        .media
                        .microphone_muted
                        .store(!muted, Ordering::Relaxed);
                    logging::event(if muted {
                        "Microphone unmuted"
                    } else {
                        "Microphone muted"
                    });
                }
            }
            let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
            if connected
                && ui
                    .button(if fullscreen {
                        t("⛶ Leave full screen")
                    } else {
                        t("⛶ Full screen")
                    })
                    .on_hover_text(t("Ctrl+Alt+Enter switches full screen on and off"))
                    .clicked()
            {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                logging::event(if fullscreen {
                    "Left full screen"
                } else {
                    "Full screen"
                });
                if !fullscreen {
                    session.view.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
                }
                session.view.refocus = true;
            }
            if connected {
                ui.menu_button(t("⌨ Keys"), |ui| {
                    if ui.button("Ctrl+Alt+Del").clicked() {
                        logging::event("Sent Ctrl+Alt+Del");
                        session.send(Command::CtrlAltDel);
                        session.view.refocus = true;
                        ui.close_menu();
                    }
                    for (label, keys) in keymap::COMBINATIONS {
                        if ui.button(t(label)).clicked() {
                            logging::event(format_args!("Sent {label}"));
                            for (key, pressed) in keymap::strokes(keys) {
                                session.send_key(key, pressed);
                            }
                            session.view.refocus = true;
                            ui.close_menu();
                        }
                    }
                })
                .response
                .on_hover_text(t("Key combinations this computer keeps for itself, \
                     or that cannot be typed here"));
            }
        });
    });
}

fn status(session: &ActiveSession) -> String {
    match session.controller.phase {
        Phase::SignIn => t("Sign in").to_string(),
        Phase::Connecting => t("Connecting…").to_string(),
        Phase::VerifyIdentity { .. } => t("Waiting for your decision").to_string(),
        Phase::Connected => {
            let size = session
                .screen
                .screen
                .as_ref()
                .map(|s| format!(" · {}×{}", s.width, s.height))
                .unwrap_or_default();
            let graphics = session
                .screen
                .graphics
                .as_ref()
                .map(|g| format!(" · {g}"))
                .unwrap_or_default();
            let screens = if session.display.screens > 1 {
                tf(" · {} screens", &[&session.display.screens])
            } else {
                String::new()
            };
            format!("{}{size}{screens}{graphics}", t("Connected"))
        }
        Phase::Reconnecting { attempt, .. } => {
            tf("Reconnecting… ({} of {})", &[&attempt, &MAX_RECONNECTS])
        }
        Phase::Ended(_) => t("Disconnected").to_string(),
    }
}
