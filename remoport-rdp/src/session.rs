//! The active session: server updates out as frames, input in as commands.
//! The loop, the reactivation sequence and the region helpers follow
//! RustConn's client (`rustconn-core`, GPL-3.0-or-later); the clipboard's
//! handling is Remoport's.

use std::sync::mpsc;
use std::time::Duration;

use ironrdp_connector::connection_activation::{
    ConnectionActivationFactory, ConnectionActivationState,
};
use ironrdp_displaycontrol::pdu::{DisplayControlMonitorLayout, MonitorLayoutEntry};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_input::{Database, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::WriteBuf;
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::{FramedRead, FramedWrite, single_sequence_step_read, split_tokio_framed};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use tokio::time::Instant;

use crate::clipboard::Clipboard;
use crate::connect::Established;
use crate::{Command, Error, Event, MouseButton, Rect, ResumeCookie};

/// The V key, pressed for a paste.
const V: u8 = 0x2F;

pub(crate) async fn run(
    established: Established,
    events: &mpsc::Sender<Event>,
    mut commands: tokio_mpsc::UnboundedReceiver<Command>,
    mut cancelled: watch::Receiver<bool>,
) -> Result<(), Error> {
    let Established {
        framed,
        result,
        mut clipboard,
        mut microphone,
        drives,
    } = established;
    // How often shared folders' changes are sent to the PC.
    let mut folder_changes = tokio::time::interval(Duration::from_millis(500));
    folder_changes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut reader, mut writer) = split_tokio_framed(framed);
    let mut image = DecodedImage::new(
        PixelFormat::BgrA32,
        result.desktop_size.width,
        result.desktop_size.height,
    );
    let activation = result.activation_factory;
    let message_channel_id = result.message_channel_id;
    let mut stage = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();
    let mut input = Database::new();
    let _ = events.send(Event::Connected {
        width: image.width(),
        height: image.height(),
    });

    let lost = |e: std::io::Error| Error::ConnectionLost(e.to_string());
    loop {
        let holding = clipboard.as_ref().is_some_and(Clipboard::holding);
        let release_at = clipboard
            .as_ref()
            .and_then(Clipboard::deadline)
            .map_or_else(
                || Instant::now() + Duration::from_secs(3600),
                Instant::from_std,
            );
        tokio::select! {
            _ = cancelled.changed() => {
                if let Ok(frames) = stage.graceful_shutdown() {
                    for frame in frames {
                        if let ActiveStageOutput::ResponseFrame(data) = frame {
                            let _ = writer.write_all(&data).await;
                        }
                    }
                }
                return Ok(());
            }
            () = tokio::time::sleep_until(release_at), if holding => {
                // The PC did not answer the paste in time: the keys go anyway.
                for command in clipboard.as_mut().map(Clipboard::release).unwrap_or_default() {
                    apply_command(command, &mut stage, &mut image, &mut writer, &mut input, events).await;
                }
            }
            _ = folder_changes.tick(), if drives => {
                if let Some(frame) = folder_change_frame(&mut stage) {
                    writer.write_all(&frame).await.map_err(lost)?;
                }
            }
            Some(messages) = next_microphone_packet(&mut microphone) => {
                // A packet that cannot be encoded is a gap in the sound.
                if let Ok(frame) = stage.encode_dvc_messages(messages) {
                    writer.write_all(&frame).await.map_err(lost)?;
                }
            }
            Some(command) = commands.recv() => {
                match (&mut clipboard, command) {
                    // Input keeps its order behind a paste the PC has not taken yet.
                    (Some(clip), command) if clip.holding() => clip.hold(command),
                    (Some(clip), Command::Paste(text)) => {
                        if let Some(frame) = clip.paste(text, &mut stage, events) {
                            writer.write_all(&frame).await.map_err(lost)?;
                        }
                        let press = Command::Key { scancode: V, extended: false, pressed: true };
                        if clip.holding() {
                            clip.hold(press);
                        } else {
                            apply_command(press, &mut stage, &mut image, &mut writer, &mut input, events).await;
                        }
                    }
                    (_, command) => {
                        apply_command(command, &mut stage, &mut image, &mut writer, &mut input, events).await;
                    }
                }
            }
            read = reader.read_pdu() => {
                let (action, payload) = read.map_err(|e| Error::ConnectionLost(e.to_string()))?;
                match stage.process(&mut image, action, &payload, None) {
                    Ok(outputs) => {
                        for output in outputs {
                            let ended = handle_output(
                                output, &mut writer, &mut reader, events, &mut image,
                                &mut stage, &activation,
                            ).await?;
                            if ended {
                                return Ok(());
                            }
                        }
                    }
                    Err(_) if is_message_channel_pdu(action, &payload, message_channel_id) => {
                        // Auto-detect, heartbeat and similar on the message
                        // channel: not understood, not fatal.
                    }
                    Err(e) => return Err(Error::Protocol(format!("session error: {e}"))),
                }
                if let Some(clip) = &mut clipboard {
                    let outcome = clip.drain(&mut stage, events);
                    for frame in outcome.frames {
                        writer.write_all(&frame).await.map_err(lost)?;
                    }
                    for command in outcome.release {
                        apply_command(command, &mut stage, &mut image, &mut writer, &mut input, events).await;
                    }
                }
            }
        }
    }
}

