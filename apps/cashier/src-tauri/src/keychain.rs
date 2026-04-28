use crate::error::{AppError, AppResult};

/// Reverse-DNS service identifier used as the OS keychain namespace.
pub(crate) const SERVICE: &str = "com.lofi-pos.cashier";

/// Abstraction over OS-managed secret storage. `MemKeyStore` (cfg(test))
/// backs unit tests; `OsKeyStore` is the production impl.
pub trait KeyStore: Send + Sync {
    /// Fetch a secret by name, returning `Ok(None)` if absent.
    fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>>;
    /// Insert or overwrite a secret by name.
    fn set(&self, name: &str, value: &[u8]) -> AppResult<()>;
    /// Remove a secret by name; absent entries are not an error.
    fn delete(&self, name: &str) -> AppResult<()>;
}

/// Production `KeyStore` backed by the platform keyring (Keychain / Secret
/// Service / Credential Manager) via the `keyring` crate.
pub struct OsKeyStore {
    service: String,
}

impl OsKeyStore {
    /// Build an `OsKeyStore` scoped to the given reverse-DNS service name.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl KeyStore for OsKeyStore {
    fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        match entry.get_secret() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Keychain(e.to_string())),
        }
    }
    fn set(&self, name: &str, value: &[u8]) -> AppResult<()> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        entry
            .set_secret(value)
            .map_err(|e| AppError::Keychain(e.to_string()))
    }
    fn delete(&self, name: &str) -> AppResult<()> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AppError::Keychain(e.to_string())),
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MemKeyStore(Mutex<HashMap<String, Vec<u8>>>);

    impl KeyStore for MemKeyStore {
        fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &[u8]) -> AppResult<()> {
            self.0.lock().unwrap().insert(name.into(), value.into());
            Ok(())
        }
        fn delete(&self, name: &str) -> AppResult<()> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::MemKeyStore;

    #[test]
    fn set_get_roundtrip() {
        let ks = MemKeyStore::default();
        ks.set("k", b"hello").unwrap();
        assert_eq!(ks.get("k").unwrap().as_deref(), Some(&b"hello"[..]));
    }

    #[test]
    fn delete_removes() {
        let ks = MemKeyStore::default();
        ks.set("k", b"x").unwrap();
        ks.delete("k").unwrap();
        assert_eq!(ks.get("k").unwrap(), None);
    }

    #[test]
    fn missing_returns_none() {
        let ks = MemKeyStore::default();
        assert_eq!(ks.get("nope").unwrap(), None);
    }
}
