//! Sign-in credentials, remembered passwords, and session tokens.
//!
//! Encapsulates username and password inputs, keyring lookups,
//! credential caching for reconnects, and session cookies.
//!
//! This module contains pure credential logic and does not depend on UI (R3).

use std::sync::Arc;

use zeroize::{Zeroize, Zeroizing};

use remoport_rdp::ResumeCookie;

use crate::logging;
use crate::ports::{PendingSecret, SecretOutcome, SecretStore};

/// What the keyring reported when polled.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyringEvent {
    /// Password was found in the system keyring.
    Found,
    /// Password was remembered in the system keyring.
    Saved,
    /// Keyring is unavailable or returned an error.
    Failed(String),
}

/// Reason a sign-in attempt was rejected, formatted in the UI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInError {
    RefusedSavedPassword,
    RefusedCredentials,
}

/// The 12 fields tracking credentials, passwords, and reconnection tokens.
pub struct SignInState {
    /// Username typed or loaded from bookmark.
    pub username: String,
    /// Password entered in the form; zeroized when sent to connection.
    pub password: Zeroizing<String>,
    /// Whether "Remember in the system keyring" is ticked.
    pub remember: bool,
    /// Password retrieved from the system keyring.
    pub saved_password: Option<Zeroizing<String>>,
    /// Whether the last connection attempt used `saved_password`.
    pub used_saved_password: bool,
    /// Ongoing background keyring lookup/save/forget operation.
    pub keyring: Option<PendingSecret>,
    /// Password to remember in the keyring once connection succeeds.
    pub to_remember: Option<Zeroizing<String>>,
    /// Error from a failed sign-in attempt.
    pub sign_in_error: Option<SignInError>,
    /// Password kept in memory for reconnecting; wiped when session ends.
    pub session_password: Option<Zeroizing<String>>,
    /// Gateway password entered in the form; zeroized when sent.
    pub gateway_password: Zeroizing<String>,
    /// Gateway password kept in memory for reconnecting.
    pub session_gateway_password: Option<Zeroizing<String>>,
    /// Session cookie provided by the PC to resume a dropped connection.
    pub resume: Option<ResumeCookie>,
    /// Persistent store used for saved passwords.
    store: Arc<dyn SecretStore>,
}

impl SignInState {
    /// Creates a new sign-in state for a PC.
    #[must_use]
    pub fn new(
        username: String,
        saved: bool,
        bookmark_id: uuid::Uuid,
        store: Arc<dyn SecretStore>,
    ) -> Self {
        let keyring = saved.then(|| store.lookup(bookmark_id));
        Self {
            username,
            password: Zeroizing::new(String::new()),
            remember: false,
            saved_password: None,
            used_saved_password: false,
            keyring,
            to_remember: None,
            sign_in_error: None,
            session_password: None,
            gateway_password: Zeroizing::new(String::new()),
            session_gateway_password: None,
            resume: None,
            store,
        }
    }

    /// Polls the pending keyring operation, updating state and returning any user-facing event.
    pub fn poll_keyring(&mut self, pc_name: &str) -> Option<KeyringEvent> {
        let outcome = self.keyring.as_ref().and_then(PendingSecret::poll)?;
        self.keyring = None;
        match outcome {
            SecretOutcome::Found(password) => {
                logging::event(format_args!(
                    "A password for {pc_name} is saved in the keyring"
                ));
                self.saved_password = Some(password);
                Some(KeyringEvent::Found)
            }
            SecretOutcome::Missing => None,
            SecretOutcome::Saved => {
                logging::event(format_args!(
                    "Password for {pc_name} remembered in the keyring"
                ));
                Some(KeyringEvent::Saved)
            }
            SecretOutcome::Forgotten => {
                logging::event(format_args!("Saved password for {pc_name} forgotten"));
                None
            }
            SecretOutcome::Failed(error) => {
                logging::event(format_args!("Keyring unavailable: {error}"));
                Some(KeyringEvent::Failed(error))
            }
        }
    }

    /// Forgets the saved password for this PC.
    pub fn forget_saved(&mut self, bookmark_id: uuid::Uuid) {
        self.saved_password = None;
        self.keyring = Some(self.store.forget(bookmark_id));
    }

