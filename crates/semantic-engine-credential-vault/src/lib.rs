use std::fmt;

use keyring::{Entry, Error as KeyringError};
use zeroize::Zeroize;

pub const DEFAULT_SERVICE_NAME: &str = "semantic-engine";
pub const MAX_SECRET_BYTES: usize = 2_048;

const MAX_CREDENTIAL_ID_CHARS: usize = 128;

pub trait CredentialVault: Send + Sync {
    fn store(&self, credential_id: &str, secret: &[u8]) -> Result<(), VaultError>;
    fn load(&self, credential_id: &str) -> Result<SecretValue, VaultError>;
    fn delete(&self, credential_id: &str) -> Result<(), VaultError>;
}

#[derive(Clone, Debug)]
pub struct OsCredentialVault {
    service_name: String,
}

impl OsCredentialVault {
    pub fn new(service_name: impl Into<String>) -> Result<Self, VaultError> {
        let service_name = service_name.into();
        validate_service_name(&service_name)?;
        Entry::store_status().as_ref().map_err(map_keyring_error_ref)?;
        Ok(Self { service_name })
    }

    pub fn semantic_engine() -> Result<Self, VaultError> {
        Self::new(DEFAULT_SERVICE_NAME)
    }

    fn entry(&self, credential_id: &str) -> Result<Entry, VaultError> {
        validate_credential_id(credential_id)?;
        Entry::new(&self.service_name, credential_id).map_err(map_keyring_error)
    }
}

impl CredentialVault for OsCredentialVault {
    fn store(&self, credential_id: &str, secret: &[u8]) -> Result<(), VaultError> {
        validate_secret(secret)?;
        self.entry(credential_id)?.set_secret(secret).map_err(map_keyring_error)
    }

    fn load(&self, credential_id: &str) -> Result<SecretValue, VaultError> {
        let secret = self.entry(credential_id)?.get_secret().map_err(map_keyring_error)?;
        validate_secret(&secret)?;
        SecretValue::new(secret)
    }

    fn delete(&self, credential_id: &str) -> Result<(), VaultError> {
        match self.entry(credential_id)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self, VaultError> {
        validate_secret(&bytes)?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultError {
    Invalid(&'static str),
    Missing,
    Unavailable(String),
    Access(String),
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "invalid credential: {reason}"),
            Self::Missing => write!(formatter, "credential does not exist"),
            Self::Unavailable(message) => {
                write!(formatter, "OS credential vault unavailable: {message}")
            }
            Self::Access(message) => {
                write!(formatter, "OS credential vault access failed: {message}")
            }
        }
    }
}

impl std::error::Error for VaultError {}

fn validate_service_name(value: &str) -> Result<(), VaultError> {
    if value.is_empty()
        || value.chars().count() > MAX_CREDENTIAL_ID_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(VaultError::Invalid("service name is invalid"));
    }
    Ok(())
}

fn validate_credential_id(value: &str) -> Result<(), VaultError> {
    if value.is_empty()
        || value.chars().count() > MAX_CREDENTIAL_ID_CHARS
        || value.starts_with(['.', '-'])
        || value.ends_with(['.', '-'])
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(VaultError::Invalid("credential identifier is invalid"));
    }
    Ok(())
}

fn validate_secret(secret: &[u8]) -> Result<(), VaultError> {
    if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
        return Err(VaultError::Invalid("secret is empty or too large"));
    }
    Ok(())
}

fn map_keyring_error(error: KeyringError) -> VaultError {
    match error {
        KeyringError::NoEntry => VaultError::Missing,
        KeyringError::NoDefaultStore => VaultError::Unavailable("no native store".to_owned()),
        other => VaultError::Access(other.to_string()),
    }
}

fn map_keyring_error_ref(error: &KeyringError) -> VaultError {
    match error {
        KeyringError::NoEntry => VaultError::Missing,
        KeyringError::NoDefaultStore => VaultError::Unavailable("no native store".to_owned()),
        other => VaultError::Access(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_output_is_redacted() {
        let secret = SecretValue(b"private-refresh-token".to_vec());
        assert_eq!(format!("{secret:?}"), "SecretValue([REDACTED])");
        assert_eq!(secret.expose(), b"private-refresh-token");
    }

    #[test]
    fn identifiers_and_secret_sizes_are_bounded() {
        assert!(validate_credential_id("twitch-main").is_ok());
        assert!(validate_credential_id("../escape").is_err());
        assert!(validate_secret(&[]).is_err());
        assert!(validate_secret(&vec![0; MAX_SECRET_BYTES + 1]).is_err());
    }
}