/// Shared folders' changes the PC waits for, ready to send.
fn folder_change_frame(stage: &mut ActiveStage) -> Option<Vec<u8>> {
    let rdpdr = stage.get_svc_processor_mut::<ironrdp_rdpdr::Rdpdr>()?;
    let messages = rdpdr.poll_deferred_messages().ok()?;
    if messages.is_empty() {
        return None;
    }
    stage
        .process_svc_processor_messages(
            ironrdp_svc::SvcProcessorMessages::<ironrdp_rdpdr::Rdpdr>::new(messages),
        )
        .ok()
}

/// The next microphone packet, or never when the PC may not use it.
async fn next_microphone_packet(
    packets: &mut Option<tokio_mpsc::Receiver<Vec<ironrdp_svc::SvcMessage>>>,
) -> Option<Vec<ironrdp_svc::SvcMessage>> {
    match packets {
        Some(packets) => packets.recv().await,
        None => std::future::pending().await,
    }
}

/// Whether an undecodable PDU arrived on the MCS message channel, where
/// optional server features travel; those are skipped rather than fatal.
fn is_message_channel_pdu(
    action: ironrdp_pdu::Action,
    payload: &[u8],
    message_channel_id: Option<u16>,
) -> bool {
    if action != ironrdp_pdu::Action::X224 {
        return false;
    }
    let Some(channel) = message_channel_id else {
        return false;
    };
    ironrdp_pdu::mcs::decode_send_data_indication(payload)
        .is_ok_and(|ctx| ctx.channel_id == channel)
}

/// Handles one output of the session; returns whether the session ended.
async fn handle_output<S>(
    output: ActiveStageOutput,
    writer: &mut impl FramedWrite,
    reader: &mut ironrdp_tokio::Framed<S>,
    events: &mpsc::Sender<Event>,
    image: &mut DecodedImage,
    stage: &mut ActiveStage,
    activation: &ConnectionActivationFactory,
) -> Result<bool, Error>
where
    S: FramedRead + Unpin + Send,
{
    match output {
        ActiveStageOutput::ResponseFrame(data) => {
            writer
                .write_all(&data)
                .await
                .map_err(|e| Error::ConnectionLost(e.to_string()))?;
        }
        ActiveStageOutput::GraphicsUpdate(region) => {
            let rect = clamp(
                inclusive_region(region.left, region.top, region.right, region.bottom),
                image.width(),
                image.height(),
            );
            if rect.width > 0 && rect.height > 0 {
                let bgra = region_data(image, rect);
                let _ = events.send(Event::Frame { rect, bgra });
            }
        }
        ActiveStageOutput::PointerHidden => {
            let _ = events.send(Event::PointerHidden(true));
        }
        ActiveStageOutput::PointerDefault | ActiveStageOutput::PointerBitmap(_) => {
            let _ = events.send(Event::PointerHidden(false));
        }
        ActiveStageOutput::Terminate(reason) => {
            let _ = events.send(Event::Progress(format!(
                "The PC ended the session ({reason:?})"
            )));
            return Ok(true);
        }
        ActiveStageOutput::DeactivateAll => {
            reactivate(activation, reader, writer, image, stage, events).await;
        }
        ActiveStageOutput::ServerRedirection(_) => return Err(Error::BrokerRedirect),
        ActiveStageOutput::MonitorLayout(monitors) => {
            let _ = events.send(Event::Monitors(monitor_rects(&monitors)));
        }
        ActiveStageOutput::AutoReconnectCookie(cookie) => {
            let _ = events.send(Event::ResumeCookie(ResumeCookie::new(
                cookie.logon_id,
                cookie.random_bits,
            )));
        }
        ActiveStageOutput::AutoReconnectFailed => {
            let _ = events.send(Event::Progress(
                "The PC did not resume the earlier session; it signed in anew".to_string(),
            ));
        }
        _ => {}
    }
    Ok(false)
}

