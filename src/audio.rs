//! The PC's sound on this computer's speakers, through cpal.
//!
//! The engine hands samples over on its own thread; the output stream lives
//! on a thread of its own, since some platforms do not let a stream move
//! between threads. The buffer and the volume follow RustConn's player
//! (`rustconn`, GPL-3.0-or-later).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use remoport_rdp::SoundOutput;

use crate::logging;

/// The most sound held back, in seconds: anything older is dropped rather
/// than letting the sound fall behind the picture.
const MAX_QUEUED_SECONDS: usize = 1;

#[derive(Default)]
struct Buffer {
    samples: VecDeque<i16>,
    max: usize,
}

impl Buffer {
    fn push(&mut self, samples: &[i16]) {
        self.samples.extend(samples);
        let excess = self.samples.len().saturating_sub(self.max);
        self.samples.drain(..excess);
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
            buffer.max = rate as usize * usize::from(channels) * MAX_QUEUED_SECONDS;
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
            buffer.samples.clear();
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
    let mut buffer = buffer.lock().ok();
    for slot in data.iter_mut() {
        *slot = match buffer.as_mut().and_then(|b| b.samples.pop_front()) {
            Some(sample) => convert(f32::from(sample) * level),
            None => silence,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buffer_keeps_only_the_latest_sound() {
        let mut buffer = Buffer {
            samples: VecDeque::new(),
            max: 4,
        };
        buffer.push(&[1, 2, 3]);
        buffer.push(&[4, 5, 6]);
        assert_eq!(Vec::from(buffer.samples), vec![3, 4, 5, 6]);
    }

    #[test]
    fn a_callback_takes_only_what_it_plays() {
        let buffer = Mutex::new(Buffer {
            samples: VecDeque::from(vec![100, 200, 300]),
            max: 10,
        });
        let volume = AtomicU32::new(0.5f32.to_bits());
        let mut out = [9i16; 2];
        fill(&mut out, &buffer, &volume, |v| v as i16, 0);
        assert_eq!(out, [50, 100]);
        let mut out = [9i16; 3];
        fill(&mut out, &buffer, &volume, |v| v as i16, 0);
        assert_eq!(out, [150, 0, 0]);
    }
}
