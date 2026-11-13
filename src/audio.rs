//! The PC's sound on this computer's speakers, and this computer's
//! microphone for the PC, through cpal.
//!
//! The engine calls in on its own thread; each stream lives on a thread of
//! its own, since some platforms do not let a stream move between threads.
//! The buffer, the volume and the packet builder follow RustConn's audio
//! (`rustconn`, GPL-3.0-or-later).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use remoport_rdp::{MicrophonePackets, SoundInput, SoundOutput};

use crate::logging;
use crate::ports::AudioFactory;

/// Platform audio adapter using `cpal`.
pub struct CpalAudioFactory;

impl AudioFactory for CpalAudioFactory {
    fn sound_output(&self) -> Box<dyn SoundOutput> {
        Box::new(Speaker::new())
    }

    fn sound_input(&self, muted: Arc<AtomicBool>) -> Box<dyn SoundInput> {
        Box::new(Microphone::new(muted))
    }
}

/// The most sound held back, in seconds: anything older is dropped rather
/// than letting the sound fall behind the picture.
const MAX_QUEUED_SECONDS: usize = 1;

/// How much sound gathers before it plays: a cushion, so that a block arriving a
/// little late does not cut the sound. A sound shorter than this plays anyway
/// once it has waited this long.
const CUSHION: Duration = Duration::from_millis(120);

/// The most the cushion grows to, when the sound keeps being cut.
const MAX_CUSHION: Duration = Duration::from_millis(480);

/// Sound resuming within this long of running dry was cut, not over.
const GAP: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Buffer {
    samples: VecDeque<i16>,
    max: usize,
    /// Samples per second, across the channels.
    per_second: usize,
    /// Samples to gather before playing. Kept while the session lasts, since
    /// what the connection needs does not change from one sound to the next.
    cushion: usize,
    playing: bool,
    /// When the sound now gathering began.
    gathering_since: Option<Instant>,
    /// When the sound last ran out.
    dry_since: Option<Instant>,
    /// For the log: samples played, and how often the sound was cut.
    played: u64,
    gaps: u64,
}

impl Buffer {
    fn push(&mut self, samples: &[i16]) {
        self.samples.extend(samples);
        let excess = self.samples.len().saturating_sub(self.max);
        self.samples.drain(..excess);
        if !self.playing && self.gathering_since.is_none() {
            self.gathering_since = Some(Instant::now());
        }
        if self
            .dry_since
            .take()
            .is_some_and(|since| since.elapsed() < GAP)
        {
            self.gaps += 1;
            let limit = self.per_second * MAX_CUSHION.as_millis() as usize / 1000;
            self.cushion = (self.cushion * 3 / 2).min(limit);
        }
    }

    /// The cushion, as a time.
    fn cushion_time(&self) -> Duration {
        Duration::from_secs_f64(self.cushion as f64 / self.per_second.max(1) as f64)
    }

    /// Whether to play now: enough has gathered, or it has waited long enough.
    fn ready(&mut self) -> bool {
        if !self.playing && !self.samples.is_empty() {
            let cushion = self.cushion_time();
            let waited = self
                .gathering_since
                .is_some_and(|since| since.elapsed() >= cushion);
            if waited || self.samples.len() >= self.cushion {
                self.playing = true;
                self.gathering_since = None;
            }
        }
        self.playing
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.playing = false;
        self.gathering_since = None;
        self.dry_since = None;
        self.played = 0;
        self.gaps = 0;
    }
}

/// A stream playing on its own thread until the sender is dropped.
struct Stream {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

pub struct Speaker {
    buffer: Arc<Mutex<Buffer>>,
    /// The volume, as the bits of an `f32` from 0.0 to 1.0.
    volume: Arc<AtomicU32>,
    format: Option<(u32, u16)>,
    stream: Option<Stream>,
}

impl Speaker {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Buffer::default())),
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            format: None,
            stream: None,
        }
    }

    fn start(&mut self, rate: u32, channels: u16) {
        self.stop();
        if let Ok(mut buffer) = self.buffer.lock() {
            let per_second = rate as usize * usize::from(channels);
            buffer.max = per_second * MAX_QUEUED_SECONDS;
            if buffer.per_second != per_second {
                buffer.per_second = per_second;
                buffer.cushion = per_second * CUSHION.as_millis() as usize / 1000;
            }
        }
        let buffer = Arc::clone(&self.buffer);
        let volume = Arc::clone(&self.volume);
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("remoport-sound".to_string())
            .spawn(move || match open(rate, channels, buffer, volume) {
                Ok(stream) => {
                    logging::event(format_args!(
                        "Playing the PC's sound ({rate} Hz, {channels} ch)"
                    ));
                    // Plays until the speaker stops it.
                    let _ = stopped.recv();
                    drop(stream);
                }
                Err(e) => logging::event(format_args!("The PC's sound cannot be played: {e}")),
            });
        self.stream = thread.ok().map(|thread| Stream { stop, thread });
        self.format = Some((rate, channels));
    }
}

