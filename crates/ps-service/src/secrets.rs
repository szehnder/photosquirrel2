//! API keys for hosted LLM providers (ADR-0031).
//!
//! Keys live in the OS keychain (macOS Keychain, Windows Credential Manager, Secret Service on Linux) through
//! `keyring`, under service [`SERVICE`] and the account name recorded in settings. Environment variables
//! (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`) override stored keys. When no keychain is
//! available, keys are kept in memory for this session only and [`StoredIn::Session`] says so, so the UI can
//! tell the user. Nothing here ever writes a key to a file, and the keychain is only touched when a key is
//! read or written (never at start-up).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ps_model::settings::ProviderId;

/// The keychain service name.
pub const SERVICE: &str = "io.github.szehnder.photosquirrel";

/// The override variable for a provider, if it has one.
pub fn env_var(provider: ProviderId) -> Option<&'static str> {
    match provider {
        ProviderId::OpenAi => Some("OPENAI_API_KEY"),
        ProviderId::Anthropic => Some("ANTHROPIC_API_KEY"),
        ProviderId::Gemini => Some("GEMINI_API_KEY"),
        ProviderId::LmStudio | ProviderId::OpenAiCompatible => None,
    }
}

/// The default keychain account for a provider.
pub fn default_account(provider: ProviderId) -> String {
    format!("{}-api-key", provider.id())
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No keychain service on this machine (or it refused access).
    #[error("no keychain is available: {0}")]
    Unavailable(String),
    #[error("keychain error: {0}")]
    Backend(String),
}

/// A credential store. [`Keychain`] in the app; [`MemoryBackend`] in tests.
pub trait SecretBackend: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// The OS keychain via `keyring`.
#[derive(Debug, Default)]
pub struct Keychain;

fn map_keyring(err: keyring::Error) -> SecretError {
    match err {
        keyring::Error::NoStorageAccess(e) => SecretError::Unavailable(e.to_string()),
        keyring::Error::PlatformFailure(e) => SecretError::Unavailable(e.to_string()),
        other => SecretError::Backend(other.to_string()),
    }
}

impl SecretBackend for Keychain {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(map_keyring)?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_keyring(err)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        keyring::Entry::new(SERVICE, account)
            .and_then(|e| e.set_password(secret))
            .map_err(map_keyring)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(map_keyring)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(map_keyring(err)),
        }
    }
}

/// An in-memory backend (tests; also models "keychain present").
#[derive(Debug, Default)]
pub struct MemoryBackend {
    map: Mutex<HashMap<String, String>>,
}

impl SecretBackend for MemoryBackend {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self.lock().get(account).cloned())
    }
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.lock().insert(account.to_owned(), secret.to_owned());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.lock().remove(account);
        Ok(())
    }
}

impl MemoryBackend {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// A backend that has no keychain (tests of the session fallback).
#[derive(Debug, Default)]
pub struct UnavailableBackend;

impl SecretBackend for UnavailableBackend {
    fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError::Unavailable("no keychain".into()))
    }
    fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable("no keychain".into()))
    }
    fn delete(&self, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable("no keychain".into()))
    }
}

/// Where a key was stored or found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredIn {
    Keychain,
    /// No keychain: kept in memory until the app quits.
    Session,
    /// From the provider's environment variable (read-only).
    Environment,
}

/// A key and where it came from. `Debug` never prints the key.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub value: String,
    pub from: StoredIn,
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKey")
            .field("value", &"<redacted>")
            .field("from", &self.from)
            .finish()
    }
}

/// API-key access with the env override and the session fallback.
#[derive(Clone)]
pub struct Secrets {
    backend: Arc<dyn SecretBackend>,
    session: Arc<Mutex<HashMap<String, String>>>,
    env: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl Default for Secrets {
    fn default() -> Self {
        Self::os()
    }
}

impl Secrets {
    /// The OS keychain and the real environment.
    pub fn os() -> Self {
        Self::with_backend(Arc::new(Keychain))
    }

    pub fn with_backend(backend: Arc<dyn SecretBackend>) -> Self {
        Self {
            backend,
            session: Arc::default(),
            env: Arc::new(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty())),
        }
    }

    /// Replace environment lookup (tests).
    pub fn with_env(
        mut self,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.env = Arc::new(env);
        self
    }

    /// The key for `provider` stored under `account`: environment variable first, then keychain, then session.
    pub fn api_key(&self, provider: ProviderId, account: &str) -> Option<ApiKey> {
        if let Some(value) = env_var(provider).and_then(|name| (self.env)(name)) {
            return Some(ApiKey {
                value,
                from: StoredIn::Environment,
            });
        }
        match self.backend.get(account) {
            Ok(Some(value)) => {
                return Some(ApiKey {
                    value,
                    from: StoredIn::Keychain,
                });
            }
            Ok(None) => {}
            Err(err) => log::debug!("keychain lookup failed: {err}"),
        }
        self.session_map().get(account).map(|value| ApiKey {
            value: value.clone(),
            from: StoredIn::Session,
        })
    }

    /// Store a key: in the keychain when there is one, otherwise for this session only.
    pub fn set_api_key(&self, account: &str, key: &str) -> Result<StoredIn, SecretError> {
        match self.backend.set(account, key) {
            Ok(()) => {
                self.session_map().remove(account);
                Ok(StoredIn::Keychain)
            }
            Err(SecretError::Unavailable(reason)) => {
                log::warn!("no keychain ({reason}); keeping the API key for this session only");
                self.session_map()
                    .insert(account.to_owned(), key.to_owned());
                Ok(StoredIn::Session)
            }
            Err(err) => Err(err),
        }
    }

    pub fn delete_api_key(&self, account: &str) -> Result<(), SecretError> {
        self.session_map().remove(account);
        match self.backend.delete(account) {
            Ok(()) | Err(SecretError::Unavailable(_)) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn session_map(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn env_overrides_keychain_and_session_is_the_fallback() {
        let secrets = Secrets::with_backend(Arc::new(MemoryBackend::default()))
            .with_env(|name| (name == "ANTHROPIC_API_KEY").then(|| "from-env".to_owned()));
        assert_eq!(
            secrets.set_api_key("openai-api-key", "sk-1").unwrap(),
            StoredIn::Keychain
        );
        let key = secrets
            .api_key(ProviderId::OpenAi, "openai-api-key")
            .unwrap();
        assert_eq!((key.value.as_str(), key.from), ("sk-1", StoredIn::Keychain));
        let env = secrets
            .api_key(ProviderId::Anthropic, "anthropic-api-key")
            .unwrap();
        assert_eq!(env.from, StoredIn::Environment);
        assert!(
            !format!("{env:?}").contains("from-env"),
            "Debug redacts the key"
        );

        let none = Secrets::with_backend(Arc::new(UnavailableBackend)).with_env(|_| None);
        assert_eq!(none.set_api_key("a", "sk-2").unwrap(), StoredIn::Session);
        assert_eq!(
            none.api_key(ProviderId::OpenAi, "a").unwrap().from,
            StoredIn::Session
        );
        none.delete_api_key("a").unwrap();
        assert!(none.api_key(ProviderId::OpenAi, "a").is_none());
    }
}
