//! Where each team's renewable sign-in (its PocketBase token) is kept
//! between runs, so the person stays signed in after a restart.
//!
//! The desktop app keeps them in the operating system's credential store:
//! Windows Credential Manager, the macOS Keychain, or the Secret Service on
//! Linux (see `apps/desktop/src/sign_ins.rs`). That code lives in the app,
//! not here, so no test can reach the person's real store. Tests use
//! [`MemorySignIns`]. A token or password is never written to a file.

use desktop_contract::SignInsKept;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A place that keeps one token per team server.
pub trait SignInStore: Send + Sync {
    /// The token kept for `server_url`, if there is one.
    fn read(&self, server_url: &str) -> Result<Option<String>, String>;
    /// Keeps `token` for `server_url`, in place of any earlier one.
    fn keep(&self, server_url: &str, token: &str) -> Result<(), String>;
    /// Forgets the token for `server_url`. Forgetting one that is not
    /// there is not a failure.
    fn forget(&self, server_url: &str) -> Result<(), String>;
}

/// Keeps tokens in this run's memory only.
#[derive(Default)]
pub struct MemorySignIns {
    tokens: Mutex<HashMap<String, String>>,
}

impl MemorySignIns {
    fn tokens(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.tokens
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl SignInStore for MemorySignIns {
    fn read(&self, server_url: &str) -> Result<Option<String>, String> {
        Ok(self.tokens().get(server_url).cloned())
    }

    fn keep(&self, server_url: &str, token: &str) -> Result<(), String> {
        self.tokens()
            .insert(server_url.to_owned(), token.to_owned());
        Ok(())
    }

    fn forget(&self, server_url: &str) -> Result<(), String> {
        self.tokens().remove(server_url);
        Ok(())
    }
}

/// The store the teams keep their sign-ins in, and whether those outlive
/// the app.
#[derive(Clone)]
pub struct SignIns {
    pub(super) store: Arc<dyn SignInStore>,
    pub(super) kept: SignInsKept,
}

impl SignIns {
    /// Sign-ins kept in `store`, which outlives the app.
    pub fn kept_in(store: Arc<dyn SignInStore>) -> Self {
        Self {
            store,
            kept: SignInsKept::AfterRestart,
        }
    }

    /// Whether these sign-ins outlive the app, and if not, why.
    pub fn kept(&self) -> SignInsKept {
        self.kept.clone()
    }

    /// Sign-ins kept in memory only, for the plain `reason` the app says
    /// once, such as no credential store on this computer.
    pub fn until_closed(reason: impl Into<String>) -> Self {
        Self {
            store: Arc::new(MemorySignIns::default()),
            kept: SignInsKept::UntilClosed {
                reason: reason.into(),
            },
        }
    }
}
