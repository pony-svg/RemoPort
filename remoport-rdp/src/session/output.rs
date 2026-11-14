//! Translating active-stage server outputs (video frames, pointer shapes, reactivation, monitors)
//! into engine events.

use std::sync::mpsc;

use ironrdp_connector::connection_activation::{
    ConnectionActivationFactory, ConnectionActivationState,
};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::WriteBuf;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageOutput};
use ironrdp_tokio::{FramedRead, FramedWrite, single_sequence_step_read};

use crate::{Error, Event, PointerShape, Rect, RemotePointer, ResumeCookie};

/// The largest pointer the protocol carries (MS-RDPBCGR 2.2.9.1.1.4.4).
const MAX_POINTER_SIDE: u16 = 384;

/// Processes all active-stage outputs for one decoded PDU; returns whether the session ended.
pub(super) async fn handle_all<S>(
    outputs: Vec<ActiveStageOutput>,
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
    for output in outputs {
        if handle_output(output, writer, reader, events, image, stage, activation).await? {
            return Ok(true);
        }
    }
    Ok(false)
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
            let _ = events.send(Event::Pointer(RemotePointer::Hidden));
        }
        ActiveStageOutput::PointerDefault => {
            let _ = events.send(Event::Pointer(RemotePointer::Default));
        }
        ActiveStageOutput::PointerBitmap(pointer) => {
            let shape = pointer_shape(
                pointer.width,
                pointer.height,
                pointer.hotspot_x,
                pointer.hotspot_y,
                &pointer.bitmap_data,
            );
            if let Some(shape) = shape {
                let _ = events.send(Event::Pointer(RemotePointer::Shape(shape)));
            }
        }
        ActiveStageOutput::Terminate(reason) => {
            let _ = events.send(Event::Log(format!("The PC ended the session ({reason:?})")));
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
            let _ = events.send(Event::Log(
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

/// A pointer picture from the PC, if it is one: an empty or oversized picture,
/// or data that does not match its size, is not drawn.
fn pointer_shape(
    width: u16,
    height: u16,
    hotspot_x: u16,
    hotspot_y: u16,
    rgba: &[u8],
) -> Option<PointerShape> {
    let fits = width > 0 && height > 0 && width <= MAX_POINTER_SIDE && height <= MAX_POINTER_SIDE;
    (fits && rgba.len() == usize::from(width) * usize::from(height) * 4).then(|| PointerShape {
        width,
        height,
        hotspot_x: hotspot_x.min(width - 1),
        hotspot_y: hotspot_y.min(height - 1),
        rgba: rgba.to_vec(),
    })
}

/// The PC's screens as rectangles from the desktop's top-left corner.
pub(super) fn monitor_rects(monitors: &[ironrdp_pdu::gcc::Monitor]) -> Vec<Rect> {
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
    fn a_pointer_from_the_pc_is_only_trusted_when_it_adds_up() {
        let two_by_two = [7u8; 16];
        let shape = pointer_shape(2, 2, 1, 9, &two_by_two).expect("a pointer");
        assert_eq!(
            (shape.hotspot_x, shape.hotspot_y),
            (1, 1),
            "hotspot kept inside"
        );
        assert!(pointer_shape(2, 2, 0, 0, &two_by_two[..15]).is_none());
        assert!(pointer_shape(0, 2, 0, 0, &[]).is_none());
        assert!(pointer_shape(1000, 1, 0, 0, &[0; 4000]).is_none());
    }
}
