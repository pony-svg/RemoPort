//! Port traits separating the core session logic from platform-specific adapters
//! (system keyring, Wayland keyboard grab, and cpal audio).

use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use remoport_rdp::{SoundInput, SoundOutput};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Result of an asynchronous secret-store operation.
pub enum SecretOutcome {
    Found(Zeroizing<String>),
    Missing,
    Saved,
    Forgotten,
    Failed(String),
}

/// An in-flight secret-store operation polled by the session loop.
pub struct PendingSecret(mpsc::Receiver<SecretOutcome>);

impl PendingSecret {
    /// Wraps the receiving half of a background secret-store job.
    #[must_use]
    pub fn new(rx: mpsc::Receiver<SecretOutcome>) -> Self {
        Self(rx)
    }

    /// Creates an already-completed operation with a known outcome.
    #[must_use]
    pub fn resolved(outcome: SecretOutcome) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(outcome);
        Self(rx)
    }

    /// Returns the outcome once ready, or `None` if still pending.
    #[must_use]
    pub fn poll(&self) -> Option<SecretOutcome> {
        self.0.try_recv().ok()
    }
}

/// Persistent storage for saved PC passwords.
pub trait SecretStore: Send + Sync {
    /// Looks up a remembered password for `pc`.
    fn lookup(&self, pc: Uuid) -> PendingSecret;
    /// Remembers `password` for `pc`, replacing any earlier entry.
    fn save(&self, pc: Uuid, password: Zeroizing<String>) -> PendingSecret;
    /// Deletes the remembered password for `pc`.
    fn forget(&self, pc: Uuid) -> PendingSecret;
}

/// An event from a physical keyboard listener.
#[derive(Debug, Clone)]
pub enum PhysicalKeyEvent {
    /// Physical key pressed or released by evdev keycode.
    Key { evdev: u32, pressed: bool },
    /// Caps Lock and Num Lock states changed.
    Modifiers { caps: bool, num: bool },
    /// Window gained keyboard focus, with keys already held down.
    Enter { keys: Vec<u32> },
    /// Window lost keyboard focus.
    Leave,
    /// Autorepeat event for the active key.
    Repeat { evdev: u32 },
}

/// Handle to an active physical keyboard grab.
pub trait PhysicalKeyboard: Send + Sync {
    /// Notifies the listener whether the session window is fullscreen.
    fn set_fullscreen(&self, fullscreen: bool);
}

/// Starts a platform-specific physical keyboard listener when supported.
pub trait KeySource: Send + Sync {
    /// Starts listening, pushing events to `sender` and waking the UI with `wakeup`.
    fn start(
        &self,
        sender: mpsc::Sender<PhysicalKeyEvent>,
        wakeup: Box<dyn Fn() + Send + Sync>,
    ) -> Option<Box<dyn PhysicalKeyboard>>;
}

/// Creates local audio output and microphone input streams for the RDP engine.
pub trait AudioFactory: Send + Sync {
    /// Creates a speaker output stream.
    fn sound_output(&self) -> Box<dyn SoundOutput>;
    /// Creates a microphone input stream controlled by `muted`.
    fn sound_input(&self, muted: Arc<AtomicBool>) -> Box<dyn SoundInput>;
}

/// Platform capabilities injected at application startup.
#[derive(Clone)]
pub struct PlatformPorts {
    pub secrets: Arc<dyn SecretStore>,
    pub keys: Arc<dyn KeySource>,
    pub audio: Arc<dyn AudioFactory>,
}

impl PlatformPorts {
    /// No-op platform implementations suitable for headless tests.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            secrets: Arc::new(NoopSecretStore),
            keys: Arc::new(NoopKeySource),
            audio: Arc::new(NoopAudioFactory),
        }
    }
}

/// In-memory no-op secret store for headless tests.
pub struct NoopSecretStore;

impl SecretStore for NoopSecretStore {
    fn lookup(&self, _pc: Uuid) -> PendingSecret {
        PendingSecret::resolved(SecretOutcome::Missing)
    }

    fn save(&self, _pc: Uuid, _password: Zeroizing<String>) -> PendingSecret {
        PendingSecret::resolved(SecretOutcome::Saved)
    }

    fn forget(&self, _pc: Uuid) -> PendingSecret {
        PendingSecret::resolved(SecretOutcome::Forgotten)
    }
}

struct NoopKeySource;

impl KeySource for NoopKeySource {
    fn start(
        &self,
        _sender: mpsc::Sender<PhysicalKeyEvent>,
        _wakeup: Box<dyn Fn() + Send + Sync>,
    ) -> Option<Box<dyn PhysicalKeyboard>> {
        None
    }
}

struct NoopAudioFactory;

impl AudioFactory for NoopAudioFactory {
    fn sound_output(&self) -> Box<dyn SoundOutput> {
        Box::new(NoopAudio)
    }

    fn sound_input(&self, _muted: Arc<AtomicBool>) -> Box<dyn SoundInput> {
        Box::new(NoopAudio)
    }
}

struct NoopAudio;

impl SoundOutput for NoopAudio {
    fn play(&mut self, _rate: u32, _channels: u16, _samples: &[i16]) {}
    fn set_volume(&mut self, _left: f32, _right: f32) {}
    fn stop(&mut self) {}
}

impl SoundInput for NoopAudio {
    fn start(
        &mut self,
        _rate: u32,
        _channels: u16,
        _packet_bytes: usize,
        _packets: remoport_rdp::MicrophonePackets,
    ) -> bool {
        false
    }

    fn stop(&mut self) {}
}
