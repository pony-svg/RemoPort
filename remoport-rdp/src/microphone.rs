//! This computer's microphone, for the PCs the user allows it (MS-RDPEAI).
//! Capture runs only while the PC holds the microphone open, as
//! uncompressed 16-bit PCM, and the app is told each time it opens and
//! closes so it can show that.
//!
//! The formats follow RustConn's client (`rustconn-core`, GPL-3.0-or-later).

use std::sync::{Arc, Mutex, mpsc};

use ironrdp_rdpeai::client::{AudioPacketSink, RdpeaiCaptureHandler};
use ironrdp_rdpeai::pdu::pcm_format;
use ironrdp_rdpsnd::pdu::{AudioFormat, WaveFormat};

use crate::Event;

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
    pub fn send(&self, packet: Vec<u8>) {
        if let Ok(mut sink) = self.0.lock()
            && let Some(sink) = sink.as_mut()
        {
            sink(packet);
        }
    }
}

pub(crate) struct Handler {
    input: Box<dyn SoundInput>,
    formats: Vec<AudioFormat>,
    sink: Arc<Mutex<Option<AudioPacketSink>>>,
    events: mpsc::Sender<Event>,
}

impl Handler {
    pub(crate) fn new(input: Box<dyn SoundInput>, events: mpsc::Sender<Event>) -> Self {
        Self {
            input,
            formats: vec![
                pcm_format(2, 44_100, 16),
                pcm_format(1, 44_100, 16),
                pcm_format(2, 22_050, 16),
                pcm_format(1, 22_050, 16),
            ],
            sink: Arc::new(Mutex::new(None)),
            events,
        }
    }

    fn capture(&mut self, format: &AudioFormat, packet_bytes: usize) -> bool {
        if format.format != WaveFormat::PCM
            || format.bits_per_sample != 16
            || format.n_channels == 0
        {
            return false;
        }
        let packets = MicrophonePackets(Arc::clone(&self.sink));
        let started = self.input.start(
            format.n_samples_per_sec,
            format.n_channels,
            packet_bytes,
            packets,
        );
        let _ = self.events.send(Event::Progress(if started {
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

impl RdpeaiCaptureHandler for Handler {
    fn supported_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn open(
        &mut self,
        capture_format: &AudioFormat,
        _encode_format: &AudioFormat,
        packet_size: usize,
        sink: AudioPacketSink,
    ) -> i32 {
        if let Ok(mut slot) = self.sink.lock() {
            *slot = Some(sink);
        }
        if self.capture(capture_format, packet_size) {
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
        self.input.stop();
        let started = self.capture(encode_format, packet_size);
        if !started {
            let _ = self.events.send(Event::Microphone(false));
        }
        started
    }

    fn close(&mut self) {
        if let Ok(mut slot) = self.sink.lock() {
            *slot = None;
        }
        self.input.stop();
        let _ = self.events.send(Event::Microphone(false));
        let _ = self
            .events
            .send(Event::Progress("The PC closed the microphone".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        started: Arc<Mutex<Option<MicrophonePackets>>>,
    }

    impl SoundInput for Recorder {
        fn start(&mut self, _: u32, _: u16, _: usize, packets: MicrophonePackets) -> bool {
            *self.started.lock().unwrap() = Some(packets);
            true
        }
        fn stop(&mut self) {
            *self.started.lock().unwrap() = None;
        }
    }

    #[test]
    fn sound_reaches_the_pc_only_while_it_holds_the_microphone() {
        let (events, received) = mpsc::channel();
        let recorder = Recorder::default();
        let started = Arc::clone(&recorder.started);
        let mut handler = Handler::new(Box::new(recorder), events);
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

    #[test]
    fn a_compressed_format_is_refused() {
        let mut handler = Handler::new(Box::new(Recorder::default()), mpsc::channel().0);
        let adpcm = AudioFormat {
            format: WaveFormat::ADPCM,
            ..handler.supported_formats()[0].clone()
        };
        assert_eq!(handler.open(&adpcm, &adpcm, 100, Box::new(|_| {})), E_FAIL);
    }
}
