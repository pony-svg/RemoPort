//! The shared clipboard, text only, for the PCs the user allows it.
//!
//! - The PC gets only text the user pastes into the session: the local
//!   clipboard is offered when the user pastes, never on its own, and the
//!   paste keystroke waits until the PC has taken it.
//! - What the PC copies is fetched at once and handed to the app, which
//!   decides whether to take it.
//! - No files, pictures or other formats; at most [`MAX_TEXT_BYTES`] each way.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use ironrdp_cliprdr::CliprdrClient;
use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_connector::ClientConnector;
use ironrdp_session::ActiveStage;

use crate::{ClipboardText, Command, Event};

/// The largest clipboard text taken from or given to the PC, as UTF-16:
/// a million characters.
pub(crate) const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

/// How long a paste waits for the PC to take the text before the keystroke
/// goes anyway.
const OFFER_WAIT: Duration = Duration::from_millis(500);

/// What the channel reports, from inside IronRDP's processing.
#[derive(Debug)]
enum Signal {
    /// The channel is set up and waits for the first list of formats.
    Initialize,
    /// The PC copied something that includes text.
    RemoteCopied,
    /// The PC asks for the text it was offered.
    Requested(ClipboardFormatId),
    /// The PC's text, or why it was not taken.
    Received(Result<ClipboardText, String>),
    /// The PC took (or refused) the text offered to it.
    Offered(bool),
}

#[derive(Debug)]
pub(crate) struct Backend {
    signals: mpsc::Sender<Signal>,
}

ironrdp_core::impl_as_any!(Backend);

impl CliprdrBackend for Backend {
    fn temporary_directory(&self) -> &str {
        // Only used for file transfers, which are not offered.
        ".cliprdr"
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {}

    fn on_request_format_list(&mut self) {
        let _ = self.signals.send(Signal::Initialize);
    }

    fn on_format_list_response(&mut self, ok: bool) {
        let _ = self.signals.send(Signal::Offered(ok));
    }

    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        if available_formats
            .iter()
            .any(|format| format.id() == ClipboardFormatId::CF_UNICODETEXT)
        {
            let _ = self.signals.send(Signal::RemoteCopied);
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let _ = self.signals.send(Signal::Requested(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let received = if response.is_error() {
            Err("The PC's clipboard text was no longer available".to_string())
        } else if response.data().len() > MAX_TEXT_BYTES {
            Err(format!(
                "The PC copied {} KiB of text; more than 1 million characters is not taken",
                response.data().len() / 1024
            ))
        } else {
            Ok(ClipboardText::new(from_utf16(response.data())))
        };
        let _ = self.signals.send(Signal::Received(received));
    }

    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}

    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}

    fn on_lock(&mut self, _: LockDataId) {}

    fn on_unlock(&mut self, _: LockDataId) {}
}

/// The session's side of the clipboard.
pub(crate) struct Clipboard {
    signals: mpsc::Receiver<Signal>,
    /// The text the user last pasted, given to the PC when it asks.
    local: Option<ClipboardText>,
    /// Whether the PC's clipboard holds `local` right now.
    offered: bool,
    /// Input held back, in order, until the PC takes the pasted text.
    held: Option<(Instant, Vec<Command>)>,
}

/// The channel to register with the connector, and the session's side of it.
pub(crate) fn channel() -> (CliprdrClient, Clipboard) {
    let (signals, receiver) = mpsc::channel();
    let client = CliprdrClient::new(Box::new(Backend { signals }));
    let clipboard = Clipboard {
        signals: receiver,
        local: None,
        offered: false,
        held: None,
    };
    (client, clipboard)
}

/// Registers the clipboard static channel when sharing is enabled.
pub(crate) fn register(connector: &mut ClientConnector, enabled: bool) -> Option<Clipboard> {
    enabled.then(|| {
        let (client, clipboard) = channel();
        connector.static_channels.insert(client);
        clipboard
    })
}

/// What the session sends after a round of clipboard signals.
#[derive(Default)]
pub(crate) struct Outcome {
    pub frames: Vec<Vec<u8>>,
    pub release: Vec<Command>,
}

impl Clipboard {
    /// Whether input is being held back for a paste.
    pub(crate) fn holding(&self) -> bool {
        self.held.is_some()
    }

    pub(crate) fn hold(&mut self, command: Command) {
        if let Some((_, held)) = &mut self.held {
            held.push(command);
        }
    }

    /// When the held input goes regardless.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.held.as_ref().map(|(until, _)| *until)
    }

    /// The input held back, now that it may go.
    pub(crate) fn release(&mut self) -> Vec<Command> {
        self.held.take().map(|(_, held)| held).unwrap_or_default()
    }