    /// Prepares passwords for connecting: extracts effective passwords, caches them for
    /// reconnection, and zeroizes the input fields.
    pub fn prepare_connect(
        &mut self,
        saved: bool,
        has_gateway: bool,
    ) -> (Zeroizing<String>, Option<Zeroizing<String>>) {
        self.used_saved_password = self.password.is_empty() && self.saved_password.is_some();
        let password = if self.used_saved_password {
            self.saved_password.clone().unwrap_or_default()
        } else {
            self.password.clone()
        };
        self.to_remember =
            (saved && self.remember && !self.password.is_empty()).then(|| self.password.clone());
        self.session_password = Some(password.clone());
        self.password.zeroize();

        let gw_pass = has_gateway.then(|| self.gateway_password.clone());
        self.session_gateway_password = gw_pass.clone();
        self.gateway_password.zeroize();

        self.resume = None;
        self.sign_in_error = None;

        (password, gw_pass)
    }

    /// Wipes credentials and cookies cached for reconnection.
    pub fn forget_session(&mut self) {
        self.session_password = None;
        self.session_gateway_password = None;
        self.resume = None;
    }

    /// Called when the session successfully connects: saves the password in keyring if requested.
    pub fn on_connected(&mut self, bookmark_id: uuid::Uuid) {
        if let Some(password) = self.to_remember.take() {
            self.keyring = Some(self.store.save(bookmark_id, password));
        }
    }

    /// Records the resumption cookie issued by the PC.
    pub fn on_resume_cookie(&mut self, cookie: ResumeCookie) {
        logging::event("The PC gave a cookie to resume this session after a drop");
        self.resume = Some(cookie);
    }

    /// Handles session termination cleanup for credential state.
    pub fn on_session_ended(&mut self) {
        self.to_remember = None;
    }

    /// Handles authentication failure: returns `true` if a saved password was used.
    pub fn on_auth_failed(&mut self) -> bool {
        let was_saved = self.used_saved_password;
        if was_saved {
            self.saved_password = None;
        }
        was_saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::NoopSecretStore;
    use uuid::Uuid;

    fn test_store() -> Arc<dyn SecretStore> {
        Arc::new(NoopSecretStore)
    }

    #[test]
    fn signin_state_new_initialization() {
        let id = Uuid::new_v4();
        let state = SignInState::new("alice".into(), false, id, test_store());
        assert_eq!(state.username, "alice");
        assert!(state.password.is_empty());
        assert!(!state.remember);
        assert!(state.saved_password.is_none());
        assert!(!state.used_saved_password);
        assert!(state.keyring.is_none());
        assert!(state.to_remember.is_none());
        assert!(state.session_password.is_none());
        assert!(state.session_gateway_password.is_none());
        assert!(state.resume.is_none());
    }

    #[test]
    fn prepare_connect_with_typed_password_wipes_input_and_caches_session() {
        let id = Uuid::new_v4();
        let mut state = SignInState::new("alice".into(), true, id, test_store());
        state.password = Zeroizing::new("secret123".into());
        state.remember = true;

        let (pass, gw) = state.prepare_connect(true, false);
        assert_eq!(&*pass, "secret123");
        assert!(gw.is_none());
        assert!(state.password.is_empty(), "input password must be zeroized");
        assert_eq!(
            state.session_password.as_deref().map(|s| s.as_str()),
            Some("secret123")
        );
        assert_eq!(
            state.to_remember.as_deref().map(|s| s.as_str()),
            Some("secret123")
        );
        assert!(!state.used_saved_password);
    }

    #[test]
    fn prepare_connect_with_saved_password() {
        let id = Uuid::new_v4();
        let mut state = SignInState::new("alice".into(), true, id, test_store());
        state.saved_password = Some(Zeroizing::new("saved_pass".into()));

        let (pass, _) = state.prepare_connect(true, false);
        assert_eq!(&*pass, "saved_pass");
        assert!(state.used_saved_password);
        assert!(state.to_remember.is_none());
    }

    #[test]
    fn forget_session_wipes_passwords_and_resume() {
        let id = Uuid::new_v4();
        let mut state = SignInState::new("alice".into(), false, id, test_store());
        state.session_password = Some(Zeroizing::new("p1".into()));
        state.session_gateway_password = Some(Zeroizing::new("p2".into()));

        state.forget_session();
        assert!(state.session_password.is_none());
        assert!(state.session_gateway_password.is_none());
        assert!(state.resume.is_none());
    }

    #[test]
    fn on_auth_failed_clears_saved_password_if_used() {
        let id = Uuid::new_v4();
        let mut state = SignInState::new("alice".into(), true, id, test_store());
        state.saved_password = Some(Zeroizing::new("saved".into()));
        state.prepare_connect(true, false);
        assert!(state.used_saved_password);

        let was_saved = state.on_auth_failed();
        assert!(was_saved);
        assert!(state.saved_password.is_none());
    }
}
