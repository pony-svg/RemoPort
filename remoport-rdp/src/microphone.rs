//! This computer's microphone, for the PCs the user allows it (MS-RDPEAI).
//! Capture runs only while the PC holds the microphone open, as
//! uncompressed 16-bit PCM, and the app is told each time it opens and
//! closes so it can show that.
//!
//! The formats follow RustConn's client (`rustconn-core`, GPL-3.0-or-later).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use ironrdp_dvc::DrdynvcClient;
use ironrdp_rdpeai::client::{AudioPacketSink, DvcUplink, RdpeaiCaptureHandler, RdpeaiClient};
use ironrdp_rdpeai::pdu::pcm_format;
use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};
use ironrdp_svc::SvcMessage;
use tokio::sync::mpsc as tokio_mpsc;

use crate::Event;

/// Microphone packets waiting for the session; beyond this, sound is dropped
/// rather than piling up behind a slow link.
const QUEUE: usize = 64;

/// E_FAIL: the PC is told the microphone could not be opened.
const E_FAIL: i32 = 0x8000_4005_u32 as i32;

/// Where the PC's requests for the microphone go. Called on the connection's
/// thread, so a call must not block for long.
pub trait SoundInput: Send {
    /// The PC opened the microphone: capture interleaved 16-bit samples at
    /// `rate` per second and hand them to `packets` in chunks of
    /// `packet_bytes`. Returns whether capture started.
    fn start(
        &mut self,
        rate: u32,
        channels: u16,
        packet_bytes: usize,
        packets: MicrophonePackets,
    ) -> bool;
    /// The PC closed the microphone.
    fn stop(&mut self);
}

/// Carries captured sound to the PC while it keeps the microphone open;
/// after that, what is sent goes nowhere.
#[derive(Clone)]
pub struct MicrophonePackets(Arc<Mutex<Option<AudioPacketSink>>>);

impl MicrophonePackets {
    /// Packets that go nowhere, as after the PC closes the microphone.
    #[must_use]
    pub fn detached() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    pub fn send(&self, packet: Vec<u8>) {
        if let Ok(mut sink) = self.0.lock()
            && let Some(sink) = sink.as_mut()
        {
            sink(packet);
        }
    }
}

/// How the packets of one capture fared on their way to the PC, for the log:
/// a queue that grows, or packets dropped, mean the connection cannot keep up.
#[derive(Default)]
pub(crate) struct Delivery {
    sent: AtomicU64,
    dropped: AtomicU64,
    longest_queue: AtomicUsize,
}

impl Delivery {
    pub(crate) fn queued(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn waiting(&self, packets: usize) {
        self.longest_queue.fetch_max(packets, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.sent.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.longest_queue.store(0, Ordering::Relaxed);
    }

    fn summary(&self, packet_bytes: usize, bytes_per_second: u32) -> String {
        let per_packet = packet_bytes as f64 * 1000.0 / f64::from(bytes_per_second.max(1));
        format!(
            "Microphone: {} packets of {packet_bytes} bytes ({per_packet:.0} ms each), {} dropped, \
             longest queue {} packets",
            self.sent.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.longest_queue.load(Ordering::Relaxed),
        )
    }
}

/// Offers the microphone on `drdynvc`. Windows opens the channel when an app
/// starts capturing and closes it when the app stops, so each opening gets its
/// own processor: a channel registered once would answer "no microphone here"
/// from the second opening on, and Windows would stop listing the device.
///
/// Packets are sent between the PC's turns, so the channel queues them for the
/// session, returned here with the count of how they fared.
pub(crate) fn register(
    drdynvc: DrdynvcClient,
    input: Box<dyn SoundInput>,
    events: mpsc::Sender<Event>,
) -> (
    DrdynvcClient,
    tokio_mpsc::Receiver<Vec<SvcMessage>>,
    Arc<Delivery>,
) {
    let (queue, packets) = tokio_mpsc::channel(QUEUE);
    let delivery = Arc::new(Delivery::default());
    let shared = Arc::clone(&delivery);
    let input = Arc::new(Mutex::new(input));
    let drdynvc = drdynvc.with_dynamic_channel_factory(ironrdp_rdpeai::CHANNEL_NAME, move || {
        let queue = queue.clone();
        let counted = Arc::clone(&shared);
        let uplink: DvcUplink = Box::new(move |_channel, messages| {
            // A full queue drops this packet: a gap, not a growing delay.
            match queue.try_send(messages) {
                Ok(()) => {
                    counted.queued();
                    Ok(())
                }
                Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                    counted.dropped();
                    Ok(())
                }
                Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                    Err(ironrdp_pdu::pdu_other_err!("the session has ended"))
                }
            }
        });
        let handler = Handler::new(Arc::clone(&input), events.clone(), Arc::clone(&shared));
        RdpeaiClient::new(Box::new(handler), uplink)
    });
    (drdynvc, packets, delivery)
}

