//! Remote screen rendering: multi-window orchestration, placement, painting, and input collection.

use super::FULLSCREEN_HINT;
use crate::ui::i18n::{t, tf};
use crate::ui::keymap;
use crate::ui::session::screen::PointerLook;
use crate::ui::session::{strokes_for, ActiveSession};
use egui::{
    pos2, vec2, Align2, Color32, Event, FontId, Modifiers, PointerButton, Rect, Response, Rounding,
    Sense, Ui, Vec2,
};
use remoport_core::logging;
use remoport_core::session::display::{
    fits_window, picture_rect, pointer_rect, remote_point, Placement,
};
use remoport_rdp::{ClipboardText, Command, MouseButton, Rect as RemoteRect};
use std::time::Instant;

struct ScreenPlacement {
    rect: Rect,
    image: Rect,
    region: RemoteRect,
    scale: f32,
    per_point: f32,
    uv: Rect,
    response: Response,
}

/// The remote desktop: the main window shows the first screen, and each
/// further screen gets a window of its own.
pub(super) fn remote_screens(session: &mut ActiveSession, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let mut focus = remote_screen(session, ui, 0);
    for index in 1..session.display.screens {
        let title = tf(
            "{} · screen {}",
            &[&session.bookmark.display_title(), &(index + 1)],
        );
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([960.0, 600.0]);
        let id = egui::ViewportId::from_hash_of(("remoport-screen", index));
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            // The session ends from the main window; a screen's stays.
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    if let Some(found) = remote_screen(session, ui, index) {
                        focus = Some(found);
                    }
                });
        });
    }
    let has_physical_keys = session.input.key_grab.is_some();
    if !has_physical_keys {
        session.sync_modifiers(focus.map_or(Modifiers::NONE, |(modifiers, _)| modifiers));
    }
    // The session lost the keyboard: release all held keys.
    if focus.is_none() {
        if has_physical_keys {
            let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            let actions = session.input.state.handle_leave();
            session.execute_input_actions(actions, &ctx, fullscreen);
        } else {
            for key in std::mem::take(&mut session.input.fallback_keys_down) {
                session.send_key(key, false);
            }
        }
    }
    if has_physical_keys {
        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        session.input.state.set_fullscreen(fullscreen);
        session
            .input
            .state
            .set_share_clipboard(session.bookmark.share_clipboard);
        session.poll_physical_keys(&ctx, focus.is_some(), fullscreen);
        let tick_actions = session.input.state.tick(Instant::now());
        session.execute_input_actions(tick_actions, &ctx, fullscreen);
    }
    if let Some(text) = session.media.remote_copy.take() {
        // Only while the user works in the session: a PC copying on its
        // own does not get to replace this computer's clipboard.
        if focus.is_some_and(|(_, window_focused)| window_focused) {
            logging::event(format_args!(
                "Copied from the PC: {} characters",
                text.as_str().chars().count()
            ));
            ctx.output_mut(|o| o.copied_text = text.as_str().to_string());
        } else {
            logging::event("The PC's clipboard was not taken: the session was not in use");
        }
    }
    if session.display.screens > 1 {
        session.follow_layout();
    } else {
        session.follow_canvas();
    }
}

/// One window's part of the remote desktop, with its input. Returns the
/// modifiers held and whether the window has the system's focus, when
/// this window has the keyboard.
fn remote_screen(
    session: &mut ActiveSession,
    ui: &mut Ui,
    index: usize,
) -> Option<(Modifiers, bool)> {
    let place = place_screen(session, ui, index)?;
    let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
    let focused = paint_screen(session, ui, index, fullscreen, &place);
    collect_screen_input(session, ui, index, fullscreen, &place, focused)
}

fn place_screen(session: &mut ActiveSession, ui: &mut Ui, index: usize) -> Option<ScreenPlacement> {
    let px = session.measured_px(ui);
    if index == 0 {
        session.display.canvas_px = px;
        // ponytail: one scale for the whole session, the main window's;
        // screens with different scales would need one per screen here.
        session.display.screen_scale = screen_scale(ui.ctx());
    }
    if let Some(slot) = session.display.screen_px.get_mut(index) {
        *slot = px;
    }

    let per_point = session.desktop_pixels_per_point(ui.ctx());
    let region = session.region(index);
    // The picture at its own size in this window.
    let size = region.map(|r| vec2(f32::from(r.width), f32::from(r.height)) / per_point);
    let room = ui.available_size();
    let (rect, scrolled) =
        match size.filter(|size| !session.bookmark.fit_to_window && !fits_window(*size, room)) {
            Some(size) => scroll_bars(ui, index, size),
            None => (
                ui.allocate_exact_size(ui.available_size(), Sense::hover())
                    .0,
                Vec2::ZERO,
            ),
        };
    // An id of its own: an automatic one depends on what came before in the
    // window, which changes as the bar hides in full screen, and with it
    // the screen would lose the keyboard on every switch.
    let response = ui.interact(
        rect,
        egui::Id::new(("session-screen", index)),
        Sense::click_and_drag(),
    );
    ui.painter()
        .rect_filled(rect, Rounding::ZERO, Color32::BLACK);
    let (region, size) = (region?, size?);
    let screen = session.screen.screen.as_ref()?;
    let (desktop_width, desktop_height) = (screen.width as f32, screen.height as f32);
    let (width, height) = (f32::from(region.width), f32::from(region.height));
    let placement = if !session.bookmark.fit_to_window {
        Placement::Scrolled(scrolled)
    } else if session.bookmark.resolution.fixed_size().is_some() {
        Placement::Fitted
    } else {
        Placement::Following
    };
    let image = picture_rect(rect, size, placement);
    let scale = image.width() / size.x;
    let uv = Rect::from_min_max(
        pos2(
            f32::from(region.x) / desktop_width,
            f32::from(region.y) / desktop_height,
        ),
        pos2(
            (f32::from(region.x) + width) / desktop_width,
            (f32::from(region.y) + height) / desktop_height,
        ),
    );
    Some(ScreenPlacement {
        rect,
        image,
        region,
        scale,
        per_point,
        uv,
        response,
    })
}