    /// The user pasted `text` into the session: offers it to the PC, and
    /// holds input back until the PC has it. Returns the frame to send.
    pub(crate) fn paste(
        &mut self,
        text: ClipboardText,
        stage: &mut ActiveStage,
        events: &mpsc::Sender<Event>,
    ) -> Option<Vec<u8>> {
        let size = text.as_str().encode_utf16().count() * 2;
        if size > MAX_TEXT_BYTES {
            let _ = events.send(Event::Log(format!(
                "The pasted text ({} KiB) was not sent: more than 1 million characters",
                size / 1024
            )));
            return None;
        }
        if self.offered && self.local.as_ref() == Some(&text) {
            return None;
        }
        let cliprdr = stage.get_svc_processor_mut::<CliprdrClient>()?;
        if !cliprdr.is_ready() {
            return None;
        }
        let messages = cliprdr
            .initiate_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)])
            .ok()?;
        let frame = stage.process_svc_processor_messages(messages).ok()?;
        self.local = Some(text);
        self.offered = false;
        self.held = Some((Instant::now() + OFFER_WAIT, Vec::new()));
        Some(frame)
    }

    /// Answers what the channel reported while IronRDP processed the last
    /// message.
    pub(crate) fn drain(
        &mut self,
        stage: &mut ActiveStage,
        events: &mpsc::Sender<Event>,
    ) -> Outcome {
        let mut outcome = Outcome::default();
        while let Ok(signal) = self.signals.try_recv() {
            let messages = {
                let Some(cliprdr) = stage.get_svc_processor_mut::<CliprdrClient>() else {
                    return outcome;
                };
                match signal {
                    Signal::Initialize => cliprdr.initiate_copy(&[]),
                    Signal::RemoteCopied => {
                        self.offered = false;
                        cliprdr.initiate_paste(ClipboardFormatId::CF_UNICODETEXT)
                    }
                    Signal::Requested(format) => {
                        let response = match &self.local {
                            Some(text) if format == ClipboardFormatId::CF_UNICODETEXT => {
                                OwnedFormatDataResponse::new_data(to_utf16(text.as_str()))
                            }
                            _ => OwnedFormatDataResponse::new_error(),
                        };
                        cliprdr.submit_format_data(response)
                    }
                    Signal::Received(Ok(text)) => {
                        let _ = events.send(Event::Clipboard(text));
                        continue;
                    }
                    Signal::Received(Err(why)) => {
                        let _ = events.send(Event::Log(why));
                        continue;
                    }
                    Signal::Offered(ok) => {
                        self.offered = ok;
                        if !ok {
                            let _ = events.send(Event::Log(
                                "The PC did not take the pasted text".to_string(),
                            ));
                        }
                        outcome.release.extend(self.release());
                        continue;
                    }
                }
            };
            let Ok(messages) = messages else {
                continue;
            };
            if let Ok(frame) = stage.process_svc_processor_messages(messages) {
                outcome.frames.push(frame);
            }
        }
        outcome
    }
}

/// Text as the PC's clipboard holds it: UTF-16LE, Windows line ends, ending
/// in a NUL.
fn to_utf16(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2 + 2);
    let mut previous = '\0';
    for c in text.chars() {
        if c == '\n' && previous != '\r' {
            out.extend_from_slice(&u16::from(b'\r').to_le_bytes());
        }
        let mut units = [0u16; 2];
        for unit in c.encode_utf16(&mut units) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        previous = c;
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// The PC's clipboard text, up to its first NUL, with Unix line ends.
fn from_utf16(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units).replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_out_as_windows_text() {
        let bytes = to_utf16("a\nñ\r\n😀");
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        let expected: Vec<u16> = "a\r\nñ\r\n😀\0".encode_utf16().collect();
        assert_eq!(units, expected);
    }

    #[test]
    fn text_comes_in_up_to_its_end() {
        let mut bytes = to_utf16("línea 1\nlínea 2");
        bytes.extend_from_slice(&to_utf16("garbage after the NUL"));
        assert_eq!(from_utf16(&bytes), "línea 1\nlínea 2");
        // An odd trailing byte and a missing NUL are tolerated.
        assert_eq!(from_utf16(&[b'h', 0, b'i', 0, 7]), "hi");
    }

    #[test]
    fn oversized_text_from_the_pc_is_refused() {
        let (signals, receiver) = mpsc::channel();
        let mut backend = Backend { signals };
        let huge = vec![b'a'; MAX_TEXT_BYTES + 2];
        backend.on_format_data_response(FormatDataResponse::new_data(huge.as_slice()));
        backend.on_format_data_response(FormatDataResponse::new_data(to_utf16("ok")));
        backend.on_format_data_response(FormatDataResponse::new_error());
        let received: Vec<_> = receiver.try_iter().collect();
        assert!(matches!(&received[0], Signal::Received(Err(why)) if why.contains("not taken")));
        assert!(matches!(&received[1], Signal::Received(Ok(text)) if text.as_str() == "ok"));
        assert!(matches!(&received[2], Signal::Received(Err(_))));
    }

    #[test]
    fn only_text_copied_on_the_pc_is_fetched() {
        let (signals, receiver) = mpsc::channel();
        let mut backend = Backend { signals };
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::new(2))]);
        assert!(receiver.try_recv().is_err());
        backend.on_remote_copy(&[
            ClipboardFormat::new(ClipboardFormatId::new(2)),
            ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT),
        ]);
        assert!(matches!(receiver.try_recv(), Ok(Signal::RemoteCopied)));
    }

    #[test]
    fn held_input_is_released_in_order() {
        let (_, mut clipboard) = channel();
        clipboard.held = Some((Instant::now(), Vec::new()));
        clipboard.hold(Command::CtrlAltDel);
        clipboard.hold(Command::Wheel {
            vertical: 1,
            horizontal: 0,
        });
        assert!(clipboard.holding());
        assert_eq!(
            clipboard.release(),
            vec![
                Command::CtrlAltDel,
                Command::Wheel {
                    vertical: 1,
                    horizontal: 0
                }
            ]
        );
        assert!(!clipboard.holding());
    }
}