pub(crate) struct Handler {
    input: Arc<Mutex<Box<dyn SoundInput>>>,
    formats: Vec<AudioFormat>,
    sink: Arc<Mutex<Option<AudioPacketSink>>>,
    events: mpsc::Sender<Event>,
    delivery: Arc<Delivery>,
    /// The packet size and data rate of the capture under way.
    capturing: (usize, u32),
}

impl Handler {
    pub(crate) fn new(
        input: Arc<Mutex<Box<dyn SoundInput>>>,
        events: mpsc::Sender<Event>,
        delivery: Arc<Delivery>,
    ) -> Self {
        Self {
            input,
            formats: vec![
                pcm_format(2, 48_000, 16),
                pcm_format(1, 48_000, 16),
                pcm_format(2, 44_100, 16),
                pcm_format(1, 44_100, 16),
                pcm_format(2, 22_050, 16),
                pcm_format(1, 22_050, 16),
                pcm_format(1, 16_000, 16),
                pcm_format(1, 11_025, 16),
                pcm_format(1, 8_000, 16),
            ],
            sink: Arc::new(Mutex::new(None)),
            events,
            delivery,
            capturing: (0, 0),
        }
    }

    fn stop_input(&self) {
        if let Ok(mut input) = self.input.lock() {
            input.stop();
        }
    }

    /// Captures `format`, the one the PC decodes the packets with.
    fn capture(&mut self, format: &AudioFormat, packet_bytes: usize) -> bool {
        if format.format != WaveFormat::PCM
            || format.bits_per_sample != 16
            || format.n_channels == 0
        {
            let _ = self.events.send(Event::Log(format!(
                "The PC asked for the microphone in a format not offered: {}",
                describe(format)
            )));
            return false;
        }
        let packets = MicrophonePackets(Arc::clone(&self.sink));
        let started = self.input.lock().is_ok_and(|mut input| {
            input.start(
                format.n_samples_per_sec,
                format.n_channels,
                packet_bytes,
                packets,
            )
        });
        let _ = self.events.send(Event::Log(if started {
            format!(
                "The PC opened the microphone ({} Hz, {} ch)",
                format.n_samples_per_sec, format.n_channels
            )
        } else {
            "The PC asked for the microphone, but it could not be opened".to_string()
        }));
        started
    }
}

/// A format for the log.
fn describe(format: &AudioFormat) -> String {
    format!(
        "{:?} {} Hz, {} ch, {} bit",
        format.format, format.n_samples_per_sec, format.n_channels, format.bits_per_sample
    )
}

impl RdpeaiCaptureHandler for Handler {
    fn supported_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn choose_formats(&self, server_formats: &[AudioFormat]) -> Vec<AudioFormat> {
        let common = self
            .formats
            .iter()
            .filter(|format| server_formats.contains(format))
            .count();
        let offered: Vec<String> = server_formats.iter().take(16).map(describe).collect();
        let _ = self.events.send(Event::Log(format!(
            "Microphone: the PC offers {} formats, {common} usable here: {}",
            server_formats.len(),
            offered.join("; ")
        )));
        self.formats.clone()
    }

