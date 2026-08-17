use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "orbit";
const LEGACY_SERVICE: &str = "openchat";
const LEGACY_USER: &str = "openrouter_api_key";
const USER: &str = "api_key";

fn service_for(provider: &str) -> String {
    format!("orbit:{provider}")
}

pub struct SecureStore;

/// Outcome of looking up the API key across the current and legacy keyring services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyLoadDecision {
    UseCurrent(String),
    MigrateFromLegacy(String),
    Missing,
}

/// Pure fallback decision: prefer the current service, then migrate from the legacy one.
pub(crate) fn decide_key_load(current: Option<String>, legacy: Option<String>) -> KeyLoadDecision {
    if let Some(key) = normalize_secret(current) {
        return KeyLoadDecision::UseCurrent(key);
    }
    if let Some(key) = normalize_secret(legacy) {
        return KeyLoadDecision::MigrateFromLegacy(key);
    }
    KeyLoadDecision::Missing
}

fn normalize_secret(value: Option<String>) -> Option<String> {
    value.and_then(|secret| {
        let trimmed = secret.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

impl SecureStore {
    pub const fn display_name() -> &'static str {
        #[cfg(windows)]
        {
            "Windows Credential Manager"
        }

        #[cfg(target_os = "linux")]
        {
            "your Linux Secret Service (such as GNOME Keyring or KWallet)"
        }
    }

    fn entry_for(service: &str, user: &str) -> Result<Entry> {
        Entry::new(service, user).context("Failed to open system credential-store entry")
    }

    fn read_service(service: &str, user: &str) -> Result<Option<String>> {
        match Self::entry_for(service, user)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("Failed to read API key from system credential store"),
        }
    }

    fn delete_service(service: &str, user: &str) -> Result<()> {
        match Self::entry_for(service, user)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("Failed to delete API key from system credential store"),
        }
    }

    /// OpenRouter key. Migrates `orbit`/`openrouter_api_key` and the older
    /// `openchat` service to `orbit:openrouter` on first load.
    pub fn load_key() -> Result<Option<String>> {
        Self::load_key_for(crate::providers::OPENROUTER)
    }

    pub fn load_key_for(provider: &str) -> Result<Option<String>> {
        let keyed = Self::read_service(&service_for(provider), USER)?;
        if let Some(key) = normalize_secret(keyed) {
            return Ok(Some(key));
        }
        if provider != crate::providers::OPENROUTER {
            return Ok(None);
        }
        let unscoped = Self::read_service(SERVICE, LEGACY_USER)?;
        let openchat = match &unscoped {
            Some(secret) if !secret.trim().is_empty() => None,
            _ => Self::read_service(LEGACY_SERVICE, LEGACY_USER)?,
        };
        match decide_key_load(unscoped, openchat) {
            KeyLoadDecision::UseCurrent(key) | KeyLoadDecision::MigrateFromLegacy(key) => {
                Self::save_key_for(provider, &key)?;
                let _ = Self::delete_service(SERVICE, LEGACY_USER);
                let _ = Self::delete_service(LEGACY_SERVICE, LEGACY_USER);
                tracing::info!("migrated OpenRouter API key to '{}'", service_for(provider));
                Ok(Some(key))
            }
            KeyLoadDecision::Missing => Ok(None),
        }
    }

    pub fn save_key(key: &str) -> Result<()> {
        Self::save_key_for(crate::providers::OPENROUTER, key)
    }

    pub fn save_key_for(provider: &str, key: &str) -> Result<()> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            anyhow::bail!("API key is empty.");
        }
        Self::entry_for(&service_for(provider), USER)?
            .set_password(trimmed)
            .context("Failed to write API key to system credential store")
    }

    pub fn delete_key() -> anyhow::Result<()> {
        Self::delete_key_for(crate::providers::OPENROUTER)
    }

    pub fn delete_key_for(provider: &str) -> anyhow::Result<()> {
        Self::delete_service(&service_for(provider), USER)?;
        if provider == crate::providers::OPENROUTER {
            let _ = Self::delete_service(SERVICE, LEGACY_USER);
            let _ = Self::delete_service(LEGACY_SERVICE, LEGACY_USER);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyLoadDecision, decide_key_load};

    #[test]
    fn prefers_current_service_when_both_exist() {
        let decision = decide_key_load(
            Some("sk-or-v1-current".into()),
            Some("sk-or-v1-legacy".into()),
        );
        assert_eq!(
            decision,
            KeyLoadDecision::UseCurrent("sk-or-v1-current".into())
        );
    }

    #[test]
    fn migrates_from_legacy_when_current_is_missing() {
        let decision = decide_key_load(None, Some("sk-or-v1-legacy".into()));
        assert_eq!(
            decision,
            KeyLoadDecision::MigrateFromLegacy("sk-or-v1-legacy".into())
        );
    }

    #[test]
    fn treats_empty_current_as_missing_and_falls_back() {
        let decision = decide_key_load(Some("   ".into()), Some("sk-or-v1-legacy".into()));
        assert_eq!(
            decision,
            KeyLoadDecision::MigrateFromLegacy("sk-or-v1-legacy".into())
        );
    }

    #[test]
    fn returns_missing_when_neither_service_has_a_key() {
        assert_eq!(decide_key_load(None, None), KeyLoadDecision::Missing);
        assert_eq!(
            decide_key_load(Some(String::new()), Some("  ".into())),
            KeyLoadDecision::Missing
        );
    }
}