impl SoundOutput for Speaker {
    fn play(&mut self, rate: u32, channels: u16, samples: &[i16]) {
        if self.format != Some((rate, channels)) {
            self.start(rate, channels);
        }
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push(samples);
        }
    }

    fn set_volume(&mut self, left: f32, right: f32) {
        let level = ((left + right) / 2.0).clamp(0.0, 1.0);
        self.volume.store(level.to_bits(), Ordering::Relaxed);
    }

    fn stop(&mut self) {
        if let Some(Stream { stop, thread }) = self.stream.take() {
            drop(stop);
            let _ = thread.join();
        }
        if let Ok(mut buffer) = self.buffer.lock() {
            if let (Some((rate, channels)), true) = (self.format, buffer.played > 0) {
                let per_second = f64::from(rate) * f64::from(channels);
                logging::event(format_args!(
                    "Sound: {:.1} s played, cut {} times, cushion now {} ms",
                    buffer.played as f64 / per_second.max(1.0),
                    buffer.gaps,
                    buffer.cushion_time().as_millis(),
                ));
            }
            buffer.reset();
        }
        self.format = None;
    }

    fn queued(&self) -> Duration {
        let Some((rate, channels)) = self.format else {
            return Duration::ZERO;
        };
        let queued = self.buffer.lock().map_or(0, |b| b.samples.len());
        let per_second = f64::from(rate) * f64::from(channels);
        Duration::from_secs_f64(queued as f64 / per_second.max(1.0))
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The default output, playing what the buffer holds and silence when it
/// runs dry: 16-bit samples where the device takes them, else floats.
fn open(
    rate: u32,
    channels: u16,
    buffer: Arc<Mutex<Buffer>>,
    volume: Arc<AtomicU32>,
) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("this computer has no sound output")?;
    let config = cpal::StreamConfig {
        channels,
        sample_rate: rate,
        buffer_size: cpal::BufferSize::Default,
    };
    let (i16_buffer, i16_volume) = (Arc::clone(&buffer), Arc::clone(&volume));
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                // `as` saturates, and the level keeps samples in range anyway.
                fill(data, &i16_buffer, &i16_volume, |v| v as i16, 0);
            },
            |e| logging::event(format_args!("Sound output error: {e}")),
            None,
        )
        .or_else(|_| {
            device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    fill(data, &buffer, &volume, |v| v / 32768.0, 0.0);
                },
                |e| logging::event(format_args!("Sound output error: {e}")),
                None,
            )
        })
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

/// One callback's worth of samples at the current volume, and silence once
/// the buffer runs dry.
fn fill<T: Copy>(
    data: &mut [T],
    buffer: &Mutex<Buffer>,
    volume: &AtomicU32,
    convert: impl Fn(f32) -> T,
    silence: T,
) {
    let level = f32::from_bits(volume.load(Ordering::Relaxed));
    let Ok(mut buffer) = buffer.lock() else {
        data.fill(silence);
        return;
    };
    if !buffer.ready() {
        data.fill(silence);
        return;
    }
    for slot in data.iter_mut() {
        *slot = match buffer.samples.pop_front() {
            Some(sample) => {
                buffer.played += 1;
                convert(f32::from(sample) * level)
            }
            None => {
                // Dry: gather a cushion again, instead of stuttering on each block.
                if buffer.playing {
                    buffer.playing = false;
                    buffer.dry_since = Some(Instant::now());
                }
                silence
            }
        };
    }
}

/// This computer's microphone, captured only between the PC's `start` and
/// `stop`, and silenced while `muted` is set.
pub struct Microphone {
    muted: Arc<AtomicBool>,
    stream: Option<Stream>,
}

impl Microphone {
    pub fn new(muted: Arc<AtomicBool>) -> Self {
        Self {
            muted,
            stream: None,
        }
    }
}

impl SoundInput for Microphone {
    fn start(
        &mut self,
        rate: u32,
        channels: u16,
        packet_bytes: usize,
        packets: MicrophonePackets,
    ) -> bool {
        self.stop();
        let muted = Arc::clone(&self.muted);
        let (started, ready) = mpsc::channel();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("remoport-microphone".to_string())
            .spawn(move || {
                let samples_per_second = u64::from(rate) * u64::from(channels);
                let builder = PacketBuilder::new(packet_bytes, packets, muted, samples_per_second);
                let frames = packet_bytes / (2 * usize::from(channels.max(1)));
                let frames = u32::try_from(frames).unwrap_or(u32::MAX).max(1);
                match capture(rate, channels, frames, builder) {
                    Ok(stream) => {
                        let _ = started.send(true);
                        // Captures until the PC closes the microphone.
                        let _ = stopped.recv();
                        drop(stream);
                    }
                    Err(e) => {
                        logging::event(format_args!("The microphone cannot be opened: {e}"));
                        let _ = started.send(false);
                    }
                }
            });
        let Ok(thread) = thread else {
            return false;
        };
        let ok = ready.recv_timeout(Duration::from_secs(5)).unwrap_or(false);
        self.stream = Some(Stream { stop, thread });
        if !ok {
            self.stop();
        }
        ok
    }

