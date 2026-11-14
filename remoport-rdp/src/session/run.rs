//! Main active-session event loop multiplexing cancellation, clipboard deadlines,
//! shared folders, microphone capture, UI commands, and incoming RDP PDUs.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_input::Database;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_tokio::{FramedWrite, split_tokio_framed};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use tokio::time::Instant;

use super::commands::{self, CommandTarget};
use super::output;
use crate::channels::clipboard::Clipboard;
use crate::channels::microphone::Delivery;
use crate::connect::Established;
use crate::{Command, Error, Event};

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

    let mut work = Workload::new(events.clone());
    loop {
        let holding = clipboard.as_ref().is_some_and(Clipboard::holding);
        let release_at = clipboard_deadline(clipboard.as_ref());
        tokio::select! {
            // In this order, the screen last. Decoding a frame takes this thread
            // for milliseconds, and picking at random let a burst of frames hold
            // back the microphone, which needs a packet out every 10 ms, and the
            // user's input. Those are few enough never to starve the screen.
            biased;
            _ = cancelled.changed() => {
                shutdown_stage(&mut stage, &mut writer).await;
                return Ok(());
            }
            () = tokio::time::sleep_until(release_at), if holding => {
                // The PC did not answer the paste in time: the keys go anyway.
                let mut target = CommandTarget {
                    stage: &mut stage,
                    image: &mut image,
                    writer: &mut writer,
                    input: &mut input,
                };
                for command in clipboard.as_mut().map(Clipboard::release).unwrap_or_default() {
                    commands::apply(command, &mut target, events).await;
                }
            }
            _ = folder_changes.tick(), if drives => {
                if let Some(frame) = folder_change_frame(&mut stage) {
                    writer.write_all(&frame).await.map_err(connection_lost)?;
                }
            }
            Some(messages) = next_microphone_packet(&mut microphone) => {
                // A packet that cannot be encoded is a gap in the sound.
                if let Ok(frame) = stage.encode_dvc_messages(messages) {
                    writer.write_all(&frame).await.map_err(connection_lost)?;
                }
            }
            Some(command) = commands.recv() => {
                let mut target = CommandTarget {
                    stage: &mut stage,
                    image: &mut image,
                    writer: &mut writer,
                    input: &mut input,
                };
                commands::dispatch(command, &mut clipboard, &mut target, events).await?;
            }
            read = reader.read_pdu() => {
                let (action, payload) = read.map_err(connection_lost)?;
                let began = Instant::now();
                match stage.process(&mut image, action, &payload, None) {
                    Ok(outputs) => {
                        if output::handle_all(
                            outputs, &mut writer, &mut reader, events,
                            &mut image, &mut stage, &activation,
                        ).await? {
                            return Ok(());
                        }
                    }
                    Err(_) if is_message_channel_pdu(action, &payload, message_channel_id) => {
                        // Auto-detect, heartbeat and similar on the message
                        // channel: not understood, not fatal.
                    }
                    Err(e) => return Err(Error::Protocol(format!("session error: {e}"))),
                }
                drain_clipboard(&mut clipboard, &mut stage, &mut image, &mut writer, &mut input, events).await?;
                work.add(began.elapsed());
            }
        }
    }
}

fn clipboard_deadline(clipboard: Option<&Clipboard>) -> Instant {
    clipboard.and_then(Clipboard::deadline).map_or_else(
        || Instant::now() + Duration::from_secs(3600),
        Instant::from_std,
    )
}

fn connection_lost(error: std::io::Error) -> Error {
    Error::ConnectionLost(error.to_string())
}

async fn shutdown_stage(stage: &mut ActiveStage, writer: &mut impl FramedWrite) {
    if let Ok(frames) = stage.graceful_shutdown() {
        for frame in frames {
            if let ActiveStageOutput::ResponseFrame(data) = frame {
                let _ = writer.write_all(&data).await;
            }
        }
    }
}

async fn drain_clipboard<W: FramedWrite>(
    clipboard: &mut Option<Clipboard>,
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
    writer: &mut W,
    input: &mut Database,
    events: &mpsc::Sender<Event>,
) -> Result<(), Error> {
    let Some(clip) = clipboard else {
        return Ok(());
    };
    let outcome = clip.drain(stage, events);
    for frame in outcome.frames {
        writer.write_all(&frame).await.map_err(connection_lost)?;
    }
    let mut target = CommandTarget {
        stage,
        image,
        writer,
        input,
    };
    for command in outcome.release {
        commands::apply(command, &mut target, events).await;
    }
    Ok(())
}

/// How long this thread is taken up by each message from the PC, mostly
/// decoding pictures. While it is, nothing else moves, the PC's sound included.
/// Told to the log when the session ends.
struct Workload {
    events: mpsc::Sender<Event>,
    began: Instant,
    messages: u64,
    busy: Duration,
    longest: Duration,
    over_20_ms: u64,
    over_50_ms: u64,
}

impl Workload {
    fn new(events: mpsc::Sender<Event>) -> Self {
        Self {
            events,
            began: Instant::now(),
            messages: 0,
            busy: Duration::ZERO,
            longest: Duration::ZERO,
            over_20_ms: 0,
            over_50_ms: 0,
        }
    }

    fn add(&mut self, spent: Duration) {
        self.messages += 1;
        self.busy += spent;
        self.longest = self.longest.max(spent);
        self.over_20_ms += u64::from(spent > Duration::from_millis(20));
        self.over_50_ms += u64::from(spent > Duration::from_millis(50));
    }

    fn summary(&self, session: Duration) -> String {
        format!(
            "Session: {} messages from the PC took {:.0}% of the thread, longest {} ms, {} over \
             20 ms, {} over 50 ms",
            self.messages,
            self.busy.as_secs_f64() * 100.0 / session.as_secs_f64().max(0.001),
            self.longest.as_millis(),
            self.over_20_ms,
            self.over_50_ms,
        )
    }
}

impl Drop for Workload {
    fn drop(&mut self) {
        let summary = self.summary(self.began.elapsed());
        let _ = self.events.send(Event::Log(summary));
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

/// The next microphone packet, noting how many waited behind it; or never
/// when the PC may not use the microphone.
async fn next_microphone_packet(
    microphone: &mut Option<(
        tokio_mpsc::Receiver<Vec<ironrdp_svc::SvcMessage>>,
        Arc<Delivery>,
    )>,
) -> Option<Vec<ironrdp_svc::SvcMessage>> {
    match microphone {
        Some((packets, delivery)) => {
            let next = packets.recv().await;
            delivery.waiting(packets.len());
            next
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_says_how_much_of_the_thread_the_pc_takes() {
        let mut work = Workload::new(mpsc::channel().0);
        work.add(Duration::from_millis(10));
        work.add(Duration::from_millis(30));
        work.add(Duration::from_millis(60));
        assert_eq!(
            work.summary(Duration::from_millis(1000)),
            "Session: 3 messages from the PC took 10% of the thread, longest 60 ms, 2 over 20 ms, \
             1 over 50 ms"
        );
    }
}