/// The Deactivation-Reactivation Sequence (MS-RDPBCGR), which the server runs
/// when the desktop size changes.
async fn reactivate<S>(
    activation: &ConnectionActivationFactory,
    reader: &mut ironrdp_tokio::Framed<S>,
    writer: &mut impl FramedWrite,
    image: &mut DecodedImage,
    stage: &mut ActiveStage,
    events: &mpsc::Sender<Event>,
) where
    S: FramedRead + Unpin + Send,
{
    let mut sequence = activation.create();
    let io_channel_id = activation.io_channel_id();
    let user_channel_id = activation.user_channel_id();
    let mut buf = WriteBuf::new();
    loop {
        let Ok(written) = single_sequence_step_read(reader, &mut sequence, &mut buf).await else {
            return;
        };
        if written.size().is_some() && writer.write_all(buf.filled()).await.is_err() {
            return;
        }
        if let ConnectionActivationState::Finalized {
            desktop_size,
            share_id,
            enable_server_pointer,
            pointer_software_rendering,
            static_channel_chunk_size,
            ..
        } = sequence.connection_activation_state()
        {
            *image =
                DecodedImage::new(PixelFormat::BgrA32, desktop_size.width, desktop_size.height);
            let _ = stage.reactivate(
                io_channel_id,
                user_channel_id,
                share_id,
                enable_server_pointer,
                pointer_software_rendering,
                static_channel_chunk_size,
            );
            let _ = events.send(Event::Resized {
                width: desktop_size.width,
                height: desktop_size.height,
            });
            return;
        }
    }
}