    fn stop(&mut self) {
        if let Some(Stream { stop, thread }) = self.stream.take() {
            drop(stop);
            let _ = thread.join();
        }
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        SoundInput::stop(self);
    }
}

/// Gathers captured samples into the packets the PC expects.
struct PacketBuilder {
    pending: Vec<u8>,
    packet_bytes: usize,
    packets: MicrophonePackets,
    muted: Arc<AtomicBool>,
    samples_per_second: u64,
    /// How much was captured and how loud, for the log: never the sound.
    samples: u64,
    peak: u16,
    /// Samples at the very top of the range: sound too loud for the microphone.
    clipped: u64,
}

impl PacketBuilder {
    fn new(
        packet_bytes: usize,
        packets: MicrophonePackets,
        muted: Arc<AtomicBool>,
        samples_per_second: u64,
    ) -> Self {
        let packet_bytes = packet_bytes.clamp(2, 1 << 20);
        Self {
            pending: Vec::with_capacity(packet_bytes * 2),
            packet_bytes,
            packets,
            muted,
            samples_per_second,
            samples: 0,
            peak: 0,
            clipped: 0,
        }
    }

    /// What was captured, for the log: a second count and the loudest moment.
    fn summary(&self) -> String {
        let seconds = self.samples as f64 / self.samples_per_second.max(1) as f64;
        let loudest = u32::from(self.peak) * 100 / 32768;
        let clipped = self.clipped as f64 * 100.0 / self.samples.max(1) as f64;
        format!("Microphone: {seconds:.1} s captured, loudest {loudest}%, {clipped:.1}% clipped")
    }

    fn push(&mut self, samples: impl IntoIterator<Item = i16>) {
        let muted = self.muted.load(Ordering::Relaxed);
        for sample in samples {
            let sample = if muted { 0 } else { sample };
            self.samples += 1;
            self.peak = self.peak.max(sample.unsigned_abs());
            if sample.unsigned_abs() >= 32767 {
                self.clipped += 1;
            }
            self.pending.extend_from_slice(&sample.to_le_bytes());
        }
        while self.pending.len() >= self.packet_bytes {
            let rest = self.pending.split_off(self.packet_bytes);
            self.packets
                .send(std::mem::replace(&mut self.pending, rest));
        }
    }
}

impl Drop for PacketBuilder {
    fn drop(&mut self) {
        logging::event(self.summary());
    }
}

/// The default input, as 16-bit samples where the device gives them, else
/// floats converted.
fn capture(
    rate: u32,
    channels: u16,
    frames: u32,
    builder: PacketBuilder,
) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("this computer has no microphone")?;
    let builder = Arc::new(Mutex::new(builder));
    // Chunks of the PC's packet size where the device can, so the sound leaves
    // as evenly as the PC expects it.
    let mut failure = String::new();
    for buffer_size in [cpal::BufferSize::Fixed(frames), cpal::BufferSize::Default] {
        let config = cpal::StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size,
        };
        match open_input(&device, config, &builder) {
            Ok(stream) => {
                stream.play().map_err(|e| e.to_string())?;
                return Ok(stream);
            }
            Err(e) => failure = e,
        }
    }
    Err(failure)
}

