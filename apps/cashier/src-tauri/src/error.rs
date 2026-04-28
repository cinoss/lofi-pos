use crate::acl::role::Role;
use serde::Serialize;
use thiserror::Error;

/// All cashier-side errors.
///
/// **IPC envelope** (Tauri/HTTP): serializes as `{"code": "<snake_case>", "message": "..."}`
/// for variants with payload, or just `{"code": "<snake_case>"}` for unit variants.
///
/// Examples:
/// - `AppError::Validation("bad input".into())` → `{"code":"validation","message":"bad input"}`
/// - `AppError::NotFound` → `{"code":"not_found"}`
/// - `AppError::Unauthorized` → `{"code":"unauthorized"}`
/// - `AppError::OverrideRequired(Role::Manager)` → `{"code":"override_required","message":"manager"}`
///
/// Frontend types should treat `message` as optional: `{ code: string; message?: string }`.
/// This contract is part of the public API; renaming variants or changing
/// `serialize_with` helpers is a breaking change.
#[derive(Debug, Error, Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum AppError {
    #[serde(serialize_with = "ser_string")]
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("keychain: {0}")]
    Keychain(String),
    #[serde(serialize_with = "ser_string")]
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
    #[error("validation: {0}")]
    Validation(String),
    #[error("config: {0}")]
    Config(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("override required: {0:?}")]
    OverrideRequired(Role),
}

fn ser_string<E: std::fmt::Display, S: serde::Serializer>(e: &E, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&e.to_string())
}

/// Convenience alias for `Result<T, AppError>`.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_envelope_unit_variant() {
        let s = serde_json::to_string(&AppError::NotFound).unwrap();
        assert_eq!(s, r#"{"code":"not_found"}"#);
        let s = serde_json::to_string(&AppError::Unauthorized).unwrap();
        assert_eq!(s, r#"{"code":"unauthorized"}"#);
    }

    #[test]
    fn ipc_envelope_data_variant() {
        let s = serde_json::to_string(&AppError::Validation("bad input".into())).unwrap();
        assert_eq!(s, r#"{"code":"validation","message":"bad input"}"#);
    }

    #[test]
    fn ipc_envelope_override_required() {
        let s = serde_json::to_string(&AppError::OverrideRequired(Role::Manager)).unwrap();
        assert_eq!(s, r#"{"code":"override_required","message":"manager"}"#);
    }
}