    fn open(
        &mut self,
        capture_format: &AudioFormat,
        encode_format: &AudioFormat,
        packet_size: usize,
        sink: AudioPacketSink,
    ) -> i32 {
        let _ = self.events.send(Event::Log(format!(
            "Microphone: the PC asks to capture {} and receive {}",
            describe(capture_format),
            describe(encode_format)
        )));
        if let Ok(mut slot) = self.sink.lock() {
            *slot = Some(sink);
        }
        self.delivery.reset();
        self.capturing = (packet_size, encode_format.n_avg_bytes_per_sec);
        // Nothing here compresses, so what is captured is what the PC decodes:
        // its choice among the formats offered. Windows describes its own
        // capture format differently (often WAVE_FORMAT_EXTENSIBLE).
        if self.capture(encode_format, packet_size) {
            let _ = self.events.send(Event::Microphone(true));
            0
        } else {
            if let Ok(mut slot) = self.sink.lock() {
                *slot = None;
            }
            E_FAIL
        }
    }

    fn set_format(&mut self, encode_format: &AudioFormat, packet_size: usize) -> bool {
        self.stop_input();
        let started = self.capture(encode_format, packet_size);
        if !started {
            let _ = self.events.send(Event::Microphone(false));
        }
        started
    }

    fn close(&mut self) {
        let was_open = self.sink.lock().is_ok_and(|mut slot| slot.take().is_some());
        self.stop_input();
        let _ = self.events.send(Event::Microphone(false));
        if was_open {
            let (packet_bytes, rate) = self.capturing;
            let _ = self
                .events
                .send(Event::Log(self.delivery.summary(packet_bytes, rate)));
        }
        let _ = self.events.send(Event::Log(
            if was_open {
                "The PC closed the microphone"
            } else {
                "The PC closed the microphone channel without capturing"
            }
            .to_string(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        started: Arc<Mutex<Option<MicrophonePackets>>>,
        format: Arc<Mutex<Option<(u32, u16)>>>,
    }

    impl SoundInput for Recorder {
        fn start(
            &mut self,
            rate: u32,
            channels: u16,
            _: usize,
            packets: MicrophonePackets,
        ) -> bool {
            *self.started.lock().unwrap() = Some(packets);
            *self.format.lock().unwrap() = Some((rate, channels));
            true
        }
        fn stop(&mut self) {
            *self.started.lock().unwrap() = None;
        }
    }

    fn handler(input: impl SoundInput + 'static, events: mpsc::Sender<Event>) -> Handler {
        let input: Box<dyn SoundInput> = Box::new(input);
        Handler::new(Arc::new(Mutex::new(input)), events, Arc::default())
    }

    /// The status in a Create Response, at its end: 0 is OK.
    fn creation_status(messages: &[SvcMessage]) -> u32 {
        let bytes = messages
            .last()
            .expect("an answer")
            .encode_unframed_pdu()
            .expect("encodes");
        u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().expect("4 bytes"))
    }

    /// Windows opens the channel for each capture. Found on a real PC: from the
    /// second time on it was told there was no microphone, and stopped listing it.
    #[test]
    fn the_microphone_can_be_opened_again_after_the_pc_closes_it() {
        use ironrdp_dvc::pdu::{
            CapabilitiesRequestPdu, CapsVersion, ClosePdu, CreateRequestPdu, DrdynvcServerPdu,
        };
        use ironrdp_svc::SvcProcessor as _;

        let (mut client, _packets, _delivery) = register(
            DrdynvcClient::new(),
            Box::new(Recorder::default()),
            mpsc::channel().0,
        );
        let mut send = |pdu: DrdynvcServerPdu| {
            let bytes = ironrdp_pdu::encode_vec(&pdu).expect("encodes");
            client.process(&bytes).expect("processed")
        };
        send(DrdynvcServerPdu::Capabilities(CapabilitiesRequestPdu::new(
            CapsVersion::V1,
            None,
        )));
        let name = || ironrdp_rdpeai::CHANNEL_NAME.to_string();
        for channel in [3, 4, 5] {
            let created = send(DrdynvcServerPdu::Create(CreateRequestPdu::new(
                channel,
                name(),
            )));
            assert_eq!(creation_status(&created), 0, "opening {channel}");
            send(DrdynvcServerPdu::Close(ClosePdu::new(channel)));
        }
    }

    #[test]
    fn sound_reaches_the_pc_only_while_it_holds_the_microphone() {
        let (events, received) = mpsc::channel();
        let recorder = Recorder::default();
        let started = Arc::clone(&recorder.started);
        let mut handler = handler(recorder, events);
        assert!(
            handler
                .supported_formats()
                .iter()
                .all(|f| f.format == WaveFormat::PCM && f.bits_per_sample == 16)
        );

        let delivered = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let into = Arc::clone(&delivered);
        let format = handler.supported_formats()[0].clone();
        let status = handler.open(
            &format,
            &format,
            1764,
            Box::new(move |packet| into.lock().unwrap().push(packet)),
        );
        assert_eq!(status, 0);
        let packets = started.lock().unwrap().clone().expect("capture started");
        packets.send(vec![1, 2]);
        handler.close();
        // A packet still in flight after the PC closed the microphone is dropped.
        packets.send(vec![3, 4]);

        assert_eq!(*delivered.lock().unwrap(), vec![vec![1, 2]]);
        assert!(started.lock().unwrap().is_none(), "capture still running");
        let states: Vec<bool> = received
            .try_iter()
            .filter_map(|e| match e {
                Event::Microphone(live) => Some(live),
                _ => None,
            })
            .collect();
        assert_eq!(states, vec![true, false]);
    }

    /// Windows describes its capture format as WAVE_FORMAT_EXTENSIBLE; what
    /// counts is the format it receives.
    #[test]
    fn the_pc_gets_the_format_it_chose_however_it_describes_its_capture() {
        let (events, received) = mpsc::channel();
        let recorder = Recorder::default();
        let format = Arc::clone(&recorder.format);
        let mut handler = handler(recorder, events);
        let extensible = AudioFormat {
            format: WaveFormat::EXTENSIBLE,
            n_samples_per_sec: 48_000,
            bits_per_sample: 32,
            ..pcm_format(2, 48_000, 16)
        };
        let chosen = pcm_format(1, 22_050, 16);
        assert_eq!(handler.open(&extensible, &chosen, 960, Box::new(|_| {})), 0);
        assert_eq!(*format.lock().unwrap(), Some((22_050, 1)));
        assert!(
            received
                .try_iter()
                .any(|e| matches!(e, Event::Microphone(true)))
        );
    }

    #[test]
    fn what_the_pc_offers_is_logged_and_nothing_is_silently_refused() {
        let (events, received) = mpsc::channel();
        let mut handler = handler(Recorder::default(), events);
        let offered = [pcm_format(2, 44_100, 16), pcm_format(2, 44_100, 8)];
        assert_eq!(handler.choose_formats(&offered), handler.formats);
        let adpcm = AudioFormat {
            format: WaveFormat::ADPCM,
            ..pcm_format(2, 44_100, 16)
        };
        assert_eq!(handler.open(&adpcm, &adpcm, 100, Box::new(|_| {})), E_FAIL);
        handler.close();
        let log: Vec<String> = received
            .try_iter()
            .filter_map(|e| match e {
                Event::Log(line) => Some(line),
                _ => None,
            })
            .collect();
        assert!(log[0].contains("offers 2 formats, 1 usable"), "{log:?}");
        assert!(log.iter().any(|l| l.contains("not offered")), "{log:?}");
        assert!(
            log.iter().any(|l| l.contains("without capturing")),
            "{log:?}"
        );
    }

    #[test]
    fn the_log_says_how_the_packets_fared() {
        let delivery = Delivery::default();
        delivery.queued();
        delivery.queued();
        delivery.dropped();
        delivery.waiting(3);
        delivery.waiting(1);
        assert_eq!(
            delivery.summary(8820, 176_400),
            "Microphone: 2 packets of 8820 bytes (50 ms each), 1 dropped, longest queue 3 packets"
        );
        delivery.reset();
        assert!(delivery.summary(8820, 176_400).contains(": 0 packets"));
    }

    #[test]
    fn a_compressed_format_is_refused() {
        let mut handler = handler(Recorder::default(), mpsc::channel().0);
        let adpcm = AudioFormat {
            format: WaveFormat::ADPCM,
            ..handler.supported_formats()[0].clone()
        };
        assert_eq!(handler.open(&adpcm, &adpcm, 100, Box::new(|_| {})), E_FAIL);
    }
}