fn open_input(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    builder: &Arc<Mutex<PacketBuilder>>,
) -> Result<cpal::Stream, String> {
    let (for_i16, for_f32) = (Arc::clone(builder), Arc::clone(builder));
    device
        .build_input_stream(
            config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut builder) = for_i16.lock() {
                    builder.push(data.iter().copied());
                }
            },
            |e| logging::event(format_args!("Microphone error: {e}")),
            None,
        )
        .or_else(|_| {
            device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut builder) = for_f32.lock() {
                        // `as` saturates; the clamp keeps the scale honest.
                        builder.push(data.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16));
                    }
                },
                |e| logging::event(format_args!("Microphone error: {e}")),
                None,
            )
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_says_how_much_sound_and_how_loud_never_the_sound() {
        let muted = Arc::new(AtomicBool::new(false));
        let mut builder = PacketBuilder {
            pending: Vec::new(),
            packet_bytes: 1 << 20,
            packets: MicrophonePackets::detached(),
            muted: Arc::clone(&muted),
            samples_per_second: 4,
            samples: 0,
            peak: 0,
            clipped: 0,
        };
        builder.push([100, -16384, 200, 300]);
        assert_eq!(
            builder.summary(),
            "Microphone: 1.0 s captured, loudest 50%, 0.0% clipped"
        );
        // Muted, the PC gets silence, and the log counts it as silence.
        muted.store(true, Ordering::Relaxed);
        builder.push([32767; 4]);
        assert_eq!(
            builder.summary(),
            "Microphone: 2.0 s captured, loudest 50%, 0.0% clipped"
        );
        muted.store(false, Ordering::Relaxed);
        builder.push([32767, -32768, 0, 0]);
        assert_eq!(
            builder.summary(),
            "Microphone: 3.0 s captured, loudest 100%, 16.7% clipped"
        );
    }

    #[test]
    fn the_buffer_keeps_only_the_latest_sound() {
        let mut buffer = Buffer {
            max: 4,
            ..Buffer::default()
        };
        buffer.push(&[1, 2, 3]);
        buffer.push(&[4, 5, 6]);
        assert_eq!(Vec::from(buffer.samples), vec![3, 4, 5, 6]);
    }

    fn half_volume() -> AtomicU32 {
        AtomicU32::new(0.5f32.to_bits())
    }

    #[test]
    fn a_callback_takes_only_what_it_plays() {
        let buffer = Mutex::new(Buffer {
            samples: VecDeque::from(vec![100, 200, 300]),
            max: 10,
            cushion: 1,
            ..Buffer::default()
        });
        let volume = half_volume();
        let mut out = [9i16; 2];
        fill(&mut out, &buffer, &volume, |v| v as i16, 0);
        assert_eq!(out, [50, 100]);
        let mut out = [9i16; 3];
        fill(&mut out, &buffer, &volume, |v| v as i16, 0);
        assert_eq!(out, [150, 0, 0]);
        let buffer = buffer.lock().unwrap();
        assert_eq!(buffer.played, 3);
        assert!(buffer.dry_since.is_some());
        assert_eq!(buffer.gaps, 0, "the end of a sound is not a cut");
    }

    #[test]
    fn sound_gathers_a_cushion_first_and_again_after_running_dry() {
        let buffer = Mutex::new(Buffer {
            max: 100,
            per_second: 1000,
            cushion: 4,
            ..Buffer::default()
        });
        let volume = half_volume();
        let play = |count: usize| {
            let mut out = vec![9i16; count];
            fill(&mut out, &buffer, &volume, |v| v as i16, 0);
            out
        };
        buffer.lock().unwrap().push(&[100, 100]);
        assert_eq!(play(2), [0, 0], "too little to start");
        buffer.lock().unwrap().push(&[100, 100]);
        assert_eq!(play(5), [50, 50, 50, 50, 0], "started, and ran dry");
        // Cut: the cushion grew to 6, and a late block alone does not restart it.
        buffer.lock().unwrap().push(&[100]);
        assert_eq!(buffer.lock().unwrap().cushion, 6);
        assert_eq!(play(1), [0]);
        buffer.lock().unwrap().push(&[100; 5]);
        assert_eq!(play(6), [50; 6]);
    }

    #[test]
    fn a_real_cut_grows_the_cushion_and_the_end_of_a_sound_does_not() {
        let mut buffer = Buffer {
            max: 100_000,
            per_second: 1000,
            cushion: 120,
            ..Buffer::default()
        };
        // Sound resumes right after running out: it was cut.
        buffer.dry_since = Some(Instant::now());
        buffer.push(&[1]);
        assert_eq!((buffer.gaps, buffer.cushion), (1, 180));
        // A new sound long after the last: nothing was cut.
        buffer.dry_since = Instant::now().checked_sub(GAP * 2);
        buffer.push(&[1]);
        assert_eq!((buffer.gaps, buffer.cushion), (1, 180));
        // It stops growing at its limit.
        for _ in 0..10 {
            buffer.dry_since = Some(Instant::now());
            buffer.push(&[1]);
        }
        assert_eq!(buffer.cushion, 480);
    }

    #[test]
    fn a_sound_shorter_than_the_cushion_still_plays() {
        let buffer = Mutex::new(Buffer {
            max: 100,
            per_second: 1000,
            cushion: 1000,
            ..Buffer::default()
        });
        let volume = half_volume();
        buffer.lock().unwrap().push(&[100, 100]);
        buffer.lock().unwrap().gathering_since = Instant::now().checked_sub(Duration::from_secs(1));
        let mut out = [9i16; 3];
        fill(&mut out, &buffer, &volume, |v| v as i16, 0);
        assert_eq!(out, [50, 50, 0]);
    }
}
