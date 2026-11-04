//! A keyboard of our own on the window's Wayland connection, for what egui on
//! Linux does not report, while a session is connected:
//! - the lock keys: when Caps Lock or Num Lock change here, the PC follows, so
//!   typing means the same on both;
//! - in full screen, the desktop's own shortcuts go to the PC. The compositor
//!   is asked to hold them back for this window
//!   (`zwp_keyboard_shortcuts_inhibit_v1`), so Alt+Tab and the like arrive
//!   through egui like any other key, and the Windows key, which egui never
//!   reports, comes through this keyboard.
//!
//! Ctrl+Alt+Enter leaves full screen and hands the shortcuts back. On X11, or
//! where the compositor does not offer this, the Keys menu still sends them.
//!
//! ponytail: only the main window can ask, because eframe does not hand out
//! the other screens' windows; a further screen's window keeps the desktop's
//! shortcuts until that changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use remoport_rdp::{Command, Commander};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1::{
    self, ZwpKeyboardShortcutsInhibitorV1,
};

use crate::keymap::{self, Scancode};
use crate::logging;

/// The main window's Wayland display and surface, as addresses.
#[derive(Clone, Copy)]
struct WindowPointers {
    display: usize,
    surface: usize,
}

static WINDOW: OnceLock<Option<WindowPointers>> = OnceLock::new();

/// Notes the main window's handles, the first time it is drawn. Nothing is
/// noted on X11.
pub fn remember_window(frame: &eframe::Frame) {
    WINDOW.get_or_init(|| {
        let display = frame.display_handle().ok()?.as_raw();
        let window = frame.window_handle().ok()?.as_raw();
        match (display, window) {
            (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window)) => {
                Some(WindowPointers {
                    display: display.display.as_ptr() as usize,
                    surface: window.surface.as_ptr() as usize,
                })
            }
            _ => None,
        }
    });
}

/// The session's keyboard, for as long as this lives.
pub struct KeyGrab {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeyGrab {
    /// Starts holding the shortcuts back, or says why it cannot.
    pub fn start(commander: Commander) -> Option<Self> {
        let Some(Some(window)) = WINDOW.get().copied() else {
            logging::event("This desktop is not Wayland: use the Keys menu for reserved keys");
            return None;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(false));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let shared = Shared {
            active: Arc::clone(&active),
            fullscreen: Arc::clone(&fullscreen),
        };
        let stopped = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("remoport-keys".to_string())
            .spawn(move || {
                if let Err(why) = run(window, commander, &stopped, shared) {
                    logging::event(format_args!(
                        "Reserved keys stay with this desktop ({why}): use the Keys menu"
                    ));
                }
            })
            .ok()?;
        Some(Self {
            stop,
            active,
            fullscreen,
            thread: Some(thread),
        })
    }

    /// Whether the session has the keyboard, so keys go to the PC.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    /// Whether the window is in full screen, so the desktop's shortcuts are
    /// held back and go to the PC.
    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.fullscreen.store(fullscreen, Ordering::Relaxed);
    }
}

impl Drop for KeyGrab {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The set-1 scancode for a key egui does not report, by its Linux key code.
fn scancode(evdev: u32) -> Option<Scancode> {
    match evdev {
        125 => Some(keymap::WINDOWS),
        126 => Some(keymap::WINDOWS_RIGHT),
        127 => Some(keymap::MENU),
        99 => Some(keymap::PRINT_SCREEN),
        _ => None,
    }
}

/// A key's name for the log.
fn name(code: Scancode) -> &'static str {
    match code {
        keymap::MENU => "the Menu key",
        keymap::PRINT_SCREEN => "Print Screen",
        _ => "the Windows key",
    }
}

/// Set by the session each frame.
struct Shared {
    active: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
}

/// Caps Lock and Num Lock from a Wayland `mods_locked` mask. XKB fixes Lock as
/// its second real modifier, and layouts put Num Lock on Mod2, the fifth.
const fn lock_keys(mods_locked: u32) -> (bool, bool) {
    (mods_locked & (1 << 1) != 0, mods_locked & (1 << 4) != 0)
}

struct State {
    commander: Commander,
    shared: Shared,
    keyboard: Option<WlKeyboard>,
    held: Vec<Scancode>,
    /// The lock keys the PC was last told about.
    locks: Option<(bool, bool)>,
}

impl State {
    fn send(&self, (scancode, extended): Scancode, pressed: bool) {
        self.commander.send(Command::Key {
            scancode,
            extended,
            pressed,
        });
    }

    fn release_all(&mut self) {
        for key in std::mem::take(&mut self.held) {
            self.send(key, false);
        }
    }
}