fn paint_screen(
    session: &mut ActiveSession,
    ui: &mut Ui,
    index: usize,
    fullscreen: bool,
    place: &ScreenPlacement,
) -> bool {
    let ctx = ui.ctx().clone();
    if let Some(screen) = session.screen.screen.as_mut() {
        let texture = screen.texture(&ctx).id();
        ui.painter_at(place.rect)
            .image(texture, place.image, place.uv, Color32::WHITE);
    }

    if fullscreen
        && session
            .view
            .fullscreen_hint_until
            .is_some_and(|until| Instant::now() < until)
    {
        // On a dark plate: the remote desktop can be any color.
        let painter = ui.painter();
        let text = painter.layout_no_wrap(
            t("Ctrl+Alt+Enter leaves full screen; the bar shows at the top edge").to_string(),
            FontId::proportional(14.0),
            Color32::WHITE,
        );
        let at =
            Align2::CENTER_TOP.anchor_size(place.rect.center_top() + vec2(0.0, 16.0), text.size());
        painter.rect_filled(
            at.expand(8.0),
            Rounding::same(6.0),
            Color32::from_black_alpha(190),
        );
        painter.galley(at.min, text, Color32::WHITE);
    }
    if index > 0 && !fullscreen {
        ui.painter().text(
            place.rect.center_top() + vec2(0.0, 12.0),
            Align2::CENTER_TOP,
            t("Move this window to its screen, then press Ctrl+Alt+Enter"),
            FontId::proportional(13.0),
            Color32::from_white_alpha(210),
        );
    }
    if place.response.clicked()
        || place.response.drag_started()
        || (index == 0 && std::mem::take(&mut session.view.refocus))
    {
        place.response.request_focus();
    }
    let focused = place.response.has_focus();
    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                place.response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            );
        });
    } else if place.response.hovered() {
        ui.painter().text(
            place.rect.center_bottom() - vec2(0.0, 16.0),
            Align2::CENTER_BOTTOM,
            t("Click the remote screen to type into it"),
            FontId::proportional(12.0),
            Color32::from_white_alpha(160),
        );
    }
    session.screen.flush_pointer(&ctx);
    // Drawn here at the pointer's place, so it moves with no delay and
    // looks as the PC wants: an I-beam over text, a hand over a link.
    if place.response.hovered() || place.response.dragged() {
        let hover = ctx.input(|i| i.pointer.hover_pos());
        let over_picture =
            hover.filter(|pos| place.rect.contains(*pos) && place.image.contains(*pos));
        match (&session.screen.pointer, over_picture) {
            (PointerLook::Hidden, Some(_)) => ctx.set_cursor_icon(egui::CursorIcon::None),
            (
                PointerLook::Drawn {
                    texture,
                    size,
                    hotspot,
                },
                Some(pos),
            ) => {
                ctx.set_cursor_icon(egui::CursorIcon::None);
                ui.painter().image(
                    texture.id(),
                    pointer_rect(pos, *size, *hotspot, place.scale / place.per_point),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {}
        }
    }
    focused
}

fn collect_screen_input(
    session: &mut ActiveSession,
    ui: &mut Ui,
    index: usize,
    fullscreen: bool,
    place: &ScreenPlacement,
    focused: bool,
) -> Option<(Modifiers, bool)> {
    let ctx = ui.ctx().clone();
    let bar = session.view.bar_rect.filter(|_| index == 0);
    let to_remote = |pos: egui::Pos2| -> Option<(u16, u16)> {
        if !place.rect.contains(pos) || bar.is_some_and(|bar| bar.contains(pos)) {
            return None;
        }
        remote_point(pos, place.image, place.region)
    };

    let has_physical_keys = session.input.key_grab.is_some();
    let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
    for event in events {
        match event {
            Event::PointerMoved(pos) => {
                if let Some(at) =
                    to_remote(pos).filter(|at| session.input.last_pointer != Some(*at))
                {
                    session.input.last_pointer = Some(at);
                    session.send(Command::PointerMove { x: at.0, y: at.1 });
                }
            }
            Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } => {
                // A release outside the picture still goes, so no button sticks.
                let at = to_remote(pos).or(if pressed {
                    None
                } else {
                    session.input.last_pointer
                });
                if let Some((x, y)) = at {
                    session.send(Command::MouseButton {
                        x,
                        y,
                        button: remote_button(button),
                        pressed,
                    });
                }
            }
            Event::MouseWheel { unit, delta, .. } if place.response.hovered() => {
                let step = match unit {
                    egui::MouseWheelUnit::Point => 6.0,
                    egui::MouseWheelUnit::Line => 120.0,
                    egui::MouseWheelUnit::Page => 360.0,
                };
                let vertical = to_i16(delta.y * step);
                let horizontal = to_i16(-delta.x * step);
                if vertical != 0 || horizontal != 0 {
                    session.send(Command::Wheel {
                        vertical,
                        horizontal,
                    });
                }
            }
            // Ctrl+Alt+Enter is Remoport's: full screen on and off.
            Event::Key {
                key: egui::Key::Enter,
                pressed,
                modifiers,
                ..
            } if focused && modifiers.ctrl && modifiers.alt => {
                if !has_physical_keys && pressed {
                    if !fullscreen {
                        session.view.fullscreen_hint_until = Some(Instant::now() + FULLSCREEN_HINT);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    logging::event(if fullscreen {
                        "Left full screen"
                    } else {
                        "Full screen"
                    });
                }
            }
            Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } if focused => {
                if !has_physical_keys {
                    // Modifiers first: a key and its Ctrl can land in one frame.
                    session.sync_modifiers(modifiers);
                    // Reserved combinations only: what is typed is never logged.
                    if pressed && !repeat && modifiers.alt {
                        match physical_key.unwrap_or(key) {
                            egui::Key::Tab => logging::event("Sent Alt+Tab"),
                            egui::Key::F4 => logging::event("Sent Alt+F4"),
                            _ => {}
                        }
                    }
                    if let Some(code) = keymap::scancode(physical_key.unwrap_or(key)) {
                        let strokes = strokes_for(
                            &mut session.input.fallback_keys_down,
                            code,
                            pressed,
                            modifiers.ctrl,
                        );
                        for (key, down) in strokes {
                            session.send_key(key, down);
                        }
                    }
                }
            }
            // egui turns these shortcuts into clipboard commands and
            // swallows the key press; the release still arrives as a key.
            Event::Copy if focused => {
                if !has_physical_keys {
                    session.input.fallback_keys_down.insert(keymap::C);
                    session.send_key(keymap::C, true);
                }
            }
            Event::Cut if focused => {
                if !has_physical_keys {
                    session.input.fallback_keys_down.insert(keymap::X);
                    session.send_key(keymap::X, true);
                }
            }
            Event::Paste(text) if focused => {
                let text = ClipboardText::new(text);
                if has_physical_keys {
                    let actions = session.input.state.handle_egui_paste(text, Instant::now());
                    session.execute_input_actions(actions, &ctx, fullscreen);
                } else {
                    session.input.fallback_keys_down.insert(keymap::V);
                    if session.bookmark.share_clipboard {
                        logging::event(format_args!(
                            "Pasted into the PC: {} characters",
                            text.as_str().chars().count()
                        ));
                        session.send(Command::Paste(text));
                    } else {
                        session.send_key(keymap::V, true);
                    }
                }
            }
            _ => {}
        }
    }
    focused.then(|| (modifiers, ctx.input(|i| i.focused)))
}

