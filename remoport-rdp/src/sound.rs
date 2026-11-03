//! The PC's sound, played on this computer. Only uncompressed 16-bit PCM is
//! offered, so nothing the PC sends goes through a decoder.
//!
//! The formats follow RustConn's client (`rustconn-core`, GPL-3.0-or-later).

use std::borrow::Cow;
use std::time::Duration;

use ironrdp_rdpsnd::client::RdpsndClientHandler;
use ironrdp_rdpsnd::pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat};

/// Where the PC's sound goes. Called on the connection's thread, so a call
/// must not block for long.
pub trait SoundOutput: Send {
    /// Interleaved 16-bit samples at `rate` per second.
    fn play(&mut self, rate: u32, channels: u16, samples: &[i16]);
    /// The PC's volume for each side, from 0.0 to 1.0.
    fn set_volume(&mut self, left: f32, right: f32);
    /// The PC stopped playing.
    fn stop(&mut self);
    /// How long until what was queued finishes playing.
    fn queued(&self) -> Duration {
        Duration::ZERO
    }
}

pub(crate) struct Handler {
    output: Box<dyn SoundOutput>,
    formats: Vec<AudioFormat>,
    samples: Vec<i16>,
}

impl Handler {
    pub(crate) fn new(output: Box<dyn SoundOutput>) -> Self {
        let pcm = |channels: u16, rate: u32| AudioFormat {
            format: WaveFormat::PCM,
            n_channels: channels,
            n_samples_per_sec: rate,
            n_avg_bytes_per_sec: rate * u32::from(channels) * 2,
            n_block_align: channels * 2,
            bits_per_sample: 16,
            data: None,
        };
        Self {
            output,
            formats: vec![
                pcm(2, 48_000),
                pcm(2, 44_100),
                pcm(2, 22_050),
                pcm(1, 48_000),
                pcm(1, 44_100),
            ],
            samples: Vec::new(),
        }
    }
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sound::Handler")
    }
}

impl RdpsndClientHandler for Handler {
    fn get_flags(&self) -> AudioFormatFlags {
        AudioFormatFlags::VOLUME
    }

    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        if format.format != WaveFormat::PCM
            || format.bits_per_sample != 16
            || format.n_channels == 0
        {
            return;
        }
        self.samples.clear();
        self.samples.extend(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
        );
        self.output
            .play(format.n_samples_per_sec, format.n_channels, &self.samples);
    }

    fn playback_latency(&self) -> Duration {
        self.output.queued()
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        let level = |v: u16| f32::from(v) / f32::from(u16::MAX);
        self.output
            .set_volume(level(volume.volume_left), level(volume.volume_right));
    }

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {
        self.output.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Played = Arc<Mutex<Vec<(u32, u16, Vec<i16>)>>>;

    struct Recorder(Played);

    impl SoundOutput for Recorder {
        fn play(&mut self, rate: u32, channels: u16, samples: &[i16]) {
            self.0
                .lock()
                .unwrap()
                .push((rate, channels, samples.to_vec()));
        }
        fn set_volume(&mut self, _: f32, _: f32) {}
        fn stop(&mut self) {}
    }

    #[test]
    fn only_16_bit_pcm_is_offered_and_played() {
        let played = Arc::new(Mutex::new(Vec::new()));
        let mut handler = Handler::new(Box::new(Recorder(played.clone())));
        assert!(
            handler
                .get_formats()
                .iter()
                .all(|f| f.format == WaveFormat::PCM && f.bits_per_sample == 16)
        );

        let stereo = handler.get_formats()[0].clone();
        handler.wave(&stereo, 0, Cow::Borrowed(&[0x01, 0x00, 0xFF, 0xFF, 0x7F]));
        let compressed = AudioFormat {
            format: WaveFormat::ADPCM,
            ..stereo.clone()
        };
        handler.wave(&compressed, 0, Cow::Borrowed(&[1, 2, 3, 4]));

        let played = played.lock().unwrap();
        assert_eq!(played.len(), 1);
        // The odd trailing byte is dropped.
        assert_eq!(played[0], (48_000, 2, vec![1, -1]));
    }
}
