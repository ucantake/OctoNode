//! Unified error type for the backend.
//!
//! Every fallible operation returns [`AppResult<T>`]. Errors are serialized to
//! the frontend as `{ kind, message }` so the UI can branch on `kind` (e.g.
//! show the unlock dialog on `vaultLocked`) without parsing strings.

use serde::{Serialize, Serializer};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// The OS secret store failed (D-Bus missing, Keychain denied, ...).
    #[error("secret store error: {0}")]
    SecretStore(String),

    /// The encrypted-file vault is in use but has not been unlocked yet.
    #[error("the secret vault is locked; unlock it with the master password")]
    VaultLocked,

    #[error("invalid master password")]
    InvalidPassword,

    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The working tree / index changed between rendering and acting on a diff.
    #[error("the diff is out of date, refresh and try again: {0}")]
    Stale(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// A hosting API (GitHub / GitLab) returned an error or was unreachable.
    #[error("{0}")]
    Remote(String),

    /// The user cancelled a long-running operation.
    #[error("operation cancelled")]
    Cancelled,

    /// An external process (`git`, `ssh`) failed.
    #[error("process error: {0}")]
    Process(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn not_found(entity: &'static str, id: impl ToString) -> Self {
        Self::NotFound {
            entity,
            id: id.to_string(),
        }
    }

    /// Stable machine-readable discriminator, mirrored in `src/types/ipc.ts`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Git(_) => "git",
            Self::Io(_) => "io",
            Self::Json(_) => "serialization",
            Self::SecretStore(_) => "secretStore",
            Self::VaultLocked => "vaultLocked",
            Self::InvalidPassword => "invalidPassword",
            Self::NotFound { .. } => "notFound",
            Self::InvalidInput(_) => "invalidInput",
            Self::Stale(_) => "stale",
            Self::Unsupported(_) => "unsupported",
            Self::Remote(_) => "remote",
            Self::Cancelled => "cancelled",
            Self::Process(_) => "process",
            Self::Internal(_) => "internal",
        }
    }
}

/// Poisoned locks only happen after a panic in another command; surface them
/// as an error instead of propagating the panic into every later call.
impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        Self::Internal("application state lock poisoned".into())
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        Self::Internal(format!("background task failed: {e}"))
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        Self::SecretStore(e.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}
