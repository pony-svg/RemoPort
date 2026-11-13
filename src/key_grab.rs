//! A keyboard of our own on the window's Wayland connection:
//! captures physical key events, lock states, focus enter/leave, and
//! desktop shortcut inhibition while in full screen. Events are forwarded
//! to the UI thread via a channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::thread::JoinHandle;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
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

use crate::logging;

/// An event from the Wayland keyboard listener sent to the UI thread.
#[derive(Debug, Clone)]
pub enum WaylandKeyEvent {
    /// Physical key pressed or released by evdev keycode.
    Key { evdev: u32, pressed: bool },
    /// Caps Lock and Num Lock states changed.
    Modifiers { caps: bool, num: bool },
    /// Window gained keyboard focus, with keys already held down.
    Enter { keys: Vec<u32> },
    /// Window lost keyboard focus.
    Leave,
    /// Autorepeat event for the active key.
    #[allow(dead_code)]
    Repeat { evdev: u32 },
}

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

/// The session's keyboard listener on Wayland.
pub struct KeyGrab {
    stop: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeyGrab {
    /// Starts capturing physical keyboard events on Wayland, sending them through `sender`
    /// and waking the UI thread with `wakeup`.
    pub fn start(
        sender: mpsc::Sender<WaylandKeyEvent>,
        wakeup: Box<dyn Fn() + Send + Sync>,
    ) -> Option<Self> {
        let Some(Some(window)) = WINDOW.get().copied() else {
            logging::event("This desktop is not Wayland: using window keyboard fallback");
            return None;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let shared = Shared {
            fullscreen: Arc::clone(&fullscreen),
        };
        let stopped = Arc::clone(&stop);
        let wakeup: Arc<dyn Fn() + Send + Sync> = Arc::from(wakeup);
        let thread = std::thread::Builder::new()
            .name("remoport-keys".to_string())
            .spawn(move || {
                if let Err(why) = run(window, sender, wakeup, &stopped, shared) {
                    logging::event(format_args!(
                        "Physical keys unavailable on Wayland ({why}): using window fallback"
                    ));
                }
            })
            .ok()?;
        Some(Self {
            stop,
            fullscreen,
            thread: Some(thread),
        })
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

/// Set by the session each frame.
struct Shared {
    fullscreen: Arc<AtomicBool>,
}

/// Caps Lock and Num Lock from a Wayland `mods_locked` mask. XKB fixes Lock as
/// its second real modifier, and layouts put Num Lock on Mod2, the fifth.
const fn lock_keys(mods_locked: u32) -> (bool, bool) {
    (mods_locked & (1 << 1) != 0, mods_locked & (1 << 4) != 0)
}

struct State {
    sender: mpsc::Sender<WaylandKeyEvent>,
    wakeup: Arc<dyn Fn() + Send + Sync>,
    shared: Shared,
    keyboard: Option<WlKeyboard>,
    locks: Option<(bool, bool)>,
}

fn run(
    window: WindowPointers,
    sender: mpsc::Sender<WaylandKeyEvent>,
    wakeup: Arc<dyn Fn() + Send + Sync>,
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
        sender,
        wakeup,
        shared,
        keyboard: None,
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

    let _ = state.sender.send(WaylandKeyEvent::Leave);
    (state.wakeup)();

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
                let pressed = key_state == wl_keyboard::KeyState::Pressed;
                let _ = state.sender.send(WaylandKeyEvent::Key {
                    evdev: key,
                    pressed,
                });
                (state.wakeup)();
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
                    let _ = state.sender.send(WaylandKeyEvent::Modifiers { caps, num });
                    (state.wakeup)();
                }
            }
            wl_keyboard::Event::Enter { keys, .. } => {
                let keys = keys
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_ne_bytes(*b))
                    .collect();
                let _ = state.sender.send(WaylandKeyEvent::Enter { keys });
                (state.wakeup)();
            }
            wl_keyboard::Event::Leave { .. } => {
                let _ = state.sender.send(WaylandKeyEvent::Leave);
                (state.wakeup)();
            }
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
        assert!(crate::input::is_desktop_reserved(125));
        assert!(crate::input::is_desktop_reserved(126));
        assert!(crate::input::is_desktop_reserved(127));
        assert!(!crate::input::is_desktop_reserved(15));
        assert!(!crate::input::is_desktop_reserved(30));
    }
}
