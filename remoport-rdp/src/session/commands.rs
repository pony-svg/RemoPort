//! Translating app commands (keyboard, pointer, layout, resize, paste) into IronRDP PDUs.

use std::sync::mpsc;

use ironrdp_displaycontrol::pdu::{DisplayControlMonitorLayout, MonitorLayoutEntry};
use ironrdp_input::{Database, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageOutput};
use ironrdp_tokio::FramedWrite;

use crate::channels::clipboard::Clipboard;
use crate::{Command, Error, Event, MouseButton};

/// The V key, pressed for a paste.
const V: u8 = 0x2F;

/// Mutable session handles needed while applying outgoing commands.
pub(super) struct CommandTarget<'a, W> {
    pub stage: &'a mut ActiveStage,
    pub image: &'a mut DecodedImage,
    pub writer: &'a mut W,
    pub input: &'a mut Database,
}

/// Applies an incoming command, queuing behind an in-flight paste if needed.
pub(super) async fn dispatch<W: FramedWrite>(
    command: Command,
    clipboard: &mut Option<Clipboard>,
    target: &mut CommandTarget<'_, W>,
    events: &mpsc::Sender<Event>,
) -> Result<(), Error> {
    match (clipboard, command) {
        // Input keeps its order behind a paste the PC has not taken yet.
        (Some(clip), command) if clip.holding() => clip.hold(command),
        (Some(clip), Command::Paste(text)) => {
            if let Some(frame) = clip.paste(text, target.stage, events) {
                target
                    .writer
                    .write_all(&frame)
                    .await
                    .map_err(|e| Error::ConnectionLost(e.to_string()))?;
            }
            let press = Command::Key {
                scancode: V,
                extended: false,
                pressed: true,
            };
            if clip.holding() {
                clip.hold(press);
            } else {
                apply(press, target, events).await;
            }
        }
        (_, command) => apply(command, target, events).await,
    }
    Ok(())
}

/// Converts a single UI command into RDP input, display layout, or resize frames.
pub(super) async fn apply<W: FramedWrite>(
    command: Command,
    target: &mut CommandTarget<'_, W>,
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
        Command::LockKeys { caps, num, scroll } => {
            let event = ironrdp_input::synchronize_event(scroll, num, caps, false);
            send_input(target.stage, target.image, target.writer, &[event]).await;
            return;
        }
        Command::Layout { sizes, scale } => {
            let frame = monitor_layout(&sizes, scale)
                .map_err(|e| e.to_string())
                .and_then(|layout| match target.stage.encode_monitor_layout(&layout) {
                    Some(result) => result.map_err(|e| e.to_string()),
                    None => Err("the PC cannot change its screens during the session".to_string()),
                });
            match frame {
                Ok(frame) => {
                    let _ = target.writer.write_all(&frame).await;
                }
                Err(e) => {
                    let _ = events.send(Event::Log(format!("Screen layout not sent: {e}")));
                }
            }
            return;
        }
        Command::Resize {
            width,
            height,
            scale,
        } => {
            match target
                .stage
                .encode_resize(u32::from(width), u32::from(height), Some(scale), None)
            {
                Some(Ok(frame)) => {
                    let _ = target.writer.write_all(&frame).await;
                }
                Some(Err(e)) => {
                    let _ = events.send(Event::Log(format!("Resize request failed: {e}")));
                }
                None => {
                    let _ = events.send(Event::Log(
                        "The PC cannot change its desktop size during the session".to_string(),
                    ));
                }
            }
            return;
        }
    };
    let input_events = target.input.apply(operations);
    send_input(target.stage, target.image, target.writer, &input_events).await;
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
    scale: u32,
) -> Result<DisplayControlMonitorLayout, ironrdp_core::EncodeError> {
    let (desktop_scale, device_scale) = crate::connect::scale_factors(scale);
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
        entries.push(
            entry
                .with_desktop_scale_factor(desktop_scale)?
                .with_device_scale_factor(device_scale),
        );
        left += i32::try_from(width).unwrap_or(i32::MAX);
    }
    DisplayControlMonitorLayout::new(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;
    use crate::session::output::monitor_rects;

    #[test]
    fn screens_are_laid_out_left_to_right() {
        let layout = monitor_layout(&[(1920, 1080), (1280, 1024)], 150).expect("layout");
        let encoded = format!("{layout:?}");
        assert!(
            encoded.contains("1920") && encoded.contains("1280"),
            "{encoded}"
        );
        for screen in layout.monitors() {
            assert_eq!(screen.desktop_scale_factor(), Some(150));
            assert_eq!(screen.device_scale_factor().map(|d| d.value()), Some(140));
        }
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