async fn apply_command(
    command: Command,
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
    writer: &mut impl FramedWrite,
    input: &mut Database,
    events: &mpsc::Sender<Event>,
) {
    let operations: Vec<Operation> = match command {
        Command::Key {
            scancode,
            extended,
            pressed,
        } => {
            let code = Scancode::from_u8(extended, scancode);
            vec![if pressed {
                Operation::KeyPressed(code)
            } else {
                Operation::KeyReleased(code)
            }]
        }
        Command::PointerMove { x, y } => vec![Operation::MouseMove(MousePosition { x, y })],
        // The text, if shared, was offered to the PC before this.
        Command::Paste(_) => vec![Operation::KeyPressed(Scancode::from_u8(false, V))],
        Command::MouseButton {
            x,
            y,
            button,
            pressed,
        } => {
            let button = match button {
                MouseButton::Left => ironrdp_input::MouseButton::Left,
                MouseButton::Right => ironrdp_input::MouseButton::Right,
                MouseButton::Middle => ironrdp_input::MouseButton::Middle,
                MouseButton::X1 => ironrdp_input::MouseButton::X1,
                MouseButton::X2 => ironrdp_input::MouseButton::X2,
            };
            vec![
                Operation::MouseMove(MousePosition { x, y }),
                if pressed {
                    Operation::MouseButtonPressed(button)
                } else {
                    Operation::MouseButtonReleased(button)
                },
            ]
        }
        Command::Wheel {
            vertical,
            horizontal,
        } => wheel_steps(vertical, true)
            .chain(wheel_steps(horizontal, false))
            .collect(),
        Command::CtrlAltDel => {
            let ctrl = Scancode::from_u8(false, 0x1D);
            let alt = Scancode::from_u8(false, 0x38);
            let del = Scancode::from_u8(true, 0x53);
            vec![
                Operation::KeyPressed(ctrl),
                Operation::KeyPressed(alt),
                Operation::KeyPressed(del),
                Operation::KeyReleased(del),
                Operation::KeyReleased(alt),
                Operation::KeyReleased(ctrl),
            ]
        }
        Command::Layout(sizes) => {
            let frame = monitor_layout(&sizes)
                .map_err(|e| e.to_string())
                .and_then(|layout| match stage.encode_monitor_layout(&layout) {
                    Some(result) => result.map_err(|e| e.to_string()),
                    None => Err("the PC cannot change its screens during the session".to_string()),
                });
            match frame {
                Ok(frame) => {
                    let _ = writer.write_all(&frame).await;
                }
                Err(e) => {
                    let _ = events.send(Event::Progress(format!("Screen layout not sent: {e}")));
                }
            }
            return;
        }
        Command::Resize { width, height } => {
            match stage.encode_resize(u32::from(width), u32::from(height), None, None) {
                Some(Ok(frame)) => {
                    let _ = writer.write_all(&frame).await;
                }
                Some(Err(e)) => {
                    let _ = events.send(Event::Progress(format!("Resize request failed: {e}")));
                }
                None => {
                    let _ = events.send(Event::Progress(
                        "The PC cannot change its desktop size during the session".to_string(),
                    ));
                }
            }
            return;
        }
    };
    let input_events = input.apply(operations);
    send_input(stage, image, writer, &input_events).await;
}

/// Wheel rotations in steps of at most one notch (120): the protocol field
/// holds −256…255, and a fast touchpad flick is far larger.
fn wheel_steps(total: i16, vertical: bool) -> impl Iterator<Item = Operation> {
    let mut remaining = total;
    std::iter::from_fn(move || {
        if remaining == 0 {
            return None;
        }
        let step = remaining.clamp(-120, 120);
        remaining -= step;
        Some(Operation::WheelRotations(WheelRotations {
            is_vertical: vertical,
            rotation_units: step,
        }))
    })
    .take(20)
}

async fn send_input(
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
    writer: &mut impl FramedWrite,
    events: &[FastPathInputEvent],
) {
    if events.is_empty() {
        return;
    }
    if let Ok(outputs) = stage.process_fastpath_input(image, events) {
        for output in outputs {
            if let ActiveStageOutput::ResponseFrame(data) = output {
                let _ = writer.write_all(&data).await;
            }
        }
    }
}

/// A Display Control layout for screens side by side, left to right, the
/// first one primary.
fn monitor_layout(
    sizes: &[(u16, u16)],
) -> Result<DisplayControlMonitorLayout, ironrdp_core::EncodeError> {
    let mut left = 0i32;
    let mut entries = Vec::with_capacity(sizes.len());
    for (index, &(width, height)) in sizes.iter().enumerate() {
        let (width, height) =
            MonitorLayoutEntry::adjust_display_size(u32::from(width), u32::from(height));
        let entry = if index == 0 {
            MonitorLayoutEntry::new_primary(width, height)?
        } else {
            MonitorLayoutEntry::new_secondary(width, height)?.with_position(left, 0)?
        };
        entries.push(entry);
        left += i32::try_from(width).unwrap_or(i32::MAX);
    }
    DisplayControlMonitorLayout::new(&entries)
}

