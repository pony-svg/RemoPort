//! Passwords the user chose to remember: kept only in the system keyring
//! (Secret Service, i.e. KWallet or GNOME Keyring), one per saved PC, moved to
//! it over an encrypted D-Bus session. Never in Remoport's own files, never in
//! the log.
//!
//! A keyring call can block while the keyring asks to be unlocked, so each one
//! runs on its own thread and reports back through [`Pending`].

use std::sync::mpsc;

use uuid::Uuid;
use zeroize::Zeroizing;

const SERVICE: &str = "app.remoport.Remoport";

pub enum Outcome {
    Found(Zeroizing<String>),
    Missing,
    Saved,
    Forgotten,
    Failed(String),
}

/// A keyring call in progress.
pub struct Pending(mpsc::Receiver<Outcome>);

impl Pending {
    pub fn poll(&self) -> Option<Outcome> {
        self.0.try_recv().ok()
    }
}

fn run(job: impl FnOnce() -> Outcome + Send + 'static) -> Pending {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    Pending(rx)
}

fn entry(pc: Uuid) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &pc.to_string())
}

/// The password remembered for `pc`, if any.
pub fn lookup(pc: Uuid) -> Pending {
    run(move || match entry(pc).and_then(|e| e.get_password()) {
        Ok(password) => Outcome::Found(Zeroizing::new(password)),
        Err(keyring::Error::NoEntry) => Outcome::Missing,
        Err(e) => Outcome::Failed(e.to_string()),
    })
}

/// Remembers `password` for `pc`, replacing any earlier one.
pub fn save(pc: Uuid, password: Zeroizing<String>) -> Pending {
    run(
        move || match entry(pc).and_then(|e| e.set_password(&password)) {
            Ok(()) => Outcome::Saved,
            Err(e) => Outcome::Failed(e.to_string()),
        },
    )
}

/// Forgets the password remembered for `pc`; nothing remembered is fine.
pub fn forget(pc: Uuid) -> Pending {
    run(
        move || match entry(pc).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Outcome::Forgotten,
            Err(e) => Outcome::Failed(e.to_string()),
        },
    )
}
