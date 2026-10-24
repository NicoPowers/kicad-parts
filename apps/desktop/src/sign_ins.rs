//! Keeps each team's sign-in in the operating system's credential store,
//! so the person stays signed in after a restart: Windows Credential
//! Manager, the macOS Keychain, or the Secret Service on Linux (GNOME
//! Keyring or KWallet). It holds the team server's renewable token, never
//! the password, and nothing is written to a file.
//!
//! This is the only code that touches the real store, and it lives in the
//! app so that no test can reach it. Tests and journeys use a store in
//! memory instead (`--sign-ins=memory`). A journey that restarts the app
//! with a sign-in kept uses a file in its own data folder
//! (`--sign-ins=test-file`), which only debug builds offer.

use catalog_browser::teams::{MemorySignIns, SignInStore, SignIns};
use std::path::Path;
use std::sync::Arc;

/// The service name each sign-in is kept under; the account name is the
/// team server's address.
const SERVICE: &str = "org.kicadparts.desktop";

/// The operating system's credential store, if it works on this computer;
/// otherwise sign-ins kept in memory until the app closes, with the reason
/// the app says once.
pub fn system_or_memory() -> SignIns {
    match SystemSignIns.read("https://probe.kicadparts.invalid") {
        Ok(_) => SignIns::kept_in(Arc::new(SystemSignIns)),
        Err(problem) => SignIns::until_closed(format!(
            "this computer has no password store the app can use, such as Windows Credential Manager, the macOS Keychain, or GNOME Keyring or KWallet on Linux ({problem})"
        )),
    }
}

/// What `--sign-ins` asks for, for tests: `memory` keeps sign-ins in
/// memory as if the store worked; `until-closed` acts as a computer with
/// no store. In debug builds, `test-file` keeps them in a file in
/// `data_dir`, so a journey can restart the app still signed in.
pub fn for_tests(choice: &str, data_dir: Option<&Path>) -> Result<SignIns, String> {
    match choice {
        "memory" => Ok(SignIns::kept_in(Arc::new(MemorySignIns::default()))),
        "until-closed" => Ok(SignIns::until_closed(
            "the app was started with --sign-ins=until-closed",
        )),
        #[cfg(debug_assertions)]
        "test-file" => {
            let data_dir = data_dir.ok_or("--sign-ins=test-file needs --data-dir")?;
            Ok(SignIns::kept_in(Arc::new(test_file::FileSignIns {
                file: data_dir.join("test-sign-ins.json"),
            })))
        }
        other => {
            let _ = data_dir;
            Err(format!(
                "--sign-ins takes memory or until-closed, not {other}"
            ))
        }
    }
}

/// Sign-ins in a file, for journeys that restart the app with a sign-in
/// kept. Never in a release build: the app keeps tokens only in the
/// system's password store (ADR 0018). The tokens are for disposable test
/// servers on 127.0.0.1.
#[cfg(debug_assertions)]
mod test_file {
    use catalog_browser::teams::SignInStore;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    pub struct FileSignIns {
        pub file: PathBuf,
    }

    /// One store writes at a time.
    static TURN: Mutex<()> = Mutex::new(());

    impl FileSignIns {
        fn all(&self) -> BTreeMap<String, String> {
            std::fs::read(&self.file)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default()
        }

        fn change(&self, change: impl FnOnce(&mut BTreeMap<String, String>)) -> Result<(), String> {
            let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
            let mut all = self.all();
            change(&mut all);
            let json = serde_json::to_vec(&all).map_err(|e| e.to_string())?;
            std::fs::write(&self.file, json).map_err(|e| e.to_string())
        }
    }

    impl SignInStore for FileSignIns {
        fn read(&self, server_url: &str) -> Result<Option<String>, String> {
            Ok(self.all().get(server_url).cloned())
        }

        fn keep(&self, server_url: &str, token: &str) -> Result<(), String> {
            self.change(|all| {
                all.insert(server_url.to_owned(), token.to_owned());
            })
        }

        fn forget(&self, server_url: &str) -> Result<(), String> {
            self.change(|all| {
                all.remove(server_url);
            })
        }
    }
}

struct SystemSignIns;

impl SystemSignIns {
    fn entry(server_url: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, server_url).map_err(|error| error.to_string())
    }
}

impl SignInStore for SystemSignIns {
    fn read(&self, server_url: &str) -> Result<Option<String>, String> {
        match Self::entry(server_url)?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn keep(&self, server_url: &str, token: &str) -> Result<(), String> {
        Self::entry(server_url)?
            .set_password(token)
            .map_err(|error| error.to_string())
    }

    fn forget(&self, server_url: &str) -> Result<(), String> {
        match Self::entry(server_url)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_contract::SignInsKept;

    #[test]
    fn tests_choose_a_store_in_memory() {
        // Never the system's store: these build stores in memory only.
        let kept = |choice| for_tests(choice, None).map(|sign_ins| sign_ins.kept());
        assert_eq!(kept("memory"), Ok(SignInsKept::AfterRestart));
        assert!(matches!(
            kept("until-closed"),
            Ok(SignInsKept::UntilClosed { .. })
        ));
        assert!(for_tests("system", None).is_err());
        // A file only in the journey's own data folder.
        assert!(for_tests("test-file", None).is_err());
    }
}