/// The PC's screens as rectangles from the desktop's top-left corner.
fn monitor_rects(monitors: &[ironrdp_pdu::gcc::Monitor]) -> Vec<Rect> {
    let left = monitors.iter().map(|m| m.left).min().unwrap_or(0);
    let top = monitors.iter().map(|m| m.top).min().unwrap_or(0);
    let clamp = |value: i32| u16::try_from(value.max(0)).unwrap_or(u16::MAX);
    monitors
        .iter()
        .map(|m| Rect {
            x: clamp(m.left - left),
            y: clamp(m.top - top),
            width: clamp(m.right - m.left + 1),
            height: clamp(m.bottom - m.top + 1),
        })
        .collect()
}

/// An IronRDP damage region includes its right column and bottom row.
fn inclusive_region(left: u16, top: u16, right: u16, bottom: u16) -> Rect {
    Rect {
        x: left,
        y: top,
        width: right.saturating_sub(left).saturating_add(1),
        height: bottom.saturating_sub(top).saturating_add(1),
    }
}

/// `rect` cut to the image, so the frame sent always matches its pixels.
fn clamp(rect: Rect, width: u16, height: u16) -> Rect {
    let x = rect.x.min(width);
    let y = rect.y.min(height);
    Rect {
        x,
        y,
        width: rect.width.min(width - x),
        height: rect.height.min(height - y),
    }
}

/// The BGRA pixels of `rect`, which must lie inside the image.
fn region_data(image: &DecodedImage, rect: Rect) -> Vec<u8> {
    let stride = usize::from(image.width()) * 4;
    let row = usize::from(rect.width) * 4;
    let data = image.data();
    let mut out = Vec::with_capacity(row * usize::from(rect.height));
    for y in usize::from(rect.y)..usize::from(rect.y) + usize::from(rect.height) {
        let start = y * stride + usize::from(rect.x) * 4;
        out.extend_from_slice(&data[start..start + row]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_include_their_last_row_and_column() {
        let rect = inclusive_region(10, 20, 19, 29);
        assert_eq!((rect.width, rect.height), (10, 10));
        assert_eq!(inclusive_region(5, 5, 5, 5).width, 1);
    }

    #[test]
    fn regions_are_cut_to_the_image() {
        let rect = clamp(
            Rect {
                x: 1270,
                y: 700,
                width: 64,
                height: 64,
            },
            1280,
            720,
        );
        assert_eq!(
            rect,
            Rect {
                x: 1270,
                y: 700,
                width: 10,
                height: 20
            }
        );
        let outside = clamp(
            Rect {
                x: 2000,
                y: 0,
                width: 5,
                height: 5,
            },
            1280,
            720,
        );
        assert_eq!(outside.width, 0);
    }

    #[test]
    fn region_data_matches_its_rect() {
        let mut image = DecodedImage::new(PixelFormat::BgrA32, 4, 3);
        let rect = Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        assert_eq!(region_data(&image, rect).len(), 2 * 2 * 4);
        let _ = &mut image;
    }

    #[test]
    fn screens_are_laid_out_left_to_right() {
        let layout = monitor_layout(&[(1920, 1080), (1280, 1024)]).expect("layout");
        let encoded = format!("{layout:?}");
        assert!(
            encoded.contains("1920") && encoded.contains("1280"),
            "{encoded}"
        );
        let rects = monitor_rects(&[
            ironrdp_pdu::gcc::Monitor {
                left: -1280,
                top: 0,
                right: -1,
                bottom: 1023,
                flags: ironrdp_pdu::gcc::MonitorFlags::empty(),
            },
            ironrdp_pdu::gcc::Monitor {
                left: 0,
                top: 0,
                right: 1919,
                bottom: 1079,
                flags: ironrdp_pdu::gcc::MonitorFlags::PRIMARY,
            },
        ]);
        assert_eq!(
            rects[0],
            Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 1024
            }
        );
        assert_eq!(
            rects[1],
            Rect {
                x: 1280,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn wheel_flicks_are_split_into_notches() {
        let steps: Vec<i16> = wheel_steps(300, true)
            .map(|op| match op {
                Operation::WheelRotations(WheelRotations { rotation_units, .. }) => rotation_units,
                _ => 0,
            })
            .collect();
        assert_eq!(steps, vec![120, 120, 60]);
        assert_eq!(wheel_steps(0, true).count(), 0);
    }
}