fn run(
    window: WindowPointers,
    commander: Commander,
    stop: &AtomicBool,
    shared: Shared,
) -> Result<(), String> {
    // SAFETY: both addresses come from eframe's handles for the main window,
    // whose display and surface outlive this thread: `KeyGrab` joins it when
    // dropped, and the session is dropped in `on_exit`, before the window. The
    // backend does not take ownership of a foreign display or proxy.
    let (connection, surface) = unsafe {
        let connection =
            Connection::from_backend(Backend::from_foreign_display(window.display as *mut _));
        let id = ObjectId::from_ptr(WlSurface::interface(), window.surface as *mut _)
            .map_err(|_| "the window's surface is not usable".to_string())?;
        let surface = WlSurface::from_id(&connection, id)
            .map_err(|_| "the window's surface is not usable".to_string())?;
        (connection, surface)
    };
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).map_err(|e| e.to_string())?;
    let handle = queue.handle();
    let seat: WlSeat = globals
        .bind(&handle, 3..=5, ())
        .map_err(|_| "no keyboard seat".to_string())?;
    // Without this the lock keys still work; only the shortcuts stay out of reach.
    let manager: Option<ZwpKeyboardShortcutsInhibitManagerV1> =
        globals.bind(&handle, 1..=1, ()).ok();
    if manager.is_none() {
        logging::event("This desktop keeps its reserved keys: use the Keys menu");
    }
    let mut inhibitor: Option<ZwpKeyboardShortcutsInhibitorV1> = None;
    let mut state = State {
        commander,
        shared,
        keyboard: None,
        held: Vec::new(),
        locks: None,
    };

    while !stop.load(Ordering::Relaxed) {
        let fullscreen = state.shared.fullscreen.load(Ordering::Relaxed);
        if let (true, None, Some(manager)) = (fullscreen, &inhibitor, &manager) {
            inhibitor = Some(manager.inhibit_shortcuts(&surface, &seat, &handle, ()));
        } else if !fullscreen {
            if let Some(held_back) = inhibitor.take() {
                held_back.destroy();
                logging::event("Reserved keys are back with this desktop");
            }
        }
        queue
            .dispatch_pending(&mut state)
            .map_err(|e| e.to_string())?;
        let _ = connection.flush();
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let readable = {
            let mut fds = [PollFd::new(guard.connection_fd(), PollFlags::POLLIN)];
            poll(&mut fds, PollTimeout::from(50u8)).is_ok_and(|ready| ready > 0)
        };
        if readable {
            guard.read().map_err(|e| e.to_string())?;
        }
    }

    state.release_all();
    if let Some(held_back) = inhibitor.take() {
        held_back.destroy();
        logging::event("Reserved keys are back with this desktop");
    }
    if let Some(keyboard) = state.keyboard.take() {
        keyboard.release();
    }
    if let Some(manager) = manager {
        manager.destroy();
    }
    // A request only seats of this version know.
    if seat.version() >= 5 {
        seat.release();
    }
    let _ = connection.flush();
    Ok(())
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        // Asking for a keyboard the seat lacks would end the whole connection.
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(handle, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                let Some(code) = scancode(key) else {
                    return;
                };
                let pressed = key_state == wl_keyboard::KeyState::Pressed;
                // In a window these keys are the desktop's own.
                if !state.shared.fullscreen.load(Ordering::Relaxed) {
                    if !pressed && state.held.contains(&code) {
                        state.held.retain(|held| *held != code);
                        state.send(code, false);
                    }
                } else if pressed && state.shared.active.load(Ordering::Relaxed) {
                    logging::event(format_args!("Sent {}", name(code)));
                    state.held.push(code);
                    state.send(code, true);
                } else if pressed {
                    logging::event(format_args!(
                        "{} not sent: click the remote screen to give it the keyboard",
                        name(code)
                    ));
                } else if state.held.contains(&code) {
                    state.held.retain(|held| *held != code);
                    state.send(code, false);
                }
            }
            wl_keyboard::Event::Modifiers { mods_locked, .. } => {
                let locks = lock_keys(mods_locked);
                if state.locks != Some(locks) {
                    state.locks = Some(locks);
                    let (caps, num) = locks;
                    logging::event(format_args!(
                        "Lock keys here, sent to the PC: Caps Lock {}, Num Lock {}",
                        if caps { "on" } else { "off" },
                        if num { "on" } else { "off" },
                    ));
                    state.commander.send(Command::LockKeys {
                        caps,
                        num,
                        scroll: false,
                    });
                }
            }
            wl_keyboard::Event::Leave { .. } => state.release_all(),
            _ => {}
        }
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitManagerV1,
        _: <ZwpKeyboardShortcutsInhibitManagerV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => {
                logging::event("Reserved keys go to the PC while in full screen");
            }
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => {
                logging::event("This desktop took its reserved keys back");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_keys_come_from_their_fixed_modifier_bits() {
        assert_eq!(lock_keys(0), (false, false));
        assert_eq!(lock_keys(0b0_0010), (true, false));
        assert_eq!(lock_keys(0b1_0000), (false, true));
        assert_eq!(lock_keys(0b1_0010), (true, true));
        // Shift and Ctrl latched or locked say nothing about either.
        assert_eq!(lock_keys(0b0_0101), (false, false));
    }

    #[test]
    fn only_the_keys_egui_drops_are_taken_here() {
        assert_eq!(scancode(125), Some(keymap::WINDOWS));
        assert_eq!(scancode(126), Some(keymap::WINDOWS_RIGHT));
        assert_eq!(scancode(127), Some(keymap::MENU));
        assert_eq!(scancode(99), Some(keymap::PRINT_SCREEN));
        // Tab and the letters arrive through egui; sending them here too
        // would type them twice.
        for evdev in [15, 19, 30, 56, 29] {
            assert_eq!(scancode(evdev), None);
        }
    }
}
