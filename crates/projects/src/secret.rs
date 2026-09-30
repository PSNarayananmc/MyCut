//! Secret storage: OS keyring (Secret Service) via the `os-keyring` feature,
//! with an always-available file fallback (0600). The key never reaches JS,
//! logs, or error messages.

use std::fs;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("keyring unavailable")]
    KeyringUnavailable,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub trait SecretStore: Send + Sync {
    ///
    /// # Errors
    /// [`SecretError`] surfaced to the UI without the secret value.
    fn set(&self, value: &str) -> Result<(), SecretError>;
    #[must_use]
    fn get(&self) -> Option<String>;
    ///
    /// # Errors
    /// [`SecretError`]
    fn delete(&self) -> Result<(), SecretError>;
    /// Which concrete backend a successful read/write went through.
    #[must_use]
    fn backend(&self) -> SecretBackend;
}

/// File-backed store: `<config_dir>/mycut/secret.nvapi` with 0600 perms.
pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    #[must_use]
    pub fn new(config_dir: &std::path::Path) -> Self {
        let dir = config_dir.join("mycut");
        let _ = fs::create_dir_all(&dir);
        Self {
            path: dir.join("secret.nvapi"),
        }
    }
}

impl SecretStore for FileSecretStore {
    fn set(&self, value: &str) -> Result<(), SecretError> {
        fs::write(&self.path, value.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn get(&self) -> Option<String> {
        fs::read_to_string(&self.path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn delete(&self) -> Result<(), SecretError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn backend(&self) -> SecretBackend {
        SecretBackend::File
    }
}

/// OS keyring store behind `os-keyring` (packaging enables it).
#[cfg(feature = "os-keyring")]
pub struct KeyringSecretStore {
    service: String,
    account: String,
}

/// Report which backend a successful read/write went through, so the
/// Settings UI can show "Key stored (OS keyring)" vs "Key stored (file)".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretBackend {
    Keyring,
    File,
    None,
}

impl SecretBackend {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Keyring => "OS keyring",
            Self::File => "file (0600)",
            Self::None => "none",
        }
    }
}

#[cfg(feature = "os-keyring")]
impl KeyringSecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            service: "mycut".into(),
            account: "nim_api_key".into(),
        }
    }
}

#[cfg(feature = "os-keyring")]
impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "os-keyring")]
impl SecretStore for KeyringSecretStore {
    fn set(&self, value: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|_| SecretError::KeyringUnavailable)?;
        entry
            .set_password(value)
            .map_err(|_| SecretError::KeyringUnavailable)
    }

    fn get(&self) -> Option<String> {
        let entry = keyring::Entry::new(&self.service, &self.account).ok()?;
        entry.get_password().ok()
    }

    fn delete(&self) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|_| SecretError::KeyringUnavailable)?;
        let _ = entry.delete_credential();
        Ok(())
    }

    fn backend(&self) -> SecretBackend {
        SecretBackend::Keyring
    }
}

/// Resolve the active store.
///
/// With `os-keyring` enabled, this returns a [`FallbackSecretStore`] that
/// tries the OS keyring first and falls back to a 0600 file when the keyring
/// is unavailable (e.g. headless CI, Ubuntu without `gnome-keyring-daemon`,
/// containers without a Secret Service). This is the fix for the
/// "no-api-key" bug: previously, when the keyring silently failed, the key
/// was reported saved but never persisted, and the next `get()` returned
/// `None` → `unwrap_or_default()` → empty string → `"no-api-key"` error.
///
/// Without `os-keyring`, only the file store is available.
///
/// `MYCUT_SECRET_FILE=1` forces the file store (debug/CI).
#[must_use]
pub fn default_store(config_dir: &std::path::Path) -> Box<dyn SecretStore> {
    if std::env::var("MYCUT_SECRET_FILE").is_ok() {
        return Box::new(FileSecretStore::new(config_dir));
    }
    #[cfg(feature = "os-keyring")]
    {
        Box::new(FallbackSecretStore::new(config_dir))
    }
    #[cfg(not(feature = "os-keyring"))]
    {
        Box::new(FileSecretStore::new(config_dir))
    }
}