/// This computer's screen scale as a percentage, whatever the app's zoom.
pub fn screen_scale(ctx: &egui::Context) -> u32 {
    scale_percent(ctx.native_pixels_per_point().unwrap_or(1.0))
}

pub(in crate::ui::session) fn scale_percent(pixels_per_point: f32) -> u32 {
    (pixels_per_point * 100.0).round() as u32
}

/// Scroll bars for a picture that does not fit the window. Returns the space
/// left for the picture and how far it is scrolled. The wheel stays the PC's.
fn scroll_bars(ui: &mut Ui, index: usize, size: Vec2) -> (Rect, Vec2) {
    ui.ctx().input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    // Beside the picture rather than over it, where a click belongs to the PC.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    let output = egui::ScrollArea::both()
        .id_source(("session-scroll", index))
        .auto_shrink(false)
        .drag_to_scroll(false)
        .show(ui, |ui| {
            ui.allocate_space(size);
        });
    (output.inner_rect, output.state.offset)
}

pub(in crate::ui::session) fn remote_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::X1,
        PointerButton::Extra2 => MouseButton::X2,
    }
}

/// Rounds and clamps to ±240, two wheel notches, so the cast cannot wrap.
fn to_i16(value: f32) -> i16 {
    value.round().clamp(-240.0, 240.0) as i16
}
