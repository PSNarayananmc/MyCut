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
        Self { path: dir.join("secret.nvapi") }
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
        fs::read_to_string(&self.path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    fn delete(&self) -> Result<(), SecretError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// OS keyring store behind `os-keyring` (packaging enables it).
#[cfg(feature = "os-keyring")]
pub struct KeyringSecretStore {
    service: String,
    account: String,
}

#[cfg(feature = "os-keyring")]
impl KeyringSecretStore {
    #[must_use]
    pub fn new() -> Self {
        Self { service: "mycut".into(), account: "nim_api_key".into() }
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
        entry.set_password(value).map_err(|_| SecretError::KeyringUnavailable)
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
}

/// Resolve the active store: keyring when compiled in, file fallback always
/// works. `prefer_file` honors MYCUT_SECRET_FILE=1 (debug/CI).
#[must_use]
pub fn default_store(config_dir: &std::path::Path) -> Box<dyn SecretStore> {
    if std::env::var("MYCUT_SECRET_FILE").is_ok() {
        return Box::new(FileSecretStore::new(config_dir));
    }
    #[cfg(feature = "os-keyring")]
    {
        Box::new(KeyringSecretStore::new())
    }
    #[cfg(not(feature = "os-keyring"))]
    {
        Box::new(FileSecretStore::new(config_dir))
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
            let mode = fs::metadata(&dir.path().join("mycut/secret.nvapi")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secret file must be 0600");
        }
        store.delete().unwrap();
        assert!(store.get().is_none());
    }
}
