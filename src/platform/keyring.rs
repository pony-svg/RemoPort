//! Passwords the user chose to remember: kept only in the system keyring
//! (Secret Service, i.e. KWallet or GNOME Keyring), one per saved PC, moved to
//! it over an encrypted D-Bus session. Never in Remoport's own files, never in
//! the log.
//!
//! A keyring call can block while the keyring asks to be unlocked, so each one
//! runs on its own thread and reports back through [`PendingSecret`].

use std::sync::mpsc;

use uuid::Uuid;
use zeroize::Zeroizing;

use remoport_core::ports::{PendingSecret, SecretOutcome, SecretStore};

const SERVICE: &str = "app.remoport.Remoport";

/// System keyring adapter backed by Secret Service (`keyring` crate).
pub struct SystemKeyring;

impl SecretStore for SystemKeyring {
    fn lookup(&self, pc: Uuid) -> PendingSecret {
        lookup(pc)
    }

    fn save(&self, pc: Uuid, password: Zeroizing<String>) -> PendingSecret {
        save(pc, password)
    }

    fn forget(&self, pc: Uuid) -> PendingSecret {
        forget(pc)
    }
}

fn run(job: impl FnOnce() -> SecretOutcome + Send + 'static) -> PendingSecret {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(job());
    });
    PendingSecret::new(rx)
}

fn entry(pc: Uuid) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, &pc.to_string())
}

/// The password remembered for `pc`, if any.
pub fn lookup(pc: Uuid) -> PendingSecret {
    run(move || match entry(pc).and_then(|e| e.get_password()) {
        Ok(password) => SecretOutcome::Found(Zeroizing::new(password)),
        Err(keyring::Error::NoEntry) => SecretOutcome::Missing,
        Err(e) => SecretOutcome::Failed(e.to_string()),
    })
}

/// Remembers `password` for `pc`, replacing any earlier one.
pub fn save(pc: Uuid, password: Zeroizing<String>) -> PendingSecret {
    run(
        move || match entry(pc).and_then(|e| e.set_password(&password)) {
            Ok(()) => SecretOutcome::Saved,
            Err(e) => SecretOutcome::Failed(e.to_string()),
        },
    )
}

/// Forgets the password remembered for `pc`; nothing remembered is fine.
pub fn forget(pc: Uuid) -> PendingSecret {
    run(
        move || match entry(pc).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => SecretOutcome::Forgotten,
            Err(e) => SecretOutcome::Failed(e.to_string()),
        },
    )
}