/// Composite store: keyring first, file fallback. The key ALWAYS lands
/// somewhere readable, even on systems without a Secret Service daemon.
/// `backend()` reports which backend the most recent successful operation
/// went through (Keyring if it answered at all, else File).
#[cfg(feature = "os-keyring")]
pub struct FallbackSecretStore {
    keyring: KeyringSecretStore,
    file: FileSecretStore,
}

#[cfg(feature = "os-keyring")]
impl FallbackSecretStore {
    #[must_use]
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            keyring: KeyringSecretStore::new(),
            file: FileSecretStore::new(config_dir),
        }
    }
}

#[cfg(feature = "os-keyring")]
impl SecretStore for FallbackSecretStore {
    fn set(&self, value: &str) -> Result<(), SecretError> {
        // Try keyring; on any failure, write to the file store (which never
        // fails outside of disk-full / permission errors). The user should
        // never see "keyring unavailable" — the key persists either way.
        match self.keyring.set(value) {
            Ok(()) => {
                // Mirror to file too, so a future read without a keyring
                // (e.g. the user disables gnome-keyring) still finds the key.
                // Best-effort; ignore file errors when keyring succeeded.
                let _ = self.file.set(value);
                Ok(())
            }
            Err(_) => self.file.set(value),
        }
    }

    fn get(&self) -> Option<String> {
        // Try keyring first; if it answers with a value, use it. If it
        // answers None (key not set in keyring) or errors (no daemon), fall
        // back to the file. This way "saved in keyring then daemon died"
        // still finds the key.
        match self.keyring.get() {
            Some(v) if !v.is_empty() => Some(v),
            _ => self.file.get(),
        }
    }

    fn delete(&self) -> Result<(), SecretError> {
        // Delete from both, ignoring NotFound on either.
        let _ = self.keyring.delete();
        self.file.delete()
    }

    fn backend(&self) -> SecretBackend {
        // Report whichever backend actually holds the key right now.
        if self.keyring.get().is_some() {
            SecretBackend::Keyring
        } else if self.file.get().is_some() {
            SecretBackend::File
        } else {
            SecretBackend::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip_and_perms() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path());
        assert!(store.get().is_none());
        store.set("nvapi-test-123").unwrap();
        assert_eq!(store.get().as_deref(), Some("nvapi-test-123"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dir.path().join("mycut/secret.nvapi"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secret file must be 0600");
        }
        store.delete().unwrap();
        assert!(store.get().is_none());
    }

    #[cfg(feature = "os-keyring")]
    #[test]
    fn fallback_store_persists_when_keyring_unavailable() {
        // With no Secret Service daemon in CI, KeyringSecretStore::set returns
        // KeyringUnavailable. The fallback must write to the file store so
        // the next get() finds the key. This is the regression test for the
        // original "no-api-key" bug.
        let dir = tempfile::tempdir().unwrap();
        let store = FallbackSecretStore::new(dir.path());
        assert!(store.get().is_none(), "fresh store must be empty");
        store.set("nvapi-canary-xyz").unwrap();
        // Read back through the same store — keyring.get() may return None
        // in this environment, but file.get() must have the value.
        assert_eq!(
            store.get().as_deref(),
            Some("nvapi-canary-xyz"),
            "fallback must persist the key when keyring is unavailable"
        );
        // The file must exist with 0600 perms.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("mycut/secret.nvapi");
            assert!(path.exists(), "file fallback must create the file");
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "fallback file must be 0600");
        }
        store.delete().unwrap();
        assert!(store.get().is_none(), "delete must clear all backends");
    }

    #[test]
    fn file_store_backend_label() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path());
        assert_eq!(store.backend(), SecretBackend::File);
        assert_eq!(SecretBackend::File.label(), "file (0600)");
        assert_eq!(SecretBackend::Keyring.label(), "OS keyring");
    }
}
